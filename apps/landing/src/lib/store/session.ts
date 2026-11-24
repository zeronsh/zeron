// Who is signed in, as the edge says (the site keeps no session of its
// own), and what a page does about it.
import type { AstroGlobal } from "astro";
import type { WebSession } from "./types.ts";
import { signInUrl } from "./urls.ts";

type Page = Pick<AstroGlobal, "locals" | "url" | "redirect">;

/** The session, or a redirect to sign in and come back here. */
export async function requireSession(astro: Page): Promise<WebSession | Response> {
  const session = await astro.locals.edge.session();
  return session ?? astro.redirect(signInUrl(astro.url.pathname + astro.url.search), 303);
}

/**
 * The session when it holds the `admin` role, else null: the caller answers
 * with the site's own 404, so the admin pages can't be told from pages
 * that don't exist, signed in or not.
 */
export async function adminSession(astro: Pick<AstroGlobal, "locals">): Promise<WebSession | null> {
  const session = await astro.locals.edge.session();
  return session?.roles.includes("admin") ? session : null;
}

export const isAdmin = (session: WebSession | null) => session?.roles.includes("admin") ?? false;
