// An address as the router will read it. Astro decodes a path before it
// routes it, and decodes it again (so `/%61dmin` and `/%2561dmin` are both
// `/admin`): a check on the path as sent can be walked past. So each
// address has one canonical form, and anything else is sent there.

/**
 * Characters that never need encoding and never change a path's shape:
 * RFC 3986's unreserved, but the dot (an encoded dot in a file's name is a
 * name, not a climb), and never the slash (an encoded one is part of a
 * name; the gate reads the route the router matched, so `/admin%2Fplugins`
 * is the staff's all the same).
 */
const PLAIN = /^[A-Za-z0-9\-_~]$/;

/**
 * The path in its one canonical form: slashes not doubled, and any
 * percent-encoding of a letter, a digit, `-`, `_` or `~` (`%61`, and the
 * same encoded again, `%2561`, which the router decodes twice) written as
 * itself. Everything else (`%20`, `%2E`, `%2F`, `%25` before anything but
 * those, `%3C`) stays as sent, so the site's own addresses (urls.ts, each
 * segment encodeURIComponent'd) are canonical already.
 */
export function canonicalPath(path: string): string {
  let out = path;
  for (let i = 0; i < 4; i++) {
    const next = out.replace(/%(?:25)*([0-9A-Fa-f]{2})/g, (whole, hex: string) => {
      const ch = String.fromCharCode(parseInt(hex, 16));
      return PLAIN.test(ch) ? ch : whole;
    });
    if (next === out) break;
    out = next;
  }
  return out.replace(/\/{2,}/g, "/");
}

/** Whether a path (canonical) or a route's pattern is the staff's: anything under /admin. */
export const isStaffPath = (path: string) => path === "/admin" || path.startsWith("/admin/");

/** Paths of the store's (not the landing page's): answered on the canonical host alone. */
export const isStorePath = (path: string) => /^\/(plugins|account|admin|auth\/web)(\/|$)/.test(path);
