/**
 * The edge cache in front of public card reads, so a shared link never lands
 * on the one UserDirectory instance. A single entry per username, whichever
 * host or route asked, and unclaimed names are cached too, more briefly. The
 * cache is per data center: `forgetCard` clears the one a change came through,
 * and elsewhere an entry ages out within the TTL.
 */
import type { PublicCard } from "./user-directory";

/** Seconds a card may be served stale (a renamed user, a new badge). */
export const CARD_TTL_SECONDS = 60;
/** Seconds an unclaimed name may stay unclaimed after it's taken. */
export const MISSING_CARD_TTL_SECONDS = 30;

const CARD_CACHE = "cards";

/** `v2`: cards cached before first names only carried full names. */
const cacheKey = (username: string): string => `https://cards.internal/v2/${username}`;

/** The card, or null for a name no one holds; undefined if not cached. */
export const cachedCard = async (username: string): Promise<PublicCard | null | undefined> => {
  const hit = await (await caches.open(CARD_CACHE)).match(cacheKey(username));
  return hit ? ((await hit.json()) as PublicCard | null) : undefined;
};

export const cacheCard = async (username: string, card: PublicCard | null): Promise<void> => {
  const ttl = card ? CARD_TTL_SECONDS : MISSING_CARD_TTL_SECONDS;
  await (await caches.open(CARD_CACHE)).put(
    cacheKey(username),
    new Response(JSON.stringify(card), { headers: { "cache-control": `public, max-age=${ttl}` } })
  );
};

export const forgetCard = async (username: string): Promise<void> => {
  await (await caches.open(CARD_CACHE)).delete(cacheKey(username));
};
