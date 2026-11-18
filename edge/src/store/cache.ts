/**
 * The Workers Cache API in front of public store reads.
 *
 * - Keys are rebuilt from a route's validated parameters (never the raw URL,
 *   a header or the host), so no request can plant an answer under another
 *   key, and junk parameters can't bust the cache.
 * - A request that carries credentials neither reads nor fills it
 *   (amendment 18): what a signed-in caller is answered is theirs alone.
 * - Only 200s are kept, for the TTL the route gives.
 * - The cache is per data center. A change (approve, yank, unyank, remove,
 *   suspend, restore, feature, transfer) purges the keys it moves here;
 *   elsewhere they age out within their TTL: 60 s for lists and pages, 300 s
 *   for packages and pictures.
 */
import { hasCredentials } from "./principal";

const CACHE_NAME = "store";
const ORIGIN = "https://store-cache.internal/v1/";

export const LIST_TTL = 60;
export const FILE_TTL = 300;

export const cacheKey = (path: string, params: Record<string, string | undefined> = {}): string => {
  const query = Object.entries(params)
    .filter((e): e is [string, string] => e[1] !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join("&");
  return `${ORIGIN}${path}${query ? `?${query}` : ""}`;
};

const open = (): Promise<Cache> => caches.open(CACHE_NAME);

/**
 * `compute`'s answer for `request`, through the cache under `key` for
 * `ttl` seconds. `compute` must answer the same for everyone without
 * credentials: it sets the public `cache-control` itself.
 */
export const throughCache = async (request: Request, key: string, ttl: number, compute: () => Promise<Response>): Promise<Response> => {
  if (hasCredentials(request)) {
    const fresh = await compute();
    const headers = new Headers(fresh.headers);
    headers.set("cache-control", "private, no-store");
    return new Response(fresh.body, { status: fresh.status, headers });
  }
  const cache = await open();
  const hit = await cache.match(key);
  if (hit) return hit;
  const fresh = await compute();
  if (fresh.status !== 200) return fresh;
  const headers = new Headers(fresh.headers);
  headers.set("cache-control", `public, max-age=${ttl}`);
  const stored = new Response(fresh.body, { status: 200, headers });
  await cache.put(key, stored.clone());
  return stored;
};

export const purge = async (keys: readonly string[]): Promise<void> => {
  const cache = await open();
  await Promise.all(keys.map((key) => cache.delete(key)));
};
