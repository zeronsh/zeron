// What may name a plugin, a version and a file in a URL. A route checks its
// parameters with these before it asks the edge anything, so a malformed
// address is the site's 404 and never a request.

/** A manifest `id`: 2 to 40 lowercase letters, digits and dashes (apps/zgx/docs/plugins.md). */
const PLUGIN_ID = /^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$/;

/** Strict SemVer 2.0.0 (semver.org's own expression). */
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

/** A username (edge/src/username.ts): 3 to 20, letters, digits, `_` or `-`, lowercase. */
const USERNAME = /^[a-z0-9][a-z0-9_-]{2,19}$/;

export const isPluginId = (value: unknown): value is string => typeof value === "string" && PLUGIN_ID.test(value);

/**
 * A version from an address. Astro decodes a path with decodeURI, which
 * leaves a reserved character escaped, so a version's "+" (its build, as
 * in 1.0.0+build.7) arrives as %2B: put back here, and nothing else.
 */
export const versionParam = (raw: string | undefined) => raw?.replace(/%2B/gi, "+");

export const isVersion = (value: unknown): value is string => typeof value === "string" && value.length <= 64 && SEMVER.test(value);

export const isUsername = (value: unknown): value is string => typeof value === "string" && USERNAME.test(value);

/** Whether a version is a pre-release (`1.2.0-beta.1`). */
export const isPrerelease = (version: string): boolean => /^[^+]*-/.test(version);

/**
 * The words that are the site's own pages under /plugins/ (and the store's
 * other pages), never a plugin's id: docs/plugin-store-data.json `site_ids`,
 * settled across the lanes, closed once anyone can publish. (names.test.ts
 * holds this to that file.)
 */
export const SITE_IDS: readonly string[] = [
  "all", "new", "search", "browse", "discover", "featured", "popular", "updated", "category", "categories", "keyword", "keywords",
  "tags", "publishing", "publish", "guide", "report", "reports", "review", "queue", "audit", "sitemap", "feed", "static", "docs",
  "help", "about", "account", "admin", "api", "install", "me", "settings", "terms", "privacy", "assets",
];

/** The longest path in a package, in UTF-8 bytes (the edge's max_path). */
export const MAX_PATH = 1024;

/**
 * A file's path inside a package, normalized, or null when it isn't one: no
 * leading slash, no backslash, no empty, `.` or `..` part, no control
 * characters, at most 512 bytes. (The engine's path rules; the edge holds
 * packages to them, so anything else names no file.)
 */
export function packagePath(value: string): string | null {
  // (1 to 1024 bytes of UTF-8, as the edge takes them: docs/plugin-store-data.json max_path.)
  if (value.length === 0 || new TextEncoder().encode(value).length > MAX_PATH) return null;
  if (value.startsWith("/") || value.includes("\\")) return null;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(value)) return null;
  const parts = value.split("/");
  if (parts.some((part) => part === "" || part === "." || part === "..")) return null;
  return parts.join("/");
}

/**
 * Resolves `reference` (a README's relative link to a file) against the
 * package's root, as a browser would against the README's own address:
 * `./shots/a.png`, `shots/a.png` and `docs/../shots/a.png` all name
 * `shots/a.png`. Null when it climbs out of the package, or isn't a path.
 */
export function resolvePackagePath(reference: string): string | null {
  let path: string;
  try {
    path = decodeURIComponent(reference.replace(/[?#][^]*$/, ""));
  } catch {
    return null;
  }
  if (path === "" || path.startsWith("/") || path.includes("\\")) return null;
  const out: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (out.length === 0) return null;
      out.pop();
      continue;
    }
    out.push(part);
  }
  return packagePath(out.join("/"));
}
