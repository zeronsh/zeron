// The site's own pages.
import type { APIRoute } from "astro";
import { urlset, XML } from "../lib/store/sitemap.ts";

export const GET: APIRoute = ({ site }) => new Response(urlset(["/", "/privacy", "/plugins"].map((path) => ({ loc: new URL(path, site).href }))), { headers: XML });
