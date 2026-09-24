/**
 * The MessageRail — `crates/ui/src/rail.rs::render_rail` (:411-572), ported
 * 1:1. A fixed-footprint minimap of at most 12 ticks down the transcript's
 * left edge: the active tick brightens, hover grows the bar and shows a
 * preview card, click glides the list to that prompt (the DOM glide driver is
 * `StickController.scrollToRow`).
 *
 * Pure logic (ticks, buckets, active detection, previews) lives in
 * `../lib/rail.ts`; this component only maps it onto the DOM. The visibility
 * gate is the TRANSCRIPT CONTAINER's width (`railVisible`), never the
 * viewport's; the surface (transcript.tsx) additionally hard-disables the
 * rail for the subagent override instance.
 */

import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import {
  PREVIEW_PROMPT_CHARS,
  PREVIEW_REPLY_CHARS,
  RAIL_FALLBACK_VIEWPORT_HEIGHT,
  TICK_GAP,
  TICK_SLOT,
  activeTick,
  bucketOf,
  railSlots,
  railVisible,
  tickBuckets,
  truncatePreview,
  type RailTick,
} from "../lib/rail";
import type { TranscriptRow } from "../lib/transcript";

export interface MessageRailProps {
  /** One tick per user prompt, doc order, echoes un-deduped (`railTicks`). */
  readonly ticks: readonly RailTick[];
  /** The transcript's row model — user rows share the tick's message id. */
  readonly rows: readonly TranscriptRow[];
  /** The reading-line row (viewport top advanced past the titlebar chrome). */
  readonly topRow: number;
  /** The scroller's client height (pre-layout 0 assumes 600, rail.rs:467). */
  readonly viewportHeight: number;
  /** The transcript container's width — the rail's visibility gate. */
  readonly containerWidth: number;
  /** The transcript container's height — clamps the preview card. */
  readonly containerHeight: number;
  /** Click a tick: glide the list so `row` sits at the viewport top. */
  readonly onJumpToRow: (row: number) => void;
}

export function MessageRail(props: MessageRailProps): ReactNode {
  const [hover, setHover] = useState<number | null>(null);
  if (!railVisible(props.containerWidth)) {
    return null;
  }
  // Map each tick to its transcript row (user rows share the entry id).
  const pairs: Array<{ readonly tick: RailTick; readonly row: number }> = [];
  for (const tick of props.ticks) {
    const row = props.rows.findIndex((r) => r.id === tick.messageId);
    if (row >= 0) {
      pairs.push({ tick, row });
    }
  }
  // A minimap of one exchange is noise, not navigation (rail.rs:433).
  if (pairs.length < 2) {
    return null;
  }
  const tickRows = pairs.map((pair) => pair.row);
  const active = activeTick(tickRows, props.topRow);
  const capacity = railSlots(
    props.viewportHeight > 0 ? props.viewportHeight : RAIL_FALLBACK_VIEWPORT_HEIGHT,
  );
  const buckets = tickBuckets(pairs.length, capacity);
  const activeBucket = active === null ? null : bucketOf(buckets, active);
  // The tick stack is vertically centered (justify_center): its top offset
  // and per-tick centers follow from the stack's own arithmetic.
  const stackHeight = buckets.length * TICK_SLOT + (buckets.length - 1) * TICK_GAP;
  const stackTop = (props.containerHeight - stackHeight) / 2;

  return (
    <div className="message-rail" aria-hidden>
      {buckets.map((range, ix) => (
        <RailTickView
          key={ix}
          ix={ix}
          range={range}
          pair={pairs[active !== null && active >= range[0] && active < range[1] ? active : range[0]]!}
          active={activeBucket === ix}
          hovered={hover === ix}
          centerY={stackTop + ix * (TICK_SLOT + TICK_GAP) + TICK_SLOT / 2}
          containerHeight={props.containerHeight}
          onHover={setHover}
          onJumpToRow={props.onJumpToRow}
        />
      ))}
    </div>
  );
}

interface RailTickViewProps {
  readonly ix: number;
  readonly range: readonly [number, number];
  readonly pair: { readonly tick: RailTick; readonly row: number };
  readonly active: boolean;
  readonly hovered: boolean;
  /** The tick's vertical center inside the rail container. */
  readonly centerY: number;
  readonly containerHeight: number;
  readonly onHover: (ix: number | null) => void;
  readonly onJumpToRow: (row: number) => void;
}

/** One tick's hit row: the 10px slot, the 2px bar, hover + click (rail.rs:540-569). */
export function RailTickView(props: RailTickViewProps): ReactNode {
  const bucketLen = props.range[1] - props.range[0];
  return (
    <div
      className="rail-tick"
      data-ix={props.ix}
      data-active={props.active ? "1" : "0"}
      data-hovered={props.hovered ? "1" : "0"}
      onMouseEnter={() => props.onHover(props.ix)}
      onMouseLeave={() => props.onHover(null)}
      onClick={() => props.onJumpToRow(props.pair.row)}
    >
      <span className="rail-tick-bar" />
      {props.hovered && (
        <RailPreviewCard
          prompt={truncatePreview(props.pair.tick.prompt, PREVIEW_PROMPT_CHARS)}
          reply={
            props.pair.tick.reply === null
              ? null
              : truncatePreview(props.pair.tick.reply, PREVIEW_REPLY_CHARS)
          }
          bucketLen={bucketLen}
          centerY={props.centerY}
          containerHeight={props.containerHeight}
        />
      )}
    </div>
  );
}

interface RailPreviewCardProps {
  readonly prompt: string;
  readonly reply: string | null;
  readonly bucketLen: number;
  readonly centerY: number;
  readonly containerHeight: number;
}

/**
 * The hover preview card (rail.rs:505-539): `popover_card w(280) p(8) flex
 * flex_col gap(6)`, anchored at the tick's LeftCenter and snapped to the
 * window with an 8px margin — here an absolute card beside the rail column
 * (`left: RAIL_WIDTH`), vertically centered on the tick and clamped, and
 * transparent to the pointer: the desktop's card vanishes the moment the
 * pointer leaves the tick, so it is a peek, not a surface.
 */
export function RailPreviewCard(props: RailPreviewCardProps): ReactNode {
  const ref = useRef<HTMLDivElement | null>(null);
  const [clampedTop, setClampedTop] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el !== null) {
      const top = Math.min(
        Math.max(props.centerY - el.offsetHeight / 2, 8),
        Math.max(props.containerHeight - el.offsetHeight - 8, 8),
      );
      setClampedTop((current) => (current === top ? current : top));
    }
  });
  return (
    <div
      ref={ref}
      className="rail-preview"
      style={
        clampedTop === null
          ? { top: props.centerY, transform: "translateY(-50%)" }
          : { top: clampedTop, transform: "none" }
      }
    >
      <div className="rail-preview-prompt">{props.prompt}</div>
      {props.reply !== null && <div className="rail-preview-reply">{props.reply}</div>}
      {props.bucketLen > 1 && <div className="rail-preview-count">{`${props.bucketLen} prompts`}</div>}
    </div>
  );
}
