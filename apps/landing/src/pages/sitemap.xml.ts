// The sitemaps: the site's own pages (built with it), and the store's
// (made as the store changes).
import type { APIRoute } from "astro";
import { sitemapIndex, XML } from "../lib/store/sitemap.ts";

export const GET: APIRoute = ({ site }) =>
  new Response(sitemapIndex([new URL("/sitemap-pages.xml", site).href, new URL("/plugins/sitemap.xml", site).href]), { headers: XML });
