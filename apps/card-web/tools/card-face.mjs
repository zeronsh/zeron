// The member card function, ported from edge/src/card.ts (the card-usernames
// branch): a card comes from a seed and nothing else. `node tools/card-face.mjs`
// checks the port against the published test vectors.
export const CARD_SCENES = 20;
export const HOLOS = ["aurora", "reverse", "secret", "starlight", "glyph"];

export const cardHash = (s) => {
  let h = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(s)) h = Math.imul(h ^ byte, 0x01000193);
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
};

export const deriveCard = (seed, scenes = CARD_SCENES) => ({
  scene: cardHash(`${seed}:scene`) % scenes,
  holo: HOLOS[cardHash(`${seed}:holo`) % HOLOS.length],
  sky: cardHash(`${seed}:sky`)
});

if (process.argv[1] && import.meta.url.endsWith(process.argv[1].split("/").pop())) {
  const vectors = [
    ["", 2872998923], ["a", 444641715], ["zeron", 3920131753],
    ["user_test_0001:scene", 62640894], ["user_test_0001:holo", 1378625095], ["user_test_0001:sky", 1678271501],
    ["é", 2387039943], ["ユーザー", 3445456616], ["🂡", 3106548733]
  ];
  let bad = 0;
  for (const [input, hash] of vectors) if (cardHash(input) !== hash) { bad++; console.log(`hash(${JSON.stringify(input)}) = ${cardHash(input)}, want ${hash}`); }
  const cards = [["user_test_0001", 14, "aurora", 1678271501], ["user_test_0002", 10, "reverse", 2564363760], ["user_mara", 19, "secret", 3389156178]];
  for (const [id, scene, holo, sky] of cards) {
    const c = deriveCard(id, 20);
    if (c.scene !== scene || c.holo !== holo || c.sky !== sky) { bad++; console.log(`${id}: ${JSON.stringify(c)}`); }
  }
  console.log(bad ? `${bad} mismatches` : `card function: ${vectors.length + cards.length} vectors pass`);
}
