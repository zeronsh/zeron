/**
 * The Workers Cache API in front of public store reads.
 *
 * - A key is built from a route's validated parts, each percent-encoded
 *   behind a `~` as one path segment (`cacheKey`), never from the raw URL, a header or the
 *   host: no part can be `..` to the URL parser, so no request can reach
 *   another key, and junk parameters can't bust the cache.
 * - A request that carries credentials neither reads nor fills it
 *   (amendment 18): what a signed-in caller is answered is theirs alone.
 * - Only a GET fills it, and only with a 200. A HEAD reads it (its body
 *   dropped) and never fills it, so an empty answer can't stand in for a
 *   GET's.
 * - The cache is per data center. Lists and pages live 60 s (`LIST_TTL`).
 *   A package or file is served only after its version's state is checked,
 *   and that state is kept at most 60 s (`STATE_TTL`, amendment 23); the
 *   bytes behind it are kept a day under their sha256, since they never
 *   change. A change purges the keys it moves here; elsewhere they age out
 *   within those 60 s, so a takedown reaches every data center within a
 *   minute.
 */
import { hasCredentials } from "./principal";

const CACHE_NAME = "store";
const ORIGIN = "https://store-cache.internal/v1/";

export const LIST_TTL = 60;
export const STATE_TTL = 60;
export const BYTES_TTL = 86_400;

export const cacheKey = (segments: readonly string[], params: Record<string, string | undefined> = {}): string => {
  const query = Object.entries(params)
    .filter((e): e is [string, string] => e[1] !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join("&");
  // Every segment encoded and behind a `~`: the URL parser takes `.`, `..`,
  // `%2e%2e` (any case) as dot segments, never `~` followed by anything.
  const path = segments.map((s) => `~${encodeURIComponent(s)}`).join("/");
  const key = `${ORIGIN}${path}${query ? `?${query}` : ""}`;
  // The key is used as a URL: it must read back as exactly what was built.
  if (new URL(key).href !== key) throw new Error(`store: a cache key that doesn't read back: ${key}`);
  return key;
};

export const openCache = (): Promise<Cache> => caches.open(CACHE_NAME);

const headOnly = (response: Response): Response => new Response(null, { status: response.status, headers: response.headers });

/**
 * `compute`'s answer for `request`, through the cache under `key` for
 * `ttl` seconds. `compute` must answer the same for everyone without
 * credentials (it sets the public `cache-control` itself) and always with
 * its body: a HEAD is answered from it, body dropped.
 */
export const throughCache = async (request: Request, key: string, ttl: number, compute: () => Promise<Response>): Promise<Response> => {
  const head = request.method === "HEAD";
  if (hasCredentials(request)) {
    const fresh = await compute();
    const headers = new Headers(fresh.headers);
    headers.set("cache-control", "private, no-store");
    return new Response(head ? null : fresh.body, { status: fresh.status, headers });
  }
  const cache = await openCache();
  const hit = await cache.match(key);
  if (hit) return head ? headOnly(hit) : hit;
  const fresh = await compute();
  if (fresh.status !== 200 || head) return head ? headOnly(fresh) : fresh;
  const headers = new Headers(fresh.headers);
  headers.set("cache-control", `public, max-age=${ttl}`);
  const stored = new Response(fresh.body, { status: 200, headers });
  await cache.put(key, stored.clone());
  return stored;
};

/** A value kept in the cache as JSON for `ttl` seconds (`undefined` is never
 * kept). For a request with credentials, always fresh. */
export const cachedValue = async <T>(request: Request, key: string, ttl: number, compute: () => Promise<T | undefined>): Promise<T | undefined> => {
  if (hasCredentials(request)) return compute();
  const cache = await openCache();
  const hit = await cache.match(key);
  if (hit) return (await hit.json()) as T;
  const value = await compute();
  if (value !== undefined) {
    await cache.put(key, new Response(JSON.stringify(value), { headers: { "content-type": "application/json", "cache-control": `public, max-age=${ttl}` } }));
  }
  return value;
};

/** Immutable bytes kept under their content's key for a day. */
export const cachedBytes = async (request: Request, key: string, read: () => Promise<Uint8Array | undefined>): Promise<Uint8Array | undefined> => {
  if (hasCredentials(request)) return read();
  const cache = await openCache();
  const hit = await cache.match(key);
  if (hit) return new Uint8Array(await hit.arrayBuffer());
  const bytes = await read();
  if (bytes) await cache.put(key, new Response(bytes, { headers: { "cache-control": `public, max-age=${BYTES_TTL}` } }));
  return bytes;
};

export const purge = async (keys: readonly string[]): Promise<void> => {
  const cache = await openCache();
  await Promise.all(keys.map((key) => cache.delete(key)));
};
