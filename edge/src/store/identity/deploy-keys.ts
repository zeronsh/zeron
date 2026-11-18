/**
 * Deploy keys (amendment 44): an ed25519 key made on the author's device
 * that publishes versions of plugins its account owns, without a signed-in
 * session (signed-request.ts), from CI. A key belongs to the account and
 * carries a scope fixed when it is made: a list of the account's plugin
 * ids, or all of its plugins (those it owns then and those it first
 * publishes later; never one transferred in). It never claims a new id. The
 * secret never comes here.
 *
 *   GET    /store/v1/me/keys
 *     → { keys: [{ key, fingerprint, name, scope, created_at, last_used_at, last_used_plugin, expires_at, revoked_at? }] }
 *   POST   /store/v1/me/keys { key, name, scope, expires?, proof, stepUp? } → 201 that key
 *   DELETE /store/v1/me/keys/:key → { revoked: true }
 *
 * `scope` is `{ all: true }` or `{ plugins: [id…] }` (1 to 50 ids, each the
 * caller's now; refused 400 `invalid_scope` with the bad or repeated `ids`,
 * or 403 `not_owner` with the ids that aren't theirs). Only the signed-in
 * account (a session bearer or the web's cookie; never a deploy key)
 * manages its keys.
 *
 * Registering is like a device's key (`POST /plugin-keys`): `proof` is the
 * key's signature over `"zeron-deploy-key:v2:" + publisher_id + ":" +
 * expiry + ":" + scopeText + ":" + key` (signing.ts), so a proof is good for
 * one account's key with one lifetime and one scope; and a new key needs a
 * fresh sign-in's step-up ticket: `stepUp` from `/auth/exchange` with a
 * bearer, or with the web's cookie the ticket its sign-in earned. Without
 * one: 403 `step_up_required`. The same live key again is a no-op.
 * `expires` (epoch ms, optional) must be in the future, within 5 years.
 * Limits (user-directory.ts): 20 live keys an account, 10 new a day.
 *
 * A revoked or expired key publishes nothing, at once. A plugin that leaves
 * the account leaves every key's list for good (`dropFromDeployKeys`).
 * `GET /plugin-keys/:key?plugin=` answers whose a key is (plugin-keys.ts),
 * verified only for a plugin its scope holds that its account owns.
 */
import type { Env } from "../../env";
import { adminAuthorized } from "../../cloud/billing";
import { jsonError, readJsonBody } from "../../http";
import { stepUpHash, stepUpRequired } from "../../step-up";
import { b64urlEncode, parseEd25519PublicKey, verifyEd25519 } from "../../cloud/crypto";
import { userDirectoryStub, type DeployKeyRow, type DeployKeyScope, type PluginOwnership, MAX_DEPLOY_KEY_PLUGINS } from "../../user-directory";
import { publisherId } from "../cards";
import { cookieSession, bearerUser, forbidden, headerBearer, unauthenticated } from "./callers";
import { readBody } from "./body";
import { PLUGIN_ID_RE, deployKeyProofMessage, scopeText } from "./signing";

/** Who owns a store plugin id, since when, and when it was first claimed;
 * null when no one has claimed it. The store lane's (D1). */
export type PluginOwnerLookup = (env: Env, pluginId: string) => Promise<PluginOwnership | null>;

const MAX_BODY = 2048;
const MAX_NAME = 64;
const MAX_EXPIRY_MS = 5 * 365 * 86_400_000;

const fresh = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", "cache-control": "no-store" } });


/** A canonically spelled 32-byte key, or undefined. */
const parseKey = (key: unknown): string | undefined => {
  if (typeof key !== "string") return undefined;
  const raw = parseEd25519PublicKey(key);
  return raw && b64urlEncode(raw) === key ? key : undefined;
};

/** A deploy key as the contract lists it: times in epoch ms (null for
 * none), `revoked_at` only once revoked. Never a secret. */
const wireKey = (row: DeployKeyRow) => ({
  key: row.key,
  fingerprint: row.fingerprint,
  name: row.name,
  scope: row.scope,
  created_at: row.createdAt,
  last_used_at: row.lastUsedAt,
  last_used_plugin: row.lastUsedPlugin,
  expires_at: row.expiresAt,
  ...(row.revokedAt === null ? {} : { revoked_at: row.revokedAt })
});

/** Plugin `pluginId` left account `userId` (the store's transfer or
 * release): it leaves every one of their deploy keys' lists, for good. */
export const dropFromDeployKeys = async (env: Env, userId: string, pluginId: string): Promise<number> => {
  const directory = userDirectoryStub(env);
  if (!directory) throw new Error("The user directory isn't set up on this server.");
  return directory.dropFromDeployKeys(userId, pluginId);
};

/** A request's `scope`, sorted: or the refusal, naming the ids at fault. */
const readScope = (raw: unknown): DeployKeyScope | Response => {
  const invalid = (message: string, ids?: readonly unknown[]) =>
    new Response(JSON.stringify({ error: "invalid_scope", message, ...(ids?.length ? { ids: ids.map(String).slice(0, MAX_DEPLOY_KEY_PLUGINS) } : {}) }), {
      status: 400,
      headers: { "content-type": "application/json", "cache-control": "no-store" }
    });
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return invalid("scope is { all: true } or { plugins: [id, …] }.");
  const scope = raw as Record<string, unknown>;
  const keys = Object.keys(scope);
  if (keys.length === 1 && scope.all === true) return { all: true };
  if (keys.length !== 1 || !Array.isArray(scope.plugins)) return invalid("scope is { all: true } or { plugins: [id, …] }.");
  const ids = scope.plugins as unknown[];
  if (ids.length < 1 || ids.length > MAX_DEPLOY_KEY_PLUGINS) return invalid(`A key's scope lists 1 to ${MAX_DEPLOY_KEY_PLUGINS} plugins.`);
  const bad = ids.filter((id) => typeof id !== "string" || !PLUGIN_ID_RE.test(id));
  if (bad.length) return invalid("Each plugin is a plugin id.", bad);
  const repeated = ids.filter((id, i) => ids.indexOf(id) !== i);
  // (A repeat is refused, never dropped: the proof names the scope as sent.)
  if (repeated.length) return invalid("Name each plugin once.", [...new Set(repeated)]);
  return { plugins: (ids as string[]).slice().sort() };
};

/** The signed-in caller (never a deploy key) with the step-up ticket a web
 * sign-in earned, or the answer refusing them. */
const signedIn = async (request: Request, env: Env): Promise<{ userId: string; sessionStepUp: string | null } | Response> => {
  const header = request.headers.get("authorization");
  if (header !== null) {
    const token = headerBearer(request);
    if (!token) return /^ZeronKey\s/i.test(header) ? jsonError(403, "key_not_allowed", "A deploy key can't manage keys.") : unauthenticated();
    const user = await bearerUser(env, token);
    return user instanceof Response ? user : { userId: user.userId, sessionStepUp: null };
  }
  const session = await cookieSession(request, env);
  if (!session) return unauthenticated();
  if (session instanceof Response) return session;
  return { userId: session.userId, sessionStepUp: session.stepUpHash };
};

/** `/store/v1/me/keys[/:key]`, or undefined when it isn't one. */
export const handleDeployKeyRoute = async (
  request: Request,
  env: Env,
  url: URL,
  pluginOwner: PluginOwnerLookup
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/");
  // ["", "store", "v1", "me", "keys", key?]
  if (parts[1] !== "store" || parts[2] !== "v1" || parts[3] !== "me" || parts[4] !== "keys" || parts.length > 6) return undefined;
  const keyPart = parts[5];
  const methods = keyPart === undefined ? ["GET", "POST"] : ["DELETE"];
  if (!methods.includes(request.method)) {
    return new Response(JSON.stringify({ error: "method_not_allowed" }), {
      status: 405,
      headers: { "content-type": "application/json", allow: methods.join(", ") }
    });
  }
  const caller = await signedIn(request, env);
  if (caller instanceof Response) return caller;
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");

  if (request.method === "GET") return fresh({ keys: (await directory.deployKeys(caller.userId)).map(wireKey) });

  if (request.method === "DELETE") {
    const key = parseKey(keyPart);
    if (!key || !(await directory.revokeDeployKey(caller.userId, key))) return jsonError(404, "unknown_key", "You have no such deploy key.");
    return fresh({ revoked: true });
  }

  const raw = await readBody(request, MAX_BODY);
  if (typeof raw === "string") return jsonError(413, "too_large", "The request body is too large.");
  let body: Record<string, unknown> | undefined;
  try {
    const parsed: unknown = JSON.parse(new TextDecoder().decode(raw));
    body = parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : undefined;
  } catch {
    body = undefined;
  }
  const key = parseKey(body?.key);
  if (!key) return jsonError(400, "invalid_key", "The key must be a 32-byte ed25519 public key, base64url without padding.");
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  if (name.length < 1 || name.length > MAX_NAME || /[\u0000-\u001f\u007f]/.test(name)) {
    return jsonError(400, "invalid_name", `Name the key in 1–${MAX_NAME} characters.`);
  }
  const now = Date.now();
  const expires = body?.expires ?? null;
  if (expires !== null && !(Number.isSafeInteger(expires) && (expires as number) > now && (expires as number) <= now + MAX_EXPIRY_MS)) {
    return jsonError(400, "invalid_expiry", "expires must be a future time in epoch milliseconds, within 5 years.");
  }
  const scope = readScope(body?.scope);
  if (scope instanceof Response) return scope;
  // Every listed plugin is the caller's now: a key never claims an id.
  if ("plugins" in scope) {
    const owners = await Promise.all(scope.plugins.map((id) => pluginOwner(env, id)));
    const notOwned = scope.plugins.filter((_, i) => owners[i]?.owner !== caller.userId);
    if (notOwned.length) {
      return new Response(JSON.stringify({ error: "not_owner", message: `You don't own ${notOwned.join(", ")} in the store.`, ids: notOwned }), {
        status: 403,
        headers: { "content-type": "application/json", "cache-control": "no-store" }
      });
    }
  }
  const message = deployKeyProofMessage(await publisherId(caller.userId), expires as number | null, scopeText(scope)!, key);
  if (typeof body?.proof !== "string" || !(await verifyEd25519(key, message, body.proof))) {
    return jsonError(400, "invalid_proof", "The proof isn't this key's signature over this account, expiry and scope.");
  }
  const ticket = body?.stepUp === undefined ? (caller.sessionStepUp ?? undefined) : await stepUpHash(body.stepUp);
  const result = await directory.registerDeployKey(caller.userId, key, name, expires as number | null, scope, ticket);
  switch (result) {
    case "registered": {
      const row = (await directory.deployKeys(caller.userId)).find((k) => k.key === key)!;
      return fresh(wireKey(row), 201);
    }
    case "step_up_required":
      return stepUpRequired();
    case "key_taken":
      return jsonError(409, "key_taken", "This key is already registered for something else.");
    case "key_revoked":
      return jsonError(409, "key_revoked", "This key was revoked. Make a new one.");
    case "too_many_keys":
      return jsonError(409, "too_many_keys", "This account has as many live deploy keys as it can (20). Revoke one first.");
    case "deleted_user":
      return forbidden("This account has been deleted.");
    case "rate_limited":
      return new Response(JSON.stringify({ error: "rate_limited", message: "Too many new deploy keys today (10). Try again tomorrow." }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "3600" }
      });
  }
};

/**
 * Operator routes (ADMIN_TOKEN, before the bearer gate):
 *   GET  /admin/deploy-keys                      → { live }   (live deploy keys)
 *   POST /admin/deploy-keys/revoke { pluginId }  → { revoked } (its owner's keys that may publish it)
 *   POST /admin/deploy-keys/revoke { all: true } → { revoked } (every one)
 * For a leaked key's plugin, and before rolling the edge back past deploy
 * keys: code from before them can't tell a deploy key from a device's, so
 * every deploy key must be revoked first (docs/plans/2026-11-18-plugin-store-identity-rollout.md).
 */
export const handleDeployKeyAdminRoute = async (request: Request, env: Env, url: URL, pluginOwner: PluginOwnerLookup): Promise<Response | undefined> => {
  if (url.pathname !== "/admin/deploy-keys" && url.pathname !== "/admin/deploy-keys/revoke") return undefined;
  if (!env.ADMIN_TOKEN) return jsonError(404, "not_found", "Not found.");
  if (!(await adminAuthorized(request, env.ADMIN_TOKEN))) return jsonError(401, "unauthenticated", "Admin token required.");
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  if (url.pathname === "/admin/deploy-keys") {
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
    return fresh({ live: await directory.liveDeployKeys() });
  }
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const body = (await readJsonBody(request, 1024)) as { pluginId?: unknown; all?: unknown } | undefined;
  if (body?.all === true && body.pluginId === undefined) {
    const revoked = await directory.revokeAllDeployKeys();
    console.log("deploy keys: an operator revoked all", revoked);
    return fresh({ revoked });
  }
  if (typeof body?.pluginId === "string" && PLUGIN_ID_RE.test(body.pluginId) && body.all === undefined) {
    // Its owner's keys whose scope may hold it (an `all` key among them).
    const owner = await pluginOwner(env, body.pluginId);
    return fresh({ revoked: owner ? await directory.revokeDeployKeysFor(owner.owner, body.pluginId) : 0 });
  }
  return jsonError(400, "bad_request", "Expected {pluginId} or {all: true}.");
};
