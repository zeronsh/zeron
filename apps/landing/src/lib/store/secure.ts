// The headers that keep each response the Worker renders what it is (the
// middleware's last step): the cookies the edge set while it answered, the
// site's security headers, and a page's Content Security Policy.
import { CONTENT_SECURITY_POLICY, SECURITY_HEADERS } from "./security.ts";

export async function secure(cache: { set(input: false): void }, response: Response, cookies: string[]): Promise<Response> {
  // A response that carries someone's cookie is theirs alone: Astro adds a
  // page's shared-cache headers after the middleware returns, so it is told
  // here to add none, and no cache keeps one person's cookie for the next.
  if (cookies.length > 0) cache.set(false);
  // An empty 404 (a route with nothing here) is answered by the site's 404
  // page, put in after this: a file, with that file's headers, so a page
  // that answers "not here" this way can't be told from one that isn't
  // there. (Only a staff session ever rotates, so only an admin's 404
  // carries a cookie.) A 404 the Worker drew itself, with a body (a plugin
  // that isn't listed), is a page like any other: it gets the headers
  // below, its CSP among them, and is never kept.
  if (response.status === 404 && response.body === null) {
    if (cookies.length === 0) return response;
    const notFound = new Headers(response.headers);
    for (const cookie of cookies) notFound.append("set-cookie", cookie);
    notFound.set("cache-control", "private, no-store");
    return new Response(response.body, { status: 404, statusText: response.statusText, headers: notFound });
  }
  const headers = new Headers(response.headers);
  if (response.status === 404 && !headers.has("cache-control")) headers.set("cache-control", "no-store");
  // The session the edge rotated while it answered goes back to the browser,
  // and a response that carries one is that person's alone: never kept.
  for (const cookie of cookies) headers.append("set-cookie", cookie);
  if (cookies.length > 0) headers.set("cache-control", "private, no-store");
  for (const [name, value] of Object.entries(SECURITY_HEADERS)) if (!headers.has(name)) headers.set(name, value);
  const type = headers.get("content-type") ?? "";
  // A file from a package keeps the sandbox its route gave it.
  if (!type.startsWith("text/html") || headers.has("content-security-policy")) {
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  }
  // A page: the build's own inline styles are allowed by their hashes
  // (security.ts), and nothing else inline is.
  headers.set("content-security-policy", CONTENT_SECURITY_POLICY);
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
}
