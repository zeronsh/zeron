/**
 * Reading a `.zeron-plugin` exactly as the engine does
 * (crates/engine/src/plugins/{package.rs `parse`, install.rs `parse`,
 * signing.rs}): every check before anything is kept, in the engine's order,
 * so a package the store takes is one every device takes, and the reverse.
 *
 *   { "format": "zeron-plugin:v1", "manifest": {…signed…}, "files": { rel: base64 } }
 *
 * 1. The JSON (json.ts, serde_json's rules); `format`; `manifest` an object;
 *    `files` an object of at most 2,000.
 * 2. Each file, in path order: a plain path (paths.ts), not the manifest
 *    file, its data strict standard base64 (padded, canonical). Their total
 *    at most 50 MB.
 * 3. The signature (`manifest.signature = {key, files, sig}`): `files` must
 *    equal every file's sha256, `sig` an ed25519 signature by `key` over
 *    `"zeron-plugin:v1\n" + canonical(manifest with signature = {key, files})`.
 * 4. The manifest's rules (manifest.ts), its id, its entry file present.
 *
 * Then the content hash: sha256 of `canonical(manifest without signature) +
 * "\n" + canonical(file hashes)`.
 */
import { type Json, type JsonObject, JsonError, canonicalJson, isObject, parseJson } from "./json";
import { MANIFEST_FILE, type Manifest, idProblem, readManifest } from "./manifest";
import { plainPath } from "./paths";

export const FORMAT = "zeron-plugin:v1";
export const PAYLOAD_PREFIX = "zeron-plugin:v1\n";
export const MAX_FILES = 2000;
export const MAX_BYTES = 50 * 1024 * 1024;
/** package.rs `MAX_PACKAGE_BYTES`: the engine's own ceiling. */
export const MAX_PACKAGE_BYTES = Math.floor((MAX_BYTES * 4) / 3) + 1024 * 1024;

/** Why a package was refused: install.rs's reason words, plus the plugin
 * level ones (`bad_manifest`, `bad_id`, `missing_entry`). */
export type PackageError =
  | "not_a_package"
  | "unsigned"
  | "tampered"
  | "bad_signature"
  | "too_big"
  | "bad_file"
  | "bad_manifest"
  | "bad_id"
  | "missing_entry";

export const PACKAGE_WORDS: Record<PackageError, string> = {
  not_a_package: "That isn't a Zeron plugin",
  unsigned: "It isn't signed, so there's no telling who made it",
  tampered: "Its files don't match its signature: it was changed after it was signed",
  bad_signature: "Its signature doesn't check out",
  too_big: "It's too big for a plugin",
  bad_file: "It has a file Zeron won't write",
  bad_manifest: "Its manifest is broken",
  bad_id: "Its id can't be used",
  missing_entry: "Its entry file is missing"
};

export class PackageRefusal extends Error {
  constructor(
    readonly code: PackageError,
    message: string
  ) {
    super(message);
  }
}

export interface VerifiedPackage {
  readonly manifest: Manifest;
  /** The signer's public key, b64url without padding. */
  readonly key: string;
  /** Every file, by path, in path order. */
  readonly files: ReadonlyMap<string, Uint8Array>;
  /** Each file's sha256, hex. */
  readonly hashes: ReadonlyMap<string, string>;
  /** The engine's content hash. */
  readonly hash: string;
  /** sha256 of the package file, hex. */
  readonly sha256: string;
  readonly bytes: number;
}

const encoder = new TextEncoder();

export const hex = (bytes: Uint8Array): string => {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
};

export const sha256Hex = async (bytes: Uint8Array): Promise<string> =>
  hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));

const STANDARD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const table = (alphabet: string): Int16Array => {
  const t = new Int16Array(128).fill(-1);
  for (let i = 0; i < alphabet.length; i++) t[alphabet.charCodeAt(i)] = i;
  return t;
};
const STANDARD_TABLE = table(STANDARD);
const URL_SAFE_TABLE = table(URL_SAFE);

/**
 * The base64 crate's decoding (0.22): `padded` is STANDARD (padding
 * required and canonical), otherwise URL_SAFE_NO_PAD (no padding). Either
 * way the unused bits of the last symbol must be zero, and nothing but the
 * alphabet is taken (no whitespace).
 */
export const decodeBase64 = (text: string, padded: boolean): Uint8Array | undefined => {
  const lookup = padded ? STANDARD_TABLE : URL_SAFE_TABLE;
  let length = text.length;
  if (padded) {
    if (length % 4 !== 0) return undefined;
    if (length > 0 && text.charCodeAt(length - 1) === 0x3d) {
      length--;
      if (text.charCodeAt(length - 1) === 0x3d) length--;
    }
  } else if (length % 4 === 1) {
    return undefined;
  }
  const remainder = length % 4;
  if (remainder === 1) return undefined;
  const out = new Uint8Array(Math.floor(length / 4) * 3 + (remainder === 0 ? 0 : remainder - 1));
  let o = 0;
  let i = 0;
  const symbol = (at: number): number => {
    const c = text.charCodeAt(at);
    return c < 128 ? lookup[c]! : -1;
  };
  for (; i + 4 <= length; i += 4) {
    const a = symbol(i);
    const b = symbol(i + 1);
    const c = symbol(i + 2);
    const d = symbol(i + 3);
    if ((a | b | c | d) < 0) return undefined;
    const n = (a << 18) | (b << 12) | (c << 6) | d;
    out[o++] = n >> 16;
    out[o++] = (n >> 8) & 0xff;
    out[o++] = n & 0xff;
  }
  if (remainder === 2) {
    const a = symbol(i);
    const b = symbol(i + 1);
    if ((a | b) < 0 || (b & 0x0f) !== 0) return undefined;
    out[o++] = (a << 2) | (b >> 4);
  } else if (remainder === 3) {
    const a = symbol(i);
    const b = symbol(i + 1);
    const c = symbol(i + 2);
    if ((a | b | c) < 0 || (c & 0x03) !== 0) return undefined;
    out[o++] = (a << 2) | (b >> 4);
    out[o++] = ((b & 0x0f) << 4) | (c >> 2);
  }
  return out;
};

const filesValue = (hashes: ReadonlyMap<string, string>): JsonObject => new Map([...hashes].map(([k, v]) => [k, v] as [string, Json]));

/** signing.rs `payload_for`: what a signature covers. */
export const signaturePayload = (prefix: string, manifest: JsonObject, key: string, hashes: ReadonlyMap<string, string>): string => {
  const value: JsonObject = new Map(manifest);
  value.set("signature", new Map<string, Json>([
    ["key", key],
    ["files", filesValue(hashes)]
  ]));
  return prefix + canonicalJson(value);
};

/** signing.rs `content_hash`. */
export const contentHash = async (manifest: JsonObject, hashes: ReadonlyMap<string, string>): Promise<string> => {
  const unsigned: JsonObject = new Map(manifest);
  unsigned.delete("signature");
  return sha256Hex(encoder.encode(`${canonicalJson(unsigned)}\n${canonicalJson(filesValue(hashes))}`));
};

export const verifyEd25519 = async (key: Uint8Array, message: Uint8Array, signature: Uint8Array): Promise<boolean> => {
  try {
    const imported = await crypto.subtle.importKey("raw", key, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, imported, signature, message);
  } catch {
    return false;
  }
};

export type SignatureCheck = { readonly valid: string } | { readonly invalid: "unsigned" | "files_differ" | "bad_signature" };

/** signing.rs `check_raw`. */
export const checkSignature = async (
  prefix: string,
  manifest: JsonObject,
  hashes: ReadonlyMap<string, string>
): Promise<SignatureCheck> => {
  const signature = manifest.get("signature");
  if (signature === undefined) return { invalid: "unsigned" };
  const key = isObject(signature) ? signature.get("key") : undefined;
  const sig = isObject(signature) ? signature.get("sig") : undefined;
  const signed = isObject(signature) ? signature.get("files") : undefined;
  if (typeof key !== "string" || typeof sig !== "string" || !isObject(signed)) return { invalid: "bad_signature" };
  if (![...signed.values()].every((v) => typeof v === "string")) return { invalid: "bad_signature" };
  if (signed.size !== hashes.size || [...hashes].some(([path, hash]) => signed.get(path) !== hash)) return { invalid: "files_differ" };
  const keyBytes = decodeBase64(key, false);
  const sigBytes = decodeBase64(sig, false);
  if (!keyBytes || !sigBytes || keyBytes.length !== 32 || sigBytes.length !== 64) return { invalid: "bad_signature" };
  const message = encoder.encode(signaturePayload(prefix, manifest, key, hashes));
  return (await verifyEd25519(keyBytes, message, sigBytes)) ? { valid: key } : { invalid: "bad_signature" };
};

const refuse = (code: PackageError, detail?: string): never => {
  throw new PackageRefusal(code, detail ? `${PACKAGE_WORDS[code]}: ${detail}` : PACKAGE_WORDS[code]);
};

/** Compare as Rust's `String` `Ord` does: bytewise on UTF-8, which for
 * well-formed strings is code point order. */
const byCodePoint = (a: string, b: string): number => {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    const x = a.codePointAt(i)!;
    const y = b.codePointAt(i)!;
    if (x !== y) return x - y;
    if (x > 0xffff) i++;
  }
  return a.length - b.length;
};

/**
 * The package `raw`, verified as install.rs `parse` verifies it, or a
 * `PackageRefusal` naming the engine's reason. `maxBytes` is the caller's
 * ceiling on the file itself (the engine's, or the store's lower one).
 */
export const verifyPackage = async (raw: Uint8Array, maxBytes = MAX_PACKAGE_BYTES): Promise<VerifiedPackage> => {
  if (raw.length > maxBytes) refuse("too_big");
  let value: Json;
  try {
    value = parseJson(raw);
  } catch (e) {
    if (e instanceof JsonError) return refuse("not_a_package");
    throw e;
  }
  if (!isObject(value) || value.get("format") !== FORMAT) refuse("not_a_package");
  const top = value as JsonObject;
  const manifestValue = top.get("manifest");
  const filesValueRaw = top.get("files");
  if (!isObject(filesValueRaw) || !isObject(manifestValue)) return refuse("not_a_package");
  if (filesValueRaw.size > MAX_FILES) refuse("too_big");
  const files = new Map<string, Uint8Array>();
  let total = 0;
  for (const rel of [...filesValueRaw.keys()].sort(byCodePoint)) {
    if (!plainPath(rel) || rel === MANIFEST_FILE) refuse("bad_file", JSON.stringify(rel));
    const data = filesValueRaw.get(rel);
    const bytes = typeof data === "string" ? decodeBase64(data, true) : undefined;
    if (!bytes) return refuse("bad_file", JSON.stringify(rel));
    total += bytes.length;
    files.set(rel, bytes);
  }
  if (total > MAX_BYTES) refuse("too_big");
  const hashes = new Map<string, string>();
  for (const [rel, bytes] of files) hashes.set(rel, await sha256Hex(bytes));
  const checked = await checkSignature(PAYLOAD_PREFIX, manifestValue, hashes);
  if ("invalid" in checked) {
    return refuse(checked.invalid === "files_differ" ? "tampered" : checked.invalid);
  }
  const manifest = readManifest(manifestValue);
  if (manifest.problem) refuse("bad_manifest", manifest.problem);
  const badId = idProblem(manifest.id);
  if (badId) refuse("bad_id", badId);
  if (!files.has(manifest.entry)) refuse("missing_entry", `Its entry file ${manifest.entry} is missing`);
  return {
    manifest,
    key: checked.valid,
    files,
    hashes,
    hash: await contentHash(manifestValue, hashes),
    sha256: await sha256Hex(raw),
    bytes: raw.length
  };
};
