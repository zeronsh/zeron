// The site's own addresses for the store, made in one place.
import { KIND_PATH, type Kind } from "./kind.ts";
import { packagePath } from "./names.ts";

/** A plugin's page, or a template's. */
export const pluginUrl = (id: string, kind: Kind = "plugin") => `${KIND_PATH[kind]}/${encodeURIComponent(id)}`;

/** A file of a version's package, through the site's file route. */
export const fileUrl = (id: string, version: string, path: string, kind: Kind = "plugin") =>
  `${pluginUrl(id, kind)}/files/${encodeURIComponent(version)}/${path.split("/").map(encodeURIComponent).join("/")}`;

/**
 * A picture of a plugin's on this site, from its path in the package and
 * the version it's from (amendment 35: always a path, never a URL). Null
 * when there's no version, or the path isn't one (packagePath).
 */
export function pictureUrl(id: string, version: string | null, path: string | null | undefined, kind: Kind = "plugin"): string | null {
  const clean = path ? packagePath(path) : null;
  return clean && version ? fileUrl(id, version, clean, kind) : null;
}

/**
 * An author's link (a homepage, a repository) as it may be followed: parsed,
 * https only, no user or password in it, given back as the parsed address.
 * Null for anything else. The author's text itself is never put in an href.
 */
export function httpsUrl(value: string | null | undefined): string | null {
  // (As the store holds them, amendment 31: no backslash, whitespace or control character.)
  // eslint-disable-next-line no-control-regex
  if (!value || /[\\\s\u0000-\u001f\u007f]/.test(value)) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !url.username && !url.password && url.hostname ? url.href : null;
  } catch {
    return null;
  }
}

/** A member's card (served by the edge on this host). */
export const memberUrl = (username: string) => `/@${encodeURIComponent(username)}`;

/**
 * Zeron's own address for a plugin or a template (apps/zgx/src/links.rs), naming a version when given (the
 * contract's "The link can name a version"). The app reads `version` only
 * as a version reads, unescaped (letters, digits, ".", "+", "-", at most
 * 32): one that doesn't is left out, and the link opens the plugin.
 */
export const appUrl = (id: string, version?: string, kind: Kind = "plugin") =>
  `zeron://open/${kind}/${encodeURIComponent(id)}${version && linkable(version) ? `?version=${version}` : ""}`;

/**
 * Whether Zeron's link can name this version. One it can't is never named
 * on the button either: a label that names a version the link drops would
 * open the newest under another's name (the identity lane's finding).
 */
export const linkable = (version: string) => /^[A-Za-z0-9.+-]{1,32}$/.test(version);

/** Sign in, then come back to `path` (a path on this site, never anywhere else). */
export const signInUrl = (path: string) => `/auth/web/login?${new URLSearchParams({ return_to: sitePath(path) })}`;

/** `value` if it's a path on this site, else "/": never a way off it. */
export function sitePath(value: string | null | undefined): string {
  if (!value || !value.startsWith("/") || value.startsWith("//") || value.startsWith("/\\")) return "/";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f\\]/.test(value)) return "/";
  try {
    const url = new URL(value, "https://zeron.sh");
    // (Normalising can turn a path that read as one, "/.//host", into "//host".)
    if (url.origin !== "https://zeron.sh" || url.pathname.startsWith("//")) return "/";
    return `${url.pathname}${url.search}${url.hash}`;
  } catch {
    return "/";
  }
}
