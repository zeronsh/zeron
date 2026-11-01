/**
 * The member card function: a card comes from a seed, the user's id, and from
 * nothing else that can change. Pure and free of imports, so any client can
 * port it; docs/card-vectors.json is the list every port must reproduce.
 *
 *   hash(s)  FNV-1a 32-bit over the UTF-8 bytes, then murmur3 fmix32
 *   scene    hash(id + ":scene") % scenes, a 0-based index into the scenes
 *            ordered by `number`
 *   holo     hash(id + ":holo") % 5, indexing HOLOS
 *   sky      hash(id + ":sky"), the character field's offset
 *
 * `scenes` is the count stored with the user at their first username, never
 * the current one: new scenes are appended, and a member's card must not move
 * when they are.
 */

/** How many scenes a new card can draw from. Scenes are only ever appended;
 * bump this with the manifest. */
export const CARD_SCENES = 26;

export const HOLOS = ["aurora", "reverse", "secret", "starlight", "glyph"] as const;

export type Holo = (typeof HOLOS)[number];

export interface CardFace {
  /** 0-based index into the scenes ordered by `number`. */
  readonly scene: number;
  readonly holo: Holo;
  /** An unsigned 32-bit value. */
  readonly sky: number;
}

export const cardHash = (s: string): number => {
  let h = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(s)) h = Math.imul(h ^ byte, 0x01000193);
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
};

/** Longest first name a card shows, in characters. */
export const MAX_CARD_FIRST_NAME = 32;

/**
 * The only name a card shows: the first word of the WorkOS first name, or the
 * username when there is none. The first word, because sign-ins often put the
 * whole name in `first_name` (and an initial after it), and a card must never
 * show a surname; a two-word first name shows its first. The last name is
 * never read, so a user with only a last name shows their username.
 */
export const cardFirstName = (firstName: string | null, username: string): string => {
  const word = (firstName ?? "")
    .normalize("NFC")
    .replace(/[\p{C}]/gu, " ")
    .trim()
    .split(/\s+/u)[0];
  return word ? [...word].slice(0, MAX_CARD_FIRST_NAME).join("") : username;
};

export const deriveCard = (seed: string, scenes: number): CardFace => ({
  scene: cardHash(`${seed}:scene`) % scenes,
  holo: HOLOS[cardHash(`${seed}:holo`) % HOLOS.length]!,
  sky: cardHash(`${seed}:sky`)
});
