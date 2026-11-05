// Subsets the card's four Geist faces (apps/zgx/fonts, the faces the native
// host registers) to the characters a card can show and writes them as WOFF2
// into site/fonts. Run from anywhere: `node tools/fonts.mjs`.
//
// The card sets its name in Geist SemiBold, its mark in Geist Bold, its
// number, date, tagline and caption in Geist Mono, and its badges in Geist
// Mono Medium. The subset keeps Basic Latin, Latin-1, Latin Extended-A and
// the general punctuation block (the caption's em dash and middle dot).
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import subsetFont from "subset-font";

const here = dirname(fileURLToPath(import.meta.url));
const fonts = join(here, "..", "..", "zgx", "fonts");
const out = join(here, "..", "site", "fonts");

const FACES = [
  ["Geist-SemiBold.ttf", "geist-semibold.woff2"],
  ["Geist-Bold.ttf", "geist-bold.woff2"],
  ["GeistMono.ttf", "geist-mono.woff2"],
  ["GeistMono-Medium.ttf", "geist-mono-medium.woff2"]
];

const range = (from, to) => {
  let s = "";
  for (let c = from; c <= to; c++) s += String.fromCodePoint(c);
  return s;
};
const TEXT = range(0x20, 0x7e) + range(0xa0, 0x17f) + range(0x2000, 0x206f);

await mkdir(out, { recursive: true });
for (const [source, target] of FACES) {
  const ttf = await readFile(join(fonts, source));
  const woff2 = await subsetFont(ttf, TEXT, { targetFormat: "woff2" });
  await writeFile(join(out, target), woff2);
  console.log(`${target}: ${ttf.length} → ${woff2.length} bytes`);
}
