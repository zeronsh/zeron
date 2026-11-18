// The headers that keep each response the Worker renders what it is (the
// middleware's last step): the cookies the edge set while it answered, the
// site's security headers, and a page's Content Security Policy.
import { CONTENT_SECURITY_POLICY, SECURITY_HEADERS } from "./security.ts";

export async function secure(cache: { set(input: false): void }, response: Response, cookies: string[]): Promise<Response> {
  // A response that carries someone's cookie is theirs alone: Astro adds a
  // page's shared-cache headers after the middleware returns, so it is told
  // here to add none, and no cache keeps one person's cookie for the next.
  if (cookies.length > 0) cache.set(false);
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
