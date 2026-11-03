/**
 * HTTP surface of the Cloud control plane (docs/design/cloud-device.md,
 * "HTTP contract"):
 *
 *   GET    /cloud/{orgId}                          account status (user)
 *   POST   /cloud/{orgId}/enable                   turn Cloud on (user)
 *   DELETE /cloud/{orgId}                          turn Cloud off: every session machine (user)
 *   GET    /cloud/{orgId}/usage?from=&to=          credits: balance + use per day (user)
 *   GET    /cloud/{orgId}/billing                  plan, credits, what can be bought (user)
 *   POST   /cloud/{orgId}/billing/checkout         {kind: subscription, plan} | {kind: credits, packs} → {url}
 *   POST   /cloud/{orgId}/billing/portal           → {url} (Stripe Billing Portal)
 *   POST   /stripe/webhook                         pre-bearer: Stripe-signed events
 *   GET    /billing/done                           pre-bearer: "return to Zeron" after Checkout
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
import { DAY_MS, dayKey, isDayKey } from "./metering";
import { CLOUD_MODELS } from "./models";

/** The longest range a usage read covers. */
const MAX_USAGE_DAYS = 366;
import { CLOUD_DEVICE_PREFIX, cloudAccountName } from "./policy";
import { MAX_PACKS, billingAvailable, parseEvent, planById, verifySignature, type StripeEnv } from "./stripe";
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
  | "CLOUD_ACCESS"
> &
  ProviderEnv &
  StripeEnv;

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

/** Whether Cloud is offered to this caller (`CLOUD_ACCESS`): the feature
 * flag that keeps Cloud to a beta list. A runner acts for its machine's
 * owner, so it's judged as that user. */
export const cloudOffered = (env: Pick<Env, "CLOUD_ACCESS" | "AUTH_MODE">, auth: Pick<Verified, "userId" | "orgId">): boolean => {
  const access = env.CLOUD_ACCESS?.trim();
  if (!access) return env.AUTH_MODE === "dev";
  if (access === "everyone") return true;
  const listed = new Set(access.split(/[\s,]+/).filter(Boolean));
  return listed.has(auth.userId) || (auth.orgId !== undefined && listed.has(auth.orgId));
};

const rpc = async <T>(run: () => Promise<CloudResult<T>>): Promise<Response> => {
  try {
    return reply(await run());
  } catch (e) {
    console.error("cloud rpc failed", String(e));
    return jsonError(503, "unavailable", "Cloud is temporarily unavailable; try again.");
  }
};

/** Stripe events are a few KB; anything larger isn't one. */
const MAX_WEBHOOK_BODY = 256 * 1024;

/**
 * `POST /stripe/webhook` (Stripe-signed, no bearer) and `GET /billing/done`
 * (the page Checkout and the portal return to) — routed BEFORE the bearer
 * gate. A verified event goes to the CloudAccount its metadata names; one
 * billing ignores is acknowledged so Stripe stops retrying it.
 */
export const handleBillingPublicRoute = async (
  request: Request,
  env: CloudEnv,
  url: URL
): Promise<Response | undefined> => {
  if (url.pathname === "/billing/done" && request.method === "GET") return billingDonePage(url);
  if (url.pathname !== "/stripe/webhook") return undefined;
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  if (!env.STRIPE_WEBHOOK_SECRET || !env.CLOUD_ACCOUNTS) {
    return jsonError(503, "unavailable", "Billing isn't set up on this server.");
  }
  const payload = await request.text();
  if (payload.length > MAX_WEBHOOK_BODY) return jsonError(413, "too_large", "Body too large.");
  if (!(await verifySignature(env.STRIPE_WEBHOOK_SECRET, request.headers.get("stripe-signature"), payload))) {
    return jsonError(400, "bad_signature", "Signature check failed.");
  }
  let raw: unknown;
  try {
    raw = JSON.parse(payload);
  } catch {
    return jsonError(400, "bad_request", "Body must be JSON.");
  }
  if (!isObject(raw)) return jsonError(400, "bad_request", "Body must be a JSON object.");
  const event = parseEvent(env, raw);
  if (!event) return json({ received: true });
  try {
    const result = await accountStub(env.CLOUD_ACCOUNTS, event.account.orgId, event.account.userId).applyBillingEvent(event);
    return json({ received: true, applied: result.applied });
  } catch (e) {
    console.error("billing: applying a webhook failed", event.eventId, String(e));
    // Stripe retries a non-2xx: nothing is lost.
    return jsonError(503, "unavailable", "Try again.");
  }
};

/** The page a browser lands on after Checkout or the portal. */
const billingDonePage = (url: URL): Response => {
  const result = url.searchParams.get("result");
  const [title, body] =
    result === "success"
      ? ["You're all set", "Your purchase is complete. Head back to Zeron — your credits are on their way."]
      : result === "canceled"
        ? ["Checkout canceled", "Nothing was charged. You can head back to Zeron."]
        : ["Done", "You can head back to Zeron."];
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title} · Zeron</title>
<style>:root{color-scheme:light dark}body{margin:0;min-height:100vh;display:grid;place-items:center;font:15px/1.5 -apple-system,BlinkMacSystemFont,"Inter",system-ui,sans-serif;background:#0d0d0f;color:#ededef}
@media (prefers-color-scheme:light){body{background:#f7f7f8;color:#18181b}.card{background:#fff;border-color:#e4e4e7}p{color:#52525b}}
.card{max-width:380px;padding:40px 32px;border:1px solid #27272a;border-radius:14px;background:#161618;text-align:center}
h1{margin:0 0 8px;font-size:19px;font-weight:600}p{margin:0;color:#a1a1aa}</style></head>
<body><div class="card"><h1>${title}</h1><p>${body}</p></div></body></html>`;
  return new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } });
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
  if (!cloudOffered(env, auth)) {
    // Off the beta list: Cloud reads as unavailable (clients hide it) and
    // nothing new starts. Cleanup still works — deleting a chat deletes its
    // machine, and turning Cloud off removes the rest — so taking someone
    // off the list never strands what they already have.
    if (parts.length === 2 && request.method === "GET") {
      return json({ state: "off", awakeSessions: 0, maxAwakeSessions: 0, available: false });
    }
    const cleanup =
      request.method === "DELETE" && (parts.length === 2 || (parts.length === 4 && parts[2] === "sessions"));
    if (!cleanup) return jsonError(404, "not_found", "Cloud isn't available for this account.");
  }
  // A runner acts for the chat its machine hosts (runner-policy.ts admits
  // only session routes; the CloudAccount checks each is its own delegate).
  const caller =
    auth.kind === "runner" && auth.deviceId ? { orgId, userId: auth.userId, deviceId: auth.deviceId } : { orgId, userId: auth.userId };
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
    // Default: the last 30 days, today included.
    const to = url.searchParams.get("to") ?? dayKey(Date.now());
    const from = url.searchParams.get("from") ?? dayKey(Date.parse(`${to}T00:00:00Z`) - 29 * DAY_MS);
    if (!isDayKey(from) || !isDayKey(to) || from > to) {
      return jsonError(400, "bad_request", "from and to must be YYYY-MM-DD, from ≤ to.");
    }
    if ((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS >= MAX_USAGE_DAYS) {
      return jsonError(400, "bad_request", `At most ${MAX_USAGE_DAYS} days.`);
    }
    if (!ns) return json({ from, to, days: [], credits: 0, balance: 0, available: false });
    return rpc(() => accountStub(ns, orgId, auth.userId).usage(caller, from, to));
  }
  if (rest.length === 1 && rest[0] === "models" && request.method === "GET") {
    // Static (models.ts): readable whether or not this account turned Cloud on.
    return json({ models: CLOUD_MODELS });
  }
  if (rest[0] === "billing") {
    if (!ns) return unavailable();
    const stub = accountStub(ns, orgId, auth.userId);
    // Checkout and the portal come back to this edge's own page, in the
    // browser they were opened in.
    const returnUrl = `${url.origin}/billing/done`;
    if (rest.length === 1 && request.method === "GET") {
      return rpc(() => stub.billingStatus(caller, billingAvailable(env)));
    }
    if (!billingAvailable(env)) return jsonError(503, "unavailable", "Billing isn't set up on this server yet.");
    if (rest.length === 2 && rest[1] === "checkout" && request.method === "POST") {
      const body = await readJsonBody(request, 1024);
      if (!isObject(body)) return jsonError(400, "bad_request", "Body must be a JSON object.");
      if (body.kind === "subscription" && typeof body.plan === "string" && planById(body.plan)) {
        const plan = planById(body.plan)!.id;
        return rpc(() => stub.checkout(caller, { kind: "subscription", plan }, returnUrl));
      }
      if (
        body.kind === "credits" &&
        typeof body.packs === "number" &&
        Number.isInteger(body.packs) &&
        body.packs >= 1 &&
        body.packs <= MAX_PACKS
      ) {
        const packs = body.packs;
        return rpc(() => stub.checkout(caller, { kind: "credits", packs }, returnUrl));
      }
      return jsonError(400, "bad_request", `Expected {kind: "subscription", plan} or {kind: "credits", packs: 1–${MAX_PACKS}}.`);
    }
    if (rest.length === 2 && rest[1] === "portal" && request.method === "POST") {
      return rpc(() => stub.portal(caller, returnUrl));
    }
    return jsonError(404, "not_found", "Not found.");
  }
  if (!ns) return unavailable();
  const stub = accountStub(ns, orgId, auth.userId);
  // Session engines talk to the same origin the user's client used (or an
  // explicit override for dev tunnels).
  const edgeUrl = env.CLOUD_EDGE_URL ?? url.origin;

  if (rest.length === 0 && request.method === "DELETE") {
    // Off the beta list (cleanup only): the reply says so, or a client would
    // read "off but available" and set Cloud up again.
    const offered = cloudOffered(env, auth);
    return rpc(async () => {
      const result = await stub.destroy(caller);
      return result.ok && !offered ? { ok: true as const, value: { ...result.value, available: false } } : result;
    });
  }
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
      if (auth.kind === "runner") {
        // A delegate: the project and repository are the parent's.
        const { chatId, parentChatId, branch, config } = body;
        if (parentChatId === undefined) {
          return jsonError(403, "forbidden", "A Cloud machine starts sessions only for the chat it hosts.");
        }
        if (typeof chatId !== "string" || !ID_RE.test(chatId)) return jsonError(400, "bad_request", "Bad chatId.");
        if (typeof parentChatId !== "string" || !ID_RE.test(parentChatId)) return jsonError(400, "bad_request", "Bad parentChatId.");
        if (branch !== undefined && (typeof branch !== "string" || branch.length > 255)) return jsonError(400, "bad_request", "Bad branch.");
        if (config !== undefined && !isChildConfig(config)) {
          return jsonError(400, "bad_request", "Expected config {harness: codex | claude-code, …}.");
        }
        return rpc(() => stub.createChildSession(caller, chatId, parentChatId, edgeUrl, branch, config));
      }
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
  env: Pick<Env, "CLOUD_ACCOUNTS" | "CLOUD_ACCESS" | "AUTH_MODE">,
  ctx: Pick<ExecutionContext, "waitUntil">,
  auth: Verified,
  deviceId: string
): void => {
  const ns = env.CLOUD_ACCOUNTS;
  if (!ns || auth.kind !== "user" || !auth.orgId || !deviceId.startsWith(CLOUD_DEVICE_PREFIX)) return;
  // Off the beta list: nothing wakes (their machines stay asleep until deleted).
  if (!cloudOffered(env, auth)) return;
  const caller = { orgId: auth.orgId, userId: auth.userId };
  ctx.waitUntil(
    accountStub(ns, caller.orgId, caller.userId)
      .autoWake(caller, deviceId)
      .then(() => undefined)
      .catch((e: unknown) => console.warn("cloud auto-wake failed", String(e)))
  );
};

/** A delegate's agent settings: Cloud runs Codex and Claude Code; the rest is
 * the host's to validate (it ignores what it doesn't know). Bounded. */
const isChildConfig = (config: unknown): config is Record<string, unknown> =>
  isObject(config) &&
  (config.harness === "codex" || config.harness === "claude-code") &&
  JSON.stringify(config).length <= 4096;
