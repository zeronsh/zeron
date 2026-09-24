import { describe, expect, it } from "vitest";
import {
  anchorHelperPlacement,
  escapeFinalFocusTarget,
  exitMotionMs,
  noFlipPositionerProps,
  shouldVetoDismissal,
  virtualAnchorAt,
  ANCHOR_GAP,
  ESCAPE_KEY_REASON,
  MENU_OUT_BASE_MS,
  NO_FLIP_COLLISION_AVOIDANCE,
  SNAP_MARGIN,
} from "../src/components/base/positioning";
import { overlayKeyboardTransition } from "../src/components/base/overlay";
import { overlayKeyboard } from "../src/state/keymap";

/**
 * The `components/base/*` wrapper contracts (Base UI adoption, blueprint
 * Phase 0): the no-flip/clamp-only positioning preset (popover.rs:420-638),
 * the anchor-helper table that replaces `lib/popover-anchor.ts` (§5), the
 * escape-vs-dismissal focus split (pickers.rs:871-890), the exit window's
 * speed scaling (`motion::MENU_OUT` × `speed_scale`), and the
 * `overlayKeyboard` wiring that keeps session-nav shortcuts quiet under
 * overlays (shell.rs:3681-3683). Pure parts only — the node environment
 * renders nothing.
 */

describe("noFlipPositionerProps", () => {
  it("encodes the clamp-only preset: shift both axes, no perpendicular fallback, 8px margin, fixed", () => {
    expect(noFlipPositionerProps({ side: "top", align: "end" })).toEqual({
      side: "top",
      align: "end",
      sideOffset: ANCHOR_GAP,
      alignOffset: undefined,
      collisionAvoidance: NO_FLIP_COLLISION_AVOIDANCE,
      collisionPadding: SNAP_MARGIN,
      positionMethod: "fixed",
    });
  });

  it("never flips: side 'shift' + fallbackAxisSide 'none' on every placement", () => {
    for (const side of ["top", "bottom", "left", "right"] as const) {
      for (const align of ["start", "center", "end"] as const) {
        const props = noFlipPositionerProps({ side, align });
        expect(props.collisionAvoidance).toEqual({
          side: "shift",
          align: "shift",
          fallbackAxisSide: "none",
        });
      }
    }
  });

  it("keeps the caller's offsets verbatim", () => {
    expect(noFlipPositionerProps({ side: "bottom", align: "start", sideOffset: 10, alignOffset: -4 })).toMatchObject({
      sideOffset: 10,
      alignOffset: -4,
    });
  });
});

describe("anchorHelperPlacement (the popover-anchor.ts replacement, §5 table)", () => {
  it("anchorBelow → bottom/start/6", () => {
    expect(anchorHelperPlacement("anchorBelow")).toEqual({ side: "bottom", align: "start" });
    expect(noFlipPositionerProps(anchorHelperPlacement("anchorBelow")).sideOffset).toBe(6);
  });

  it("anchorBelowGap → bottom/start with the caller's gap", () => {
    expect(anchorHelperPlacement("anchorBelowGap", 10)).toEqual({ side: "bottom", align: "start", sideOffset: 10 });
    expect(anchorHelperPlacement("anchorBelowGap")).toEqual({ side: "bottom", align: "start", sideOffset: 6 });
  });

  it("anchorBelowEnd → bottom/end/6", () => {
    expect(anchorHelperPlacement("anchorBelowEnd")).toEqual({ side: "bottom", align: "end" });
  });

  it("anchorAbove/anchorAboveAt → top/start/6", () => {
    expect(anchorHelperPlacement("anchorAbove")).toEqual({ side: "top", align: "start" });
    expect(anchorHelperPlacement("anchorAboveAt")).toEqual({ side: "top", align: "start" });
  });

  it("anchorAboveEnd → top/end/6", () => {
    expect(anchorHelperPlacement("anchorAboveEnd")).toEqual({ side: "top", align: "end" });
  });

  it("fullWidthMenuAbove → top/start (width rides CSS --anchor-width)", () => {
    expect(anchorHelperPlacement("fullWidthMenuAbove")).toEqual({ side: "top", align: "start" });
  });

  it("menuAt → bottom/start at the point with NO gap", () => {
    expect(anchorHelperPlacement("menuAt")).toEqual({ side: "bottom", align: "start", sideOffset: 0 });
  });

  it("virtualAnchorAt anchors at the point with a zero rect", () => {
    const anchor = virtualAnchorAt(120, 40);
    expect(anchor.getBoundingClientRect()).toEqual({
      x: 120,
      y: 40,
      width: 0,
      height: 0,
      top: 40,
      right: 120,
      bottom: 40,
      left: 120,
    });
    // The caret variant anchors above the point (ticket 14): side top.
    expect(noFlipPositionerProps({ ...anchorHelperPlacement("anchorAboveAt") }).side).toBe("top");
  });
});

describe("escapeFinalFocusTarget (pickers.rs:871-890)", () => {
  const composer = {} as HTMLElement;

  it("returns the target on the escape-key reason", () => {
    expect(escapeFinalFocusTarget(ESCAPE_KEY_REASON, composer)).toBe(composer);
    expect(escapeFinalFocusTarget(ESCAPE_KEY_REASON, () => composer)).toBeInstanceOf(Function);
  });

  it("keeps focus put (false) on every other dismissal reason", () => {
    for (const reason of ["outside-press", "trigger-press", "focus-out", "close-press", null]) {
      expect(escapeFinalFocusTarget(reason, composer)).toBe(false);
    }
  });

  it("keeps focus put when no escape target was supplied", () => {
    expect(escapeFinalFocusTarget(ESCAPE_KEY_REASON, undefined)).toBe(false);
  });
});

describe("exitMotionMs", () => {
  it("is MENU_OUT (100ms) at 1× speed", () => {
    expect(exitMotionMs()).toBe(MENU_OUT_BASE_MS);
    expect(exitMotionMs(1)).toBe(100);
  });

  it("scales with the motion-speed setting", () => {
    expect(exitMotionMs(2)).toBe(200);
    expect(exitMotionMs(0.5)).toBe(50);
  });
});

describe("shouldVetoDismissal (ticket 09 gap rows 29/87 — Tab never closes)", () => {
  it("vetoes focus-out closes: Base UI's non-modal default, not the desktop's", () => {
    expect(shouldVetoDismissal("focus-out")).toBe(true);
  });

  it("lets every real dismissal through", () => {
    for (const reason of ["outside-press", "escape-key", "trigger-press", "close-press", "none"]) {
      expect(shouldVetoDismissal(reason)).toBe(false);
    }
  });
});

describe("overlayKeyboardTransition (shell.rs:3681-3683)", () => {
  it("registers on close→open and unregisters on open→close", () => {
    expect(overlayKeyboardTransition("composer-pickers", false, true)).toEqual([["composer-pickers", true]]);
    expect(overlayKeyboardTransition("composer-pickers", true, false)).toEqual([["composer-pickers", false]]);
  });

  it("does nothing when the state did not change", () => {
    expect(overlayKeyboardTransition("add-space", false, false)).toEqual([]);
    expect(overlayKeyboardTransition("add-space", true, true)).toEqual([]);
  });

  it("drives the real registry: owns while any source is open, quiet again on close", () => {
    expect(overlayKeyboard.owns()).toBe(false);
    overlayKeyboard.set("test-surface", true);
    expect(overlayKeyboard.owns()).toBe(true);
    // A second open surface cannot un-register the first.
    overlayKeyboard.set("test-surface-2", true);
    expect(overlayKeyboard.owns()).toBe(true);
    overlayKeyboard.set("test-surface-2", false);
    expect(overlayKeyboard.owns()).toBe(true);
    overlayKeyboard.set("test-surface", false);
    expect(overlayKeyboard.owns()).toBe(false);
  });
});
