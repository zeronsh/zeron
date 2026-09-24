import { describe, expect, it } from "vitest";
import {
  bottomStackMeasurementMatches,
  dockClearanceCorrection,
  dockFrameSettled,
  dockHeight,
  dockGlideSeconds,
  DockState,
  dockVisualsAdvance,
  dockVisualsReturnFromPanel,
  dockVisualsSettled,
  Glide,
  lerp,
  newThreadTransitionMorph,
  PANEL_HANDOFF_SECONDS,
  PanelHandoff,
  routeChromeOpacities,
  stage,
} from "../src/lib/composer-dock";
import { flipMorphProgress, flipMorphHeight } from "../src/lib/composer-flip";

/**
 * The composer's route dock — each describe named after the
 * `composer_dock.rs` / `composer.rs` unit test it mirrors. The dock is one
 * retargetable clock: the composer glides on a critically damped spring
 * between the hero and the thread, its chrome cross fades on four staged
 * channels that never duplicate a picker, and a short fade-through hides the
 * column's horizontal geometry switch.
 */

describe("route_chrome_crossfade_never_duplicates_picker_controls (composer.rs:9340)", () => {
  it("derives the ramps and never overlaps across 21 samples", () => {
    expect(routeChromeOpacities(1).newThread).toBe(1);
    expect(routeChromeOpacities(1).session).toBe(0);
    expect(routeChromeOpacities(0.5).newThread).toBe(0);
    expect(routeChromeOpacities(0.5).session).toBe(0);
    expect(routeChromeOpacities(0).newThread).toBe(0);
    expect(routeChromeOpacities(0).session).toBe(1);
    for (let step = 0; step <= 20; step += 1) {
      const { newThread, session } = routeChromeOpacities(step / 20);
      expect(newThread === 0 || session === 0).toBe(true);
    }
  });
});

describe("new_thread_route_changes_use_the_coordinated_timeline (composer.rs:9321)", () => {
  it("is 0 at t=0, strictly between at 250 ms, exact at 420 ms, both directions", () => {
    const from = 124;
    const to = 49;
    const docking = newThreadTransitionMorph(from, 0);
    const undocking = newThreadTransitionMorph(to, 0);
    expect(flipMorphHeight(docking, to, 0)).toBe(from);
    const mid = flipMorphHeight(docking, to, 250);
    expect(mid).toBeGreaterThan(to);
    expect(mid).toBeLessThan(from);
    expect(flipMorphProgress(docking, 420)).toBe(1);
    expect(flipMorphHeight(docking, to, 420)).toBe(to);
    expect(flipMorphHeight(undocking, from, 0)).toBe(to);
    const midBack = flipMorphHeight(undocking, from, 250);
    expect(midBack).toBeGreaterThan(to);
    expect(midBack).toBeLessThan(from);
    expect(flipMorphHeight(undocking, from, 420)).toBe(from);
  });
});

describe("dock_height (composer.rs:7490-7500)", () => {
  it("lerps hero height to session height with the 60px session floor", () => {
    // Hero: one line → 22.75 + 20 = 42.75 → clamped to 76 + 48 = 124.
    expect(dockHeight(0, 22.75, false)).toBe(124);
    // Session compact: COMPACT_TOTAL_HEIGHT.
    expect(dockHeight(1, 22.75, false)).toBe(49);
    // Session expanded with a short draft: 60 + 46 + 2 = 108 — skinnier
    // than the hero's 124, which is the point of the 76−16 floor.
    expect(dockHeight(1, 22.75, true)).toBe(108);
    // Session expanded keeps the 260 box cap → 308.
    expect(dockHeight(1, 300, true)).toBe(308);
    // Mid-flight interpolation.
    expect(dockHeight(0.5, 22.75, false)).toBeCloseTo((124 + 49) / 2, 5);
  });
});

describe("dock_clearance_correction (composer.rs:7583)", () => {
  it("reserves the destination footprint, never the animated height", () => {
    const contentHeight = 22.75;
    const strips = 36;
    // Docked destination at a compact session height, pill currently tall:
    const correction = dockClearanceCorrection({ docked: true }, contentHeight, false, strips, 124);
    expect(correction).toBe(49 + strips - 124);
    // The published stack height (measured 124 + correction) is constant at
    // destination + strips while the animated pill height varies below.
    const published = (pill: number): number =>
      pill + dockClearanceCorrection({ docked: true }, contentHeight, false, strips, pill);
    expect(published(124)).toBe(published(80));
    expect(published(80)).toBe(49 + strips);
    // Undocking reserves the hero destination.
    expect(dockClearanceCorrection({ docked: false }, contentHeight, false, 0, 49)).toBe(124 - 49);
  });
});

describe("bottom_stack_measurement_matches (shell.rs:837, 8267-8270)", () => {
  it("compares the measured and expected composer presence", () => {
    expect(bottomStackMeasurementMatches(false, false)).toBe(true);
    expect(bottomStackMeasurementMatches(true, true)).toBe(true);
    expect(bottomStackMeasurementMatches(false, true)).toBe(false);
    expect(bottomStackMeasurementMatches(true, false)).toBe(false);
  });
});

describe("choreography_is_direction_specific_and_selectors_never_duplicate (composer_dock.rs:639)", () => {
  const hero = dockVisualsSettled(false);
  const thread = dockVisualsSettled(true);

  it("stages the four channels on their direction-specific windows", () => {
    expect(dockVisualsAdvance(hero, true, 0.19).transcript).toBe(0);
    expect(dockVisualsAdvance(hero, true, 0.65).transcript).toBe(1);
    expect(dockVisualsAdvance(hero, true, 0.55).selectors).toBe(1);
    expect(dockVisualsAdvance(thread, false, 0.25).transcript).toBe(0);
    expect(dockVisualsAdvance(thread, false, 0.49).selectors).toBe(0);
  });

  it("keeps selectors == 0 || footer == 0 at every point on both timelines", () => {
    for (let step = 0; step <= 100; step += 1) {
      const time = step / 100;
      for (const values of [dockVisualsAdvance(hero, true, time), dockVisualsAdvance(thread, false, time)]) {
        expect(values.selectors === 0 || values.footer === 0).toBe(true);
      }
    }
  });

  it("return_from_panel runs its own 0.320 s clock", () => {
    const returning = dockVisualsAdvance(thread, true, 0.5);
    const early = dockVisualsReturnFromPanel(returning, 0.0);
    expect(early.transcript).toBeCloseTo(returning.transcript, 5);
    const done = dockVisualsReturnFromPanel(returning, 1);
    expect(done.transcript).toBe(0);
    expect(done.footer).toBe(0);
    expect(done.selectors).toBe(1);
    expect(done.dissolve).toBe(0);
  });
});

describe("new_thread_selectors_restore_the_compact_floating_row (composer.rs:9334)", () => {
  it("the selector channel survives a full route round trip", () => {
    // The floating row's channel: 1 on the canvas, 0 docked, and back to 1
    // after the return — the row remounts with its geometry intact (the
    // absolute top -28 / left-right 26 placement is CSS, owned once).
    const canvas = dockVisualsSettled(false);
    const docked = dockVisualsAdvance(canvas, true, 1);
    expect(docked.selectors).toBe(0);
    const returned = dockVisualsAdvance(docked, false, 1);
    expect(returned.selectors).toBe(1);
    expect(returned.transcript).toBe(0);
    expect(returned.footer).toBe(0);
    expect(returned.dissolve).toBe(0);
  });
});

describe("reversal_preserves_position_and_velocity (composer_dock.rs:655)", () => {
  it("a mid-flight reversal is continuous", () => {
    const glide = new Glide(300);
    glide.advance(800, 0.12, 0.42);
    const before = { value: glide.value, velocity: glide.velocity };
    glide.advance(300, 0, 0.47);
    expect(Math.abs(glide.value - before.value)).toBeLessThan(0.001);
    expect(Math.abs(glide.velocity - before.velocity)).toBeLessThan(0.001);
    for (let i = 0; i < 60; i += 1) {
      glide.advance(300, 1 / 120, 0.47);
    }
    expect(Math.abs(glide.value - 300)).toBeLessThan(0.5);
  });
});

describe("normal_dock_is_monotone_and_frame_rate_independent (composer_dock.rs:668)", () => {
  it("settles within a fraction of a pixel at 30/60/120 hz", () => {
    for (const hz of [30, 60, 120]) {
      const glide = new Glide(300);
      for (let i = 0; i < hz / 2; i += 1) {
        const old = glide.value;
        glide.advance(800, 1 / hz, 0.42);
        expect(glide.value).toBeGreaterThanOrEqual(old);
        expect(glide.value).toBeLessThanOrEqual(800);
      }
      expect(Math.abs(glide.value - 800)).toBeLessThan(0.1);
    }
  });
});

describe("initial_and_reduced_motion_frames_snap (composer_dock.rs:680)", () => {
  it("the first frame and reduced motion land at the target with no active clock", () => {
    const state = new DockState();
    expect(state.tick(true, false, 0).amount).toBe(1);
    expect(state.tick(false, true, 0).amount).toBe(0);
    expect(state.frame.active).toBe(false);
  });
});

describe("idle_time_is_not_consumed_by_a_new_target (composer_dock.rs:624)", () => {
  it("a retarget starts the clock at zero and reverses without advancing", () => {
    const state = new DockState();
    state.tick(false, false, 0);
    // The desktop's test injects `state.position`; the web's prepaint call
    // establishes the same live glide pair.
    state.prepaint({ left: 0, top: 700, height: 172 }, 881, false, 0);
    const click = 30_000;
    expect(state.tick(true, false, click).amount).toBe(0);
    const moving = state.tick(true, false, click + 100);
    expect(moving.amount).toBeGreaterThan(0);
    expect(moving.amount).toBeLessThan(1);
    const reverse = state.tick(false, false, click + 100);
    expect(moving.amount).toBe(reverse.amount);
    expect(moving.visuals).toEqual(reverse.visuals);
  });
});

describe("panel_handoff_hides_background_during_geometry_switch_in_both_sidebar_states (composer_dock.rs:456)", () => {
  it("opacity is 0 across p ≈ 0.18-0.26 in both directions", () => {
    for (const docked of [true, false]) {
      const sourcePane = docked ? 0 : 480;
      const targetPane = docked ? 480 : 0;
      const state = new DockState();
      const now = 0;
      state.observePane(!docked, sourcePane, true, now);
      state.tick(!docked, false, now);
      // Establish the live position glides (the desktop's test injects
      // `state.position` directly).
      state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, now);
      state.observePane(docked, targetPane, true, now);
      state.tick(docked, false, now);
      for (const progress of [0.19, 0.22, 0.25]) {
        const at = now + progress * PANEL_HANDOFF_SECONDS * 1000;
        state.observePane(docked, targetPane, true, at);
        expect(state.tick(docked, false, at).visuals.dissolve).toBe(1);
      }
      const at = now + 321;
      state.observePane(docked, targetPane, true, at);
      expect(state.tick(docked, false, at).visuals.dissolve).toBe(docked ? 1 : 0);
    }
  });
});

describe("panel_exit_retains_source_transcript_width_only_until_handoff_ends (composer_dock.rs:488)", () => {
  it("the departing column keeps its source width until the handoff ends", () => {
    const state = new DockState();
    expect(state.transcriptWidth(540, true, false)).toBe(540);
    state.frame = dockFrameSettled(true);
    expect(state.transcriptWidth(1040, false, true)).toBe(540);
    state.frame = dockFrameSettled(false);
    expect(state.transcriptWidth(1040, false, true)).toBe(540);
    expect(state.transcriptWidth(1040, false, false)).toBe(1040);
    expect(state.transcriptWidth(540, true, true)).toBe(540);
    state.frame = dockFrameSettled(true);
    expect(state.transcriptWidth(1040, false, false)).toBe(1040);
  });
});

describe("panel_return_sizes_while_hidden_and_finishes_controls_with_input (composer_dock.rs:502)", () => {
  it("the width snaps inside the invisible interval and controls finish with the input", () => {
    const now = 0;
    const state = new DockState();
    state.observePane(true, 480, true, now);
    state.tick(true, false, now);
    // Establish the live position glides (the desktop's test injects
    // `state.position` directly).
    state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, now);
    state.observePane(false, 0, true, now);
    expect(state.tick(false, false, now).amount).toBe(1);
    // 75 ms in — p ≈ 0.23, inside the invisible interval.
    const hidden = now + 75;
    state.observePane(false, 0, true, hidden);
    const frame = state.tick(false, false, hidden);
    expect(state.opacity()).toBe(0);
    expect(frame.amount).toBe(0);
    for (const seconds of [0.321, 0.4, 0.5]) {
      const at = now + seconds * 1000;
      state.observePane(false, 0, true, at);
      const next = state.tick(false, false, at);
      expect(next.visuals.selectors).toBe(1);
      expect(next.visuals.dissolve).toBe(0);
      expect(next.amount).toBe(0);
    }
  });

  it("layout_width snaps to the target inside the invisible interval", () => {
    const now = 0;
    const state = new DockState();
    state.observePane(true, 480, true, now);
    state.tick(true, false, now);
    state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, now);
    // Docking with a wider column: the glide would animate, but inside
    // p >= 0.22 the width snaps.
    state.observePane(false, 0, true, now);
    state.tick(false, false, now);
    const hidden = now + 75;
    state.observePane(false, 0, true, hidden);
    state.tick(false, false, hidden);
    expect(state.layoutWidth(300, false, hidden)).toBeCloseTo(300, 3);
  });
});

describe("ordinary_resizing_and_same_column_navigation_do_not_fade (panel_handoff.rs)", () => {
  it("no handoff without a docked flip + width change", () => {
    const handoff = new PanelHandoff();
    const now = 0;
    for (const [docked, width] of [
      [true, 0],
      [true, 480],
      [true, 0],
      [false, 0],
    ] as const) {
      expect(handoff.sample(docked, width, true, now, PANEL_HANDOFF_SECONDS)).toBe(false);
      expect(handoff.opacity()).toBe(1);
    }
  });

  it("reversal preserves opacity and disabling resets it to 1", () => {
    const handoff = new PanelHandoff();
    const now = 0;
    handoff.sample(false, 0, true, now, PANEL_HANDOFF_SECONDS);
    handoff.sample(true, 480, true, now, PANEL_HANDOFF_SECONDS);
    const later = now + 180;
    handoff.sample(true, 480, true, later, PANEL_HANDOFF_SECONDS);
    const alpha = handoff.opacity();
    handoff.sample(false, 0, true, later, PANEL_HANDOFF_SECONDS);
    expect(handoff.opacity()).toBe(alpha);
    expect(handoff.sample(false, 0, false, later, PANEL_HANDOFF_SECONDS)).toBe(false);
    expect(handoff.opacity()).toBe(1);
  });
});

describe("prepaint anchors and travel (composer_dock.rs:368-427)", () => {
  it("centers by the TOP of the surface on the hero and glides when docked", () => {
    const state = new DockState();
    const now = 0;
    state.tick(false, false, now);
    const bounds = { left: 262, top: 700, height: 172 };
    // First prepaint: not moving → snap to the hero position.
    const first = state.prepaint(bounds, 881, false, now);
    const heroY = (881 - 172) * 0.5 + 8;
    expect(first.dy).toBeCloseTo(heroY - 700, 3);
    expect(first.moving).toBe(false);
    // Retarget to docked: the reversal-safe glide begins from the painted spot.
    state.tick(true, false, now + 30_000);
    const second = state.prepaint(bounds, 881, false, now + 30_000);
    expect(second.dy).toBeCloseTo(first.dy, 3);
    const later = state.prepaint(bounds, 881, false, now + 30_250);
    expect(later.dy).toBeLessThan(Math.abs(second.dy));
    expect(later.moving).toBe(true);
  });
});

describe("glide constants (motion.rs:476-484)", () => {
  it("match the desktop's shipped values", () => {
    expect(dockGlideSeconds(true)).toBe(0.42);
    expect(dockGlideSeconds(false)).toBe(0.47);
    expect(lerp(10, 20, 0.25)).toBe(12.5);
    expect(stage(0.5, 0, 1)).toBe(0.5);
    expect(stage(-1, 0, 1)).toBe(0);
    expect(stage(2, 0, 1)).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// Host-level cases (ticket 36 §3/§6) — `ConversationPage`'s render-body
// sequence, in the desktop's paint order
// ---------------------------------------------------------------------------

/**
 * One pass of the page's per-render dock wiring, in the order the desktop
 * paints it: `observe_pane` (shell.rs:7901-7906) with the pane's SYNCHRONOUS
 * width, then `transcript_width` (:7915-7919, before the tick so the
 * retention capture sees the pre-flip frame), then `tick` (render_main,
 * :5882-5885), then the per-commit `prepaint` (the layout effect's
 * re-anchor). The suite runs in vitest's node environment with no DOM, so
 * the host wiring is pinned by driving the dock through this exact call
 * order with the exact inputs the page computes — the component glue
 * itself is typechecked by `tsc --noEmit` in `pnpm -r build`.
 */
function hostPass(
  state: DockState,
  input: {
    docked: boolean;
    /** The pane's synchronous width — `right_now`, 0 on the canvas. */
    paneWidth: number;
    /** The conversation column's content width (`main_content_width`). */
    mainWidth: number;
    now: number;
    enabled?: boolean;
  },
): { handoff: boolean; retained: number; frame: ReturnType<DockState["tick"]> } {
  const enabled = input.enabled ?? true;
  const handoff = state.observePane(input.docked, input.paneWidth, enabled, input.now);
  const retained = state.transcriptWidth(input.mainWidth, input.docked, handoff);
  const frame = state.tick(input.docked, false, input.now);
  state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, input.now);
  return { handoff, retained, frame };
}

describe("host: new-thread → chat with stored-open pane (case B, shell.rs:7901-7919)", () => {
  const PANE = 480;
  const CANVAS = 1040;
  const NARROW = CANVAS - PANE;

  it("arms on the navigation commit itself and hides the geometry switch", () => {
    const state = new DockState();
    // Canvas frames: no selection, the pane forced to width 0.
    const canvas = hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    expect(canvas.handoff).toBe(false);
    const flip = 100;
    // The navigation commit: the docked flip AND the pane width 0→480 in the
    // SAME sample — the synchronous input that makes the flagship case arm
    // (the old measured-`columnWidth` input delivered the width one commit
    // late, after the flip, so it never armed at all).
    const armed = hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: flip });
    expect(armed.handoff).toBe(true);
    expect(state.opacity()).toBe(1);
    // The composer's width enters frozen at the canvas cap (768, the
    // stale-measured column at the flip commit)…
    expect(state.layoutWidth(768, false, flip)).toBeCloseTo(768, 3);

    // p = 0.10 — the panel_departure FAST dissolve: already past half on the
    // 0.320 s clock, where the plain docking channel would barely have
    // started (the 0.420 s window) — "a FAST kill, not the 0.420 channel".
    const early = hostPass(state, {
      docked: true,
      paneWidth: PANE,
      mainWidth: NARROW,
      now: flip + 0.1 * PANEL_HANDOFF_SECONDS * 1000,
    });
    const plainChannel = dockVisualsAdvance(
      dockVisualsSettled(false),
      true,
      (0.1 * PANEL_HANDOFF_SECONDS) / dockGlideSeconds(true),
    );
    expect(early.frame.visuals.dissolve).toBeGreaterThan(0.5);
    expect(early.frame.visuals.dissolve).toBeGreaterThan(plainChannel.dissolve);

    // p ∈ [0.19, 0.25] — the invisible interval: the composer is at opacity
    // 0, the hero fully dissolved, and the width switch happens only from
    // p ≥ 0.22 — frozen before, snapped after, while invisible either way.
    for (const progress of [0.19, 0.22, 0.25]) {
      const at = flip + progress * PANEL_HANDOFF_SECONDS * 1000;
      const mid = hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: at });
      expect(state.opacity()).toBe(0);
      expect(mid.frame.visuals.dissolve).toBe(1);
      // The pump gate (shell.rs:7907-7909's motion_active peer): the
      // handoff keeps frames coming while its clock runs.
      expect(state.paneProgress()).not.toBe(null);
    }
    expect(state.layoutWidth(768, false, flip + 0.19 * PANEL_HANDOFF_SECONDS * 1000)).toBeCloseTo(768, 3);
    expect(state.layoutWidth(NARROW, false, flip + 0.25 * PANEL_HANDOFF_SECONDS * 1000)).toBeCloseTo(
      NARROW,
      3,
    );

    // p ≥ 1 — settled: the handoff releases and the composer is back.
    const done = hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: flip + 321 });
    expect(done.handoff).toBe(false);
    expect(state.opacity()).toBe(1);
    expect(state.paneProgress()).toBe(null);
  });

  it("carries the 12 px docking travel, decaying over stage(p, 0.22, 1)", () => {
    const state = new DockState();
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: 100 });
    const at = 100 + 0.25 * PANEL_HANDOFF_SECONDS * 1000;
    hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: at });
    const { dy } = state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, at);
    expect(dy).toBeCloseTo(12 * (1 - stage(0.25, 0.22, 1)), 3);
  });

  it("a mid-handoff reversal preserves the current opacity", () => {
    const state = new DockState();
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: 100 });
    const mid = 100 + 0.1 * PANEL_HANDOFF_SECONDS * 1000;
    hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: mid });
    const alpha = state.opacity();
    expect(alpha).toBeGreaterThan(0);
    expect(alpha).toBeLessThan(1);
    // Back to the canvas in the same frame: the reverse sample re-arms with
    // `from_opacity` set to the CURRENT value — no restart from 1.
    const reversed = hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: mid });
    expect(reversed.handoff).toBe(true);
    expect(state.opacity()).toBeCloseTo(alpha, 5);
  });
});

describe("host: chat with pane → new-thread (case D', shell.rs:7915-7919)", () => {
  const PANE = 480;
  const CANVAS = 1040;
  const NARROW = CANVAS - PANE;

  it("retains the source column width, hidden, until the handoff ends", () => {
    const state = new DockState();
    // Docked frames on the chat-with-pane: the column is narrow.
    expect(hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: 0 }).retained).toBe(
      NARROW,
    );
    const flip = 100;
    // The reverse flip: docked true→false AND pane 480→0 in the same sample.
    const armed = hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: flip });
    expect(armed.handoff).toBe(true);
    // `transcript_width` captured the SOURCE column (560), not the canvas
    // target — the call runs BEFORE the flip's tick, so the capture sees
    // the pre-flip `frame.docked`.
    expect(armed.retained).toBe(NARROW);
    // `amount` is held at the painted value until p ≥ 0.22, then 0.
    expect(armed.frame.amount).toBe(1);
    for (const progress of [0.1, 0.19, 0.25]) {
      const at = flip + progress * PANEL_HANDOFF_SECONDS * 1000;
      const mid = hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: at });
      expect(mid.retained).toBe(NARROW);
      expect(state.paneProgress()).not.toBe(null);
    }
    // The invisible interval: opacity 0 while the geometry switches.
    for (const progress of [0.19, 0.22, 0.25]) {
      hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: flip + progress * PANEL_HANDOFF_SECONDS * 1000 });
      expect(state.opacity()).toBe(0);
    }
    // p ≥ 0.22 — the amount released…
    expect(
      hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: flip + 0.3 * PANEL_HANDOFF_SECONDS * 1000 })
        .frame.amount,
    ).toBe(0);
    // …and the controls finish with the input (return_from_panel).
    const done = hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: flip + 321 });
    expect(done.handoff).toBe(false);
    expect(done.frame.visuals.selectors).toBe(1);
    expect(done.frame.visuals.dissolve).toBe(0);
    // The handoff ended → the retention releases to the canvas width.
    expect(done.retained).toBe(CANVAS);
    expect(state.opacity()).toBe(1);
  });

  it("carries the 8 px undocking travel, decaying over stage(p, 0.22, 1)", () => {
    const state = new DockState();
    hostPass(state, { docked: true, paneWidth: PANE, mainWidth: NARROW, now: 0 });
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 100 });
    const at = 100 + 0.25 * PANEL_HANDOFF_SECONDS * 1000;
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: at });
    const { dy } = state.prepaint({ left: 224, top: 700, height: 172 }, 881, false, at);
    const heroY = (881 - 172) * 0.5 + 8;
    expect(dy).toBeCloseTo(heroY + 8 * (1 - stage(0.25, 0.22, 1)) - 700, 3);
  });
});

describe("host: ordinary resizing and same-column navigation do not fade (panel_handoff.rs:92)", () => {
  const CANVAS = 1040;

  it("cases A and D (no pane on either side) never arm — the pane width is 0→0", () => {
    const state = new DockState();
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    // new-thread → chat WITHOUT a pane: the docked flip arrives with the
    // width equal, so no handoff — the plain 0.420 s dock choreography.
    expect(
      hostPass(state, { docked: true, paneWidth: 0, mainWidth: CANVAS, now: 100 }).handoff,
    ).toBe(false);
    expect(state.opacity()).toBe(1);
    // and back: chat-without-pane → new-thread.
    expect(
      hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 300 }).handoff,
    ).toBe(false);
    expect(state.opacity()).toBe(1);
    expect(state.paneProgress()).toBe(null);
  });

  it("same-column chat→chat (docked stays true) never arms, whatever the flags", () => {
    const state = new DockState();
    hostPass(state, { docked: true, paneWidth: 480, mainWidth: 560, now: 0 });
    // chat A with pane → chat B with pane: same column, no fade.
    expect(
      hostPass(state, { docked: true, paneWidth: 480, mainWidth: 560, now: 100 }).handoff,
    ).toBe(false);
    // chat A with pane → chat B WITHOUT: the pane flags differ (the pane
    // column SNAPS — ticket §2.4.5), but the docked flag does not flip, so
    // the composer never fades.
    expect(
      hostPass(state, { docked: true, paneWidth: 0, mainWidth: CANVAS, now: 300 }).handoff,
    ).toBe(false);
    expect(state.opacity()).toBe(1);
  });

  it("an ordinary pane open/close or drag on the same chat never arms", () => {
    const state = new DockState();
    hostPass(state, { docked: true, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    // The pane opens (width 0→480) with docked unchanged.
    expect(
      hostPass(state, { docked: true, paneWidth: 480, mainWidth: 560, now: 100 }).handoff,
    ).toBe(false);
    // A drag's width stream: continuous changes, docked constant.
    expect(
      hostPass(state, { docked: true, paneWidth: 430, mainWidth: 610, now: 116 }).handoff,
    ).toBe(false);
    expect(state.opacity()).toBe(1);
    expect(state.paneProgress()).toBe(null);
  });

  it("reduced motion disables the handoff entirely — opacity resets to 1", () => {
    const state = new DockState();
    hostPass(state, { docked: false, paneWidth: 0, mainWidth: CANVAS, now: 0 });
    // The flip under `dockReduced` (reduced motion or the phone layer):
    // `enabled=false` resets the handoff (panel_handoff.rs:32-35).
    const flip = hostPass(state, {
      docked: true,
      paneWidth: 480,
      mainWidth: 560,
      now: 100,
      enabled: false,
    });
    expect(flip.handoff).toBe(false);
    expect(state.opacity()).toBe(1);
    expect(state.paneProgress()).toBe(null);
  });
});

describe("host: panel exit retains source transcript width only until handoff ends (composer_dock.rs:488)", () => {
  it("the departing column keeps its source width for exactly the handoff window", () => {
    const state = new DockState();
    // The chat's column: 560 wide (the pane takes 480 of a 1040 viewport).
    expect(state.transcriptWidth(560, true, false)).toBe(560);
    // Establish the docked sample (previous = (true, 480)) and the docked
    // frame the capture condition reads…
    expect(state.observePane(true, 480, true, 0)).toBe(false);
    state.tick(true, false, 0);
    // …then the undock flip: `transcript_width` runs BEFORE the tick flips
    // `frame.docked`, so the capture holds the SOURCE column.
    const handoff = state.observePane(false, 0, true, 100);
    expect(handoff).toBe(true);
    expect(state.transcriptWidth(1040, false, handoff)).toBe(560);
    state.tick(false, false, 100);
    // Mid-handoff the retention holds (the frame is already undocked).
    expect(state.transcriptWidth(1040, false, true)).toBe(560);
    // The handoff ends → the retention releases.
    state.observePane(false, 0, true, 100 + 321);
    expect(state.transcriptWidth(1040, false, false)).toBe(1040);
    // A plain undock (no pane, no handoff) never retains.
    expect(state.transcriptWidth(1040, true, false)).toBe(1040);
    state.frame = dockFrameSettled(true);
    expect(state.transcriptWidth(1040, false, false)).toBe(1040);
  });
});
