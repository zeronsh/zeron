/**
 * `/plugin-keys`: who signed a plugin. Each engine makes an ed25519 signing
 * key and registers it here for the signed-in account; an engine verifying a
 * plugin's signature asks who the key belongs to. Runner tokens never get
 * here (runner-policy.ts names no `/plugin-keys`).
 *
 *   POST   /plugin-keys { key, deviceId?, proof, stepUp } → 201 { key, registered: true }
 *   GET    /plugin-keys/:key → { key, username, mine, revoked, verified, fingerprint, firstSeen, revokedAt }
 *   GET    /plugin-keys[?since=ms] → { keys: [{ key, fingerprint, deviceId, createdAt, revokedAt }], now }
 *   DELETE /plugin-keys/:key                      → { revoked: true }
 *
 * `key` is the raw 32-byte public key, b64url without padding; `proof` is
 * its 64-byte signature over `PLUGIN_KEY_PROOF_PREFIX + key`, so the key's
 * holder made this request and the bearer names the account. A lookup never
 * answers the owner's user id, only their current username.
 *
 * Binding a NEW key needs `stepUp`, a fresh sign-in's ticket (step-up.ts):
 * a bearer alone (any local process an engine gives its token to) must not
 * be able to make its key read as "yours" on every device of the account.
 * Without one: 403 `step_up_required`. Re-registering a key already live on
 * the account needs none.
 *
 * New keys show up in the list: an engine polls `GET /plugin-keys?since=` and
 * tells its person about keys added from other devices (with `now` as the
 * next `since`). Revocation (DELETE) is bearer-only and applies to the next
 * lookup. Lookups and lists are `no-store`.
 *
 * Only `verified` keys may read as "Made by @username": not revoked, owner
 * holds a live username. To anyone but the owner, a live key whose owner has
 * no username answers 404 like an unregistered one.
 *
 * Stored in the user directory (user-directory.ts, `plugin_keys`).
 */
import type { Verified } from "./auth";
import { b64urlEncode, parseEd25519PublicKey, verifyEd25519 } from "./cloud/crypto";
import type { Env } from "./env";
import { jsonError, readJsonBody } from "./http";
import { stepUpHash, stepUpRequired } from "./step-up";
import { userDirectoryStub } from "./user-directory";

export const PLUGIN_KEY_PROOF_PREFIX = "zeron-plugin-key:v1:";

const DEVICE_ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const MAX_BODY = 1024;

/** The canonical spelling of a 32-byte key, or undefined. Strict, so one key
 * can't be registered twice under two spellings. */
const parseKey = (key: unknown): string | undefined => {
  if (typeof key !== "string") return undefined;
  const raw = parseEd25519PublicKey(key);
  return raw && b64urlEncode(raw) === key ? key : undefined;
};

/** Who owns a key changes the moment it's revoked: never cached. */
const fresh = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" }
  });

const unknownKey = (): Response => jsonError(404, "unknown_key", "No one has registered this key.");

export const handlePluginKeyRoute = async (
  request: Request,
  env: Env,
  auth: Verified,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "plugin-keys" || parts.length > 2) return undefined;
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");

  if (parts.length === 1) {
    if (request.method === "GET") {
      const raw = url.searchParams.get("since");
      const since = raw === null ? undefined : Number(raw);
      if (since !== undefined && !(Number.isSafeInteger(since) && since >= 0)) {
        return jsonError(400, "invalid_since", "since must be epoch milliseconds.");
      }
      // `now` before the read: a key landing in between shows up next time too.
      const now = Date.now();
      return fresh({ keys: await directory.pluginKeys(auth.userId, since), now });
    }
    if (request.method !== "POST") return jsonError(405, "method_not_allowed", "GET or POST only.");
    const body = (await readJsonBody(request, MAX_BODY)) as
      | { key?: unknown; deviceId?: unknown; proof?: unknown; stepUp?: unknown }
      | undefined;
    const key = parseKey(body?.key);
    if (!key) return jsonError(400, "invalid_key", "The key must be a 32-byte ed25519 public key, base64url without padding.");
    const deviceId = body?.deviceId ?? null;
    if (deviceId !== null && !(typeof deviceId === "string" && DEVICE_ID_RE.test(deviceId))) {
      return jsonError(400, "invalid_device", "deviceId must be 1–128 letters, digits, _ or -.");
    }
    if (typeof body?.proof !== "string" || !(await verifyEd25519(key, PLUGIN_KEY_PROOF_PREFIX + key, body.proof))) {
      return jsonError(400, "invalid_proof", "The proof isn't this key's signature.");
    }
    switch (await directory.registerPluginKey(auth.userId, key, deviceId, await stepUpHash(body?.stepUp))) {
      case "registered":
        return fresh({ key, registered: true }, 201);
      case "step_up_required":
        return stepUpRequired();
      case "key_taken":
        return jsonError(409, "key_taken", "Another account registered this key.");
      case "key_revoked":
        return jsonError(409, "key_revoked", "This key was revoked. Make a new one.");
      case "deleted_user":
        return jsonError(403, "deleted_user", "This account has been deleted.");
      case "rate_limited":
        return new Response(JSON.stringify({ error: "rate_limited", message: "Too many new keys today. Try again tomorrow." }), {
          status: 429,
          headers: { "content-type": "application/json", "retry-after": "3600" }
        });
    }
  }

  // The b64url alphabet needs no escaping, so an escaped key isn't one.
  const key = parseKey(parts[1]);
  if (request.method === "GET") {
    const found = key ? await directory.pluginKey(key, auth.userId) : undefined;
    return found ? fresh(found) : unknownKey();
  }
  if (request.method !== "DELETE") return jsonError(405, "method_not_allowed", "GET or DELETE only.");
  if (!key || !(await directory.revokePluginKey(auth.userId, key))) return unknownKey();
  return fresh({ revoked: true });
};
