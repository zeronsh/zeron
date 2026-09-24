import { describe, expect, it } from "vitest";
import {
  DEFAULT_GEOMETRY,
  ImageViewState,
  fit,
  fitScale,
  imageOrigin,
  panBy,
  resize,
  zoom,
  type ImageGeometry,
} from "../src/lib/image-geometry";

/** The desktop's `geometry()` fixture: 1000×500 natural, 500×300 viewport. */
function geometry(): ImageGeometry {
  return resize(DEFAULT_GEOMETRY, { width: 1000, height: 500 }, { width: 500, height: 300 });
}

describe("image geometry (image_viewer.rs)", () => {
  it("fit_preserves_aspect_ratio_and_never_upscales", () => {
    let g = geometry();
    expect(g.scale).toBe(0.5);
    expect(imageOrigin(g)).toEqual({ x: 0, y: 25 });
    g = resize(g, { width: 100, height: 50 }, { width: 500, height: 300 });
    expect(g.scale).toBe(1);
    expect(imageOrigin(g)).toEqual({ x: 200, y: 125 });
  });

  it("zoom_keeps_the_cursor_over_the_same_image_point", () => {
    const g = geometry();
    const anchor = { x: 100, y: 150 };
    const originBefore = imageOrigin(g);
    const before = {
      x: (anchor.x - originBefore.x) / g.scale,
      y: (anchor.y - originBefore.y) / g.scale,
    };
    const zoomed = zoom(g, 2, anchor);
    const originAfter = imageOrigin(zoomed);
    const after = {
      x: (anchor.x - originAfter.x) / zoomed.scale,
      y: (anchor.y - originAfter.y) / zoomed.scale,
    };
    expect(Math.abs(before.x - after.x)).toBeLessThan(0.001);
    expect(Math.abs(before.y - after.y)).toBeLessThan(0.001);
  });

  it("pan_zoom_and_resize_stay_bounded", () => {
    let g = geometry();
    g = zoom(g, 1e9, { x: 250, y: 150 });
    expect(g.scale).toBe(32);
    const panned = panBy(g, { x: 1e9, y: -1e9 });
    expect({ x: panned.geometry.pan.x, y: panned.geometry.pan.y }).toEqual({ x: 15750, y: -7850 });
    g = zoom(panned.geometry, 1e-9, { x: 250, y: 150 });
    expect(g.scale).toBe(0.01);
    expect({ x: g.pan.x + 0, y: g.pan.y + 0 }).toEqual({ x: 0, y: 0 });
    const nanZoom = zoom(g, Number.NaN, { x: 0, y: 0 });
    expect(nanZoom.scale).toBe(0.01);
    // A fitted view re-fits on resize; a manual zoom survives and clamps.
    g = fit(nanZoom);
    g = resize(g, { width: 1000, height: 500 }, { width: 250, height: 200 });
    expect(g.scale).toBe(0.25);
    g = zoom(g, 1, { x: 125, y: 100 });
    g = resize(g, { width: 1000, height: 500 }, { width: 300, height: 200 });
    expect(g.scale).toBe(1);
  });

  it("wheel_requires_control_and_normalizes_lines", () => {
    const state = new ImageViewState();
    state.geometry = geometry();
    // Unmodified wheel pans when it can — a fitted image that fits cannot
    // pan, so the event is not consumed; the scale never changes without
    // ctrl either way.
    expect(
      state.wheel({ x: 250, y: 150, deltaX: 0, deltaY: 1, deltaMode: 1, ctrl: false }),
    ).toBe(false);
    expect(state.geometry.scale).toBe(0.5);
    // Ctrl+wheel zooms: wheel up (DOM deltaY < 0) zooms in — the GPUI
    // positive-Y convention with the DOM sign flip.
    expect(
      state.wheel({ x: 250, y: 150, deltaX: 0, deltaY: -1, deltaMode: 1, ctrl: true }),
    ).toBe(true);
    const scale = state.geometry.scale;
    expect(scale).toBeGreaterThan(0.5);
    // Pixel mode at 40px equals one line.
    state.geometry = fit(state.geometry);
    expect(state.geometry.scale).toBe(0.5);
    state.wheel({ x: 250, y: 150, deltaX: 0, deltaY: -40, deltaMode: 0, ctrl: true });
    expect(state.geometry.scale).toBeCloseTo(scale, 5);
    // Wheel down reverses the zoom back to the fit scale.
    state.wheel({ x: 250, y: 150, deltaX: 0, deltaY: 1, deltaMode: 1, ctrl: true });
    expect(Math.abs(state.geometry.scale - 0.5)).toBeLessThan(0.0001);
  });

  it("pinch_accumulates_native_deltas_and_a_drag_does_not_click", () => {
    const state = new ImageViewState();
    state.geometry = geometry();
    state.pinch({ x: 250, y: 150, phase: "started", delta: 0 });
    state.pinch({ x: 250, y: 150, phase: "moved", delta: 0.2 });
    state.pinch({ x: 250, y: 150, phase: "moved", delta: 0.2 });
    expect(Math.abs(state.geometry.scale - 0.7)).toBeLessThan(0.001);
    state.pinch({ x: 250, y: 150, phase: "ended", delta: 0 });
    state.pointerDown({ x: 200, y: 150 });
    expect(state.pointerMove({ x: 202, y: 150 }, true)).toBe(false);
    expect(state.pointerMove({ x: 220, y: 150 }, true)).toBe(true);
    expect(state.dragged).toBe(true);
    state.pointerDown({ x: 200, y: 150 });
    expect(state.dragged).toBe(false);
  });
});
