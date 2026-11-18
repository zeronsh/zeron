// Every response the Worker renders (the prerendered pages are files, and
// never pass through here at runtime): the visitor's view of the edge for
// the route to use, and the headers that keep a page what it is.
import { defineMiddleware } from "astro:middleware";
import { env } from "cloudflare:workers";
import { Edge } from "./lib/store/edge.ts";
import { contentSecurityPolicy, SECURITY_HEADERS } from "./lib/store/security.ts";

export const onRequest = defineMiddleware(async (context, next) => {
  if (context.isPrerendered) return next();
  const edge = new Edge(env.EDGE, context.request);
  context.locals.edge = edge;
  const response = await next();
  return secure(response, edge.setCookies);
});

async function secure(response: Response, cookies: string[]): Promise<Response> {
  const headers = new Headers(response.headers);
  // The session the edge rotated while it answered goes back to the browser,
  // and a response that carries one is that person's alone: never kept.
  for (const cookie of cookies) headers.append("set-cookie", cookie);
  if (cookies.length > 0) {
    headers.set("cache-control", "private, no-store");
    headers.delete("cloudflare-cdn-cache-control");
    headers.delete("cache-tag");
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
