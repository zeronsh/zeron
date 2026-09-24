// @vitest-environment jsdom

/**
 * Round 3, report 1 — the scheduler's DEFAULT frame-clock binding. Every
 * other suite injects `requestFrame`/`cancelFrame` (node has no rAF), so
 * the production wiring — `new HeroRenderScheduler({ sample, render })`
 * with nothing else — never executed under test, and a bare native
 * `requestAnimationFrame` captured into the options object is invoked as a
 * METHOD of that object: the native function demands the window receiver
 * and threw `TypeError: Illegal invocation` straight through the sidebar
 * signal's notify chain (the sidebar-click crash at `z0e.arm`).
 *
 * This suite replaces the window's rAF/cAF with receiver-recording stubs
 * (the transcript-replay idiom), constructs the scheduler through the
 * production wiring alone, arms the page-scoped sidebar signal, and proves
 * the default path requests frames with the WINDOW receiver, renders at
 * the sampled geometry, and tears the frame down on settle. No JSX
 * (createElement), per-file jsdom pragma only.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { dockGlideSignal } from "../src/lib/dock-glide";
import { HeroRenderScheduler, sidebarTweenSignal, type HeroGeometrySample } from "../src/lib/sidebar-tween";

const GEOMETRY: HeroGeometrySample = {
  hero: { x: 0, y: 0, width: 1200, height: 600 },
  composer: { x: 520, y: 400, width: 160, height: 120 },
  dpr: 1,
};

/** One queued frame callback — the scheduler holds at most one pending rAF. */
const queue: FrameRequestCallback[] = [];
/** The receiver each default-path frame request was invoked with. */
const receivers: unknown[] = [];
let priorRaf: typeof window.requestAnimationFrame | undefined;
let priorCaf: typeof window.cancelAnimationFrame | undefined;

beforeAll(() => {
  priorRaf = window.requestAnimationFrame;
  priorCaf = window.cancelAnimationFrame;
  window.requestAnimationFrame = function (this: unknown, callback: FrameRequestCallback): number {
    receivers.push(this);
    queue.push(callback);
    return queue.length;
  };
  window.cancelAnimationFrame = function (handle: number): void {
    queue.splice(handle - 1, 1);
  };
});

afterAll(() => {
  window.requestAnimationFrame = priorRaf as typeof window.requestAnimationFrame;
  window.cancelAnimationFrame = priorCaf as typeof window.cancelAnimationFrame;
});

beforeEach(() => {
  receivers.length = 0;
  queue.length = 0;
});

afterEach(() => {
  sidebarTweenSignal.settle();
  dockGlideSignal.settle();
});

/** Fire every queued frame callback once (callbacks may re-arm). */
function pumpFrames(): void {
  queue.splice(0).forEach((callback) => {
    callback(performance.now());
  });
}

describe("HeroRenderScheduler default frame clock (round-3 crash regression)", () => {
  it("arms, renders, and settles through the default rAF binding with the window receiver", () => {
    const renders: HeroGeometrySample[] = [];
    const scheduler = new HeroRenderScheduler({
      sample: () => GEOMETRY,
      render: (geometry) => {
        renders.push(geometry);
      },
    });
    try {
      // The sidebar flip: the signal's notify chain runs synchronously into
      // the scheduler's edge — the reported crash site. Must not throw, and
      // must land exactly one frame request on the default clock.
      sidebarTweenSignal.arm();
      expect(queue.length).toBe(1);
      // The frame fires: the callback renders at the sampled geometry and
      // re-arms (the tween still runs).
      pumpFrames();
      expect(renders).toEqual([GEOMETRY]);
      expect(queue.length).toBe(1);
      // The settle edge cancels the pending frame through the default
      // cancel binding; the converged geometry is coalesced (same frame,
      // same sample — no second render), and nothing stays queued.
      sidebarTweenSignal.settle();
      expect(queue.length).toBe(0);
      expect(renders).toEqual([GEOMETRY]);
    } finally {
      scheduler.dispose();
    }
    // Every default-path request was invoked with the WINDOW receiver — the
    // "Illegal invocation" regression itself.
    expect(receivers.length).toBeGreaterThan(0);
    expect(receivers.every((receiver) => receiver === window)).toBe(true);
  });
});
