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
export function publicPage(astro: Pick<AstroGlobal, "cache" | "response">, tags: string[] = []): void {
  astro.cache.set({ maxAge: FRESH, swr: STALE, tags });
  astro.response.headers.set("cache-control", `public, max-age=${FRESH}`);
}

/** What a public page shows, as its cache tags: the catalog's lists, or one plugin. */
export const CATALOG_TAG = "catalog";
export const pluginTag = (id: string) => `plugin:${id}`;

/**
 * After a change made here (a takedown, an approval, a version no longer
 * offered), the pages that showed the plugin are dropped from the Worker
 * cache at once, rather than in their minute. (Changes made elsewhere are
 * the edge's to purge; these pages are at most a minute behind it.)
 */
export async function purge(astro: Pick<AstroGlobal, "cache">, id: string): Promise<void> {
  await astro.cache.invalidate({ tags: [CATALOG_TAG, pluginTag(id)] }).catch(() => {});
}

/** One person's page: never kept anywhere, never indexed. */
export function privatePage(astro: Pick<AstroGlobal, "response">): void {
  astro.response.headers.set("cache-control", "private, no-store");
  astro.response.headers.set("x-robots-tag", "noindex, nofollow");
}
