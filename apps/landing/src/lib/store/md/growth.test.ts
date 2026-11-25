// How the renderer's cost grows with what it is given, family by family
// (md/families.ts): the identity lane's attack, made a test. For each
// family, at sizes up to the README's cap:
// - what it reads (the meter's count, md/meter.ts) is at most READ_PER_BYTE
//   a byte, at every size, stopped or not;
// - and grows linearly, judged only over the sizes where it was not stopped
//   (a stopped run measures the bound, not the renderer). A family drawn at
//   fewer than three of the sizes is measured at 4, 8 and 16 KiB too, so a
//   family stopped from 64 or 128 KiB on still has its growth checked; one
//   that is still drawn at fewer than three is named in UNFITTED, with why.
//   A family's exponent, fitted from 32 KiB, is at most 1.05. A family
//   measured from 4 KiB is judged by its steps, as a seeded one is (below):
//   a fixed head (a definition at its caps, before the text that reuses it)
//   weighs on a fit at 4 KiB as growth would. A seeded
//   document repeats one block, whose first copies cost less than later
//   ones (nothing comes before them), so a fit reads a little over 1 while
//   its reads per byte level off; it is judged by what each doubling adds,
//   per byte, instead: level for linear growth however its start, twice the
//   last for a square. Each step's may be at most 1.1 times the one before;
// - what it keeps never passes KEEP_MOST, what it writes never passes the
//   output's bound;
// - its time, for work the meter might not count: within its own 32 KiB
//   time times TIME_GROWTH at the cap (16 would be linear; the rest is the
//   collector), and within TIME_MOST. A family stopped before the cap is
//   cut off there, so its time at the cap says nothing of the renderer: the
//   largest size it is drawn at is held to the smallest it's measured at,
//   by DRAWN_ROOM. Each time is the fastest of up to five. What this can't see: a family whose own work at its smallest
//   size is already large hides a slow path that costs less than that
//   ("an ordinary README": an uncounted square that failed "prose and
//   tables" at 526 ms against 232 passed it);
// - and the shapes held in OUTCOMES end as they do today: drawn where they
//   are drawn, and stopped for the same reason where they stop (a table of
//   64 columns stops at what it writes, not at what it keeps: rows kept
//   filled would move it). A stop where none was, or for another reason,
//   is a regression the counts alone would miss.
// MD_SWEEP=1 runs 100 seeds instead of 5.

import { describe, expect, test } from "vitest";
import { FAMILIES, seeded } from "./families.ts";
import { toHtml, TooLarge } from "./html.ts";
import { TRIES } from "./fastest.ts";
import { KEEP_MOST, meter, TOO_MUCH, type Meter } from "./meter.ts";

/** At most 2·NESTING + 8 a byte: a one-byte line under 66 open blocks is matched against each (md/blocks.ts). */
export const READ_PER_BYTE = 72;
const MOST = 2 * 1024 * 1024;
const TIME_MOST = 1_500;
/**
 * Over the sizes a family stopped early is drawn at, its time may be this many times linear. (Measured over 129
 * families, three times quiet and three loaded: 2.6 at most, and 3.3. A square the meter didn't count: 14.8 and 27.)
 */
const DRAWN_ROOM = 6;
/** Sixteen times the text: sixteen times the time, and room for the collector. */
const TIME_GROWTH = 48;

const SWEEP = !!process.env.MD_SWEEP;
const SIZES = [32, 64, 128, 256, 512].map((k) => k * 1024);
/** Where a family stopped early is measured as well (the identity lane's finding: else its growth goes unchecked). */
const SMALL = [4, 8, 16].map((k) => k * 1024);
/** Families drawn at fewer than three sizes even from 4 KiB: too few to fit, held by the bounds alone. */
const UNFITTED: Record<string, string> = {
  // (Each a line of one byte that writes 64 cells: past the output's bound at once.)
  "table wide 64 rows 1byte": "stopped at what it writes from 8 KiB (OUTCOMES holds it)",
  "table wide 64 center rows": "stopped at what it writes from 4 KiB",
};

type Row = { size: number; n: number; read: number };

/** What each doubling adds, a byte: each at most 1.1 times the one before (level for linear growth however its start, twice for a square). */
function steps(rows: Row[]) {
  for (let i = 2; i < rows.length; i++) {
    const before = (rows[i - 1].read - rows[i - 2].read) / (rows[i - 1].n - rows[i - 2].n);
    const now = (rows[i].read - rows[i - 1].read) / (rows[i].n - rows[i - 1].n);
    expect(now, `reads a byte added from ${rows[i - 1].n} to ${rows[i].n}, against ${before.toFixed(2)} the step before`).toBeLessThanOrEqual(before * 1.1);
  }
}
const SEEDS = SWEEP ? Array.from({ length: 100 }, (_, i) => i + 1) : [1, 2, 3, 4, 5];

type Outcome = "drawn" | "size" | "keep";

/** How these end today, by size (KiB): drawn, or stopped at what they'd write (size) or keep (keep). */
const OUTCOMES: Record<string, Record<number, Outcome>> = {
  "prose and tables": { 32: "drawn", 64: "drawn", 128: "drawn", 256: "drawn", 512: "drawn" },
  "an ordinary README": { 32: "drawn", 64: "drawn", 128: "drawn", 256: "drawn", 512: "drawn" },
  "table wide 64 rows 1byte": { 4: "drawn", 8: "size", 32: "size", 128: "size", 512: "size" },
  "a sparse table 1,300 columns wide": { 32: "drawn", 128: "drawn" },
  "definitions used once each, a paragraph each": { 32: "drawn", 128: "drawn", 512: "drawn" },
};

const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };

function run(md: string): { ms: number; m: Meter; stopped: boolean; outcome: Outcome } {
  const m = meter();
  const started = performance.now();
  let outcome: Outcome = "drawn";
  try {
    toHtml(md, options, MOST, m);
  } catch (error) {
    if (!(error instanceof TooLarge)) throw error;
    outcome = error === TOO_MUCH ? "keep" : "size";
  }
  return { ms: performance.now() - started, m, stopped: outcome !== "drawn", outcome };
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
    const measure = (size: number) => {
      const md = make(size);
      // (The best of two: a time is the least it took.)
      const a = run(md);
      const b = run(md);
      return { size, n: md.length, read: a.m.read, peak: Math.max(a.m.peak, b.m.peak), written: a.m.written, ms: Math.min(a.ms, b.ms), stopped: a.stopped };
    };
    const rows = SIZES.map(measure);
    const small = rows.filter((r) => !r.stopped).length < 3 ? SMALL.map(measure) : [];
    for (const row of [...small, ...rows]) {
      expect(row.read / row.n, `read a byte at ${row.n}`).toBeLessThanOrEqual(READ_PER_BYTE);
      expect(row.peak, `kept at ${row.n}`).toBeLessThanOrEqual(KEEP_MOST);
      expect(row.written, `written at ${row.n}`).toBeLessThanOrEqual(MOST);
    }
    const drawn = [...small, ...rows].filter((r) => !r.stopped);
    if (name in UNFITTED) expect(drawn.length, `${name} is in UNFITTED: ${UNFITTED[name]}`).toBeLessThan(3);
    else expect(drawn.length, "drawn at three sizes at least, from 4 KiB").toBeGreaterThanOrEqual(3);
    if (drawn.length >= 3) {
      // (Seeded: its steps from 64 KiB. Measured from 4 KiB: its steps from there.)
      if (name.startsWith("seeded ")) steps(drawn.filter((r) => r.size >= 64 * 1024));
      else if (small.length) steps(drawn);
      else expect(exponent(drawn.map((r) => [r.n, r.read])), "read's exponent").toBeLessThanOrEqual(1.05);
    }
    const first = rows[0];
    const last = rows[rows.length - 1];
    // (Stopped before the cap: where it's drawn, from the smallest size measured to the largest drawn.)
    const from = drawn[0];
    const to = last.stopped && drawn.length > 1 ? drawn[drawn.length - 1] : undefined;
    const room = (a: Row & { ms: number }, b: Row & { ms: number }) => Math.max(a.ms, 2) * DRAWN_ROOM * (b.size / a.size);
    // (Its time is the fastest of up to five, at each end: the machine's other work is none of the renderer's. The two
    // measured above are the first two tries. The limits stay as they are.)
    const within = () => last.ms <= Math.max(first.ms, 2) * TIME_GROWTH && last.ms < TIME_MOST && (!to || to.ms <= room(from, to));
    for (let tries = 2; tries < TRIES && !within(); tries++) for (const row of new Set([first, last, ...(to ? [from, to] : [])])) row.ms = Math.min(row.ms, run(make(row.size)).ms);
    expect(last.ms, "time at the cap, against its own at 32 KiB").toBeLessThanOrEqual(Math.max(first.ms, 2) * TIME_GROWTH);
    expect(last.ms, "time at the cap").toBeLessThan(TIME_MOST);
    if (to) expect(to.ms, `time at ${to.size / 1024} KiB, the largest drawn, against its own at ${from.size / 1024} KiB`).toBeLessThanOrEqual(room(from, to));
  });

  test.each(Object.entries(OUTCOMES))("%s ends as it does today", (name, outcomes) => {
    for (const [kb, outcome] of Object.entries(outcomes)) expect(run(FAMILIES[name](Number(kb) * 1024)).outcome, `${name} at ${kb} KiB`).toBe(outcome);
  });
});
