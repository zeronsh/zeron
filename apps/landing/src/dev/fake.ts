// Any of the fake edge's own switches, for a screenshot (dev only:
// astro.config.mjs devPages): /_dev/fake?do=bare&id=pr-radar&to=/plugins
// posts /_fake/bare?id=pr-radar, then goes where `to` says.
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";

export const prerender = false;

export const GET: APIRoute = async ({ url, redirect }) => {
  const params = new URLSearchParams(url.searchParams);
  const what = params.get("do") ?? "";
  const to = params.get("to") ?? "/plugins";
  params.delete("do");
  params.delete("to");
  if (/^[a-z]+$/.test(what)) await env.EDGE.fetch(new Request(`https://edge.invalid/_fake/${what}?${params}`, { method: "POST" }));
  return redirect(to.startsWith("/") && !to.startsWith("//") ? to : "/plugins", 303);
};
