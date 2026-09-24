/**
 * Ticket 71 — the tool-fold scroll-ownership policy, as pure functions
 * (.scratch/web-parity/issues/71-tool-fold-scroll-and-thought-policy.md
 * §2.3: "at most one active compensator/controller").
 *
 * The transcript already has viewport owners: the tail-follow spring (the
 * pin), the own-turn runway (a held hold over its reservation), the
 * user-prompt fold compensation, the escape anchor's per-commit preserve,
 * and a pending viewport restore. The tool folds add exactly one more
 * compensator with a strict ownership contract, not a second controller:
 *
 * - An EXPLICIT group/detail click takes the viewport by releasing the
 *   follow/hold first (`begin_scroll_navigation` — the reservation
 *   survives as scrollable space), then anchors the clicked header at its
 *   captured screen position for the fold tween's duration. The anchor is
 *   CONSTANT: each frame's write only corrects drift (a browser clamp near
 *   the scroll end, a leftover write), never interpolates, so reduced
 *   motion needs no separate branch — the geometry snaps and the anchor
 *   correction lands in the same frame.
 * - An AUTOMATIC transition (thought completion, auto-open expiry) never
 *   releases anything: it arms only while no other owner is live, and it
 *   yields the moment one appears. A pinned tail keeps its existing
 *   tail-follow through the same compensated path; a manually escaped
 *   reading anchor is preserved, not dragged.
 *
 * The anchoring uses the virtualizer's measured row tops (`positions`) —
 * never guessed user-row heights; the scroller resolves the row index from
 * the row id per frame so splices above the row re-target correctly.
 */

import { TOOL_FOLD_MS } from "./tool-motion";

/** One armed compensation: keep one content-space anchor at a fixed screen y. */
export interface ToolFoldCompensation {
  /** The anchored row's id (re-resolved to an index per frame). */
  readonly rowId: string;
  /** The anchor's offset from the row's top, in content space. */
  readonly offsetInRow: number;
  /** The anchor's fixed screen-space y (viewport-top-relative). */
  readonly screenY: number;
  readonly startedAt: number;
  readonly endsAt: number;
}

/** The anchor capture: the content offset is re-read per frame; the screen
 *  position is the constant the viewport must preserve. */
export interface ToolFoldAnchorInput {
  readonly rowId: string;
  readonly offsetInRow: number;
  readonly screenY: number;
  readonly now: number;
}

/** Capture one anchor for the fold tween's duration (140ms, `TOOL_FOLD`). */
export function armToolFoldCompensation(anchor: ToolFoldAnchorInput): ToolFoldCompensation {
  return {
    rowId: anchor.rowId,
    offsetInRow: anchor.offsetInRow,
    screenY: anchor.screenY,
    startedAt: anchor.now,
    endsAt: anchor.now + TOOL_FOLD_MS,
  };
}

/**
 * The scrollTop that keeps the anchor at its screen position; null when it
 * is already there (or the row vanished — the caller disarms then).
 */
export function toolFoldCompensationWrite(
  compensation: ToolFoldCompensation,
  input: { readonly rowTop: number; readonly scrollTop: number },
): number | null {
  const target = input.rowTop + compensation.offsetInRow - compensation.screenY;
  return Math.abs(input.scrollTop - target) > 0.1 ? target : null;
}

/** The tween has played out; the compensator stands down. */
export function toolFoldCompensationDone(compensation: ToolFoldCompensation, now: number): boolean {
  return now >= compensation.endsAt;
}

/** §2.3's ownership gate inputs: every live viewport owner. */
export interface ToolFoldOwnershipState {
  readonly pinned: boolean;
  readonly ownTurnHeld: boolean;
  readonly userFoldCompensating: boolean;
  readonly escapeAnchor: boolean;
  readonly pendingViewportRestore: boolean;
}

/**
 * An AUTOMATIC fold transition (thought completion, outer-group auto-close)
 * arms the reading-anchor compensation only while no other owner holds the
 * viewport — never fighting the tail-follow spring, a held runway, the
 * user-prompt fold compensation, the escape anchor's per-commit preserve,
 * or a pending viewport restore. An explicitly CLICKED fold never consults
 * this gate: it takes ownership by releasing the follow/hold first.
 */
export function automaticToolFoldCompensationArms(state: ToolFoldOwnershipState): boolean {
  return (
    !state.pinned &&
    !state.ownTurnHeld &&
    !state.userFoldCompensating &&
    !state.escapeAnchor &&
    !state.pendingViewportRestore
  );
}
