/**
 * A file's path inside a package, held to the engine's rules
 * (crates/plugin-guard/src/path.rs `problem`, crates/engine/src/plugins/
 * package.rs `plain_path`): whatever a package names stays inside its
 * folder on every system. The store builds R2 keys only from paths that
 * pass `plainPath`.
 */
import { LIMITS } from "./data";

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
