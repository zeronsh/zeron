import { describe, expect, it } from "vitest";
import { motion } from "@zeron/theme";
import { cubicBezierEval } from "../src/state/layout";
import type { MessagePart, SessionMessageEntry } from "@zeron/proto";
import {
  GlideTimeline,
  SCROLL_GLIDE_CURVE,
  SCROLL_GLIDE_MS,
  activeTick,
  bucketOf,
  railCapacity,
  railSlots,
  railTicks,
  railVisible,
  tickBuckets,
  truncatePreview,
} from "../src/lib/rail";

function entry(id: string, role: SessionMessageEntry["role"], text: string): SessionMessageEntry {
  return {
    id,
    role,
    parts: [{ kind: "text", id: "t0", text } as MessagePart],
    createdAt: 0,
    deviceId: "d",
    status: "complete",
  };
}

// ---------------------------------------------------------------------------
// Fixed footprint (rail.rs tests:596-648)
// ---------------------------------------------------------------------------

describe("rail pure logic", () => {
  it("capacity_counts_slots_that_fit", () => {
    // 880px viewport − 48 margin = 832 usable → (832+3)/13 = 64 slots.
    expect(railCapacity(880)).toBe(64);
    // Tiny (or unmeasured) heights still hand out one slot.
    expect(railCapacity(0)).toBe(1);
    expect(railCapacity(200)).toBeGreaterThanOrEqual(10);
    // The rail itself is hard-capped: compact on any window height.
    expect(railSlots(880)).toBe(12);
    expect(railSlots(2000)).toBe(12);
    // Short rails still shrink below the cap.
    expect(railSlots(100)).toBeLessThan(12);
  });

  it("buckets_are_identity_under_capacity", () => {
    // n <= capacity: one tick per prompt — the old per-prompt rail.
    const b = tickBuckets(5, 64);
    expect(b.length).toBe(5);
    expect(b.every((range, k) => range[0] === k && range[1] === k + 1)).toBe(true);
  });

  it("buckets_partition_evenly_over_capacity", () => {
    // 100 prompts into 8 slots: every tick in exactly one bucket, in order,
    // first starts at 0, last ends at n, sizes within ±1 of even.
    const n = 100;
    const b = tickBuckets(n, 8);
    expect(b.length).toBe(8);
    expect(b[0]![0]).toBe(0);
    expect(b[b.length - 1]![1]).toBe(n);
    for (let k = 0; k + 1 < b.length; k++) {
      expect(b[k]![1]).toBe(b[k + 1]![0]);
    }
    for (const [s, e] of b) {
      expect(e - s === 12 || e - s === 13).toBe(true);
    }
  });

  it("bucket_of_maps_ticks_to_their_bucket", () => {
    const b = tickBuckets(10, 3); // [0,3) [3,6) [6,10)
    expect(bucketOf(b, 0)).toBe(0);
    expect(bucketOf(b, 3)).toBe(1);
    expect(bucketOf(b, 9)).toBe(2);
    expect(bucketOf(b, 10)).toBeNull();
    // Degenerate inputs.
    expect(tickBuckets(0, 8)).toEqual([]);
    expect(tickBuckets(3, 0)).toEqual([[0, 3]]);
  });

  it("ticks_map_user_prompts_with_reply_openings", () => {
    const entries = [
      entry("u1", "user", "first question"),
      entry("a1", "assistant", "first answer"),
      entry("u2", "user", "second question"),
      entry("a2", "assistant", "second answer"),
    ];
    const ticks = railTicks(entries, []);
    expect(ticks.length).toBe(2);
    expect(ticks[0]!.messageId).toBe("u1");
    expect(ticks[0]!.prompt).toBe("first question");
    expect(ticks[0]!.reply).toBe("first answer");
    expect(ticks[1]!.reply).toBe("second answer");
  });

  it("ticks_include_echoes_deduped", () => {
    const entries = [entry("u1", "user", "sent")];
    const echoes = [
      entry("u1", "user", "sent"), // confirmed already → deduped
      entry("u2", "user", "pending"),
    ];
    const ticks = railTicks(entries, echoes);
    expect(ticks.length).toBe(2);
    expect(ticks[1]!.messageId).toBe("u2");
    expect(ticks[1]!.reply).toBeNull();
  });

  it("tick_without_reply_yet", () => {
    const entries = [
      entry("u1", "user", "q"),
      entry("a1", "assistant", "reply to first"),
      entry("u2", "user", "latest"),
    ];
    const ticks = railTicks(entries, []);
    // The last prompt has no assistant entry after it.
    expect(ticks[1]!.reply).toBeNull();
    // Empty transcript → no ticks.
    expect(railTicks([], [])).toEqual([]);
  });

  it("active_tick_tracks_viewport_top", () => {
    const tickRows = [0, 5, 9];
    expect(activeTick(tickRows, 0)).toBe(0);
    expect(activeTick(tickRows, 4)).toBe(0);
    expect(activeTick(tickRows, 5)).toBe(1);
    expect(activeTick(tickRows, 8)).toBe(1);
    expect(activeTick(tickRows, 100)).toBe(2);
    // Above the first tick row → first tick still active.
    expect(activeTick([3, 7], 1)).toBe(0);
    expect(activeTick([], 4)).toBeNull();
  });

  it("rail_width_gate", () => {
    expect(railVisible(768)).toBe(true);
    expect(railVisible(1200)).toBe(true);
    expect(railVisible(767.9)).toBe(false);
    expect(railVisible(0)).toBe(false);
  });

  it("preview_truncation", () => {
    expect(truncatePreview("short", 10)).toBe("short");
    expect(truncatePreview("  padded  ", 10)).toBe("padded");
    const long = "x".repeat(50);
    const cut = truncatePreview(long, 10);
    expect([...cut].length).toBeLessThanOrEqual(10);
    expect(cut.endsWith("…")).toBe(true);
    // Multi-byte safety.
    const uni = "héllo wörld attaché case overflowing";
    const uniCut = truncatePreview(uni, 12);
    expect(uniCut.endsWith("…")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// GlideTimeline (rail.rs tests:714-788)
// ---------------------------------------------------------------------------

describe("GlideTimeline", () => {
  /** Consuming `(e'−e)/(1−e)` of the current remainder telescopes to exactly
   *  the absolute eased timeline `start + e(t)·total` when the distance
   *  estimate is stable — the glide is timeline-driven, not
   *  percent-of-remaining. */
  it("glide_timeline_matches_absolute_eased_interpolation", () => {
    const timeline = new GlideTimeline();
    const start = 1000;
    const target = 0;
    let pos = start;
    for (let i = 1; i <= 60; i++) {
      const t = i / 60;
      const eased = cubicBezierEval(SCROLL_GLIDE_CURVE, t);
      const frac = timeline.step(eased);
      pos -= frac * (pos - target);
      const absolute = start + eased * (target - start);
      expect(Math.abs(pos - absolute)).toBeLessThan(0.05);
    }
    expect(pos).toBe(target); // eased hits 1.0 → frac 1.0 → exact landing.
  });

  /** A mid-flight distance re-estimate (anchor re-glued / row measured)
   *  continues the SAME timeline over the corrected remainder: no restart,
   *  no compensating jump, exact landing. */
  it("glide_timeline_survives_remaining_distance_reestimate", () => {
    const timeline = new GlideTimeline();
    let pos = 500;
    let prevFrac = 0;
    for (let i = 1; i <= 60; i++) {
      const t = i / 60;
      const frac = timeline.step(cubicBezierEval(SCROLL_GLIDE_CURVE, t));
      if (i === 30) {
        // The layout re-glued the anchor: remaining distance doubles.
        pos *= 2;
      }
      pos -= frac * pos;
      // Fractions depend only on the timeline — the re-estimate cannot make
      // a step consume a larger share than the curve dictates.
      expect(frac).toBeGreaterThanOrEqual(0);
      expect(frac).toBeLessThanOrEqual(1);
      if (i > 1 && i < 55) {
        expect(frac).toBeGreaterThanOrEqual(prevFrac - 0.05);
      }
      prevFrac = frac;
    }
    expect(pos).toBe(0);
  });

  /** Timeline steps clamp: regressions in eased input yield zero movement,
   *  and completion always yields the full remainder. */
  it("glide_timeline_step_clamps", () => {
    const timeline = new GlideTimeline();
    expect(timeline.step(0.4)).toBeCloseTo(0.4);
    expect(timeline.step(0.3)).toBe(0); // non-monotone input → no move
    expect(timeline.step(1.0)).toBe(1); // done → land exactly
    expect(timeline.step(1.0)).toBe(1); // idempotent at the end
  });

  /** The first 16ms frame of the 500ms glide covers under 2% of the
   *  distance — no first-frame majority jump by construction. */
  it("glide_first_frame_is_gentle", () => {
    const spec = motion.specs.find((candidate) => candidate.name === "scrollGlide");
    expect(spec?.durationMs ?? SCROLL_GLIDE_MS).toBe(500);
    const first = cubicBezierEval(SCROLL_GLIDE_CURVE, 16 / 500);
    expect(first).toBeLessThan(0.02);
    // And the ease-in-out midpoint is exactly half the distance.
    const mid = cubicBezierEval(SCROLL_GLIDE_CURVE, 0.5);
    expect(Math.abs(mid - 0.5)).toBeLessThan(0.01);
  });
});
