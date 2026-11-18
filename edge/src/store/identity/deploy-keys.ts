/**
 * Deploy keys: an ed25519 key made on the author's device, scoped to one
 * plugin, that publishes that plugin's versions without a signed-in session
 * (signed-request.ts), from CI. The secret never comes here.
 *
 *   GET    /store/v1/plugins/:id/keys
 *     → { keys: [{ key, fingerprint, name, createdAt, lastUsedAt, expiresAt, revokedAt }] }
 *   POST   /store/v1/plugins/:id/keys { key, name, expires?, proof, stepUp? }
 *     → 201 { key, plugin, name, fingerprint, createdAt, expiresAt }
 *   DELETE /store/v1/plugins/:id/keys/:key → { revoked: true }
 *
 * Only the plugin's owner, signed in (a session bearer or the web's cookie;
 * never a deploy key), manages its keys; a plugin no one owns yet has none
 * (publish its first version from your machine). Anyone else: 404.
 *
 * Registering is like a device's key (`POST /plugin-keys`): `proof` is the
 * key's signature over `"zeron-deploy-key:v1:" + id + ":" + key` (its own
 * prefix, naming the plugin, so no other proof stands in for it), and a new
 * key needs a fresh sign-in's step-up ticket: `stepUp` from `/auth/exchange`
 * with a bearer, or with the web's cookie the ticket its sign-in earned.
 * Without one: 403 `step_up_required`. The same live key again is a no-op.
 * `expires` (epoch ms, optional) must be in the future, within 5 years.
 * Limits (user-directory.ts): 10 live keys a plugin, 10 new a plugin and 20
 * new a user in 24 hours.
 *
 * A revoked or expired key publishes nothing, at once. `GET /plugin-keys/:key`
 * still answers whose it is (plugin-keys.ts), as unverified once revoked or
 * expired.
 */
import type { Env } from "../../env";
import { jsonError } from "../../http";
import { stepUpHash, stepUpRequired } from "../../step-up";
import { b64urlEncode, parseEd25519PublicKey, verifyEd25519 } from "../../cloud/crypto";
import { pluginKeyFingerprint, userDirectoryStub } from "../../user-directory";
import { cookieSession, bearerUser, forbidden, headerBearer, unauthenticated } from "./callers";
import { readBody } from "./body";
import { PLUGIN_ID_RE } from "./signed-request";

export const DEPLOY_KEY_PROOF_PREFIX = "zeron-deploy-key:v1:";

/** What a deploy key's proof of possession signs. */
export const deployKeyProofMessage = (pluginId: string, key: string): string => `${DEPLOY_KEY_PROOF_PREFIX}${pluginId}:${key}`;

/** The owner of a store plugin id (a user id), or null when no one has
 * claimed it. The store lane's (D1). */
export type PluginOwnerLookup = (env: Env, pluginId: string) => Promise<string | null>;

const MAX_BODY = 2048;
const MAX_NAME = 64;
const MAX_EXPIRY_MS = 5 * 365 * 86_400_000;

const fresh = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", "cache-control": "no-store" } });

const notFound = (): Response => jsonError(404, "not_found", "No plugin of yours has this id.");

/** A canonically spelled 32-byte key, or undefined. */
const parseKey = (key: unknown): string | undefined => {
  if (typeof key !== "string") return undefined;
  const raw = parseEd25519PublicKey(key);
  return raw && b64urlEncode(raw) === key ? key : undefined;
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

/** `/store/v1/plugins/:id/keys[/:key]`, or undefined when it isn't one. */
export const handleDeployKeyRoute = async (
  request: Request,
  env: Env,
  url: URL,
  pluginOwner: PluginOwnerLookup
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/");
  // ["", "store", "v1", "plugins", id, "keys", key?]
  if (parts[1] !== "store" || parts[2] !== "v1" || parts[3] !== "plugins" || parts[5] !== "keys" || parts.length > 7) return undefined;
  const pluginId = parts[4]!;
  const keyPart = parts[6];
  const methods = keyPart === undefined ? ["GET", "POST"] : ["DELETE"];
  if (!methods.includes(request.method)) {
    return new Response(JSON.stringify({ error: "method_not_allowed" }), {
      status: 405,
      headers: { "content-type": "application/json", allow: methods.join(", ") }
    });
  }
  const caller = await signedIn(request, env);
  if (caller instanceof Response) return caller;
  if (!PLUGIN_ID_RE.test(pluginId) || (await pluginOwner(env, pluginId)) !== caller.userId) return notFound();
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");

  if (request.method === "GET") return fresh({ keys: await directory.deployKeys(pluginId, caller.userId) });

  if (request.method === "DELETE") {
    const key = parseKey(keyPart);
    if (!key || !(await directory.revokeDeployKey(pluginId, caller.userId, key))) {
      return jsonError(404, "unknown_key", "This plugin has no such deploy key.");
    }
    return fresh({ revoked: true });
  }

  const raw = await readBody(request, MAX_BODY);
  if (raw === "too_large") return jsonError(413, "too_large", "The request body is too large.");
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
  if (typeof body?.proof !== "string" || !(await verifyEd25519(key, deployKeyProofMessage(pluginId, key), body.proof))) {
    return jsonError(400, "invalid_proof", "The proof isn't this key's signature.");
  }
  const ticket = body?.stepUp === undefined ? (caller.sessionStepUp ?? undefined) : await stepUpHash(body.stepUp);
  const result = await directory.registerDeployKey(caller.userId, pluginId, key, name, expires as number | null, ticket);
  switch (result) {
    case "registered": {
      const row = (await directory.deployKeys(pluginId, caller.userId)).find((k) => k.key === key)!;
      return fresh({ key, plugin: pluginId, name: row.name, fingerprint: await pluginKeyFingerprint(key), createdAt: row.createdAt, expiresAt: row.expiresAt }, 201);
    }
    case "step_up_required":
      return stepUpRequired();
    case "key_taken":
      return jsonError(409, "key_taken", "This key is already registered for something else.");
    case "key_revoked":
      return jsonError(409, "key_revoked", "This key was revoked. Make a new one.");
    case "too_many_keys":
      return jsonError(409, "too_many_keys", "This plugin has as many live deploy keys as it can. Revoke one first.");
    case "deleted_user":
      return forbidden("This account has been deleted.");
    case "rate_limited":
      return new Response(JSON.stringify({ error: "rate_limited", message: "Too many new deploy keys today. Try again tomorrow." }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "3600" }
      });
  }
};
