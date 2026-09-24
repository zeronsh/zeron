import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  evalWidthTween,
  TITLEBAR_CONTROL_GAP,
  TITLEBAR_GROUP_GAP,
  TITLEBAR_HEIGHT,
  TITLEBAR_TOP_PAD,
} from "../src/state/layout";
import {
  ISLAND_TWEEN_MS,
  IslandTweenDriver,
  islandTarget,
  titlebarIslandHorizontalGeometry,
  titlebarIslandVerticalGeometry,
} from "../src/components/titlebar";

/**
 * Ticket 34 — the titlebar island's pure logic, mirrored from the desktop:
 * `titlebar_island_vertical_geometry` (shell.rs:829-835) and `island_target`
 * (shell.rs:3990-4001, as amended by ticket 48's resolved-artwork
 * predicate). The tween's frames are `evalWidthTween`, already covered by
 * `tests/layout.test.ts`. Ticket 60 adds the horizontal half: the island's
 * span with the `+` conditioned out of the cluster row. Ticket 66 corrects
 * that half: an absolute child's insets resolve against the parent's
 * PADDING box, so over the desktop's padded cluster (and the web's
 * ticket-66 cluster of the same shape) the island is [6, 102] / [6, 134] —
 * the [16, 92] these tests previously encoded was the desktop's CONTENT
 * box, 20px too narrow. The geometry assertions below are derived from the
 * SHIPPED stylesheet, not a private arithmetic oracle, so the helper cannot
 * drift from the rendered island; actual browser rectangles remain runtime
 * evidence owed by the ticket's visual matrix.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

/** The declarations of one top-level rule, matched on the BASE block. */
function cssBlock(selector: string): string {
  const block = css.match(new RegExp(`\\.${selector}\\s*\\{[^}]*\\}`))?.[0];
  expect(block, `.${selector} rule must exist`).toBeDefined();
  return block!;
}

/** A `--rb-*` custom property's px value out of `:root`. */
function rootLength(token: string): number {
  const value = css.match(new RegExp(`\\${token}:\\s*(\\d+(?:\\.\\d+)?)px;`))?.[1];
  expect(value, `${token} must resolve to px`).toBeDefined();
  return Number(value);
}

/** A declaration's px length: either literal, or one `--rb-*` var() resolved. */
function cssLength(block: string, property: string): number {
  const declaration = block.match(new RegExp(`${property}:\\s*([^;]+);`))?.[1]?.trim();
  expect(declaration, `${property} must be declared`).toBeDefined();
  // A unitless `0` is a valid length too (the island's `right: 0`).
  const direct = declaration!.match(/^(\d+(?:\.\d+)?)(?:px)?$/);
  if (direct !== null) {
    return Number(direct[1]);
  }
  const token = declaration!.match(/^var\((--rb-[\w-]+)\)$/)?.[1];
  expect(token, `${property} must be a px literal or a single var()`).toBeDefined();
  return rootLength(token!);
}

/**
 * The island's rendered window-space geometry, derived from the SHIPPED
 * stylesheet: the cluster's `left` and `padding-inline`, the island
 * wrapper's own insets, the 24px `.window-control` and the two gap tokens —
 * the same shrink-to-fit content the browser lays out (three controls on
 * the group/within-group rhythm, the `+`'s 8+24 slot only while mounted,
 * matching `show_plus.then` — shell.rs:4093-4104).
 */
function renderedTitlebarGeometry(showsPlus: boolean): {
  readonly buttons: { readonly left: number; readonly right: number };
  readonly island: { readonly left: number; readonly right: number };
} {
  const cluster = cssBlock("titlebar-cluster");
  const island = cssBlock("titlebar-island");
  const control = cssBlock("window-control");
  const clusterLeft = cssLength(cluster, "left");
  const clusterPad = cssLength(cluster, "padding-inline");
  const controlSize = cssLength(control, "width");
  const controls =
    controlSize * 3 +
    TITLEBAR_GROUP_GAP +
    TITLEBAR_CONTROL_GAP +
    (showsPlus ? TITLEBAR_GROUP_GAP + controlSize : 0);
  // The gap tokens are stylesheet custom properties — the model above must
  // consume the SHIPPED values, not just layout.ts's copy of them.
  expect(rootLength("--rb-titlebar-group-gap")).toBe(TITLEBAR_GROUP_GAP);
  expect(rootLength("--rb-titlebar-control-gap")).toBe(TITLEBAR_CONTROL_GAP);
  const buttonsLeft = clusterLeft + clusterPad;
  const buttonsRight = buttonsLeft + controls;
  return {
    buttons: { left: buttonsLeft, right: buttonsRight },
    island: {
      left: clusterLeft + cssLength(island, "left"),
      right: buttonsRight + clusterPad - cssLength(island, "right"),
    },
  };
}

/**
 * `island_stays_centered_on_controls_while_expanding` (shell.rs:8259-8270),
 * mirrored: the island grows 28→32 across the tween with the padded row's
 * center pinned constant — 4px of air around the 24px controls at full
 * expansion.
 */
describe("island_stays_centered_on_controls_while_expanding", () => {
  const center = (TITLEBAR_HEIGHT + TITLEBAR_TOP_PAD) * 0.5;

  it("the center never moves while the height grows 28→32", () => {
    for (let step = 0; step <= 20; step += 1) {
      const { top, height } = titlebarIslandVerticalGeometry(step / 20);
      expect(top + height * 0.5).toBe(center);
      expect(height).toBeGreaterThanOrEqual(28);
      expect(height).toBeLessThanOrEqual(32);
    }
  });

  it("full expansion centers the 24px controls with 4px of air", () => {
    expect(center).toBe(21);
    const { top, height } = titlebarIslandVerticalGeometry(1);
    expect(center - 12 - top).toBe(4);
    expect(top + height - (center + 12)).toBe(4);
  });

  it("clamps progress outside [0, 1] to the endpoints", () => {
    expect(titlebarIslandVerticalGeometry(-1).height).toBe(28);
    expect(titlebarIslandVerticalGeometry(2).height).toBe(32);
  });
});

/**
 * Tickets 60/66 — the island's horizontal geometry, mirrored from the
 * desktop: `left(6).right_0()` over the cluster (shell.rs:4027-4028)
 * resolving against the PADDING box (taffy 0.12.2 flexbox.rs:2164-2167,
 * 2336-2340) of `left_0()` + `.px(TITLEBAR_CLUSTER_PAD)`
 * (shell.rs:4025-4034), with the `+`'s 32px slot in that box ONLY while the
 * `+` is rendered — `show_plus.then` (shell.rs:4093-4104) reserves no
 * phantom slot at alpha 0. The reported state (sidebar closed on the
 * new-thread canvas) has no `+`, so the island is the desktop's [6, 102],
 * width 96 — ticket 66's correction of the earlier research, which read the
 * content box [10, 92] as the padding box and shipped a 76px island.
 */
describe("titlebarIslandHorizontalGeometry (tickets 60/66 — desktop padding box)", () => {
  it("with the `+` hidden the island spans the desktop's [6, 102]", () => {
    const { left, right } = titlebarIslandHorizontalGeometry(false);
    expect(left).toBe(6);
    expect(right).toBe(102);
    expect(right - left).toBe(96);
  });

  it("with the `+` shown its 32px slot extends the span to [6, 134]", () => {
    const { left, right } = titlebarIslandHorizontalGeometry(true);
    expect(left).toBe(6);
    expect(right).toBe(134);
    expect(right - left).toBe(128);
  });

  it("centers the icons like the desktop: pill center 54 over the [10, 92] controls", () => {
    const { left, right } = titlebarIslandHorizontalGeometry(false);
    // The visible controls — the 24px toggle and the back/forward pair —
    // still span [10, 92] (the desktop's content box; the cluster's padding
    // puts them there, the padding does not move them); the pill's own 6/0
    // insets over the padded cluster put its center 3px right of that box's
    // center, exactly the desktop's relationship (its glyph center 52 under
    // its pill center 54, the research's measurement).
    const controlsCenter = (10 + 92) / 2;
    const pillCenter = (left + right) / 2;
    expect(controlsCenter).toBe(51);
    expect(pillCenter).toBe(54);
    expect(pillCenter - controlsCenter).toBe(3);
  });

  it("the helper mirrors the SHIPPED stylesheet, not a private oracle", () => {
    // The rendered model derives every number from app.css itself (cluster
    // left/padding, island insets, control width, gap tokens), so a CSS
    // regression fails here even if the helper's arithmetic stays put.
    expect(titlebarIslandHorizontalGeometry(false)).toEqual(
      renderedTitlebarGeometry(false).island,
    );
    expect(titlebarIslandHorizontalGeometry(true)).toEqual(
      renderedTitlebarGeometry(true).island,
    );
  });
});

describe("rendered island bounds (ticket 66 — contains the control row)", () => {
  it("without the `+`: island [6, 102] contains the [10, 92] button rectangles", () => {
    const { buttons, island } = renderedTitlebarGeometry(false);
    // The desktop's exact contract (shell.rs:4025-4041)…
    expect(island).toEqual({ left: 6, right: 102 });
    expect(buttons).toEqual({ left: 10, right: 92 });
    // …and the property it exists for: every 24px control — hover boxes
    // included, they ARE the button boxes — sits fully inside the island,
    // with the desktop's 4px of air at the left and 10px at the right.
    expect(island.left).toBeLessThanOrEqual(buttons.left);
    expect(island.right).toBeGreaterThanOrEqual(buttons.right);
  });

  it("with the `+` shown: island [6, 134] contains the [10, 124] control span", () => {
    const { buttons, island } = renderedTitlebarGeometry(true);
    expect(island).toEqual({ left: 6, right: 134 });
    expect(buttons).toEqual({ left: 10, right: 124 });
    expect(island.left).toBeLessThanOrEqual(buttons.left);
    expect(island.right).toBeGreaterThanOrEqual(buttons.right);
  });
});

/**
 * Ticket 64 §2.4 — the island's imperative frame owner. The driver holds
 * the `{from, to, startedAt}` tween ref and writes the scalar per rAF
 * frame; React hears only the mount-gate crossings. These tests run the
 * REAL driver on a manual clock/frame queue and record its paint/gate
 * output — no source greps, no copied arithmetic (the expectations evaluate
 * the shipped `evalWidthTween`).
 */
describe("IslandTweenDriver — the imperative frame owner (ticket 64)", () => {
  function harness(initialTarget: number) {
    let now = 1000;
    const paints: number[] = [];
    const mounts: boolean[] = [];
    let pending: (() => void) | null = null;
    const driver = new IslandTweenDriver(initialTarget, {
      paint: (p) => {
        paints.push(p);
      },
      publishMounted: (mounted) => {
        mounts.push(mounted);
      },
      now: () => now,
      requestFrame: (callback) => {
        pending = callback;
        return 1;
      },
      cancelFrame: () => {
        pending = null;
      },
    });
    return {
      driver,
      paints,
      mounts,
      hasPendingFrame: () => pending !== null,
      /** Advance the clock and run the pending frame, if one is armed. */
      step: (ms: number): void => {
        now += ms;
        const callback = pending;
        pending = null;
        callback?.();
      },
    };
  }

  it("the initial presentation is settled — no entrance animation, the panel only above the gate", () => {
    const shown = harness(1);
    expect(shown.paints).toEqual([1]);
    expect(shown.mounts).toEqual([true]);
    expect(shown.hasPendingFrame()).toBe(false);
    const hidden = harness(0);
    expect(hidden.paints).toEqual([0]);
    expect(hidden.mounts).toEqual([false]);
    expect(hidden.hasPendingFrame()).toBe(false);
  });

  it("a flip tweens over the 200ms RESIZE ease-out and lands the exact endpoint", () => {
    const h = harness(0);
    h.driver.setTarget(1, false);
    expect(h.hasPendingFrame()).toBe(true);
    h.step(50);
    expect(h.paints.at(-1)).toBe(evalWidthTween(0, 1, 50));
    // The gate crossed exactly once, on the first frame above 0.001.
    expect(h.mounts).toEqual([false, true]);
    // Elapsed 200: the endpoint exactly, and the loop dies with it.
    h.step(150);
    expect(h.paints.at(-1)).toBe(1);
    expect(h.hasPendingFrame()).toBe(false);
    h.step(50);
    expect(h.paints).toEqual([0, evalWidthTween(0, 1, 50), 1]);
  });

  it("a mid-glide reversal starts from the PAINTED progress — no endpoint jump", () => {
    const h = harness(0);
    h.driver.setTarget(1, false);
    h.step(100);
    const paintedAtFlip = evalWidthTween(0, 1, 100);
    expect(h.driver.painted).toBe(paintedAtFlip);
    expect(paintedAtFlip).toBeGreaterThan(0.5);
    h.driver.setTarget(0, false);
    // The retarget itself writes nothing; the next frame continues FROM the
    // painted progress (the desktop's painted-value capture, shell.rs:4013-4024).
    expect(h.paints.at(-1)).toBe(paintedAtFlip);
    h.step(16);
    expect(h.paints.at(-1)).toBe(evalWidthTween(paintedAtFlip, 0, 16));
    expect(Math.abs(h.paints.at(-1)! - paintedAtFlip)).toBeLessThan(0.2);
    h.step(200);
    expect(h.paints.at(-1)).toBe(0);
    expect(h.hasPendingFrame()).toBe(false);
    // Two gate crossings over the round trip: up past 0.001, back down.
    expect(h.mounts).toEqual([false, true, false]);
  });

  it("reduced motion settles immediately mid-tween — endpoint, gate, no pending frame", () => {
    const h = harness(0);
    h.driver.setTarget(1, false);
    h.step(100);
    h.driver.setTarget(1, true);
    // The endpoint, synchronously — the tween is cleared with the frame.
    expect(h.paints.at(-1)).toBe(1);
    expect(h.hasPendingFrame()).toBe(false);
    expect(h.mounts).toEqual([false, true]);
    h.step(50);
    expect(h.paints.at(-1)).toBe(1);
  });

  it("the reduced-motion flip works even when the target is unchanged", () => {
    const settled = harness(1);
    settled.driver.setTarget(1, true);
    expect(settled.paints).toEqual([1, 1]);
    expect(settled.mounts).toEqual([true]);
    expect(settled.hasPendingFrame()).toBe(false);
  });

  it("reduced motion with a target flip writes the new endpoint and the gate at once", () => {
    const h = harness(1);
    h.driver.setTarget(0, true);
    expect(h.paints).toEqual([1, 0]);
    expect(h.mounts).toEqual([true, false]);
    expect(h.hasPendingFrame()).toBe(false);
  });

  it("dispose cancels the pending frame — teardown paints nothing more", () => {
    const h = harness(0);
    h.driver.setTarget(1, false);
    h.driver.dispose();
    h.step(50);
    expect(h.paints).toEqual([0]);
  });

  it("a repeated arm toward the running target does not restart the curve", () => {
    const h = harness(0);
    h.driver.setTarget(1, false);
    h.step(100);
    const painted = h.driver.painted;
    h.driver.setTarget(1, false);
    // No re-arm: the running tween is untouched, so no paint and no reset.
    expect(h.paints.at(-1)).toBe(painted);
    h.step(100);
    expect(h.paints.at(-1)).toBe(1);
  });

  it("rides the motion catalog's 200ms resize spec", () => {
    expect(ISLAND_TWEEN_MS).toBe(200);
  });
});

describe("island_target", () => {
  it("is 1 exactly in the reported state — canvas, sidebar collapsed, background resolving", () => {
    expect(
      islandTarget({
        isChatRoute: true,
        hasSelectedChat: false,
        sidebarCollapsed: true,
        backgroundResolves: true,
      }),
    ).toBe(1);
  });

  it("is 0 in every counter-state", () => {
    const canvas = {
      isChatRoute: true,
      hasSelectedChat: false,
      sidebarCollapsed: true,
      backgroundResolves: true,
    };
    // A chat is selected.
    expect(islandTarget({ ...canvas, hasSelectedChat: true })).toBe(0);
    // The sidebar is open.
    expect(islandTarget({ ...canvas, sidebarCollapsed: false })).toBe(0);
    // Settings (or any non-chat route).
    expect(islandTarget({ ...canvas, isChatRoute: false })).toBe(0);
    // The background did not resolve.
    expect(islandTarget({ ...canvas, backgroundResolves: false })).toBe(0);
  });

  it("keys off the RESOLVED background, never the raw setting", () => {
    // A stored-but-undecodable entry falls back to the bundled default
    // (ticket 48's resolution): `backgroundResolves` stays true and the
    // island still shows — the caller passes the resolver's output, so
    // only a resolution failure (false) hides it.
    const gate = {
      isChatRoute: true,
      hasSelectedChat: false,
      sidebarCollapsed: true,
    };
    expect(islandTarget({ ...gate, backgroundResolves: true })).toBe(1);
    expect(islandTarget({ ...gate, backgroundResolves: false })).toBe(0);
  });
});
