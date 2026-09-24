import { describe, expect, it } from "vitest";
import { motion } from "@zeron/theme";
import {
  CHAT_PANEL_MIN,
  PANE_RESIZE_HITBOX_HALF_WIDTH,
  PANEL_TOGGLE_SLOTS,
  RESIZE_EDGE_NUDGE,
  SIDEBAR_DEFAULT,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  TITLEBAR_CONTENT_START,
  CLUSTER_BUTTONS_WIDTH,
  TITLEBAR_ACTION_SLOT_WIDTH,
  TITLEBAR_HEIGHT,
  captionButtonsWidth,
  clampSidebarWidth,
  clusterClearance,
  clusterButtonsStart,
  conversationWidth,
  cubicBezierEval,
  evalWidthTween,
  resizeBounceOffset,
  resizeDragSample,
  rightPaneMaxWidth,
  rightPaneTakeoverWidth,
  rightPanelContentWidth,
  sidebarLayout,
  sidebarTarget,
  stablePanelContentWidth,
  titlebarPaneBandWidth,
  titlebarAvailableTitlebarWidth,
  titlebarRowLeft,
  titlebarSpacerWidth,
} from "../src/state/layout";
import {
  RIGHT_PANE_DEFAULT,
  RIGHT_PANE_MIN,
  resolvePaneWidth,
  type ChatPaneState,
} from "../src/state/right-pane";
import { uiSettings } from "../src/state/ui-settings";

/**
 * The shell's column arithmetic, against the desktop's (`shell.rs:189`,
 * `:444`, `:450` and `settings.rs`'s bounds). These numbers are the parity
 * contract: a window has to divide the same way in both clients.
 */

function pane(over: Partial<ChatPaneState> = {}): ChatPaneState {
  return {
    open: true,
    expanded: false,
    filesOpen: false,
    active: { kind: "diff", id: "d1" },
    tabs: [{ kind: "diff", id: "d1" }],
    width: RIGHT_PANE_DEFAULT,
    ...over,
  };
}

describe("column widths", () => {
  it("matches the desktop's constants", () => {
    expect([SIDEBAR_MIN, SIDEBAR_DEFAULT, SIDEBAR_MAX]).toEqual([224, 256, 400]);
    expect([RIGHT_PANE_MIN, RIGHT_PANE_DEFAULT, CHAT_PANEL_MIN]).toEqual([360, 520, 300]);
  });

  it("divides a 1440px window three ways", () => {
    expect(conversationWidth(1440, 256, 0)).toBe(1184);
    expect(conversationWidth(1440, 0, 0)).toBe(1440);
    expect(conversationWidth(1440, 256, 520)).toBe(664);
    expect(conversationWidth(1440, 0, 520)).toBe(920);
  });

  it("never reports a negative conversation", () => {
    expect(conversationWidth(600, 400, 400)).toBe(0);
  });

  it("caps a drag at the conversation's floor", () => {
    // 1440 - 256 - 300; the chat keeps 300 whatever the pointer does.
    expect(rightPaneMaxWidth(1440, 256)).toBe(884);
    expect(rightPaneMaxWidth(1440, 0)).toBe(1140);
    expect(conversationWidth(1440, 256, rightPaneMaxWidth(1440, 256))).toBe(CHAT_PANEL_MIN);
  });

  it("lets the pane — not the chat — yield on a window too small for both", () => {
    // 900 - 256 - 300 = 344, under the pane's own 360 minimum. The desktop
    // deliberately hands the scarce space to the conversation.
    const max = rightPaneMaxWidth(900, 256);
    expect(max).toBe(344);
    expect(max).toBeLessThan(RIGHT_PANE_MIN);
    expect(conversationWidth(900, 256, max)).toBe(CHAT_PANEL_MIN);
  });

  it("gives takeover everything but the sidebar", () => {
    expect(rightPaneTakeoverWidth(1440, 256)).toBe(1184);
    expect(rightPaneTakeoverWidth(1440, 0)).toBe(1440);
    expect(conversationWidth(1440, 256, rightPaneTakeoverWidth(1440, 256))).toBe(0);
  });

  it("right_pane_ceiling_preserves_the_chat_floor (shell.rs:8283)", () => {
    // The desktop's own asserted pair, on its own window sizes.
    expect(rightPaneMaxWidth(1200, 256)).toBe(644);
    expect(rightPaneMaxWidth(800, 256)).toBe(244);
    expect(conversationWidth(800, 256, rightPaneMaxWidth(800, 256))).toBe(CHAT_PANEL_MIN);
  });

  it("right_pane_takeover_consumes_the_chat_column (shell.rs:8293)", () => {
    expect(rightPaneTakeoverWidth(1200, 256)).toBe(944);
    expect(conversationWidth(1200, 256, 944)).toBe(0);
    // The conversation floors at zero, never negative (shell.rs:8297).
    expect(conversationWidth(1320, 256, 520)).toBe(544);
    expect(conversationWidth(1320, 256, 1064)).toBe(0);
  });
});

describe("sidebar width", () => {
  it("clamps a drag into the desktop's bounds", () => {
    expect(clampSidebarWidth(100)).toBe(SIDEBAR_MIN);
    expect(clampSidebarWidth(900)).toBe(SIDEBAR_MAX);
    expect(clampSidebarWidth(300)).toBe(300);
  });

  it("heals a corrupted width to the default", () => {
    expect(clampSidebarWidth(Number.NaN)).toBe(SIDEBAR_DEFAULT);
  });

  it("lays out at zero while collapsed, retaining the dragged width", () => {
    expect(sidebarTarget({ width: 320, collapsed: true })).toBe(0);
    expect(sidebarTarget({ width: 320, collapsed: false })).toBe(320);
  });
});

describe("titlebarRowLeft", () => {
  const row = (sidebar: number, over: { takeover?: boolean; showsNewSession?: boolean } = {}) =>
    titlebarRowLeft({
      sidebar,
      showsNewSession: over.showsNewSession ?? true,
      takeover: over.takeover ?? false,
    });

  it("starts titlebar content past the control cluster", () => {
    // 10 cluster pad + 82 buttons + 12 identity gap.
    expect(TITLEBAR_CONTENT_START).toBe(104);
  });

  it("puts the identity on the conversation's own left edge", () => {
    // The desktop capture: a 256px sidebar puts the identity at 272.
    expect(row(256)).toBe(272);
    expect(row(400)).toBe(416);
  });

  it("glides with the sidebar but never slides under the cluster", () => {
    // 104 + the 32px new-session slot. Collapsing stops here, not at 16.
    expect(row(0)).toBe(136);
    expect(row(100)).toBe(136);
    // Past the clamp it tracks the sidebar one-for-one.
    expect(row(130)).toBe(146);
  });

  it("drops the new-session slot when the + is absent", () => {
    expect(row(0, { showsNewSession: false })).toBe(104);
  });

  it("pulls back 8px left of the seam in takeover", () => {
    // The strip's own 8px pad then lands its first chip on the pane gutter.
    expect(row(256, { takeover: true })).toBe(248);
    // Still clears the cluster when the sidebar is collapsed.
    expect(row(0, { takeover: true })).toBe(104 - 12 + 32 - 14);
  });
});

/*
 * The phone geometry inputs (ticket 50, mobile-native — the desktop has no
 * phone layout, so no desktop test maps): at ≤768px the sidebar is a fixed
 * overlay out of flow, and `AppShell` feeds the desktop width functions the
 * term 0 (`sidebarForGeometry = phone ? 0 : sidebarWidth`) instead of the
 * dragged column width. Today's phone reality feeds the live width (e.g.
 * 304), which pushes the identity to x=320 of a 375px window and collapses
 * the pane's width inputs to 0.
 */
describe("phone geometry inputs", () => {
  it("titlebarRowLeft phone sidebar is out of flow", () => {
    // With a chat selected the `+` slot rides in: max(0 + 16, 104 + 32) = 136;
    // otherwise, and on the blank canvas, max(16, 104) = 104 — the identity
    // sits next to the window-control cluster instead of at 320 (304 + 16).
    expect(titlebarRowLeft({ sidebar: 0, showsNewSession: true, takeover: false })).toBe(136);
    expect(titlebarRowLeft({ sidebar: 0, showsNewSession: false, takeover: false })).toBe(104);
  });

  it("right_pane_ceiling_keeps_a_phone_floor", () => {
    // 375 - 0 - 300: the pane keeps a 75px floor when the sidebar term is 0,
    // instead of the 0 the live width produces (375 - 304 - 300 < 0).
    expect(rightPaneMaxWidth(375, 0)).toBe(75);
    expect(rightPaneMaxWidth(375, 304)).toBe(0);
  });

  it("titlebarPaneBandWidth phone inputs no longer collapse to zero", () => {
    // rowLeft 136 (the phone identity inset) and a real pane width: the band
    // resolves above zero — min(75 - 6, 375 - 136 - 6 - 24) - 56 = 13 —
    // where the live-width inputs collapsed it to 0.
    expect(
      titlebarPaneBandWidth({ viewport: 375, paneWidth: 75, rowLeft: 136, takeover: false }),
    ).toBe(13);
  });
});

/*
 * The phone pane drawer's width inputs (ticket 52, mobile-native — the
 * desktop has no phone layout, so no desktop test maps): 50's
 * `sidebarForGeometry = phone ? 0 : sidebarWidth` term is consumed here,
 * not re-landed — these cases pin what it buys the pane once the phone
 * form is the drawer (§2.3's verification math).
 */
describe("phone pane drawer inputs (ticket 52)", () => {
  it("resolvePaneWidth phone inputs: sidebar term is zero", () => {
    // The stored 520 against the phone ceiling: min(520, 375 - 0 - 300) = 75
    // — the number `--rb-pane-open` carries, where the live dragged width
    // (375 - 304 - 300 < 0) starved it to 0.
    expect(resolvePaneWidth(pane({ width: 520 }), 375, 0)).toBe(75);
    // The expanded arm hands the drawer the whole viewport: 375 - 0.
    expect(resolvePaneWidth(pane({ width: 520, expanded: true }), 375, 0)).toBe(375);
  });

  it("titlebarPaneBandWidth phone inputs no longer collapse the band", () => {
    // The phone-corrected inputs (sidebar 0 → rowLeft 136, pane 75): 13 —
    // the band the strip's in-drawer header supersedes, but which the
    // titlebar still consumes so nothing downstream reads 0-by-accident.
    // (The b1484015 three-gap budget does not bite here: the pane side of
    // the min — 75 - 6 - 56 — is the narrower one.)
    expect(
      titlebarPaneBandWidth({ viewport: 375, paneWidth: 75, rowLeft: 136, takeover: false }),
    ).toBe(13);
    // Today's shape, documented: the live dragged sidebar (304 → rowLeft
    // 320, pane 0) starves the band to 0 — the "i dont see the tabs" bug.
    expect(
      titlebarPaneBandWidth({ viewport: 375, paneWidth: 0, rowLeft: 320, takeover: false }),
    ).toBe(0);
  });
});

describe("titlebar cluster geometry", () => {
  it("titlebar_cluster_matches_zeron_window_controls (shell.rs:8447)", () => {
    // 24·3 controls + the 8px group gap + the 2px control gap.
    expect(CLUSTER_BUTTONS_WIDTH).toBe(82);
    // The `+`'s slot: the 8px group gap plus its own 24px control.
    expect(TITLEBAR_ACTION_SLOT_WIDTH).toBe(32);
  });

  it("titlebar_spacer_selects_per_platform_and_fullscreen (shell.rs:8462)", () => {
    // Off macOS there is no spacer at all — no phantom flex child.
    expect(titlebarSpacerWidth(false, false, 10)).toBe(0);
    expect(titlebarSpacerWidth(false, true, 10)).toBe(0);
    // macOS clears its traffic lights: 88 (12 fullscreen) minus the pad.
    expect(titlebarSpacerWidth(true, false, 10)).toBe(78);
    expect(titlebarSpacerWidth(true, true, 10)).toBe(2);
    // The browser owns the window: no captions to clear, cluster start flat.
    expect(titlebarSpacerWidth(false, false, 0)).toBe(0);
    expect(clusterButtonsStart(false, false, 0)).toBe(10);
    expect(TITLEBAR_CONTENT_START).toBe(104);
  });

  it("cluster_clearance_clears_the_overlay_buttons (shell.rs:8508)", () => {
    // Web: 10 + 82 + 8 − 10 = 90 — a full-bleed header starts past the
    // cluster with room for the group gap.
    expect(clusterClearance(false, false, 0, 10)).toBe(90);
    // macOS: the cluster starts at 88 instead of 10.
    expect(clusterClearance(true, false, 0, 10)).toBe(168);
    // Linux left captions: two caption buttons push the cluster right.
    expect(captionButtonsWidth(2)).toBe(50);
    expect(clusterClearance(false, false, 2, 10)).toBe(142);
  });
});

describe("titlebarPaneBandWidth", () => {
  const band = (paneWidth: number, over: { takeover?: boolean; sidebar?: number } = {}) => {
    const sidebar = over.sidebar ?? 256;
    const takeover = over.takeover ?? false;
    return titlebarPaneBandWidth({
      viewport: 1440,
      paneWidth,
      rowLeft: titlebarRowLeft({ sidebar, showsNewSession: true, takeover }),
      takeover,
    });
  };

  it("is zero while the pane is shut", () => {
    expect(band(0)).toBe(0);
  });

  it("tracks the pane, less the edge inset and the toggle slots", () => {
    // 520 - 6 - 56. The strip's right edge then lands on the pane's, beside
    // the two fixed toggle anchors.
    expect(band(520)).toBe(458);
    expect(band(360)).toBe(298);
  });

  it("budgets the second fixed slot — PANEL_TOGGLE_SLOTS (tabs.rs:48/59)", () => {
    // The desktop's two fixed right-edge anchors — the Files toggle and
    // the pane toggle — each keep a 28px slot even while the pane is shut,
    // so the band ends 56 short of the pane's own width, not 28.
    expect(PANEL_TOGGLE_SLOTS).toBe(56);
    expect(band(520)).toBe(520 - 6 - PANEL_TOGGLE_SLOTS);
    // A pane exactly as wide as the two slots leaves the band empty.
    expect(band(62)).toBe(0);
  });

  it("rides intermediate widths, so it glides with the column", () => {
    expect(band(260)).toBe(198);
  });

  it("stays capped to the room the row has left", () => {
    // A pane wider than the row can hold must not overflow and clip right.
    const wide = band(1400);
    // avail = 1440 - 272 - 6 - 24 (three gaps: title, Actions, spacer —
    // b1484015's budget) = 1138; minus the 56px toggle pair.
    expect(wide).toBe(1082);
  });

  it("still animates in takeover rather than snapping to full width", () => {
    const full = band(1184, { takeover: true });
    // avail = 1440 - 248 - 6 - 8 = 1178; pane-6 = 1178. Same, minus 56.
    expect(full).toBe(1122);
    // Half way through the glide it is genuinely half way.
    expect(band(700, { takeover: true })).toBe(638);
  });
});

describe("titlebarAvailableTitlebarWidth (b1484015 parity, tabs.rs)", () => {
  it("subtracts the row's left inset, edge inset, trailing strip and three gaps", () => {
    // 1440 - 272 - 6 - 486 - 24 = 652.
    expect(
      titlebarAvailableTitlebarWidth({ viewport: 1440, rowLeft: 272, trailingWidth: 486 }),
    ).toBe(652);
  });

  it("reads the full free row while the pane is shut", () => {
    expect(
      titlebarAvailableTitlebarWidth({ viewport: 1440, rowLeft: 272, trailingWidth: 0 }),
    ).toBe(1138);
  });

  it("never goes negative on phone inputs", () => {
    expect(
      titlebarAvailableTitlebarWidth({ viewport: 375, rowLeft: 320, trailingWidth: 0 }),
    ).toBe(25);
    expect(
      titlebarAvailableTitlebarWidth({ viewport: 375, rowLeft: 360, trailingWidth: 75 }),
    ).toBe(0);
  });
});

describe("resolvePaneWidth", () => {
  it("is zero while closed", () => {
    expect(resolvePaneWidth(pane({ open: false }), 1440, 256)).toBe(0);
  });

  it("uses the stored width when it fits", () => {
    expect(resolvePaneWidth(pane(), 1440, 256)).toBe(520);
  });

  it("shrinks a too-wide stored width without destroying it", () => {
    const stored = pane({ width: 1000 });
    // 1440 - 256 - 300 = 884.
    expect(resolvePaneWidth(stored, 1440, 256)).toBe(884);
    // Widen the window and the user's own width comes back.
    expect(resolvePaneWidth(stored, 1920, 256)).toBe(1000);
  });

  it("follows the sidebar: collapsing it hands the room to the conversation", () => {
    const stored = pane({ width: 1000 });
    expect(resolvePaneWidth(stored, 1440, 0)).toBe(1000);
  });

  it("takes the window over when expanded", () => {
    expect(resolvePaneWidth(pane({ expanded: true }), 1440, 256)).toBe(1184);
  });
});

describe("right_pane_ceiling_preserves_the_chat_floor", () => {
  it("asserted values from shell.rs:8283-8291", () => {
    expect(rightPaneMaxWidth(1200, 256)).toBe(644);
    // 800 - 256 - 300 = 244, below the pane's own 360 floor — the pane yields.
    expect(rightPaneMaxWidth(800, 256)).toBe(244);
    expect(rightPaneMaxWidth(800, 256)).toBeLessThan(RIGHT_PANE_MIN);
  });
});

describe("right_pane_takeover_consumes_the_chat_column", () => {
  it("asserted values from shell.rs:8293-8297", () => {
    expect(rightPaneTakeoverWidth(1200, 256)).toBe(944);
    expect(conversationWidth(1320, 256, 520)).toBe(544);
    expect(conversationWidth(1320, 256, 1064)).toBe(0);
  });
});

describe("right_pane_takeover_control_reverses_direction", () => {
  it("flips between the stored width and the takeover width", () => {
    // `toggle_right_pane_expand` swaps the transition's direction; the pure
    // half of that is the target flipping between the two width rules.
    const stored = pane({ width: 520 });
    expect(resolvePaneWidth(stored, 1320, 256)).toBe(520);
    expect(resolvePaneWidth({ ...stored, expanded: true }, 1320, 256)).toBe(1064);
    expect(resolvePaneWidth({ ...stored, expanded: false }, 1320, 256)).toBe(520);
  });
});

describe("right_panel_content_keeps_the_larger_width_only_during_transition", () => {
  it("asserted values from shell.rs:8416-8444", () => {
    // Open/close: the content holds the LARGER endpoint (520, not 0).
    expect(rightPanelContentWidth(0, [520, 0], null)).toBe(520);
    // Takeover: the content tween OVERRIDES — it tracks the frame (760).
    expect(rightPanelContentWidth(1064, [520, 1064], 760)).toBe(760);
    // Steady state: the target itself.
    expect(stablePanelContentWidth(520, null)).toBe(520);
    expect(stablePanelContentWidth(0, [520, 0])).toBe(520);
  });
});

describe("cubic_bezier_eval", () => {
  it("evaluates the resize curve (ease-out) at the known points", () => {
    const easeOut: [number, number, number, number] = [0, 0, 0.58, 1];
    expect(cubicBezierEval(easeOut, 0)).toBe(0);
    expect(cubicBezierEval(easeOut, 1)).toBe(1);
    // CSS ease-out at half progress ≈ 0.685 — the UnitBezier solve's
    // published value for cubic-bezier(0, 0, 0.58, 1).
    expect(cubicBezierEval(easeOut, 0.5)).toBeCloseTo(0.685, 3);
    // Endpoints fixed at (0,0)/(1,1): outside inputs clamp.
    expect(cubicBezierEval(easeOut, -0.5)).toBe(0);
    expect(cubicBezierEval(easeOut, 1.5)).toBe(1);
  });

  it("is the identity when x and y share one curve", () => {
    // Same control points on both axes: y(t(x)) = x for every input.
    const identity: [number, number, number, number] = [0, 0, 1, 1];
    for (const x of [0, 0.1, 0.25, 0.5, 0.75, 0.9, 1]) {
      expect(cubicBezierEval(identity, x)).toBeCloseTo(x, 6);
    }
  });

  it("is monotonic non-decreasing along the resize curve", () => {
    const easeOut: [number, number, number, number] = [0, 0, 0.58, 1];
    let previous = 0;
    for (let step = 1; step <= 100; step += 1) {
      const value = cubicBezierEval(easeOut, step / 100);
      expect(value).toBeGreaterThanOrEqual(previous);
      previous = value;
    }
  });
});

describe("eval_tween_widths", () => {
  it("rides the catalog's resize spec — 200ms on the ease-out curve", () => {
    const spec = motion.specs.find((entry) => entry.name === "resize");
    expect(spec).toBeDefined();
    expect(spec!.durationMs).toBe(200);
    expect(motion.curves[spec!.curve]).toEqual([0, 0, 0.58, 1]);
  });

  it("starts at `from`, ends exactly at `to`, never overshoots", () => {
    expect(evalWidthTween(520, 1064, 0)).toBe(520);
    expect(evalWidthTween(520, 1064, 200)).toBe(1064);
    // Stale (past the duration): exactly the target — the settled inline
    // style the rAF loop hands over to is the same value.
    expect(evalWidthTween(520, 1064, 10_000)).toBe(1064);
    expect(evalWidthTween(520, 0, 200)).toBe(0);
    expect(evalWidthTween(520, 0, 0)).toBe(520);
  });

  it("lerps on the eased progress, monotonic toward the target", () => {
    const spec = motion.specs.find((entry) => entry.name === "resize")!;
    const curve = motion.curves[spec.curve]!;
    expect(evalWidthTween(520, 1064, 50)).toBeCloseTo(
      520 + (1064 - 520) * cubicBezierEval(curve, 0.25),
      5,
    );
    let previous = 520;
    for (let elapsed = 0; elapsed <= 200; elapsed += 10) {
      const value = evalWidthTween(520, 1064, elapsed);
      expect(value).toBeGreaterThanOrEqual(previous);
      expect(value).toBeLessThanOrEqual(1064);
      previous = value;
    }
  });
});

describe("pane_resize_hitboxes_yield_the_titlebar_chrome", () => {
  it("the 20px target starts TITLEBAR_HEIGHT down", () => {
    // shell.rs:173-175, asserted at 8401-8406: vertical seams yield the
    // global titlebar so its chrome stays clickable across an animated
    // pane boundary.
    expect(PANE_RESIZE_HITBOX_HALF_WIDTH).toBe(10);
    expect(TITLEBAR_HEIGHT).toBe(38);
  });
});

describe("sidebar_drag_nudges_each_edge_once_until_rearmed", () => {
  it("a held pointer bounces once per edge, rearmed by leaving it", () => {
    // shell.rs:8137: the latch is what makes a held pointer produce exactly
    // one nudge instead of restarting the animation on every drag event.
    // First arrival at the min edge (latch empty) arms the bounce.
    let sample = resizeDragSample(180, SIDEBAR_MIN, SIDEBAR_MAX, null, false);
    expect(sample.width).toBe(SIDEBAR_MIN);
    expect(sample.edge).toBe("min");
    expect(sample.startsBounce).toBe(true);
    let latched = sample.edge;
    // Held at the same edge: clamped, at the edge, but NO second bounce.
    sample = resizeDragSample(178, SIDEBAR_MIN, SIDEBAR_MAX, latched, false);
    expect(sample.startsBounce).toBe(false);
    // The max edge is a different edge: it bounces even mid-press.
    sample = resizeDragSample(500, SIDEBAR_MIN, SIDEBAR_MAX, latched, false);
    expect(sample.edge).toBe("max");
    expect(sample.startsBounce).toBe(true);
    latched = sample.edge;
    // Leaving the edge rearms the latch.
    sample = resizeDragSample(300, SIDEBAR_MIN, SIDEBAR_MAX, latched, false);
    expect(sample.edge).toBeNull();
    expect(sample.width).toBe(300);
    expect(resizeDragSample(180, SIDEBAR_MIN, SIDEBAR_MAX, null, false).startsBounce).toBe(true);
  });
});

describe("sidebar_drag_stays_exact_in_range_and_reduced_motion_never_nudges", () => {
  it("in-range widths pass through unclamped", () => {
    const sample = resizeDragSample(320, SIDEBAR_MIN, SIDEBAR_MAX, null, false);
    expect(sample.width).toBe(320);
    expect(sample.edge).toBeNull();
    expect(sample.startsBounce).toBe(false);
  });

  it("reduced motion never arms a bounce, even at a fresh edge", () => {
    // shell.rs:8166 — `eval_resize_edge_bounce` returns 0 under reduce.
    const sample = resizeDragSample(180, SIDEBAR_MIN, SIDEBAR_MAX, null, true);
    expect(sample.width).toBe(SIDEBAR_MIN);
    expect(sample.edge).toBe("min");
    expect(sample.startsBounce).toBe(false);
  });
});

describe("right_pane_uses_the_shared_clamp_and_edge_latch", () => {
  it("clamps into [RIGHT_PANE_MIN, max] with the same edge rules", () => {
    // shell.rs:8185: the right pane shares resize_drag_sample; max is
    // right_pane_max_width (the chat floor already priced in).
    const max = rightPaneMaxWidth(1440, 256);
    expect(max).toBe(884);
    const low = resizeDragSample(200, RIGHT_PANE_MIN, max, null, false);
    expect(low.width).toBe(RIGHT_PANE_MIN);
    expect(low.edge).toBe("min");
    expect(low.startsBounce).toBe(true);
    const high = resizeDragSample(2000, RIGHT_PANE_MIN, max, "min", false);
    expect(high.width).toBe(884);
    expect(high.edge).toBe("max");
    expect(high.startsBounce).toBe(true);
    const held = resizeDragSample(2000, RIGHT_PANE_MIN, max, "max", false);
    expect(held.startsBounce).toBe(false);
  });
});

describe("sidebar_bounce_has_rounded_out_and_return_phases", () => {
  it("rises 5px out over the first 32%, returns over the rest, zero at joins", () => {
    // shell.rs:8208 — a two-phase smoothstep pulse with zero velocity at
    // both joins. Outbound share 0.32 of 220ms = 70.4ms.
    expect(resizeBounceOffset("max", 0)).toBe(0);
    expect(resizeBounceOffset("max", 220)).toBe(0);
    expect(resizeBounceOffset("max", -50)).toBe(0);
    expect(resizeBounceOffset("max", 10_000)).toBe(0);

    const peak = resizeBounceOffset("max", 70);
    expect(peak).toBeGreaterThan(4.9);
    expect(peak).toBeLessThanOrEqual(RESIZE_EDGE_NUDGE);
    // The return phase is genuinely on the way back by half the window.
    const half = resizeBounceOffset("max", 110);
    expect(half).toBeGreaterThan(0);
    expect(half).toBeLessThan(peak);
    // Monotone out: 10ms in is smaller than 40ms in.
    expect(resizeBounceOffset("max", 10)).toBeLessThan(resizeBounceOffset("max", 40));
    // The min edge mirrors: negative offsets.
    expect(resizeBounceOffset("min", 70)).toBe(-peak);
    // No edge, no offset.
    expect(resizeBounceOffset(null, 70)).toBe(0);
  });
});

describe("sidebarLayout", () => {
  it("projects the persisted geometry out of the settings store", () => {
    // The geometry lives in ui-settings.ts now; this store is the shell's view
    // onto it, so a write through either side is visible from both.
    uiSettings.update({ sidebarWidth: 300, sidebarCollapsed: false }, "immediate");
    expect(sidebarLayout.getSnapshot()).toEqual({ width: 300, collapsed: false });

    sidebarLayout.toggleCollapsed();
    expect(uiSettings.getSnapshot().sidebarCollapsed).toBe(true);
    // A collapse keeps the dragged width so reopening restores it.
    expect(sidebarTarget(sidebarLayout.getSnapshot())).toBe(0);
    expect(sidebarLayout.getSnapshot().width).toBe(300);

    // A drag sample is clamped before it ever reaches memory, and it reopens
    // the sidebar (`shell.rs`'s drag handler).
    sidebarLayout.setWidth(9999);
    expect(sidebarLayout.getSnapshot()).toEqual({ width: SIDEBAR_MAX, collapsed: false });

    sidebarLayout.reset();
    expect(sidebarLayout.getSnapshot()).toEqual({ width: SIDEBAR_DEFAULT, collapsed: false });
  });

  it("hands useSyncExternalStore a stable snapshot across unrelated changes", () => {
    const before = sidebarLayout.getSnapshot();
    uiSettings.update({ terminalHeight: 400 }, "immediate");
    expect(sidebarLayout.getSnapshot()).toBe(before);
  });
});
