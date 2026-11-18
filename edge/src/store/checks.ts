/**
 * The automated checks a submission lands in the queue with (plan "Review"),
 * stored with the version as `[{ id, level, message }]` (contract amendment
 * 12). What fails a check is refused at publish, so a stored list holds
 * `ok` and `warn`: the things a reviewer should look at.
 */
import { type ConsentItem, RISKY_ASKS, askPartsOf, consentDiff, fullControl } from "./consent";
import { pictureType } from "./listing";
import type { Listing, Problem } from "./listing";
import { riskyHost } from "./manifest";
import type { VerifiedPackage } from "./package";

/** Warnings stored one by one, at most. */
export const MAX_WARNINGS = 50;

export interface Check {
  readonly id: string;
  readonly level: "ok" | "warn" | "fail";
  readonly message: string;
  /** The file it's about, if one. */
  readonly path?: string;
}

/** How a packaged file is served and shown. */
export type FileKind = "png" | "jpeg" | "svg" | "text" | "binary";

/** Extensions that are code a device could load natively or as bytecode. */
const NATIVE_EXTENSIONS = /\.(wasm|node|dylib|so|dll|exe|bin|pyc|pyo|class|jar|luac|o|a|lib|com|msi|app|deb|rpm|dmg|pkg|zip|tar|gz|tgz|7z|rar|bz2|xz)$/i;

/** Whether `bytes` is UTF-8 text (RFC 3629: no overlong forms, surrogates
 * or code points past U+10FFFF) without a NUL: checked in place, never
 * decoded into a string. */
export const isText = (bytes: Uint8Array): boolean => {
  const n = bytes.length;
  let i = 0;
  while (i < n) {
    const b = bytes[i]!;
    if (b < 0x80) {
      if (b === 0) return false;
      i++;
      continue;
    }
    let need: number;
    let min: number;
    let cp: number;
    if (b >= 0xc2 && b <= 0xdf) [need, min, cp] = [1, 0x80, b & 0x1f];
    else if (b >= 0xe0 && b <= 0xef) [need, min, cp] = [2, 0x800, b & 0x0f];
    else if (b >= 0xf0 && b <= 0xf4) [need, min, cp] = [3, 0x10000, b & 0x07];
    else return false;
    for (let k = 1; k <= need; k++) {
      const c = bytes[i + k];
      if (c === undefined || (c & 0xc0) !== 0x80) return false;
      cp = (cp << 6) | (c & 0x3f);
    }
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return false;
    i += need + 1;
  }
  return true;
};

export const fileKind = (path: string, bytes: Uint8Array): FileKind => {
  const picture = pictureType(bytes);
  if (picture === "image/png") return "png";
  if (picture === "image/jpeg") return "jpeg";
  if (NATIVE_EXTENSIONS.test(path)) return "binary";
  // Lua bytecode starts ESC "Lua"; it's never text.
  if (bytes[0] === 0x1b && bytes[1] === 0x4c && bytes[2] === 0x75 && bytes[3] === 0x61) return "binary";
  if (!isText(bytes)) return "binary";
  return /\.svg$/i.test(path) ? "svg" : "text";
};

/** The relative picture references in a README: `![…](path)` on one
 * line, without a scheme, a leading `//` or `#`, its query, fragment and
 * `./` dropped. Linear: each `![` looks for its `]` and line end from
 * positions found once and reused, never rescanned. */
export const readmePictures = (readme: string): string[] => {
  const found = new Set<string>();
  let nextClose = -1;
  let nextLine = -1;
  for (let at = readme.indexOf("!["); at >= 0; at = readme.indexOf("![", at + 2)) {
    if (nextClose <= at) nextClose = readme.indexOf("]", at + 2);
    if (nextLine <= at) nextLine = readme.indexOf("\n", at);
    if (nextClose < 0) break;
    if (nextLine >= 0 && nextLine < nextClose) continue;
    if (readme[nextClose + 1] !== "(") continue;
    let i = nextClose + 2;
    while (readme[i] === " " || readme[i] === "\t") i++;
    if (readme[i] === "<") i++;
    let end = i;
    while (end < readme.length && end - i < 2048 && !") \t\n>".includes(readme[end]!)) end++;
    const ref = readme.slice(i, end);
    if (ref === "" || /^[a-z][a-z0-9+.-]*:/i.test(ref) || ref.startsWith("//") || ref.startsWith("#")) continue;
    let path = ref.split(/[?#]/)[0]!;
    try {
      path = decodeURIComponent(path);
    } catch {
      // As written.
    }
    const parts: string[] = [];
    for (const part of path.split("/")) {
      if (part === "" || part === ".") continue;
      if (part === "..") parts.pop();
      else parts.push(part);
    }
    if (parts.length) found.add(parts.join("/"));
  }
  return [...found];
};

/** Screenshots and README pictures that won't show (see `runChecks`). */
const unshownPictures = (listing: Listing, kinds: ReadonlyMap<string, FileKind>): string[] => {
  const shown = (path: string) => {
    const kind = kinds.get(path);
    return kind === "png" || kind === "jpeg";
  };
  return [...new Set([...listing.screenshots, ...readmePictures(listing.readme)])].filter((path) => !shown(path));
};

const list = (items: readonly string[], most = 8): string =>
  items.length <= most ? items.join(", ") : `${items.slice(0, most).join(", ")} and ${items.length - most} more`;

/** A size as the site writes one (apps/landing/src/lib/store/format.ts
 * `bytes`): whole bytes under 1 KB, else KB, MB or GB by 1024s, with one
 * decimal under 10 ("9.8 KB", "1.4 MB") and whole numbers from 10 ("13 KB"). */
export const sizeText = (n: number): string => {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
};

export interface CheckInput {
  readonly pkg: VerifiedPackage;
  readonly listing: Listing;
  /** The engine's listing warnings (`binary_file`, `no_notes`). */
  readonly warnings: readonly Problem[];
  /** Each file's kind as the store serves it. */
  readonly kinds: ReadonlyMap<string, FileKind>;
  readonly via: "session" | "deploy_key";
  readonly consent: ConsentItem;
  /** The last approved version's, the diff base; null for a first version. */
  readonly base: { readonly version: string; readonly consent: ConsentItem; readonly manifest: string } | null;
  /** The highest earlier version this one had to be newer than, if any. */
  readonly previous: string | null;
}

export const runChecks = (input: CheckInput): Check[] => {
  const { pkg, listing, consent, base } = input;
  const unshown = unshownPictures(listing, input.kinds);
  const checks: Check[] = [];
  const add = (id: string, level: Check["level"], message: string) => checks.push({ id, level, message });
  let total = 0;
  for (const bytes of pkg.files.values()) total += bytes.length;

  add("signature", "ok", `Signed by ${pkg.key}, uploaded ${input.via === "deploy_key" ? "with a deploy key" : "from a signed-in device"}.`);
  add("manifest", "ok", "The manifest follows every rule a device checks.");
  add("size", "ok", `${pkg.files.size} files, ${sizeText(total)} (${sizeText(pkg.bytes)} packaged).`);
  add("version", listing.version.pre.length ? "warn" : "ok", listing.version.pre.length ? `${listing.version.text} is a pre-release.` : `${listing.version.text}.`);
  add("newer", "ok", input.previous ? `Later than ${input.previous}.` : "Its first version.");
  add("readme", "ok", "README.md is there.");
  if (listing.notes) add("changelog", "ok", "CHANGELOG.md has a section for this version.");
  add("engines", listing.engines ? "ok" : "warn", listing.engines ? `Needs Zeron ${listing.engines}.` : "It doesn't say which Zeron versions it works with.");

  // The engine's warnings, by its rule words, one per file: the first
  // MAX_WARNINGS, then how many more (the list is stored with the version).
  for (const w of input.warnings.slice(0, MAX_WARNINGS)) {
    checks.push({ id: w.rule, level: "warn", message: w.path ? `${w.path}: ${w.words}.` : `${w.words}.`, ...(w.path ? { path: w.path } : {}) });
  }
  if (input.warnings.length > MAX_WARNINGS) add("more_warnings", "warn", `${input.warnings.length - MAX_WARNINGS} more warnings like these.`);
  if (!input.warnings.some((w) => w.rule === "binary_file")) add("binary_file", "ok", "Every file is text or a picture.");
  // Pictures the page or the listing shows, that won't show: not in the
  // package, or not a PNG or JPEG image-check.ts takes.
  for (const path of unshown.slice(0, MAX_WARNINGS)) {
    checks.push({ id: "picture_unshown", level: "warn", message: `${path}: it won't show: it isn't a PNG or JPEG picture in the plugin that the store can read.`, path });
  }

  const risky = consent.asks.filter((a) => RISKY_ASKS.includes(a));
  add("asks", risky.length ? "warn" : "ok", risky.length ? `Asks for: ${list(risky, 12)}.` : "Asks for nothing that reaches past Zeron.");
  const full = consent.commands.filter(fullControl);
  if (consent.commands.length) {
    add("commands", "warn", `Runs ${list([...consent.commands])}${full.length ? ` (${list(full)} can run anything)` : ""}.`);
  }
  const local = consent.network_hosts.filter((h) => h !== "*" && riskyHost(h));
  if (local.length) add("hosts", "warn", `Reaches this device or your network: ${list(local)}.`);
  if (consent.asks.includes("network") && consent.network_hosts.length === 0) add("hosts", "warn", "Reaches any public host.");

  if (base) {
    const diff = consentDiff(base.consent, consent, { before: askPartsOf(base.manifest), after: askPartsOf(pkg.manifest.raw) });
    const grew = [
      diff.added.length ? `asks ${list(diff.added)}` : "",
      diff.added_commands.length ? `runs ${list(diff.added_commands)}` : "",
      diff.network_added.length ? `reaches ${list(diff.network_added)}` : "",
      diff.tools_added.length ? `adds tools ${list(diff.tools_added.map((t) => t.name))}` : "",
      diff.tools_changed.length ? `changes tools ${list(diff.tools_changed.map((t) => t.name))}` : "",
      diff.mcp_changed ? (diff.mcp_changed.before ? (diff.mcp_changed.after ? "changes its MCP server" : "drops its MCP server") : "runs an MCP server") : "",
      diff.service_changed ? (diff.service_changed.after ? "changes its service" : "drops its service") : "",
      diff.settings_changed ? "changes a setting that grants access" : "",
      diff.ui_change_added.length ? `changes ${list(diff.ui_change_added)}` : "",
      diff.ui_replace_added.length ? `replaces ${list(diff.ui_replace_added)}` : ""
    ].filter(Boolean);
    add("since_approved", grew.length ? "warn" : "ok", grew.length ? `Since ${base.version}, it newly ${grew.join("; ")}.` : `Asks for nothing new since ${base.version}.`);
  }
  return checks;
};
