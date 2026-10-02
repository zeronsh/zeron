/**
 * A fake Boat API for the workerd tier, installed as Miniflare's
 * `outboundService` (it runs in Node, in the vitest process). Everything the
 * Worker, the CloudDevice DO and the Workflows fetch from `https://boat.test`
 * lands here; any other host fails loudly.
 *
 * Lifecycle: create → `provisioning`, which turns `idle` on the next GET;
 * stop → `archiving` → `archived` on the next GET; resume → `provisioning`.
 * Usage is a deterministic function of the queried window (`rate` billable
 * seconds per wall second), with `running` mirroring the machine state.
 *
 * Test controls (plain HTTP from inside the tests):
 *   GET  https://boat.test/__state            sandboxes + call log
 *   POST https://boat.test/__fail             {sandboxId?, op, status, code, action?, times?}
 *   POST https://boat.test/__set              {sandboxId, rate?, state?, runningReads?}
 *   POST https://boat.test/__extra            {id, state, createdAt}: a sandbox nobody created via the API
 */
import { Response, type Request } from "miniflare";

interface FakeSandbox {
  id: string;
  state: string;
  type: string;
  createdAt: string;
  env: Record<string, string>;
  ttlSeconds: number | null;
  deleted: boolean;
  /** Billable seconds per wall-clock second of a usage window. */
  rate: number;
  /** Usage reads that still report `running: true` after a stop (meter lag). */
  runningReads: number;
}

interface Call {
  method: string;
  path: string;
  sandboxId?: string;
  query?: Record<string, string>;
  body?: unknown;
  headers: Record<string, string>;
}

interface Failure {
  sandboxId?: string;
  op: string;
  status: number;
  code: string;
  action?: string;
  times: number;
}

const ALPHABET = "23456789abcdefghjkmnpqrstuvwxyz";

export const createBoatFake = () => {
  const sandboxes = new Map<string, FakeSandbox>();
  const byKey = new Map<string, string>();
  const calls: Call[] = [];
  const failures: Failure[] = [];

  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  const error = (status: number, code: string, message = code, action?: string) =>
    reply(status, {
      ok: false,
      type: "sandbox.error",
      status,
      code,
      message,
      error: { code, message, status, ...(action ? { details: { action } } : {}) },
      requestId: "req_test"
    });
  const view = (s: FakeSandbox) => ({
    id: s.id,
    name: `Sandbox ${s.id}`,
    state: s.state,
    type: s.type,
    createdAt: s.createdAt,
    archiveAfter: null,
    desktopAvailable: false,
    snapshotAvailable: true
  });
  const injected = (op: string, sandboxId?: string) => {
    const i = failures.findIndex((f) => f.op === op && (!f.sandboxId || f.sandboxId === sandboxId));
    if (i < 0) return undefined;
    const f = failures[i]!;
    if (--f.times <= 0) failures.splice(i, 1);
    return error(f.status, f.code, `${f.code} (injected)`, f.action);
  };
  const newId = () => {
    let id = "bx_";
    for (let i = 0; i < 8; i++) id += ALPHABET[Math.floor(Math.random() * ALPHABET.length)];
    return id;
  };

  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    if (url.hostname !== "boat.test") return new Response(`unexpected outbound fetch ${url}`, { status: 599 });
    const text = request.method === "GET" || request.method === "HEAD" ? "" : await request.text();
    const body = text ? JSON.parse(text) : undefined;

    // ── test controls ──
    if (url.pathname === "/__state") {
      return reply(200, { sandboxes: [...sandboxes.values()], calls });
    }
    if (url.pathname === "/__fail") {
      failures.push({ times: 1, ...body });
      return reply(200, { ok: true });
    }
    if (url.pathname === "/__set") {
      const s = sandboxes.get(body.sandboxId);
      if (!s) return reply(404, { ok: false });
      Object.assign(s, body.rate === undefined ? {} : { rate: body.rate }, body.state === undefined ? {} : { state: body.state }, body.runningReads === undefined ? {} : { runningReads: body.runningReads });
      return reply(200, { ok: true });
    }
    if (url.pathname === "/__extra") {
      sandboxes.set(body.id, { id: body.id, state: body.state, type: "default", createdAt: body.createdAt, env: {}, ttlSeconds: null, deleted: false, rate: 1, runningReads: 0 });
      return reply(200, { ok: true });
    }

    // ── the API ──
    if (request.headers.get("authorization") !== "Bearer test-boat-key") return error(401, "unauthorized");
    const path = url.pathname.replace(/^\/api\/v1/, "");
    const m = /^\/sandboxes(?:\/(bx_[a-z0-9]+))?(?:\/([a-z]+))?$/.exec(path);
    if (!m) return error(404, "not_found");
    const [, id, action] = m;
    calls.push({
      method: request.method,
      path,
      ...(id ? { sandboxId: id } : {}),
      ...(url.search ? { query: Object.fromEntries(url.searchParams) } : {}),
      ...(body === undefined ? {} : { body }),
      headers: Object.fromEntries([...request.headers].filter(([k]) => k.startsWith("x-") || k === "idempotency-key"))
    });

    if (!id && request.method === "POST") {
      const failure = injected("create");
      if (failure) return failure;
      const key = request.headers.get("idempotency-key");
      if (key && byKey.has(key)) {
        const existing = sandboxes.get(byKey.get(key)!)!;
        if (JSON.stringify(existing.env) !== JSON.stringify(body.env ?? {})) return error(409, "idempotency_key_reused");
        return reply(202, { ok: true, type: "sandbox.created", status: existing.state, sandbox: view(existing) });
      }
      const sandbox: FakeSandbox = {
        id: newId(),
        state: "provisioning",
        type: body.type ?? "default",
        createdAt: new Date().toISOString(),
        env: body.env ?? {},
        ttlSeconds: body.ttlSeconds ?? 3600,
        deleted: false,
        rate: 1,
        runningReads: 0
      };
      sandboxes.set(sandbox.id, sandbox);
      if (key) byKey.set(key, sandbox.id);
      return reply(202, { ok: true, type: "sandbox.created", status: "provisioning", sandbox: view(sandbox) });
    }
    if (!id && request.method === "GET") {
      const all = [...sandboxes.values()].filter((s) => !s.deleted).map(view);
      return reply(200, { ok: true, type: "sandbox.list", sandboxes: all, pageInfo: { nextCursor: null, hasMore: false, limit: 200 } });
    }

    const sandbox = id ? sandboxes.get(id) : undefined;
    if (!sandbox || sandbox.deleted) return error(404, "not_found");
    const op = action ?? request.method.toLowerCase();
    const failure = injected(op, sandbox.id);
    if (failure) return failure;

    switch (`${request.method} ${action ?? ""}`) {
      case "GET ": {
        if (sandbox.state === "provisioning") sandbox.state = "idle";
        else if (sandbox.state === "archiving") sandbox.state = "archived";
        return reply(200, { ok: true, type: "sandbox.info", sandbox: view(sandbox) });
      }
      case "PATCH ":
        sandbox.ttlSeconds = body.ttlSeconds;
        return reply(200, { ok: true, type: "sandbox.info", sandbox: view(sandbox) });
      case "DELETE ":
        if (request.headers.get("x-ascii-confirm-delete") !== sandbox.id) return error(409, "confirm_required");
        sandbox.deleted = true;
        return reply(202, { ok: true, type: "sandbox.deleting", operation: { id: `bdop_${sandbox.id}`, status: "pending", stage: "removing" } });
      case "POST stop":
        if (sandbox.state !== "archived") sandbox.state = "archiving";
        return reply(202, { ok: true, type: "sandbox.stopping", status: "archiving", sandbox: view(sandbox) });
      case "POST resume":
        sandbox.state = "provisioning";
        if (body?.ttlSeconds !== undefined) sandbox.ttlSeconds = body.ttlSeconds;
        return reply(202, { ok: true, type: "sandbox.resuming", status: "resuming", sandbox: view(sandbox) });
      case "POST commands":
        return reply(200, { ok: true, type: "command.finished", success: true, exitCode: 0, stdout: "ok", stderr: "", timedOut: false });
      case "GET usage": {
        const since = Math.max(
          Date.parse(url.searchParams.get("since") ?? sandbox.createdAt),
          Date.parse(sandbox.createdAt)
        );
        const until = Math.min(Date.parse(url.searchParams.get("until") ?? new Date().toISOString()), Date.now());
        const seconds = Math.max(0, Math.floor(((until - since) / 1000) * sandbox.rate));
        let running = ["idle", "ready", "running", "provisioning"].includes(sandbox.state);
        if (!running && sandbox.runningReads > 0) {
          sandbox.runningReads--;
          running = true;
        }
        return reply(200, {
          ok: true,
          type: "sandbox.usage",
          sandboxId: sandbox.id,
          sandboxType: sandbox.type,
          billingMultiplier: 1,
          since: new Date(since).toISOString(),
          until: new Date(until).toISOString(),
          seconds,
          dollars: Math.round((seconds / 100000) * 1e6) / 1e6,
          secondsPerDollar: 100000,
          running
        });
      }
      default:
        return error(404, "not_found");
    }
  };
};
