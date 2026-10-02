/**
 * HTTP surface of the Cloud control plane (docs/design/cloud-device.md,
 * "HTTP contract"):
 *
 *   GET    /cloud/{orgId}                          account status (user)
 *   POST   /cloud/{orgId}/enable                   turn Cloud on (user)
 *   DELETE /cloud/{orgId}                          turn Cloud off: every session machine (user)
 *   GET    /cloud/{orgId}/usage?month=             metered usage (user)
 *   GET    /cloud/{orgId}/projects                 Cloud projects (user)
 *   PUT    /cloud/{orgId}/projects/{spaceId}       add/update a project (user)
 *   DELETE /cloud/{orgId}/projects/{spaceId}       remove a project + its sessions (user)
 *   GET    /cloud/{orgId}/sessions                 sessions (user)
 *   POST   /cloud/{orgId}/sessions                 {chatId, spaceId}: a machine for a new chat (user)
 *   POST   /cloud/{orgId}/sessions/{chatId}/wake|sleep|retry (user)
 *   DELETE /cloud/{orgId}/sessions/{chatId}        delete that session's machine (user)
 *   POST   /runner/enroll                  pre-bearer: one-time code → key
 *   POST   /runner/token                   pre-bearer: signed ts → runner JWT
 *   POST   /runner/heartbeat               runner bearer
 *
 * Every route lands on the caller's OWN CloudAccount DO (`cloud1/{org}/{user}`,
 * org/user from the verified bearer — or, for the two pre-bearer runner
 * routes, from the body, where the DO's key/code check is the authentication).
 * Runner bearers never reach `/cloud/*` (runner-policy.ts).
 */
import type { Verified } from "../auth";
import type { Env } from "../env";
import { json, jsonError, readJsonBody } from "../http";
import type { CloudResult, RepoInput } from "./cloud-account";
import { isEdgeOrigin } from "./install-script";
import { isMonthKey, monthKey } from "./metering";
import { CLOUD_DEVICE_PREFIX, cloudAccountName } from "./policy";
import { sandboxProvider, type ProviderEnv } from "./providers";

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const CLOUD_ID_RE = /^cloud-[A-Za-z0-9-]{1,100}$/;
const B64URL_RE = /^[A-Za-z0-9_-]{1,256}$/;
/** Runner bodies are a few ids, a code/key and a signature. */
const MAX_RUNNER_BODY = 4096;
const MAX_COUNT = 100_000;

type CloudEnv = Pick<
  Env,
  | "CLOUD_ACCOUNTS"
  | "CLOUD_PROVISION"
  | "CLOUD_WAKE"
  | "CLOUD_SLEEP"
  | "CLOUD_DELETE"
  | "RUNNER_JWT_PRIVATE_KEY"
  | "AUTH_MODE"
  | "CLOUD_EDGE_URL"
> &
  ProviderEnv;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const unavailable = () => jsonError(503, "unavailable", "Cloud is not available on this server.");

const reply = <T>(result: CloudResult<T>): Response =>
  result.ok ? json(result.value) : jsonError(result.status, result.error, result.message);

const accountStub = (ns: NonNullable<Env["CLOUD_ACCOUNTS"]>, orgId: string, userId: string) =>
  ns.get(ns.idFromName(cloudAccountName(orgId, userId)));

/** Everything a Cloud lifecycle needs on this deployment. */
export const cloudAvailable = (env: CloudEnv): boolean =>
  Boolean(
    env.CLOUD_ACCOUNTS &&
      env.CLOUD_PROVISION &&
      env.CLOUD_WAKE &&
      env.CLOUD_SLEEP &&
      env.CLOUD_DELETE &&
      sandboxProvider(env) &&
      (env.RUNNER_JWT_PRIVATE_KEY || env.AUTH_MODE === "dev")
  );

const rpc = async <T>(run: () => Promise<CloudResult<T>>): Promise<Response> => {
  try {
    return reply(await run());
  } catch (e) {
    console.error("cloud rpc failed", String(e));
    return jsonError(503, "unavailable", "Cloud is temporarily unavailable; try again.");
  }
};

/**
 * `/runner/enroll` and `/runner/token` — routed BEFORE the bearer gate: the
 * engine has no bearer yet; possession of the enrollment code (or the
 * enrolled key's signature) is what authenticates it.
 */
export const handleRunnerPublicRoute = async (
  request: Request,
  env: CloudEnv,
  url: URL
): Promise<Response | undefined> => {
  const kind = url.pathname === "/runner/enroll" ? "enroll" : url.pathname === "/runner/token" ? "token" : undefined;
  if (!kind) return undefined;
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const ns = env.CLOUD_ACCOUNTS;
  if (!ns) return unavailable();
  const body = await readJsonBody(request, MAX_RUNNER_BODY);
  if (!isObject(body)) return jsonError(400, "bad_request", "Body must be a JSON object.");
  const { orgId, userId, deviceId } = body;
  if (typeof orgId !== "string" || !ID_RE.test(orgId)) return jsonError(400, "bad_request", "Bad orgId.");
  if (typeof userId !== "string" || !ID_RE.test(userId)) return jsonError(400, "bad_request", "Bad userId.");
  if (typeof deviceId !== "string" || !CLOUD_ID_RE.test(deviceId)) {
    return jsonError(400, "bad_request", "Bad deviceId.");
  }
  const stub = accountStub(ns, orgId, userId);
  if (kind === "enroll") {
    const { code, publicKey } = body;
    if (typeof code !== "string" || !B64URL_RE.test(code)) return jsonError(400, "bad_request", "Bad code.");
    if (typeof publicKey !== "string" || !B64URL_RE.test(publicKey)) {
      return jsonError(400, "bad_request", "Bad publicKey.");
    }
    return rpc(() => stub.enroll({ orgId, userId, deviceId, code, publicKey }));
  }
  const { ts, sig } = body;
  if (typeof ts !== "number" || !Number.isSafeInteger(ts)) return jsonError(400, "bad_request", "Bad ts.");
  if (typeof sig !== "string" || !B64URL_RE.test(sig)) return jsonError(400, "bad_request", "Bad sig.");
  return rpc(() => stub.token({ orgId, userId, deviceId, ts, sig }));
};

const count = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_COUNT ? value : undefined;

/** `/cloud/*` (user bearer) and `/runner/heartbeat` (runner bearer). */
export const handleCloudRoute = async (
  request: Request,
  env: CloudEnv,
  auth: Verified,
  url: URL
): Promise<Response | undefined> => {
  if (url.pathname === "/runner/heartbeat") {
    if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
    if (auth.kind !== "runner" || !auth.deviceId || !auth.orgId) {
      return jsonError(403, "forbidden", "Runner token required.");
    }
    const ns = env.CLOUD_ACCOUNTS;
    if (!ns) return unavailable();
    const body = await readJsonBody(request, MAX_RUNNER_BODY);
    if (!isObject(body)) return jsonError(400, "bad_request", "Body must be a JSON object.");
    const activeRuns = count(body.activeRuns);
    const clients = count(body.clients);
    if (activeRuns === undefined || clients === undefined) {
      return jsonError(400, "bad_request", "Expected {activeRuns, clients}.");
    }
    const runner = { orgId: auth.orgId, userId: auth.userId, deviceId: auth.deviceId };
    return rpc(() => accountStub(ns, runner.orgId, runner.userId).heartbeat(runner, { activeRuns, clients }));
  }

  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "cloud") return undefined;
  const orgId = parts[1];
  if (!orgId || !ID_RE.test(orgId)) return jsonError(404, "not_found", "Not found.");
  if (auth.orgId !== orgId) return jsonError(403, "forbidden", "Wrong organization.");
  const caller = { orgId, userId: auth.userId };
  const ns = env.CLOUD_ACCOUNTS;
  const rest = parts.slice(2);

  if (rest.length === 0 && request.method === "GET") {
    // An unconfigured deployment still answers status, so the UI can hide
    // the enable button instead of erroring.
    if (!ns) return json({ state: "off", awakeSessions: 0, maxAwakeSessions: 0, available: false });
    const available = cloudAvailable(env);
    return rpc(async () => {
      const status = await accountStub(ns, orgId, auth.userId).status(caller);
      return status.ok && !available && status.value.state === "off"
        ? { ok: true, value: { ...status.value, available: false } }
        : status;
    });
  }
  if (rest[0] === "usage" && rest.length === 1 && request.method === "GET") {
    const month = url.searchParams.get("month") ?? monthKey(Date.now());
    if (!isMonthKey(month)) return jsonError(400, "bad_request", "month must be YYYY-MM.");
    if (!ns) return json({ month, seconds: 0, dollars: 0, sandboxes: [], closed: false, available: false });
    return rpc(() => accountStub(ns, orgId, auth.userId).usage(caller, month));
  }
  if (!ns) return unavailable();
  const stub = accountStub(ns, orgId, auth.userId);
  // Session engines talk to the same origin the user's client used (or an
  // explicit override for dev tunnels).
  const edgeUrl = env.CLOUD_EDGE_URL ?? url.origin;

  if (rest.length === 0 && request.method === "DELETE") return rpc(() => stub.destroy(caller));
  if (rest.length === 1 && rest[0] === "enable" && request.method === "POST") {
    if (!cloudAvailable(env)) return unavailable();
    return rpc(() => stub.enable(caller));
  }

  if (rest[0] === "sessions") {
    if (rest.length === 1 && request.method === "GET") return rpc(async () => stub.listSessions(caller));
    if (rest.length === 1 && request.method === "POST") {
      if (!cloudAvailable(env)) return unavailable();
      if (!isEdgeOrigin(edgeUrl)) return jsonError(500, "misconfigured", "Bad CLOUD_EDGE_URL.");
      const body = await readJsonBody(request, MAX_RUNNER_BODY);
      if (!isObject(body)) return jsonError(400, "bad_request", "Body must be a JSON object.");
      const { chatId, spaceId, repo, branch } = body;
      if (typeof chatId !== "string" || !ID_RE.test(chatId)) return jsonError(400, "bad_request", "Bad chatId.");
      if (typeof spaceId !== "string" || !ID_RE.test(spaceId)) return jsonError(400, "bad_request", "Bad spaceId.");
      // The repository (the project's GitHub origin) and the branch the
      // session starts from; the DO validates both.
      if (!isObject(repo)) return jsonError(400, "bad_request", "Expected repo {fullName, cloneUrl, defaultBranch}.");
      if (branch !== undefined && (typeof branch !== "string" || branch.length > 255)) {
        return jsonError(400, "bad_request", "Bad branch.");
      }
      return rpc(() => stub.createSession(caller, chatId, spaceId, repo as unknown as RepoInput, edgeUrl, branch));
    }
    const chatId = rest[1];
    if (!chatId || !ID_RE.test(chatId)) return jsonError(404, "not_found", "Not found.");
    if (rest.length === 2 && request.method === "DELETE") return rpc(() => stub.deleteSession(caller, chatId));
    if (rest.length === 3 && request.method === "POST") {
      if (rest[2] === "wake") return rpc(() => stub.wakeSession(caller, chatId));
      if (rest[2] === "sleep") return rpc(() => stub.sleepSession(caller, chatId));
      if (rest[2] === "retry") {
        if (!isEdgeOrigin(edgeUrl)) return jsonError(500, "misconfigured", "Bad CLOUD_EDGE_URL.");
        return rpc(() => stub.retrySession(caller, chatId, edgeUrl));
      }
    }
  }
  return jsonError(404, "not_found", "Not found.");
};

/**
 * A user nudged `/device/{deviceId}` — a send to a chat hosted there. If that
 * is one of the caller's own sleeping Cloud sessions, ask its account DO to
 * wake that session's machine — fire-and-forget: the nudge proceeds untouched
 * (nudges queue in the DeviceRoom and replay once the engine is back). Only
 * sends wake a session; dials (viewing a chat) never do. The DO is the
 * caller's own, so another user's device id is simply not found there.
 */
export const autoWakeCloudDevice = (
  env: Pick<Env, "CLOUD_ACCOUNTS">,
  ctx: Pick<ExecutionContext, "waitUntil">,
  auth: Verified,
  deviceId: string
): void => {
  const ns = env.CLOUD_ACCOUNTS;
  if (!ns || auth.kind !== "user" || !auth.orgId || !deviceId.startsWith(CLOUD_DEVICE_PREFIX)) return;
  const caller = { orgId: auth.orgId, userId: auth.userId };
  ctx.waitUntil(
    accountStub(ns, caller.orgId, caller.userId)
      .autoWake(caller, deviceId)
      .then(() => undefined)
      .catch((e: unknown) => console.warn("cloud auto-wake failed", String(e)))
  );
};
