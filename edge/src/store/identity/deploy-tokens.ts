/**
 * Deploy tokens (contract a315eef5a, "Deploy tokens replace deploy keys"): a
 * plain bearer secret (deploy-token.ts) that publishes versions of plugins
 * its account owns, from CI, as npm, PyPI and crates.io tokens do. A token
 * belongs to the account and carries a scope fixed when it is made: a list
 * of the account's plugin ids, or all of its plugins (those it owns then and
 * those it first publishes later; never one transferred in). It never
 * publishes a plugin's first version.
 *
 *   GET    /store/v1/me/tokens
 *     → { tokens: [{ id, short, name, scope, created_at, last_used_at, last_used_plugin, expires_at, revoked_at }] }
 *   POST   /store/v1/me/tokens { name, scope, expires?, stepUp? }
 *     → 201 { token, id, short, name, scope, created_at, expires_at }   (the secret, this once)
 *   DELETE /store/v1/me/tokens/:id → { revoked: true }
 *
 * Only the signed-in account manages its tokens: a session bearer (the app,
 * the CLI) or the web's cookie (the site), never a deploy token itself.
 * Making one needs a fresh sign-in's step-up ticket, used up: `stepUp` from
 * `/auth/exchange` with a bearer, or with the web's cookie the ticket its
 * sign-in earned (signing in again on the site earns another). Without one:
 * 403 `step_up_required`.
 *
 * `scope` is `{ all: true }` or `{ plugins: [id…] }` (1 to 50 ids, each the
 * caller's now; refused 400 `invalid_scope` with the bad or repeated `ids`,
 * or 403 `not_owner` with the ids that aren't theirs). `expires` (epoch ms,
 * optional) is in the future, within 5 years. Limits (user-directory.ts):
 * 20 live tokens an account, 10 new a day.
 *
 * The secret is in the create answer and nowhere else: the edge keeps its
 * SHA-256 and short id, logs neither the secret nor its hash, and no list,
 * error or audit row carries more than the short id.
 */
import type { Env } from "../../env";
import { adminAuthorized } from "../../cloud/billing";
import { jsonError, readJsonBody } from "../../http";
import { stepUpHash, stepUpRequired } from "../../step-up";
import {
  MAX_DEPLOY_TOKEN_PLUGINS,
  userDirectoryStub,
  type DeployTokenRow,
  type DeployTokenScope,
  type PluginOwnership
} from "../../user-directory";
import { cookieSession, bearerUser, forbidden, headerBearer, unauthenticated } from "./callers";
import { readBody } from "./body";
import { DEPLOY_TOKEN_ID_RE, deployTokenHash, looksLikeDeployToken, newDeployToken, newDeployTokenId, parseDeployToken } from "./deploy-token";
import { PLUGIN_ID_RE } from "./signing";

/** Who owns a store plugin id, since when, and when it was first claimed;
 * null when no one has claimed it. The store lane's (D1). */
export type PluginOwnerLookup = (env: Env, pluginId: string) => Promise<PluginOwnership | null>;

const MAX_BODY = 4096;
const MAX_NAME = 64;
const MAX_EXPIRY_MS = 5 * 365 * 86_400_000;

const fresh = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", "cache-control": "no-store" } });

/** A token as its owner's list shows it: times in epoch ms (null for
 * none). Never the secret. */
const wireToken = (row: DeployTokenRow) => ({
  id: row.id,
  short: row.short,
  name: row.name,
  scope: row.scope,
  created_at: row.createdAt,
  last_used_at: row.lastUsedAt,
  last_used_plugin: row.lastUsedPlugin,
  expires_at: row.expiresAt,
  revoked_at: row.revokedAt
});

/** Plugin `pluginId` left account `userId` (the store's transfer or
 * release): it leaves every one of their tokens' lists, for good. */
export const dropFromDeployTokens = async (env: Env, userId: string, pluginId: string): Promise<number> => {
  const directory = userDirectoryStub(env);
  if (!directory) throw new Error("The user directory isn't set up on this server.");
  return directory.dropFromDeployTokens(userId, pluginId);
};

/** A request's `scope`, sorted: or the refusal, naming the ids at fault. */
const readScope = (raw: unknown): DeployTokenScope | Response => {
  const invalid = (message: string, ids?: readonly unknown[]) =>
    new Response(JSON.stringify({ error: "invalid_scope", message, ...(ids?.length ? { ids: ids.map(String).slice(0, MAX_DEPLOY_TOKEN_PLUGINS) } : {}) }), {
      status: 400,
      headers: { "content-type": "application/json", "cache-control": "no-store" }
    });
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return invalid("scope is { all: true } or { plugins: [id, …] }.");
  const scope = raw as Record<string, unknown>;
  const keys = Object.keys(scope);
  if (keys.length === 1 && scope.all === true) return { all: true };
  if (keys.length !== 1 || !Array.isArray(scope.plugins)) return invalid("scope is { all: true } or { plugins: [id, …] }.");
  const ids = scope.plugins as unknown[];
  if (ids.length < 1 || ids.length > MAX_DEPLOY_TOKEN_PLUGINS) return invalid(`A token's scope lists 1 to ${MAX_DEPLOY_TOKEN_PLUGINS} plugins.`);
  const bad = ids.filter((id) => typeof id !== "string" || !PLUGIN_ID_RE.test(id));
  if (bad.length) return invalid("Each plugin is a plugin id.", bad);
  const repeated = ids.filter((id, i) => ids.indexOf(id) !== i);
  if (repeated.length) return invalid("Name each plugin once.", [...new Set(repeated)]);
  return { plugins: (ids as string[]).slice().sort() };
};

/** The signed-in caller (never a deploy token) with the step-up ticket a
 * web sign-in earned, or the answer refusing them. */
const signedIn = async (request: Request, env: Env): Promise<{ userId: string; sessionStepUp: string | null } | Response> => {
  const header = request.headers.get("authorization");
  if (header !== null) {
    const token = headerBearer(request);
    if (!token) return unauthenticated();
    // A token manages nothing, its own kind included.
    if (looksLikeDeployToken(token)) return jsonError(403, "token_not_allowed", "A deploy token can only publish.");
    const user = await bearerUser(env, token);
    return user instanceof Response ? user : { userId: user.userId, sessionStepUp: null };
  }
  const session = await cookieSession(request, env);
  if (!session) return unauthenticated();
  if (session instanceof Response) return session;
  return { userId: session.userId, sessionStepUp: session.stepUpHash };
};

/** `/store/v1/me/tokens[/:id]`, or undefined when it isn't one. */
export const handleDeployTokenRoute = async (request: Request, env: Env, url: URL, pluginOwner: PluginOwnerLookup): Promise<Response | undefined> => {
  const parts = url.pathname.split("/");
  // ["", "store", "v1", "me", "tokens", id?]
  if (parts[1] !== "store" || parts[2] !== "v1" || parts[3] !== "me" || parts[4] !== "tokens" || parts.length > 6) return undefined;
  const idPart = parts[5];
  const methods = idPart === undefined ? ["GET", "POST"] : ["DELETE"];
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

  if (request.method === "GET") return fresh({ tokens: (await directory.deployTokens(caller.userId)).map(wireToken) });

  if (request.method === "DELETE") {
    if (!idPart || !DEPLOY_TOKEN_ID_RE.test(idPart) || !(await directory.revokeDeployToken(caller.userId, idPart))) {
      return jsonError(404, "unknown_token", "You have no such deploy token.");
    }
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
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  if (name.length < 1 || name.length > MAX_NAME || /[\u0000-\u001f\u007f]/.test(name)) {
    return jsonError(400, "invalid_name", `Name the token in 1–${MAX_NAME} characters.`);
  }
  const now = Date.now();
  const expires = body?.expires ?? null;
  if (expires !== null && !(Number.isSafeInteger(expires) && (expires as number) > now && (expires as number) <= now + MAX_EXPIRY_MS)) {
    return jsonError(400, "invalid_expiry", "expires must be a future time in epoch milliseconds, within 5 years.");
  }
  const scope = readScope(body?.scope);
  if (scope instanceof Response) return scope;
  // Every listed plugin is the caller's now: a token never claims an id.
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
  const ticket = body?.stepUp === undefined ? (caller.sessionStepUp ?? undefined) : await stepUpHash(body.stepUp);
  const token = newDeployToken();
  const short = parseDeployToken(token)!.short;
  const id = newDeployTokenId();
  const result = await directory.createDeployToken(
    caller.userId,
    { id, hash: await deployTokenHash(token), short, name, expiresAt: expires as number | null, scope },
    ticket
  );
  switch (result) {
    case "created": {
      const row = (await directory.deployTokens(caller.userId)).find((t) => t.id === id)!;
      // The secret, this once.
      return fresh({ token, id, short, name: row.name, scope: row.scope, created_at: row.createdAt, expires_at: row.expiresAt }, 201);
    }
    case "step_up_required":
      return stepUpRequired();
    case "too_many_tokens":
      return jsonError(409, "too_many_tokens", "This account has as many live deploy tokens as it can (20). Revoke one first.");
    case "deleted_user":
      return forbidden("This account has been deleted.");
    case "rate_limited":
      return new Response(JSON.stringify({ error: "rate_limited", message: "Too many new deploy tokens today (10). Try again tomorrow." }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "3600" }
      });
  }
};

/**
 * Operator routes (ADMIN_TOKEN, before the bearer gate):
 *   GET  /admin/deploy-tokens                      → { live }
 *   POST /admin/deploy-tokens/revoke { pluginId }  → { revoked } (its owner's tokens that may publish it)
 *   POST /admin/deploy-tokens/revoke { all: true } → { revoked } (every one)
 * For a leaked token's plugin, or every token at once.
 */
export const handleDeployTokenAdminRoute = async (request: Request, env: Env, url: URL, pluginOwner: PluginOwnerLookup): Promise<Response | undefined> => {
  if (url.pathname !== "/admin/deploy-tokens" && url.pathname !== "/admin/deploy-tokens/revoke") return undefined;
  if (!env.ADMIN_TOKEN) return jsonError(404, "not_found", "Not found.");
  if (!(await adminAuthorized(request, env.ADMIN_TOKEN))) return jsonError(401, "unauthenticated", "Admin token required.");
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  if (url.pathname === "/admin/deploy-tokens") {
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
    return fresh({ live: await directory.liveDeployTokens() });
  }
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const body = (await readJsonBody(request, 1024)) as { pluginId?: unknown; all?: unknown } | undefined;
  if (body?.all === true && body.pluginId === undefined) {
    const revoked = await directory.revokeAllDeployTokens();
    console.log("deploy tokens: an operator revoked all", revoked);
    return fresh({ revoked });
  }
  if (typeof body?.pluginId === "string" && PLUGIN_ID_RE.test(body.pluginId) && body.all === undefined) {
    const owner = await pluginOwner(env, body.pluginId);
    return fresh({ revoked: owner ? await directory.revokeDeployTokensFor(owner.owner, body.pluginId) : 0 });
  }
  return jsonError(400, "bad_request", "Expected {pluginId} or {all: true}.");
};
