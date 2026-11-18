/**
 * The pure parts of deploy keys: the signed-request header and signing
 * string (signed-request.ts), which routes a deploy key may use, and the
 * proof of possession a registration carries (deploy-keys.ts). The engine's
 * CLI signs exactly these strings.
 */
import { b64urlEncode, parseEd25519PublicKey } from "../../cloud/crypto";

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

/** The hex SHA-256 of the body a signed request signs, sent beside it, so
 * the signature is checked before a byte of the body is read. */
export const CONTENT_SHA256_HEADER = "x-zeron-content-sha256";
export const SHA256_HEX_RE = /^[0-9a-f]{64}$/;

export const sha256Hex = async (bytes: Uint8Array | string): Promise<string> => {
  const data = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  const digest = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};

/** What a deploy key signs for a request. */
export const publishSigningString = (method: string, path: string, bodySha256Hex: string, ts: string): string =>
  `${PUBLISH_SIGNATURE_PREFIX}${method}\n${path}\n${bodySha256Hex}\n${ts}`;

/**
 * What a deploy key may do at `method path` (amendments 6 and 44), or
 * undefined: publish a version of a plugin (`pluginId` named by the path;
 * its scope and its account's ownership are checked after), or read its
 * account's plugins in its scope (`GET /store/v1/me/plugins`, `pluginId`
 * null). Yanking, unyanking and managing keys need a signed-in person.
 */
export const deployKeyRoute = (method: string, path: string): { readonly pluginId: string | null } | undefined => {
  if (method === "GET" && path === "/store/v1/me/plugins") return { pluginId: null };
  if (method !== "POST") return undefined;
  const parts = path.split("/");
  // ["", "store", "v1", "plugins", id, "versions"]
  if (parts.length !== 6 || parts[0] !== "" || parts[1] !== "store" || parts[2] !== "v1" || parts[3] !== "plugins" || parts[5] !== "versions") {
    return undefined;
  }
  return PLUGIN_ID_RE.test(parts[4]!) ? { pluginId: parts[4]! } : undefined;
};

export const DEPLOY_KEY_PROOF_PREFIX = "zeron-deploy-key:v2:";

/** A deploy key's scope as its proof spells it: `*` for all, else its ids,
 * each an id, sorted by code unit, joined with `,`; undefined when a list
 * names an id twice or isn't ids. */
export const scopeText = (scope: { readonly all: true } | { readonly plugins: readonly string[] }): string | undefined => {
  if ("all" in scope) return "*";
  const ids = [...scope.plugins];
  if (ids.length === 0 || !ids.every((id) => PLUGIN_ID_RE.test(id)) || new Set(ids).size !== ids.length) return undefined;
  return ids.sort().join(",");
};

/** What a deploy key's proof of possession signs (contract
 * 2026-11-19-store-shapes-44-45): the account's publisher id, the key's
 * expiry (`expires_at` in ms, or `never`), its scope and the key itself, so
 * a proof is good for one account's key with one lifetime and one scope. */
export const deployKeyProofMessage = (publisherId: string, expiresAt: number | null, scope: string, key: string): string =>
  `${DEPLOY_KEY_PROOF_PREFIX}${publisherId}:${expiresAt === null ? "never" : String(expiresAt)}:${scope}:${key}`;
