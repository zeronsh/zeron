/**
 * The new-thread background EFFECTS — the web port of
 * `crates/ui/src/new_thread_background_effects.rs`: four static per-pixel
 * rasters (dither, ascii, halftone, scanlines) computed from the ≤2048px
 * decoded artwork, plus the `(effect, light)` raster cache with ONE pending
 * job per key and the hero-geometry-free prewarm entry.
 *
 * The pure half (this file's raster functions) is plain ImageData-shaped
 * math — RGBA in, RGBA out — so it runs in unit tests and in the worker
 * alike; the desktop's BGRA channel order is a RenderImage layout concern
 * and is deliberately NOT ported. The driver half decodes via ticket 15's
 * `createImageBitmap` contract (thumbnail 2048, luma8 + rgba8) and
 * rasterizes on a dedicated worker — the peer of the desktop's background
 * executor + `cx.refresh_windows()`.
 *
 * The effects are STATIC transforms of the artwork: there is no animation
 * here in either implementation, and none may be invented.
 */

import type { Appearance } from "@zeron/theme";
import type { NewThreadBackgroundEffect } from "../state/ui-settings";

// ---------------------------------------------------------------------------
// The source and raster contracts
// ---------------------------------------------------------------------------

/**
 * The decoded artwork the rasters sample (effects.rs `BackgroundLuminance`):
 * luma8 for the density terms, rgba8 for the color terms — RGBA order, the
 * canvas ImageData layout (NOT the desktop's BGRA).
 */
export interface BackgroundLuminance {
  readonly width: number;
  readonly height: number;
  /** luma8, row-major `width × height`. */
  readonly pixels: Uint8Array;
  /** rgba8, row-major `width × height × 4`. */
  readonly colors: Uint8ClampedArray<ArrayBuffer>;
}

/** A rasterized RGBA image — the ImageData contract without the DOM. */
export interface RasterData {
  readonly width: number;
  readonly height: number;
  readonly data: Uint8ClampedArray<ArrayBuffer>;
}

/** The thumbnail ceiling (effects.rs:272, `thumbnail(2048, 2048)`). */
export const RASTER_THUMBNAIL_MAX = 2048;

/** effects.rs:201 — the 4×4 ordered-dither matrix. */
export const BAYER: readonly (readonly number[])[] = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
];

/**
 * effects.rs:126-137 — ten five-column glyphs as seven rows of 5-bit
 * columns (artwork pixels, not shaped text runs), one column/row of spacing.
 */
export const GLYPHS: readonly (readonly number[])[] = [
  [0, 0, 0, 0, 0, 0, 0],
  [0, 0, 0, 0, 0, 4, 0],
  [0, 4, 0, 0, 4, 0, 0],
  [0, 0, 0, 14, 0, 0, 0],
  [0, 0, 14, 0, 14, 0, 0],
  [0, 4, 4, 31, 4, 4, 0],
  [0, 21, 14, 31, 14, 21, 0],
  [10, 10, 31, 10, 31, 10, 10],
  [17, 2, 4, 4, 8, 16, 17],
  [14, 17, 23, 21, 23, 16, 14],
];

export function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

/**
 * The `image` crate's luma8 conversion (color.rs `SRGB_LUMA`): Rec.709
 * fixed point `(2126·R + 7152·G + 722·B) / 10000`, truncated; alpha drops.
 */
function luma8(r: number, g: number, b: number): number {
  return ((2126 * r + 7152 * g + 722 * b) / 10000) | 0;
}

function colorAt(
  source: BackgroundLuminance,
  index: number,
): [number, number, number, number] {
  const offset = index * 4;
  return [
    source.colors[offset] ?? 0,
    source.colors[offset + 1] ?? 0,
    source.colors[offset + 2] ?? 0,
    source.colors[offset + 3] ?? 0,
  ];
}

// ---------------------------------------------------------------------------
// The pure rasters (effects.rs:109-313)
// ---------------------------------------------------------------------------

/**
 * `cover_index` (effects.rs:227-240): map a raster-space `(x, y)` to the
 * flat source index through the same cover fit the hero paint uses —
 * `max(raster_w/src_w, raster_h/src_h)` scale, centered crop, clamped to the
 * source bounds.
 */
export function coverIndex(
  source: BackgroundLuminance,
  width: number,
  height: number,
  x: number,
  y: number,
): number {
  const rasterWidth = Math.max(width, 1);
  const rasterHeight = Math.max(height, 1);
  const scale = Math.max(rasterWidth / source.width, rasterHeight / source.height);
  const visibleWidth = rasterWidth / scale;
  const visibleHeight = rasterHeight / scale;
  const sourceX = Math.trunc(
    clamp((source.width - visibleWidth) * 0.5 + x / scale, 0, source.width - 1),
  );
  const sourceY = Math.trunc(
    clamp((source.height - visibleHeight) * 0.5 + y / scale, 0, source.height - 1),
  );
  return sourceY * source.width + sourceX;
}

/** `sample_cover` (effects.rs:223-225): the source luma at the cover-mapped point. */
function sampleCover(source: BackgroundLuminance, width: number, height: number, x: number, y: number): number {
  return source.pixels[coverIndex(source, width, height, x, y)] ?? 0;
}

/**
 * `dither_color` (effects.rs:303-313): `peak = max(r,g,b)`;
 * `bright = peak/255 > (threshold + 0.5)/16`; gain `255/peak` when bright
 * (else 0.08); each channel rounds to nearest, alpha is kept.
 */
export function ditherColor(
  color: readonly [number, number, number, number],
  threshold: number,
): [number, number, number, number] {
  const [r, g, b, a] = color;
  const peak = Math.max(r, g, b);
  const bright = peak / 255 > (threshold + 0.5) / 16;
  const gain = bright ? 255 / Math.max(peak, 1) : 0.08;
  return [Math.round(r * gain), Math.round(g * gain), Math.round(b * gain), a];
}

/**
 * `dither_pixels` (effects.rs:200-221): 2×2 cells stepped by 2, one
 * quantize per cell sampled at the cover-fit center `(x+1, y+1)`, indexed
 * by `BAYER[y/2 % 4][x/2 % 4]`; the quantized color fills the whole cell.
 */
export function ditherPixels(
  source: BackgroundLuminance,
  width: number,
  height: number,
): RasterData {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      // Sample/quantize once per dot, not four times per 2x2 cell.
      const index = coverIndex(source, width, height, x + 1, y + 1);
      const color = ditherColor(
        colorAt(source, index),
        BAYER[(y / 2) % 4]![(x / 2) % 4]!,
      );
      for (let dy = 0; dy < Math.min(2, height - y); dy++) {
        for (let dx = 0; dx < Math.min(2, width - x); dx++) {
          const offset = ((y + dy) * width + (x + dx)) * 4;
          data[offset] = color[0];
          data[offset + 1] = color[1];
          data[offset + 2] = color[2];
          data[offset + 3] = color[3];
        }
      }
    }
  }
  return { width, height, data };
}

/**
 * `scanline_pixels` (effects.rs:109-122): every third row gains 0.52 — dark
 * pulls the row toward black (`v·gain`), light toward paper
 * (`v + (255−v)·(1−gain)`) — per channel, alpha preserved.
 */
export function scanlinePixels(source: BackgroundLuminance, light: boolean): RasterData {
  const { width, height } = source;
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    const gain = y % 3 === 0 ? 0.52 : 1.0;
    for (let x = 0; x < width; x++) {
      const offset = (y * width + x) * 4;
      for (let channel = 0; channel < 3; channel++) {
        const value = source.colors[offset + channel] ?? 0;
        data[offset + channel] = light
          ? Math.trunc(value + (255 - value) * (1 - gain))
          : Math.trunc(value * gain);
      }
      data[offset + 3] = source.colors[offset + 3] ?? 0;
    }
  }
  return { width, height, data };
}

/**
 * `ascii_pixels` (effects.rs:123-164): 6×8 cells (5 glyph columns + 1
 * spacing, 7 rows + 1 spacing). The cell's ink density is the luma at the
 * cell center (inverted on light), the glyph index is `√(density/255)·9`,
 * and each pixel mixes 60% base color + 40% (ink ? the sampled glyph color
 * : paper) — a colored image stays beneath the glyph texture in both themes.
 */
export function asciiPixels(source: BackgroundLuminance, light: boolean): RasterData {
  const { width, height } = source;
  const data = new Uint8ClampedArray(width * height * 4);
  const paper = light ? 255 : 0;
  for (let y = 0; y < height; y++) {
    const sampleY = Math.min(Math.trunc(y / 8) * 8 + 4, height - 1);
    for (let x = 0; x < width; x++) {
      const sampleX = Math.min(Math.trunc(x / 6) * 6 + 3, width - 1);
      const sample = sampleY * width + sampleX;
      const density = light ? 255 - (source.pixels[sample] ?? 0) : source.pixels[sample] ?? 0;
      const glyph = GLYPHS[Math.trunc(Math.sqrt(density / 255) * 9)] ?? GLYPHS[0]!;
      const ink =
        x % 6 < 5 &&
        y % 8 < 7 &&
        (glyph[y % 8]! & (1 << (4 - (x % 6)))) !== 0;
      const offset = (y * width + x) * 4;
      const sampleOffset = sample * 4;
      for (let channel = 0; channel < 3; channel++) {
        const base = source.colors[offset + channel] ?? 0;
        const inkColor = source.colors[sampleOffset + channel] ?? 0;
        data[offset + channel] = Math.trunc(
          base * 0.6 + (ink ? inkColor * 0.4 : paper * 0.4),
        );
      }
      data[offset + 3] = source.colors[offset + 3] ?? 0;
    }
  }
  return { width, height, data };
}

/**
 * `halftone_pixels` (effects.rs:165-198): 4×4 cells on a paper field;
 * `luma = cover_sample(x, y)` (inverted when light),
 * `radius = 2·(0.3 + 0.7·√(luma/255))`, dot color sampled at
 * `(x+2, y+2)`; per sub-pixel `coverage = clamp(radius + 0.5 − distance, 0, 1)·α/255`
 * and `blend = source·0.60 + (dot·coverage + paper·(1−coverage))·0.40`.
 */
export function halftonePixels(
  source: BackgroundLuminance,
  width: number,
  height: number,
  light: boolean,
): RasterData {
  const paper = light ? 255 : 0;
  const data = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const offset = i * 4;
    data[offset] = paper;
    data[offset + 1] = paper;
    data[offset + 2] = paper;
    data[offset + 3] = 255;
  }
  for (let y = 0; y < height; y += 4) {
    for (let x = 0; x < width; x += 4) {
      const luma = sampleCover(source, width, height, x, y);
      const toned = light ? 255 - luma : luma;
      const radius = 2 * (0.3 + 0.7 * Math.sqrt(toned / 255));
      const dotOffset = coverIndex(source, width, height, x + 2, y + 2) * 4;
      const dotAlpha = source.colors[dotOffset + 3] ?? 0;
      for (let dy = 0; dy < Math.min(4, height - y); dy++) {
        for (let dx = 0; dx < Math.min(4, width - x); dx++) {
          const distance = Math.sqrt((dx - 1.5) ** 2 + (dy - 1.5) ** 2);
          const coverage = clamp(radius + 0.5 - distance, 0, 1) * (dotAlpha / 255);
          const offset = ((y + dy) * width + (x + dx)) * 4;
          for (let channel = 0; channel < 3; channel++) {
            const value = source.colors[offset + channel] ?? 0;
            const dot = source.colors[dotOffset + channel] ?? 0;
            data[offset + channel] = Math.trunc(
              value * 0.6 + (dot * coverage + paper * (1 - coverage)) * 0.4,
            );
          }
          data[offset + 3] = source.colors[offset + 3] ?? 0;
        }
      }
    }
  }
  return { width, height, data };
}

/**
 * The `None` branch of `raster_image` (effects.rs:75-80): the source colors,
 * unchanged — the desktop's byte swap is BGRA layout only, which the web
 * does not port.
 */
export function noneRaster(source: BackgroundLuminance): RasterData {
  return {
    width: source.width,
    height: source.height,
    data: new Uint8ClampedArray(source.colors),
  };
}

/** The `raster_image` effect dispatch (effects.rs:74-89), pure. */
export function rasterizeEffect(
  source: BackgroundLuminance,
  effect: NewThreadBackgroundEffect,
  light: boolean,
): RasterData {
  switch (effect) {
    case "none":
      return noneRaster(source);
    case "dither":
      return ditherPixels(source, source.width, source.height);
    case "halftone":
      return halftonePixels(source, source.width, source.height, light);
    case "ascii":
      return asciiPixels(source, light);
    case "scanlines":
      return scanlinePixels(source, light);
  }
}

// ---------------------------------------------------------------------------
// The (effect, light) raster cache — one pending job per key (effects.rs:53-67)
// ---------------------------------------------------------------------------

/**
 * The cache key: `(effect, light)` with Dither and None
 * appearance-independent (effects.rs:53-61) — an appearance flip reuses the
 * same raster instead of re-running the job.
 */
export function effectRasterKey(effect: NewThreadBackgroundEffect, light: boolean): string {
  const appearanceDependent = effect !== "dither" && effect !== "none";
  return `${effect}:${appearanceDependent && light ? "light" : "dark"}`;
}

/** The decode/rasterize seams, injectable so unit tests drive them synchronously. */
export interface EffectRasterDriver {
  /** effects.rs:265-280 — decode, `thumbnail(2048, 2048)`, keep luma8 + rgba8. */
  readonly loadLuminance: (url: string) => Promise<BackgroundLuminance>;
  /** effects.rs:69-105 — rasterize off the main thread; readiness refreshes the hero. */
  readonly rasterize: (
    source: BackgroundLuminance,
    effect: NewThreadBackgroundEffect,
    light: boolean,
  ) => Promise<RasterData>;
}

/** The message the rasterizer worker accepts (a whole job, source included). */
export interface RasterJobRequest {
  readonly id: number;
  readonly source: BackgroundLuminance;
  readonly effect: NewThreadBackgroundEffect;
  readonly light: boolean;
}

/** The worker's reply: the raster (buffer transferred) or the failure. */
export interface RasterJobResponse {
  readonly id: number;
  readonly raster?: RasterData;
  readonly error?: string;
}

const luminanceJobs = new Map<string, Promise<BackgroundLuminance>>();
const rasterJobs = new Map<string, Promise<RasterData>>();

/**
 * The get-or-memo-promise both job maps share: the memoized promise IS the
 * single pending job (effects.rs:243-263's reuse without the desktop-only
 * FIFO eviction) — 100 callers while it is pending share one job, and warm
 * callers reuse the settled value. A rejection evicts its own entry so a
 * retry can re-attempt (an offline fetch must not poison the cache).
 */
function memoPromise<T>(jobs: Map<string, Promise<T>>, key: string, start: () => Promise<T>): Promise<T> {
  const warm = jobs.get(key);
  if (warm !== undefined) {
    return warm;
  }
  const job = start();
  job.catch(() => {
    jobs.delete(key);
  });
  jobs.set(key, job);
  return job;
}

/**
 * The decoded artwork for a URL (effects.rs:243-263): one decode shared by
 * every effect key through `memoPromise`.
 */
export function luminanceSource(
  url: string,
  driver: EffectRasterDriver = browserDriver,
): Promise<BackgroundLuminance> {
  return memoPromise(luminanceJobs, url, () => driver.loadLuminance(url));
}

/**
 * `raster_image` (effects.rs:47-108): one raster per `(url, effect, light)`
 * key through `memoPromise` — the memoized promise is the single pending
 * job, so 100 calls while rasterizing yield ONE raster (the desktop's
 * `None` marker slot), and warm calls return the identical raster.
 * Rejections evict themselves.
 */
export function effectRaster(
  url: string,
  effect: NewThreadBackgroundEffect,
  light: boolean,
  driver: EffectRasterDriver = browserDriver,
): Promise<RasterData> {
  const key = `${url}\u0000${effectRasterKey(effect, light)}`;
  return memoPromise(rasterJobs, key, () =>
    luminanceSource(url, driver).then((source) => driver.rasterize(source, effect, light)),
  );
}

/**
 * `prepare` (effects.rs:292-302): safe to call on BOTH routes — decode and
 * effect work never depend on hero geometry, so the prewarm runs before the
 * hero requests anything. Takes the resolved appearance; the hero later
 * finds the warm cache.
 */
export function prepareNewThreadBackgroundEffects(
  effect: NewThreadBackgroundEffect,
  appearance: Appearance,
  url: string,
  driver: EffectRasterDriver = browserDriver,
): Promise<RasterData> {
  return effectRaster(url, effect, appearance === "light", driver);
}

/** Test seam: the module-level caches are stateful; tests reset them. */
export function __resetEffectRasterCacheForTests(): void {
  luminanceJobs.clear();
  rasterJobs.clear();
}

// ---------------------------------------------------------------------------
// The browser driver — createImageBitmap decode, worker rasterization
// ---------------------------------------------------------------------------

function createRasterCanvas(width: number, height: number): {
  canvas: HTMLCanvasElement | OffscreenCanvas;
  context: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
} {
  if (typeof OffscreenCanvas === "function") {
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (context !== null) {
      return { canvas, context };
    }
  }
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (context === null) {
    throw new Error("new-thread background rasterization needs a 2d context");
  }
  return { canvas, context };
}

/**
 * effects.rs:265-280 — decode by ticket 15's `createImageBitmap` contract,
 * `thumbnail(2048, 2048)` (aspect preserved; the resize itself runs in the
 * browser's decode pool), then extract luma8 + rgba8.
 */
async function loadLuminanceBrowser(url: string): Promise<BackgroundLuminance> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`new-thread background failed to load: ${url}`);
  }
  const blob = await response.blob();
  const decoded = await createImageBitmap(blob);
  const scale = Math.min(
    1,
    RASTER_THUMBNAIL_MAX / decoded.width,
    RASTER_THUMBNAIL_MAX / decoded.height,
  );
  const width = Math.max(1, Math.round(decoded.width * scale));
  const height = Math.max(1, Math.round(decoded.height * scale));
  const { context } = createRasterCanvas(width, height);
  if (width === decoded.width && height === decoded.height) {
    context.drawImage(decoded, 0, 0);
  } else {
    const resized = await createImageBitmap(blob, {
      resizeWidth: width,
      resizeHeight: height,
      resizeQuality: "high",
    });
    context.drawImage(resized, 0, 0);
    resized.close();
  }
  decoded.close();
  const { data } = context.getImageData(0, 0, width, height);
  const pixels = new Uint8Array(width * height);
  for (let i = 0, offset = 0; i < pixels.length; i++, offset += 4) {
    pixels[i] = luma8(data[offset] ?? 0, data[offset + 1] ?? 0, data[offset + 2] ?? 0);
  }
  return { width, height, pixels, colors: data };
}

/**
 * The off-main-thread rasterizer: a dedicated module worker
 * (`new-thread-background-effects-worker.ts`) — the peer of the desktop's
 * background executor. The first failure permanently falls back to the main
 * thread (a browser without module workers must still get the rasters).
 */
let rasterWorker: Worker | null = null;
let rasterWorkerBroken = false;
let rasterWorkerJobId = 0;
const rasterWorkerJobs = new Map<
  number,
  { resolve: (raster: RasterData) => void; reject: (error: unknown) => void }
>();

function failRasterWorkerJobs(): void {
  rasterWorkerBroken = true;
  for (const [id, job] of rasterWorkerJobs) {
    job.reject(new Error("new-thread background raster worker failed"));
    rasterWorkerJobs.delete(id);
  }
}

function rasterWorkerInstance(): Worker | null {
  if (rasterWorkerBroken) {
    return null;
  }
  if (rasterWorker !== null) {
    return rasterWorker;
  }
  if (typeof Worker === "undefined") {
    rasterWorkerBroken = true;
    return null;
  }
  try {
    const worker = new Worker(
      new URL("./new-thread-background-effects-worker.ts", import.meta.url),
      { type: "module" },
    );
    worker.onmessage = (event: MessageEvent<RasterJobResponse>) => {
      const response = event.data;
      const job = rasterWorkerJobs.get(response.id);
      if (job === undefined) {
        return;
      }
      rasterWorkerJobs.delete(response.id);
      if (response.error !== undefined || response.raster === undefined) {
        job.reject(new Error(response.error ?? "new-thread background raster failed"));
      } else {
        job.resolve(response.raster);
      }
    };
    worker.onerror = failRasterWorkerJobs;
    rasterWorker = worker;
    return worker;
  } catch {
    rasterWorkerBroken = true;
    return null;
  }
}

function rasterizeBrowser(
  source: BackgroundLuminance,
  effect: NewThreadBackgroundEffect,
  light: boolean,
): Promise<RasterData> {
  const worker = rasterWorkerInstance();
  if (worker === null) {
    return Promise.resolve(rasterizeEffect(source, effect, light));
  }
  const id = ++rasterWorkerJobId;
  return new Promise((resolve, reject) => {
    rasterWorkerJobs.set(id, { resolve, reject });
    worker.postMessage({ id, source, effect, light } satisfies RasterJobRequest);
  });
}

const browserDriver: EffectRasterDriver = {
  loadLuminance: loadLuminanceBrowser,
  rasterize: rasterizeBrowser,
};
