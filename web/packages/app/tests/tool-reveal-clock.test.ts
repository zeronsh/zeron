import { describe, expect, it } from "vitest";
import { ToolRevealClock, toolRowRevealProgress } from "../src/lib/tool-motion";

/**
 * Ticket 59 — the shared reveal clock (the audit fix plan's P5). The per-row
 * rAF + `setNow` loops in `ToolGroupRow` hoisted into ONE subscriber-driven
 * clock: the first subscribe arms the loop, the last unsubscribe stops it,
 * every subscriber is handed the same frame timestamp (per-row timings
 * preserved — each row still computes its own progress from `now`), and
 * `prefers-reduced-motion` never rides the loop: a subscribe under reduce
 * delivers one snap tick with no loop armed, and a flip mid-flight ends the
 * loop on the current frame.
 */

interface Harness {
  readonly clock: ToolRevealClock;
  /** The fake clock's current timestamp. */
  readonly now: number;
  /** Scheduled-but-unrun frame callbacks (1 while the loop is armed). */
  readonly pending: number;
  /** Handles handed to cancel (only when an armed loop is stopped). */
  readonly cancels: readonly number[];
  /** Advance the fake clock by one 60fps frame (16ms). */
  readonly advance: () => void;
  /** Execute the oldest scheduled frame callback. */
  readonly runFrame: () => void;
  /** Flip the reduced-motion read. */
  readonly setReduced: (reduced: boolean) => void;
}

function harness(reduced = false): Harness {
  const frames: Array<{ handle: number; callback: () => void }> = [];
  const cancels: number[] = [];
  let time = 1000;
  let reducedNow = reduced;
  let handles = 0;
  const clock = new ToolRevealClock({
    schedule: (callback) => {
      handles += 1;
      frames.push({ handle: handles, callback });
      return handles;
    },
    cancel: (handle) => {
      cancels.push(handle);
      // A real rAF cancel drops the pending callback; an already-executing
      // one (shifted out below) is a no-op.
      const ix = frames.findIndex((frame) => frame.handle === handle);
      if (ix >= 0) {
        frames.splice(ix, 1);
      }
    },
    now: () => time,
    reduced: () => reducedNow,
  });
  return {
    clock,
    get now() {
      return time;
    },
    get pending() {
      return frames.length;
    },
    get cancels() {
      return cancels;
    },
    advance: () => {
      time += 16;
    },
    runFrame: () => {
      const frame = frames.shift();
      if (frame !== undefined) {
        frame.callback();
      }
    },
    setReduced: (value: boolean) => {
      reducedNow = value;
    },
  };
}

describe("ToolRevealClock (ticket 59 — one loop for every row)", () => {
  it("the first subscriber arms one loop; the last unsubscribe stops it", () => {
    const h = harness();
    const seen: number[] = [];
    const stop = h.clock.subscribe((now) => seen.push(now));

    expect(h.clock.isRunning()).toBe(true);
    expect(h.pending).toBe(1);

    h.advance();
    h.runFrame();
    expect(seen).toEqual([1016]);
    // The loop re-arms itself: exactly one pending frame at all times.
    expect(h.pending).toBe(1);

    h.advance();
    h.runFrame();
    expect(seen).toEqual([1016, 1032]);

    stop();
    expect(h.clock.isRunning()).toBe(false);
    expect(h.cancels).toHaveLength(1);

    // No subscriber, no loop: running frames delivers nothing more.
    h.advance();
    h.runFrame();
    expect(seen).toEqual([1016, 1032]);
  });

  it("every row receives the SAME frame timestamp — one schedule, not one per row", () => {
    const h = harness();
    const a: number[] = [];
    const b: number[] = [];
    const stopA = h.clock.subscribe((now) => a.push(now));
    const stopB = h.clock.subscribe((now) => b.push(now));

    // A second row joins the live loop — it never re-arms a second one.
    expect(h.pending).toBe(1);

    h.advance();
    h.runFrame();
    expect(a).toEqual([1016]);
    expect(b).toEqual([1016]);

    // One row finishing its motion leaves the loop alive for the other.
    stopA();
    expect(h.clock.isRunning()).toBe(true);
    h.advance();
    h.runFrame();
    expect(a).toEqual([1016]);
    expect(b).toEqual([1016, 1032]);

    stopB();
    expect(h.clock.isRunning()).toBe(false);
  });

  it("per-row timings are preserved — the row sees the timestamps a per-row loop would sample", () => {
    const h = harness();
    const shared: number[] = [];
    const stop = h.clock.subscribe((now) => shared.push(now));

    // What the OLD per-row loop delivered: `performance.now()` sampled inside
    // its own rAF callback, one per frame. The shared clock hands every row
    // those exact timestamps.
    const perRow: number[] = [];
    for (let frame = 0; frame < 6; frame += 1) {
      h.advance();
      perRow.push(h.now);
      h.runFrame();
    }
    stop();
    expect(shared).toEqual(perRow);

    // The row's progress math is unchanged with those timestamps: the reveal
    // climbs monotonically and saturates past TOOL_ROW_REVEAL_MS.
    const start = 1000;
    const progress = shared.map((now) => toolRowRevealProgress(start, now, false));
    expect(progress[0]).toBeGreaterThan(0);
    for (let ix = 1; ix < progress.length; ix += 1) {
      expect(progress[ix]).toBeGreaterThanOrEqual(progress[ix - 1]!);
    }
    expect(toolRowRevealProgress(start, start + 360, false)).toBe(1);
  });

  it("a listener unsubscribing mid-dispatch stops the clock only when it was the last", () => {
    const h = harness();
    const seenB: number[] = [];
    // This row leaves on its first tick (its motion finished).
    const stopA = h.clock.subscribe(() => {
      stopA();
    });
    const stopB = h.clock.subscribe((now) => seenB.push(now));

    h.advance();
    h.runFrame();
    expect(seenB).toEqual([1016]);
    expect(h.clock.isRunning()).toBe(true);
    stopB();
    expect(h.clock.isRunning()).toBe(false);

    // The self-unsubscribing row alone: the dispatch itself stops the loop
    // before the next frame is ever scheduled.
    const h2 = harness();
    const stop = h2.clock.subscribe(() => {
      stop();
    });
    expect(h2.pending).toBe(1);
    h2.advance();
    h2.runFrame();
    expect(h2.clock.isRunning()).toBe(false);
    expect(h2.pending).toBe(0);
  });

  it("reduced motion at subscribe: ONE snap tick, no loop ever armed", () => {
    const h = harness(true);
    const seen: number[] = [];
    const stop = h.clock.subscribe((now) => seen.push(now));

    // The snap: one synchronous tick (the row renders its endpoint and
    // unsubscribes on its own) — never a scheduled frame.
    expect(seen).toEqual([1000]);
    expect(h.pending).toBe(0);
    expect(h.clock.isRunning()).toBe(false);

    stop();
    // Nothing was armed, so nothing is cancelled.
    expect(h.cancels).toHaveLength(0);
  });

  it("a reduced-motion flip mid-flight ends the loop on the current frame", () => {
    const h = harness();
    const seen: number[] = [];
    const stop = h.clock.subscribe((now) => seen.push(now));

    expect(h.pending).toBe(1);
    h.setReduced(true);
    h.advance();
    h.runFrame();

    // The flip frame is delivered (the row snaps to its endpoint on it) and
    // is the last: no next frame is scheduled, and the already-consumed
    // callback leaves nothing to cancel.
    expect(seen).toEqual([1016]);
    expect(h.clock.isRunning()).toBe(false);
    expect(h.pending).toBe(0);

    stop();
    expect(h.cancels).toHaveLength(0);
  });

  it("re-subscribing after a stop re-arms the loop", () => {
    const h = harness();
    const first = h.clock.subscribe(() => {});
    first();
    expect(h.clock.isRunning()).toBe(false);
    expect(h.cancels).toHaveLength(1);

    const seen: number[] = [];
    const stop = h.clock.subscribe((now) => seen.push(now));
    expect(h.clock.isRunning()).toBe(true);
    expect(h.pending).toBe(1);
    h.advance();
    h.runFrame();
    expect(seen).toEqual([1016]);
    stop();
  });
});
