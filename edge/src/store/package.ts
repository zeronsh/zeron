/**
 * Reading a `.zeron-plugin` as the engine does (crates/engine/src/plugins/
 * {package.rs `parse`, install.rs `parse`, signing.rs}): the same checks in
 * the same order, so a package the store takes is one every device takes.
 * The store's own bounds come on top (its body cap, the manifest's size and
 * value count, a long duplicated file key's word): within them the reverse
 * holds too, and past them it refuses what a device might take.
 *
 *   { "format": "zeron-plugin:v1", "manifest": {…signed…}, "files": { rel: base64 } }
 *
 * 1. The JSON, checked whole as serde_json reads it without being built
 *    (scan.ts); `format`; `manifest` an object; `files` an object of at
 *    most 2,000.
 * 2. Each file, in path order: a plain path (paths.ts), not the manifest
 *    file, nor a name that could be another's (a Windows short name, a
 *    character drawn as nothing), its data strict standard base64 (padded, canonical). Their total
 *    at most 50 MB.
 * 3. The signature (`manifest.signature = {key, files, sig}`): `files` must
 *    equal every file's sha256, `sig` an ed25519 signature by `key` over
 *    `"zeron-plugin:v1\n" + canonical(manifest with signature = {key, files})`.
 *    Or, for a package published with a deploy token (`signed: false`), no
 *    `signature` at all: the token is the publisher's word, and the store's
 *    record the device's (contract a315eef5a).
 * 4. The manifest's rules (manifest.ts), its id, its entry file present.
 *
 * Then the content hash: sha256 of `canonical(manifest without signature) +
 * "\n" + canonical(file hashes)`.
 */

import { type Json, type JsonObject, JsonError, canonicalJson, isObject, parseJson } from "./json";
import { MANIFEST_FILE, type Manifest, idProblem, readManifest, rustTrim } from "./manifest";
import { LIMITS, STORE_DATA } from "./data";
import type { Kind } from "./kind";
import { SOURCE_FILE, firstCollision, fold, pathProblem, plainPath, shortName, unseenName } from "./paths";
import { Scanner, decodeBase64String, decodedBound as spanBound, stringIs, stringText } from "./scan";

export const FORMAT: string = STORE_DATA.package_format;
export const PAYLOAD_PREFIX: string = STORE_DATA.payload_prefix;
export const MAX_FILES = LIMITS.max_files;
export const MAX_BYTES = LIMITS.max_bytes;
/** package.rs `MAX_PACKAGE_BYTES`: the engine's own ceiling. */
export const MAX_PACKAGE_BYTES = LIMITS.max_package_bytes;

/** Why a package was refused: install.rs's reason words, plus the plugin
 * level ones (`bad_manifest`, `bad_id`, `missing_entry`), as the engine's
 * vectors name them. */
export type PackageError =
  | "not_a_package"
  | "unsigned"
  | "tampered"
  | "bad_signature"
  | "too_big"
  | "bad_file"
  | "paths_collide"
  | "bad_manifest"
  | "bad_id"
  | "missing_entry";

/** What a refused package of `kind` is told, by why. */
const packageWords = (kind: Kind): Record<PackageError, string> => ({
  not_a_package: `That isn't a Zeron ${kind}`,
  unsigned: "It isn't signed, so there's no telling who made it",
  tampered: "Its files don't match its signature: it was changed after it was signed",
  bad_signature: "Its signature doesn't check out",
  too_big: `It's too big for a ${kind}`,
  bad_file: "It has a file Zeron won't write",
  paths_collide: "Two of its paths are one file on macOS and Windows",
  bad_manifest: "Its manifest is broken",
  bad_id: "Its id can't be used",
  missing_entry: "Its entry file is missing"
});
const PACKAGE_WORDS: Record<Kind, Record<PackageError, string>> = { plugin: packageWords("plugin"), template: packageWords("template") };

export class PackageRefusal extends Error {
  constructor(
    readonly code: PackageError,
    message: string
  ) {
    super(message);
  }
}

/** What every verified package is, whichever kind. */
export interface VerifiedContents {
  /** The manifest's canonical JSON without its signature: what the
   * content hash covers (the signature's file hashes are kept per file). */
  readonly manifestCanonical: string;
  /** The signer's public key, b64url without padding; null for an unsigned
   * package (a deploy token's). */
  readonly key: string | null;
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

export interface VerifiedPackage extends VerifiedContents {
  readonly manifest: Manifest;
}

/** A template's package (template_packages.rs `Package`). */
export interface VerifiedTemplate extends VerifiedContents {
  /** Its manifest as a value (signed; its signature removed in
   * `manifestCanonical`). */
  readonly manifest: JsonObject;
  /** Its id and version, trimmed, as a device reads them. */
  readonly id: string;
  readonly version: string;
  /** Files it carries that a device leaves out when it installs: what no
   * template may replace (nor a Windows short name, which may be one), a
   * manifest of its own, and anything but pictures under `screenshots/`. */
  readonly ignored: readonly string[];
}

export const TEMPLATE_FORMAT: string = STORE_DATA.template_format;
export const TEMPLATE_PAYLOAD_PREFIX: string = STORE_DATA.template_payload_prefix;
/** A template's manifest file, and what a device writes beside its files. */
export const TEMPLATE_MANIFEST_FILE = "zeron-template.json";
const TEMPLATE_IMPORTED_FILE = ".imported.json";
/** The one file that may start with a dot, at the root. */
const LUAURC = ".luaurc";
/** Zeron's own template's id: no one's. */
const ZERON_ID = "zeron";
const TEMPLATE_ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
/** Whether `id` can be a template's (templates.rs `valid_id`). */
export const validTemplateId = (id: string): boolean => TEMPLATE_ID_RE.test(id);

/** templates.rs `inside`: a safe path inside a template, whose parts never
 * start with a dot, but `.luaurc` at its root. */
export const insideTemplate = (rel: string): boolean => pathProblem(rel) === undefined && rel.split("/").every((part) => !part.startsWith(".") || rel === LUAURC);

/** What no template replaces, each one a file or a folder (data file
 * `template_protected`), folded as a device folds a path. */
const PROTECTED: readonly string[] = STORE_DATA.template_protected.map(fold);
/** Whether `rel` is, or is under, one of them (template_packages.rs `protected`). */
export const protectedPath = (rel: string): boolean => {
  const path = fold(rel);
  return PROTECTED.some((p) => path === p || path.startsWith(`${p}/`));
};

/** A template's own pictures' folder, and the types of them. */
export const TEMPLATE_SCREENSHOTS = "screenshots";
const SCREENSHOT_TYPES: readonly string[] = LIMITS.screenshot_types;
/** Whether `rel` is under `screenshots/` (as a disk folding names has it) and
 * isn't a PNG or JPEG there by its name: refused in the store, left out by a
 * device, never composed (listing.rs `template_stray_screenshot`). */
export const strayScreenshot = (rel: string): boolean => {
  if (rel.startsWith(`${TEMPLATE_SCREENSHOTS}/`)) {
    const dot = rel.lastIndexOf(".");
    if (dot >= 0 && SCREENSHOT_TYPES.includes(rel.slice(dot + 1).replace(/[A-Z]/g, (c) => c.toLowerCase()))) return false;
  }
  const path = fold(rel);
  return path === TEMPLATE_SCREENSHOTS || path.startsWith(`${TEMPLATE_SCREENSHOTS}/`);
};

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

const refuser = (kind: Kind) => (code: PackageError, detail?: string): never => {
  const words = PACKAGE_WORDS[kind][code];
  throw new PackageRefusal(code, detail ? `${words}: ${detail}` : words);
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
export const MAX_MANIFEST_JSON = LIMITS.max_manifest_json_bytes;
export const MAX_MANIFEST_VALUES = LIMITS.max_manifest_values;
/** A file's path is at most 1,024 bytes, and each takes at most six bytes
 * of JSON (`\u0041`): a longer key is no path, read or not. */
const MAX_KEY_JSON = 6 * 1024 + 6;
/** Stands for every key too long to be a path: one entry, however many,
 * as duplicates of one key would be (they can't be told apart unread, and
 * any of them makes the package `bad_file`). */
const TOO_LONG_KEY = "\u0000too long";

interface Span {
  readonly start: number;
  readonly end: number;
}

/** The package's JSON as the engine reads its parts: checked whole (scan.ts),
 * then `format`'s text, `manifest`'s bytes and each of `files`' members,
 * the last of a duplicated key winning, as serde_json's map keeps it. */
const readParts = (raw: Uint8Array) => {
  let format: Span | undefined;
  let manifest: Span | undefined;
  let files: Span | undefined;
  const top = new Scanner(raw);
  top.skipWhitespace();
  if (raw[top.at] !== 0x7b) return undefined;
  top.members(0, (ks, ke, vs, ve) => {
    if (stringIs(raw, ks, ke, "format")) format = { start: vs, end: ve };
    else if (stringIs(raw, ks, ke, "manifest")) manifest = { start: vs, end: ve };
    else if (stringIs(raw, ks, ke, "files")) files = { start: vs, end: ve };
  });
  top.finish();
  return { format, manifest, files };
};

/** A string value's content span, if `span` is a string. */
const stringSpan = (raw: Uint8Array, span: Span | undefined): Span | undefined =>
  span !== undefined && raw[span.start] === 0x22 ? { start: span.start + 1, end: span.end - 1 } : undefined;

/** `files`' members: each key's text (or `TOO_LONG_KEY`) and its data's
 * span (null when it isn't a string), stopping past MAX_FILES distinct. */
const readFiles = (raw: Uint8Array, files: Span): Map<string, Span | null> => {
  const entries = new Map<string, Span | null>();
  const scanner = new Scanner(raw);
  scanner.at = files.start;
  // (`files` is a member of the top level: inside one container.)
  // (A key repeated member after member is read once.)
  let lastStart = -1;
  let lastEnd = -1;
  let lastName = "";
  const sameAsLast = (ks: number, ke: number): boolean => {
    if (lastStart < 0 || ke - ks !== lastEnd - lastStart) return false;
    for (let i = 0; i < ke - ks; i++) if (raw[ks + i] !== raw[lastStart + i]) return false;
    return true;
  };
  scanner.members(1, (ks, ke, vs, ve) => {
    const name = ke - ks > MAX_KEY_JSON ? TOO_LONG_KEY : sameAsLast(ks, ke) ? lastName : stringText(raw, ks, ke);
    lastStart = ks;
    lastEnd = ke;
    lastName = name;
    entries.set(name, stringSpan(raw, { start: vs, end: ve }) ?? null);
    return entries.size <= MAX_FILES;
  });
  return entries;
};

/** A kind of package: its format, its signing domain, and which paths it
 * keeps. */
interface PackageKind {
  readonly kind: Kind;
  readonly format: string;
  readonly prefix: string;
  readonly keeps: (rel: string) => boolean;
}

const PLUGIN_PACKAGE: PackageKind = { kind: "plugin", format: FORMAT, prefix: PAYLOAD_PREFIX, keeps: (rel) => plainPath(rel) && rel !== MANIFEST_FILE && !shortName(rel) && !unseenName(rel) };
const TEMPLATE_PACKAGE: PackageKind = { kind: "template", format: TEMPLATE_FORMAT, prefix: TEMPLATE_PAYLOAD_PREFIX, keeps: (rel) => insideTemplate(rel) && !unseenName(rel) };

/** A package read and verified (package.rs `parse`), before its kind's own
 * rules. */
interface Read extends VerifiedContents {
  readonly manifestValue: JsonObject;
}

/**
 * The package `raw`, read as a device reads one of `kind`, or a
 * `PackageRefusal` naming the engine's reason. `maxBytes` is the caller's
 * ceiling on the file itself (the engine's, or the store's lower one).
 * `signed: false` reads a deploy token's package: one with no signature,
 * refused if it has one.
 *
 * The whole document is checked first, as serde_json reads it, without
 * building it (scan.ts); then only the manifest is built, under
 * `MAX_MANIFEST_JSON` and `MAX_MANIFEST_VALUES`, and each file's base64 is
 * decoded from the request's bytes into one buffer.
 */
const readPackage = async (raw: Uint8Array, kind: PackageKind, maxBytes: number, signed: boolean): Promise<Read> => {
  const refuse: (code: PackageError, detail?: string) => never = refuser(kind.kind);
  if (raw.length > maxBytes) refuse("too_big");
  let parts: ReturnType<typeof readParts>;
  try {
    parts = readParts(raw);
  } catch (e) {
    if (e instanceof JsonError) return refuse("not_a_package");
    throw e;
  }
  if (!parts) return refuse("not_a_package");
  const format = stringSpan(raw, parts.format);
  if (!format || format.end - format.start > 128 || stringText(raw, format.start, format.end) !== kind.format) return refuse("not_a_package");
  const { files: filesSpan, manifest: manifestSpan } = parts;
  if (!filesSpan || raw[filesSpan.start] !== 0x7b || !manifestSpan || raw[manifestSpan.start] !== 0x7b) return refuse("not_a_package");

  const entries = readFiles(raw, filesSpan);
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
  for (const span of entries.values()) if (span) bound += spanBound(span.start, span.end);
  const blob = new Uint8Array(bound);
  const decoded = new Map<string, Uint8Array>();
  let total = 0;
  for (const rel of paths) {
    if (rel === TOO_LONG_KEY || !kind.keeps(rel)) refuse("bad_file", JSON.stringify(rel.slice(0, 80)));
    const span = entries.get(rel);
    const written = span ? decodeBase64String(raw, span.start, span.end, blob, total) : -1;
    if (written < 0) return refuse("bad_file", JSON.stringify(rel));
    decoded.set(rel, blob.subarray(total, total + written));
    total += written;
  }
  if (total > MAX_BYTES) refuse("too_big");
  const hashes = new Map<string, string>();
  for (const [rel, bytes] of decoded) hashes.set(rel, await sha256Hex(bytes));
  let key: string | null = null;
  if (signed) {
    const checked = await checkSignature(kind.prefix, manifestValue, hashes);
    if ("invalid" in checked) return refuse(checked.invalid === "files_differ" ? "tampered" : checked.invalid);
    key = checked.valid;
  } else if (manifestValue.has("signature")) {
    return refuse("bad_signature", "A package published with a deploy token isn't signed: the token is its publisher's word");
  }
  return {
    manifestValue,
    manifestCanonical: canonicalJson(unsignedOf(manifestValue)),
    key,
    files: decoded,
    blob: blob.subarray(0, total),
    hashes,
    hash: await contentHash(manifestValue, hashes),
    sha256: await sha256Hex(raw),
    bytes: raw.length
  };
};

/** The plugin package `raw`, verified as install.rs `parse` verifies it
 * (and the collisions and the manifest's own rules after its signature). */
export const verifyPackage = async (raw: Uint8Array, maxBytes = MAX_PACKAGE_BYTES, signed = true): Promise<VerifiedPackage> => {
  const refuse = refuser("plugin");
  const { manifestValue, ...contents } = await readPackage(raw, PLUGIN_PACKAGE, maxBytes, signed);
  // Paths a device would write as one file, or a file where a folder must
  // be (amendment 42): refused as a device refuses them, after the
  // package's own checks and before the manifest's.
  const collision = firstCollision(contents.files.keys(), [MANIFEST_FILE, SOURCE_FILE]);
  if (collision) refuse("paths_collide", collision);
  const manifest = readManifest(manifestValue);
  if (manifest.problem) refuse("bad_manifest", manifest.problem);
  const badId = idProblem(manifest.id);
  if (badId) refuse("bad_id", badId);
  if (!contents.files.has(manifest.entry)) refuse("missing_entry", `Its entry file ${manifest.entry} is missing`);
  return { ...contents, manifest };
};

/** The template package `raw`, verified as template_packages.rs `parse`
 * verifies it: its id, then what a device leaves out (`ignored`) set aside,
 * then two paths of the rest that would be one file. Its other rules are
 * the listing's (listing.ts `checkTemplate`). */
export const verifyTemplate = async (raw: Uint8Array, maxBytes = MAX_PACKAGE_BYTES, signed = true): Promise<VerifiedTemplate> => {
  const refuse = refuser("template");
  const { manifestValue, ...contents } = await readPackage(raw, TEMPLATE_PACKAGE, maxBytes, signed);
  const text = (key: string): string => {
    const value = manifestValue.get(key);
    return typeof value === "string" ? rustTrim(value) : "";
  };
  const id = text("id");
  if (!validTemplateId(id) || id === ZERON_ID) refuse("bad_id", JSON.stringify(id.slice(0, 80)));
  const ignored = [...contents.files.keys()].filter((rel) => protectedPath(rel) || shortName(rel) || rel === TEMPLATE_MANIFEST_FILE || strayScreenshot(rel));
  const left = new Set(ignored);
  const kept = [...contents.files.keys()].filter((rel) => !left.has(rel));
  const collision = firstCollision(kept, [TEMPLATE_MANIFEST_FILE, TEMPLATE_IMPORTED_FILE]);
  if (collision) refuse("paths_collide", collision);
  return { ...contents, manifest: manifestValue, id, version: text("version"), ignored };
};

/**
 * What serde_json makes of `raw` as the engine reads a package from it
 * (for the differential tests, scan.test.ts): whether it is JSON at all,
 * and, for an object, its `format` if a string, its `manifest` in
 * canonical form if an object, and each of its `files` (if an object)
 * decoded from base64, or null where that fails. The same reading
 * `verifyPackage` does, without its limits.
 */
export const inspectPackageJson = (raw: Uint8Array): { doc: boolean; pkg: null | { format: string | null; manifest: string | null; files: [string, Uint8Array | null][] | null } } => {
  try {
    const whole = new Scanner(raw);
    whole.value();
    whole.finish();
  } catch (e) {
    if (e instanceof JsonError) return { doc: false, pkg: null };
    throw e;
  }
  const parts = readParts(raw);
  if (!parts) return { doc: true, pkg: null };
  const format = stringSpan(raw, parts.format);
  const manifest = parts.manifest && raw[parts.manifest.start] === 0x7b ? canonicalJson(parseJson(raw.subarray(parts.manifest.start, parts.manifest.end))) : null;
  let files: [string, Uint8Array | null][] | null = null;
  if (parts.files && raw[parts.files.start] === 0x7b) {
    const entries = new Map<string, Span | null>();
    const scanner = new Scanner(raw);
    scanner.at = parts.files.start;
    scanner.members(1, (ks, ke, vs, ve) => void entries.set(stringText(raw, ks, ke), stringSpan(raw, { start: vs, end: ve }) ?? null));
    files = [...entries.keys()].sort(byCodePoint).map((path) => {
      const span = entries.get(path);
      if (!span) return [path, null];
      const out = new Uint8Array(spanBound(span.start, span.end));
      const n = decodeBase64String(raw, span.start, span.end, out, 0);
      return [path, n < 0 ? null : out.subarray(0, n)];
    });
  }
  return { doc: true, pkg: { format: format ? stringText(raw, format.start, format.end) : null, manifest, files } };
};

