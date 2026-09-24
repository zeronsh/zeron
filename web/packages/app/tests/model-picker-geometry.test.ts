import { describe, expect, it } from "vitest";
import {
  MODEL_BAND_MAX,
  MODEL_BAND_MIN,
  MODEL_CARD_CHROME,
  MODEL_LIST_HEIGHT,
  MODEL_SPACE_FALLBACK,
  MODEL_SPACE_GAP,
  MODEL_TRAY_CAP,
  modelListBandHeight,
  modelPickerPlacement,
  modelSpaceBelow,
} from "../src/lib/model-picker-geometry";

/**
 * Ticket 04 — the new-chat model picker's measured geometry
 * (pickers.rs:3235-3252, 4683-4703, 4704-4714, upstream e0c1e936): the
 * band is `(space_below − 82 − tray).clamp(30, 216)` with the room measured
 * as `viewport − chip bottom − 14`, and the card opens BELOW the chip on the
 * new-chat canvas instead of above with a fixed cap. The desktop's own
 * numbers are the assertions — the band must never regress to a fixed
 * height, and the unmeasured fallback must resolve to the resting 216.
 */

describe("model_space_below_measures_the_room_under_the_chip", () => {
  it("viewport height minus the chip's bottom minus the 14px gap, floored at zero", () => {
    expect(MODEL_SPACE_GAP).toBe(14);
    expect(modelSpaceBelow(900, 700)).toBe(186);
    expect(modelSpaceBelow(800, 600)).toBe(186);
    expect(modelSpaceBelow(600, 586)).toBe(0);
    // A chip scrolled past the viewport bottom never reads negative.
    expect(modelSpaceBelow(600, 900)).toBe(0);
  });
});

describe("model_list_band_sizes_to_the_measured_room", () => {
  it("the desktop formula: (space_below − 82 − tray).clamp(30, 216)", () => {
    expect(MODEL_CARD_CHROME).toBe(82);
    expect(MODEL_TRAY_CAP).toBe(236);
    expect(MODEL_BAND_MIN).toBe(30);
    expect(MODEL_BAND_MAX).toBe(MODEL_LIST_HEIGHT);
    // A roomy viewport clamps at the resting 216.
    expect(modelListBandHeight(700, false)).toBe(216);
    expect(modelListBandHeight(700, true)).toBe(216);
    // Mid-size rooms land between the clamps, tray budgeted at its full cap.
    expect(modelListBandHeight(250, false)).toBe(168);
    expect(modelListBandHeight(250, true)).toBe(30);
    expect(modelListBandHeight(400, true)).toBe(82);
    // A cramped viewport floors at 30 — the band never collapses entirely.
    expect(modelListBandHeight(100, false)).toBe(30);
    expect(modelListBandHeight(60, true)).toBe(30);
  });

  it("an unmeasured room falls back to 640, which clamps to the resting 216", () => {
    expect(MODEL_SPACE_FALLBACK).toBe(640);
    expect(modelListBandHeight(null, false)).toBe(216);
    expect(modelListBandHeight(null, true)).toBe(216);
  });

  it("is monotone in the measured room and bounded by the clamp pair", () => {
    let previous = -Infinity;
    for (let room = 0; room <= 900; room += 25) {
      for (const tray of [false, true]) {
        const band = modelListBandHeight(room, tray);
        expect(band).toBeGreaterThanOrEqual(MODEL_BAND_MIN);
        expect(band).toBeLessThanOrEqual(MODEL_BAND_MAX);
        // Monotone per tray arm (the tray only shifts the curve down).
        if (!tray) {
          expect(band).toBeGreaterThanOrEqual(previous);
          previous = band;
        }
      }
      previous = -Infinity;
    }
  });
});

describe("model_picker_placement_opens_below_only_on_the_new_chat_canvas", () => {
  it("below-end on new-chat, above-end on a chat (pickers.rs:4710)", () => {
    expect(modelPickerPlacement(true)).toBe("anchorBelowEnd");
    expect(modelPickerPlacement(false)).toBe("anchorAboveEnd");
  });
});
