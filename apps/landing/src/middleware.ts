// Every response the Worker renders (the prerendered pages are files, and
// never pass through here at runtime): the visitor's view of the edge for
// the route to use, and the headers that keep a page what it is.
import type { APIContext } from "astro";
import { defineMiddleware } from "astro:middleware";
import { env } from "cloudflare:workers";
import { Edge } from "./lib/store/edge.ts";
import { adminGate } from "./lib/store/resume.ts";
import { contentSecurityPolicy, isCrossSiteForm, SECURITY_HEADERS } from "./lib/store/security.ts";

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
    if (refused) return secure(context, refused, edge.setCookies);
  }
  // A form from another site (Astro's check, made here: astro.config.mjs).
  if (crossSite) return new Response(`Cross-site ${request.method} form submissions are forbidden`, { status: 403 });
  // One address per page: "/plugins//x" and "//plugins/x" are "/plugins/x".
  if (plain !== sent.pathname) {
    return new Response(null, { status: 301, headers: { location: `${plain}${sent.search}`, "cache-control": "public, max-age=3600" } });
  }
  return secure(context, await next(), edge.setCookies);
});

async function secure(context: APIContext, response: Response, cookies: string[]): Promise<Response> {
  // The site's 404 page goes out exactly as it does for an address that
  // doesn't exist (a file, with that file's headers), so a page that
  // answers "not here" can't be told from one that isn't there. (Only a
  // staff session ever rotates, so only an admin's 404 carries a cookie.)
  // (A route's "not here" is still empty at this point: the 404 page is
  // put in after. A file route's 404 keeps the headers it set itself.)
  if (response.status === 404) {
    if (cookies.length === 0) return response;
    const notFound = new Headers(response.headers);
    for (const cookie of cookies) notFound.append("set-cookie", cookie);
    notFound.set("cache-control", "private, no-store");
    return new Response(response.body, { status: 404, statusText: response.statusText, headers: notFound });
  }
  const headers = new Headers(response.headers);
  // The session the edge rotated while it answered goes back to the browser,
  // and a response that carries one is that person's alone: never kept.
  for (const cookie of cookies) headers.append("set-cookie", cookie);
  if (cookies.length > 0) {
    headers.set("cache-control", "private, no-store");
    // (Astro adds a page's shared-cache headers after this runs: told here
    // to add none, so no cache keeps one person's cookie for the next.)
    context.cache.set(false);
  }
  for (const [name, value] of Object.entries(SECURITY_HEADERS)) if (!headers.has(name)) headers.set(name, value);
  const type = headers.get("content-type") ?? "";
  // A file from a package keeps the sandbox its route gave it.
  if (!type.startsWith("text/html") || headers.has("content-security-policy")) {
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  }
  // A page: its own inline styles (Astro's, and the fonts') are allowed by
  // their hashes, and nothing else inline is.
  const html = await response.text();
  headers.set("content-security-policy", await contentSecurityPolicy(html));
  return new Response(html, { status: response.status, statusText: response.statusText, headers });
}
