// A plugin's own colour, from its id alone (never its icon's pixels): the
// store's identity, every plugin on its own hue, so a list reads as many
// things and a plugin is known by its colour wherever it shows.
//
// The id's UTF-8 bytes through 32-bit FNV-1a (offset 0x811c9dc5, prime
// 0x01000193), its value modulo 24: one of 24 steps 15° apart round the
// oklch circle. FNV-1a is small, stable everywhere and spreads ids that
// differ by a letter ("focus-timer", "focus-timers") far apart. A step, not
// a degree, because the page may carry no style of its own (its policy
// allows only the build's): each step is a class, `.h0` to `.h23`, setting
// `--h` (styles/look/store.css), and each has its ink filter for icons
// (components/look/Inks.astro).

export const HUE_STEPS = 24;

/** The id's step round the hue circle, 0 to 23. */
export function hueStep(id: string): number {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(id)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % HUE_STEPS;
}

/** The class that gives an element the id's hue. */
export const hueClass = (id: string) => `h${hueStep(id)}`;
