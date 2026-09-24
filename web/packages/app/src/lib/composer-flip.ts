/**
 * The composer's compact↔expanded flip, height morph, and scroll math —
 * ported whole from `crates/ui/src/composer.rs` (constants §2.0, pure
 * logic §3, render anchors composer.rs:7300-7850).
 *
 * The desktop's discipline, which this port keeps:
 *
 * - the flip decision input is a **layout-stable width pair** — the text's
 *   unwrapped widest-line width vs the compact-mode wrap capacity. Neither
 *   number may depend on which mode is currently rendered: the post-flip
 *   measured width "differs per mode and would feed back into the decision"
 *   (the ping-pong that used to crash the web port with React's nested-update
 *   limit);
 * - a newline always expands;
 * - a too-narrow composer (`capacity < MIN_COMPACT_INPUT_WIDTH`) always
 *   expands — there is no compact layout to fall back to;
 * - hysteresis: expanding and collapsing share no boundary
 *   (`COLLAPSE_HYSTERESIS` slack), so a text width right at the flip
 *   threshold cannot oscillate;
 * - while the layout is being interactively resized, an expanded composer
 *   stays expanded until the widths settle (`RESIZE_SETTLE_MS`);
 * - a committed mode change starts ONE 180ms ease-out height morph from the
 *   last rendered height; auto-grow retargets the morph mid-flight; reduced
 *   motion and route snaps never arm one (flip_morph_step).
 */

import { motion } from "@zeron/theme";
import { STRIP_PAD_TOP } from "./attachments";

// ---------------------------------------------------------------------------
// Geometry constants (composer.rs:46-115) — transcribed verbatim
// ---------------------------------------------------------------------------

/// `TEXTAREA_PAD_V` (composer.rs:48) — `pt-4`(16) + `pb-1`(4) on the
/// expanded textarea box.
export const TEXTAREA_PAD_V = 20;
/// `TEXTAREA_MIN` (composer.rs:53) — floor of the textarea BOX, applies even
/// when empty (what makes the always-expanded canvas 124px tall).
export const TEXTAREA_MIN = 76;
/// `TEXTAREA_MAX` (composer.rs:54) — cap of the textarea BOX.
export const TEXTAREA_MAX = 260;
/// `ACTIONS_ROW_HEIGHT` (composer.rs:58) — `pt-1`(4) + 32px chips + `pb-2.5`(10).
export const ACTIONS_ROW_HEIGHT = 46;
/// `PILL_BORDER_V` (composer.rs:60) — the 1px hairline, top + bottom.
export const PILL_BORDER_V = 2;
/// `COMPOSER_RADIUS` (composer.rs:62) — pill + queue-tray corner radius.
export const COMPOSER_RADIUS = 26;
/// `COMPOSER_MIN_HEIGHT` (composer.rs:65) — 76 + 46 + 2.
export const COMPOSER_MIN_HEIGHT = TEXTAREA_MIN + ACTIONS_ROW_HEIGHT + PILL_BORDER_V;
/// `COMPOSER_MAX_HEIGHT` (composer.rs:66) — 260 + 46 + 2.
export const COMPOSER_MAX_HEIGHT = TEXTAREA_MAX + ACTIONS_ROW_HEIGHT + PILL_BORDER_V;
/// `COMPACT_TOTAL_HEIGHT` (composer.rs:70) — `py-3`(24) + one 22.75px line + 2.
export const COMPACT_TOTAL_HEIGHT = 49;
/// `COMPOSER_MAX_WIDTH` (composer.rs:72) — `max-w-3xl` column width.
export const COMPOSER_MAX_WIDTH = 768;
/// `QUEUE_SIDE_INSET` (composer.rs:74) — the queue tray's per-side inset.
export const QUEUE_SIDE_INSET = 16;
/// `QUEUE_COMPOSER_OVERLAP` (composer.rs:77) — how much of the tray's bottom
/// the pill covers.
export const QUEUE_COMPOSER_OVERLAP = 18;
/// `SESSION_FOOTER_HEIGHT` (composer.rs:83) — the footer slot height.
export const SESSION_FOOTER_HEIGHT = 24;
/// `COMPOSER_WIDTH_EPSILON` (composer.rs:94) — subpixel noise threshold.
export const COMPOSER_WIDTH_EPSILON = 0.5;
/// `MIN_COMPACT_INPUT_WIDTH` (composer.rs:96) — below this the pill always expands.
export const MIN_COMPACT_INPUT_WIDTH = 200;
/// `INPUT_LINE_HEIGHT` (composer.rs:98) — 14 × 1.625 (`leading-relaxed`).
export const INPUT_LINE_HEIGHT = 22.75;
/// `INPUT_TEXT_SIZE` (composer.rs:99).
export const INPUT_TEXT_SIZE = 14;
/// `INPUT_FADE_BAND` (composer.rs:101) — the scroll fade ramp inside the input.
export const INPUT_FADE_BAND = 12;
/// `DRAG_SCROLL_FRAME_MS` (composer.rs:105) — drag-selection autoscroll cadence.
export const DRAG_SCROLL_FRAME_MS = 16;
/// `COLLAPSE_HYSTERESIS` (composer.rs:111) — expanded→compact slack.
export const COLLAPSE_HYSTERESIS = 32;
/// `RESIZE_SETTLE_MS` (composer.rs:115) — collapse deferral during resize.
export const RESIZE_SETTLE_MS = 150;
/// `CARET_BLINK_MS` (composer.rs:151) — caret blink half-period.
export const CARET_BLINK_MS = 500;
/// `ROUTE_SNAP_MS` (composer.rs:455) — flips within this of a nav SNAP.
export const ROUTE_SNAP_MS = 250;
/// `CLUSTER_Y_DELTA` (composer.rs:394) — Send/attach sits 29px above the
/// expanded pill's bottom versus 24.5px in compact; the morph glides this
/// optical adjustment instead of snapping.
export const CLUSTER_Y_DELTA = 4.5;
/// `CLUSTER_X_DELTA` (composer.rs:402) — Send's right-inset delta (pr-2 ↔ px-3).
export const CLUSTER_X_DELTA = 4;
/// `ACTION_UTILITY_GAP` (composer.rs:406) — pickers ↔ paperclip optical join.
export const ACTION_UTILITY_GAP = 2;
/// `ACTION_PRIMARY_GAP` (composer.rs:408) — `Theme::SPACE_SM` (8).
export const ACTION_PRIMARY_GAP = 8;
/// The auto-grow retarget epsilon (composer.rs:7556) — arming threshold for
/// the height morph.
export const HEIGHT_RETARGET_EPSILON = 0.5;

/**
 * `model_handoff` (composer.rs:412, e0c1e936): fade out at the old endpoint,
 * relocate while invisible, then fade in at the new endpoint. Only a
 * six-pixel nudge is visible; a long label never sweeps across the prompt.
 * Compact amount is reversible with the shared clock. Returns
 * `(side, opacity, drift)` — which horizontal slot (0 left, 1 right), the
 * chip's opacity, and the visible nudge in px (positive toward the right
 * group, negative back toward the left).
 */
export function modelHandoff(compact: number): [number, number, number] {
  const amount = Math.min(Math.max(compact, 0), 1);
  const side = amount < 0.5 ? 0 : 1;
  const opacity = Math.max(Math.abs(amount - 0.5) - 0.06, 0) / 0.44;
  const drift = (1 - opacity) * (side === 0 ? 6 : -6);
  return [side, opacity, drift];
}

/**
 * `motion::EASE_IN_OUT` (proto motion.rs:226) — the handoff's OWN curve:
 * it rides the flip morph's RAW timeline, not the collapse spec's eased
 * progress, so reversals continue from the current phase (composer.rs:7741).
 */
const EASE_IN_OUT: readonly [number, number, number, number] =
  motion.curves.easeInOut ?? [0.42, 0, 0.58, 1];

/** The inputs one render frame resolves the model handoff position from. */
export interface ModelHandoffInputs {
  /** The position captured when the flip morph last (re)started. */
  readonly from: number;
  /** The RENDERED mode's compact target: 0 expanded, 1 compact. */
  readonly compactTarget: number;
  /** The running flip morph, if any — its RAW timeline drives the lerp. */
  readonly morph: FlipMorph | null;
  /** A dock frame is installed AND active. */
  readonly dockActive: boolean;
  /** The session's own expanded state (the dock drives compact routes only). */
  readonly sessionExpanded: boolean;
  /** The shared dock clock's amount, 0..1. */
  readonly dockAmount: number;
  /** Commit time in ms on the caller's monotonic clock. */
  readonly nowMs: number;
}

/**
 * `model_handoff_position` (composer.rs:7726-7749, e0c1e936): the handoff
 * rides the SAME height/route clock as every other inner channel — the dock
 * amount directly while a compact route glides, else a lerp from the
 * captured phase toward the rendered mode's target through EASE_IN_OUT over
 * the flip morph's RAW timeline.
 */
export function modelHandoffPosition(inputs: ModelHandoffInputs): number {
  if (inputs.dockActive && !inputs.sessionExpanded) {
    return inputs.dockAmount;
  }
  if (inputs.morph === null) {
    return inputs.compactTarget;
  }
  return lerp(
    inputs.from,
    inputs.compactTarget,
    cubicBezierY(EASE_IN_OUT, flipMorphRaw(inputs.morph, inputs.nowMs)),
  );
}

/**
 * `model_travel` (composer.rs:412, e0c1e936): the horizontal distance
 * between the chip's two anchor slots — expanded, beside the attachment at
 * the row's left; compact, before Send at the row's right. `surfaceWidth` is
 * the pill's border-box width, `modelWidth` the chip's rendered width (the
 * desktop's `model_bounds` canvas), `clusterInset` the morphing right inset.
 */
export function modelTravel(surfaceWidth: number, modelWidth: number, clusterInset: number): number {
  return Math.max(
    surfaceWidth -
      PILL_BORDER_V -
      12 -
      28 -
      ACTION_UTILITY_GAP -
      modelWidth -
      ACTION_PRIMARY_GAP -
      28 -
      clusterInset,
    0,
  );
}

/** The model slot's per-frame transform: the offset plus the fade opacity. */
export interface ModelSlotGeometry {
  /** `model_offset` — `left` px from the chip's natural slot in this mode. */
  readonly left: number;
  /** `model_opacity` — 1 at rest, 0 through the invisible mid-flip relocate. */
  readonly opacity: number;
}

/**
 * `model_offset` (composer.rs:4765-4766, e0c1e936): the per-frame transform
 * applied to the chip's natural slot — the invisible mid-flip relocation
 * (the side jump lands while the opacity is ~0) plus the visible 6px drift,
 * with the matching opacity from `model_handoff`.
 */
export function modelSlotOffset(
  handoffPosition: number,
  compactTarget: number,
  travel: number,
): ModelSlotGeometry {
  const [side, opacity, drift] = modelHandoff(handoffPosition);
  return { left: (side - compactTarget) * travel + drift, opacity };
}

// Attachment strip metrics (composer.rs:288-296) moved to `lib/attachments.ts`
// with the strip's own rendering (ticket 17); re-exported so the composer and
// this module's historical importers keep one address.
export { attachmentStripHeight } from "./attachments";

/// `badges::BADGE_HEIGHT` (badges.rs:58) — the comments chip's row height.
export const BADGE_HEIGHT = 24;

// ---------------------------------------------------------------------------
// The flip decision (composer.rs:126-144)
// ---------------------------------------------------------------------------

/**
 * `composer_flip`: compact expands only when `textWidth > capacity`; expanded
 * collapses only when `textWidth < capacity - COLLAPSE_HYSTERESIS`. `resizing`
 * keeps an expanded composer expanded until the drag settles.
 */
export function composerFlip(
  expanded: boolean,
  textWidth: number,
  capacity: number,
  hasNewline: boolean,
  resizing: boolean,
): boolean {
  if (hasNewline) {
    return true;
  }
  if (capacity < MIN_COMPACT_INPUT_WIDTH) {
    return true;
  }
  if (expanded) {
    return resizing || textWidth >= capacity - COLLAPSE_HYSTERESIS;
  }
  return textWidth > capacity;
}

/**
 * `composer_width_changed` (composer.rs:146): a width change must exceed the
 * epsilon to count. `previous == null` (the first measurement) always counts.
 */
export function composerWidthChanged(previous: number | null, current: number): boolean {
  return previous === null || Math.abs(current - previous) > COMPOSER_WIDTH_EPSILON;
}

/**
 * The desktop's `width_changed_at` bookkeeping (composer.rs:7220-7236): a
 * same-mode input width move larger than the epsilon marks an interactive
 * resize in flight; the flag stays armed for `RESIZE_SETTLE_MS`.
 */
export function resizeSettling(
  changedAtMs: number | null,
  nowMs: number,
  previousWidth: number,
  width: number,
): { readonly resizing: boolean; readonly changedAtMs: number | null } {
  if (Math.abs(width - previousWidth) > COMPOSER_WIDTH_EPSILON && previousWidth > 0) {
    return { resizing: false, changedAtMs: nowMs };
  }
  return {
    resizing: changedAtMs !== null && nowMs - changedAtMs < RESIZE_SETTLE_MS,
    changedAtMs,
  };
}

/**
 * The one-flip-per-layout-pass guard (composer.rs:7213-7215,
 * `measured_since_flip`): only measurements taken after the last committed
 * flip may drive the next one — a flip invalidates the widths.
 */
export function measuredSinceFlip(epoch: number, flipEpoch: number, lastWidth: number): boolean {
  return epoch > flipEpoch && lastWidth > 0;
}

// ---------------------------------------------------------------------------
// Caret / auto-grow / scroll math (composer.rs:156-284)
// ---------------------------------------------------------------------------

/** `caret_visible` (composer.rs:156) — the blink phase since last activity. */
export function caretVisible(msSinceActivity: number): boolean {
  return Math.floor(msSinceActivity / CARET_BLINK_MS) % 2 === 0;
}

/** `input_content_height` (composer.rs:161) — zero lines still measures one. */
export function inputContentHeight(wrappedLines: number): number {
  return Math.max(wrappedLines, 1) * INPUT_LINE_HEIGHT;
}

/**
 * `composer_total_height` (composer.rs:169): the textarea BOX (content +
 * `pt-4 cb-1`) clamps to 76–260, then the 46px actions row and the hairline
 * ride on top. Range 124–308.
 */
export function composerTotalHeight(contentHeight: number): number {
  return (
    Math.min(Math.max(contentHeight + TEXTAREA_PAD_V, TEXTAREA_MIN), TEXTAREA_MAX) +
    ACTIONS_ROW_HEIGHT +
    PILL_BORDER_V
  );
}

/** `input_max_scroll` (composer.rs:175). */
export function inputMaxScroll(contentHeight: number, viewportHeight: number): number {
  return Math.max(contentHeight - viewportHeight, 0);
}

/**
 * `input_overflow_edges` (composer.rs:181): only **settled** overflow gets a
 * scroll fade — the animated viewport can be smaller for a few frames while
 * an otherwise fitting draft grows into it.
 */
export function inputOverflowEdges(
  contentHeight: number,
  settledHeight: number,
  visibleHeight: number,
  scrollTop: number,
): readonly [boolean, boolean] {
  if (inputMaxScroll(contentHeight, settledHeight) <= 1.0) {
    return [false, false];
  }
  const maxScroll = inputMaxScroll(contentHeight, visibleHeight);
  return [scrollTop > 1.0, scrollTop < maxScroll - 1.0];
}

/**
 * `input_reveal_height` (composer.rs:196): while resizing, stop the clip at a
 * complete row boundary — never slice glyphs with a moving clip.
 */
export function inputRevealHeight(
  visible: number,
  scroll: number,
  lineHeight: number,
  resizing: boolean,
): number {
  if (!resizing) {
    return visible;
  }
  const rowEnd = Math.floor((scroll + visible + 0.001) / lineHeight) * lineHeight;
  return Math.min(Math.max(rowEnd - scroll, 0), visible);
}

/**
 * `input_scroll_offset` (composer.rs:206): apply a wheel delta to a
 * top-origin offset. Positive deltas scroll toward the start.
 */
export function inputScrollOffset(
  current: number,
  deltaY: number,
  contentHeight: number,
  viewportHeight: number,
): number {
  return Math.min(
    Math.max(current - deltaY, 0),
    inputMaxScroll(contentHeight, viewportHeight),
  );
}

/**
 * `input_scroll_offset_for_cursor` (composer.rs:216): minimally adjust the
 * viewport so the caret row is fully visible, measured against the SETTLED
 * viewport (`settledHeight ?? viewportHeight`) so existing text stays fixed
 * relative to the input origin throughout a height animation.
 */
export function inputScrollOffsetForCursor(
  current: number,
  cursorTop: number,
  cursorHeight: number,
  contentHeight: number,
  viewportHeight: number,
  settledHeight: number | null,
): number {
  const viewport = settledHeight ?? viewportHeight;
  let next = current;
  if (cursorTop < next) {
    next = cursorTop;
  } else if (cursorTop + cursorHeight > next + viewport) {
    next = cursorTop + cursorHeight - viewport;
  }
  return Math.min(Math.max(next, 0), inputMaxScroll(contentHeight, viewport));
}

/** `PressIntent` (composer.rs:236-266) — what a mouse press asks for. */
export type PressIntent = "selectAll" | "extendSelection" | "placeCaret";

/**
 * `press_intent`: two clicks or more take the whole field, and every further
 * click keeps it. The whole field wins over the shift modifier.
 */
export function pressIntent(clickCount: number, shift: boolean): PressIntent {
  if (clickCount >= 2) {
    return "selectAll";
  }
  if (shift) {
    return "extendSelection";
  }
  return "placeCaret";
}

/**
 * `PressIntent::arms_drag`: a select-all must not arm the drag, or the next
 * mouse move would shrink it back to a drag from the press position.
 */
export function pressArmsDrag(intent: PressIntent): boolean {
  return intent !== "selectAll";
}

/**
 * `input_drag_scroll_delta` (composer.rs:270): per-frame drag-selection
 * scroll — distance-proportional, capped at one text row per frame.
 */
export function inputDragScrollDelta(
  pointerY: number,
  viewportTop: number,
  viewportBottom: number,
  lineHeight: number,
): number {
  let distance = 0;
  if (pointerY < viewportTop) {
    distance = pointerY - viewportTop;
  } else if (pointerY > viewportBottom) {
    distance = pointerY - viewportBottom;
  } else {
    return 0;
  }
  const magnitude = Math.min(Math.max(Math.abs(distance) * 0.2, 1.0), lineHeight);
  return Math.sign(distance) * magnitude;
}

/** `comment_strip_height` (composer.rs:306): 0 or 12 + 24. */
export function commentStripHeight(count: number): number {
  return count === 0 ? 0 : STRIP_PAD_TOP + BADGE_HEIGHT;
}

// ---------------------------------------------------------------------------
// The height morph (composer.rs:330-483)
// ---------------------------------------------------------------------------

/** The motion specs a `FlipMorph` can ride (the route spec is ticket 15's). */
export type FlipMorphSpec = "collapse" | "newThreadTransition";

/** `FlipMorph` (composer.rs:330) — one committed height animation. */
export interface FlipMorph {
  /** Rendered height when the flip committed — the animation's start point. */
  readonly from: number;
  /** Commit time in ms on the caller's monotonic clock. */
  readonly startMs: number;
  /// The motion spec (`motion::COLLAPSE` for typing flips; the 420ms
  /// `motion::NEW_THREAD_TRANSITION` is the first-send handoff, ticket 15).
  readonly spec: FlipMorphSpec;
}

function specDurationMs(spec: FlipMorphSpec): number {
  const name = spec === "collapse" ? "collapse" : "newThreadTransition";
  const found = motion.specs.find((entry) => entry.name === name);
  return found?.durationMs ?? (spec === "collapse" ? 180 : 420);
}

function specCurve(spec: FlipMorphSpec): readonly [number, number, number, number] {
  const name = spec === "collapse" ? "collapse" : "newThreadTransition";
  const found = motion.specs.find((entry) => entry.name === name);
  const curve = found?.curve ?? "easeOut";
  return motion.curves[curve] ?? [0, 0, 0.58, 1];
}

/**
 * `CubicBezier::eval` (crates/proto/src/motion.rs:125-205), ported: solve the
 * easing curve's y at a given x with Newton-Raphson and a bisection fallback,
 * output clamped hard.
 */
export function cubicBezierY(
  curve: readonly [number, number, number, number],
  x: number,
): number {
  const [x1, y1, x2, y2] = curve;
  if (x <= 0) {
    return 0;
  }
  if (x >= 1) {
    return 1;
  }
  const coefficients = (a: number, b: number): readonly [number, number, number] => {
    const c = 3 * a;
    const bb = 3 * (b - a) - c;
    const aa = 1 - c - bb;
    return [aa, bb, c];
  };
  const [ax, bx, cx] = coefficients(x1, x2);
  const [ay, by, cy] = coefficients(y1, y2);
  const sampleX = (t: number): number => ((ax * t + bx) * t + cx) * t;
  const sampleDX = (t: number): number => (3 * ax * t + 2 * bx) * t + cx;
  // Newton-Raphson.
  let t = x;
  for (let i = 0; i < 8; i++) {
    const dx = sampleDX(t);
    if (Math.abs(dx) < 1e-6) {
      break;
    }
    const delta = sampleX(t) - x;
    t -= delta / dx;
    if (t <= 0) {
      t = 0;
      break;
    }
    if (t >= 1) {
      t = 1;
      break;
    }
  }
  if (sampleX(t) !== x) {
    // Bisection fallback.
    let low = 0;
    let high = 1;
    t = x;
    while (high - low > 1e-6) {
      const current = sampleX(t);
      if (Math.abs(current - x) < 1e-6) {
        break;
      }
      if (x > current) {
        low = t;
      } else {
        high = t;
      }
      t = (low + high) / 2;
    }
  }
  const y = ((ay * t + by) * t + cy) * t;
  return Math.min(Math.max(y, 0), 1);
}

function lerp(from: number, to: number, progress: number): number {
  return from + (to - from) * progress;
}

/** Raw timeline position 0..1 over this morph's motion spec. */
export function flipMorphRaw(morph: FlipMorph, nowMs: number): number {
  return Math.min(Math.max((nowMs - morph.startMs) / specDurationMs(morph.spec), 0), 1);
}

/** Eased progress 0..1 — also drives the inner geometry handoff. */
export function flipMorphProgress(morph: FlipMorph, nowMs: number): number {
  const raw = flipMorphRaw(morph, nowMs);
  if (raw <= 0) {
    return 0;
  }
  if (raw >= 1) {
    return 1;
  }
  return cubicBezierY(specCurve(morph.spec), raw);
}

/** The morph's timeline has fully elapsed. */
export function flipMorphDone(morph: FlipMorph, nowMs: number): boolean {
  return flipMorphRaw(morph, nowMs) >= 1;
}

/**
 * Committed-height evaluation: eased lerp from the flip-time height to the
 * LIVE target (auto-grow may move the target mid-morph — the morph tracks it
 * instead of finishing on a stale height).
 */
export function flipMorphHeight(morph: FlipMorph, target: number, nowMs: number): number {
  return lerp(morph.from, target, flipMorphProgress(morph, nowMs));
}

/**
 * `flip_morph_step` (composer.rs:465): advance the flip morph across one
 * render pass. While the committed mode holds, the morph is kept (a finished
 * one clears) — same-mode renders can NEVER restart the animation. A
 * committed mode change starts one morph from the last rendered height,
 * which mid-flight is the CURRENT animated height, so a reverse flip hands
 * off seamlessly. Reduced motion (or a first paint with no measured height
 * yet) snaps, and `routeSnap` (a navigation within `ROUTE_SNAP_MS`) both
 * blocks arming AND kills anything in flight — navigation never animates
 * the pill.
 */
export function flipMorphStep(
  morph: FlipMorph | null,
  modeChanged: boolean,
  lastHeight: number,
  nowMs: number,
  reducedMotion: boolean,
  routeSnap: boolean,
): FlipMorph | null {
  if (routeSnap || reducedMotion) {
    return null;
  }
  if (!modeChanged) {
    return morph !== null && !flipMorphDone(morph, nowMs) ? morph : null;
  }
  if (lastHeight <= 0.0) {
    return null;
  }
  return { from: lastHeight, startMs: nowMs, spec: "collapse" };
}

// -- morph anchoring (composer.rs:381-445) ---------------------------------
//
// The pill sits at the BOTTOM of the column: growth moves its TOP edge, the
// bottom edge is stationary on screen. The controls pin to the stationary
// bottom edge and only the TEXT glides with the sweeping top edge.

/**
 * `morph_cluster_inset` (composer.rs:413): the right inset for the in-flight
 * morph — eases from the OLD mode's resting inset (compact 8 ↔ expanded 12)
 * so there is no sideways step at the commit.
 */
export function morphClusterInset(expanded: boolean, progress: number): number {
  return expanded
    ? lerp(8, 8 + CLUSTER_X_DELTA, progress)
    : lerp(8 + CLUSTER_X_DELTA, 8, progress);
}

/** `morph_text_pad` (composer.rs:425): the expanded text top padding, 12→16. */
export function morphTextPad(progress: number): number {
  return lerp(12, 16, progress);
}

/**
 * `collapse_text_glide` (composer.rs:434): the decaying relative offset that
 * walks the compact text down from its expanded resting place.
 */
export function collapseTextGlide(from: number, progress: number): number {
  return Math.max(from - 53, 0) * (1 - progress);
}

/**
 * `morph_cluster_dy` (composer.rs:443): the decaying `CLUSTER_Y_DELTA` —
 * controls share this bottom anchor; the model's horizontal fade is applied
 * independently so its endpoint matches Attachment and Send.
 */
export function morphClusterDy(progress: number): number {
  return CLUSTER_Y_DELTA * (1 - progress);
}

// ---------------------------------------------------------------------------
// Route-aware inner geometry (composer.rs:7589-7632, 7723, 7767, 7835,
// 7793-7800) — ticket 74
// ---------------------------------------------------------------------------

/** The inner channels one render frame resolves — all on the SAME clock. */
export interface RouteInputGeometry {
  /** `layout_morph_t` — the dock amount on a compact route, else the flip's. */
  readonly layoutProgress: number;
  /** `text_pt` — the expanded text top padding (12→16). */
  readonly textPad: number;
  /** `textarea_height` — the animated input box, floored on a compact route. */
  readonly boxHeight: number;
  /** The textarea viewport: the box less its padding (expanded) or one line. */
  readonly inputHeight: number;
  /** `settled_height` — the scroll/fade viewport measured on the COMMITTED base. */
  readonly settledViewport: number;
  /** `cluster_dy` — the control cluster's decaying 2.5px centering delta. */
  readonly clusterDy: number;
  /** The cluster's right inset, gliding 8↔12 with the layout clock. */
  readonly clusterInset: number;
  /** `text_glide` — the compact text's decaying offset (route or local flip). */
  readonly textGlide: number;
}

export interface RouteInputGeometryInputs {
  /** The RENDERED mode (`expanded || new_chat`), not the session's flip state. */
  readonly renderedExpanded: boolean;
  /** `session_expanded` — the composer's OWN expanded state. */
  readonly sessionExpanded: boolean;
  /** A dock frame is installed AND active. */
  readonly dockActive: boolean;
  /** The shared dock clock's amount, 0..1. */
  readonly dockAmount: number;
  /** `morph_t` — the local flip's eased progress (1 when no flip runs). */
  readonly flipProgress: number;
  /** The running flip morph's `from` height; null when no flip is animating. */
  readonly flipFrom: number | null;
  /** The animated pill height this frame (strips included). */
  readonly pillHeight: number;
  /** The settled base height this frame (no strips). */
  readonly baseHeight: number;
  /** The attachment + comment strip budget riding on the pill. */
  readonly stripHeight: number;
  /** `dock_height(0.0)` — the undocked (hero) height, the route glide's `from`. */
  readonly undockedHeight: number;
}

/**
 * The route layout clock (composer.rs:7592-7601): while the dock frame is
 * active and the session's own mode is compact, the shared dock amount
 * drives every inner channel — the expanded render reads `1 − amount`, the
 * compact render `amount` — so pill, padding, controls and text glide all
 * describe the SAME frame. Every other case (typing flips, an expanded
 * destination) keeps the local flip clock.
 *
 * The compact-route floors (composer.rs:7604-7632) keep at least one input
 * line plus its padding alive while the pill sweeps down to 49px: the box
 * floors at `22.75 + textPad + 4`, the settled viewport at one line. The
 * compact text glide (composer.rs:7793-7800) walks down from the UNDOCKED
 * height on an active route, else rides the local flip morph.
 */
export function routeInputGeometry(inputs: RouteInputGeometryInputs): RouteInputGeometry {
  const routeToSingleLine = inputs.dockActive && !inputs.sessionExpanded;
  const layoutProgress = routeToSingleLine
    ? inputs.renderedExpanded
      ? 1 - inputs.dockAmount
      : inputs.dockAmount
    : inputs.flipProgress;
  const textPad = morphTextPad(layoutProgress);
  const boxHeight = Math.max(
    inputs.pillHeight - inputs.stripHeight - PILL_BORDER_V - ACTIONS_ROW_HEIGHT,
    routeToSingleLine ? INPUT_LINE_HEIGHT + textPad + 4 : 0,
  );
  const inputHeight = inputs.renderedExpanded ? Math.max(boxHeight - textPad - 4, 0) : INPUT_LINE_HEIGHT;
  const settledViewport = inputs.renderedExpanded
    ? Math.max(
        inputs.baseHeight - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - TEXTAREA_PAD_V,
        routeToSingleLine ? INPUT_LINE_HEIGHT : 0,
      )
    : INPUT_LINE_HEIGHT;
  const clusterDy = morphClusterDy(layoutProgress);
  const clusterInset = morphClusterInset(inputs.renderedExpanded, layoutProgress);
  const textGlide = inputs.renderedExpanded
    ? 0
    : inputs.dockActive
      ? collapseTextGlide(inputs.undockedHeight, inputs.dockAmount)
      : inputs.flipFrom !== null
        ? collapseTextGlide(inputs.flipFrom, inputs.flipProgress)
        : 0;
  return {
    layoutProgress,
    textPad,
    boxHeight,
    inputHeight,
    settledViewport,
    clusterDy,
    clusterInset,
    textGlide,
  };
}

// ---------------------------------------------------------------------------
// Layout-pass bookkeeping (web seams for the desktop's gpui-harness tests)
// ---------------------------------------------------------------------------

/**
 * The layout-cache key (composer.rs `ComposerInput` layout reuse,
 * `layout_cache_reuses_resize_frames_and_invalidates_text_inputs`): a frame
 * can reuse the shaped text when nothing that RESHAPES it changed. Height,
 * scroll position and selection never enter the key; width, font size, the
 * text itself, and the IME marked range do.
 */
export interface LayoutFrameInputs {
  readonly width: number;
  readonly textSize: number;
  readonly text: string;
  /** An IME composition is active — decoration must repaint. */
  readonly markedRange: boolean;
}

export function layoutFrameKey(inputs: LayoutFrameInputs): string {
  return `${inputs.width}|${inputs.textSize}|${inputs.markedRange ? 1 : 0}|${inputs.text}`;
}

/**
 * `resolved_layout_does_not_keep_notifying_on_repaint`: a repaint that did
 * not reshape anything must not schedule another layout pass. The key is the
 * pass's identity — unchanged key, unchanged layout.
 */
export function layoutReshaped(previous: string | null, key: string): boolean {
  return previous !== key;
}

/**
 * The width-driven reflow scheduler for a stable outer width
 * (`stable_outer_width_only_schedules_reflow_on_real_changes` +
 * `set_available_width`): only an epsilon-exceeding move of the CLAMPED
 * column width schedules a reflow.
 */
export function availableWidthReflow(
  lastWidth: number | null,
  measured: number,
): { readonly width: number; readonly reflow: boolean } {
  const width = Math.min(Math.max(measured, 0), COMPOSER_MAX_WIDTH);
  return { width, reflow: composerWidthChanged(lastWidth, width) };
}
