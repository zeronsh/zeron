// How long a page may be kept, and by whom.
import type { AstroGlobal } from "astro";

/** Seconds a public store page is fresh, then may be served stale while it's asked again. */
export const FRESH = 30;
export const STALE = 30;

/**
 * A page that is the same for everyone (it reads no cookie): the Worker
 * cache keeps it, and so may the browser, for as long as the store allows
 * data to be old (a minute, all told).
 */
export function publicPage(astro: Pick<AstroGlobal, "cache" | "response">): void {
  astro.cache.set({ maxAge: FRESH, swr: STALE });
  astro.response.headers.set("cache-control", `public, max-age=${FRESH}`);
}

/** One person's page: never kept anywhere, never indexed. */
export function privatePage(astro: Pick<AstroGlobal, "response">): void {
  astro.response.headers.set("cache-control", "private, no-store");
  astro.response.headers.set("x-robots-tag", "noindex, nofollow");
}
