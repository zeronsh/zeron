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
import { Scanner, type Span, decodeBase64String, spanDecodedBound, stringText } from "./scan";

export const FORMAT = "zeron-plugin:v1";
export const PAYLOAD_PREFIX = "zeron-plugin:v1\n";
export const MAX_FILES = 2000;
export const MAX_BYTES = 50 * 1024 * 1024;
/** package.rs `MAX_PACKAGE_BYTES`: the engine's own ceiling. */
export const MAX_PACKAGE_BYTES = Math.floor((MAX_BYTES * 4) / 3) + 1024 * 1024;

/** Why a package was refused: install.rs's reason words, plus
 * `bad_manifest` for a manifest Zeron won't run (its rules, its id, its
 * entry file), as the engine's vectors name them. */
export type PackageError =
  | "not_a_package"
  | "unsigned"
  | "tampered"
  | "bad_signature"
  | "too_big"
  | "bad_file"
  | "bad_manifest";

export const PACKAGE_WORDS: Record<PackageError, string> = {
  not_a_package: "That isn't a Zeron plugin",
  unsigned: "It isn't signed, so there's no telling who made it",
  tampered: "Its files don't match its signature: it was changed after it was signed",
  bad_signature: "Its signature doesn't check out",
  too_big: "It's too big for a plugin",
  bad_file: "It has a file Zeron won't write",
  bad_manifest: "Its manifest is broken"
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
  /** The manifest's canonical JSON without its signature: what the
   * content hash covers (the signature's file hashes are kept per file). */
  readonly manifestCanonical: string;
  /** The signer's public key, b64url without padding. */
  readonly key: string;
  /** Every file, by path, in path order: views of `blob`. */
  readonly files: ReadonlyMap<string, Uint8Array>;
  /** Every file's bytes, one after another in path order. */
  readonly blob: Uint8Array;
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
 * The base64 crate's decoding (0.22), into `out` at `at`: `padded` is
 * STANDARD (padding required and canonical), otherwise URL_SAFE_NO_PAD (no
 * padding). Either way the unused bits of the last symbol must be zero and
 * nothing but the alphabet is taken (no whitespace). Answers the bytes
 * written, or -1 if `text` isn't base64. `out` needs `decodedBound` room.
 */
export const decodeBase64Into = (text: string, padded: boolean, out: Uint8Array, at: number): number =>
  decodeSymbols((i) => text.charCodeAt(i), text.length, padded, out, at);

/** `decodeBase64Into` over the ASCII bytes `src[start, end)`. */
export const decodeBase64BytesInto = (src: Uint8Array, start: number, end: number, padded: boolean, out: Uint8Array, at: number): number =>
  decodeSymbols((i) => src[start + i]!, end - start, padded, out, at);

const decodeSymbols = (code: (index: number) => number, textLength: number, padded: boolean, out: Uint8Array, at: number): number => {
  const lookup = padded ? STANDARD_TABLE : URL_SAFE_TABLE;
  let length = textLength;
  if (padded) {
    if (length % 4 !== 0) return -1;
    if (length > 0 && code(length - 1) === 0x3d) {
      length--;
      if (code(length - 1) === 0x3d) length--;
    }
  }
  const remainder = length % 4;
  if (remainder === 1) return -1;
  let o = at;
  let i = 0;
  const symbol = (index: number): number => {
    const c = code(index);
    return c < 128 ? lookup[c]! : -1;
  };
  for (; i + 4 <= length; i += 4) {
    const a = symbol(i);
    const b = symbol(i + 1);
    const c = symbol(i + 2);
    const d = symbol(i + 3);
    if ((a | b | c | d) < 0) return -1;
    const n = (a << 18) | (b << 12) | (c << 6) | d;
    out[o++] = n >> 16;
    out[o++] = (n >> 8) & 0xff;
    out[o++] = n & 0xff;
  }
  if (remainder === 2) {
    const a = symbol(i);
    const b = symbol(i + 1);
    if ((a | b) < 0 || (b & 0x0f) !== 0) return -1;
    out[o++] = (a << 2) | (b >> 4);
  } else if (remainder === 3) {
    const a = symbol(i);
    const b = symbol(i + 1);
    const c = symbol(i + 2);
    if ((a | b | c) < 0 || (c & 0x03) !== 0) return -1;
    out[o++] = (a << 2) | (b >> 4);
    out[o++] = ((b & 0x0f) << 4) | (c >> 2);
  }
  return o - at;
};

/** The most bytes `text` can decode to. */
export const decodedBound = (text: { readonly length: number }): number => Math.ceil(text.length / 4) * 3;

export const decodeBase64 = (text: string, padded: boolean): Uint8Array | undefined => {
  const out = new Uint8Array(decodedBound(text));
  const n = decodeBase64Into(text, padded, out, 0);
  return n < 0 ? undefined : out.subarray(0, n);
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

const unsignedOf = (manifest: JsonObject): JsonObject => {
  const unsigned: JsonObject = new Map(manifest);
  unsigned.delete("signature");
  return unsigned;
};

/** signing.rs `content_hash`. */
export const contentHash = async (manifest: JsonObject, hashes: ReadonlyMap<string, string>): Promise<string> =>
  sha256Hex(encoder.encode(`${canonicalJson(unsignedOf(manifest))}\n${canonicalJson(filesValue(hashes))}`));

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
  manifest: Json,
  hashes: ReadonlyMap<string, string>
): Promise<SignatureCheck> => {
  // (`Value::get` on anything but an object finds nothing.)
  if (!isObject(manifest)) return { invalid: "unsigned" };
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

/** The longest manifest JSON the store reads (bytes as written), and the
 * most values in it: bounds on what building it can cost. The canonical
 * manifest is held to 256 KB as a listing rule (listing.ts). */
export const MAX_MANIFEST_JSON = 1024 * 1024;
export const MAX_MANIFEST_VALUES = 100_000;
/** A file's path is at most 1,024 bytes, and each takes at most six bytes
 * of JSON (`\u0041`): a longer key is no path, read or not. */
const MAX_KEY_JSON = 6 * 1024 + 6;

const keyText = (raw: Uint8Array, span: Span): string | undefined => (span.end - span.start <= 96 ? stringText(raw, span) : undefined);

/**
 * The package `raw`, verified as install.rs `parse` verifies it, or a
 * `PackageRefusal` naming the engine's reason. `maxBytes` is the caller's
 * ceiling on the file itself (the engine's, or the store's lower one).
 *
 * The whole document is checked first, as serde_json reads it, without
 * building it (scan.ts); then only the manifest is built, under
 * `MAX_MANIFEST_JSON` and `MAX_MANIFEST_VALUES`, and each file's base64 is
 * decoded from the request's bytes into one buffer.
 */
export const verifyPackage = async (raw: Uint8Array, maxBytes = MAX_PACKAGE_BYTES): Promise<VerifiedPackage> => {
  if (raw.length > maxBytes) refuse("too_big");
  let format: Span | undefined;
  let manifestSpan: Span | undefined;
  let filesSpan: Span | undefined;
  try {
    const top = new Scanner(raw);
    top.skipWhitespace();
    if (raw[top.at] !== 0x7b) return refuse("not_a_package");
    // Duplicate keys: the last one wins, as serde_json's map keeps it.
    top.members((key, value) => {
      const name = keyText(raw, key);
      if (name === "format") format = value;
      else if (name === "manifest") manifestSpan = value;
      else if (name === "files") filesSpan = value;
    });
    top.end();
  } catch (e) {
    if (e instanceof JsonError) return refuse("not_a_package");
    throw e;
  }
  const isString = (span: Span | undefined): span is Span => span !== undefined && raw[span.start] === 0x22;
  const inner = (span: Span): Span => ({ start: span.start + 1, end: span.end - 1 });
  if (!isString(format) || format.end - format.start > 128 || stringText(raw, inner(format)) !== FORMAT) return refuse("not_a_package");
  if (!filesSpan || raw[filesSpan.start] !== 0x7b || !manifestSpan || raw[manifestSpan.start] !== 0x7b) return refuse("not_a_package");

  const entries = new Map<string, Span | null>();
  let tooLong = 0;
  const files = new Scanner(raw);
  files.at = filesSpan.start;
  files.members((key, value) => {
    // (A key too long to be a path stands for itself: it's a bad file.)
    const name = key.end - key.start > MAX_KEY_JSON ? `\u0000${tooLong++}` : stringText(raw, key);
    entries.set(name, isString(value) ? inner(value) : null);
    return entries.size <= MAX_FILES;
  });
  if (entries.size > MAX_FILES) refuse("too_big");
  if (manifestSpan.end - manifestSpan.start > MAX_MANIFEST_JSON) refuse("too_big", "Its manifest is over 1 MB");
  let manifestValue: JsonObject;
  try {
    manifestValue = parseJson(raw.subarray(manifestSpan.start, manifestSpan.end), { maxValues: MAX_MANIFEST_VALUES }) as JsonObject;
  } catch (e) {
    if (e instanceof JsonError) return refuse("too_big", "Its manifest has too many values");
    throw e;
  }

  const paths = [...entries.keys()].sort(byCodePoint);
  // Every file decodes into one buffer, in path order: the files are views
  // of it, and it is what the store keeps as the version's files.
  let bound = 0;
  for (const span of entries.values()) if (span) bound += spanDecodedBound(span);
  const blob = new Uint8Array(bound);
  const decoded = new Map<string, Uint8Array>();
  let total = 0;
  for (const rel of paths) {
    if (rel.startsWith("\u0000") || !plainPath(rel) || rel === MANIFEST_FILE) refuse("bad_file", JSON.stringify(rel.slice(0, 80)));
    const span = entries.get(rel);
    const written = span ? decodeBase64String(raw, span, blob, total) : -1;
    if (written < 0) return refuse("bad_file", JSON.stringify(rel));
    decoded.set(rel, blob.subarray(total, total + written));
    total += written;
  }
  if (total > MAX_BYTES) refuse("too_big");
  const hashes = new Map<string, string>();
  for (const [rel, bytes] of decoded) hashes.set(rel, await sha256Hex(bytes));
  const checked = await checkSignature(PAYLOAD_PREFIX, manifestValue, hashes);
  if ("invalid" in checked) {
    return refuse(checked.invalid === "files_differ" ? "tampered" : checked.invalid);
  }
  const manifest = readManifest(manifestValue);
  if (manifest.problem) refuse("bad_manifest", manifest.problem);
  const badId = idProblem(manifest.id);
  if (badId) refuse("bad_manifest", badId);
  if (!decoded.has(manifest.entry)) refuse("bad_manifest", `Its entry file ${manifest.entry} is missing`);
  return {
    manifest,
    manifestCanonical: canonicalJson(unsignedOf(manifestValue)),
    key: checked.valid,
    files: decoded,
    blob: blob.subarray(0, total),
    hashes,
    hash: await contentHash(manifestValue, hashes),
    sha256: await sha256Hex(raw),
    bytes: raw.length
  };
};
