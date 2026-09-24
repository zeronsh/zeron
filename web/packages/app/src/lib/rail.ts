/**
 * The MessageRail's pure logic — `crates/ui/src/rail.rs`, ported 1:1: the
 * width gate, tick extraction, bucketing, active detection, preview
 * truncation, and the duration-based scroll-glide timeline. Rendering is
 * `components/message-rail.tsx`; the DOM glide driver is
 * `components/stick-controller.ts` (`scrollToRow`).
 *
 * The rail is a fixed-footprint minimap of at most 12 ticks down the
 * transcript's left edge, one per user prompt (bucketed when there are more),
 * that highlights the prompt you are reading, shows a preview card on hover,
 * and glides the list to that prompt on click.
 */

import type { SessionMessageEntry } from "@zeron/proto";
import { motion } from "@zeron/theme";
import { userMessageRailText } from "./attachments";
import { singleLine } from "./transcript";

/** 48rem — the container width below which the rail (and wide gutters) collapse (rail.rs:21). */
export const RAIL_MIN_CONTAINER_WIDTH = 768;

/** Preview text caps (rail.rs:28-29). */
export const PREVIEW_PROMPT_CHARS = 160;
export const PREVIEW_REPLY_CHARS = 200;

/** One tick's hit-row height and the gap between ticks (rail.rs:119-120). */
export const TICK_SLOT = 10;
export const TICK_GAP = 3;
/** Vertical breathing room kept clear above/below the tick stack (rail.rs:122). */
export const RAIL_V_MARGIN = 24;

/** Hard cap on visible ticks — the always-compact outline (rail.rs:127). */
export const MAX_RAIL_TICKS = 12;

/** Viewport height assumed for the one pre-layout frame where it reads 0 (rail.rs:467). */
export const RAIL_FALLBACK_VIEWPORT_HEIGHT = 600;

/** `motion::SCROLL_GLIDE` — the 500ms ease-in-out glide to a clicked row. */
export const SCROLL_GLIDE_MS =
  motion.specs.find((spec) => spec.name === "scrollGlide")?.durationMs ?? 500;
/** The glide's `EASE_IN_OUT` curve (motion.rs:226), for `cubicBezierEval`. */
export const SCROLL_GLIDE_CURVE: readonly [number, number, number, number] =
  motion.curves.easeInOut ?? [0.42, 0, 0.58, 1];
/** The glide's frame cadence (rail.rs:268's 16ms timer). */
export const GLIDE_FRAME_MS = 16;

/** The rail's left edge and width (rail.rs:472-476). */
export const RAIL_LEFT = 16;
export const RAIL_WIDTH = 26;

/** `rail_visible` (rail.rs:23): the transcript container's width gate. */
export function railVisible(containerWidth: number): boolean {
  return containerWidth >= RAIL_MIN_CONTAINER_WIDTH;
}

/** One rail tick: a user prompt and the opening of the reply that followed. */
export interface RailTick {
  /** Message id — equals the user row's id in the transcript row model. */
  readonly messageId: string;
  readonly prompt: string;
  readonly reply: string | null;
}

function userText(entry: SessionMessageEntry): string {
  const raw = entry.parts
    .filter((part): part is Extract<SessionMessageEntry["parts"][number], { kind: "text" }> => part.kind === "text")
    .map((part) => part.text)
    .join("\n\n");
  // Attachment refs ride the message text — the rail shows the visible
  // prompt, or "Attached image" for image-only sends.
  return userMessageRailText(raw);
}

function firstReplyText(entries: readonly SessionMessageEntry[]): string | null {
  const assistant = entries.find((entry) => entry.role === "assistant");
  if (assistant === undefined) {
    return null;
  }
  for (const part of assistant.parts) {
    if (part.kind === "text" && part.text.trim().length > 0) {
      return part.text.trim();
    }
  }
  return null;
}

/**
 * Extract rail ticks (rail.rs:74-99): one per user entry in doc order, then
 * un-deduped user echoes — matching transcript row order. Each tick carries
 * the opening of the first assistant entry AFTER it, for the hover preview.
 */
export function railTicks(
  entries: readonly SessionMessageEntry[],
  echoes: readonly SessionMessageEntry[],
): RailTick[] {
  const ticks: RailTick[] = [];
  entries.forEach((entry, ix) => {
    if (entry.role !== "user") {
      return;
    }
    ticks.push({
      messageId: entry.id,
      prompt: userText(entry),
      reply: firstReplyText(entries.slice(ix + 1)),
    });
  });
  for (const echo of echoes) {
    if (echo.role === "user" && !ticks.some((tick) => tick.messageId === echo.id)) {
      ticks.push({ messageId: echo.id, prompt: userText(echo), reply: null });
    }
  }
  return ticks;
}

/**
 * The active tick for a scroll position (rail.rs:104-112): the last tick whose
 * transcript row is at or above the viewport-top row (the prompt whose section
 * you're reading). Before the first tick's row, the first tick is active.
 */
export function activeTick(tickRows: readonly number[], topRow: number): number | null {
  if (tickRows.length === 0) {
    return null;
  }
  let found: number | null = null;
  for (let ix = 0; ix < tickRows.length; ix++) {
    if (tickRows[ix]! <= topRow) {
      found = ix;
    }
  }
  return found ?? 0;
}

/**
 * How many tick slots fit in a rail of `height` px (rail.rs:130-133, always
 * ≥ 1).
 */
export function railCapacity(height: number): number {
  const usable = Math.max(height - 2 * RAIL_V_MARGIN, TICK_SLOT);
  return Math.max(1, Math.floor((usable + TICK_GAP) / (TICK_SLOT + TICK_GAP)));
}

/** Slots the rail actually uses (rail.rs:137-139): what fits, capped at 12. */
export function railSlots(height: number): number {
  return Math.min(railCapacity(height), MAX_RAIL_TICKS);
}

/**
 * `tick_buckets` (rail.rs:149-155): when prompts outnumber the slots that fit
 * the viewport, ticks become evenly-sized BUCKETS over the conversation (a
 * downsampled minimap) instead of overflowing. With `n <= capacity` every
 * bucket is a single tick — the identity, i.e. the old per-prompt rail.
 */
export function tickBuckets(n: number, capacity: number): Array<readonly [number, number]> {
  if (n === 0) {
    return [];
  }
  const cap = Math.min(Math.max(capacity, 1), n);
  const out: Array<readonly [number, number]> = [];
  for (let k = 0; k < cap; k++) {
    out.push([Math.floor((k * n) / cap), Math.floor(((k + 1) * n) / cap)]);
  }
  return out;
}

/** The bucket containing tick `ix` (rail.rs:158-160), for active/hover mapping. */
export function bucketOf(buckets: readonly (readonly [number, number])[], ix: number): number | null {
  for (let b = 0; b < buckets.length; b++) {
    const [start, end] = buckets[b]!;
    if (ix >= start && ix < end) {
      return b;
    }
  }
  return null;
}

/**
 * Char-cap a preview with an ellipsis (rail.rs:165-172). Whitespace runs
 * (including newlines — prompts and replies are free text) collapse to single
 * spaces first: the preview card's title is a one-line surface.
 */
export function truncatePreview(text: string, maxChars: number): string {
  const flat = singleLine(text);
  if ([...flat].length <= maxChars) {
    return flat;
  }
  const cut = [...flat].slice(0, Math.max(0, maxChars - 1)).join("");
  return `${cut.trimEnd()}…`;
}

/**
 * Duration-based scroll glide (rail.rs:191-219): each frame hands out a
 * fraction of whatever distance CURRENTLY remains, `(e − e_prev)/(1 −
 * e_prev)` for eased progress `e`. With a stable distance estimate this
 * telescopes to exactly `start + e(t)·total`; when the estimate changes
 * mid-flight (a row got measured), the SAME timeline simply continues over
 * the corrected remainder — no restart, no compensating jump.
 */
export class GlideTimeline {
  #easedPrev = 0;

  /**
   * Fraction of the CURRENT remaining distance to consume for eased progress
   * `eased` (monotone, 0..=1; 1.0 lands exactly).
   */
  step(eased: number): number {
    const clamped = Math.min(Math.max(eased, this.#easedPrev), 1);
    const denom = 1 - this.#easedPrev;
    const frac = denom <= 1e-6 ? 1 : (clamped - this.#easedPrev) / denom;
    this.#easedPrev = clamped;
    return Math.min(Math.max(frac, 0), 1);
  }
}
