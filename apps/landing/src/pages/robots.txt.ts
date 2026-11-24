// Everything public may be read; where the sitemap is.
import type { APIRoute } from "astro";

export const GET: APIRoute = ({ site }) =>
  new Response(`User-agent: *\nAllow: /\n\nSitemap: ${new URL("/sitemap.xml", site)}\n`, { headers: { "content-type": "text/plain; charset=utf-8" } });
