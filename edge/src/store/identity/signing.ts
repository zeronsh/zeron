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

export const DEPLOY_KEY_PROOF_PREFIX = "zeron-deploy-key:v1:";

/** What a deploy key's proof of possession signs. */
export const deployKeyProofMessage = (pluginId: string, key: string): string => `${DEPLOY_KEY_PROOF_PREFIX}${pluginId}:${key}`;
