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
import { MAX_BYTES, MAX_FILES } from "./package";
import { LIMITS, STORE_DATA } from "./data";
import { MARK_LOOKALIKES } from "./mark-lookalikes";
import { foldPath, plainPath } from "./paths";
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

export type ListingRule =
  | "bad_package"
  | "id_reserved"
  | "not_semver"
  | "missing_readme"
  | "missing_listing"
  | "bad_listing"
  | "too_large"
  | "binary_file"
  | "no_notes";

export interface Problem {
  readonly rule: ListingRule;
  readonly field: string | null;
  readonly path: string | null;
  readonly words: string;
}

const problem = (rule: ListingRule, field: string | null, words: string, path: string | null = null): Problem => ({ rule, field, path, words });

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

/** A character that may be drawn as nothing, which `name` and
 * `description` refuse (amendment 31; `keywords` are ASCII): a control or
 * format character (Cc, Cf: bidi controls among them), any other
 * Default_Ignorable_Code_Point (the Hangul fillers, which are letters), and
 * any variation selector but U+FE0F directly after a character with the
 * Emoji property ("⭐️"). */
const UNSEEN = /[\p{Cc}\p{Cf}]|(?!\p{Variation_Selector})\p{Default_Ignorable_Code_Point}|(?!\u{fe0f})\p{Variation_Selector}|(?<!\p{Emoji})\u{fe0f}/u;
/** Every default-ignorable character, variation selectors included, and
 * every enclosing mark (Me: a keycap's, a circle's), which the fold keeps:
 * gone before the mark's word test, so none can split a word. */
const IGNORABLE = /[\p{Default_Ignorable_Code_Point}\p{Me}]/gu;

/**
 * The store's mark (amendment 45): a listing's name or description may not
 * claim it. The shared rule (data file `mark_claims`, listing.rs
 * `mark_claim`): the text read as NFKC, lowercased, then NFKD without its
 * nonspacing marks (`markText`); a claim is any of the glyphs anywhere, or
 * one of the words with no ASCII letter or digit right before or after it.
 */
const MARK_GLYPHS: readonly string[] = STORE_DATA.mark_claims.glyphs;
const MARK_WORDS: readonly string[] = STORE_DATA.mark_claims.words;
const ASCII_WORD = /[A-Za-z0-9]/;

export const markText = (text: string): string =>
  text.normalize("NFKC").toLowerCase().normalize("NFKD").replace(/\p{Mn}/gu, "");

/** The first of the mark's words in `folded` as a whole word, if any. */
const markWord = (folded: string): string | undefined =>
  MARK_WORDS.find((word) => {
    for (let at = folded.indexOf(word); at !== -1; at = folded.indexOf(word, at + 1)) {
      const before = folded[at - 1];
      const after = folded[at + word.length];
      if (!(before !== undefined && ASCII_WORD.test(before)) && !(after !== undefined && ASCII_WORD.test(after))) return true;
    }
    return false;
  });

/**
 * The edge's own reading (a device's check has none: the vectors' cases
 * with `edge_problems`): a mark word spelled with characters that pass for
 * its letters, from Unicode's confusables (UTS #39) plus the Latin small
 * capitals and the digit 3 (mark-lookalikes.ts, generated). `i` and `l`,
 * and so their look-alikes, stand for each other (capital I and lowercase l
 * are one stroke in sans-serif; UTS #39 folds I to l). A letter may also
 * be two that pass for it together (`LOOKALIKE_PAIRS`). Whole words, as
 * the shared rule's.
 */
const LOOKALIKE_PAIRS: Readonly<Record<string, readonly string[]>> = { d: ["cl"] };
const lookalikeChars = (letter: string): string[] => [letter, ...(MARK_LOOKALIKES[letter] ?? "")];
const charClass = (chars: readonly string[]): string => `[${chars.map((ch) => `\\u{${ch.codePointAt(0)!.toString(16)}}`).join("")}]`;
const letterPattern = (letter: string): string => {
  const chars = letter === "i" || letter === "l" ? [...lookalikeChars("i"), ...lookalikeChars("l")] : lookalikeChars(letter);
  const pairs = (LOOKALIKE_PAIRS[letter] ?? []).map((pair) => [...pair].map((ch) => charClass(lookalikeChars(ch))).join(""));
  return pairs.length ? `(?:${[charClass(chars), ...pairs].join("|")})` : charClass(chars);
};
const LOOKALIKE_WORDS: readonly (readonly [string, RegExp])[] = MARK_WORDS.map(
  (word) => [word, new RegExp(`(?<![A-Za-z0-9])${[...word].map(letterPattern).join("")}(?![A-Za-z0-9])`, "u")] as const
);
/** `text` with its runs of single characters, each apart by up to three
 * spaces or punctuation marks, read as one word ("V E R I F I E D",
 * "V.E.R.I.F.I.E.D.", "V. E. R. I. F. I. E. D.", "V·E·R·I·F·I·E·D"). */
const SPACED = /(?<![^\s\p{P}])[^\s\p{P}](?:[\s\p{P}]{1,3}[^\s\p{P}])+(?![^\s\p{P}])/gu;
const unspaced = (text: string): string => text.replace(SPACED, (run) => run.replace(/[\s\p{P}]/gu, ""));

/** Ticks the shared glyphs leave out, refused on the edge: √ (used as one)
 * and 𐄂 AEGEAN CHECK MARK. */
const EDGE_GLYPHS: readonly string[] = ["\u{221a}", "\u{10102}"];

/**
 * What in `text` claims the store's mark, if anything: the shared rule's
 * glyph or word, or (`lookalikes`, the edge's alone) one of its own ticks
 * or a mark word spelled with look-alike characters ("Vеrified" with a
 * Cyrillic е, "ᴠᴇʀɪꜰɪᴇᴅ", "VER1F1ED", "OFFlClAL", "V.E.R.I.F.I.E.D.").
 */
export const markClaim = (text: string, lookalikes = true): string | undefined => {
  // Nothing drawn between its letters, or around one, splits a word.
  const folded = markText(text).replace(IGNORABLE, "");
  const glyph = [...folded].find((ch) => MARK_GLYPHS.includes(ch) || (lookalikes && EDGE_GLYPHS.includes(ch)));
  if (glyph !== undefined) return glyph;
  const word = markWord(folded);
  if (word !== undefined || !lookalikes) return word;
  // Nor spaces between single letters.
  const readings = [folded, unspaced(folded)];
  return LOOKALIKE_WORDS.find(([, re]) => readings.some((r) => re.test(r)))?.[0];
};

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

/** listing.rs `check`: what stands in the way of publishing, and what's
 * worth knowing, each once, in the engine's order. */
export const checkListing = (
  manifest: Manifest,
  files: ReadonlyMap<string, Uint8Array>,
  isText: (bytes: Uint8Array) => boolean,
  /** The edge's look-alike reading of the mark's words (`markClaim`); off,
   * the device's check exactly. */
  lookalikes = true
): { problems: Problem[]; warnings: Problem[] } => {
  const problems: Problem[] = [];
  const warnings: Problem[] = [];
  const raw = manifest.raw;
  if (manifest.problem) problems.push(problem("bad_package", "manifest", manifest.problem));
  const idError = idProblem(manifest.id);
  if (idError && idError !== RESERVED_ID_ERROR) problems.push(problem("bad_package", "id", idError));
  else if (BUILTIN_IDS.includes(manifest.id) || SITE_IDS.includes(manifest.id) || manifest.id.startsWith("zeron") || idError === RESERVED_ID_ERROR) {
    problems.push(problem("id_reserved", "id", `"${manifest.id}" is kept for Zeron`));
  }
  if (!parseVersion(manifest.version)) problems.push(problem("not_semver", "version", `"${manifest.version}" isn't a version (MAJOR.MINOR.PATCH)`));
  if (chars(manifest.name) > MAX_NAME_LEN) problems.push(problem("bad_listing", "name", `Its name is longer than ${MAX_NAME_LEN} characters`));
  else if (UNSEEN.test(manifest.name)) problems.push(problem("bad_listing", "name", "Its name has a control or invisible formatting character"));
  else {
    const claim = markClaim(manifest.name, lookalikes);
    if (claim !== undefined) problems.push(problem("bad_listing", "name", `Its name says "${claim}": only the store marks a plugin verified`));
  }
  if (manifest.entry !== "" && !files.has(manifest.entry)) problems.push(problem("bad_package", "entry", `Its entry file ${manifest.entry} is missing`));

  const readme = files.get(README_FILE);
  if (!readme) problems.push(problem("missing_readme", null, "It needs a README.md: its page in the store"));
  else if (readme.length > MAX_README_BYTES) problems.push(problem("too_large", null, `Its README.md is at most ${MAX_README_BYTES / 1024} KB`, README_FILE));
  else {
    const text = decodeText(readme);
    if (text === undefined || rustTrim(text) === "") problems.push(problem("missing_readme", null, "Its README.md is empty or isn't text", README_FILE));
  }

  const description = rustTrim(manifest.description);
  if (description === "") problems.push(problem("missing_listing", "description", 'It needs a "description": one line saying what it does'));
  else if (chars(description) > MAX_DESCRIPTION_LEN || description.includes("\n")) {
    problems.push(problem("bad_listing", "description", `Its description is one line of at most ${MAX_DESCRIPTION_LEN} characters`));
  } else if (UNSEEN.test(description)) problems.push(problem("bad_listing", "description", "Its description has a control or invisible formatting character"));
  else {
    const claim = markClaim(description, lookalikes);
    if (claim !== undefined) problems.push(problem("bad_listing", "description", `Its description says "${claim}": only the store marks a plugin verified`));
  }
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

  const keywords = raw.get("keywords");
  if (isArray(keywords)) {
    const words = strings(keywords);
    if (keywords.length > MAX_KEYWORDS || duplicates(words)) problems.push(problem("bad_listing", "keywords", `It has at most ${MAX_KEYWORDS} keywords, each once`));
    else if (words.some((w) => w === undefined || !keyword(w))) {
      const bad = words.find((w) => w === undefined || !keyword(w));
      problems.push(problem("bad_listing", "keywords", `Keyword "${bad ?? "?"}" must be lowercase letters, digits, dashes and single spaces, at most ${MAX_KEYWORD_LEN} characters`));
    }
  } else if (keywords !== undefined && keywords !== null) problems.push(problem("bad_listing", "keywords", 'Its "keywords" must be a list'));

  for (const field of ["homepage", "repository"]) {
    const url = raw.get(field);
    if (url === undefined || url === null || (typeof url === "string" && httpsUrl(url) && parsedUrl(url) !== undefined)) continue;
    problems.push(problem("bad_listing", field, `Its "${field}" must be an https:// link`));
  }
  const license = raw.get("license");
  if (!(license === undefined || license === null || (typeof license === "string" && rustTrim(license) !== "" && byteLength(license) <= MAX_LICENSE_LEN && ![...license].some(isControl)))) {
    problems.push(problem("bad_listing", "license", `Its "license" is a short name, like MIT (at most ${MAX_LICENSE_LEN} characters)`));
  }
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
      const bytes = plainPath(path) ? files.get(path) : undefined;
      if (!typed || !bytes || !isScreenshot(bytes)) problems.push(problem("bad_listing", "screenshots", "A screenshot must be a PNG or JPEG picture in it", path));
    }
  } else if (screenshots !== undefined && screenshots !== null) problems.push(problem("bad_listing", "screenshots", 'Its "screenshots" must be a list'));

  const range = zeronRange(raw);
  if ("error" in range) problems.push(problem("bad_listing", "engines", range.error));
  else if (range.range !== null && !parseRange(range.range)) problems.push(problem("bad_listing", "engines", `"${range.range}" isn't a range of versions`));

  let total = 0;
  for (const bytes of files.values()) total += bytes.length;
  if (files.size > MAX_FILES || total > MAX_BYTES) problems.push(problem("too_large", null, "It's too big for a plugin (2,000 files, 50 MB)"));

  for (const [rel, bytes] of [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (!readable(rel, bytes, isText)) warnings.push(problem("binary_file", null, "It isn't text or a picture: reviewers look closely at files like this", rel));
  }
  const changelog = files.get(CHANGELOG_FILE);
  const notes = changelog && changelog.length <= MAX_CHANGELOG_BYTES ? changelogNotes(lossy.decode(changelog), manifest.version) : undefined;
  if (notes !== undefined && byteLength(notes) > MAX_NOTES_BYTES) {
    problems.push(problem("too_large", null, `This version's notes in CHANGELOG.md are at most ${MAX_NOTES_BYTES / 1024} KB`, CHANGELOG_FILE));
  }
  if (changelog && changelog.length <= MAX_CHANGELOG_BYTES && notes === undefined) {
    warnings.push(problem("no_notes", null, `CHANGELOG.md has no section for ${manifest.version}: this version shows without notes`, CHANGELOG_FILE));
  }

  // Amendment 31's rules, after the engine's own.
  const changelogBytes = files.get(CHANGELOG_FILE);
  if (changelogBytes && changelogBytes.length > MAX_CHANGELOG_BYTES) problems.push(problem("too_large", null, "CHANGELOG.md is over 512 KB", CHANGELOG_FILE));
  // (As written: without the signature, which the store keeps apart.)
  const unsigned: JsonObject = new Map(raw);
  unsigned.delete("signature");
  if (byteLength(canonicalJson(unsigned)) > MAX_MANIFEST_BYTES) problems.push(problem("too_large", "manifest", "Its manifest is over 256 KB"));
  // Two paths that are one file on a Mac or a Windows machine (the
  // manifest's own name among them), or a file where another path needs
  // a folder of that name there (amendment 42): each at the later path, in
  // code point order; a path with a problem is never recorded.
  const folded = new Map<string, string>([[foldPath(MANIFEST_FILE), MANIFEST_FILE]]);
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

  return { problems, warnings };
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

export const pictureType = (bytes: Uint8Array): "image/png" | "image/jpeg" | undefined => {
  if (bytes[0] === 0x89 && cleanImage("image/png", bytes)) return "image/png";
  if (bytes[0] === 0xff && cleanImage("image/jpeg", bytes)) return "image/jpeg";
  return undefined;
};
