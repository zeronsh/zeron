// The site's own addresses for the store, made in one place.

/** A plugin's page. */
export const pluginUrl = (id: string) => `/plugins/${encodeURIComponent(id)}`;

/** A file of a version's package, through the site's file route. */
export const fileUrl = (id: string, version: string, path: string) =>
  `/plugins/${encodeURIComponent(id)}/files/${encodeURIComponent(version)}/${path.split("/").map(encodeURIComponent).join("/")}`;

/**
 * A picture of a plugin's, on this site: from the edge's address for it
 * (/store/v1/plugins/<id>/files/<version>/<path>, which the site serves at
 * the same path without /store/v1), or from its path in the package and
 * the version it's from. Null when it names nothing this site serves.
 */
export function pictureUrl(id: string, version: string | null, value: string | null | undefined): string | null {
  if (!value) return null;
  const edge = `/store/v1/plugins/${encodeURIComponent(id)}/files/`;
  if (value.startsWith("/store/v1/")) return value.startsWith(edge) && !/[?#]/.test(value) ? value.slice("/store/v1".length) : null;
  if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(value) || value.startsWith("/")) return null;
  return version ? fileUrl(id, version, value) : null;
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

/** The app's own link to a plugin's page in Settings → Plugins (apps/zgx/src/links.rs). */
export const appUrl = (id: string) => `zeron://open/plugin/${encodeURIComponent(id)}`;

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
