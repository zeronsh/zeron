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
 * - Scope: a deploy key belongs to one plugin, and works only on that
 *   plugin's publishing routes (`deployKeyRoute`): submitting a version and
 *   withdrawing, yanking or unyanking one. Anything else: 403 `key_not_allowed`.
 * - The key must be live: registered, not revoked, not expired, its owner's
 *   account not deleted. Its owner owning the plugin is the store's check,
 *   as it is for any publisher.
 *
 * Errors: 401 `unauthenticated` (malformed, unknown key, query string),
 * `invalid_signature`, `stale_request`, `replayed_request`, `key_revoked`,
 * `key_expired`; 403 `key_not_allowed`; 413 `too_large`.
 */
import { b64urlEncode, parseEd25519PublicKey, verifyEd25519 } from "../../cloud/crypto";
import type { Env } from "../../env";
import { jsonError } from "../../http";
import { userDirectoryStub } from "../../user-directory";
import { readBody } from "./body";

export const PUBLISH_SIGNATURE_PREFIX = "zeron-plugin-publish:v1\n";
/** How far a signed request's `ts` may be from the edge's clock, either way. */
export const MAX_REQUEST_SKEW_MS = 5 * 60_000;

/** A store plugin id (the manifest's `id` rule). */
export const PLUGIN_ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;

export interface ZeronKeyAuthorization {
  readonly key: string;
  /** As sent: the digits that were signed. */
  readonly ts: string;
  readonly sig: string;
}

/** `ZeronKey key=…, ts=…, sig=…` (any order, each once, nothing else), or
 * undefined. The key must be a canonically spelled 32-byte key. */
export const parseZeronKeyAuthorization = (header: string | null): ZeronKeyAuthorization | undefined => {
  const match = header ? /^ZeronKey[ ]+(.+)$/i.exec(header.trim()) : null;
  if (!match) return undefined;
  const params = new Map<string, string>();
  for (const part of match[1]!.split(",")) {
    const eq = part.indexOf("=");
    if (eq < 0) return undefined;
    const name = part.slice(0, eq).trim();
    const value = part.slice(eq + 1).trim();
    if (!["key", "ts", "sig"].includes(name) || params.has(name) || !value) return undefined;
    params.set(name, value);
  }
  const key = params.get("key");
  const ts = params.get("ts");
  const sig = params.get("sig");
  if (!key || !ts || !sig || !/^\d{1,16}$/.test(ts) || !/^[A-Za-z0-9_-]{86}$/.test(sig)) return undefined;
  const raw = parseEd25519PublicKey(key);
  if (!raw || b64urlEncode(raw) !== key) return undefined;
  return { key, ts, sig };
};

export const sha256Hex = async (bytes: Uint8Array | string): Promise<string> => {
  const data = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  const digest = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};

/** What a deploy key signs for a request. */
export const publishSigningString = (method: string, path: string, bodySha256Hex: string, ts: string): string =>
  `${PUBLISH_SIGNATURE_PREFIX}${method}\n${path}\n${bodySha256Hex}\n${ts}`;

const VERSION_ACTIONS = new Set(["withdraw", "yank", "unyank"]);

/** The plugin a deploy key may act on at `method path`, or undefined: the
 * publishing routes only. */
export const deployKeyRoute = (method: string, path: string): { readonly pluginId: string } | undefined => {
  if (method !== "POST") return undefined;
  const parts = path.split("/");
  // ["", "store", "v1", "plugins", id, "versions", (version, action)?]
  if (parts[0] !== "" || parts[1] !== "store" || parts[2] !== "v1" || parts[3] !== "plugins" || parts[5] !== "versions") return undefined;
  const pluginId = parts[4]!;
  if (!PLUGIN_ID_RE.test(pluginId)) return undefined;
  if (parts.length === 6) return { pluginId };
  if (parts.length === 8 && parts[6] && VERSION_ACTIONS.has(parts[7]!)) return { pluginId };
  return undefined;
};

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
  if (!route) return jsonError(403, "key_not_allowed", "A deploy key only publishes its plugin's versions.");
  if (Math.abs(now - Number(auth.ts)) > MAX_REQUEST_SKEW_MS) {
    return jsonError(401, "stale_request", "The request's time is more than 5 minutes from the server's. Check the clock.");
  }
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  // The key first, so nobody's body is read for a key that can't publish here.
  const status = await directory.deployKeyStatus(auth.key, route.pluginId);
  if (status !== "ok") return deployKeyRefusal(status);
  const body = await readBody(request);
  if (body === "too_large") return jsonError(413, "too_large", "The request body is too large.");
  const message = publishSigningString(request.method, url.pathname, await sha256Hex(body), auth.ts);
  if (!(await verifyEd25519(auth.key, message, auth.sig))) {
    return jsonError(401, "invalid_signature", "The request's signature doesn't verify with this key.");
  }
  const used = await directory.useDeployKey(auth.key, route.pluginId, await sha256Hex(`${auth.key}\n${message}`));
  if ("error" in used) return deployKeyRefusal(used.error);
  return { userId: used.userId, username: used.username, key: auth.key, pluginId: route.pluginId };
};

const deployKeyRefusal = (error: "unknown_key" | "key_revoked" | "key_expired" | "key_not_allowed" | "replayed_request"): Response => {
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
