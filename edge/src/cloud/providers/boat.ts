/**
 * Boat (https://docs.boat.dev) as a `SandboxProvider`. The ONLY module that
 * knows Boat's URLs, field names, state vocabulary, headers, error codes and
 * account rules; everything else speaks sandbox-provider.ts.
 *
 * Wire facts (verified against the live API):
 * - the lifecycle field is `sandbox.state` (action replies also carry a
 *   coarse top-level `status` like "archiving" — not the state);
 * - user sandboxes are ALWAYS created `noEnv: true` (Boat's platform rule:
 *   nothing of our Boat account reaches a user's machine) with what they
 *   need passed as per-sandbox `env`, which `exec` commands see;
 * - `Idempotency-Key` makes create retry-safe (same key + same body; a
 *   different body is 409 `idempotency_key_reused`);
 * - DELETE requires `X-Ascii-Confirm-Delete: {id}` and afterwards usage is
 *   gone (404), so the delete path reads the final figure first;
 * - commands are never retried by Boat and may still be running after a
 *   502, so only idempotent scripts go through `exec`;
 * - scoped API keys answer 403 `api_key_action_forbidden` with
 *   `error.details.action` naming the missing permission.
 */
import {
  SandboxError,
  type CreateSandbox,
  type ExecResult,
  type ListedSandbox,
  type SandboxAction,
  type SandboxErrorKind,
  type SandboxFailure,
  type SandboxInfo,
  type SandboxMachine,
  type SandboxProvider,
  type SandboxState,
  type SandboxUsage
} from "../sandbox-provider";

export type BoatFetch = (input: string, init?: RequestInit) => Promise<Response>;

export interface BoatConfig {
  readonly apiKey: string;
  /** Default `https://boat.dev/api/v1`. */
  readonly base?: string;
  readonly fetch?: BoatFetch;
}

const PROVIDER = "boat";
const DEFAULT_BASE = "https://boat.dev/api/v1";

/** Boat `sandbox.state` → normalized state. */
export const boatState = (state: string): SandboxState => {
  switch (state) {
    case "ready":
    case "idle":
    case "running":
      // `idle`/`running` only reflect Boat's own prompt queue — a sandbox
      // whose engine is busy reads `idle`. Both mean "machine is up".
      return "ready";
    case "archiving":
      return "stopping";
    case "archived":
      return "stopped";
    case "error":
    case "cancelled":
      return "error";
    default:
      // init · provisioning · provisioned · cloning, and anything Boat adds.
      return "provisioning";
  }
};

const ACTIONS: Record<string, SandboxAction> = {
  "sandbox.create": "create",
  "sandbox.read": "read",
  "sandbox.update": "update",
  "sandbox.stop": "stop",
  "sandbox.resume": "resume",
  "sandbox.delete": "delete",
  exec: "exec"
};

const CAPACITY = new Set(["limit_reached", "member_limit_reached", "daily_limit_reached"]);
const CONFIG = new Set([
  "unauthorized",
  "forbidden",
  "api_key_action_forbidden",
  "trial_auto_stop_required",
  "trial_machine_class_not_allowed",
  "billing_required",
  "org_suspended"
]);

/** Boat HTTP status + code → normalized kind. */
export const boatErrorKind = (status: number, code: string): SandboxErrorKind => {
  if (status === 0 || status === 408 || status === 425 || status >= 500) return "retryable";
  if (status === 409 && (code === "idempotency_in_progress" || code === "boat_starting")) return "retryable";
  // A command sent while a machine starts: held for up to a minute, then this.
  if (status === 409 && code === "sandbox_starting") return "retryable";
  if (status === 400 && code === "machine_not_running") return "retryable";
  if (status === 429 && code === "rate_limited") return "retryable";
  if (status === 404) return "notFound";
  if (status === 401 || status === 402 || status === 403 || CONFIG.has(code)) return "config";
  if (status === 409) return "conflict";
  return "fatal";
};

/** "This Sandbox needs 118.3 GiB of disk but the largest 'large' machine holds
 * 111.8 GiB. Resume or fork it as a bigger type (--type)." */
const DISK_NEEDED_RE = /needs\s+(\d+(?:\.\d+)?)\s*GiB\s+of\s+disk/i;
/** Its other spelling, a 400 `type_too_small` on a resume to a smaller type, in
 * decimal GB of everything on the disk, base image included: "This sandbox uses
 * 77.7 GB of disk, which does not fit a 'default' machine (76.0 GB)." */
const DISK_USES_RE = /uses\s+(\d+(?:\.\d+)?)\s*GB\s+of\s+disk/i;
/** healthReason/error on an expired sandbox: "This sandbox's data was deleted
 * 14 days after it stopped. …" */
const DATA_DELETED_RE = /data was deleted/i;

/**
 * A Boat error (or a sandbox's own `error`/`healthReason`) as a failure a
 * retry can't fix, or undefined. `type_too_small` is the documented code for a
 * refused shrink; the message with the figure is what Boat answers on a resume
 * that doesn't fit, whatever its code.
 */
export const boatFailure = (code: string | undefined, message: string | null | undefined): SandboxFailure | undefined => {
  if (code === "snapshot_data_expired" || (message && DATA_DELETED_RE.test(message))) return { kind: "data_expired" };
  const needed = message ? DISK_NEEDED_RE.exec(message) : null;
  if (needed) {
    const neededGiB = Number(needed[1]);
    return Number.isFinite(neededGiB) ? { kind: "disk_too_small", neededGiB } : { kind: "disk_too_small" };
  }
  const uses = message ? DISK_USES_RE.exec(message) : null;
  if (uses) {
    const neededGiB = Math.round(((Number(uses[1]) * 1e9) / 2 ** 30 - BOAT_IMAGE_GIB) * 10) / 10;
    return Number.isFinite(neededGiB) && neededGiB > 0 ? { kind: "disk_too_small", neededGiB } : { kind: "disk_too_small" };
  }
  return code === "type_too_small" ? { kind: "disk_too_small" } : undefined;
};

const boatErrorMessage = (code: string, message: string, action?: string): string => {
  if (code === "api_key_action_forbidden") return `the Boat API key lacks ${action ?? "a required action"}`;
  if (code === "unauthorized") return "the Boat API key was rejected";
  if (code === "trial_auto_stop_required") return "this Boat account requires auto-stop; set CLOUD_TTL_SECONDS (max 7200 on trial)";
  if (code === "billing_required" || code === "org_suspended") return `Boat billing: ${message}`;
  if (CAPACITY.has(code)) return `Boat is at capacity (${code}); try again later`;
  return message;
};

interface Envelope {
  code?: unknown;
  message?: unknown;
  retryable?: unknown;
  error?: { code?: unknown; message?: unknown; retryable?: unknown; details?: { action?: unknown } };
}

interface BoatSandbox {
  id: string;
  state: string;
  type?: string;
  error?: string | null;
  createdAt?: string | null;
  /** When Boat deleted its data; null while the data is kept. */
  dataExpiredAt?: string | null;
  /** When it last stopped, by any cause; null while it isn't stopped. */
  stoppedAt?: string | null;
  healthReason?: string | null;
  /** True once every file of its snapshot is on its disk. */
  hydrated?: boolean | null;
}

/** Boat's published rule: a stopped sandbox's disk is kept for 14 days, and "a
 * resume starts the 14 days again from the next stop" (docs.boat.dev). */
const STOPPED_RETENTION_MS = 14 * 24 * 60 * 60_000;

/** Boat's refusal counts what the machine added, not its base image, which
 * `df` does. Measured on a hydrated machine: `df` 72.4 GiB, refused at
 * "needs 51.8 GiB". A fresh machine, with Zeron and a small checkout, reads
 * 20.7. */
export const BOAT_IMAGE_GIB = 20.5;

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const parseTime = (value: string | null | undefined): number | undefined => {
  const t = Date.parse(value ?? "");
  return Number.isFinite(t) ? t : undefined;
};

export class BoatProvider implements SandboxProvider {
  readonly name = PROVIDER;
  // Boat's own cap; a trial account additionally requires auto-stop ≤ 7200 s
  // (CLOUD_TTL_SECONDS), which Boat reports as `trial_auto_stop_required`.
  readonly capabilities = {
    ttlRequired: false,
    maxTtlSeconds: 2_592_000,
    stoppedRetentionMs: STOPPED_RETENTION_MS,
    diskOverheadGiB: BOAT_IMAGE_GIB
  };
  private readonly base: string;
  private readonly apiKey: string;
  private readonly doFetch: BoatFetch;

  constructor(config: BoatConfig) {
    this.apiKey = config.apiKey;
    this.base = (config.base ?? DEFAULT_BASE).replace(/\/+$/, "");
    // Never call the global `fetch` as a method of this object: workerd
    // rejects a foreign `this` ("Illegal invocation").
    this.doFetch = config.fetch ?? ((input, init) => fetch(input, init));
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    headers: Record<string, string> = {}
  ): Promise<T> {
    let response: Response;
    try {
      response = await this.doFetch(`${this.base}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${this.apiKey}`,
          accept: "application/json",
          ...(body === undefined ? {} : { "content-type": "application/json" }),
          ...headers
        },
        body: body === undefined ? undefined : JSON.stringify(body)
      });
    } catch (e) {
      throw new SandboxError("retryable", `Boat unreachable: ${String(e)}`, "network", PROVIDER);
    }
    const text = await response.text();
    let parsed: unknown;
    try {
      parsed = text ? JSON.parse(text) : {};
    } catch {
      parsed = undefined;
    }
    if (!response.ok || (parsed as { ok?: unknown } | undefined)?.ok === false) {
      const envelope = (parsed ?? {}) as Envelope;
      const status = response.ok ? 502 : response.status;
      const code = str(envelope.code) ?? str(envelope.error?.code) ?? `http_${status}`;
      const message = str(envelope.message) ?? str(envelope.error?.message) ?? (text.slice(0, 200) || `HTTP ${status}`);
      const action = str(envelope.error?.details?.action);
      const hinted = envelope.retryable ?? envelope.error?.retryable;
      let kind = boatErrorKind(status, code);
      // Boat's explicit hint wins for the transient/terminal split.
      if (hinted === true) kind = "retryable";
      else if (hinted === false && kind === "retryable") kind = "conflict";
      // Never retried, whatever else the reply hints: the same request gets
      // the same answer.
      const failure = boatFailure(code, message);
      if (failure) kind = "conflict";
      throw new SandboxError(kind, boatErrorMessage(code, message, action), code, PROVIDER, action ? ACTIONS[action] : undefined, failure);
    }
    if (parsed === undefined) {
      throw new SandboxError("retryable", "Boat returned a non-JSON reply", "invalid_json_response", PROVIDER);
    }
    return parsed as T;
  }

  private static sandboxOf(reply: { sandbox?: BoatSandbox }): BoatSandbox {
    if (!reply.sandbox || typeof reply.sandbox.id !== "string") {
      throw new SandboxError("retryable", "Boat reply had no sandbox", "invalid_json_response", PROVIDER);
    }
    return reply.sandbox;
  }

  private path(id: string, suffix = ""): string {
    return `/sandboxes/${encodeURIComponent(id)}${suffix}`;
  }

  async create(request: CreateSandbox): Promise<{ sandboxId: string; createdAt?: number }> {
    const sandbox = BoatProvider.sandboxOf(
      await this.request(
        "POST",
        "/sandboxes",
        {
          type: request.machine,
          ttlSeconds: request.ttlSeconds,
          noEnv: true,
          env: request.env,
          // A named snapshot; its own env/noEnv never apply because ours are explicit.
          ...(request.from ? { from: request.from } : {})
        },
        { "idempotency-key": request.idempotencyKey }
      )
    );
    const createdAt = parseTime(sandbox.createdAt);
    return createdAt === undefined ? { sandboxId: sandbox.id } : { sandboxId: sandbox.id, createdAt };
  }

  async get(sandboxId: string): Promise<SandboxInfo> {
    try {
      const sandbox = BoatProvider.sandboxOf(await this.request("GET", this.path(sandboxId)));
      const state = boatState(sandbox.state);
      const failure: SandboxFailure | undefined = sandbox.dataExpiredAt
        ? { kind: "data_expired" }
        : (boatFailure(undefined, sandbox.error) ?? boatFailure(undefined, sandbox.healthReason));
      const stoppedAt = parseTime(sandbox.stoppedAt);
      return {
        state,
        ...(sandbox.error && state === "error" ? { error: sandbox.error } : {}),
        ...(failure ? { failure } : {}),
        ...(stoppedAt === undefined ? {} : { stoppedAt }),
        ...(typeof sandbox.hydrated === "boolean" ? { hydrated: sandbox.hydrated } : {})
      };
    } catch (e) {
      if (e instanceof SandboxError && e.kind === "notFound") return { state: "deleted" as const };
      throw e;
    }
  }

  /** Never `force`: a forced stop discards everything since the last good
   * snapshot. Boat refuses a stop while snapshots fail (billing nothing). */
  async stop(sandboxId: string): Promise<void> {
    await this.request("POST", this.path(sandboxId, "/stop"), {});
  }

  async resume(sandboxId: string, options: { ttlSeconds: number | null; machine?: SandboxMachine }): Promise<void> {
    try {
      await this.request("POST", this.path(sandboxId, "/resume"), {
        ...(options.machine ? { type: options.machine } : {}),
        ttlSeconds: options.ttlSeconds
      });
    } catch (e) {
      // Already on its way up (a retried step whose first reply was lost).
      if (!(e instanceof SandboxError && e.providerCode === "sandbox_resuming")) throw e;
    }
  }

  async delete(sandboxId: string): Promise<{ operationId?: string }> {
    const reply = await this.request<{ operation?: { id?: unknown }; operationId?: unknown }>(
      "DELETE",
      this.path(sandboxId),
      undefined,
      { "x-ascii-confirm-delete": sandboxId }
    );
    const operationId = str(reply.operation?.id) ?? str(reply.operationId);
    return operationId ? { operationId } : {};
  }

  async extendTtl(sandboxId: string, ttlSeconds: number | null): Promise<void> {
    await this.request("PATCH", this.path(sandboxId), { ttlSeconds });
  }

  async exec(sandboxId: string, options: { command: string; timeoutSeconds: number }): Promise<ExecResult> {
    const reply = await this.request<{ exitCode?: unknown; stdout?: unknown; stderr?: unknown; timedOut?: unknown }>(
      "POST",
      this.path(sandboxId, "/commands"),
      { command: options.command, timeoutSeconds: options.timeoutSeconds }
    );
    return {
      exitCode: typeof reply.exitCode === "number" ? reply.exitCode : null,
      stdout: str(reply.stdout) ?? "",
      stderr: str(reply.stderr) ?? "",
      timedOut: reply.timedOut === true
    };
  }

  async usage(sandboxId: string, window: { since: number; until: number }): Promise<SandboxUsage> {
    const query = new URLSearchParams({
      since: new Date(window.since).toISOString(),
      until: new Date(window.until).toISOString()
    });
    const reply = await this.request<{ seconds?: unknown; dollars?: unknown; sandboxType?: unknown; running?: unknown }>(
      "GET",
      this.path(sandboxId, `/usage?${query}`)
    );
    if (typeof reply.seconds !== "number" || typeof reply.dollars !== "number") {
      throw new SandboxError("retryable", "Boat usage reply had no figures", "invalid_json_response", PROVIDER);
    }
    return {
      seconds: reply.seconds,
      dollars: reply.dollars,
      machine: str(reply.sandboxType) ?? "default",
      running: reply.running === true
    };
  }

  async *list(): AsyncIterable<ListedSandbox> {
    let cursor: string | undefined;
    for (let page = 0; page < 50; page++) {
      const query = new URLSearchParams({ limit: "200" });
      if (cursor) query.set("cursor", cursor);
      const reply = await this.request<{
        sandboxes?: BoatSandbox[];
        pageInfo?: { nextCursor?: string | null; hasMore?: boolean };
      }>("GET", `/sandboxes?${query}`);
      for (const sandbox of reply.sandboxes ?? []) {
        const createdAt = parseTime(sandbox.createdAt);
        yield { sandboxId: sandbox.id, state: boatState(sandbox.state), ...(createdAt === undefined ? {} : { createdAt }) };
      }
      const next = reply.pageInfo?.nextCursor;
      if (!next || reply.pageInfo?.hasMore === false) return;
      cursor = next;
    }
  }
}
