// The fake edge's queue as a screenshot needs it (dev only: astro.config.mjs
// devPages): /_dev/queue?as=empty, ?as=40, or ?as= for as it is. Then the queue.
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";

export const prerender = false;

export const GET: APIRoute = async ({ url, redirect }) => {
  await env.EDGE.fetch(new Request(`https://edge.invalid/_fake/queue?${new URLSearchParams({ as: url.searchParams.get("as") ?? "" })}`, { method: "POST" }));
  return redirect("/admin/plugins", 303);
};
