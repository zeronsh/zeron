/**
 * Badges a member earns by what they did, rather than one an operator grants.
 * The directory decides them from facts it stores (user-directory.ts) and
 * never from what a client says. Once earned, a badge stays: a rename, a
 * cleared or released name or a new claim never takes it back. Two things do:
 * deleting the account, with everything else on the card, and an operator
 * revoking it (`/admin/badges/revoke`), which waives it for that member so the
 * award never gives it back. Granting it to them again lifts the waiver and
 * awards it only if they earned it. An operator can't define an earned badge,
 * or grant one to "all".
 */

/** V3 BETA: the member claimed their card (a username) during the beta. */
export const V3_BETA = {
  id: "v3-beta",
  name: "V3 BETA",
  description: "Claimed a card before 2026-11-05 01:00 PST.",
  icon: null
} as const;

/**
 * The end of the beta: 2026-11-05 01:00 PST. Daylight saving ended on
 * 2026-11-01, so PST is UTC−8 and this is 09:00 UTC. A first claim strictly
 * before this instant earns V3 BETA; one at or after it doesn't.
 */
export const V3_BETA_CUTOFF_MS = Date.UTC(2026, 10, 5, 9, 0, 0);

/** Whether a member whose first claim was at `firstClaimedAt` (epoch ms,
 * null if they never held a name) earned V3 BETA. The one place this is
 * decided. */
export const earnsV3Beta = (firstClaimedAt: number | null): boolean =>
  firstClaimedAt !== null && firstClaimedAt < V3_BETA_CUTOFF_MS;

/** Every earned badge: the ids `/admin/badges` won't touch. */
export const EARNED_BADGES: readonly { readonly id: string; readonly name: string; readonly description: string; readonly icon: null }[] = [V3_BETA];

export const isEarnedBadge = (id: string): boolean => EARNED_BADGES.some((b) => b.id === id);
