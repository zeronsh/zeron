// Every listed plugin's page and each category's, for search engines:
// read from the catalog a page at a time, kept for an hour.
import type { APIRoute } from "astro";
import { urlset, XML } from "../../lib/store/sitemap.ts";
import { pluginUrl } from "../../lib/store/urls.ts";

export const prerender = false;

/** A sitemap holds 50,000 addresses at most; the store is far from it, and this stops well short. */
const MOST_PAGES = 200;

export const GET: APIRoute = async ({ locals, site, cache }) => {
  const entries: { loc: string; lastmod?: number }[] = [];
  const categories = await locals.edge.categories();
  if (categories.ok) for (const c of categories.data.categories) entries.push({ loc: new URL(`/plugins?category=${encodeURIComponent(c.id)}`, site).href });
  let cursor: string | undefined;
  for (let page = 0; page < MOST_PAGES; page++) {
    const result = await locals.edge.catalog({ sort: "new", limit: 50, cursor });
    if (!result.ok) return new Response(null, { status: 503, headers: { "retry-after": "60" } });
    for (const p of result.data.items) entries.push({ loc: new URL(pluginUrl(p.id), site).href, lastmod: p.updated_at });
    cursor = result.data.next_cursor;
    if (!cursor) break;
  }
  cache.set({ maxAge: 3600 });
  return new Response(urlset(entries), { headers: { ...XML, "cache-control": "public, max-age=3600" } });
};
