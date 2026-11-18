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
 * - The body's hash is in what's signed: the package can't be swapped.
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
 * Errors: 401 `unauthenticated` (malformed, unknown key, query string),
 * `invalid_signature`, `stale_request`, `replayed_request`, `key_revoked`,
 * `key_expired`; 403 `key_not_allowed`; 413 `too_large`.
 */
import { verifyEd25519 } from "../../cloud/crypto";
import type { Env } from "../../env";
import { jsonError } from "../../http";
import { userDirectoryStub, type DeployKeyRefusal } from "../../user-directory";
import { readBody } from "./body";
import { MAX_REQUEST_SKEW_MS, deployKeyRoute, parseZeronKeyAuthorization, publishSigningString, sha256Hex } from "./signing";

export interface DeployKeyCaller {
  readonly userId: string;
  readonly username: string | null;
  readonly key: string;
  readonly pluginId: string;
}

const unauthenticated = (message: string): Response => jsonError(401, "unauthenticated", message);

/** The deploy key that signed `request`, checked end to end, or the answer
 * refusing it. Reads the body (body.ts), so the route reads it from there. */
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
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  // The key first, so nobody's body is read for a key that can't publish here.
  const status = await directory.deployKeyStatus(auth.key, route.pluginId);
  if ("error" in status) return deployKeyRefusal(status.error);
  const body = await readBody(request);
  if (body === "too_large") return jsonError(413, "too_large", "The request body is too large.");
  const message = publishSigningString(request.method, url.pathname, await sha256Hex(body), auth.ts);
  if (!(await verifyEd25519(auth.key, message, auth.sig))) {
    return jsonError(401, "invalid_signature", "The request's signature doesn't verify with this key.");
  }
  const used = await directory.useDeployKey(auth.key, route.pluginId, await sha256Hex(`${auth.key}\n${message}`));
  if ("error" in used) return deployKeyRefusal(used.error);
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
