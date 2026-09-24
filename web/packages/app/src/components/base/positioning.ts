/**
 * The Base UI positioning/exit/escape parity core — the numbers every
 * `components/base/*` wrapper shares (the adoption blueprint's §5/§6,
 * replacing `lib/popover-anchor.ts`'s one-shot rect math with Floating UI
 * props). Pure data and functions only, so the wrapper contracts stay
 * unit-testable in the node vitest environment.
 *
 * Desktop contracts encoded here, once:
 * - `snap_to_window_with_margin(px(8))` (popover.rs:420-638 family) — every
 *   anchored variant clamps the card 8px inside the window on the FIXED side;
 *   the side never flips (`menu_at` is clamp-only, popover.rs:621-638).
 *   Base UI equivalent: `collisionAvoidance { side: 'shift', align: 'shift',
 *   fallbackAxisSide: 'none' }` + `collisionPadding: 8`.
 * - `ANCHOR_GAP = 6` — the trigger→card gap the anchored-menu family uses.
 * - `MENU_OUT` (100ms) scaled by the motion-speed setting (`motion.rs`) —
 *   the wrapper feeds the result to `--rb-motion-menu-out` and Base UI's
 *   animation-aware unmount (`getAnimations()`) honors it.
 * - The escape-vs-dismissal focus split (`pickers.rs:871-890`): Escape
 *   returns focus to the opener, outside presses leave focus where it
 *   landed. Encoded as the `reason === 'escape-key'` decision on Base UI's
 *   `finalFocus`.
 */

/** The anchored-menu family's trigger→card gap (popover.rs:447 etc.). */
export const ANCHOR_GAP = 6;
/** `snap_to_window_with_margin` — the window edge margin (popover.rs:621). */
export const SNAP_MARGIN = 8;
/** `popover.rs:306`'s card inset — the 4px padding `.popover-card` carries. */
export const CARD_INSET = 4;
/** `motion::MENU_OUT.total()` — the exit animation's base span (100ms). */
export const MENU_OUT_BASE_MS = 100;
/** Base UI's dismissal reason for the Escape key path (`pickers.rs:866`). */
export const ESCAPE_KEY_REASON = "escape-key";

export type RbSide = "top" | "bottom" | "left" | "right";
export type RbAlign = "start" | "center" | "end";

/**
 * `side: 'shift'` keeps the chosen side and moves the popup inside the
 * boundary; `fallbackAxisSide: 'none'` forbids the perpendicular fallback —
 * clamp-only, never flip, exactly `menu_at`/`anchored_menu*` semantics. A
 * specific menu that later asks for a flip fallback overrides `side: 'flip'`
 * at its own call site.
 */
export const NO_FLIP_COLLISION_AVOIDANCE = {
  side: "shift",
  align: "shift",
  fallbackAxisSide: "none",
} as const;

/** A placement request against an anchor (the §5 anchor-helper table's shape). */
export interface AnchorPlacement {
  readonly side: RbSide;
  readonly align: RbAlign;
  /** Defaults to `ANCHOR_GAP` (6). */
  readonly sideOffset?: number;
  readonly alignOffset?: number;
}

/** The full no-flip Positioner prop set the wrappers spread onto Base UI. */
export interface NoFlipPositionerProps {
  readonly side: RbSide;
  readonly align: RbAlign;
  readonly sideOffset: number;
  readonly alignOffset: number | undefined;
  readonly collisionAvoidance: typeof NO_FLIP_COLLISION_AVOIDANCE;
  readonly collisionPadding: number;
  readonly positionMethod: "fixed";
}

/**
 * The parity preset, encoded once: fixed side, no flip, 8px window margin,
 * `position: fixed` (portal-to-body + fixed matches the current
 * `.popover-layer` placement). With body portals, Base UI's
 * `'clipping-ancestors'` collision boundary resolves to the viewport —
 * `liveViewport()`'s equivalent — so no override is needed.
 */
export function noFlipPositionerProps(placement: AnchorPlacement): NoFlipPositionerProps {
  return {
    side: placement.side,
    align: placement.align,
    sideOffset: placement.sideOffset ?? ANCHOR_GAP,
    alignOffset: placement.alignOffset,
    collisionAvoidance: NO_FLIP_COLLISION_AVOIDANCE,
    collisionPadding: SNAP_MARGIN,
    positionMethod: "fixed",
  };
}

/**
 * The `lib/popover-anchor.ts` helper table (the desktop's anchored-menu
 * family) as Base UI placements — the mapping doc for Phase 2's per-surface
 * migration. `anchorAboveAt`/`menuAt` additionally need the explicit point:
 * pass `virtualAnchorAt(x, y)` as the Positioner's `anchor`.
 */
export type AnchorHelperId =
  | "anchorBelow"
  | "anchorBelowGap"
  | "anchorBelowEnd"
  | "anchorAbove"
  | "anchorAboveAt"
  | "anchorAboveEnd"
  | "anchorRight"
  | "fullWidthMenuAbove"
  | "menuAt";

/** The helper→placement table (§5). `anchorBelowGap` takes the caller's gap. */
export function anchorHelperPlacement(helper: AnchorHelperId, gap = ANCHOR_GAP): AnchorPlacement {
  switch (helper) {
    case "anchorBelow":
      return { side: "bottom", align: "start" };
    case "anchorBelowGap":
      // The caller's gap (changes header passes ~10).
      return { side: "bottom", align: "start", sideOffset: gap };
    case "anchorBelowEnd":
      return { side: "bottom", align: "end" };
    case "anchorAbove":
      return { side: "top", align: "start" };
    case "anchorAboveAt":
      return { side: "top", align: "start" };
    case "anchorAboveEnd":
      return { side: "top", align: "end" };
    case "anchorRight":
      // `anchored_menu_right` (popover.rs:526-549): the card's top-left pins
      // at the trigger's top-right + 6 — the sidebar user-menu shape
      // (shell.rs:6430), opening rightward out of a clipped column.
      return { side: "right", align: "start" };
    case "fullWidthMenuAbove":
      // Width spans the anchor via CSS `width: var(--anchor-width)`.
      return { side: "top", align: "start" };
    case "menuAt":
      // Context menus: at the point, clamp-only, NO gap.
      return { side: "bottom", align: "start", sideOffset: 0 };
  }
}

/** The side a nested flyout opens on — `nested_menu(left: bool)` (popover.rs:584-612). */
export type NestedMenuSide = "left" | "right";

/**
 * `nested_menu` (popover.rs:584-612): the nested card opens beside its
 * trigger ROW, top-aligned, on the caller's chosen side (never flipped —
 * the caller picks the side that has room). The anchor point sits
 * `CARD_INSET + ANCHOR_GAP` (10px) beyond the row's edge so the
 * card-to-card visual gap reads 6px — the row is itself inset 4px inside
 * the parent card, exactly the desktop's `-(CARD_INSET + 6.0)` anchor.
 * Vertical placement stays within the window's eight-pixel gutter through
 * the no-flip preset's align shift.
 */
export function nestedMenuPlacement(side: NestedMenuSide): AnchorPlacement {
  return { side, align: "start", sideOffset: CARD_INSET + ANCHOR_GAP };
}

/** The rect shape Floating UI's `VirtualElement` reads. */
export interface VirtualAnchorRect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
  readonly left: number;
}

export interface VirtualAnchor {
  getBoundingClientRect(): VirtualAnchorRect;
}

/**
 * A zero-size virtual anchor at a viewport point — `anchorAboveAt`'s caret
 * point (ticket 14) and `menuAt`'s pointer position both become a
 * Positioner `anchor` prop instead of one-shot rect math.
 */
export function virtualAnchorAt(x: number, y: number): VirtualAnchor {
  return {
    getBoundingClientRect: () => ({ x, y, width: 0, height: 0, top: y, right: x, bottom: y, left: x }),
  };
}

/**
 * The exit window's duration under the motion-speed setting
 * (`motion::speed_scale`): the desktop's `MENU_OUT` scaled. The wrapper sets
 * `--rb-motion-menu-out` to this value so CSS stays the single motion
 * source and Base UI's `getAnimations()` wait follows whatever results.
 */
export function exitMotionMs(speedScale = 1): number {
  return MENU_OUT_BASE_MS * speedScale;
}

/**
 * `pickers.rs:871-890`: Escape returns focus to the opener (the composer);
 * every other close leaves focus where it landed. Base UI's `finalFocus`
 * returns `false` to keep focus put, or the escape target on the escape
 * path — this function IS that decision, so the wrappers share it.
 */
export function escapeFinalFocusTarget(
  lastReason: string | null,
  escapeTarget: HTMLElement | (() => HTMLElement | null) | undefined,
): HTMLElement | (() => HTMLElement | null) | false {
  if (lastReason === ESCAPE_KEY_REASON && escapeTarget !== undefined) {
    return escapeTarget;
  }
  return false;
}

/**
 * The dismissal reasons every wrapper VETOES: Base UI's non-modal popovers
 * close when focus moves out (`focus-out` — Tab away, a programmatic blur),
 * but the desktop's popover family has no Tab handling and no focus trap
 * (ticket 09 gap rows 29/87 — "Tab does not close it"), and the old
 * hand-rolled layer matched the desktop. The wrapper cancels these closes
 * before the store sees them; outside presses and Escape dismiss normally.
 */
export const VETOED_DISMISSAL_REASONS: ReadonlySet<string> = new Set(["focus-out"]);

/** Whether a dismissal reason is one the wrapper cancels outright. */
export function shouldVetoDismissal(reason: string): boolean {
  return VETOED_DISMISSAL_REASONS.has(reason);
}
