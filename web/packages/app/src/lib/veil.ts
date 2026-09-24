/**
 * Streaming fade veil — the pure math of `crates/ui/src/markdown/veil.rs`
 * (itself a port of mugen-markdown's FadePainter): per-appended-chunk opacity
 * over already-committed text, opacity only, never a positional offset. The
 * DOM side (wrapping text-node ranges in animating spans) lives in
 * `../components/transcript.tsx`.
 */

/** EMA seed for the inter-append gap (mugen `EMA_SEED_MS`). */
export const VEIL_EMA_SEED_MS = 160;
/** Duration clamp (mugen `MIN_FADE_MS` / `MAX_FADE_MS`). */
export const VEIL_MIN_FADE_MS = 120;
export const VEIL_MAX_FADE_MS = 400;
/** Dissolve exponent (mugen: `alpha = (1 - p) ** 1.6`). */
export const VEIL_CURVE_POW = 1.6;
/** Gap clamp feeding the EMA (mugen: `min(gap, 1000)`). */
const VEIL_GAP_CLAMP_MS = 1000;

/** Text alpha for a fade progress `p` (0..1): the veil dissolves as `(1 − p)^1.6`. */
export function veilOpacity(p: number): number {
  const clamped = Math.min(1, Math.max(0, p));
  return 1 - Math.pow(1 - clamped, VEIL_CURVE_POW);
}

/** Chunk fade duration for the current inter-append EMA. */
export function veilDurationMs(emaMs: number): number {
  return Math.min(VEIL_MAX_FADE_MS, Math.max(VEIL_MIN_FADE_MS, emaMs * 3));
}

/** Fast-stream boost: 3+ chunks fading concurrently speed up by 30% each. */
export function veilBoost(activeChunks: number): number {
  return 1 + 0.3 * Math.max(0, activeChunks - 2);
}

/** EMA update on a new append gap (`ema*0.7 + min(gap,1000)*0.3`). */
export function veilEmaNext(emaMs: number, gapMs: number): number {
  return emaMs * 0.7 + Math.min(gapMs, VEIL_GAP_CLAMP_MS) * 0.3;
}

/** Longest common prefix length in code points (never splits a surrogate pair). */
export function commonPrefix(a: string, b: string): number {
  const ac = [...a];
  const bc = [...b];
  let p = 0;
  while (p < ac.length && p < bc.length && ac[p] === bc[p]) {
    p++;
  }
  // Return as a UTF-16 index so callers can slice the original strings.
  return ac.slice(0, p).join("").length;
}

/**
 * Per-element chunk tracker: remembers the last rendered flat text and fades
 * every newly appended suffix exactly once. Times are ms from any epoch the
 * caller chooses consistently (e.g. `performance.now()`).
 */
export class VeilTracker {
  #prev = "";
  #emaMs = VEIL_EMA_SEED_MS;
  #lastAppend: number | null = null;
  /** Active chunks: [start, end, startedAt, durationMs]. */
  #chunks: Array<{ start: number; end: number; started: number; durationMs: number }> = [];

  /** Adopt `text` as the committed baseline without fading it (attach semantics). */
  seed(text: string): void {
    this.#prev = text;
  }

  /**
   * Advance to `text` at `now`, registering a fading chunk for the appended
   * suffix. Returns the chunk ranges (string indices into `text`) currently
   * fading, oldest first. Idempotent for unchanged text.
   */
  advance(text: string, now: number): Array<{ start: number; end: number; durationMs: number; started: number }> {
    if (text !== this.#prev) {
      const p = commonPrefix(this.#prev, text);
      // A non-append rewrite clamps in-flight chunks to the shared prefix;
      // only the changed tail re-veils.
      this.#chunks = this.#chunks.filter((chunk) => {
        chunk.end = Math.min(chunk.end, p);
        return chunk.start < chunk.end;
      });
      if (text.length > p) {
        if (this.#lastAppend !== null) {
          this.#emaMs = veilEmaNext(this.#emaMs, now - this.#lastAppend);
        }
        this.#lastAppend = now;
        this.#chunks.push({ start: p, end: text.length, started: now, durationMs: veilDurationMs(this.#emaMs) });
      }
      this.#prev = text;
    }
    const boost = veilBoost(this.#chunks.length);
    this.#chunks = this.#chunks.filter((chunk) => (now - chunk.started) * boost < chunk.durationMs);
    return this.#chunks.map((chunk) => ({ ...chunk }));
  }

  /** Whether any chunk is still fading as of the last `advance`. */
  isFading(): boolean {
    return this.#chunks.length > 0;
  }
}

// ---------------------------------------------------------------------------
// Per-line slicing (render.rs `slice_spans`, :2065-2070 / 2141-2146)
// ---------------------------------------------------------------------------

/** A fading chunk range over a block's flat text. */
export interface VeilRange {
  readonly start: number;
  readonly end: number;
}

/**
 * Split one rendered line's tokens at chunk boundaries (flat-text
 * coordinates): a chunk that starts or ends mid-token cuts it, so each
 * fading range wraps exactly its own text — the code-block half of the
 * desktop's veil, where code lines dissolve exactly like prose.
 */
export function sliceTokensForVeil<
  Token extends { readonly text: string },
  Chunk extends VeilRange,
>(
  tokens: readonly Token[],
  lineStart: number,
  lineEnd: number,
  chunks: readonly Chunk[],
): Array<{ readonly text: string; readonly token: Token; readonly chunk: Chunk | null }> {
  const out: Array<{ text: string; token: Token; chunk: Chunk | null }> = [];
  let at = lineStart;
  for (const token of tokens) {
    const tokenStart = at;
    const tokenEnd = at + token.text.length;
    at = tokenEnd;
    const covering = chunks.filter((chunk) => chunk.start < tokenEnd && chunk.end > tokenStart);
    if (covering.length === 0) {
      out.push({ text: token.text, token, chunk: null });
      continue;
    }
    const cuts = new Set<number>([tokenStart, tokenEnd]);
    for (const chunk of covering) {
      cuts.add(Math.max(tokenStart, chunk.start));
      cuts.add(Math.min(tokenEnd, chunk.end));
    }
    const sorted = [...cuts].sort((a, b) => a - b);
    for (let ix = 0; ix + 1 < sorted.length; ix++) {
      const start = sorted[ix]!;
      const end = sorted[ix + 1]!;
      if (end <= start) {
        continue;
      }
      const text = token.text.slice(start - tokenStart, end - tokenStart);
      const chunk = covering.find((candidate) => candidate.start <= start && end <= candidate.end) ?? null;
      if (text.length > 0) {
        out.push({ text, token, chunk });
      }
    }
  }
  return out;
}
