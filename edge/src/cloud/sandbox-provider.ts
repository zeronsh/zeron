/**
 * The sandbox provider seam: everything the Cloud control plane (the
 * CloudAccount DO, the lifecycle Workflows, metering, billing) needs from
 * whoever hosts the Cloud device's machine, in provider-neutral terms.
 * Provider specifics — URLs, field names, state vocabularies, headers, error
 * codes, account rules — live ONLY in `providers/<name>.ts`.
 *
 * Every persisted sandbox reference carries `{provider, sandboxId}` (sandbox
 * history, ledger, usage rows, the billing index), so a user can hold
 * sandboxes on two providers during a migration and billing stays exact.
 *
 * ── Adding a sandbox provider ───────────────────────────────────────────────
 * Implement `SandboxProvider` in `providers/<name>.ts`, register it in
 * `providers/index.ts`, select it with the `SANDBOX_PROVIDER` var. A provider
 * MUST guarantee:
 *   1. Idempotent create: the same `idempotencyKey` (with the same request)
 *      returns the same sandbox instead of a second billable one.
 *   2. The disk persists across stop/resume (the engine's data dir, its
 *      runner key, the user's clones).
 *   3. Enabled systemd system units start again on resume (the engine runs
 *      as `zeron-cloud.service`; nothing re-runs the installer on wake).
 *   4. Per-sandbox `env` from create is visible to `exec` commands (the
 *      installer reads its enrollment from there).
 *   5. `exec` runs as an unprivileged user with passwordless sudo, on Linux
 *      x86_64 or aarch64, with curl + tar + systemd available.
 *   6. `usage` is readable until the sandbox is deleted, is exact for any
 *      window (stopped time never counts), and reports `running: false`
 *      once a stopped sandbox's meter has settled — the delete path reads
 *      the final figure then, because nothing is readable afterwards.
 *   7. Nothing of the provider account leaks into user sandboxes (Boat:
 *      every sandbox is created `noEnv`); only `env` reaches them.
 * Map every failure to a `SandboxError` kind; `config` errors should name
 * what the operator must fix.
 */

export type SandboxMachine = "small" | "default" | "large";

/** Normalized lifecycle state. `deleted` = the provider no longer knows it. */
export type SandboxState = "provisioning" | "ready" | "stopping" | "stopped" | "error" | "deleted";

export interface SandboxCapabilities {
  /** Auto-stop cannot be disabled (create/resume must pass a TTL). */
  readonly ttlRequired: boolean;
  /** Largest TTL the provider accepts; null = unbounded. */
  readonly maxTtlSeconds: number | null;
}

export interface CreateSandbox {
  /** Same key + same request = same sandbox. */
  readonly idempotencyKey: string;
  readonly env: Record<string, string>;
  readonly machine: SandboxMachine;
  /** Auto-stop after this long; null = never (subject to capabilities). */
  readonly ttlSeconds: number | null;
  /** Start from this named snapshot (a template with the engine
   * pre-installed and no identity) instead of a blank machine. */
  readonly from?: string;
}

export interface SandboxInfo {
  readonly state: SandboxState;
  readonly error?: string;
}

export interface ExecResult {
  readonly exitCode: number | null;
  readonly stdout: string;
  readonly stderr: string;
  readonly timedOut?: boolean;
}

export interface SandboxUsage {
  /** Billable seconds in the window (machine-size multiplier applied). */
  readonly seconds: number;
  /** Provider list price for `seconds`, USD. */
  readonly dollars: number;
  readonly machine: string;
  /** The meter is still moving. */
  readonly running: boolean;
}

export interface ListedSandbox {
  readonly sandboxId: string;
  readonly state: SandboxState;
  /** Unix ms, when known (young sandboxes are not orphans yet). */
  readonly createdAt?: number;
}

/** Normalized provider actions (for config errors: what the key lacks). */
export type SandboxAction =
  | "create"
  | "read"
  | "update"
  | "stop"
  | "resume"
  | "delete"
  | "exec"
  | "usage"
  | "list";

export interface SandboxProvider {
  /** Persisted with every sandbox reference. Never rename a shipped one. */
  readonly name: string;
  readonly capabilities: SandboxCapabilities;
  create(request: CreateSandbox): Promise<{ readonly sandboxId: string; readonly createdAt?: number }>;
  get(sandboxId: string): Promise<SandboxInfo>;
  /** Snapshot + stop. Never discards data: a provider that cannot save the
   * disk must refuse (a retryable/conflict error), not force. */
  stop(sandboxId: string): Promise<void>;
  resume(sandboxId: string, options: { readonly ttlSeconds: number | null }): Promise<void>;
  /** Irreversible. */
  delete(sandboxId: string): Promise<{ readonly operationId?: string }>;
  extendTtl(sandboxId: string, ttlSeconds: number | null): Promise<void>;
  exec(sandboxId: string, options: { readonly command: string; readonly timeoutSeconds: number }): Promise<ExecResult>;
  /** Unix-ms window `[since, until)`. */
  usage(sandboxId: string, window: { readonly since: number; readonly until: number }): Promise<SandboxUsage>;
  /** Every sandbox on the provider account (the orphan scan). */
  list(): AsyncIterable<ListedSandbox>;
}

export type SandboxErrorKind = "retryable" | "config" | "notFound" | "conflict" | "fatal";

export class SandboxError extends Error {
  constructor(
    readonly kind: SandboxErrorKind,
    /** Human detail, phrased by the provider ("the Boat API key lacks sandbox.resume"). */
    message: string,
    readonly providerCode: string,
    readonly provider: string,
    /** For `config`: the action the provider refused. */
    readonly action?: SandboxAction
  ) {
    super(message);
    this.name = "SandboxError";
  }
}

const VERBS: Record<SandboxAction, string> = {
  create: "create",
  read: "read",
  update: "update",
  stop: "stop",
  resume: "wake",
  delete: "delete",
  exec: "run commands in",
  usage: "meter",
  list: "list"
};

/**
 * The message a user (or operator) sees in `CloudStatus.error`. Config
 * problems name the fix; a vanished sandbox says how to recover; anything
 * else keeps the provider's detail.
 */
export const sandboxUserMessage = (error: unknown, doing: string): string => {
  if (!(error instanceof SandboxError)) {
    return `Couldn't ${doing}: ${error instanceof Error ? error.message : String(error)}`;
  }
  switch (error.kind) {
    case "config":
      return error.action
        ? `Cloud isn't configured to ${VERBS[error.action]} sandboxes (${error.message}).`
        : `Cloud is misconfigured (${error.message}).`;
    case "notFound":
      return "The Cloud sandbox no longer exists. Delete Cloud and enable it again.";
    default:
      return `Couldn't ${doing}: ${error.message}`;
  }
};

/**
 * How a Workflow step treats a failure: retry under the step's backoff, or
 * end the lifecycle now. Non-provider errors (a dropped DO call, a runtime
 * hiccup) are presumed transient.
 */
export const classifyFailure = (
  error: unknown,
  doing: string
): { readonly retryable: boolean; readonly message: string } => ({
  retryable: !(error instanceof SandboxError) || error.kind === "retryable",
  message: sandboxUserMessage(error, doing)
});

/** Stopping is idempotent and a provider refuses a stop while it cannot
 * save the disk, so anything but a hard "no" (config / gone) is retried. */
export const stopRetryable = (error: unknown): boolean =>
  !(error instanceof SandboxError && (error.kind === "config" || error.kind === "notFound"));

/** The TTL to request: the configured one, clamped to what the provider
 * accepts, or the provider's maximum when it insists on auto-stop. */
export const effectiveTtl = (capabilities: SandboxCapabilities, configured: number | null): number | null => {
  if (configured === null) return capabilities.ttlRequired ? capabilities.maxTtlSeconds : null;
  return capabilities.maxTtlSeconds === null ? configured : Math.min(configured, capabilities.maxTtlSeconds);
};
