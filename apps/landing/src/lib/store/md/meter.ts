// What a rendering costs, counted as it happens: every character read,
// every character written, and what is kept while it runs (its blocks, a
// text's inlines, the tables a text's links need), in bytes as estimated
// below. What the counts show, and md/growth.test.ts holds for every family
// of shapes it makes (md/families.ts), at sizes up to the README's cap:
// - read: at most 72 a byte (2·NESTING + 8: a one-byte line under 66 open
//   blocks is matched against each), growing linearly; most shapes 3 to 22;
// - written: at most the caller's bound (2 MB for a page);
// - kept: at most KEEP_MOST at once; an honest README at its cap, about
//   6 MB. md/heap.test.ts renders the heaviest under a 48 MB heap.
//
// Two bounds stop a rendering (TooLarge; the caller shows the text):
// - what it writes, at the caller's `most` (md/writer.ts);
// - what it keeps at once, at KEEP_MOST.

/** A rendering that would write more than its bound, or keep more than KEEP_MOST: shown as its text. */
export class TooLarge extends Error {}

/**
 * The one TooLarge thrown, made once: an error made where a rendering stops
 * would capture a stack whose frames hold the rendering's state, kept by
 * the engine until another error replaces it (16 MB and more, measured).
 */
export const TOO_LARGE = new TooLarge("more to write than its bound");
/** The one TooLarge thrown when a rendering would keep more than KEEP_MOST. */
export const TOO_MUCH = new TooLarge("more to keep than KEEP_MOST");

export interface Meter {
  /** Characters read, each time one is looked at. */
  read: number;
  /** Bytes kept now (estimated): blocks for the whole document, a text's inlines while it is written. */
  kept: number;
  /** The most kept at once. */
  peak: number;
  /** Characters written. */
  written: number;
}

export const meter = (): Meter => ({ read: 0, kept: 0, peak: 0, written: 0 });

/**
 * What a rendering may keep at once, in estimated bytes: 16 MB, an eighth
 * of a Worker's memory. An honest README at its 512 KB cap keeps about
 * 6 MB; the estimates below are what the heap shows (or more), so a
 * rendering stopped here has used about this much.
 */
export const KEEP_MOST = 16 * 1024 * 1024;

/**
 * Estimated bytes of one parsed node (an open block, an inline, a
 * delimiter, a bracket), or of one finished block with its arrays: an
 * object, its fields, and its share of the arrays that hold it (measured
 * on V8: 100 to 130).
 */
export const NODE = 128;

/** Counts `bytes` as kept, and stops rather than pass KEEP_MOST (before what they are for is made). */
export function keep(m: Meter, bytes: number): void {
  if (m.kept + bytes > KEEP_MOST) throw TOO_MUCH;
  m.kept += bytes;
  if (m.kept > m.peak) m.peak = m.kept;
}

/** Counts `bytes` as let go. */
export function release(m: Meter, bytes: number): void {
  m.kept -= bytes;
}
