import { describe, expect, it } from "vitest";
import {
  AT_BOTTOM_PX,
  STICK_THRESHOLD_PX,
  jumpButtonShown,
  jumpVisibility,
  shouldAnchorLiveStream,
  shouldBreakPin,
  shouldRestick,
} from "../src/lib/stick-spring";

/**
 * The stick decision family — the pure halves of the desktop's
 * `handle_scroll` (crates/ui/src/transcript.rs:3117-3206). The desktop's own
 * `restick_is_direction_aware` (transcript.rs:8270-8281) is ported name for
 * name; `shouldBreakPin` carries the escape rule (transcript.rs:3182-3189)
 * whose web port needed the baseline refresh in `StickController.kick()`.
 */

describe("shouldBreakPin", () => {
  it("a scroll growing past the band breaks the pin", () => {
    // The desktop's rule: distance grew by more than 1px AND is already past
    // AT_BOTTOM_PX — user input moving away from the bottom.
    expect(shouldBreakPin(100, 0)).toBe(true);
    expect(shouldBreakPin(100, 98)).toBe(true);
  });

  it("a no-op scroll attempt at the end keeps the pin", () => {
    expect(shouldBreakPin(0, 0)).toBe(false);
    // Inside AT_BOTTOM_PX (2px): never an escape, however it moved.
    expect(shouldBreakPin(2, 0)).toBe(false);
  });

  it("a small notch inside the band is not an escape", () => {
    // 3px past the bottom, grown by 1px or less: still held.
    expect(shouldBreakPin(3, 2)).toBe(false);
    expect(shouldBreakPin(3, 3)).toBe(false);
  });

  it("growth between scrolls does not read as the next scroll's intent", () => {
    // The trap the baseline refresh exists for (transcript.rs:3534-3540):
    // content grew 500px after the last scroll (baseline refreshed to 500
    // by the kick), and the user then wheels DOWN 10px — distance 490 is
    // BELOW the refreshed baseline, so the pin holds and the spring follows.
    expect(shouldBreakPin(490, 500)).toBe(false);
    // With the stale pre-growth baseline the same scroll read as an escape:
    expect(shouldBreakPin(490, 0)).toBe(true);
  });

  it("uses the shared constants", () => {
    expect(AT_BOTTOM_PX).toBeGreaterThan(0);
    expect(STICK_THRESHOLD_PX).toBeGreaterThan(AT_BOTTOM_PX);
  });
});

describe("shouldRestick (transcript.rs:8270-8281, ported)", () => {
  it("restick_is_direction_aware", () => {
    // Scrolling away from the bottom never resticks, even inside the band.
    expect(shouldRestick(0, 0)).toBe(false);
    expect(shouldRestick(20, 0)).toBe(false);
    expect(shouldRestick(69, 30)).toBe(false);
    // Returning toward the bottom resticks once inside the 70px band…
    expect(shouldRestick(69, 120)).toBe(true);
    expect(shouldRestick(0, 30)).toBe(true);
    // …but never outside it, and never without motion toward it.
    expect(shouldRestick(200, 300)).toBe(false);
    expect(shouldRestick(50, 50)).toBe(false);
  });
});

describe("shouldAnchorLiveStream", () => {
  it("hard-anchors a live stream resting at the end only", () => {
    expect(shouldAnchorLiveStream(true, 0, true)).toBe(true);
    expect(shouldAnchorLiveStream(true, 1, true)).toBe(true);
    // Gliding back toward the bottom (past AT_BOTTOM_PX) keeps the spring.
    expect(shouldAnchorLiveStream(true, 100, true)).toBe(false);
    // Escaped, or no live stream.
    expect(shouldAnchorLiveStream(false, 0, true)).toBe(false);
    expect(shouldAnchorLiveStream(true, 0, false)).toBe(false);
  });
});

describe("jumpVisibility", () => {
  it("offers past the threshold and holds until close to the end", () => {
    expect(jumpVisibility(false, 400)).toBe(true);
    expect(jumpVisibility(true, 100)).toBe(true);
    expect(jumpVisibility(true, 1)).toBe(false);
    expect(jumpVisibility(false, 100)).toBe(false);
  });
});

describe("jumpButtonShown (transcript.rs:3198/:3743-3745, ported)", () => {
  it("hidden while pinned, hidden while the own-turn hold is live, otherwise hysteresis", () => {
    // The desktop's jump_button_stays_available_when_scrolling_down_until_
    // near_bottom (transcript.rs:7748), riding the full gate.
    let shown = false;
    for (const distance of [500, 330, 319, 200, 100]) {
      shown = jumpButtonShown(shown, distance, false, false);
      expect(shown, `button vanished with ${distance}px remaining`).toBe(true);
    }
    expect(jumpButtonShown(shown, AT_BOTTOM_PX, false, false)).toBe(false);
    expect(jumpButtonShown(false, 319, false, false)).toBe(false);
    expect(jumpButtonShown(false, 321, false, false)).toBe(true);
    // Pinned ⇒ hidden (handle_scroll's `&& !this.pinned`, :3198): the pill
    // never flashes while the bottom spring settles near the end.
    expect(jumpButtonShown(true, 100, true, false)).toBe(false);
    // The own-turn hold also suppresses (:3174-3175).
    expect(jumpButtonShown(true, 100, false, true)).toBe(false);
  });
});
