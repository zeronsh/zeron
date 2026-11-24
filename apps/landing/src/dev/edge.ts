// The fake edge's store as if it were down, for a screenshot (dev only:
// astro.config.mjs devPages): /_dev/edge?down=1, or ?down=0. Then where `to` says.
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";

export const prerender = false;

export const GET: APIRoute = async ({ url, redirect }) => {
  await env.EDGE.fetch(new Request(`https://edge.invalid/_fake/down?on=${url.searchParams.get("down") === "1" ? "1" : "0"}`, { method: "POST" }));
  const to = url.searchParams.get("to") ?? "/plugins";
  return redirect(to.startsWith("/") && !to.startsWith("//") ? to : "/plugins", 303);
};
