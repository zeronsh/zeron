/**
 * A deploy key's signed request (contract "Deploy-key request signature"):
 *
 *   Authorization: ZeronKey key=<b64url>, ts=<ms>, sig=<b64url>
 *   sig = ed25519(key, "zeron-plugin-publish:v1\n" + method + "\n" + path + "\n"
 *                      + sha256hex(body) + "\n" + ts)
 *
 * - `path` is the URL's path exactly as requested; a signed request carries
 *   no query string (it would be unsigned), so one with a query is refused.
 * - `ts` is the signer's clock, within `MAX_REQUEST_SKEW_MS` of the edge's.
 * - The body's hash is in what's signed, and travels beside it as
 *   `X-Zeron-Content-SHA256: <hex>`: the key, the signature and replay are
 *   all checked before a byte of the body is read (so nobody can make the
 *   edge read a package without the key's secret). The hash is then bound
 *   to the request (body.ts): the route's read of the body answers
 *   `body_mismatch` (or its stream fails) unless it is the body signed.
 * - Replay: the directory remembers what each accepted request signed for
 *   longer than its `ts` is accepted (`DEPLOY_REPLAY_WINDOW_MS`), so the
 *   same request is never accepted twice; once forgotten, its `ts` is stale.
 * - Scope: a deploy key belongs to one plugin, and does only what
 *   `deployKeyRoute` allows: submit a version of that plugin, and read that
 *   plugin's state (`GET /store/v1/me/plugins`, a GET signing the hash of an
 *   empty body). Anything else: 403 `key_not_allowed`.
 * - The key must be live: registered, not revoked, not expired, its owner's
 *   account not deleted. Its owner owning the plugin is the store's check,
 *   as it is for any publisher.
 *
 * Errors: 401 `unauthenticated` (malformed, unknown key, query string, no
 * body hash), `invalid_signature`, `stale_request`, `replayed_request`,
 * `key_revoked`, `key_expired`; 403 `key_not_allowed`.
 */
import { verifyEd25519 } from "../../cloud/crypto";
import type { Env } from "../../env";
import { jsonError } from "../../http";
import { userDirectoryStub, type DeployKeyRefusal } from "../../user-directory";
import { expectBodyHash } from "./body";
import {
  CONTENT_SHA256_HEADER,
  MAX_REQUEST_SKEW_MS,
  SHA256_HEX_RE,
  deployKeyRoute,
  parseZeronKeyAuthorization,
  publishSigningString,
  sha256Hex
} from "./signing";

export interface DeployKeyCaller {
  readonly userId: string;
  readonly username: string | null;
  readonly key: string;
  readonly pluginId: string;
}

const unauthenticated = (message: string): Response => jsonError(401, "unauthenticated", message);

/** The deploy key that signed `request`, checked before its body is read,
 * or the answer refusing it. The route reads the body through body.ts,
 * which holds it to the signed hash. */
export const verifySignedRequest = async (
  request: Request,
  env: Env,
  url: URL,
  now = Date.now()
): Promise<DeployKeyCaller | Response> => {
  const auth = parseZeronKeyAuthorization(request.headers.get("authorization"));
  if (!auth) return unauthenticated("The ZeronKey authorization is malformed.");
  if (url.search !== "") return unauthenticated("A signed request carries no query string.");
  const route = deployKeyRoute(request.method, url.pathname);
  if (!route) return jsonError(403, "key_not_allowed", "A deploy key only publishes its plugin's versions and reads its state.");
  if (Math.abs(now - Number(auth.ts)) > MAX_REQUEST_SKEW_MS) {
    return jsonError(401, "stale_request", "The request's time is more than 5 minutes from the server's. Check the clock.");
  }
  const bodyHash = request.headers.get(CONTENT_SHA256_HEADER);
  if (bodyHash === null || !SHA256_HEX_RE.test(bodyHash)) {
    return unauthenticated("A signed request carries its body's SHA-256 (hex) in X-Zeron-Content-SHA256.");
  }
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  // Everything about the request is checked before its body is read: the
  // key may act here, the signature covers the claimed body hash, and the
  // request is new. Only then is the body read, and held to that hash.
  const status = await directory.deployKeyStatus(auth.key, route.pluginId);
  if ("error" in status) return deployKeyRefusal(status.error);
  const message = publishSigningString(request.method, url.pathname, bodyHash, auth.ts);
  if (!(await verifyEd25519(auth.key, message, auth.sig))) {
    return jsonError(401, "invalid_signature", "The request's signature doesn't verify with this key.");
  }
  const used = await directory.useDeployKey(auth.key, route.pluginId, await sha256Hex(`${auth.key}\n${message}`));
  if ("error" in used) return deployKeyRefusal(used.error);
  // Whichever way the route reads the body, it gets this one or nothing.
  expectBodyHash(request, bodyHash);
  return { userId: used.userId, username: used.username, key: auth.key, pluginId: used.pluginId };
};

const deployKeyRefusal = (error: DeployKeyRefusal | "replayed_request"): Response => {
  switch (error) {
    case "unknown_key":
      return unauthenticated("No one has registered this deploy key.");
    case "key_revoked":
      return jsonError(401, "key_revoked", "This deploy key was revoked.");
    case "key_expired":
      return jsonError(401, "key_expired", "This deploy key has expired.");
    case "key_not_allowed":
      return jsonError(403, "key_not_allowed", "This deploy key belongs to another plugin.");
    case "replayed_request":
      return jsonError(401, "replayed_request", "This request was already made. Sign each request afresh.");
  }
};
