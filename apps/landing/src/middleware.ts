// Every response the Worker renders (the prerendered pages are files, and
// never pass through here at runtime): the visitor's view of the edge for
// the route to use, and the headers that keep a page what it is.
import { defineMiddleware } from "astro:middleware";
import { env } from "cloudflare:workers";
import { Edge } from "./lib/store/edge.ts";
import { MOST_FORM } from "./lib/store/forms.ts";
import { canonicalPath, isStaffPath, isStorePath } from "./lib/store/paths.ts";
import { adminGate } from "./lib/store/resume.ts";
import { secure } from "./lib/store/secure.ts";
import { isCrossSiteForm } from "./lib/store/security.ts";

export const onRequest = defineMiddleware(async (context, next) => {
  // (A built page is a file: its headers are the built files', _headers,
  // which astro.config.mjs writes for the 404 page.)
  if (context.isPrerendered) return next();
  const request = context.request;
  // The address as sent (Astro's own is already tidied, and decoded twice),
  // and its one canonical form (lib/store/paths.ts).
  const sent = new URL(request.url);
  const canonical = canonicalPath(sent.pathname);
  const crossSite = isCrossSiteForm(request);
  const edge = new Edge(env.EDGE, request);
  context.locals.edge = edge;
  // The admin pages, every address under /admin whether a page is there or
  // not (pages/admin/[...rest].ts sends the rest here too), as the router
  // reads it (the route it matched, or the canonical path, never only the
  // path as sent): nobody but an admin can tell any of them from an address
  // that isn't there, by any answer, before anything else is asked of them.
  if (isStaffPath(canonical) || isStaffPath(context.routePattern)) {
    const refused = await adminGate(request, await edge.session(), canonical, crossSite);
    if (refused) return secure(context.cache, refused, edge.setCookies);
  }
  // A form from another site (Astro's check, made here: astro.config.mjs).
  if (crossSite) return new Response(`Cross-site ${request.method} form submissions are forbidden`, { status: 403 });
  // The store's pages on another name of this Worker (an old hostname, its
  // workers.dev address) are the site's own origin's.
  if (isStorePath(canonical) && otherName(sent.host)) {
    const origin = env.SITE_ORIGIN ?? "https://zeron.sh";
    return new Response(null, { status: 301, headers: { location: `${origin}${canonical}${sent.search}`, "cache-control": "public, max-age=3600" } });
  }
  // A form's body past what any form here sends: never read (forms.ts MOST_FORM).
  if (request.method === "POST" && Number(request.headers.get("content-length") ?? "0") > MOST_FORM) {
    return new Response("Too large", { status: 413, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
  }
  // One address per page: "/plugins//x", "//plugins/x" and "/%70lugins/x" are "/plugins/x".
  if (canonical !== sent.pathname) {
    return new Response(null, { status: 301, headers: { location: `${canonical}${sent.search}`, "cache-control": "public, max-age=3600" } });
  }
  return secure(context.cache, await next(), edge.setCookies);
});

/** This Worker's other names: its old hostname, and its workers.dev address. */
const otherName = (host: string) => host === "comet.zeron.sh" || host.endsWith(".workers.dev");
