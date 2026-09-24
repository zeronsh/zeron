import { describe, expect, it } from "vitest";
import {
  BOTTOM_FADE_GRADIENT,
  CUTOUT_REVEAL_OPACITY,
  cutoutBottomFadeAlpha,
  cutoutHoleAlpha,
  cutoutMaskAlpha,
  cutoutMaskRaster,
  decodeBackgroundBlob,
  heroMaskGeometry,
  newThreadBackgroundElementOpacity,
  newThreadBackgroundHeight,
  newThreadBackgroundOpacity,
  NEW_THREAD_BACKGROUND_IDB_PATH,
  Readiness,
  rectEquals,
  resolveActiveNewThreadBackground,
  resolveNewThreadBackground,
  type Rect,
} from "../src/lib/new-thread-background";
import { memoryBackgroundBlobStore } from "../src/lib/background-blob-store";
import { SidebarTweenSignal } from "../src/lib/sidebar-tween";
import {
  computeHeroShaderUniforms,
  HeroRenderScheduler,
  ManualFramePump,
  ProposedDockSignal,
  referenceHeroPixel,
  renderHeroPixel,
  shaderMaskAlpha,
  shaderMaskTerms,
  shaderSourceCoord,
  type AnalyticArtwork,
  type HeroGeometrySample,
  type HeroShaderUniforms,
} from "./helpers/hero-renderer-prototype";

/**
 * The new-thread hero's geometry — each describe named after the
 * `new_thread_background_mask.rs` / `shell.rs` unit test it mirrors. The
 * hero's cutout is what makes the composer read as a window into the
 * artwork rather than a sticker on top of it, and the bottom fade is what
 * dissolves the artwork into the canvas instead of cropping it. The mask
 * ramp cases assert the desktop's per-pixel shader exactly: the
 * smoothstep-over-SDF hole (hard 8px margin, one-sided 120–280px dome)
 * composed with the shared bottom fade by MIN inside one mask.
 */

const HERO: Rect = { x: 224.25, y: 40.5, width: 1000, height: 440 };
const COMPOSER: Rect = { x: 352, y: 406, width: 736, height: 124 };

describe("new_thread_background_height (shell.rs:8265-8281)", () => {
  it("maps the viewport ratio with the 760px ceiling", () => {
    expect(newThreadBackgroundHeight(400)).toBeCloseTo(288, 5);
    expect(newThreadBackgroundHeight(600)).toBeCloseTo(432, 5);
    expect(newThreadBackgroundHeight(1000)).toBeCloseTo(720, 5);
    expect(newThreadBackgroundHeight(1200)).toBe(760);
    // Negative viewports clamp to zero, never a negative height.
    expect(newThreadBackgroundHeight(-100)).toBe(0);
  });
});

describe("new_thread_background_opacity (shell.rs:844-850)", () => {
  it("is 0.84 frosted and 1.0 opaque", () => {
    expect(newThreadBackgroundOpacity(true)).toBe(0.84);
    expect(newThreadBackgroundOpacity(false)).toBe(1);
  });
});

describe("new_thread_background element opacity (shell.rs:880, 5860-5864, 5893)", () => {
  it("is (1 − dissolve) × readiness × surface multiplier, clamped", () => {
    expect(newThreadBackgroundElementOpacity(0, 1, "opaque")).toBe(1);
    expect(newThreadBackgroundElementOpacity(0, 1, "frosted")).toBeCloseTo(0.84, 5);
    expect(newThreadBackgroundElementOpacity(0.5, 1, "opaque")).toBe(0.5);
    expect(newThreadBackgroundElementOpacity(0, 0.25, "frosted")).toBeCloseTo(0.21, 5);
    expect(newThreadBackgroundElementOpacity(0.5, 0.5, "opaque")).toBe(0.25);
    // Dissolve clamps before the multiply.
    expect(newThreadBackgroundElementOpacity(-0.5, 1, "opaque")).toBe(1);
    expect(newThreadBackgroundElementOpacity(1.5, 1, "frosted")).toBe(0);
  });
});

describe("mask_tracks_current_surface_in_window_space_without_rounding (mask.rs:180)", () => {
  it("consumes the composer's exact window-space rect", () => {
    for (const sidebar of [0, 112.25, 224]) {
      for (const rightPanel of [0, 360]) {
        const hero: Rect = { x: sidebar, y: 40, width: 1200 - sidebar, height: 440 };
        const composer: Rect = {
          x: sidebar + 40.5,
          y: 360.25,
          width: 900 - sidebar - rightPanel,
          height: 124,
        };
        const mask = heroMaskGeometry(hero, composer, true);
        expect(rectEquals(mask.bounds, composer)).toBe(true);
        expect(mask.bottomFade.end).toBeCloseTo(480, 5);
        expect(mask.bottomFade.height).toBeCloseTo(440, 5);
        expect(mask.feather).toBeCloseTo(440 * 0.52, 5);
        expect(mask.clearance).toBe(8);
        expect(mask.radius).toBe(26);
      }
    }
  });
});

describe("taller_background_stays_cleared_below_the_composer (mask.rs:201)", () => {
  it("extends the cleared rect to the hero's bottom at the feather ceiling", () => {
    const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    const mask = heroMaskGeometry(hero, composer, true);
    expect(mask.bounds.x).toBe(composer.x);
    expect(mask.bounds.y).toBe(composer.y);
    expect(mask.bounds.width).toBe(composer.width);
    expect(mask.bounds.y + mask.bounds.height).toBeCloseTo(691.2, 5);
    expect(mask.feather).toBe(280);
  });
});

describe("new_thread_cutout_reveal_preserves_the_bottom_fade_and_image_extent (mask.rs:211)", () => {
  it("reveal pass has no hole, the same fade, and a parked exclusion rect", () => {
    const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    const cutout = heroMaskGeometry(hero, composer, true);
    const reveal = heroMaskGeometry(hero, composer, false);
    expect(cutout.bottomFade).toEqual(reveal.bottomFade);
    // The exclusion rect parks entirely below the image.
    expect(reveal.bounds.y - reveal.feather).toBeGreaterThanOrEqual(hero.y + hero.height);
    expect(reveal.radius).toBe(0);
    expect(reveal.clearance).toBe(0);
    expect(CUTOUT_REVEAL_OPACITY).toBe(0.5);
  });
});

describe("new_thread_main_fade_uses_the_full_height_at_every_window_size (mask.rs:224)", () => {
  it("the fade spans the full hero height on both passes", () => {
    for (const height of [288, 489.6, 691.2, 760]) {
      const hero: Rect = { x: 224.25, y: 40.5, width: 1000, height };
      const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
      for (const cutout of [false, true]) {
        const fade = heroMaskGeometry(hero, composer, cutout).bottomFade;
        expect(Math.abs(fade.end - fade.height - hero.y)).toBeLessThan(0.0001);
        expect(fade.end).toBeCloseTo(hero.y + hero.height, 5);
      }
    }
  });
});

describe("background_paint_sees_same_frame_composer_bounds_even_when_painted_first (mask.rs:91)", () => {
  it("the mask raster is a pure function of the CURRENT measured bounds", () => {
    // The web's paint-time contract: the mask grid is a pure function of
    // the CURRENT measured bounds — there is no cached geometry to go
    // stale, so the hero can never read last frame's composer box.
    // (Rastered at quarter resolution for the test's sake; the shader is
    // scale-invariant.)
    const hero: Rect = { x: 40, y: 0, width: 768, height: 440 };
    const at = (x: number, width: number): Rect => ({ x, y: 360.25, width, height: 124 });
    const first = cutoutMaskRaster(hero, at(40, 768), true, 192, 110, 0.25);
    expect(Array.from(first)).not.toEqual(Array.from(cutoutMaskRaster(hero, at(264, 544), true, 192, 110, 0.25)));
    expect(Array.from(cutoutMaskRaster(hero, at(40, 768), true, 192, 110, 0.25))).toEqual(Array.from(first));
    // The hole tracks the composer's x: a pixel inside the old pill reads
    // 0; under the moved pill it is mid-ramp recovery.
    const mask = heroMaskGeometry(hero, at(264, 544), true);
    expect(cutoutMaskAlpha(mask, 44.5, 380.5)).toBeGreaterThan(0);
    expect(cutoutMaskAlpha(heroMaskGeometry(hero, at(40, 768), true), 44.5, 380.5)).toBe(0);
  });
});

describe("the hole ramp (shaders.wgsl image_mask_alpha over mask.rs geometry)", () => {
  // A tall hero (feather pinned at the 280 ceiling) with the composer well
  // inside its y-range, so the SDF at y=500 is purely horizontal distance.
  const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
  const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
  const mask = heroMaskGeometry(hero, composer, true);
  const rightEdge = composer.x + composer.width;
  const atDistance = (d: number, y: number): number => cutoutHoleAlpha(mask, rightEdge + d, y);

  it("alpha is exactly 0 for SDF distance d ≤ 8 (the hard margin, inside included)", () => {
    expect(atDistance(0, 500)).toBe(0);
    expect(atDistance(4, 500)).toBe(0);
    expect(atDistance(8, 500)).toBe(0);
    // Deep inside the cleared rect the alpha is 0 as well.
    expect(cutoutHoleAlpha(mask, 400, 500)).toBe(0);
  });

  it("alpha is 0.5 at d = 8 + feather/2 and exactly 1 at d ≥ 8 + feather", () => {
    expect(mask.feather).toBe(280);
    expect(atDistance(8 + 140, 500)).toBeCloseTo(0.5, 6);
    expect(atDistance(8 + 280, 500)).toBe(1);
    // The ramp's compact support: nothing beyond 8 + feather.
    expect(atDistance(8 + 281, 500)).toBe(1);
    expect(atDistance(8 + 500, 500)).toBe(1);
    // Monotone through the dome.
    expect(atDistance(8 + 70, 500)).toBeGreaterThan(0);
    expect(atDistance(8 + 70, 500)).toBeLessThan(atDistance(8 + 140, 500));
  });

  it("feather clamps at 120/280 for short/tall heroes (mask.rs:36-40)", () => {
    const short = heroMaskGeometry({ x: 0, y: 0, width: 900, height: 200 }, composer, true);
    expect(short.feather).toBe(120);
    const mid = heroMaskGeometry({ x: 0, y: 0, width: 900, height: 500 }, composer, true);
    expect(mid.feather).toBeCloseTo(500 * 0.52, 5);
    const tall = heroMaskGeometry({ x: 0, y: 0, width: 900, height: 760 }, composer, true);
    expect(tall.feather).toBe(280);
  });
});

describe("the combined mask composes by min, not product (shaders.wgsl:1335-1338)", () => {
  it("a pixel where hole and fade are both mid-ramp yields min(hole, fade)", () => {
    const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    const mask = heroMaskGeometry(hero, composer, true);
    // The fade is exactly 0.5 at the hero's mid-height (691.2 − 345.6); the
    // hole is mid-ramp there (the row sits above the composer's top, so the
    // SDF mixes both axes — mid-ramp either way).
    const y = 345.6;
    const x = 1236;
    const hole = cutoutHoleAlpha(mask, x, y);
    const fade = cutoutBottomFadeAlpha(mask, y);
    expect(fade).toBeCloseTo(0.5, 6);
    expect(hole).toBeGreaterThan(0.05);
    expect(hole).toBeLessThan(0.95);
    expect(cutoutMaskAlpha(mask, x, y)).toBeCloseTo(Math.min(hole, fade), 6);
    // min is brighter than the product — element-mask multiplication would
    // darken this pixel to hole × fade.
    expect(cutoutMaskAlpha(mask, x, y)).toBeGreaterThan(hole * fade);
  });
});

describe("cutoutMaskRaster (the shader's per-pixel grid)", () => {
  it("evaluates cutoutMaskAlpha at every raster pixel's window-space center", () => {
    const hero: Rect = { x: 12.5, y: 9.25, width: 90, height: 60 };
    const composer: Rect = { x: 30, y: 40, width: 40, height: 12 };
    for (const cutout of [false, true]) {
      const grid = cutoutMaskRaster(hero, composer, cutout, 45, 30, 0.5);
      expect(grid.length).toBe(45 * 30);
      const mask = heroMaskGeometry(hero, composer, cutout);
      for (let y = 0; y < 30; y++) {
        for (let x = 0; x < 45; x++) {
          expect(grid[y * 45 + x]).toBeCloseTo(
            cutoutMaskAlpha(mask, hero.x + (x + 0.5) / 0.5, hero.y + (y + 0.5) / 0.5),
            6,
          );
        }
      }
    }
  });

  it("device pixels sample the same window-space shader (scale invariance)", () => {
    const hero: Rect = { x: 4, y: 6, width: 60, height: 40 };
    const composer: Rect = { x: 20, y: 30, width: 20, height: 6 };
    const grid = cutoutMaskRaster(hero, composer, true, 120, 80, 2);
    const mask = heroMaskGeometry(hero, composer, true);
    // Raster pixel (60, 40) at scale 2 samples window (4 + 60.5/2, 6 + 40.5/2).
    expect(grid[40 * 120 + 60]).toBeCloseTo(cutoutMaskAlpha(mask, 4 + 60.5 / 2, 6 + 40.5 / 2), 6);
  });

  it("the reveal pass is the fade alone (its exclusion parks below the image)", () => {
    const hero: Rect = { x: 0, y: 0, width: 100, height: 80 };
    const composer: Rect = { x: 30, y: 60, width: 40, height: 10 };
    const grid = cutoutMaskRaster(hero, composer, false, 100, 80);
    const mask = heroMaskGeometry(hero, composer, false);
    for (let y = 0; y < 80; y++) {
      const fade = cutoutBottomFadeAlpha(mask, y + 0.5);
      for (let x = 0; x < 100; x += 7) {
        expect(grid[y * 100 + x]).toBeCloseTo(fade, 6);
      }
    }
  });
});

describe("bottom fade gradient stops (ticket §2.8 CSS mapping)", () => {
  it("approximates smoothstep with quarter-point stops", () => {
    expect(BOTTOM_FADE_GRADIENT).toContain("rgba(0,0,0,0.156) 25%");
    expect(BOTTOM_FADE_GRADIENT).toContain("rgba(0,0,0,0.5) 50%");
    expect(BOTTOM_FADE_GRADIENT).toContain("rgba(0,0,0,0.844) 75%");
  });
});

describe("readiness_opacity (new_thread_background_effects.rs:328-330)", () => {
  it("is exactly 0.5 at 60 ms, restarts only on id change, snaps reduced", () => {
    const readiness = new Readiness();
    // No image: clear state, 0.
    expect(readiness.opacity(null, false, 0)).toBe(0);
    // First sighting stores the clock.
    expect(readiness.opacity("art-1", false, 0)).toBe(0);
    expect(readiness.opacity("art-1", false, 60)).toBe(0.5);
    expect(readiness.opacity("art-1", false, 120)).toBe(1);
    // The same artwork never re-fades.
    expect(readiness.opacity("art-1", false, 5000)).toBe(1);
    // A DIFFERENT id restarts the clock.
    expect(readiness.opacity("art-2", false, 5000)).toBe(0);
    expect(readiness.opacity("art-2", false, 5060)).toBe(0.5);
    // Reduced motion snaps to 1.
    expect(readiness.opacity("art-3", true, 9000)).toBe(1);
    // Clearing again resets.
    expect(readiness.opacity(null, false, 9100)).toBe(0);
    expect(readiness.opacity("art-1", false, 9200)).toBe(0);
  });
});

describe("resolve_new_thread_background (the decode contract)", () => {
  it("falls back to the bundled default when nothing is installed", async () => {
    expect(await resolveNewThreadBackground(null, "/default.png")).toBe("/default.png");
    // A stored path that does not decode (SVG, missing) is rejected.
    expect(await resolveNewThreadBackground({ path: "missing.png", name: "x" }, "/default.png")).toBe(
      "/default.png",
    );
  });

  it("accepts only what actually decodes as a blob", async () => {
    // The positive decode path needs a real bitmap decoder
    // (`createImageBitmap`); jsdom has none, so it only runs in a browser.
    if (typeof createImageBitmap === "function") {
      const png = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])], {
        type: "image/png",
      });
      expect(await decodeBackgroundBlob(png)).toBe(true);
    }
    // A text/SVG blob cannot become a bitmap — staging accepts it as an
    // attachment, the background must not.
    const svg = new Blob(['<svg xmlns="http://www.w3.org/2000/svg"></svg>'], { type: "image/svg+xml" });
    expect(await decodeBackgroundBlob(svg)).toBe(false);
  });
});

describe("resolve_active_new_thread_background (ticket 48)", () => {
  it("resolves the bundled default with name Zeron when nothing is stored", async () => {
    expect(await resolveActiveNewThreadBackground(null, "/default.png")).toEqual({
      url: "/default.png",
      name: "Zeron",
      isDefault: true,
    });
  });

  it("resolves the stored entry while its resource lives, null when it breaks", async () => {
    const blobs = memoryBackgroundBlobStore();
    const setting = { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" };
    // Nothing staged yet: unresolved (null), never silently the default —
    // the Appearance row's "Image unavailable" state.
    expect(await resolveActiveNewThreadBackground(setting, "/default.png", blobs)).toBe(null);
    await blobs.put(new Blob(["bytes"], { type: "image/png" }));
    const resolved = await resolveActiveNewThreadBackground(setting, "/default.png", blobs);
    expect(resolved).toEqual({ url: expect.any(String), name: "wall.png", isDefault: false });
    // A stored entry whose resource is gone is unresolved again.
    await blobs.delete();
    expect(await resolveActiveNewThreadBackground(setting, "/default.png", blobs)).toBe(null);
  });

  it("keeps the painter's default fallback in every state (the thin wrapper)", async () => {
    const blobs = memoryBackgroundBlobStore();
    const setting = { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" };
    await blobs.put(new Blob(["bytes"], { type: "image/png" }));
    expect(await resolveNewThreadBackground(null, "/default.png", blobs)).toBe("/default.png");
    expect(await resolveNewThreadBackground(setting, "/default.png", blobs)).not.toBe("/default.png");
    // A broken stored entry still paints the default (existing behavior —
    // the page-vs-painter split on the broken state).
    await blobs.delete();
    expect(await resolveNewThreadBackground(setting, "/default.png", blobs)).toBe("/default.png");
  });
});

/**
 * Ticket 65 Stage-A (§2.5 gate item 3): the candidate GPU mask renderer's
 * math, prototyped test-scoped in `tests/helpers/hero-renderer-prototype.ts`
 * with the production structure — a uniform bundle precomputed per
 * geometry/source change, a per-pixel window-space fragment evaluation, and
 * the two-pass source-over composite — compared against the production
 * oracle (`heroMaskGeometry` / `cutoutHoleAlpha` / `cutoutBottomFadeAlpha` /
 * `cutoutMaskAlpha` / `cutoutMaskRaster`). The gate's runtime items
 * (matched captures, measured workload) remain PENDING — see
 * `.scratch/web-parity/research-2026-09-20/background-raster-design.md`.
 */

const ORACLE_ARTWORK: AnalyticArtwork = {
  width: 3008,
  height: 2000,
  at: (sx, sy) => ({
    r: (Math.abs(sx) * 3 + Math.abs(sy) * 7) % 251,
    g: (Math.abs(sx) * 5 + Math.abs(sy) * 11) % 239,
    b: (Math.abs(sx) * 13 + Math.abs(sy) * 17) % 223,
  }),
};
const ORACLE_PAGE = { r: 24, g: 24, b: 27 };

const ORACLE_FIXTURES: ReadonlyArray<{ readonly name: string; readonly hero: Rect; readonly composer: Rect }> = [
  {
    name: "default desktop, fractional hero origin",
    hero: { x: 224.25, y: 40.5, width: 1000, height: 440 },
    composer: { x: 352, y: 406, width: 736, height: 124 },
  },
  {
    name: "tall hero at the 280px feather ceiling",
    hero: { x: 0, y: 0, width: 1440, height: 691.2 },
    composer: { x: 352, y: 406, width: 736, height: 124 },
  },
  {
    name: "short hero at the 120px feather floor",
    hero: { x: 0, y: 0, width: 900, height: 200 },
    composer: { x: 100, y: 150, width: 400, height: 60 },
  },
  {
    name: "tiny composer clamps the radius to half its width",
    hero: { x: 0, y: 0, width: 1440, height: 691.2 },
    composer: { x: 500, y: 600, width: 40, height: 30 },
  },
  {
    name: "fractional dimensions throughout, composer past the hero's bottom",
    hero: { x: 10.75, y: 20.5, width: 999.5, height: 440.25 },
    composer: { x: 100.25, y: 380.75, width: 700.5, height: 120.25 },
  },
];

function smoothstepValue(t: number): number {
  return t * t * (3 - 2 * t);
}

/**
 * Probes on the hole ramp with exact expected values: pure horizontal/vertical
 * rays (inside the rect's span, away from the rounded caps, so the SDF is
 * single-axis) plus the 45° corner diagonal (exercising the `hypot` branch).
 */
function holeRampProbes(
  hero: Rect,
  composer: Rect,
): ReadonlyArray<{ readonly name: string; readonly x: number; readonly y: number; readonly hole: number }> {
  const mask = heroMaskGeometry(hero, composer, true);
  const { bounds, feather, clearance } = mask;
  // The radius the shader applies — clamped to half each mask dimension
  // (ImageAlphaMask::scale), which matters for the corner diagonal probe.
  const radius = Math.min(
    Math.max(mask.radius, 0),
    Math.max(bounds.width, 0) * 0.5,
    Math.max(bounds.height, 0) * 0.5,
  );
  const centerX = bounds.x + bounds.width / 2;
  const centerY = bounds.y + bounds.height / 2;
  const right = bounds.x + bounds.width;
  const top = bounds.y;
  const diagonal = (radius + clearance + feather / 2) / Math.SQRT2;
  return [
    { name: "hole center", x: centerX, y: centerY, hole: 0 },
    { name: "inside, short of the edge", x: right - radius - 1, y: centerY, hole: 0 },
    { name: "on the rect edge", x: right, y: centerY, hole: 0 },
    { name: "inside the hard clearance (d = 4)", x: right + 4, y: centerY, hole: 0 },
    { name: "clearance end (d = 8)", x: right + clearance, y: centerY, hole: 0 },
    { name: "feather quarter", x: right + clearance + feather * 0.25, y: centerY, hole: smoothstepValue(0.25) },
    { name: "feather midpoint", x: right + clearance + feather * 0.5, y: centerY, hole: 0.5 },
    { name: "feather three-quarter", x: right + clearance + feather * 0.75, y: centerY, hole: smoothstepValue(0.75) },
    { name: "feather end", x: right + clearance + feather, y: centerY, hole: 1 },
    { name: "past the dome", x: right + clearance + feather + 40, y: centerY, hole: 1 },
    { name: "vertical ray, feather midpoint", x: centerX, y: top - clearance - feather * 0.5, hole: 0.5 },
    { name: "corner diagonal, feather midpoint", x: right - radius + diagonal, y: top + radius - diagonal, hole: 0.5 },
  ];
}

/** Probes along the shared bottom fade with exact smoothstep expectations. */
function fadeProbes(hero: Rect): ReadonlyArray<{ readonly name: string; readonly y: number; readonly fade: number }> {
  const end = hero.y + hero.height;
  return [
    { name: "hero top", y: hero.y, fade: 1 },
    { name: "quarter height", y: hero.y + hero.height * 0.25, fade: smoothstepValue(0.75) },
    { name: "mid height", y: hero.y + hero.height * 0.5, fade: 0.5 },
    { name: "three-quarter height", y: hero.y + hero.height * 0.75, fade: smoothstepValue(0.25) },
    { name: "hero bottom", y: end, fade: 0 },
    { name: "below the hero", y: end + 50, fade: 0 },
  ];
}

function inHero(hero: Rect, x: number, y: number): boolean {
  return x >= hero.x && x <= hero.x + hero.width && y >= hero.y && y <= hero.y + hero.height;
}

describe("ticket 65 stage-A shader model vs the pure-mask oracle (gate item 3)", () => {
  it("the uniform cover-fit reproduces the component's drawImage mapping at every probed point", () => {
    const sources = [
      { width: 3008, height: 2000 }, // landscape photo
      { width: 2000, height: 3008 }, // portrait
      { width: 1600, height: 1600 }, // square
      { width: 640, height: 480 }, // small source, upscaled
    ];
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    for (const { hero } of ORACLE_FIXTURES) {
      for (const source of sources) {
        for (const dpr of [1, 1.5, 2]) {
          const uniforms = computeHeroShaderUniforms(hero, composer, true, source, dpr);
          // The component's device-space cover-fit (new-thread-background.tsx:301-333).
          const canvasWidth = Math.max(1, Math.round(hero.width * dpr));
          const canvasHeight = Math.max(1, Math.round(hero.height * dpr));
          const cover = Math.max(canvasWidth / source.width, canvasHeight / source.height);
          const destX = (canvasWidth - source.width * cover) / 2;
          const destY = (canvasHeight - source.height * cover) / 2;
          for (const [x, y] of [
            [hero.x + 0.5, hero.y + 0.5],
            [hero.x + hero.width - 0.5, hero.y + 0.5],
            [hero.x + hero.width - 0.5, hero.y + hero.height - 0.5],
            [hero.x + 0.5, hero.y + hero.height - 0.5],
            [hero.x + hero.width / 2, hero.y + hero.height / 2],
            [hero.x + hero.width * 0.317, hero.y + hero.height * 0.773],
          ] as const) {
            const expected = {
              sx: ((x - hero.x) * dpr - destX) / cover,
              sy: ((y - hero.y) * dpr - destY) / cover,
            };
            const actual = shaderSourceCoord(uniforms, x, y);
            expect(actual.sx).toBeCloseTo(expected.sx, 8);
            expect(actual.sy).toBeCloseTo(expected.sy, 8);
          }
        }
      }
    }
  });

  it("the fitted image always covers the canvas, exactly on one dimension (cover, never contain)", () => {
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    for (const { hero } of ORACLE_FIXTURES) {
      for (const source of [
        { width: 3008, height: 2000 },
        { width: 2000, height: 3008 },
        { width: 1600, height: 1600 },
      ]) {
        for (const dpr of [1, 2]) {
          const uniforms = computeHeroShaderUniforms(hero, composer, true, source, dpr);
          const coverScale = Math.max(
            uniforms.canvasWidth / source.width,
            uniforms.canvasHeight / source.height,
          );
          expect(source.width * coverScale).toBeGreaterThanOrEqual(uniforms.canvasWidth - 1e-9);
          expect(source.height * coverScale).toBeGreaterThanOrEqual(uniforms.canvasHeight - 1e-9);
          expect(
            Math.min(source.width * coverScale - uniforms.canvasWidth, source.height * coverScale - uniforms.canvasHeight),
          ).toBeCloseTo(0, 9);
          // Centered crop: the fitted origin sits at or before the hero origin.
          expect(uniforms.fittedOriginX).toBeLessThanOrEqual(hero.x + 1e-9);
          expect(uniforms.fittedOriginY).toBeLessThanOrEqual(hero.y + 1e-9);
        }
      }
    }
  });

  it("the hole ramp matches cutoutHoleAlpha at clearance, feather quarter/mid/end, and past the dome", () => {
    for (const { name, hero, composer } of ORACLE_FIXTURES) {
      const oracleMask = heroMaskGeometry(hero, composer, true);
      const uniforms = computeHeroShaderUniforms(hero, composer, true, ORACLE_ARTWORK, 2);
      for (const probe of holeRampProbes(hero, composer)) {
        const terms = shaderMaskTerms(uniforms, probe.x, probe.y);
        expect(terms.hole, `${name} / ${probe.name}`).toBeCloseTo(probe.hole, 6);
        expect(terms.hole, `${name} / ${probe.name}`).toBeCloseTo(
          cutoutHoleAlpha(oracleMask, probe.x, probe.y),
          10,
        );
      }
    }
  });

  it("the bottom fade matches cutoutBottomFadeAlpha across the full height, on both passes", () => {
    for (const { name, hero, composer } of ORACLE_FIXTURES) {
      for (const cutout of [true, false]) {
        const oracleMask = heroMaskGeometry(hero, composer, cutout);
        const uniforms = computeHeroShaderUniforms(hero, composer, cutout, ORACLE_ARTWORK, 1.5);
        for (const probe of fadeProbes(hero)) {
          const terms = shaderMaskTerms(uniforms, hero.x + hero.width / 2, probe.y);
          expect(terms.fade, `${name} / ${probe.name}`).toBeCloseTo(probe.fade, 6);
          expect(terms.fade, `${name} / ${probe.name}`).toBeCloseTo(
            cutoutBottomFadeAlpha(oracleMask, probe.y),
            10,
          );
        }
      }
    }
  });

  it("the composed mask matches cutoutMaskAlpha where the hole and fade ramps overlap", () => {
    for (const { name, hero, composer } of ORACLE_FIXTURES) {
      const oracleMask = heroMaskGeometry(hero, composer, true);
      const uniforms = computeHeroShaderUniforms(hero, composer, true, ORACLE_ARTWORK, 2);
      const { bounds, feather, clearance, radius } = oracleMask;
      const clampedRadius = Math.min(
        Math.max(radius, 0),
        Math.max(bounds.width, 0) * 0.5,
        Math.max(bounds.height, 0) * 0.5,
      );
      const right = bounds.x + bounds.width;
      // Points where BOTH ramps are strictly between 0 and 1: the min is a
      // real choice, and the product hole×fade would be darker.
      const overlap: ReadonlyArray<{ readonly name: string; readonly x: number; readonly y: number }> = [
        {
          name: "hole wins over the fade",
          x: right + clearance + feather * 0.05,
          y: bounds.y + clampedRadius + 1,
        },
        {
          name: "fade wins over the hole",
          x: bounds.x + bounds.width / 2,
          y: hero.y + hero.height * 0.5,
        },
        {
          name: "both mid-ramp at the fade's midpoint",
          x: right + clearance + feather * 0.5,
          y: hero.y + hero.height * 0.5,
        },
        {
          name: "fractional coordinates inside the overlap",
          x: right + clearance + feather * 0.375 + 0.25,
          y: hero.y + hero.height * 0.625 + 0.75,
        },
      ];
      let sawHoleWin = false;
      let sawFadeWin = false;
      for (const probe of overlap) {
        const hole = cutoutHoleAlpha(oracleMask, probe.x, probe.y);
        const fade = cutoutBottomFadeAlpha(oracleMask, probe.y);
        const oracleValue = cutoutMaskAlpha(oracleMask, probe.x, probe.y);
        expect(oracleValue, `${name} / ${probe.name}`).toBeCloseTo(Math.min(hole, fade), 10);
        expect(shaderMaskAlpha(uniforms, probe.x, probe.y), `${name} / ${probe.name}`).toBeCloseTo(
          oracleValue,
          10,
        );
        // The composition is a genuine min — never the darker product.
        if (hole > 0 && hole < 1 && fade > 0 && fade < 1) {
          expect(oracleValue).toBeGreaterThan(hole * fade);
          sawHoleWin ||= hole < fade;
          sawFadeWin ||= fade < hole;
        }
      }
      expect(sawHoleWin && sawFadeWin, `${name}: the min must pick both terms somewhere`).toBe(true);
    }
  });

  it("the reveal pass stays the fade alone (its exclusion parks below the image)", () => {
    for (const { name, hero, composer } of ORACLE_FIXTURES) {
      const oracleMask = heroMaskGeometry(hero, composer, false);
      const uniforms = computeHeroShaderUniforms(hero, composer, false, ORACLE_ARTWORK, 2);
      for (const fx of [0.125, 0.5, 0.875]) {
        for (const fy of [0.125, 0.5, 0.875]) {
          const x = hero.x + hero.width * fx;
          const y = hero.y + hero.height * fy;
          const terms = shaderMaskTerms(uniforms, x, y);
          expect(terms.hole, name).toBe(1);
          expect(terms.mask, name).toBeCloseTo(cutoutMaskAlpha(oracleMask, x, y), 10);
          expect(terms.mask, name).toBeCloseTo(cutoutBottomFadeAlpha(oracleMask, y), 10);
        }
      }
    }
  });

  it("matches cutoutMaskRaster across whole grids at CSS and device scales", () => {
    const fixtures = [ORACLE_FIXTURES[0]!, ORACLE_FIXTURES[4]!];
    for (const { hero, composer } of fixtures) {
      for (const cutout of [true, false]) {
        const uniforms = computeHeroShaderUniforms(hero, composer, cutout, ORACLE_ARTWORK, 1);
        for (const scale of [0.5, 1, 2]) {
          const width = 60;
          const height = 40;
          const grid = cutoutMaskRaster(hero, composer, cutout, width, height, scale);
          for (let y = 0; y < height; y++) {
            for (let x = 0; x < width; x++) {
              const windowX = hero.x + (x + 0.5) / scale;
              const windowY = hero.y + (y + 0.5) / scale;
              // The grid is a Float32Array and the GPU fragment math is
              // single-precision too: equivalence holds within float32
              // rounding (~1e-7), comfortably inside 1e-6.
              expect(shaderMaskAlpha(uniforms, windowX, windowY)).toBeCloseTo(grid[y * width + x]!, 6);
            }
          }
        }
      }
    }
  });

  it("the two-pass source-over composite matches the oracle composite", () => {
    const fixtures = [ORACLE_FIXTURES[0]!, ORACLE_FIXTURES[1]!, ORACLE_FIXTURES[4]!];
    for (const { name, hero, composer } of fixtures) {
      for (const dpr of [1, 2]) {
        const reveal = computeHeroShaderUniforms(hero, composer, false, ORACLE_ARTWORK, dpr);
        const cutout = computeHeroShaderUniforms(hero, composer, true, ORACLE_ARTWORK, dpr);
        const probes = [
          ...holeRampProbes(hero, composer),
          ...fadeProbes(hero).map((probe) => ({
            name: probe.name,
            x: hero.x + hero.width / 2,
            y: probe.y,
          })),
          { name: "hero's top-left corner pixel", x: hero.x + 0.5, y: hero.y + 0.5 },
          { name: "hero's top-right corner pixel", x: hero.x + hero.width - 0.5, y: hero.y + 0.5 },
          { name: "hero's bottom-left corner pixel", x: hero.x + 0.5, y: hero.y + hero.height - 0.5 },
          { name: "fractional interior point", x: hero.x + hero.width * 0.317, y: hero.y + hero.height * 0.773 },
        ];
        for (const probe of probes) {
          if (!inHero(hero, probe.x, probe.y)) {
            continue; // never rasterized — the canvas clips at the hero
          }
          const candidate = renderHeroPixel(reveal, cutout, ORACLE_ARTWORK, ORACLE_PAGE, probe.x, probe.y);
          const reference = referenceHeroPixel(hero, composer, ORACLE_ARTWORK, ORACLE_PAGE, dpr, probe.x, probe.y);
          expect(candidate.r, `${name} / ${probe.name}`).toBeCloseTo(reference.r, 6);
          expect(candidate.g, `${name} / ${probe.name}`).toBeCloseTo(reference.g, 6);
          expect(candidate.b, `${name} / ${probe.name}`).toBeCloseTo(reference.b, 6);
          expect(candidate.a, `${name} / ${probe.name}`).toBeCloseTo(reference.a, 10);
        }
      }
    }
  });

  it("inside the hole the half-strength reveal shows through the cleared cutout", () => {
    const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
    const composer: Rect = { x: 352, y: 406, width: 736, height: 124 };
    const centerX = composer.x + composer.width / 2;
    const centerY = composer.y + composer.height / 2;
    const reveal = computeHeroShaderUniforms(hero, composer, false, ORACLE_ARTWORK, 2);
    const cutout = computeHeroShaderUniforms(hero, composer, true, ORACLE_ARTWORK, 2);
    // The cutout clears the pixel entirely; the reveal carries the fade at 0.5.
    expect(shaderMaskAlpha(cutout, centerX, centerY)).toBe(0);
    const fade = cutoutBottomFadeAlpha(heroMaskGeometry(hero, composer, false), centerY);
    const { sx, sy } = shaderSourceCoord(cutout, centerX, centerY);
    const source = ORACLE_ARTWORK.at(sx, sy);
    const alpha = fade * CUTOUT_REVEAL_OPACITY;
    const pixel = renderHeroPixel(reveal, cutout, ORACLE_ARTWORK, ORACLE_PAGE, centerX, centerY);
    expect(pixel.r).toBeCloseTo(source.r * alpha + ORACLE_PAGE.r * (1 - alpha), 8);
    expect(pixel.g).toBeCloseTo(source.g * alpha + ORACLE_PAGE.g * (1 - alpha), 8);
    expect(pixel.b).toBeCloseTo(source.b * alpha + ORACLE_PAGE.b * (1 - alpha), 8);
  });

  it("growth raster covers both endpoint windows without exposed edges", () => {
    // The sidebar-collapse glide (ticket 57's geometry): the hero widens
    // from 1216 to 1440 while the pill stays centered. The candidate
    // re-renders at the CURRENT width every frame, so every probed hero
    // pixel samples real artwork; the ticket-57 fixed raster window
    // (1216px, centered) leaves the growth band to the page background.
    const source = ORACLE_ARTWORK;
    const heroHeight = 691.2;
    const startWidth = 1216;
    const endWidth = 1440;
    for (let step = 0; step <= 8; step++) {
      const width = startWidth + ((endWidth - startWidth) * step) / 8; // fractional intermediates
      const hero: Rect = { x: 0, y: 0, width, height: heroHeight };
      const composer: Rect = { x: width / 2 - 368, y: 406, width: 736, height: 124 };
      const uniforms = computeHeroShaderUniforms(hero, composer, true, source, 2);
      for (const fx of [0.5, width * 0.25, width / 2, width * 0.75, width - 0.5]) {
        for (const fy of [0.5, heroHeight / 2, heroHeight - 0.5]) {
          const { sx, sy } = shaderSourceCoord(uniforms, fx, fy);
          expect(sx, `width ${width} @ (${fx}, ${fy})`).toBeGreaterThanOrEqual(0);
          expect(sx, `width ${width} @ (${fx}, ${fy})`).toBeLessThanOrEqual(source.width + 1e-6);
          expect(sy, `width ${width} @ (${fx}, ${fy})`).toBeGreaterThanOrEqual(0);
          expect(sy, `width ${width} @ (${fx}, ${fy})`).toBeLessThanOrEqual(source.height + 1e-6);
        }
      }
      // The fixed pre-tween window's exposed band (hero-local): pixels
      // outside [exposedPerSide, width − exposedPerSide] show the page.
      const exposedPerSide = (width - startWidth) / 2;
      if (step === 0) {
        expect(exposedPerSide).toBe(0);
      } else {
        expect(exposedPerSide).toBeGreaterThan(0);
        expect(0.5).toBeLessThan(exposedPerSide + 0.5); // the band covers the hero's edge pixel
      }
    }
    // At the endpoint the band would be 112px per side — ticket 57's
    // sanctioned trade-off, eliminated by the per-frame renderer.
    expect((endWidth - startWidth) / 2).toBe(112);
  });

  it("the hole tracks a transform-only composer move (dock placement frames)", () => {
    const hero: Rect = { x: 0, y: 0, width: 1440, height: 691.2 };
    // The dock pump translates the wrapper; the size is constant, so the
    // ResizeObserver is silent — the renderer must re-sample per frame.
    let previousLeft = 0;
    for (let frame = 0; frame <= 10; frame++) {
      const composer: Rect = { x: 352 + frame * 12.5, y: 406 - frame * 1.25, width: 736, height: 124 };
      const oracleMask = heroMaskGeometry(hero, composer, true);
      const uniforms = computeHeroShaderUniforms(hero, composer, true, ORACLE_ARTWORK, 2);
      const midY = composer.y + 30;
      // The current frame's hole: cleared through the hard margin...
      expect(shaderMaskAlpha(uniforms, composer.x + composer.width / 2, midY)).toBe(0);
      expect(shaderMaskAlpha(uniforms, composer.x - 4, midY)).toBe(0);
      // ...and the oracle agrees at the same current bounds.
      expect(shaderMaskAlpha(uniforms, composer.x - 4, midY)).toBe(
        cutoutMaskAlpha(oracleMask, composer.x - 4, midY),
      );
      if (frame === 10) {
        // A point the glide vacated (the frame-0 left edge) is mid-dome or
        // fully recovered under the frame-10 geometry — never a stale hole.
        const vacated = shaderMaskAlpha(uniforms, 352 - 4, 406 - 1.25 * 10 + 1.25 + 30);
        expect(vacated).toBeGreaterThan(0);
        expect(previousLeft).toBeLessThan(composer.x);
      }
      previousLeft = composer.x;
    }
  });
});

/**
 * Ticket 65 Stage-A (§2.5 gate item 2): the render scheduler prototype
 * composing the sidebar tween signal with the PROPOSED dock subscription —
 * arm/frame/artwork/settle/cancel, driven by a manual frame clock. The
 * sidebar leg uses the REAL `SidebarTweenSignal`; the dock leg stands in
 * for the post-gate `DockGlideSignal` (which gains `subscribe()` only when
 * the gate passes).
 */
describe("ticket 65 stage-A render scheduler prototype (gate item 2)", () => {
  interface Harness {
    readonly sidebar: SidebarTweenSignal;
    readonly dock: ProposedDockSignal;
    readonly pump: ManualFramePump;
    readonly renders: HeroGeometrySample[];
    readonly uploads: () => number;
    readonly setGeometry: (geometry: HeroGeometrySample) => void;
    readonly fireDockFrame: () => void;
    readonly scheduler: HeroRenderScheduler;
  }

  function schedulerHarness(prearm?: (signals: { sidebar: SidebarTweenSignal; dock: ProposedDockSignal }) => void): Harness {
    const sidebar = new SidebarTweenSignal();
    const dock = new ProposedDockSignal();
    prearm?.({ sidebar, dock });
    const pump = new ManualFramePump();
    const renders: HeroGeometrySample[] = [];
    let uploads = 0;
    let dockFrameListener: (() => void) | null = null;
    let current: HeroGeometrySample = {
      hero: { x: 0, y: 0, width: 1440, height: 691.2 },
      composer: { x: 352, y: 406, width: 736, height: 124 },
      dpr: 2,
    };
    const scheduler = new HeroRenderScheduler({
      sidebar,
      dock,
      onDockFrame: (listener) => {
        dockFrameListener = listener;
        return () => {
          if (dockFrameListener === listener) {
            dockFrameListener = null;
          }
        };
      },
      requestFrame: pump.requestFrame,
      cancelFrame: pump.cancelFrame,
      frameId: pump.frameId,
      sample: () => current,
      render: (geometry) => {
        renders.push(geometry);
      },
      uploadSource: () => {
        uploads += 1;
      },
    });
    return {
      sidebar,
      dock,
      pump,
      renders,
      uploads: () => uploads,
      setGeometry: (geometry) => {
        current = geometry;
      },
      fireDockFrame: () => {
        dockFrameListener?.();
      },
      scheduler,
    };
  }

  const centered = (width: number, height = 691.2): HeroGeometrySample => ({
    hero: { x: 0, y: 0, width, height },
    composer: { x: width / 2 - 368, y: 406, width: 736, height: 124 },
    dpr: 2,
  });

  it("sidebar-only glide: notes absorbed, one render per frame, one final paint at settle", () => {
    const h = schedulerHarness();
    h.sidebar.arm();
    const frames = 12; // the 200ms glide at 60Hz
    for (let frame = 1; frame <= frames; frame++) {
      const width = 1216 + ((1440 - 1216) * frame) / frames;
      h.setGeometry(centered(width));
      h.scheduler.noteGeometry(); // the hero's ResizeObserver
      h.scheduler.noteGeometry(); // the composer surface's ResizeObserver
      expect(h.renders).toHaveLength(frame - 1); // absorbed — never synchronous during motion
      h.pump.pump();
      expect(h.renders).toHaveLength(frame); // exactly one render per frame
      expect(h.renders[frame - 1]!.hero.width).toBe(width); // this frame's geometry, never stale
      expect(h.pump.pendingCount).toBe(1); // the renderer's own rAF self-sustains
    }
    // transitionend lands between frames with the geometry already at the
    // endpoint: the final paint coalesces with the last frame's render.
    h.sidebar.settle();
    expect(h.renders).toHaveLength(frames);
    expect(h.pump.pendingCount).toBe(0); // the loop stands down
    // A settle whose geometry advanced past the last sampled frame paints once.
    h.sidebar.arm();
    h.setGeometry(centered(1300));
    h.pump.pump();
    expect(h.renders).toHaveLength(frames + 1);
    h.setGeometry(centered(1216));
    h.sidebar.settle();
    expect(h.renders).toHaveLength(frames + 2);
    expect(h.renders[frames + 1]!.hero.width).toBe(1216);
    // Settled again: plain geometry notes paint (one per frame, coalesced).
    h.scheduler.noteGeometry();
    h.scheduler.noteGeometry();
    expect(h.renders).toHaveLength(frames + 2); // same frame, same geometry
    h.pump.pump();
    h.setGeometry(centered(1200));
    h.scheduler.noteGeometry();
    expect(h.renders).toHaveLength(frames + 3);
  });

  it("dock-only glide: the pump's post-prepaint hook drives renders with zero resize notifications", () => {
    const h = schedulerHarness();
    h.dock.arm();
    const frames = 28; // the ~470ms glide at 60Hz
    for (let frame = 1; frame <= frames; frame++) {
      // Transform-only placement: the composer translates at constant size,
      // so the ResizeObserver never fires (no noteGeometry calls at all).
      h.setGeometry({
        hero: { x: 0, y: 0, width: 1440, height: 691.2 },
        composer: { x: 352 + frame * 4.5, y: 406 - frame * 1.25, width: 736, height: 124 },
        dpr: 2,
      });
      h.pump.pump(); // the dock pump's rAF (its own callbacks)
      h.fireDockFrame(); // writeDockGlideVars, after the transform write
      expect(h.renders).toHaveLength(frame);
      expect(h.renders[frame - 1]!.composer.x).toBe(352 + frame * 4.5);
      expect(h.pump.pendingCount).toBe(0); // the renderer owns no second rAF
    }
    // The converged frame already painted the settled geometry through the
    // hook: the settle edge coalesces to zero extra renders.
    h.dock.settle();
    expect(h.renders).toHaveLength(frames);
    // A teardown settle (no converged frame — the loop stopped mid-flight)
    // still lands exactly one final paint at the measured geometry.
    h.dock.arm();
    h.setGeometry({
      hero: { x: 0, y: 0, width: 1440, height: 691.2 },
      composer: { x: 400, y: 390, width: 736, height: 124 },
      dpr: 2,
    });
    h.pump.pump();
    h.fireDockFrame();
    expect(h.renders).toHaveLength(frames + 1);
    h.setGeometry({
      hero: { x: 0, y: 0, width: 1440, height: 691.2 },
      composer: { x: 418, y: 386, width: 736, height: 124 },
      dpr: 2,
    });
    h.dock.settle();
    expect(h.renders).toHaveLength(frames + 2);
    expect(h.renders[frames + 1]!.composer.x).toBe(418);
  });

  it("overlapping sidebar and dock motion keeps one cadence and settles once", () => {
    const h = schedulerHarness();
    h.sidebar.arm();
    h.dock.arm();
    // Both active: the dock hook owns the cadence; the renderer's rAF stands down.
    for (let frame = 1; frame <= 4; frame++) {
      h.setGeometry(centered(1440 - frame * 10));
      h.scheduler.noteGeometry();
      h.pump.pump();
      h.fireDockFrame();
    }
    expect(h.renders).toHaveLength(4);
    expect(h.pump.pendingCount).toBe(0);
    // The sidebar settling mid-dock releases nothing: the hook cadence continues.
    h.sidebar.settle();
    expect(h.renders).toHaveLength(4);
    for (let frame = 5; frame <= 8; frame++) {
      h.setGeometry(centered(1400 - frame * 10));
      h.pump.pump();
      h.fireDockFrame();
    }
    expect(h.renders).toHaveLength(8);
    // The dock settling while the sidebar runs hands the cadence to the rAF.
    h.sidebar.arm();
    h.dock.settle();
    expect(h.renders).toHaveLength(8); // no final paint — one signal still runs
    expect(h.pump.pendingCount).toBe(1);
    for (let frame = 9; frame <= 10; frame++) {
      h.setGeometry(centered(1350 - frame * 10));
      h.pump.pump();
    }
    expect(h.renders).toHaveLength(10);
    // The last settle coalesces with the final frame's render and stops the loop.
    h.sidebar.settle();
    expect(h.renders).toHaveLength(10);
    expect(h.pump.pendingCount).toBe(0);
  });

  it("a mid-motion reversal re-arms without a final paint until the last settle", () => {
    const h = schedulerHarness();
    h.sidebar.arm();
    for (let frame = 1; frame <= 3; frame++) {
      h.setGeometry(centered(1440 - frame * 20));
      h.pump.pump();
    }
    expect(h.renders).toHaveLength(3);
    // The reversal: the CSS transition retargets from the painted width —
    // the signal re-arms (subscribe fires again) without a final paint.
    h.sidebar.arm();
    expect(h.renders).toHaveLength(3);
    expect(h.pump.pendingCount).toBe(1); // the loop rides on
    for (let frame = 4; frame <= 6; frame++) {
      h.setGeometry(centered(1380 + (frame - 3) * 10));
      h.pump.pump();
    }
    expect(h.renders).toHaveLength(6);
    // The last sampled frame sits short of the endpoint; the settle edge
    // paints the true endpoint exactly once.
    h.setGeometry(centered(1440));
    h.sidebar.settle();
    expect(h.renders).toHaveLength(7);
    expect(h.renders[6]!.hero.width).toBe(1440);
  });

  it("an artwork change mid-motion re-uploads and renders at the current geometry", () => {
    const h = schedulerHarness();
    h.dock.arm();
    h.setGeometry(centered(1440));
    h.pump.pump();
    h.fireDockFrame();
    expect(h.renders).toHaveLength(1);
    // Mid-motion artwork arrival: the source re-uploads and re-renders at
    // once (the raster re-fixes the window), coalescing with the frame.
    h.scheduler.noteArtwork();
    expect(h.uploads()).toBe(1);
    expect(h.renders).toHaveLength(2);
    expect(h.renders[1]!.hero.width).toBe(1440);
    h.fireDockFrame(); // the same frame's hook — geometry unchanged, coalesced
    expect(h.renders).toHaveLength(2);
    h.setGeometry(centered(1430));
    h.pump.pump();
    h.fireDockFrame();
    expect(h.renders).toHaveLength(3);
    h.dock.settle();
    expect(h.renders).toHaveLength(3); // the last hook frame rendered this geometry
    // Settled: an artwork change still re-uploads and paints immediately.
    h.scheduler.noteArtwork();
    expect(h.uploads()).toBe(2);
    expect(h.renders).toHaveLength(4);
  });

  it("settled geometry notes coalesce to one render per frame across both observers", () => {
    const h = schedulerHarness();
    expect(h.pump.pendingCount).toBe(0); // no motion, no loop
    h.scheduler.noteGeometry(); // hero RO
    h.scheduler.noteGeometry(); // composer-surface RO — same frame, same geometry
    expect(h.renders).toHaveLength(1);
    h.pump.pump(); // the next frame
    h.setGeometry(centered(1280));
    h.scheduler.noteGeometry();
    expect(h.renders).toHaveLength(2);
    expect(h.renders[1]!.hero.width).toBe(1280);
  });

  it("dispose cancels the pending frame and every entry point becomes a no-op", () => {
    const h = schedulerHarness();
    h.sidebar.arm();
    h.pump.pump();
    expect(h.renders).toHaveLength(1);
    expect(h.pump.pendingCount).toBe(1);
    h.scheduler.dispose();
    expect(h.pump.pendingCount).toBe(0); // the rAF is cancelled
    h.setGeometry(centered(1300));
    h.pump.pump();
    h.pump.pump();
    expect(h.renders).toHaveLength(1);
    // No edge, note, or dock frame renders after disposal (no stale callbacks).
    h.sidebar.settle();
    h.dock.arm();
    h.fireDockFrame();
    h.dock.settle();
    h.scheduler.noteGeometry();
    h.scheduler.noteArtwork();
    h.scheduler.noteSettle();
    expect(h.renders).toHaveLength(1);
    expect(h.uploads()).toBe(0);
  });

  it("a scheduler created mid-glide picks up the live cadence immediately", () => {
    // The hero can mount while the dock is still dissolving a selection
    // away (heroLayerMounted consumes the live frame) — the scheduler must
    // attach to the running cadence, not wait for a fresh arm.
    const h = schedulerHarness(({ dock }) => dock.arm());
    h.setGeometry(centered(1440));
    h.pump.pump();
    h.fireDockFrame();
    expect(h.renders).toHaveLength(1);
    expect(h.pump.pendingCount).toBe(0); // the dock hook, not a second rAF
    h.dock.settle();
    expect(h.renders).toHaveLength(1); // coalesced with the hook's frame
  });
});
