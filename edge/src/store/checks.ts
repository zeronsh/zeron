/**
 * The automated checks a submission lands in the queue with (plan "Review"),
 * stored with the version as `[{ id, level, message }]` (contract amendment
 * 12). What fails a check is refused at publish, so a stored list holds
 * `ok` and `warn`: the things a reviewer should look at.
 */
import { type ConsentItem, RISKY_ASKS, consentDiff, fullControl } from "./consent";
import { decodeText, pictureType } from "./listing";
import type { Listing } from "./listing";
import { riskyHost } from "./manifest";
import type { VerifiedPackage } from "./package";

export interface Check {
  readonly id: string;
  readonly level: "ok" | "warn" | "fail";
  readonly message: string;
}

/** How a packaged file is served and shown. */
export type FileKind = "png" | "jpeg" | "svg" | "text" | "binary";

/** Extensions that are code a device could load natively or as bytecode. */
const NATIVE_EXTENSIONS = /\.(wasm|node|dylib|so|dll|exe|bin|pyc|pyo|class|jar|luac|o|a|lib|com|msi|app|deb|rpm|dmg|pkg|zip|tar|gz|tgz|7z|rar|bz2|xz)$/i;

export const fileKind = (path: string, bytes: Uint8Array): FileKind => {
  const picture = pictureType(bytes);
  if (picture === "image/png") return "png";
  if (picture === "image/jpeg") return "jpeg";
  if (NATIVE_EXTENSIONS.test(path)) return "binary";
  const text = decodeText(bytes);
  if (text === undefined || text.includes("\u0000")) return "binary";
  // Lua bytecode starts ESC "Lua"; it's never text.
  if (text.startsWith("\u001bLua")) return "binary";
  return /\.svg$/i.test(path) ? "svg" : "text";
};

const list = (items: readonly string[], most = 8): string =>
  items.length <= most ? items.join(", ") : `${items.slice(0, most).join(", ")} and ${items.length - most} more`;

const megabytes = (bytes: number): string => `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`;

export interface CheckInput {
  readonly pkg: VerifiedPackage;
  readonly listing: Listing;
  readonly kinds: ReadonlyMap<string, FileKind>;
  readonly via: "session" | "deploy_key";
  readonly consent: ConsentItem;
  /** The last approved version's, the diff base; null for a first version. */
  readonly base: { readonly version: string; readonly consent: ConsentItem } | null;
  /** The highest earlier version this one had to be newer than, if any. */
  readonly previous: string | null;
}

export const runChecks = (input: CheckInput): Check[] => {
  const { pkg, listing, kinds, consent, base } = input;
  const checks: Check[] = [];
  const add = (id: string, level: Check["level"], message: string) => checks.push({ id, level, message });
  let total = 0;
  for (const bytes of pkg.files.values()) total += bytes.length;

  add("signature", "ok", `Signed by ${pkg.key}, uploaded ${input.via === "deploy_key" ? "with a deploy key" : "from a signed-in device"}.`);
  add("manifest", "ok", "The manifest follows every rule a device checks.");
  add("size", "ok", `${pkg.files.size} files, ${megabytes(total)} (${megabytes(pkg.bytes)} packaged).`);
  add("version", listing.version.pre.length ? "warn" : "ok", listing.version.pre.length ? `${listing.version.text} is a pre-release.` : `${listing.version.text}.`);
  add("newer", "ok", input.previous ? `Later than ${input.previous}.` : "Its first version.");
  add("readme", "ok", "README.md is there.");
  add("changelog", listing.notes ? "ok" : "warn", listing.notes ? "CHANGELOG.md has a section for this version." : "No changelog section for this version.");
  add("engines", listing.engines ? "ok" : "warn", listing.engines ? `Needs Zeron ${listing.engines}.` : "It doesn't say which Zeron versions it works with.");

  const binary = [...kinds].filter(([, kind]) => kind === "binary").map(([path]) => path);
  add("binary", binary.length ? "warn" : "ok", binary.length ? `Files that aren't text or pictures: ${list(binary)}.` : "Every file is text or a picture.");

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
    const diff = consentDiff(base.consent, consent);
    const grew = [
      diff.added.length ? `asks ${list(diff.added)}` : "",
      diff.added_commands.length ? `runs ${list(diff.added_commands)}` : "",
      diff.network_added.length ? `reaches ${list(diff.network_added)}` : "",
      diff.tools_added.length ? `adds tools ${list(diff.tools_added)}` : "",
      diff.ui_change_added.length ? `changes ${list(diff.ui_change_added)}` : "",
      diff.ui_replace_added.length ? `replaces ${list(diff.ui_replace_added)}` : ""
    ].filter(Boolean);
    add("since_approved", grew.length ? "warn" : "ok", grew.length ? `Since ${base.version}, it newly ${grew.join("; ")}.` : `Asks for nothing new since ${base.version}.`);
  }
  return checks;
};
