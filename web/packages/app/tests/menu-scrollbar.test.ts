/**
 * The floating menu scrollbar's visibility model (lib/menu-scrollbar.ts) —
 * the web peer of the desktop `MenuScrollbarState` unit tests
 * (popover.rs): show on scroll MOTION or track-hover/drag, linger 1400ms,
 * fade 260ms, freeze under the pointer.
 */

import { describe, expect, it } from "vitest";

import {
  MENU_SCROLLBAR_FADE_MS,
  MENU_SCROLLBAR_LINGER_MS,
  clearScrollBaseline,
  createMenuScrollbarVisibility,
  nextWakeMs,
  noteScrollOffset,
  railActive,
  railFade,
  railVisible,
  setBarHovered,
  setGrabbing,
  setListHovered,
} from "../src/lib/menu-scrollbar";

describe("menu scrollbar visibility", () => {
  it("shows on scroll motion, not on the first observation", () => {
    const state = createMenuScrollbarVisibility();
    expect(noteScrollOffset(state, 0, 1000)).toBe(false);
    expect(railVisible(state, 1000)).toBe(false);
    expect(noteScrollOffset(state, 24, 1100)).toBe(true);
    expect(railVisible(state, 1100)).toBe(true);
    expect(railFade(state, 1100)).toBe(1);
  });

  it("lingers 1400ms then fades over 260ms", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    noteScrollOffset(state, 50, 1000);
    // Full through the linger window.
    expect(railFade(state, 1000 + MENU_SCROLLBAR_LINGER_MS - 1)).toBe(1);
    // Half-way through the fade.
    const midFade = railFade(state, 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS / 2);
    expect(midFade).toBeGreaterThan(0.3);
    expect(midFade).toBeLessThan(0.7);
    // Gone past the window.
    expect(railVisible(state, 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1)).toBe(
      false,
    );
    expect(railFade(state, 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1)).toBe(0);
  });

  it("hovering the list alone shows nothing", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    setListHovered(state, true, 100);
    expect(railVisible(state, 200)).toBe(false);
  });

  it("a track-hover holds the rail open past the fade window", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    noteScrollOffset(state, 50, 1000);
    setBarHovered(state, true, 1500);
    expect(
      railVisible(state, 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 5000),
    ).toBe(true);
    expect(railFade(state, 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 5000)).toBe(
      1,
    );
    // Leaving the track restarts the wait — it hides a beat later, not now.
    const leave = 1000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 5000;
    setBarHovered(state, false, leave);
    expect(railVisible(state, leave + 100)).toBe(true);
    expect(railVisible(state, leave + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1)).toBe(
      false,
    );
  });

  it("leaving the list straight off the track restarts the linger", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    noteScrollOffset(state, 10, 100);
    setListHovered(state, true, 200);
    setBarHovered(state, true, 300);
    setListHovered(state, false, 4000);
    expect(state.barHovered).toBe(false);
    expect(railVisible(state, 4200)).toBe(true);
  });

  it("a drag holds the rail open and its release lingers like a stopped scroll", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    setGrabbing(state, true, 1000);
    expect(railActive(state)).toBe(true);
    expect(railVisible(state, 9000)).toBe(true);
    setGrabbing(state, false, 9000);
    expect(railActive(state)).toBe(false);
    expect(railVisible(state, 9000 + 100)).toBe(true);
    expect(railVisible(state, 9000 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1)).toBe(
      false,
    );
  });

  it("clearScrollBaseline rewinds without marking motion", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    noteScrollOffset(state, 80, 1000);
    clearScrollBaseline(state);
    expect(railVisible(state, 1001)).toBe(false);
    expect(noteScrollOffset(state, 160, 2000)).toBe(false);
    expect(railVisible(state, 2000)).toBe(false);
  });

  it("nextWakeMs steps from the linger end through the fade", () => {
    const state = createMenuScrollbarVisibility();
    noteScrollOffset(state, 0, 0);
    noteScrollOffset(state, 50, 1000);
    // Mid-linger: the wake lands as the fade starts.
    expect(nextWakeMs(state, 1500)).toBe(900);
    // Inside the fade: frame steps.
    expect(nextWakeMs(state, 1000 + MENU_SCROLLBAR_LINGER_MS + 10)).toBe(16);
    // Held open or fully elapsed: no wake.
    setBarHovered(state, true, 2500);
    expect(nextWakeMs(state, 2500)).toBeNull();
    setBarHovered(state, false, 2600);
    expect(
      nextWakeMs(state, 2600 + MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1),
    ).toBeNull();
  });
});
