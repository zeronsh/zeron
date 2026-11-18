// A form posted after its session ended (an admin's lasts 30 minutes idle):
// what was typed isn't lost. The browser is sent to sign in, then back to
// the page that asks the same thing, with the fields filled in as they
// were. Only the named fields travel, in the address of a page on this
// site, which the edge keeps through sign-in (2048 characters at most:
// web-cookies.ts MAX_RETURN_TO). If what was typed is longer, the longest
// field is cut to fit and the page says so: never lost without a word.
import type { AstroGlobal } from "astro";
import { field, formOf } from "./forms.ts";
import type { WebSession } from "./types.ts";
import { signInUrl } from "./urls.ts";

/** The longest address the edge brings back after signing in. */
const MOST = 2048;

/** The cookie the edge keeps a web session in (edge/src/web-cookies.ts). */
const SESSION_COOKIE = "__Host-zeron_session";

/** The page at `path` with `fields` of `form` in its query, short enough to come back. */
export function resumeAddress(path: string, form: FormData | null, fields: readonly string[]): string {
  const values = new Map<string, string>();
  for (const name of fields) {
    const value = form ? field(form, name, 2000) : "";
    if (value) values.set(name, value);
  }
  const address = (cut: boolean) => {
    const params = new URLSearchParams([...values]);
    if (cut) params.set("cut", "1");
    return `${path}${params.size ? `?${params}` : ""}`;
  };
  let cut = false;
  while (address(cut).length > MOST && values.size > 0) {
    const [longest] = [...values].sort((a, b) => b[1].length - a[1].length)[0]!;
    const value = values.get(longest)!;
    // (Cut by whole characters, a tenth at a time.)
    const chars = [...value];
    const kept = chars.slice(0, Math.floor(chars.length * 0.9)).join("");
    if (kept) values.set(longest, kept);
    else values.delete(longest);
    cut = true;
  }
  return address(cut);
}

/** Sign in again, then come back to `path` with what was typed. */
export function signInAndResume(astro: Pick<AstroGlobal, "redirect">, path: string, form: FormData | null, fields: readonly string[]): Response {
  return astro.redirect(signInUrl(resumeAddress(path, form, fields)), 303);
}

/** Whether the request carries a session cookie: signed in once, maybe no longer. */
export const hadSession = (request: Request) => new RegExp(`(?:^|;\\s*)${SESSION_COOKIE}=`).test(request.headers.get("cookie") ?? "");

/** A value the page was given back after signing in again, bounded. */
export const resumed = (url: URL, name: string, max = 2000) => (url.searchParams.get(name) ?? "").slice(0, max);

/** Whether what came back was cut to fit. */
export const wasCut = (url: URL) => url.searchParams.get("cut") === "1";

/** Every field a form had typed into it (its token aside), for the address it comes back to. */
export const typedFields = (form: FormData | null): string[] => (form ? [...new Set(form.keys())].filter((name) => name !== "csrf") : []);

/**
 * The gate in front of every address under /admin, whether a page is there
 * or not, so that nobody but an admin can tell which are: an admin goes
 * on; a form posted from this site whose session ended goes to sign in and
 * back to the same address with what was typed (whatever the address); a
 * form from anywhere else, and everyone else, gets the site's 404.
 */
export async function adminGate(request: Request, session: WebSession | null, path: string, crossSite: boolean): Promise<Response | null> {
  if (session?.roles.includes("admin")) return null;
  if (!session && !crossSite && request.method === "POST" && hadSession(request)) {
    const form = await formOf(request);
    return new Response(null, { status: 303, headers: { location: signInUrl(resumeAddress(path, form, typedFields(form))) } });
  }
  return new Response(null, { status: 404 });
}
