import { beforeEach, describe, expect, it } from "vitest";
import {
  BAYER,
  GLYPHS,
  __resetEffectRasterCacheForTests,
  asciiPixels,
  coverIndex,
  ditherColor,
  ditherPixels,
  effectRaster,
  effectRasterKey,
  halftonePixels,
  luminanceSource,
  noneRaster,
  prepareNewThreadBackgroundEffects,
  rasterizeEffect,
  scanlinePixels,
  type BackgroundLuminance,
  type EffectRasterDriver,
  type RasterData,
} from "../src/lib/new-thread-background-effects";
import type { NewThreadBackgroundEffect } from "../src/state/ui-settings";

/**
 * The four effect rasters and the (effect, light) cache — each describe
 * named after the `new_thread_background_effects.rs` unit test it mirrors.
 * The rasters are pure per-pixel transforms of the decoded artwork (RGBA in
 * and out — the desktop's BGRA byte order is a RenderImage layout concern
 * that the web deliberately does not port); the cache guarantees ONE
 * pending raster per key, 100 callers or not, with Dither and None shared
 * across appearances.
 *
 * The driver is injected (node has no `fetch`/`createImageBitmap`/worker),
 * so the decode/rasterize seams are fakes over the same fixture the desktop
 * tests use; the browser driver is exercised only in the real app.
 */

const FIXTURE_URL = "fixture://new-thread-background";
const FIXTURE_WIDTH = 60;
const FIXTURE_HEIGHT = 32;

/** effects.rs:375-383 — 60×32, luma 128, warm [128, 64, 32, 200]. */
function fixture(): BackgroundLuminance {
  const pixels = new Uint8Array(FIXTURE_WIDTH * FIXTURE_HEIGHT).fill(128);
  const colors = new Uint8ClampedArray(FIXTURE_WIDTH * FIXTURE_HEIGHT * 4);
  for (let offset = 0; offset < colors.length; offset += 4) {
    colors[offset] = 128;
    colors[offset + 1] = 64;
    colors[offset + 2] = 32;
    colors[offset + 3] = 200;
  }
  return { width: FIXTURE_WIDTH, height: FIXTURE_HEIGHT, pixels, colors };
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

beforeEach(() => {
  __resetEffectRasterCacheForTests();
});

describe("BAYER / GLYPHS (effects.rs:126-137, 201)", () => {
  it("matches the desktop tables verbatim", () => {
    expect(BAYER).toEqual([
      [0, 8, 2, 10],
      [12, 4, 14, 6],
      [3, 11, 1, 9],
      [15, 7, 13, 5],
    ]);
    expect(GLYPHS.length).toBe(10);
    expect(GLYPHS[7]).toEqual([10, 10, 31, 10, 31, 10, 10]);
    expect(GLYPHS[9]).toEqual([14, 17, 23, 21, 23, 16, 14]);
  });
});

describe("dither_color (effects.rs:303-313)", () => {
  it("boosts bright cells to the peak and dims dark cells to 0.08 gain", () => {
    // peak 128 > (0 + 0.5)/16 → bright: gain 255/128.
    expect(ditherColor([128, 64, 32, 200], 0)).toEqual([255, 128, 64, 200]);
    // A low peak stays bright at threshold 0 (gain 255/8).
    expect(ditherColor([8, 4, 2, 255], 0)).toEqual([255, 128, 64, 255]);
    // peak 128 ≤ (15 + 0.5)/16 → dark: gain 0.08.
    expect(ditherColor([128, 64, 32, 200], 15)).toEqual([10, 5, 3, 200]);
    // Alpha is kept regardless of gain.
    expect(ditherColor([0, 0, 0, 200], 0)).toEqual([0, 0, 0, 200]);
  });
});

describe("scanline_pixels (effects.rs:109-122)", () => {
  it("dark gain 0.52 every third row; light lifts toward paper; alpha kept", () => {
    const source = fixture();
    const dark = scanlinePixels(source, false);
    const light = scanlinePixels(source, true);
    // y = 0: gain 0.52 → dark 128·0.52 = 66.56 (truncated), light
    // 128 + 127·0.48 = 188.96 (truncated).
    expect(dark.data[0]).toBe(66);
    expect(light.data[0]).toBe(188);
    // y = 1: gain 1.0 → the source channels unchanged in both themes.
    const rowOne = 4 * FIXTURE_WIDTH;
    expect(dark.data[rowOne]).toBe(128);
    expect(light.data[rowOne]).toBe(128);
  });
});

describe("ascii_pixels (effects.rs:123-164)", () => {
  it("draws paper where the glyph bit is clear and the sampled color where it is set", () => {
    const source = fixture();
    const dark = asciiPixels(source, false);
    // Uniform luma 128 → density 128 → index √(128/255)·9 = 6 (truncated);
    // glyph row 0 of GLYPHS[6] is 0 → pixel (0, 0) is paper:
    // 128·0.60 + 0·0.40 = 76.8 → 76.
    expect(dark.data[0]).toBe(76);
    // Glyph row 5 of GLYPHS[6] is 14 (0b01110): bit (4 − 2) is set → pixel
    // (2, 5) is ink: 128·0.60 + 128·0.40 = 128.
    expect(dark.data[(5 * FIXTURE_WIDTH + 2) * 4]).toBe(128);
    // The spacing column x % 6 == 5 is always paper.
    expect(dark.data[(5 * FIXTURE_WIDTH + 5) * 4]).toBe(76);
  });
});

describe("dither_pixels (effects.rs:200-221)", () => {
  it("fills each 2×2 cell with the quantized cover-center color", () => {
    const source = fixture();
    const raster = ditherPixels(source, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    // Cell (0, 0): BAYER[0][0] = 0 → bright → gain 255/128.
    expect(Array.from(raster.data.slice(0, 8))).toEqual([
      255, 128, 64, 200,
      255, 128, 64, 200,
    ]);
    // Cell (2, 0): BAYER[0][1] = 8 → (8.5)/16 > 128/255 → dark → 0.08 gain.
    expect(Array.from(raster.data.slice(8, 16))).toEqual([
      10, 5, 3, 200,
      10, 5, 3, 200,
    ]);
  });
});

describe("cover_index (effects.rs:227-240)", () => {
  it("is the identity at source scale, and crops/scales like the hero cover fit", () => {
    const source = fixture();
    // Same size: the centered crop is the whole source.
    expect(coverIndex(source, 60, 32, 5, 7)).toBe(7 * 60 + 5);
    // Half-size raster: cover scale 0.5 → the full source is visible, so
    // raster (3, 5) samples source (6, 10).
    expect(coverIndex(source, 30, 16, 3, 5)).toBe(10 * 60 + 6);
    // A narrower raster crops the centered columns: raster (0, 0) samples
    // source x = (60 − 16)/2 = 22.
    expect(coverIndex(source, 16, 32, 0, 0)).toBe(22);
    // Sampling past the edge clamps to the last source pixel.
    expect(coverIndex(source, 30, 16, 29.5, 15.5)).toBe(31 * 60 + 59);
  });
});

describe("light_treatments_use_light_paper_without_inverting_source_hues (effects.rs:423)", () => {
  it("light paper is brighter than dark and the warm source stays warm", () => {
    const source = fixture();
    const brightness = (raster: RasterData): number => {
      let sum = 0;
      for (let offset = 0; offset < raster.data.length; offset += 4) {
        sum += raster.data[offset]! + raster.data[offset + 1]! + raster.data[offset + 2]!;
      }
      return sum;
    };
    const pairs: [RasterData, RasterData][] = [
      [asciiPixels(source, true), asciiPixels(source, false)],
      [scanlinePixels(source, true), scanlinePixels(source, false)],
      [
        halftonePixels(source, FIXTURE_WIDTH, FIXTURE_HEIGHT, true),
        halftonePixels(source, FIXTURE_WIDTH, FIXTURE_HEIGHT, false),
      ],
    ];
    for (const [light, dark] of pairs) {
      expect(brightness(light)).toBeGreaterThan(brightness(dark));
      expect(light.width).toBe(dark.width);
      expect(light.height).toBe(dark.height);
      // RGBA port of the desktop's stored-order assertion: the warm source
      // (r ≥ g ≥ b) remains warm on light paper — hues are not inverted.
      for (let offset = 0; offset < light.data.length; offset += 4) {
        expect(light.data[offset]!).toBeGreaterThanOrEqual(light.data[offset + 1]!);
        expect(light.data[offset + 1]!).toBeGreaterThanOrEqual(light.data[offset + 2]!);
      }
    }
  });
});

describe("raster_treatments_preserve_source_dimensions_and_alpha (effects.rs:485)", () => {
  it("every raster keeps the source dimensions and the source alpha", () => {
    const source = fixture();
    const rasters = [
      ditherPixels(source, FIXTURE_WIDTH, FIXTURE_HEIGHT),
      asciiPixels(source, false),
      scanlinePixels(source, false),
      asciiPixels(source, true),
      scanlinePixels(source, true),
      noneRaster(source),
    ];
    for (const raster of rasters) {
      expect(raster.width).toBe(FIXTURE_WIDTH);
      expect(raster.height).toBe(FIXTURE_HEIGHT);
      for (let offset = 3; offset < raster.data.length; offset += 4) {
        expect(raster.data[offset]).toBe(200);
      }
    }
    const halftone = halftonePixels(source, FIXTURE_WIDTH, FIXTURE_HEIGHT, false);
    expect(halftone.width).toBe(FIXTURE_WIDTH);
    expect(halftone.height).toBe(FIXTURE_HEIGHT);
  });
});

describe("every_effect_is_generated_once_independently_of_viewport (effects.rs:384)", () => {
  it("100 calls while pending yield ONE raster job; warm calls return the same raster", async () => {
    const source = fixture();
    let rasterizeCalls = 0;
    const effects: NewThreadBackgroundEffect[] = ["dither", "ascii", "halftone", "scanlines"];
    for (const [index, effect] of effects.entries()) {
      const gate = deferred();
      const driver: EffectRasterDriver = {
        loadLuminance: async () => source,
        rasterize: async (src, requestedEffect, light) => {
          rasterizeCalls++;
          await gate.promise;
          return rasterizeEffect(src, requestedEffect, light);
        },
      };
      const calls: Promise<RasterData>[] = [];
      for (let i = 0; i < 100; i++) {
        calls.push(effectRaster(FIXTURE_URL, effect, false, driver));
      }
      // Let the decode settle so the single job reaches (and blocks on) the
      // gate — while it is pending, no second rasterize was spawned.
      await new Promise((settle) => setTimeout(settle, 0));
      expect(rasterizeCalls).toBe(index + 1);
      gate.resolve();
      const rasters = await Promise.all(calls);
      // One raster for 100 calls — the memoized promise was the only job.
      expect(rasterizeCalls).toBe(index + 1);
      const first = rasters[0]!;
      for (const raster of rasters) {
        expect(raster).toBe(first);
      }
      expect(first.width).toBe(FIXTURE_WIDTH);
      expect(first.height).toBe(FIXTURE_HEIGHT);
      for (let i = 0; i < 100; i++) {
        expect(await effectRaster(FIXTURE_URL, effect, false, driver)).toBe(first);
      }
    }
    expect(rasterizeCalls).toBe(4);
  });
});

describe("appearance_changes_cache_both_variants_and_share_unchanged_dither (effects.rs:447)", () => {
  it("caches both light and dark rasters; Dither is shared across appearances", async () => {
    const source = fixture();
    const rasterized: { effect: NewThreadBackgroundEffect; light: boolean }[] = [];
    const driver: EffectRasterDriver = {
      loadLuminance: async () => source,
      rasterize: async (src, effect, light) => {
        rasterized.push({ effect, light });
        return rasterizeEffect(src, effect, light);
      },
    };
    for (const effect of ["ascii", "halftone", "scanlines", "dither"] as const) {
      void effectRaster(FIXTURE_URL, effect, false, driver);
      void effectRaster(FIXTURE_URL, effect, true, driver);
    }
    for (const effect of ["ascii", "halftone", "scanlines", "dither"] as const) {
      const dark = await effectRaster(FIXTURE_URL, effect, false, driver);
      const light = await effectRaster(FIXTURE_URL, effect, true, driver);
      expect(dark === light).toBe(effect === "dither");
      for (let i = 0; i < 100; i++) {
        expect(await effectRaster(FIXTURE_URL, effect, false, driver)).toBe(dark);
        expect(await effectRaster(FIXTURE_URL, effect, true, driver)).toBe(light);
      }
    }
    // 7 cache entries: three appearance-dependent effects × 2 + Dither × 1
    // (Dither rasterizes once, with the appearance-independent key).
    expect(rasterized.length).toBe(7);
    expect(rasterized.filter((entry) => entry.effect === "dither")).toEqual([
      { effect: "dither", light: false },
    ]);
  });
});

describe("prewarming_decodes_off_thread_and_reuses_artwork_without_hero_geometry (effects.rs:339)", () => {
  it("prepare reuses the decoded artwork, needs no hero geometry, and stays warm", async () => {
    const source = fixture();
    let loads = 0;
    const driver: EffectRasterDriver = {
      loadLuminance: async () => {
        loads++;
        return source;
      },
      rasterize: async (src) => noneRaster(src),
    };
    // The first prewarm starts the decode; nothing about the hero is passed.
    const pending = prepareNewThreadBackgroundEffects("none", "dark", FIXTURE_URL, driver);
    expect(loads).toBe(1);
    await pending;
    // Twenty more prewarms (either appearance — None is keyed independent)
    // reuse the one decode and the one raster.
    for (let i = 0; i < 20; i++) {
      await prepareNewThreadBackgroundEffects("none", "light", FIXTURE_URL, driver);
    }
    expect(loads).toBe(1);
    // The artwork is reused — the desktop's Arc::ptr_eq, as identity.
    expect(await luminanceSource(FIXTURE_URL, driver)).toBe(source);
    for (let i = 0; i < 20; i++) {
      expect(await luminanceSource(FIXTURE_URL, driver)).toBe(source);
    }
    // The None raster is the source colors (RGBA — the desktop's BGRA swap
    // is layout only; its test asserts the swapped bytes [231, 89, 173,
    // 180] for source [173, 89, 231, 180], the same pixel identity).
    const raster = await prepareNewThreadBackgroundEffects("none", "dark", FIXTURE_URL, driver);
    expect(Array.from(raster.data.slice(0, 4))).toEqual([128, 64, 32, 200]);
  });
});

describe("effectRasterKey (effects.rs:53-61)", () => {
  it("Dither and None are appearance-independent; the others split", () => {
    expect(effectRasterKey("dither", true)).toBe(effectRasterKey("dither", false));
    expect(effectRasterKey("none", true)).toBe(effectRasterKey("none", false));
    expect(effectRasterKey("ascii", true)).not.toBe(effectRasterKey("ascii", false));
    expect(effectRasterKey("halftone", true)).not.toBe(effectRasterKey("halftone", false));
    expect(effectRasterKey("scanlines", true)).not.toBe(effectRasterKey("scanlines", false));
  });
});

describe("a rejected job evicts itself so a retry re-attempts (memoPromise)", () => {
  it("a failed decode leaves no warm entry: the retry re-attempts and lands", async () => {
    const source = fixture();
    let attempts = 0;
    const failing: EffectRasterDriver = {
      loadLuminance: () => {
        attempts += 1;
        return Promise.reject(new Error("offline fetch"));
      },
      rasterize: async (src) => noneRaster(src),
    };
    await expect(luminanceSource(FIXTURE_URL, failing)).rejects.toThrow("offline fetch");

    // The rejection evicted the memo entry — the healed driver is called,
    // not handed the poisoned promise.
    const healed: EffectRasterDriver = {
      loadLuminance: async () => source,
      rasterize: async (src) => noneRaster(src),
    };
    await expect(luminanceSource(FIXTURE_URL, healed)).resolves.toBe(source);
    expect(attempts).toBe(1);
  });

  it("a failed raster evicts its (url, effect, light) entry the same way", async () => {
    const source = fixture();
    let attempts = 0;
    const failing: EffectRasterDriver = {
      loadLuminance: async () => source,
      rasterize: () => {
        attempts += 1;
        return Promise.reject(new Error("raster worker died"));
      },
    };
    await expect(effectRaster(FIXTURE_URL, "ascii", false, failing)).rejects.toThrow("raster worker died");

    const healed: EffectRasterDriver = {
      loadLuminance: async () => source,
      rasterize: async (src, effect, light) => rasterizeEffect(src, effect, light),
    };
    await expect(effectRaster(FIXTURE_URL, "ascii", false, healed)).resolves.toBeDefined();
    expect(attempts).toBe(1);
  });
});
