/**
 * A file's path inside a package, held to the engine's rules
 * (crates/plugin-guard/src/path.rs `problem`, crates/engine/src/plugins/
 * package.rs `plain_path`): whatever a package names stays inside its
 * folder on every system. The store builds R2 keys only from paths that
 * pass `plainPath`.
 */
import { LIMITS } from "./data";
import { compareUtf8 } from "./json";

/** The longest package path, in UTF-8 bytes. */
export const MAX_PATH = LIMITS.max_path;

const encoder = new TextEncoder();

/** Rust's `char::is_control`: general category Cc. */
const isControl = (c: number): boolean => c <= 0x1f || (c >= 0x7f && c <= 0x9f);

const FORBIDDEN = new Set(["\\", ":", "<", ">", '"', "|", "?", "*"]);

/** plugin-guard `reserved_on_windows`. */
export const reservedOnWindows = (part: string): boolean => {
  const stem = (part.split(".")[0] ?? "").replace(/ +$/, "");
  const upper = stem.replace(/[a-z]/g, (c) => c.toUpperCase());
  if (["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].includes(upper)) return true;
  const chars = [...stem];
  const head = chars.slice(0, 3).join("").replace(/[a-z]/g, (c) => c.toUpperCase());
  const rest = chars.slice(3);
  return (head === "COM" || head === "LPT") && rest.length === 1 && /^[0-9¹²³]$/.test(rest[0]!);
};

/** Why `rel` can't be a package path, if it can't. */
export const pathProblem = (rel: string): string | undefined => {
  const length = encoder.encode(rel).length;
  if (length === 0 || length > MAX_PATH) return "a path is 1 to 1024 bytes";
  if (rel.startsWith("/")) return "it's relative to the package";
  for (const part of rel.split("/")) {
    if (part === "" || part === "." || part === "..") return "its parts are names";
    for (const ch of part) {
      if (isControl(ch.codePointAt(0)!) || FORBIDDEN.has(ch)) return `${JSON.stringify(ch)} isn't allowed in a name`;
    }
    if (part.endsWith(".") || part.endsWith(" ")) return "a name can't end with a dot or a space";
    if (reservedOnWindows(part)) return "that name is a device on Windows";
  }
  return undefined;
};

/** package.rs `plain_path`: a safe path whose parts never start with a dot. */
export const plainPath = (rel: string): boolean =>
  pathProblem(rel) === undefined && rel.split("/").every((part) => !part.startsWith("."));

/** plugin-guard `fold` (path.rs): how the files no template replaces are
 * compared: `\` as `/`, each character's uppercase then its lowercase,
 * empty and `.` parts dropped, each part without its trailing dots and
 * spaces. */
export const fold = (rel: string): string =>
  [...rel.replaceAll("\\", "/")]
    .map((c) => c.toUpperCase().toLowerCase())
    .join("")
    .split("/")
    .filter((part) => part !== "" && part !== ".")
    .map((part) => part.replace(/[. ]+$/, ""))
    .join("/");

/** plugin-guard `fold`, after Unicode normalization (NFC): how macOS and
 * Windows compare two paths (listing.rs `fold_path`). Two that fold the
 * same are one file there. */
export const foldPath = (rel: string): string => fold(rel.normalize("NFC"));

/** What a device writes beside a plugin's files, which no file may fold to
 * (install.rs `SOURCE_FILE`). */
export const SOURCE_FILE = ".source.json";

/**
 * The first two of `paths` that would be one file, or a file where the
 * other needs a folder, on a disk that folds names as macOS's and Windows'
 * do, with `taken` (files already there) counted first: in words naming
 * both, or undefined (install.rs `colliding`, which a device refuses as
 * `paths_collide`). Paths are taken in UTF-8 order, as the engine's map.
 */
export const firstCollision = (paths: Iterable<string>, taken: readonly string[]): string | undefined => {
  const files = new Map<string, string>(taken.map((rel) => [foldPath(rel), rel]));
  const folders = new Map<string, string>();
  for (const rel of [...paths].sort(compareUtf8)) {
    const folded = foldPath(rel);
    const other = files.get(folded);
    if (other !== undefined) return `${JSON.stringify(rel)} and ${JSON.stringify(other)} are one file on macOS and Windows`;
    const inside = folders.get(folded);
    if (inside !== undefined) return `${JSON.stringify(rel)} is a file where ${JSON.stringify(inside)} needs a folder of that name on macOS and Windows`;
    const parts = folded.split("/");
    for (let end = 1; end < parts.length; end++) {
      const folder = parts.slice(0, end).join("/");
      const file = files.get(folder);
      if (file !== undefined) return `${JSON.stringify(file)} is a file where ${JSON.stringify(rel)} needs a folder of that name on macOS and Windows`;
      if (!folders.has(folder)) folders.set(folder, rel);
    }
    files.set(folded, rel);
  }
  return undefined;
};
