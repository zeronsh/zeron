/**
 * The gate in front of every public card route: `/@…` (the page and anything
 * under it) and `/cards/…`. It limits reads per client IP, so usernames can't
 * be enumerated in bulk, and turns any failure behind it into one plain 503,
 * so no internal detail reaches the public. The gate runs before the edge
 * cache, so cached answers count too.
 *
 * There is no listing or search of usernames, and every name that isn't a
 * live card answers the same 404, so a reader learns only "card" or "no card",
 * at most CARD_READS's rate.
 */
import type { Env } from "./env";

/** Whether `pathname` is a public card route. */
export const isPublicCardPath = (pathname: string): boolean => /^\/(?:@|%40|cards(?:\/|$))/i.test(pathname);

const plain = (status: number, error: string, extra: Record<string, string> = {}): Response =>
  new Response(JSON.stringify({ error }), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...extra }
  });

/**
 * Serve a public card route through the gate: undefined when `url` isn't one,
 * else `serve`'s answer, a 429 past the limit, or a 503 if it throws. Requests
 * with no `cf-connecting-ip` (a service binding, a test) aren't limited:
 * Cloudflare sets it on every request from outside.
 */
export const publicCardGate = async (
  request: Request,
  env: Pick<Env, "CARD_READS">,
  url: URL,
  serve: () => Promise<Response | undefined>
): Promise<Response | undefined> => {
  if (!isPublicCardPath(url.pathname)) return undefined;
  const ip = request.headers.get("cf-connecting-ip");
  if (ip && env.CARD_READS && !(await env.CARD_READS.limit({ key: ip })).success) {
    return plain(429, "rate_limited", { "retry-after": "60" });
  }
  try {
    return (await serve()) ?? plain(404, "not_found");
  } catch (e) {
    console.error("cards: a public read failed", String(e));
    return plain(503, "unavailable");
  }
};
