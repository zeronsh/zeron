import { describe, expect, it } from "vitest";
import {
  ACTION_PRIMARY_GAP,
  ACTION_UTILITY_GAP,
  ACTIONS_ROW_HEIGHT,
  attachmentStripHeight,
  caretVisible,
  CLUSTER_X_DELTA,
  CLUSTER_Y_DELTA,
  COLLAPSE_HYSTERESIS,
  COMPOSER_MAX_HEIGHT,
  COMPOSER_MAX_WIDTH,
  COMPOSER_MIN_HEIGHT,
  composerFlip,
  composerTotalHeight,
  COMPOSER_WIDTH_EPSILON,
  composerWidthChanged,
  availableWidthReflow,
  collapseTextGlide,
  commentStripHeight,
  COMPACT_TOTAL_HEIGHT,
  flipMorphDone,
  flipMorphHeight,
  flipMorphProgress,
  flipMorphStep,
  inputContentHeight,
  inputDragScrollDelta,
  INPUT_LINE_HEIGHT,
  inputMaxScroll,
  inputOverflowEdges,
  inputRevealHeight,
  inputScrollOffset,
  inputScrollOffsetForCursor,
  layoutFrameKey,
  layoutReshaped,
  measuredSinceFlip,
  MIN_COMPACT_INPUT_WIDTH,
  modelHandoff,
  modelHandoffPosition,
  modelSlotOffset,
  modelTravel,
  morphClusterDy,
  morphClusterInset,
  morphTextPad,
  PILL_BORDER_V,
  pressArmsDrag,
  pressIntent,
  resizeSettling,
  RESIZE_SETTLE_MS,
  routeInputGeometry,
  TEXTAREA_PAD_V,
  type FlipMorph,
} from "../src/lib/composer-flip";
import { dockHeight } from "../src/lib/composer-dock";

/**
 * The composer's compact↔expanded flip, height morph, and scroll math —
 * each describe named after the `composer.rs` unit test it mirrors. The web
 * port must make the same calls the desktop does — the ping-pong crash the
 * flip rules prevent is exactly the bug that once took the web composer
 * down, and the morph machine is what keeps the pill's bottom edge
 * stationary while the text glides.
 */

const COLLAPSE_FROM: FlipMorph = { from: 49, startMs: 0, spec: "collapse" };

describe("flip_decision", () => {
  it("compact stays compact while the text fits, expands on overflow", () => {
    // Fits in the pill → compact stays compact.
    expect(composerFlip(false, 150.0, 300.0, false, false)).toBe(false);
    // Overflow → expand.
    expect(composerFlip(false, 320.0, 300.0, false, false)).toBe(true);
    // Newline always expands (either mode, even mid-resize).
    expect(composerFlip(false, 10.0, 300.0, true, false)).toBe(true);
    expect(composerFlip(true, 10.0, 300.0, true, true)).toBe(true);
    // Narrow column (< MIN_COMPACT_INPUT_WIDTH) always expands.
    expect(composerFlip(false, 10.0, 199.0, false, false)).toBe(true);
    expect(composerFlip(false, 10.0, 200.0, false, false)).toBe(false);
  });
});

describe("flip_hysteresis_band_prevents_oscillation", () => {
  it("shares no boundary between the expand and collapse thresholds", () => {
    const cap = 300.0;
    // Text just over capacity expands…
    expect(composerFlip(false, cap + 1.0, cap, false, false)).toBe(true);
    // …and the SAME width, now expanded, does NOT collapse back — the
    // collapse threshold sits COLLAPSE_HYSTERESIS below the expand one.
    expect(composerFlip(true, cap + 1.0, cap, false, false)).toBe(true);
    // Anywhere inside the band the two modes are both stable (no width in
    // (cap - 32, cap] flips in either direction).
    const inBand = cap - COLLAPSE_HYSTERESIS + 1.0;
    expect(composerFlip(false, inBand, cap, false, false)).toBe(false);
    expect(composerFlip(true, inBand, cap, false, false)).toBe(true);
    // Comfortably under the band → collapses.
    expect(composerFlip(true, cap - COLLAPSE_HYSTERESIS - 1.0, cap, false, false)).toBe(false);
  });
});

describe("resize_expands_live_but_defers_collapse", () => {
  it("expands immediately under resize, collapses only once settled", () => {
    // A compact composer expands immediately as its text or controls stop
    // fitting, even while the divider is moving.
    expect(composerFlip(false, 500.0, 300.0, false, true)).toBe(true);
    expect(composerFlip(false, 10.0, 150.0, false, true)).toBe(true);
    // An expanded composer waits for the drag to settle before collapsing,
    // avoiding mode chatter while the user reverses direction.
    expect(composerFlip(true, 0.0, 300.0, false, true)).toBe(true);
    // Once settled, the same wide layout may collapse.
    expect(composerFlip(false, 500.0, 300.0, false, false)).toBe(true);
    expect(composerFlip(true, 0.0, 300.0, false, false)).toBe(false);
    // The narrow column stays expanded either way.
    expect(composerFlip(false, 10.0, 150.0, false, false)).toBe(true);
  });
});

describe("stable_outer_width_only_schedules_reflow_on_real_changes", () => {
  it("composerWidthChanged ignores sub-epsilon moves", () => {
    expect(composerWidthChanged(null, 400.0)).toBe(true);
    expect(composerWidthChanged(400.0, 400.0)).toBe(false);
    expect(composerWidthChanged(400.0, 400.5)).toBe(false);
    expect(composerWidthChanged(400.0, 400.51)).toBe(true);
  });

  it("set_available_width clamps to 768 and reflows only on real moves", () => {
    expect(availableWidthReflow(null, 400).width).toBe(400);
    expect(availableWidthReflow(null, 400).reflow).toBe(true);
    expect(availableWidthReflow(400, 400).reflow).toBe(false);
    expect(availableWidthReflow(400, 900).reflow).toBe(true);
    // The clamp keeps an over-wide measurement stable.
    expect(availableWidthReflow(COMPOSER_MAX_WIDTH, 900).reflow).toBe(false);
    expect(availableWidthReflow(COMPOSER_MAX_WIDTH, 900).width).toBe(COMPOSER_MAX_WIDTH);
  });
});

describe("resizeSettling", () => {
  it("arms the settle window on an epsilon-exceeding width change", () => {
    const now = 1_000;
    // A sub-epsilon move is noise, not a resize.
    const steady = resizeSettling(null, now, 300, 300 + COMPOSER_WIDTH_EPSILON);
    expect(steady.resizing).toBe(false);
    expect(steady.changedAtMs).toBe(null);
    // A real move arms the window (not yet settling — the drag is live).
    const armed = resizeSettling(null, now, 300, 260);
    expect(armed.resizing).toBe(false);
    expect(armed.changedAtMs).toBe(now);
    // After the move, the window holds `resizing` for RESIZE_SETTLE_MS.
    expect(resizeSettling(now, now + 10, 260, 260).resizing).toBe(true);
    expect(resizeSettling(now, now + RESIZE_SETTLE_MS - 1, 260, 260).resizing).toBe(true);
    expect(resizeSettling(now, now + RESIZE_SETTLE_MS, 260, 260).resizing).toBe(false);
    // A never-moved width never settles-resizes.
    expect(resizeSettling(null, now, 260, 260).resizing).toBe(false);
    // The width jump of a committed flip (last-seen reset to 0) is not an
    // interactive resize (composer.rs:7282).
    expect(resizeSettling(null, now, 0, 400).changedAtMs).toBe(null);
  });
});

describe("resolved_layout_does_not_keep_notifying_on_repaint", () => {
  it("a repaint that reshaped nothing schedules no further layout", () => {
    // The first resolved layout is published once…
    const key = layoutFrameKey({ width: 400, textSize: 14, text: "A long draft.", markedRange: false });
    expect(layoutReshaped(null, key)).toBe(true);
    // …and thirty unchanged draws schedule nothing more.
    for (let draw = 0; draw < 30; draw += 1) {
      expect(layoutReshaped(key, key)).toBe(false);
    }
    // The epoch guard agrees: a consumed flip measurement cannot re-fire.
    expect(measuredSinceFlip(7, 7, 400)).toBe(false);
    expect(measuredSinceFlip(8, 7, 400)).toBe(true);
    expect(measuredSinceFlip(8, 7, 0)).toBe(false);
  });
});

describe("layout_cache_reuses_resize_frames_and_invalidates_text_inputs", () => {
  it("height, scroll and selection reuse the frame; text/width/font/IME do not", () => {
    const base = { width: 400, textSize: 14, text: "A wrapped draft with enough text to measure.", markedRange: false };
    const key = layoutFrameKey(base);
    // 120 resize frames: viewport height, scroll position and selection
    // never enter the key.
    for (let frame = 0; frame < 120; frame += 1) {
      expect(layoutReshaped(key, layoutFrameKey(base))).toBe(false);
    }
    expect(layoutReshaped(key, layoutFrameKey({ ...base, text: "Edited draft" }))).toBe(true);
    expect(layoutReshaped(key, layoutFrameKey({ ...base, width: 200 }))).toBe(true);
    expect(layoutReshaped(key, layoutFrameKey({ ...base, textSize: 18 }))).toBe(true);
    // IME marking must repaint decoration.
    expect(layoutReshaped(key, layoutFrameKey({ ...base, markedRange: true }))).toBe(true);
  });
});

describe("caret_blink_phase", () => {
  it("is solid through the first half-period, then alternates", () => {
    expect(caretVisible(0)).toBe(true);
    expect(caretVisible(499)).toBe(true);
    expect(caretVisible(500)).toBe(false);
    expect(caretVisible(999)).toBe(false);
    expect(caretVisible(1000)).toBe(true);
  });
});

describe("auto_grow_math", () => {
  it("the pill is the clamped textarea box plus 46 plus 2 (124–308)", () => {
    expect(COMPOSER_MIN_HEIGHT).toBe(124);
    expect(COMPOSER_MAX_HEIGHT).toBe(308);
    // One line sits at the floor: the textarea BOX clamps UP to 76 exactly
    // like `Math.max(scrollHeight, 76)` — this is what makes the
    // always-expanded canvas 124px tall.
    expect(composerTotalHeight(inputContentHeight(1))).toBe(COMPOSER_MIN_HEIGHT);
    // Growth is linear once the textarea box exceeds its 76px floor.
    const h4 = composerTotalHeight(inputContentHeight(4));
    expect(h4).toBe(4 * INPUT_LINE_HEIGHT + TEXTAREA_PAD_V + ACTIONS_ROW_HEIGHT + PILL_BORDER_V);
    // Caps at a 260px textarea box.
    expect(composerTotalHeight(inputContentHeight(100))).toBe(COMPOSER_MAX_HEIGHT);
    // Zero lines still measures one.
    expect(inputContentHeight(0)).toBe(INPUT_LINE_HEIGHT);
  });

  it("attachmentStripHeight wraps thumbs per row and commentStripHeight is 0 or 36", () => {
    expect(attachmentStripHeight(0, 700)).toBe(0);
    expect(attachmentStripHeight(1, 700)).toBe(68);
    // 5 thumbs at a 700px hint: 3 per row (676 usable → floor((676+8)/64) = 10…
    // wait — per_row = floor((usable + 8) / 64) with usable = 700 − 32 = 668
    // → floor(676/64) = 10 per row, so 5 thumbs fit on one row.
    expect(attachmentStripHeight(5, 700)).toBe(68);
    // A narrow hint forces the wrap: usable 56 → 1 per row.
    expect(attachmentStripHeight(2, 56 + 32)).toBe(12 + 2 * 56 + 8);
    expect(commentStripHeight(0)).toBe(0);
    expect(commentStripHeight(2)).toBe(36);
    // The compact total and the row beneath it.
    expect(COMPACT_TOTAL_HEIGHT).toBe(49);
  });
});

describe("input_wheel_scroll_uses_gpui_direction_and_clamps", () => {
  it("positive deltas move toward the start; neither edge overscrolls", () => {
    expect(inputScrollOffset(40.0, 20.0, 200.0, 100.0)).toBe(20.0);
    expect(inputScrollOffset(40.0, -30.0, 200.0, 100.0)).toBe(70.0);
    expect(inputScrollOffset(10.0, 50.0, 200.0, 100.0)).toBe(0.0);
    expect(inputScrollOffset(90.0, -50.0, 200.0, 100.0)).toBe(100.0);
    expect(inputScrollOffset(20.0, -50.0, 80.0, 100.0)).toBe(0.0);
  });
});

describe("input_scroll_reveals_only_when_caret_leaves_viewport", () => {
  it("a visible caret preserves the viewport; edges reveal minimally", () => {
    expect(inputScrollOffsetForCursor(40.0, 60.0, 20.0, 300.0, 100.0, null)).toBe(40.0);
    expect(inputScrollOffsetForCursor(80.0, 30.0, 20.0, 300.0, 100.0, null)).toBe(30.0);
    expect(inputScrollOffsetForCursor(20.0, 130.0, 20.0, 300.0, 100.0, null)).toBe(50.0);
    expect(inputScrollOffsetForCursor(0.0, 290.0, 20.0, 300.0, 100.0, null)).toBe(200.0);
  });
});

describe("input_drag_autoscroll_is_edge_proportional_and_capped", () => {
  it("is zero inside, proportional past the edge, capped at one row", () => {
    const top = 100.0;
    const bottom = 300.0;
    const line = INPUT_LINE_HEIGHT;
    expect(inputDragScrollDelta(200.0, top, bottom, line)).toBe(0.0);
    expect(inputDragScrollDelta(90.0, top, bottom, line)).toBe(-2.0);
    expect(inputDragScrollDelta(315.0, top, bottom, line)).toBe(3.0);
    expect(inputDragScrollDelta(-100.0, top, bottom, line)).toBe(-line);
    expect(inputDragScrollDelta(500.0, top, bottom, line)).toBe(line);
  });
});

describe("resize_reveals_only_complete_rows", () => {
  it("the clip ends between rows while resizing, full otherwise", () => {
    for (const visible of [0.0, 5.0, 22.0, 22.75, 30.0, 45.5, 70.0, 150.0]) {
      const height = inputRevealHeight(visible, 0.0, INPUT_LINE_HEIGHT, true);
      expect(height).toBeLessThanOrEqual(visible);
      expect(height % INPUT_LINE_HEIGHT).toBe(0);
    }
    // The row grid moves with scrolling; the clip still ends between rows.
    expect(inputRevealHeight(39.0, 7.0, 20.0, true)).toBe(33.0);
    // Normal overflow scrolling keeps its full viewport and existing fades.
    expect(inputRevealHeight(39.0, 7.0, 20.0, false)).toBe(39.0);
    expect(inputRevealHeight(100.0, 0.0, 20.0, true)).toBe(100.0);
  });
});

describe("resize_keeps_text_anchored_to_the_input_origin", () => {
  it("caret-follow never scrolls a fitting draft through the top clip", () => {
    for (const visible of [0.0, 22.75, 60.0, 110.0, 159.25]) {
      expect(inputScrollOffsetForCursor(0.0, 136.5, 22.75, 159.25, visible, 159.25)).toBe(0.0);
    }
    // A genuinely overflowing draft keeps the same caret-follow offset
    // through every frame of the reveal, measured against the SETTLED
    // viewport rather than the animating one.
    for (const visible of [30.0, 100.0, 180.0, 240.0]) {
      expect(inputScrollOffsetForCursor(160.0, 377.25, 22.75, 400.0, visible, 240.0)).toBe(160.0);
    }
    // Deleting back to a fitting draft resets scroll immediately, even
    // while the old, larger viewport is still shrinking.
    expect(inputScrollOffsetForCursor(160.0, 77.25, 22.75, 100.0, 240.0, 100.0)).toBe(0.0);
  });
});

describe("scroll_fade_ignores_temporary_resize_overflow", () => {
  it("only settled overflow gets a fade", () => {
    for (const visibleHeight of [0.0, 20.0, 60.0, 100.0, 160.0]) {
      const scroll = inputMaxScroll(160.0, visibleHeight);
      expect(inputOverflowEdges(160.0, 160.0, visibleHeight, scroll)).toEqual([false, false]);
    }
    // Deleting a capped draft disables fading immediately, even while its
    // scroll position and outer height are still settling.
    expect(inputOverflowEdges(100.0, 100.0, 240.0, 80.0)).toEqual([false, false]);
  });
});

describe("scroll_fade_tracks_real_overflow_edges", () => {
  it("each edge fades exactly when its side overflows", () => {
    const cases: readonly [number, boolean, boolean][] = [
      [0.0, false, true],
      [80.0, true, true],
      [160.0, true, false],
    ];
    for (const [scroll, top, bottom] of cases) {
      expect(inputOverflowEdges(400.0, 240.0, 240.0, scroll)).toEqual([top, bottom]);
    }
  });
});

describe("a_press_of_two_or_more_clicks_takes_the_whole_field_and_leaves_the_drag_disarmed", () => {
  it("two-plus clicks select all; only caret and extend presses arm the drag", () => {
    expect(pressIntent(1, false)).toBe("placeCaret");
    expect(pressIntent(1, true)).toBe("extendSelection");
    expect(pressIntent(2, false)).toBe("selectAll");
    // A triple click keeps the whole field.
    expect(pressIntent(3, false)).toBe("selectAll");
    // The whole field wins over the shift modifier.
    expect(pressIntent(2, true)).toBe("selectAll");
    // Only a caret press arms the drag — a select-all that armed it would
    // collapse to a drag selection on the next mouse move.
    expect(pressArmsDrag(pressIntent(1, false))).toBe(true);
    expect(pressArmsDrag(pressIntent(1, true))).toBe(true);
    expect(pressArmsDrag(pressIntent(2, false))).toBe(false);
  });
});

/// One frame short of the full morph timeline (never rounds up to done).
const ALMOST = 179.0;

describe("flip_morph_starts_once_per_committed_flip", () => {
  it("same-mode renders never restart the morph; a finished one clears", () => {
    // No committed flip → no morph.
    expect(flipMorphStep(null, false, 49.0, 0.0, false, false)).toBeNull();
    // A committed flip starts one, from the last rendered height…
    const m = flipMorphStep(null, true, 49.0, 100.0, false, false)!;
    expect(m.from).toBe(49.0);
    expect(m.startMs).toBe(100.0);
    // …and same-mode renders keep it UNCHANGED (no restart at the boundary,
    // whatever the heights are doing).
    expect(flipMorphStep(m, false, 80.0, 150.0, false, false)).toEqual(m);
    // A finished morph clears on the next same-mode render.
    expect(flipMorphStep(m, false, 124.0, 100.0 + ALMOST, false, false)).toEqual(m);
    expect(flipMorphStep(m, false, 124.0, 300.0, false, false)).toBeNull();
  });
});

describe("content_resize_retargets_from_visible_height_and_settles", () => {
  it("a mid-growth delete reverses from what is on screen, no snap", () => {
    const start = composerTotalHeight(inputContentHeight(3));
    const target = composerTotalHeight(inputContentHeight(6));
    const grow = flipMorphStep(null, true, start, 0.0, false, false)!;
    const visible = flipMorphHeight(grow, target, 60.0);
    expect(visible).toBeGreaterThan(start);
    expect(visible).toBeLessThan(target);
    // A delete during growth reverses from what is on screen.
    const shrink = flipMorphStep(grow, true, visible, 60.0, false, false)!;
    expect(flipMorphHeight(shrink, start, 60.0)).toBe(visible);
    expect(flipMorphHeight(shrink, start, 120.0)).toBeLessThan(visible);
    expect(flipMorphHeight(shrink, start, 240.0)).toBe(start);
    expect(flipMorphStep(shrink, false, start, 240.0, false, false)).toBeNull();
    // Toggling reduced motion also cancels an already running resize.
    expect(flipMorphStep(grow, false, visible, 60.0, true, false)).toBeNull();
  });
});

describe("flip_morph_height_ramps_monotonically_to_target", () => {
  it("starts at the committed height, never regresses, lands exactly", () => {
    const m = COLLAPSE_FROM;
    // Starts exactly at the committed height…
    let prev = flipMorphHeight(m, 124.0, 0.0);
    expect(prev).toBe(49.0);
    // …ramps without ever moving backwards…
    for (let step = 1; step <= 18; step += 1) {
      const h = flipMorphHeight(m, 124.0, step * 10.0);
      expect(h).toBeGreaterThanOrEqual(prev);
      prev = h;
    }
    // …and lands exactly on the target when done (and stays there).
    expect(flipMorphHeight(m, 124.0, 180.0)).toBe(124.0);
    expect(flipMorphDone(m, 180.0)).toBe(true);
    expect(flipMorphHeight(m, 124.0, 500.0)).toBe(124.0);
    // Collapse runs the same ramp downward.
    expect(flipMorphHeight(m, 124.0, 90.0)).toBeGreaterThan(49.0);
    const down: FlipMorph = { from: 124.0, startMs: 0.0, spec: "collapse" };
    expect(flipMorphHeight(down, 49.0, 90.0)).toBeLessThan(124.0);
    expect(flipMorphHeight(down, 49.0, 90.0)).toBeGreaterThan(49.0);
  });
});

describe("flip_morph_reverse_hands_off_from_current_height", () => {
  it("a reverse flip mid-flight commits a new morph FROM the animated height", () => {
    const m = COLLAPSE_FROM;
    const mid = flipMorphHeight(m, 124.0, 90.0);
    expect(mid).toBeGreaterThan(49.0);
    expect(mid).toBeLessThan(124.0);
    const rev = flipMorphStep(m, true, mid, 90.0, false, false)!;
    expect(rev.from).toBe(mid);
    expect(flipMorphHeight(rev, 49.0, 90.0)).toBe(mid);
  });
});

describe("flip_morph_snaps_for_reduced_motion_and_first_paint", () => {
  it("reduced motion and a flip before anything rendered never arm", () => {
    expect(flipMorphStep(null, true, 49.0, 0.0, true, false)).toBeNull();
    expect(flipMorphStep(null, true, 0.0, 0.0, false, false)).toBeNull();
  });
});

describe("route_change_never_arms_the_morph", () => {
  it("a flip inside the route-snap window snaps, and kills anything in flight", () => {
    expect(flipMorphStep(null, true, 49.0, 0.0, false, true)).toBeNull();
    const m = COLLAPSE_FROM;
    expect(flipMorphStep(m, false, 80.0, 50.0, false, true)).toBeNull();
    expect(flipMorphStep(m, true, 80.0, 50.0, false, true)).toBeNull();
    // …while outside the window the same flip animates as usual.
    const armed = flipMorphStep(null, true, 49.0, 300.0, false, false)!;
    expect(armed.from).toBe(49.0);
  });
});

describe("morph_anchoring_holds_controls_and_glides_text", () => {
  it("steady state rests; the commit instant starts from the old geometry", () => {
    expect(morphClusterDy(1.0)).toBe(0.0);
    expect(morphTextPad(1.0)).toBe(16.0);
    expect(collapseTextGlide(124.0, 1.0)).toBe(0.0);
    expect(morphTextPad(0.0)).toBe(12.0);
    expect(morphClusterDy(0.0)).toBe(CLUSTER_Y_DELTA);
    // Collapse glide: starts where the expanded text sat (17px below the
    // committed pill top → `from − 53` above the compact resting spot)…
    expect(collapseTextGlide(124.0, 0.0)).toBe(71.0);
    // …decays monotonically to zero…
    let prev = collapseTextGlide(124.0, 0.0);
    for (let step = 1; step <= 10; step += 1) {
      const g = collapseTextGlide(124.0, step / 10.0);
      expect(g).toBeLessThanOrEqual(prev);
      prev = g;
    }
    // …and can't go negative on shallow mid-flight reversals.
    expect(collapseTextGlide(50.0, 0.0)).toBe(0.0);
  });
});

describe("cluster_inset_glides_between_the_source_endpoints", () => {
  it("the morph starts from the OLD mode's resting inset, monotonically", () => {
    expect(ACTION_UTILITY_GAP).toBe(2);
    expect(ACTION_PRIMARY_GAP).toBe(8);
    expect(ACTION_UTILITY_GAP < ACTION_PRIMARY_GAP).toBe(true);
    expect(morphClusterInset(true, 0.0)).toBe(8.0);
    expect(morphClusterInset(true, 1.0)).toBe(12.0);
    expect(morphClusterInset(false, 0.0)).toBe(12.0);
    expect(morphClusterInset(false, 1.0)).toBe(8.0);
    // Monotone, bounded by the 4px source delta.
    let prev = morphClusterInset(true, 0.0);
    for (let step = 1; step <= 10; step += 1) {
      const v = morphClusterInset(true, step / 10.0);
      expect(v).toBeGreaterThanOrEqual(prev);
      expect(v).toBeLessThanOrEqual(8.0 + CLUSTER_X_DELTA);
      prev = v;
    }
  });
});

// e0c1e936 (composer.rs, ported): the model picker fades between its two
// horizontal anchors instead of sweeping across the prompt.
describe("model_handoff_hides_relocation_and_keeps_visible_motion_local", () => {
  it("rests fully opaque at both endpoints; relocation is invisible mid-flip", () => {
    expect(modelHandoff(0.0)).toEqual([0.0, 1.0, 0.0]);
    expect(modelHandoff(1.0)).toEqual([1.0, 1.0, -0.0]);
    for (const amount of [0.44, 0.49, 0.5, 0.51, 0.56]) {
      expect(modelHandoff(amount)[1]).toBeLessThan(0.0001);
    }
    for (let step = 0; step <= 100; step += 1) {
      const [side, opacity, drift] = modelHandoff(step / 100.0);
      expect(opacity).toBeGreaterThanOrEqual(0.0);
      expect(opacity).toBeLessThanOrEqual(1.0);
      expect(Math.abs(drift)).toBeLessThanOrEqual(6.0);
      expect(side === 0.0 || side === 1.0).toBe(true);
    }
  });
});

/**
 * e0c1e936's wired half (ticket 03): the handoff POSITION rides the shared
 * height/route clock — the dock amount on a compact route, else an
 * EASE_IN_OUT lerp over the flip morph's RAW timeline — and the per-frame
 * offset lands the invisible relocation against `model_travel` (the
 * measured distance between the chip's two anchor slots). The endpoints
 * must match the slot geometry the CSS lays out: expanded = 0 left of the
 * natural left slot, compact = 0 left of the natural right slot.
 */
describe("model_handoff_rides_the_shared_clock_and_lands_on_the_measured_slots", () => {
  const EXPANDED = 0;
  const COMPACT = 1;

  it("rests at the rendered mode's target with no offset and full opacity", () => {
    expect(modelHandoffPosition({ from: 1, compactTarget: EXPANDED, morph: null, dockActive: false, sessionExpanded: false, dockAmount: 0, nowMs: 0 })).toBe(EXPANDED);
    expect(modelHandoffPosition({ from: 0, compactTarget: COMPACT, morph: null, dockActive: false, sessionExpanded: true, dockAmount: 0, nowMs: 0 })).toBe(COMPACT);
    const restingExpanded = modelSlotOffset(EXPANDED, EXPANDED, 420);
    expect(restingExpanded.left).toBe(0);
    expect(restingExpanded.opacity).toBe(1);
    const restingCompact = modelSlotOffset(COMPACT, COMPACT, 420);
    expect(restingCompact.left).toBe(0);
    expect(restingCompact.opacity).toBe(1);
  });

  it("the dock amount drives the position on a compact route, ignoring the morph", () => {
    for (const amount of [0, 0.25, 0.5, 0.75, 1]) {
      const position = modelHandoffPosition({
        from: 0,
        compactTarget: COMPACT,
        morph: { from: 124, startMs: 0, spec: "collapse" },
        dockActive: true,
        sessionExpanded: false,
        dockAmount: amount,
        nowMs: 90,
      });
      expect(position).toBe(amount);
    }
    // An expanded session never hands the clock to the dock.
    expect(
      modelHandoffPosition({
        from: 0,
        compactTarget: EXPANDED,
        morph: null,
        dockActive: true,
        sessionExpanded: true,
        dockAmount: 1,
        nowMs: 0,
      }),
    ).toBe(EXPANDED);
  });

  it("lerps from the captured phase through the morph's RAW timeline, reaching the target at its end", () => {
    const morph: FlipMorph = { from: 124, startMs: 0, spec: "collapse" };
    let previous = 1;
    for (const nowMs of [0, 45, 90, 135, 180, 400]) {
      const position = modelHandoffPosition({
        from: 1,
        compactTarget: EXPANDED,
        morph,
        dockActive: false,
        sessionExpanded: false,
        dockAmount: 0,
        nowMs,
      });
      // Monotone toward the target over time (EASE_IN_OUT never reverses).
      expect(position).toBeLessThanOrEqual(previous);
      previous = position;
    }
    expect(
      modelHandoffPosition({
        from: 1,
        compactTarget: EXPANDED,
        morph,
        dockActive: false,
        sessionExpanded: false,
        dockAmount: 0,
        nowMs: 180,
      }),
    ).toBe(EXPANDED);
    // Monotone in the other direction too (a collapse).
    let rising = 0;
    for (const nowMs of [0, 45, 90, 135, 180]) {
      const position = modelHandoffPosition({
        from: 0,
        compactTarget: COMPACT,
        morph,
        dockActive: false,
        sessionExpanded: true,
        dockAmount: 0,
        nowMs,
      });
      expect(position).toBeGreaterThanOrEqual(rising);
      rising = position;
    }
  });

  it("model_travel is the measured distance between the two anchor slots, never negative", () => {
    // A 768px pill, a 160px chip, the 8px compact inset: 768 − 2 − 12 − 28
    // − 2 − 160 − 8 − 28 − 8 = 520.
    expect(modelTravel(768, 160, 8)).toBe(520);
    expect(modelTravel(768, 160, 12)).toBe(516);
    // A chip wider than the pill clamps at zero — no negative lefts.
    expect(modelTravel(120, 400, 8)).toBe(0);
  });

  it("the offset fades out at the OLD endpoint, relocates invisibly, fades in at the NEW one", () => {
    const travel = 320;
    // Expanding (rendered expanded, target 0): at the start the chip still
    // sits at its compact slot — offset = travel — fully opaque.
    const start = modelSlotOffset(1, EXPANDED, travel);
    expect(start.left).toBeCloseTo(travel);
    expect(start.opacity).toBe(1);
    // The invisible window [0.44, 0.56] spans the side flip — the
    // relocation itself is never on screen.
    for (const amount of [0.44, 0.5, 0.56]) {
      expect(modelSlotOffset(amount, EXPANDED, travel).opacity).toBeLessThan(0.0001);
      expect(modelSlotOffset(amount, COMPACT, travel).opacity).toBeLessThan(0.0001);
    }
    // Just past the crossing: relocated to the LEFT slot, fading back in,
    // within the 6px drift.
    const past = modelSlotOffset(0.4, EXPANDED, travel);
    expect(past.opacity).toBeGreaterThan(0);
    expect(past.left).toBeLessThanOrEqual(6.0);
    // Collapsing (rendered compact, target 1): the mirrored geometry —
    // the chip starts at the expanded slot, `−travel` from its own.
    const collapseStart = modelSlotOffset(0, COMPACT, travel);
    expect(collapseStart.left).toBeCloseTo(-travel);
    expect(collapseStart.opacity).toBe(1);
    const collapsePast = modelSlotOffset(0.6, COMPACT, travel);
    expect(collapsePast.opacity).toBeGreaterThan(0);
    expect(collapsePast.left).toBeGreaterThanOrEqual(-6.0);
    // Every offset stays within the travel band plus the drift.
    for (let step = 0; step <= 100; step += 1) {
      const expanding = modelSlotOffset(step / 100, EXPANDED, travel);
      expect(expanding.left).toBeGreaterThanOrEqual(-6.0);
      expect(expanding.left).toBeLessThanOrEqual(travel + 6.0);
      const collapsing = modelSlotOffset(step / 100, COMPACT, travel);
      expect(collapsing.left).toBeGreaterThanOrEqual(-travel - 6.0);
      expect(collapsing.left).toBeLessThanOrEqual(6.0);
    }
  });
});

describe("flip_morph_tracks_live_target_and_drives_fade", () => {
  it("auto-grow moves the target mid-morph; progress is the inner handoff", () => {
    const m = COLLAPSE_FROM;
    expect(flipMorphHeight(m, 159.0, 90.0)).toBeGreaterThan(flipMorphHeight(m, 124.0, 90.0));
    expect(flipMorphProgress(m, 0.0)).toBe(0.0);
    expect(flipMorphProgress(m, 180.0)).toBe(1.0);
    const mid = flipMorphProgress(m, 90.0);
    expect(mid).toBeGreaterThan(0.0);
    expect(mid).toBeLessThan(1.0);
  });
});

/**
 * Ticket 74 — the pure half of `dock_morph_restores_skinny_height_with_a_
 * continuous_editor_origin` (composer.rs:8045): the route layout clock, the
 * one-line input floor and the route text glide at the desktop test's own
 * samples (0, .2, .6, .98, 1) in both directions. jsdom has no layout
 * engine: editor origin, textarea identity and the painted footprint are
 * browser evidence, tracked on the ticket.
 *
 * The scenario: a one-line "Hi" draft, no strips — the hero side measures
 * `composerTotalHeight(22.75)` = 124, the session side COMPACT_TOTAL_HEIGHT
 * = 49, the pill lerps between them. Docking renders COMPACT from the
 * navigation commit (`expanded || new_chat` flips with the route); undocking
 * to the canvas renders EXPANDED the whole way.
 */
describe("dock_morph_restores_skinny_height_with_a_continuous_editor_origin (route geometry)", () => {
  const HERO = COMPOSER_MIN_HEIGHT; // composerTotalHeight(INPUT_LINE_HEIGHT)
  const SAMPLES = [0.0, 0.2, 0.6, 0.98, 1.0];

  function routeGeometry(
    amount: number,
    renderedExpanded: boolean,
    overrides?: Partial<Parameters<typeof routeInputGeometry>[0]>,
  ) {
    const contentHeight = INPUT_LINE_HEIGHT;
    return routeInputGeometry({
      renderedExpanded,
      sessionExpanded: false,
      dockActive: true,
      dockAmount: amount,
      flipProgress: 1,
      flipFrom: null,
      pillHeight: dockHeight(amount, contentHeight, false),
      baseHeight: dockHeight(amount, contentHeight, false),
      stripHeight: 0,
      undockedHeight: dockHeight(0, contentHeight, false),
      ...overrides,
    });
  }

  it("the pill lerps 124 → 49 across the route (the desktop's outer clock)", () => {
    for (const amount of SAMPLES) {
      expect(dockHeight(amount, INPUT_LINE_HEIGHT, false)).toBeCloseTo(HERO + (COMPACT_TOTAL_HEIGHT - HERO) * amount, 10);
    }
    expect(HERO).toBe(124);
    expect(COMPACT_TOTAL_HEIGHT).toBe(49);
  });

  it("DOCKING renders compact on the dock clock: layoutProgress IS the amount", () => {
    for (const amount of [0.0, 0.2, 0.6, 0.98]) {
      const g = routeGeometry(amount, false);
      const pill = dockHeight(amount, INPUT_LINE_HEIGHT, false);
      expect(g.layoutProgress).toBe(amount);
      expect(g.textPad).toBe(morphTextPad(amount));
      expect(g.boxHeight).toBe(Math.max(pill - PILL_BORDER_V - ACTIONS_ROW_HEIGHT, INPUT_LINE_HEIGHT + g.textPad + 4));
      // Compact render: the input viewport is always exactly one line.
      expect(g.inputHeight).toBe(INPUT_LINE_HEIGHT);
      expect(g.settledViewport).toBe(INPUT_LINE_HEIGHT);
      // The route text glide walks down from the undocked hero height…
      expect(g.textGlide).toBe(collapseTextGlide(HERO, amount));
      // …and the cluster rides the same clock.
      expect(g.clusterInset).toBe(morphClusterInset(false, amount));
      expect(g.clusterDy).toBe(morphClusterDy(amount));
    }
  });

  it("the one-line box floor engages as the pill sweeps under it (compact route)", () => {
    // Early samples: the raw budget still exceeds the floor.
    for (const amount of [0.0, 0.2]) {
      const g = routeGeometry(amount, false);
      const raw = dockHeight(amount, INPUT_LINE_HEIGHT, false) - PILL_BORDER_V - ACTIONS_ROW_HEIGHT;
      expect(raw).toBeGreaterThan(INPUT_LINE_HEIGHT + g.textPad + 4);
      expect(g.boxHeight).toBe(raw);
    }
    // Late samples: the floor holds at least one line plus its padding.
    for (const amount of [0.6, 0.98]) {
      const g = routeGeometry(amount, false);
      const raw = dockHeight(amount, INPUT_LINE_HEIGHT, false) - PILL_BORDER_V - ACTIONS_ROW_HEIGHT;
      expect(raw).toBeLessThan(INPUT_LINE_HEIGHT + g.textPad + 4);
      expect(g.boxHeight).toBe(INPUT_LINE_HEIGHT + g.textPad + 4);
    }
  });

  it("UNDOCKING renders expanded on the reversed clock: layoutProgress is 1 − amount", () => {
    for (const amount of [1.0, 0.98, 0.6, 0.2]) {
      const g = routeGeometry(amount, true);
      expect(g.layoutProgress).toBe(1 - amount);
      expect(g.textPad).toBe(morphTextPad(1 - amount));
      expect(g.clusterInset).toBe(morphClusterInset(true, 1 - amount));
      expect(g.clusterDy).toBe(morphClusterDy(1 - amount));
      // Expanded render has no text glide…
      expect(g.textGlide).toBe(0);
      // …and the input never loses its one visible line, even at the
      // compact extreme (amount 1: the pill is 49, the raw box budget 1).
      expect(g.inputHeight).toBeGreaterThanOrEqual(INPUT_LINE_HEIGHT);
      expect(g.settledViewport).toBeGreaterThanOrEqual(INPUT_LINE_HEIGHT);
    }
    const atCompact = routeGeometry(1.0, true);
    expect(atCompact.inputHeight).toBe(INPUT_LINE_HEIGHT);
    expect(atCompact.settledViewport).toBe(INPUT_LINE_HEIGHT);
    expect(atCompact.boxHeight).toBe(INPUT_LINE_HEIGHT + morphTextPad(0) + 4);
    // Back at the canvas the full box is restored.
    const atCanvas = routeGeometry(0.0, true);
    expect(atCanvas.boxHeight).toBe(76);
    expect(atCanvas.inputHeight).toBe(76 - 16 - 4);
    expect(atCanvas.settledViewport).toBe(76 - 16 - 4);
  });

  it("endpoint equality: the settle instant matches the route formulas (no hand-off jump)", () => {
    // Undock endpoint (amount 0, expanded render): EVERY channel agrees
    // between the active route frame and the settled frame.
    expect(routeGeometry(0.0, true, { dockActive: true })).toEqual(routeGeometry(0.0, true, { dockActive: false }));
    // Dock endpoint (amount 1, compact render): every COMPACT-consumed
    // channel agrees; the box height alone differs (42.75 floored vs 1) and
    // the compact JSX never consumes it.
    const active = routeGeometry(1.0, false, { dockActive: true });
    const settled = routeGeometry(1.0, false, { dockActive: false });
    expect(active.inputHeight).toBe(settled.inputHeight);
    expect(active.settledViewport).toBe(settled.settledViewport);
    expect(active.textPad).toBe(settled.textPad);
    expect(active.clusterDy).toBe(settled.clusterDy);
    expect(active.clusterInset).toBe(settled.clusterInset);
    expect(active.textGlide).toBe(settled.textGlide);
  });

  it("reversed trajectory: the geometry depends on the frame, not the direction of travel", () => {
    for (const renderedExpanded of [false, true]) {
      const ascending = SAMPLES.map((amount) => routeGeometry(amount, renderedExpanded));
      const descending = [...SAMPLES].reverse().map((amount) => routeGeometry(amount, renderedExpanded));
      expect(descending.reverse()).toEqual(ascending);
    }
  });

  it("an expanded destination keeps the local flip clock — no compact route geometry", () => {
    const g = routeInputGeometry({
      renderedExpanded: true,
      sessionExpanded: true,
      dockActive: true,
      dockAmount: 0.5,
      flipProgress: 0.3,
      flipFrom: null,
      pillHeight: 100,
      baseHeight: 100,
      stripHeight: 0,
      undockedHeight: HERO,
    });
    // The dock amount must NOT leak into the inner channels…
    expect(g.layoutProgress).toBe(0.3);
    expect(g.textPad).toBe(morphTextPad(0.3));
    // …and no one-line floor is imposed: the raw budget stands.
    expect(g.boxHeight).toBe(100 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT);
    expect(g.inputHeight).toBe(100 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - morphTextPad(0.3) - 4);
    expect(g.settledViewport).toBe(100 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - TEXTAREA_PAD_V);
    // The compact RENDER's route glide, though, keys off the active frame
    // alone (composer.rs:7793-7795) — even with an expanded session state.
    const compactRender = routeInputGeometry({
      renderedExpanded: false,
      sessionExpanded: true,
      dockActive: true,
      dockAmount: 0.5,
      flipProgress: 0.3,
      flipFrom: null,
      pillHeight: 100,
      baseHeight: 100,
      stripHeight: 0,
      undockedHeight: HERO,
    });
    expect(compactRender.layoutProgress).toBe(0.3);
    expect(compactRender.textGlide).toBe(collapseTextGlide(HERO, 0.5));
    expect(compactRender.boxHeight).toBe(100 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT);
  });

  it("strips subtract exactly once from the box budget", () => {
    const stripHeight = 68 + 36; // one attachment row + the comments chip
    const bare = routeGeometry(0.2, false);
    const withStrips = routeInputGeometry({
      renderedExpanded: false,
      sessionExpanded: false,
      dockActive: true,
      dockAmount: 0.2,
      flipProgress: 1,
      flipFrom: null,
      pillHeight: dockHeight(0.2, INPUT_LINE_HEIGHT, false) + stripHeight,
      baseHeight: dockHeight(0.2, INPUT_LINE_HEIGHT, false),
      stripHeight,
      undockedHeight: HERO,
    });
    // pill + strips − strips − border − actions ≡ pill − border − actions.
    expect(withStrips.boxHeight).toBe(bare.boxHeight);
    expect(withStrips.inputHeight).toBe(bare.inputHeight);
    expect(withStrips.settledViewport).toBe(bare.settledViewport);
  });

  it("no one-line floor on the ordinary settled/morphing calculation", () => {
    const settledSmall = routeInputGeometry({
      renderedExpanded: true,
      sessionExpanded: false,
      dockActive: false,
      dockAmount: 0,
      flipProgress: 1,
      flipFrom: null,
      pillHeight: 60,
      baseHeight: 60,
      stripHeight: 0,
      undockedHeight: HERO,
    });
    expect(settledSmall.boxHeight).toBe(60 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT);
    expect(settledSmall.settledViewport).toBe(0);
    // The settled empty canvas: the full 76px box, 56px viewport.
    const canvas = routeGeometry(0.0, true, { dockActive: false });
    expect(canvas.boxHeight).toBe(76);
    expect(canvas.settledViewport).toBe(124 - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - TEXTAREA_PAD_V);
    // The settled compact pill: a 1px box budget, the one-line viewport.
    const compact = routeGeometry(1.0, false, { dockActive: false });
    expect(compact.boxHeight).toBe(COMPACT_TOTAL_HEIGHT - PILL_BORDER_V - ACTIONS_ROW_HEIGHT);
    expect(compact.settledViewport).toBe(INPUT_LINE_HEIGHT);
    // The local flip glide still walks the compact text down off-route.
    const flipping = routeGeometry(0.0, false, { dockActive: false, flipProgress: 0.5, flipFrom: 124 });
    expect(flipping.textGlide).toBe(collapseTextGlide(124, 0.5));
  });
});
