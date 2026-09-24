import { describe, expect, it } from "vitest";
import {
  corridor,
  HoverIntent,
  HOVER_INTENT_GRACE_MS,
  type Bounds,
  type Point,
} from "../src/lib/hover-intent";

/*
 * The submenu hover-intent state machine, ported against the desktop tests
 * it mirrors (popover/hover_intent.rs, upstream f9563394). Names follow the
 * Rust tests one-for-one so the two suites read side by side.
 */

function bounds(left: number, top: number, right: number, bottom: number): Bounds {
  return { left, top, right, bottom };
}

describe("HoverIntent", () => {
  it("diagonal_travel_renews_grace_but_reversing_switches_on_either_side", () => {
    for (const left of [false, true]) {
      const p = (x: number, y: number): Point => ({ x: left ? 400 - x : x, y });
      // p(200,80) sized 80x160: for left it mirrors to 120..200.
      const child = bounds(
        left ? 400 - 280 : 200,
        80,
        left ? 400 - 200 : 280,
        240,
      );
      const intent = new HoverIntent<number>();
      intent.recordOrigin(p(100, 100));
      expect(intent.enter(0, 1, p(150, 140), child, left)).toBe("defer");
      expect(intent.moved(0, 1, p(153, 141), child, left)).toBe("defer");
      expect(intent.moved(0, 1, p(153.5, 141), child, left)).toBe("none");
      expect(intent.moved(0, 1, p(151, 141), child, left)).toBe("open");
      expect(intent.pending()).toBe(null);
    }
  });

  it("leaving_or_dismissing_cancels_pending_switches", () => {
    const intent = new HoverIntent<number>();
    const child = bounds(200, 80, 280, 240);
    const origin: Point = { x: 100, y: 100 };
    const pointer: Point = { x: 150, y: 140 };
    intent.recordOrigin(origin);
    expect(intent.enter(0, 1, pointer, child, false)).toBe("defer");
    intent.leave(2);
    expect(intent.pending()).toBe(1);
    intent.leave(1);
    expect(intent.pending()).toBe(null);
    expect(intent.moved(0, 1, pointer, child, false)).toBe("none");
    intent.enter(0, 1, pointer, child, false);
    intent.reset();
    expect(intent.pending()).toBe(null);
    // A dismissed menu opens the next sibling immediately — no lingering
    // grace after a reset.
    expect(intent.enter(0, 1, pointer, child, false)).toBe("open");
  });

  it("trigger_child_and_corridor_are_safe_but_unrelated_space_is_not", () => {
    const intent = new HoverIntent<number>();
    const trigger = bounds(50, 85, 150, 115);
    const child = bounds(200, 80, 280, 240);
    expect(intent.containsPointer(trigger, child, { x: 100, y: 100 }, false)).toBe(true);
    expect(intent.containsPointer(trigger, child, { x: 175, y: 140 }, false)).toBe(true);
    expect(intent.containsPointer(trigger, child, { x: 240, y: 160 }, false)).toBe(true);
    expect(intent.containsPointer(trigger, child, { x: 175, y: 300 }, false)).toBe(false);
    // A pointer outside the corridor switches immediately on enter.
    expect(intent.enter(0, 1, { x: 100, y: 140 }, child, false)).toBe("open");
  });

  it("no_current_submenu_opens_immediately_and_null_child_bounds_never_dismiss", () => {
    const intent = new HoverIntent<number>();
    const trigger = bounds(50, 85, 150, 115);
    expect(intent.enter(null, 1, { x: 100, y: 100 }, null, false)).toBe("open");
    // containsPointer with no child: only the trigger itself is safe…
    expect(intent.containsPointer(trigger, null, { x: 100, y: 100 }, false)).toBe(true);
    // …but with no child bounds the corridor cannot judge, so it holds
    // (a child that has not laid out yet never dismisses its parent).
    expect(intent.containsPointer(trigger, null, { x: 400, y: 400 }, false)).toBe(true);
  });

  it("the grace period matches the desktop's 300ms timer", () => {
    expect(HOVER_INTENT_GRACE_MS).toBe(300);
  });

  it("corridor_sweeps_the_triangle_from_origin_to_the_child_edge", () => {
    const origin: Point = { x: 100, y: 100 };
    const child = bounds(200, 80, 280, 240);
    // Toward the child, inside the swept triangle.
    expect(corridor(origin, { x: 150, y: 140 }, child, false)).toBe(true);
    // Toward the child but far off its vertical band.
    expect(corridor(origin, { x: 150, y: 10 }, child, false)).toBe(false);
    // Moving away from the child (advance <= 0).
    expect(corridor(origin, { x: 90, y: 140 }, child, false)).toBe(false);
    // Beyond the child's near edge by more than the 8px overshoot.
    expect(corridor(origin, { x: 400, y: 140 }, child, false)).toBe(false);
    // The left side mirrors: edge = the child's right, direction -1.
    const leftChild = bounds(120, 80, 200, 240);
    expect(corridor({ x: 300, y: 100 }, { x: 250, y: 140 }, leftChild, true)).toBe(true);
    expect(corridor({ x: 300, y: 100 }, { x: 310, y: 140 }, leftChild, true)).toBe(false);
  });
});
