// The site's own addresses for the store, made in one place.

/** A plugin's page. */
export const pluginUrl = (id: string) => `/plugins/${encodeURIComponent(id)}`;

/** A file of a version's package, through the site's file route. */
export const fileUrl = (id: string, version: string, path: string) =>
  `/plugins/${encodeURIComponent(id)}/files/${encodeURIComponent(version)}/${path.split("/").map(encodeURIComponent).join("/")}`;

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
