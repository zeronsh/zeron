// A plugin's tile, as the app draws it (apps/zgx/ui/screens/plugins/words.luau
// `tile` and `hue`, ds/tile.luau): the icon it ships as a glyph in the text's
// ink on the quiet wash, else its name's initial on its hue.

const HUES = ["indigo", "rose", "sky", "emerald", "amber", "violet", "orange", "teal"] as const;
export type Hue = (typeof HUES)[number];

/** A plugin's hue, steady per id: the app's own sum over the id's bytes. */
export function hue(id: string): Hue {
  let n = 0;
  for (const byte of new TextEncoder().encode(id)) n = (n * 31 + byte) % 9973;
  return HUES[n % HUES.length]!;
}

/** Its name's first character, whole (a name may start past ASCII), in capitals. */
export function initial(name: string): string {
  const first = new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(name.trim())[Symbol.iterator]().next().value;
  return first ? first.segment.toLocaleUpperCase() : "?";
}

/** A tile's corner, a step bigger as the tile is (ds/tile.luau `corner`). */
export function corner(size: number): "sm" | "md" | "xl" {
  return size >= 56 ? "xl" : size >= 32 ? "md" : "sm";
}
