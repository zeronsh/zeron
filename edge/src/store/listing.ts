/**
 * What the store needs of a plugin beyond what a device needs (plan
 * "Listing metadata"): its page (`README.md`), a description, an icon, one
 * or two categories from the fixed list, a strict semver version; and the
 * optional listing fields, held to rules here. Sideloading needs none of it.
 *
 *   categories   1–2, from CATEGORIES
 *   keywords     up to 8: 1–32 characters, lowercase, no control characters
 *   homepage, repository   https URLs, ≤ 2048
 *   license      ≤ 64 characters
 *   screenshots  up to 8 package paths, each a PNG or JPEG image-check takes
 *   icon         a package path to an SVG ≤ 32 KB that starts with `<svg`
 *                (what the engine draws, scan.rs `icon_svg`)
 *   engines.zeron  a range (semver.ts)
 *
 * README.md is at most 512 KB and the changelog section at most 64 KB: both
 * live in the version's row.
 */
import { cleanImage } from "../image-check";
import { type Json, isArray, isObject } from "./json";
import type { Manifest } from "./manifest";
import { plainPath } from "./paths";
import { MAX_VERSION_LENGTH, type Range, type Version, parseRange, parseVersion } from "./semver";

/** The fixed list (contract amendment 4), with the words shown for each. */
export const CATEGORIES: readonly { readonly id: string; readonly name: string }[] = [
  { id: "agents", name: "Agents" },
  { id: "appearance", name: "Appearance" },
  { id: "chat", name: "Chat" },
  { id: "developer-tools", name: "Developer tools" },
  { id: "git", name: "Git" },
  { id: "integrations", name: "Integrations" },
  { id: "productivity", name: "Productivity" },
  { id: "utilities", name: "Utilities" }
];
export const isCategory = (id: string): boolean => CATEGORIES.some((c) => c.id === id);

export const README_FILE = "README.md";
export const CHANGELOG_FILE = "CHANGELOG.md";
export const MAX_README_BYTES = 512 * 1024;
export const MAX_NOTES_BYTES = 64 * 1024;
export const MAX_ICON_BYTES = 32 * 1024;
export const MAX_NAME = 64;
export const MAX_DESCRIPTION = 300;
export const MAX_KEYWORDS = 8;
export const MAX_KEYWORD = 32;
export const MAX_SCREENSHOTS = 8;
export const MAX_URL = 2048;
export const MAX_LICENSE = 64;

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

export type ListingError = { readonly error: "not_semver" | "missing_readme" | "missing_listing" | "too_large"; readonly message: string };

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
export const decodeText = (bytes: Uint8Array): string | undefined => {
  try {
    return utf8.decode(bytes);
  } catch {
    return undefined;
  }
};

const hasControl = (s: string): boolean => /[\u0000-\u001f\u007f-\u009f]/.test(s);
const chars = (s: string): number => [...s].length;

const httpsUrl = (value: Json | undefined): string | null | undefined => {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string" || value.length > MAX_URL || hasControl(value)) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.username === "" && url.password === "" ? value : undefined;
  } catch {
    return undefined;
  }
};

const fail = (error: ListingError["error"], message: string): ListingError => ({ error, message });

/** Escape for a RegExp. */
const literal = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * The section of a changelog for `version`: the text under the first
 * heading that names it (`## 1.2.0`, `## [1.2.0] - 2026-11-01`, `# v1.2.0`)
 * up to the next heading at its level or above. Undefined when none names it.
 */
export const changelogSection = (changelog: string, version: string): string | undefined => {
  const lines = changelog.split(/\r\n|\n|\r/);
  const names = new RegExp(`^\\[?v?${literal(version)}\\]?(?:$|[^0-9A-Za-z.+-])`);
  let start = -1;
  let level = 0;
  for (let i = 0; i < lines.length; i++) {
    const heading = /^ {0,3}(#{1,6})[ \t]+(.*?)[ \t#]*$/.exec(lines[i]!);
    if (!heading) continue;
    if (start >= 0 && heading[1]!.length <= level) return lines.slice(start, i).join("\n").trim();
    if (start < 0 && names.test(heading[2]!.trim())) {
      start = i + 1;
      level = heading[1]!.length;
    }
  }
  return start >= 0 ? lines.slice(start).join("\n").trim() : undefined;
};

/** The listing `manifest` and `files` make, or why the store can't list it. */
export const readListing = (manifest: Manifest, files: ReadonlyMap<string, Uint8Array>): Listing | ListingError => {
  const raw = manifest.raw;
  const version = manifest.version.length <= MAX_VERSION_LENGTH ? parseVersion(manifest.version) : undefined;
  if (!version) {
    return fail("not_semver", `"${manifest.version}" isn't a version the store takes: strict semver (1.2.3, 1.2.3-beta.1), each number at most 2^53−1, at most 128 characters.`);
  }
  const readmeBytes = files.get(README_FILE);
  if (!readmeBytes) return fail("missing_readme", "The store shows README.md as the plugin's page: add one to the plugin's folder.");
  if (readmeBytes.length > MAX_README_BYTES) return fail("too_large", "README.md is over 512 KB.");
  const readme = decodeText(readmeBytes);
  if (readme === undefined) return fail("missing_readme", "README.md isn't UTF-8 text.");

  if (chars(manifest.name) > MAX_NAME || hasControl(manifest.name)) return fail("missing_listing", `"name" is one line of at most ${MAX_NAME} characters.`);
  const summary = manifest.description.trim();
  if (summary === "") return fail("missing_listing", 'The store needs a "description": a line saying what it does.');
  if (chars(summary) > MAX_DESCRIPTION || hasControl(summary)) return fail("missing_listing", `"description" is one line of at most ${MAX_DESCRIPTION} characters.`);

  const icon = raw.get("icon");
  if (typeof icon !== "string" || icon === "") return fail("missing_listing", 'The store needs an "icon": an SVG file in the plugin.');
  const iconBytes = plainPath(icon) ? files.get(icon) : undefined;
  const iconText = iconBytes && iconBytes.length <= MAX_ICON_BYTES && icon.endsWith(".svg") ? decodeText(iconBytes) : undefined;
  if (iconText === undefined || !iconText.trimStart().startsWith("<svg")) {
    return fail("missing_listing", `Its icon "${icon}" must be an SVG file in the plugin, at most 32 KB, starting with <svg.`);
  }

  const categoriesValue = raw.get("categories");
  const categories = isArray(categoriesValue) ? categoriesValue : [];
  if (categories.length < 1 || categories.length > 2 || !categories.every((c) => typeof c === "string" && isCategory(c)) || new Set(categories).size !== categories.length) {
    return fail("missing_listing", `"categories" is one or two of: ${CATEGORIES.map((c) => c.id).join(", ")}.`);
  }

  const keywordsValue = raw.get("keywords") ?? null;
  const keywordList = keywordsValue === null ? [] : isArray(keywordsValue) ? keywordsValue : undefined;
  if (
    !keywordList ||
    keywordList.length > MAX_KEYWORDS ||
    !keywordList.every((k) => typeof k === "string" && k.trim() === k && k !== "" && chars(k) <= MAX_KEYWORD && k === k.toLowerCase() && !hasControl(k))
  ) {
    return fail("missing_listing", `"keywords" is up to ${MAX_KEYWORDS} lowercase words or phrases, each at most ${MAX_KEYWORD} characters.`);
  }
  const keywords = [...new Set(keywordList as string[])];

  const homepage = httpsUrl(raw.get("homepage"));
  if (homepage === undefined) return fail("missing_listing", '"homepage" is an https link.');
  const repository = httpsUrl(raw.get("repository"));
  if (repository === undefined) return fail("missing_listing", '"repository" is an https link.');
  const licenseValue = raw.get("license") ?? null;
  if (licenseValue !== null && (typeof licenseValue !== "string" || licenseValue.trim() === "" || chars(licenseValue) > MAX_LICENSE || hasControl(licenseValue))) {
    return fail("missing_listing", `"license" is a short name (MIT, Apache-2.0), at most ${MAX_LICENSE} characters.`);
  }

  const shotsValue = raw.get("screenshots") ?? null;
  const shots = shotsValue === null ? [] : isArray(shotsValue) ? shotsValue : undefined;
  if (!shots || shots.length > MAX_SCREENSHOTS || !shots.every((s) => typeof s === "string")) {
    return fail("missing_listing", `"screenshots" is a list of up to ${MAX_SCREENSHOTS} pictures in the plugin.`);
  }
  for (const shot of shots as string[]) {
    const bytes = plainPath(shot) ? files.get(shot) : undefined;
    if (!bytes || pictureType(bytes) === undefined) {
      return fail("missing_listing", `Screenshot "${shot}" must be a PNG or JPEG in the plugin, at most 4096 pixels a side.`);
    }
  }

  let engines: string | null = null;
  let enginesRange: Range | null = null;
  const enginesValue = raw.get("engines");
  if (enginesValue !== undefined && enginesValue !== null) {
    const zeron = isObject(enginesValue) ? enginesValue.get("zeron") : undefined;
    if (!isObject(enginesValue) || (zeron !== undefined && zeron !== null && typeof zeron !== "string")) {
      return fail("missing_listing", '"engines" is { "zeron": ">=0.4.0" }.');
    }
    if (typeof zeron === "string") {
      enginesRange = parseRange(zeron) ?? null;
      if (!enginesRange) return fail("missing_listing", `"engines.zeron" "${zeron}" isn't a range (">=0.4.0", "^0.4.1", ">=0.4.0 <0.6.0 || >=1.0.0").`);
      engines = zeron;
    }
  }

  const changelogBytes = files.get(CHANGELOG_FILE);
  const changelog = changelogBytes ? decodeText(changelogBytes) : undefined;
  const notes = changelog === undefined ? null : (changelogSection(changelog, manifest.version) ?? null);
  if (notes !== null && new TextEncoder().encode(notes).length > MAX_NOTES_BYTES) return fail("too_large", "This version's changelog section is over 64 KB.");

  return {
    name: manifest.name,
    summary,
    icon,
    categories: categories as string[],
    keywords,
    homepage,
    repository,
    license: licenseValue as string | null,
    screenshots: shots as string[],
    engines,
    enginesRange,
    readme,
    notes,
    version
  };
};

/** `image/png` or `image/jpeg` when image-check.ts takes the bytes as that
 * picture, else undefined. */
export const pictureType = (bytes: Uint8Array): "image/png" | "image/jpeg" | undefined => {
  if (bytes[0] === 0x89 && cleanImage("image/png", bytes)) return "image/png";
  if (bytes[0] === 0xff && cleanImage("image/jpeg", bytes)) return "image/jpeg";
  return undefined;
};
