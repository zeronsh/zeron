// Every response the Worker renders (the prerendered pages are files, and
// never pass through here at runtime): the visitor's view of the edge for
// the route to use, and the headers that keep a page what it is.
import { defineMiddleware } from "astro:middleware";
import { env } from "cloudflare:workers";
import { Edge } from "./lib/store/edge.ts";
import { adminGate } from "./lib/store/resume.ts";
import { secure } from "./lib/store/secure.ts";
import { isCrossSiteForm } from "./lib/store/security.ts";

export const onRequest = defineMiddleware(async (context, next) => {
  if (context.isPrerendered) return next();
  const request = context.request;
  // The address as sent (Astro's own is already tidied).
  const sent = new URL(request.url);
  const plain = sent.pathname.replace(/\/{2,}/g, "/");
  const crossSite = isCrossSiteForm(request);
  const edge = new Edge(env.EDGE, request);
  context.locals.edge = edge;
  // The admin pages, every address under /admin whether a page is there or
  // not (pages/admin/[...rest].ts sends the rest here too): nobody but an
  // admin can tell any of them from an address that isn't there, by any
  // answer, before anything else is asked of them.
  if (plain === "/admin" || plain.startsWith("/admin/")) {
    const refused = await adminGate(request, await edge.session(), plain, crossSite);
    if (refused) return secure(context.cache, refused, edge.setCookies);
  }
  // A form from another site (Astro's check, made here: astro.config.mjs).
  if (crossSite) return new Response(`Cross-site ${request.method} form submissions are forbidden`, { status: 403 });
  // One address per page: "/plugins//x" and "//plugins/x" are "/plugins/x".
  if (plain !== sent.pathname) {
    return new Response(null, { status: 301, headers: { location: `${plain}${sent.search}`, "cache-control": "public, max-age=3600" } });
  }
  return secure(context.cache, await next(), edge.setCookies);
});
