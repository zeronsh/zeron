import { describe, expect, it } from "vitest";
import { __viewerGeometryForTests } from "../src/components/lightbox";

/**
 * The lightbox's pan/zoom geometry — the web port of `image_viewer.rs`'s
 * `#[cfg(test)]` suite. The pure functions advance a `ViewerGeometry`; the
 * assertions mirror the desktop's exact fixtures.
 */
const { resizeGeometry, zoomAt, panBy, imageOrigin } = __viewerGeometryForTests;

function geometry() {
  return resizeGeometry(
    __viewerGeometryForTests.initialGeometry(),
    { width: 1000, height: 500 },
    { width: 500, height: 300 },
  );
}

describe("lightbox viewer geometry", () => {
  it("fit preserves aspect ratio and never upscales", () => {
    const g = geometry();
    expect(g.scale).toBe(0.5);
    expect(imageOrigin(g)).toEqual({ x: 0, y: 25 });
    const small = resizeGeometry(g, { width: 100, height: 50 }, { width: 500, height: 300 });
    expect(small.scale).toBe(1);
    expect(imageOrigin(small)).toEqual({ x: 200, y: 125 });
  });

  it("zoom keeps the cursor over the same image point", () => {
    const g = geometry();
    const anchor = { x: 100, y: 150 };
    const before = {
      x: (anchor.x - imageOrigin(g).x) / g.scale,
      y: (anchor.y - imageOrigin(g).y) / g.scale,
    };
    const zoomed = zoomAt(g, 2, anchor.x, anchor.y);
    const after = {
      x: (anchor.x - imageOrigin(zoomed).x) / zoomed.scale,
      y: (anchor.y - imageOrigin(zoomed).y) / zoomed.scale,
    };
    expect(Math.abs(before.x - after.x)).toBeLessThan(0.001);
    expect(Math.abs(before.y - after.y)).toBeLessThan(0.001);
  });

  it("pan, zoom and resize stay bounded", () => {
    let g = geometry();
    g = zoomAt(g, 1e9, 250, 150);
    expect(g.scale).toBe(32);
    g = panBy(g, 1e9, -1e9);
    expect({ x: g.panX, y: g.panY }).toEqual({ x: 15750, y: -7850 });
    g = zoomAt(g, 1e-9, 250, 150);
    expect(g.scale).toBe(0.01);
    // `+ 0` normalizes the −0 a clamp at limit 0 can produce.
    expect({ x: g.panX + 0, y: g.panY + 0 }).toEqual({ x: 0, y: 0 });
    g = zoomAt(g, Number.NaN, 0, 0);
    expect(g.scale).toBe(0.01);
    // The desktop sequence: fit, then a resize re-fits (0.25 at the smaller
    // viewport); a zoom then survives a resize with only its pan clamped.
    g = __viewerGeometryForTests.fit(g);
    g = resizeGeometry(g, { width: 1000, height: 500 }, { width: 250, height: 200 });
    expect(g.scale).toBe(0.25);
    g = zoomAt(g, 1, 125, 100);
    g = resizeGeometry(g, { width: 1000, height: 500 }, { width: 300, height: 200 });
    expect(g.scale).toBe(1);
  });
});
