// How the renderer's cost grows with what it is given, family by family
// (md/families.ts): the identity lane's attack, made a test. For each
// family, at sizes up to the README's cap:
// - what it reads (the meter's count, md/meter.ts) is at most READ_PER_BYTE
//   a byte, and grows linearly: its exponent, fitted over the sizes from
//   128 KiB (where a cheaper start, a first block with nothing before it,
//   no longer shows, as the identity lane fits it), at most 1.05;
// - what it keeps never passes KEEP_MOST, and what it writes never passes
//   the output's bound (each stops it, said so, and is held here anyway);
// - its time, a second witness, loosely: within TIME_MOST at the cap, and
//   where it is long enough to measure, an exponent at most 1.6 (a garbage
//   collector's work grows a little faster than what it collects).
// A fixed set of seeds runs here; MD_SWEEP=1 runs more sizes and seeds.

import { describe, expect, test } from "vitest";
import { FAMILIES, seeded } from "./families.ts";
import { toHtml, TooLarge } from "./html.ts";
import { KEEP_MOST, meter, type Meter } from "./meter.ts";

/** At most 2·NESTING + 8 a byte: a one-byte line under 66 open blocks is matched against each (md/blocks.ts). */
export const READ_PER_BYTE = 72;
const MOST = 2 * 1024 * 1024;
const TIME_MOST = 1_500;

const SWEEP = !!process.env.MD_SWEEP;
const SIZES = (SWEEP ? [32, 64, 128, 256, 512] : [32, 128, 512]).map((k) => k * 1024);
const SEEDS = SWEEP ? Array.from({ length: 100 }, (_, i) => i + 1) : [1, 2, 3, 4, 5];

const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };

function run(md: string): { ms: number; m: Meter } {
  const m = meter();
  const started = performance.now();
  try {
    toHtml(md, options, MOST, m);
  } catch (error) {
    if (!(error instanceof TooLarge)) throw error;
  }
  return { ms: performance.now() - started, m };
}

/** The slope of log y against log x: how y grows with x. */
function exponent(points: [number, number][]): number {
  const xs = points.map(([x]) => Math.log(x));
  const ys = points.map(([, y]) => Math.log(Math.max(y, 1e-3)));
  const mx = xs.reduce((a, b) => a + b) / xs.length;
  const my = ys.reduce((a, b) => a + b) / ys.length;
  let num = 0;
  let den = 0;
  for (let i = 0; i < xs.length; i++) {
    num += (xs[i] - mx) * (ys[i] - my);
    den += (xs[i] - mx) ** 2;
  }
  return num / den;
}

const families: [string, (n: number) => string][] = [...Object.entries(FAMILIES), ...SEEDS.map((s) => [`seeded ${s}`, seeded(s)] as [string, (n: number) => string])];

describe("linear in what it is given, family by family", () => {
  // (Warm: the first run of anything compiles it.)
  for (const [, make] of families.slice(0, 20)) run(make(4096));

  test.each(families.map(([name]) => name))("%s", (name) => {
    const make = families.find(([n]) => n === name)![1];
    const rows = SIZES.map((size) => {
      const md = make(size);
      // (The best of two: a time is the least it took.)
      const a = run(md);
      const b = run(md);
      return { size, n: md.length, read: a.m.read, peak: Math.max(a.m.peak, b.m.peak), written: a.m.written, ms: Math.min(a.ms, b.ms) };
    });
    for (const row of rows) {
      expect(row.read / row.n, `read a byte at ${row.n}`).toBeLessThanOrEqual(READ_PER_BYTE);
      expect(row.peak, `kept at ${row.n}`).toBeLessThanOrEqual(KEEP_MOST);
      expect(row.written, `written at ${row.n}`).toBeLessThanOrEqual(MOST + 64);
    }
    const large = rows.filter((r) => r.size >= 128 * 1024);
    expect(exponent(large.map((r) => [r.n, r.read])), "read's exponent").toBeLessThanOrEqual(1.05);
    const last = rows[rows.length - 1];
    expect(last.ms, "time at the cap").toBeLessThan(TIME_MOST);
    if (last.ms >= 25) expect(exponent(rows.map((r) => [r.n, r.ms])), "time's exponent").toBeLessThanOrEqual(1.6);
  });
});
