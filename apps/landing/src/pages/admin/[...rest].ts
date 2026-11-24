// Every other address under /admin: the site's 404, as for any address that
// isn't there. (It's here so the middleware's admin gate sees these too, and
// answers them as it answers the pages that are there.)
import type { APIRoute } from "astro";

export const prerender = false;

export const ALL: APIRoute = () => new Response(null, { status: 404 });
