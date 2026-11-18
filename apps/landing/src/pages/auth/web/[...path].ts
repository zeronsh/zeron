// Web sign-in is the edge's (/auth/web/login, callback, logout, session).
// On zeron.sh the edge's own path route answers these before the site ever
// sees them; anywhere it doesn't (a local copy of the site, a preview),
// they are handed to the edge through the binding as they came, and its
// answer (its redirects, its cookie) comes back as it gave it.
import type { APIRoute } from "astro";
import { env } from "cloudflare:workers";

export const prerender = false;

export const ALL: APIRoute = ({ request }) => env.EDGE.fetch(new Request(request, { redirect: "manual" }));
