// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import {
  createHeroBackgroundRenderer,
  type HeroBackgroundRenderer,
} from "../src/lib/new-thread-background-renderer";

/**
 * Ticket 65's production renderer contract, jsdom-honest: jsdom has no
 * WebGL (the capability probe fails), so `createHeroBackgroundRenderer`
 * selects the CPU FALLBACK — today's exact paint path — and flags
 * `kind: "cpu-fallback"` for the component's `data-renderer` attribute
 * (which gates the ticket-57 raster-window CSS). jsdom also has no 2d
 * canvas backend (`getContext("2d")` returns null), so the paint path's
 * early contracts — the raster-width record, the clear paths — are what
 * these tests observe; the numeric mask math is covered by the Stage-A
 * oracle suites (tests/new-thread-background.test.ts) and the GL shader is
 * the validated model they compared.
 */

const HERO = { x: 0, y: 0, width: 1200, height: 600 };
const COMPOSER = { x: 520, y: 400, width: 160, height: 120 };

function makeRenderer(): {
  renderer: HeroBackgroundRenderer;
  reveal: HTMLCanvasElement;
  cutout: HTMLCanvasElement;
  rasterWidths: number[];
} {
  const reveal = document.createElement("canvas");
  const cutout = document.createElement("canvas");
  const rasterWidths: number[] = [];
  const renderer = createHeroBackgroundRenderer(reveal, cutout, {
    setRasterWidth: (cssWidth) => {
      rasterWidths.push(cssWidth);
    },
  });
  return { renderer, reveal, cutout, rasterWidths };
}

describe("createHeroBackgroundRenderer (ticket 65)", () => {
  it("selects the CPU fallback when WebGL is unavailable (jsdom has none)", () => {
    const { renderer } = makeRenderer();
    expect(renderer.kind).toBe("cpu-fallback");
    renderer.dispose();
  });

  it("the CPU paint records the raster window width (ticket 57a's CSS feed)", () => {
    const { renderer, rasterWidths } = makeRenderer();
    renderer.setSource({ drawable: document.createElement("canvas"), width: 1920, height: 1600 });
    renderer.render({ hero: HERO, composer: COMPOSER, dpr: 1 });
    // One record per painted pass (the raster window is the pass's own box).
    expect(rasterWidths).toEqual([1200, 1200]);
    renderer.dispose();
  });

  it("no source (or a zero geometry) clears both canvases — the old clear contract", () => {
    const { renderer, reveal, cutout } = makeRenderer();
    reveal.width = 100;
    cutout.width = 100;
    renderer.setSource(null);
    renderer.render({ hero: HERO, composer: COMPOSER, dpr: 1 });
    expect(reveal.width).toBe(0);
    expect(cutout.width).toBe(0);
    // A zero-width composer (the surface not laid out yet) clears too.
    reveal.width = 100;
    cutout.width = 100;
    renderer.setSource({ drawable: document.createElement("canvas"), width: 10, height: 10 });
    renderer.render({ hero: HERO, composer: { x: 0, y: 0, width: 0, height: 0 }, dpr: 1 });
    expect(reveal.width).toBe(0);
    expect(cutout.width).toBe(0);
    renderer.dispose();
  });

  it("dispose is idempotent and the renders after it are inert", () => {
    const { renderer, rasterWidths } = makeRenderer();
    renderer.setSource({ drawable: document.createElement("canvas"), width: 1920, height: 1600 });
    renderer.render({ hero: HERO, composer: COMPOSER, dpr: 1 });
    renderer.dispose();
    renderer.dispose();
    renderer.setSource({ drawable: document.createElement("canvas"), width: 10, height: 10 });
    renderer.render({ hero: HERO, composer: COMPOSER, dpr: 1 });
    expect(rasterWidths).toEqual([1200, 1200]);
  });

  it("setSource accepts a decoded image's natural size (the `none` effect path)", () => {
    const { renderer } = makeRenderer();
    const image = new Image();
    vi.spyOn(image, "naturalWidth", "get").mockReturnValue(1920);
    vi.spyOn(image, "naturalHeight", "get").mockReturnValue(1600);
    renderer.setSource({ drawable: image, width: 1920, height: 1600 });
    renderer.render({ hero: HERO, composer: COMPOSER, dpr: 2 });
    renderer.dispose();
  });
});
