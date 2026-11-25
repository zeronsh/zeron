/**
 * What the store needs of a plugin beyond what a device needs (plan
 * "Listing metadata"), held to the engine's rules exactly
 * (crates/engine/src/plugins/listing.rs `check`, run on the device before
 * an upload and here on what arrives; docs/plugin-package-vectors.json
 * `listing` holds both to the same cases). Each problem carries the
 * engine's stable rule word, the field or path it's about, and words.
 *
 * On top, two limits of the store's own: README.md at most 512 KB and a
 * version's notes at most 64 KB (`too_large`), since both live in the
 * version's row.
 */
import { cleanImage } from "../image-check";
import { type Json, type JsonObject, canonicalJson, isArray, isObject } from "./json";
import { MANIFEST_FILE, type Manifest, RESERVED_ID_ERROR, idProblem, isRustWhitespace, rustTrim, rustTrimStart } from "./manifest";
import { MAX_BYTES, MAX_FILES, TEMPLATE_MANIFEST_FILE, TEMPLATE_SCREENSHOTS, insideTemplate, protectedPath, strayScreenshot, validTemplateId } from "./package";
import { LIMITS, STORE_DATA } from "./data";
import { asWritten, firstUnseen, keywordClaim, markClaim, problemWord } from "./mark";
import type { Kind } from "./kind";
import { foldPath, plainPath, shortName, unseenName } from "./paths";
import { type Range, type Version, parseRange, parseVersion } from "./semver";

/** The fixed list (contract amendment 4), with the words shown for each. */
export const CATEGORIES: readonly { readonly id: string; readonly name: string }[] = STORE_DATA.categories;
export const isCategory = (id: string): boolean => CATEGORIES.some((c) => c.id === id);

/** Zeron's own plugins' ids: nobody publishes them. */
export const BUILTIN_IDS: readonly string[] = STORE_DATA.builtin_ids;
/** The store site's own page words under `/plugins/` (listing.rs
 * `SITE_IDS`): no plugin is ever listed with one, and nothing lifts that,
 * a reservation included. Zeron itself runs such a plugin as any other. */
export const SITE_IDS: readonly string[] = STORE_DATA.site_ids;

export const README_FILE = "README.md";
export const CHANGELOG_FILE = "CHANGELOG.md";
export const MAX_CATEGORIES = LIMITS.max_categories;
export const MAX_KEYWORDS = LIMITS.max_keywords;
export const MAX_KEYWORD_LEN = LIMITS.max_keyword_len;
export const MAX_SCREENSHOTS = LIMITS.max_screenshots;
export const MAX_URL_LEN = LIMITS.max_url_len;
export const MAX_LICENSE_LEN = LIMITS.max_license_len;
export const MAX_NAME_LEN = LIMITS.max_name_len;
export const MAX_DESCRIPTION_LEN = LIMITS.max_description_len;
export const MAX_ICON_BYTES = LIMITS.max_icon_bytes;
export const SCREENSHOT_TYPES: readonly string[] = LIMITS.screenshot_types;
/** README.md and CHANGELOG.md, a version's notes, the canonical manifest
 * without its signature (amendments 22, 31). */
export const MAX_README_BYTES = LIMITS.max_readme_bytes;
export const MAX_CHANGELOG_BYTES = LIMITS.max_changelog_bytes;
export const MAX_MANIFEST_BYTES = LIMITS.max_manifest_bytes;
export const MAX_NOTES_BYTES = LIMITS.max_notes_bytes;
/** A template's preview, its card's cover: at most this many bytes. */
export const MAX_PREVIEW_BYTES = LIMITS.max_preview_bytes;
export const PREVIEW_FILE = "preview.png";
/** What a device writes beside a template's files (no file may fold to it). */
const TEMPLATE_IMPORTED_FILE = ".imported.json";
/** Members a template's manifest may not have in the store. */
const TEMPLATE_REFUSED_MEMBERS: readonly string[] = STORE_DATA.template_refused_members;
/** Every member it may have, or is refused for by name: any other but its
 * signature is refused too. */
const TEMPLATE_KNOWN_MEMBERS: ReadonlySet<string> = new Set([
  "signature",
  ...STORE_DATA.template_manifest_fields.required,
  ...STORE_DATA.template_manifest_fields.allowed,
  ...TEMPLATE_REFUSED_MEMBERS
]);
/** A README holding this sentence is the one Zeron wrote, unchanged. */
const TEMPLATE_README_STUB: string = STORE_DATA.template_readme_stub;

export type ListingRule =
  | "bad_package"
  | "id_reserved"
  | "not_semver"
  | "missing_readme"
  | "missing_listing"
  | "bad_listing"
  | "too_large"
  | "protected_file"
  | "binary_file"
  | "no_notes";

/** Why a name, description or keywords are refused (`listing_causes`): a
 * closed list an app says in its own words. */
export type ListingCause = "too_long" | "not_one_line" | "unseen_char" | "mark_word" | "mark_glyph" | "keyword_shape" | "too_many" | "repeated";

export interface Problem {
  readonly rule: ListingRule;
  readonly field: string | null;
  readonly path: string | null;
  readonly words: string;
  /** A bad name, description or keywords: why, and (where there is one)
   * the word at fault, cleaned (`problemWord`); the member a template's
   * manifest may not have. */
  readonly cause?: ListingCause;
  readonly word?: string;
}

const problem = (rule: ListingRule, field: string | null, words: string, path: string | null = null): Problem => ({ rule, field, path, words });
const because = (p: Problem, cause: ListingCause, word?: string): Problem => {
  const cleaned = problemWord(word);
  return { ...p, cause, ...(cleaned === undefined ? {} : { word: cleaned }) };
};
/** A mark claim's problem: its cause by whether the claim is a glyph, its
 * word where the text says it, as written (the claim when nowhere). */
const markProblem = (field: string, text: string, claim: string, words: string, item: Kind): Problem =>
  because(problem("bad_listing", field, words), [...claim].length === 1 && MARK_GLYPHS.includes(claim) ? "mark_glyph" : "mark_word", asWritten(text, claim, item) ?? claim);
/** The first character unseen refuses, as U+ and four or more hex digits. */
const unseenProblem = (field: string, text: string, words: string): Problem => {
  const c = firstUnseen(text);
  return because(problem("bad_listing", field, words), "unseen_char", c === undefined ? undefined : `U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}`);
};

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const lossy = new TextDecoder("utf-8", { fatal: false, ignoreBOM: true });
/** The bytes as UTF-8 text (a BOM kept, as Rust's `from_utf8` keeps it), or undefined. */
export const decodeText = (bytes: Uint8Array): string | undefined => {
  try {
    return utf8.decode(bytes);
  } catch {
    return undefined;
  }
};

const encoder = new TextEncoder();
const byteLength = (s: string): number => encoder.encode(s).length;
const chars = (s: string): number => [...s].length;
const isControl = (ch: string): boolean => {
  const c = ch.codePointAt(0)!;
  return c <= 0x1f || (c >= 0x7f && c <= 0x9f);
};

/** The mark's glyphs (data file `mark_claims.glyphs`). */
const MARK_GLYPHS: readonly string[] = STORE_DATA.mark_claims.glyphs;

/** The store's mark and claims of being Zeron's own (mark.ts: the engine's
 * mark.rs, ported). */
export { asWritten, firstUnseen, keywordClaim, markClaim, markText, problemWord } from "./mark";

/** (The fold of a path, as the store and devices compare paths: paths.ts.) */
export { foldPath } from "./paths";

/** A link's parsed text (amendment 31): it begins `https://`, holds no
 * backslash, whitespace, control character or `user:password@` part, and
 * parses as a URL; what's stored is the parser's text of it
 * (`https://example.com` → `https://example.com/`). `https:\\evil.com`,
 * which a browser reads as `https://evil.com/`, is refused. */
export const parsedUrl = (raw: string): string | undefined => {
  if (!raw.startsWith("https://") || raw.includes("\\") || [...raw].some((c) => isRustWhitespace(c) || isControl(c))) return undefined;
  try {
    const url = new URL(raw);
    return url.protocol === "https:" && url.username === "" && url.password === "" ? url.href : undefined;
  } catch {
    return undefined;
  }
};

/** listing.rs `https_url`. */
const httpsUrl = (url: string): boolean => {
  if (!url.startsWith("https://")) return false;
  const host = url.slice(8).split(/[/?#]/)[0] ?? "";
  return (
    byteLength(url) <= MAX_URL_LEN &&
    host !== "" &&
    !host.startsWith(".") &&
    host.includes(".") &&
    !/[@\\]/.test(host) &&
    ![...url].some((c) => isRustWhitespace(c) || isControl(c))
  );
};

/** listing.rs `keyword` (ASCII only: no control or format character). */
const keyword = (k: string): boolean =>
  k !== "" && byteLength(k) <= MAX_KEYWORD_LEN && rustTrim(k) === k && !k.includes("  ") && /^[a-z0-9 -]+$/.test(k);

/** listing.rs `is_screenshot`: a PNG or a JPEG, by its signature. */
const isScreenshot = (b: Uint8Array): boolean =>
  [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every((x, i) => b[i] === x) || (b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff);

/** listing.rs `is_picture`: a PNG, JPEG, GIF or WebP by its first bytes. */
const isPicture = (b: Uint8Array): boolean => {
  const starts = (sig: number[]) => sig.every((x, i) => b[i] === x);
  return (
    starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) ||
    starts([0xff, 0xd8, 0xff]) ||
    starts([0x47, 0x49, 0x46, 0x38, 0x37, 0x61]) ||
    starts([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]) ||
    (b.length >= 12 && starts([0x52, 0x49, 0x46, 0x46]) && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50)
  );
};

const COMPILED = [".luac", ".pyc", ".class", ".wasm", ".so", ".dylib", ".dll", ".exe", ".node", ".jar", ".zip"];

/** listing.rs `readable`: text or a picture; anything else is flagged. */
export const readable = (rel: string, bytes: Uint8Array, isText: (b: Uint8Array) => boolean): boolean => {
  const lower = rel.replace(/[A-Z]/g, (c) => c.toLowerCase());
  if (COMPILED.some((end) => lower.endsWith(end))) return false;
  return isPicture(bytes) || isText(bytes);
};

/** Rust's `str::lines`: split on `\n`, a `\r` before it dropped, no last
 * empty line. */
const lines = (text: string): string[] => {
  const out = text.split("\n").map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
  if (text.endsWith("\n")) out.pop();
  return out;
};

/** listing.rs `changelog_notes`: the section under the heading whose first
 * word is `version` (`1.2.0`, `v1.2.0`, `[1.2.0]`), up to the next heading as
 * high or higher; trimmed; undefined when there's none or it's empty. */
export const changelogNotes = (changelog: string, version: string): string | undefined => {
  const heading = (line: string): [number, string] | undefined => {
    let level = 0;
    while (line[level] === "#") level++;
    if (level < 1 || level > 6 || line[level] !== " ") return undefined;
    const words = [...line.slice(level)].reduce<string[]>((acc, ch) => {
      if (isRustWhitespace(ch)) acc.push("");
      else acc[acc.length - 1] += ch;
      return acc;
    }, [""]).filter((w) => w !== "");
    let first = (words[0] ?? "").replace(/^\[+/, "").split("]")[0] ?? "";
    if (first.startsWith("v") || first.startsWith("V")) first = first.slice(1);
    return [level, first];
  };
  const all = lines(changelog);
  let i = 0;
  let level = 0;
  for (;; i++) {
    if (i >= all.length) return undefined;
    const h = heading(all[i]!);
    if (h && h[1] === version) {
      level = h[0];
      i++;
      break;
    }
  }
  const section: string[] = [];
  for (; i < all.length; i++) {
    const h = heading(all[i]!);
    if (h && h[0] <= level) break;
    section.push(all[i]!);
  }
  const notes = rustTrim(section.join("\n"));
  return notes === "" ? undefined : notes;
};

/** Rust's `String` order: by code point (UTF-8's byte order). */
const byCodePoint = (a: string, b: string): number => {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    const x = a.codePointAt(i)!;
    const y = b.codePointAt(i)!;
    if (x !== y) return x - y;
    if (x > 0xffff) i++;
  }
  return a.length - b.length;
};

const strings = (items: readonly Json[]): (string | undefined)[] => items.map((v) => (typeof v === "string" ? v : undefined));
const duplicates = (items: readonly (string | undefined)[]): boolean => new Set(items).size !== items.length;

/** listing.rs `zeron_range`: the range written, none, or why it's malformed. */
export const zeronRange = (raw: ReadonlyMap<string, Json>): { range: string | null } | { error: string } => {
  const engines = raw.get("engines");
  if (engines === undefined || engines === null) return { range: null };
  if (!isObject(engines)) return { error: 'Its "engines" must be an object ("engines": { "zeron": ">=0.4.0" })' };
  const zeron = engines.get("zeron");
  if (zeron === undefined || zeron === null) return { range: null };
  return typeof zeron === "string" ? { range: zeron } : { error: 'Its "engines.zeron" must be a range of versions, as text' };
};

/** `id_reserved` when the store keeps `id` (one namespace: a plugin's or a
 * template's alike). */
const reservedId = (id: string, problems: Problem[]): void => {
  if (SITE_IDS.includes(id)) problems.push(problem("id_reserved", "id", `"${id}" is kept for the store's own pages: pick another id`));
  else if (BUILTIN_IDS.includes(id) || id.startsWith("zeron") || idProblem(id) === RESERVED_ID_ERROR) {
    problems.push(problem("id_reserved", "id", `"${id}" is kept for Zeron`));
  }
};

/** Its name: at most `MAX_NAME_LEN`, nothing drawn as nothing, no mark. */
const checkName = (name: string, item: Kind, problems: Problem[]): void => {
  if (chars(name) > MAX_NAME_LEN) problems.push(because(problem("bad_listing", "name", `Its name is longer than ${MAX_NAME_LEN} characters`), "too_long"));
  else if (firstUnseen(name) !== undefined) problems.push(unseenProblem("name", name, "Its name has a control or invisible formatting character"));
  else {
    const claim = markClaim(name, true, item);
    if (claim !== undefined) {
      problems.push(markProblem("name", name, claim, `Its name says "${claim}": only the store marks a ${item} verified or official. Name it for what it does, without that word`, item));
    }
  }
};

/** Its page: a README.md of text, at most `MAX_README_BYTES`; never one
 * holding `stub` (the one Zeron wrote, unchanged). */
const checkReadme = (files: ReadonlyMap<string, Uint8Array>, stub: string | undefined, problems: Problem[]): void => {
  const readme = files.get(README_FILE);
  if (!readme) problems.push(problem("missing_readme", null, "It needs a README.md: its page in the store"));
  else if (readme.length > MAX_README_BYTES) problems.push(problem("too_large", null, `Its README.md is at most ${MAX_README_BYTES / 1024} KB`, README_FILE));
  else {
    const text = decodeText(readme);
    if (text !== undefined && stub !== undefined && text.includes(stub)) {
      problems.push(problem("missing_readme", null, "Its README.md is still the one Zeron wrote: say what it is and how it looks, for its page in the store", README_FILE));
    } else if (text === undefined || rustTrim(text) === "") problems.push(problem("missing_readme", null, "Its README.md is empty or isn't text", README_FILE));
  }
};

/** What the store shows on its card: one line, at most
 * `MAX_DESCRIPTION_LEN`, nothing drawn as nothing, no mark said of it. */
const checkDescription = (written: string, item: Kind, problems: Problem[]): void => {
  const description = rustTrim(written);
  if (description === "") problems.push(problem("missing_listing", "description", 'It needs a "description": one line saying what it does'));
  else if (chars(description) > MAX_DESCRIPTION_LEN || description.includes("\n")) {
    const why: ListingCause = chars(description) > MAX_DESCRIPTION_LEN ? "too_long" : "not_one_line";
    problems.push(because(problem("bad_listing", "description", `Its description is one line of at most ${MAX_DESCRIPTION_LEN} characters`), why));
  } else if (firstUnseen(description) !== undefined) problems.push(unseenProblem("description", description, "Its description has a control or invisible formatting character"));
  else {
    const claim = markClaim(description, false, item);
    if (claim !== undefined) {
      problems.push(
        markProblem(
          "description",
          description,
          claim,
          `Its description says "${claim}" of the ${item} itself: only the store marks a ${item} verified or official. Say what it does instead, or say whose thing it is ("works with the official Slack app")`,
          item
        )
      );
    }
  }
};

/** Its keywords: at most `MAX_KEYWORDS`, each once, each a keyword, none
 * saying the mark. */
const checkKeywords = (raw: ReadonlyMap<string, Json>, item: Kind, problems: Problem[]): void => {
  const keywords = raw.get("keywords");
  if (isArray(keywords)) {
    const words = strings(keywords);
    if (keywords.length > MAX_KEYWORDS || duplicates(words)) {
      const bad = problem("bad_listing", "keywords", `It has at most ${MAX_KEYWORDS} keywords, each once`);
      // (The first keyword given again; none when it isn't text.)
      const twice = words.find((w, at) => words.slice(0, at).includes(w));
      problems.push(keywords.length > MAX_KEYWORDS ? because(bad, "too_many") : because(bad, "repeated", twice));
    } else if (words.some((w) => w === undefined || !keyword(w))) {
      const bad = words.find((w) => w === undefined || !keyword(w));
      problems.push(
        because(problem("bad_listing", "keywords", `Keyword "${bad ?? "?"}" must be lowercase letters, digits, dashes and single spaces, at most ${MAX_KEYWORD_LEN} characters`), "keyword_shape", bad)
      );
    } else {
      // (No badge lifts this; "zeron" in a keyword is anyone's.)
      for (const w of words as string[]) {
        const claim = keywordClaim(w);
        if (claim === undefined) continue;
        problems.push(
          because(problem("bad_listing", "keywords", `Keyword "${w}" says "${claim}": only the store marks a ${item} verified or official. Drop it, or use a word for what the ${item} does`), "mark_word", w)
        );
        break;
      }
    }
  } else if (keywords !== undefined && keywords !== null) problems.push(problem("bad_listing", "keywords", 'Its "keywords" must be a list'));
};

/** Its `homepage` and `repository` (https links) and `license` (a short name). */
const checkLinks = (raw: ReadonlyMap<string, Json>, problems: Problem[]): void => {
  for (const field of ["homepage", "repository"]) {
    const url = raw.get(field);
    if (url === undefined || url === null || (typeof url === "string" && httpsUrl(url) && parsedUrl(url) !== undefined)) continue;
    problems.push(problem("bad_listing", field, `Its "${field}" must be an https:// link`));
  }
  const license = raw.get("license");
  if (!(license === undefined || license === null || (typeof license === "string" && rustTrim(license) !== "" && byteLength(license) <= MAX_LICENSE_LEN && ![...license].some(isControl)))) {
    problems.push(problem("bad_listing", "license", `Its "license" is a short name, like MIT (at most ${MAX_LICENSE_LEN} characters)`));
  }
};

/** Its screenshots: at most `MAX_SCREENSHOTS`, each once, each a PNG or
 * JPEG in it (under `folder`, when its pictures must be there). */
const checkScreenshots = (raw: ReadonlyMap<string, Json>, files: ReadonlyMap<string, Uint8Array>, folder: string | undefined, problems: Problem[]): void => {
  const screenshots = raw.get("screenshots");
  if (isArray(screenshots)) {
    const paths = strings(screenshots);
    if (screenshots.length > MAX_SCREENSHOTS || duplicates(paths)) problems.push(problem("bad_listing", "screenshots", `It has at most ${MAX_SCREENSHOTS} screenshots, each once`));
    for (const path of paths) {
      if (path === undefined) {
        problems.push(problem("bad_listing", "screenshots", "Each screenshot is the path of a picture in it"));
        continue;
      }
      const dot = path.lastIndexOf(".");
      const typed = dot >= 0 && SCREENSHOT_TYPES.includes(path.slice(dot + 1).replace(/[A-Z]/g, (c) => c.toLowerCase()));
      const there = plainPath(path) && (folder === undefined || path.startsWith(`${folder}/`));
      const bytes = there ? files.get(path) : undefined;
      if (!typed || !bytes || !isScreenshot(bytes)) {
        problems.push(
          problem("bad_listing", "screenshots", folder === undefined ? "A screenshot must be a PNG or JPEG picture in it" : `A screenshot must be a PNG or JPEG picture in its ${folder}/ folder`, path)
        );
      }
    }
  } else if (screenshots !== undefined && screenshots !== null) problems.push(problem("bad_listing", "screenshots", 'Its "screenshots" must be a list'));
};

/** How big: at most `MAX_FILES` files and `MAX_BYTES`. */
const checkSize = (files: ReadonlyMap<string, Uint8Array>, item: Kind, problems: Problem[]): void => {
  let total = 0;
  for (const bytes of files.values()) total += bytes.length;
  if (files.size > MAX_FILES || total > MAX_BYTES) problems.push(problem("too_large", null, `It's too big for a ${item} (2,000 files, 50 MB)`));
};

const checkChangelogSize = (files: ReadonlyMap<string, Uint8Array>, problems: Problem[]): void => {
  const changelog = files.get(CHANGELOG_FILE);
  if (changelog && changelog.length > MAX_CHANGELOG_BYTES) problems.push(problem("too_large", null, "CHANGELOG.md is over 512 KB", CHANGELOG_FILE));
};

/** The manifest as written, without the signature, which the store keeps
 * apart: at most `MAX_MANIFEST_BYTES`. */
const checkManifestSize = (raw: JsonObject, problems: Problem[]): void => {
  const unsigned: JsonObject = new Map(raw);
  unsigned.delete("signature");
  if (byteLength(canonicalJson(unsigned)) > MAX_MANIFEST_BYTES) problems.push(problem("too_large", "manifest", "Its manifest is over 256 KB"));
};

/** Worth knowing: files that aren't text or a picture (warnings), and
 * `version`'s notes in CHANGELOG.md (too long is a problem; none, a warning). */
const worthKnowing = (files: ReadonlyMap<string, Uint8Array>, version: string, isText: (bytes: Uint8Array) => boolean, problems: Problem[], warnings: Problem[]): void => {
  for (const [rel, bytes] of [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (!readable(rel, bytes, isText)) warnings.push(problem("binary_file", null, "It isn't text or a picture: reviewers look closely at files like this", rel));
  }
  const changelog = files.get(CHANGELOG_FILE);
  const notes = changelog && changelog.length <= MAX_CHANGELOG_BYTES ? changelogNotes(lossy.decode(changelog), version) : undefined;
  if (notes !== undefined && byteLength(notes) > MAX_NOTES_BYTES) {
    problems.push(problem("too_large", null, `This version's notes in CHANGELOG.md are at most ${MAX_NOTES_BYTES / 1024} KB`, CHANGELOG_FILE));
  }
  if (changelog && changelog.length <= MAX_CHANGELOG_BYTES && notes === undefined) {
    warnings.push(problem("no_notes", null, `CHANGELOG.md has no section for ${version}: this version shows without notes`, CHANGELOG_FILE));
  }
};

/** Two paths that are one file on a Mac or a Windows machine (`taken`,
 * what's written beside them, among them), or a file where another path
 * needs a folder of that name there (amendment 42): each at the later path,
 * in code point order; a path with a problem is never recorded. */
const checkClashes = (files: ReadonlyMap<string, Uint8Array>, taken: readonly string[], problems: Problem[]): void => {
  const folded = new Map<string, string>(taken.map((rel) => [foldPath(rel), rel]));
  const folders = new Map<string, string>();
  for (const rel of [...files.keys()].sort(byCodePoint)) {
    const key = foldPath(rel);
    const other = folded.get(key);
    if (other !== undefined) {
      problems.push(problem("bad_listing", "files", `"${rel}" and "${other}" are the same file on macOS and Windows`, rel));
      continue;
    }
    const inside = folders.get(key);
    if (inside !== undefined) {
      problems.push(problem("bad_listing", "files", `"${rel}" is a file where "${inside}" needs a folder of that name on macOS and Windows`, rel));
      continue;
    }
    const parts = key.split("/");
    const parents = parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
    const clash = parents.map((folder) => folded.get(folder)).find((file) => file !== undefined);
    if (clash !== undefined) {
      problems.push(problem("bad_listing", "files", `"${clash}" is a file where "${rel}" needs a folder of that name on macOS and Windows`, rel));
      continue;
    }
    // (Only a path with no problem is recorded, as the engine's `clashes`.)
    for (const folder of parents) if (!folders.has(folder)) folders.set(folder, rel);
    folded.set(key, rel);
  }
};

/** listing.rs `check`: what stands in the way of publishing, and what's
 * worth knowing, each once, in the engine's order. */
export const checkListing = (
  manifest: Manifest,
  files: ReadonlyMap<string, Uint8Array>,
  isText: (bytes: Uint8Array) => boolean
): { problems: Problem[]; warnings: Problem[] } => {
  const problems: Problem[] = [];
  const warnings: Problem[] = [];
  const raw = manifest.raw;
  if (manifest.problem) problems.push(problem("bad_package", "manifest", manifest.problem));
  const idError = idProblem(manifest.id);
  if (idError && idError !== RESERVED_ID_ERROR) problems.push(problem("bad_package", "id", idError));
  else reservedId(manifest.id, problems);
  if (!parseVersion(manifest.version)) problems.push(problem("not_semver", "version", `"${manifest.version}" isn't a version (MAJOR.MINOR.PATCH)`));
  checkName(manifest.name, "plugin", problems);
  if (manifest.entry !== "" && !files.has(manifest.entry)) problems.push(problem("bad_package", "entry", `Its entry file ${manifest.entry} is missing`));
  checkReadme(files, undefined, problems);
  checkDescription(manifest.description, "plugin", problems);
  const icon = raw.get("icon");
  if (icon === undefined || icon === null) problems.push(problem("missing_listing", "icon", 'It needs an "icon": an SVG file in it'));
  else if (typeof icon === "string") {
    const bytes = files.get(icon);
    const text = bytes && bytes.length <= MAX_ICON_BYTES ? decodeText(bytes) : undefined;
    if (!(icon.endsWith(".svg") && text !== undefined && rustTrimStart(text).startsWith("<svg"))) {
      problems.push(problem("bad_listing", "icon", `Its icon must be an SVG file in it, at most ${MAX_ICON_BYTES / 1024} KB`, icon));
    }
  } else problems.push(problem("bad_listing", "icon", 'Its "icon" must be the path of an SVG file in it'));

  const categories = raw.get("categories");
  if (categories === undefined || categories === null) problems.push(problem("missing_listing", "categories", 'It needs a category ("categories": ["productivity"])'));
  else if (isArray(categories)) {
    const names = strings(categories);
    if (categories.length === 0) problems.push(problem("missing_listing", "categories", "It needs a category"));
    else if (categories.length > MAX_CATEGORIES || duplicates(names)) problems.push(problem("bad_listing", "categories", `It's in 1 or ${MAX_CATEGORIES} categories, each once`));
    else {
      const bad = names.find((n) => n === undefined || !isCategory(n));
      if (names.some((n) => n === undefined || !isCategory(n))) {
        problems.push(problem("bad_listing", "categories", `"${bad ?? "?"}" isn't one of the store's categories (${CATEGORIES.map((c) => c.id).join(", ")})`));
      }
    }
  } else problems.push(problem("bad_listing", "categories", 'Its "categories" must be a list'));

  checkKeywords(raw, "plugin", problems);
  checkLinks(raw, problems);
  checkScreenshots(raw, files, undefined, problems);

  const range = zeronRange(raw);
  if ("error" in range) problems.push(problem("bad_listing", "engines", range.error));
  else if (range.range !== null && !parseRange(range.range)) problems.push(problem("bad_listing", "engines", `"${range.range}" isn't a range of versions`));

  checkSize(files, "plugin", problems);
  worthKnowing(files, manifest.version, isText, problems, warnings);
  // Amendment 31's rules, after the engine's own.
  checkChangelogSize(files, problems);
  checkManifestSize(raw, problems);
  // A name that could be another of its files' (a device refuses the package
  // as it reads it).
  for (const rel of [...files.keys()].sort(byCodePoint)) {
    if (shortName(rel)) problems.push(problem("bad_package", "files", `"${rel}" could be a Windows short name of another file (a ~ with a digit after it): rename it`, rel));
    else if (unseenName(rel)) problems.push(problem("bad_package", "files", `"${rel}" has a control or invisible character in its name: rename it`, rel));
  }
  checkClashes(files, [MANIFEST_FILE], problems);
  return { problems, warnings };
};

/** A PNG's first bytes. */
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/** Whether a PNG has an animation control chunk (`acTL`), read chunk by chunk
 * while a whole chunk header remains (listing.rs `animated_png`): a cover is
 * a still. */
const isAnimatedPng = (bytes: Uint8Array): boolean => {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (let at = PNG_SIGNATURE.length; at + 8 <= bytes.length; at += 12 + view.getUint32(at)) {
    if (view.getUint32(at + 4) === 0x6163544c) return true;
  }
  return false;
};

/** listing.rs `check_template`: what stands in the way of publishing a
 * template with `manifest` (its own, as the package carries it, which the
 * signature isn't part of here) and `files` (every file the package
 * carries), and what's worth knowing, each once, in the engine's order. A
 * template asks for nothing (it runs as Zeron), so its rules are the
 * listing's, its files' and its preview's. */
export const checkTemplate = (
  raw: JsonObject,
  files: ReadonlyMap<string, Uint8Array>,
  isText: (bytes: Uint8Array) => boolean
): { problems: Problem[]; warnings: Problem[] } => {
  const problems: Problem[] = [];
  const warnings: Problem[] = [];
  for (const member of TEMPLATE_REFUSED_MEMBERS) {
    if (raw.has(member)) problems.push(problem("bad_listing", member, `A template has no "${member}": it asks for nothing, and runs as Zeron itself`));
  }
  for (const member of [...raw.keys()].filter((key) => !TEMPLATE_KNOWN_MEMBERS.has(key)).sort(byCodePoint)) {
    const word = problemWord(member);
    problems.push({ ...problem("bad_listing", "manifest", `"${word ?? ""}" isn't something a template's manifest says in the store: leave it out`), ...(word === undefined ? {} : { word }) });
  }
  const text = (key: string): string | undefined => {
    const value = raw.get(key);
    return typeof value === "string" ? rustTrim(value) : undefined;
  };
  const id = text("id") ?? "";
  if (!validTemplateId(id)) {
    problems.push(problem("bad_package", "id", `"${id}" can't be a template's id: use 2 to 40 lowercase letters, digits and dashes, starting with a letter or digit`));
  } else reservedId(id, problems);
  const version = text("version") ?? "";
  if (!parseVersion(version)) problems.push(problem("not_semver", "version", `"${version}" isn't a version (MAJOR.MINOR.PATCH)`));
  const madeFrom = text("zeron_version");
  if (madeFrom === undefined || madeFrom === "") problems.push(problem("missing_listing", "zeron_version", 'It needs a "zeron_version": the Zeron it was made from'));
  else if (!parseVersion(madeFrom)) problems.push(problem("not_semver", "zeron_version", `"${madeFrom}" isn't a version (MAJOR.MINOR.PATCH)`));
  const name = text("name");
  if (name === undefined || name === "") problems.push(problem("missing_listing", "name", 'It needs a "name"'));
  else checkName(name, "template", problems);
  checkReadme(files, TEMPLATE_README_STUB, problems);
  checkDescription(text("description") ?? "", "template", problems);
  const preview = files.get(PREVIEW_FILE);
  if (!preview) problems.push(problem("missing_listing", "preview", "It needs a preview.png: its card's cover in the store"));
  else if (preview.length > MAX_PREVIEW_BYTES || !PNG_SIGNATURE.every((b, i) => preview[i] === b) || isAnimatedPng(preview)) {
    problems.push(problem("bad_listing", "preview", `Its preview.png must be a still PNG picture, at most ${MAX_PREVIEW_BYTES / (1024 * 1024)} MB`, PREVIEW_FILE));
  }
  checkKeywords(raw, "template", problems);
  checkLinks(raw, problems);
  checkScreenshots(raw, files, TEMPLATE_SCREENSHOTS, problems);
  checkChangelogSize(files, problems);
  checkManifestSize(raw, problems);
  // Its files: each one Zeron writes, none of those no template replaces.
  for (const rel of [...files.keys()].sort(byCodePoint)) {
    if (!insideTemplate(rel) || unseenName(rel)) problems.push(problem("bad_package", "files", `It has a file Zeron won't write: "${rel}"`, rel));
    else if (protectedPath(rel)) {
      problems.push(problem("protected_file", "files", `"${rel}" is one of Zeron's own files no template replaces (the plugins and templates pages, your account, .luaurc, ds/themes/template-dark.json and template-light.json, any tokens but ds/tokens.luau): leave it out`, rel));
    } else if (shortName(rel)) {
      problems.push(problem("protected_file", "files", `"${rel}" could be a Windows short name of one of Zeron's own files (a ~ with a digit after it): rename it`, rel));
    } else if (strayScreenshot(rel)) {
      problems.push(problem("bad_listing", "files", `"${rel}" isn't a PNG or JPEG: ${TEMPLATE_SCREENSHOTS}/ holds its pictures alone`, rel));
    } else if (rel.startsWith(`${TEMPLATE_SCREENSHOTS}/`) && !isScreenshot(files.get(rel)!)) {
      problems.push(problem("bad_listing", "files", `"${rel}" isn't a PNG or JPEG picture, whatever its name says`, rel));
    }
  }
  checkClashes(files, [TEMPLATE_MANIFEST_FILE, TEMPLATE_IMPORTED_FILE], problems);
  checkSize(files, "template", problems);
  worthKnowing(files, version, isText, problems, warnings);
  return { problems, warnings };
};

/** A template's preview, past the device's check: a picture the store will
 * serve (image-check's), so its card's cover is never a 404. Undefined when
 * it is. */
export const previewProblem = (files: ReadonlyMap<string, Uint8Array>): Problem | undefined => {
  const preview = files.get(PREVIEW_FILE);
  if (preview && pictureType(preview) === "image/png") return undefined;
  return problem("bad_listing", "preview", "Its preview.png isn't a PNG picture the store can show", PREVIEW_FILE);
};

/** What the store keeps of a listing that passed `checkListing`. */
export interface Listing {
  readonly name: string;
  readonly summary: string;
  readonly icon: string;
  readonly categories: readonly string[];
  readonly keywords: readonly string[];
  readonly homepage: string | null;
  readonly repository: string | null;
  readonly license: string | null;
  readonly screenshots: readonly string[];
  readonly engines: string | null;
  readonly enginesRange: Range | null;
  readonly readme: string;
  readonly notes: string | null;
  readonly version: Version;
}

/** The listing of a manifest `checkListing` found no problem in, or the
 * store's own size limits it breaks. */
export const listingOf = (manifest: Manifest, files: ReadonlyMap<string, Uint8Array>): Listing | Problem => {
  const raw = manifest.raw;
  const readmeBytes = files.get(README_FILE)!;
  const changelog = files.get(CHANGELOG_FILE);
  const notes = changelog ? (changelogNotes(lossy.decode(changelog), manifest.version) ?? null) : null;
  const text = (key: string) => {
    const v = raw.get(key);
    return typeof v === "string" ? v : null;
  };
  const list = (key: string) => {
    const v = raw.get(key);
    return isArray(v) ? (v as string[]) : [];
  };
  const range = zeronRange(raw);
  const engines = "range" in range ? range.range : null;
  return {
    name: manifest.name,
    summary: rustTrim(manifest.description),
    icon: text("icon")!,
    categories: list("categories"),
    keywords: list("keywords"),
    homepage: text("homepage") === null ? null : (parsedUrl(text("homepage")!) ?? null),
    repository: text("repository") === null ? null : (parsedUrl(text("repository")!) ?? null),
    license: text("license"),
    screenshots: list("screenshots"),
    engines,
    enginesRange: engines === null ? null : (parseRange(engines) ?? null),
    readme: decodeText(readmeBytes)!.replace(/^\uFEFF/, ""),
    notes,
    version: parseVersion(manifest.version)!
  };
};

/** The listing of a template `checkTemplate` found no problem in: its icon
 * and categories empty (a template has neither), and the Zerons it's for
 * `>=` the one it was made from. */
export const listingOfTemplate = (raw: JsonObject, files: ReadonlyMap<string, Uint8Array>): Listing => {
  const text = (key: string): string | undefined => {
    const value = raw.get(key);
    return typeof value === "string" ? value : undefined;
  };
  const trimmed = (key: string): string => rustTrim(text(key) ?? "");
  const list = (key: string): string[] => {
    const value = raw.get(key);
    return isArray(value) ? (value as string[]) : [];
  };
  const version = trimmed("version");
  const changelog = files.get(CHANGELOG_FILE);
  const engines = `>=${trimmed("zeron_version")}`;
  const link = (key: string): string | null => (text(key) === undefined ? null : (parsedUrl(text(key)!) ?? null));
  return {
    name: trimmed("name"),
    summary: trimmed("description"),
    icon: "",
    categories: [],
    keywords: list("keywords"),
    homepage: link("homepage"),
    repository: link("repository"),
    license: text("license") ?? null,
    screenshots: list("screenshots"),
    engines,
    enginesRange: parseRange(engines) ?? null,
    readme: decodeText(files.get(README_FILE)!)!.replace(/^\uFEFF/, ""),
    notes: changelog ? (changelogNotes(lossy.decode(changelog), version) ?? null) : null,
    version: parseVersion(version)!
  };
};

export const pictureType = (bytes: Uint8Array): "image/png" | "image/jpeg" | undefined => {
  if (bytes[0] === 0x89 && cleanImage("image/png", bytes)) return "image/png";
  if (bytes[0] === 0xff && cleanImage("image/jpeg", bytes)) return "image/jpeg";
  return undefined;
};
