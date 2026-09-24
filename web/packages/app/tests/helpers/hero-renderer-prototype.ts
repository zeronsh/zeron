/**
 * TEST-SCOPED Stage-A prototype for ticket 65 (new-chat background raster
 * continuity). This is NOT production code — nothing under `src/` imports
 * it, and production integration is gated on the runtime evidence recorded
 * in `.scratch/web-parity/research-2026-09-20/background-raster-design.md`.
 *
 * Two halves, structured the way the candidate production renderer is
 * specified in that design:
 *
 * 1. THE SHADER MODEL — the candidate GPU renderer's math as the fragment
 *    shader consumes it: a uniform bundle precomputed per geometry/source
 *    change (mask center/half-size, radius clamped exactly like
 *    `ImageAlphaMask::scale`, feather/clearance, the shared bottom fade,
 *    the cover-fit origin/scale expressed in window space, the pass
 *    opacity), then a per-pixel window-space evaluation and the two-pass
 *    source-over composite. `tests/new-thread-background.test.ts` compares
 *    this against the production oracle (`heroMaskGeometry` /
 *    `cutoutHoleAlpha` / `cutoutBottomFadeAlpha` / `cutoutMaskAlpha` /
 *    `cutoutMaskRaster` from `src/lib/new-thread-background.ts`) — §2.5
 *    gate item 3.
 *
 * 2. THE RENDER SCHEDULER — the arm/frame/artwork/settle/cancel state
 *    machine composing the sidebar tween signal and the PROPOSED dock
 *    glide subscription (gate item 2): geometry notes coalesce to at most
 *    one render per animation frame; while motion is active the render
 *    cadence comes from the animation itself (the dock pump's
 *    post-prepaint hook preferred, the renderer's own rAF otherwise);
 *    when the last signal settles exactly one final paint lands at the
 *    measured end geometry; disposal leaves no stale callbacks.
 */

import {
  CUTOUT_REVEAL_OPACITY,
  cutoutMaskAlpha,
  heroMaskGeometry,
  rectEquals,
  type Rect,
} from "../../src/lib/new-thread-background";

function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

/** The GLSL smoothstep both ramps ride: `t²(3 − 2t)` over `clamp(value/edge, 0, 1)`. */
function smoothstep(value: number, edge: number): number {
  const t = clamp(value / edge, 0, 1);
  return t * t * (3 - 2 * t);
}

// ---------------------------------------------------------------------------
// 1. The shader model
// ---------------------------------------------------------------------------

export interface SourceSize {
  readonly width: number;
  readonly height: number;
}

/**
 * The uniform bundle the GPU renderer computes once per geometry/source
 * change (never per pixel). Everything is window-space CSS pixels except
 * the canvas backing dims — the same split the desktop shader uses.
 */
export interface HeroShaderUniforms {
  /** Canvas backing dims (device px), rounded exactly like the component's paint(). */
  readonly canvasWidth: number;
  readonly canvasHeight: number;
  readonly dpr: number;
  readonly hero: Rect;
  readonly maskCenterX: number;
  readonly maskCenterY: number;
  readonly maskHalfWidth: number;
  readonly maskHalfHeight: number;
  /** Radius clamped to half each mask dimension — `ImageAlphaMask::scale`. */
  readonly radius: number;
  readonly feather: number;
  readonly clearance: number;
  /** The shader's early-out: a non-positive feather disables the whole mask. */
  readonly maskEnabled: boolean;
  readonly fadeEnd: number;
  readonly fadeHeight: number;
  /** Cover-fit in window space: `source px × fittedScale = window px`. */
  readonly fittedOriginX: number;
  readonly fittedOriginY: number;
  readonly fittedScale: number;
  /** The pass's element opacity (0.5 reveal / 1 cutout) folded into the composite. */
  readonly passOpacity: number;
}

/**
 * Precompute the uniform bundle. The mask parameters are REUSED from the
 * production `heroMaskGeometry` (§2.4: the pure helpers are the numeric
 * reference or the implementation); the cover-fit is the component's
 * device-space `drawImage` dest-rect math divided back into window space,
 * computed from the ROUNDED backing dims exactly like the component.
 */
export function computeHeroShaderUniforms(
  hero: Rect,
  composer: Rect,
  cutout: boolean,
  source: SourceSize,
  dpr: number,
): HeroShaderUniforms {
  const mask = heroMaskGeometry(hero, composer, cutout);
  const canvasWidth = Math.max(1, Math.round(hero.width * dpr));
  const canvasHeight = Math.max(1, Math.round(hero.height * dpr));
  const cover = Math.max(canvasWidth / source.width, canvasHeight / source.height);
  const bounds = mask.bounds;
  return {
    canvasWidth,
    canvasHeight,
    dpr,
    hero,
    maskCenterX: bounds.x + bounds.width * 0.5,
    maskCenterY: bounds.y + bounds.height * 0.5,
    maskHalfWidth: bounds.width * 0.5,
    maskHalfHeight: bounds.height * 0.5,
    radius: Math.min(
      Math.max(mask.radius, 0),
      Math.max(bounds.width, 0) * 0.5,
      Math.max(bounds.height, 0) * 0.5,
    ),
    feather: mask.feather,
    clearance: mask.clearance,
    maskEnabled: mask.feather > 0,
    fadeEnd: mask.bottomFade.end,
    fadeHeight: mask.bottomFade.height,
    fittedOriginX: hero.x + (canvasWidth - source.width * cover) / 2 / dpr,
    fittedOriginY: hero.y + (canvasHeight - source.height * cover) / 2 / dpr,
    fittedScale: cover / dpr,
    passOpacity: cutout ? 1 : CUTOUT_REVEAL_OPACITY,
  };
}

/**
 * The fragment shader's mask terms: the rounded-rect SDF smoothstep dome
 * and the shared bottom fade, composed by MIN inside one mask — the
 * desktop's `image_mask_alpha`, fed from the precomputed uniforms instead
 * of the raw geometry (the transform chain the oracle comparison
 * validates). The terms are returned separately so the tests can compare
 * each against its own production oracle (`cutoutHoleAlpha`,
 * `cutoutBottomFadeAlpha`).
 */
export function shaderMaskTerms(
  uniforms: HeroShaderUniforms,
  x: number,
  y: number,
): { readonly hole: number; readonly fade: number; readonly mask: number } {
  if (!uniforms.maskEnabled) {
    return { hole: 1, fade: 1, mask: 1 };
  }
  const qx = Math.abs(x - uniforms.maskCenterX) - uniforms.maskHalfWidth + uniforms.radius;
  const qy = Math.abs(y - uniforms.maskCenterY) - uniforms.maskHalfHeight + uniforms.radius;
  const distance =
    Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - uniforms.radius;
  const hole = smoothstep(distance - uniforms.clearance, uniforms.feather);
  const fade = uniforms.fadeHeight <= 0 ? 1 : smoothstep(uniforms.fadeEnd - y, uniforms.fadeHeight);
  return { hole, fade, mask: Math.min(hole, fade) };
}

/** The composed mask alpha — `min(hole, fade)`, the desktop's ONE shader. */
export function shaderMaskAlpha(uniforms: HeroShaderUniforms, x: number, y: number): number {
  return shaderMaskTerms(uniforms, x, y).mask;
}

/** The fragment's texture lookup: window point → source-image coordinates. */
export function shaderSourceCoord(
  uniforms: HeroShaderUniforms,
  x: number,
  y: number,
): { readonly sx: number; readonly sy: number } {
  return {
    sx: (x - uniforms.fittedOriginX) / uniforms.fittedScale,
    sy: (y - uniforms.fittedOriginY) / uniforms.fittedScale,
  };
}

/** An artwork the tests sample exactly (no filtering — both sides sample identically). */
export interface AnalyticArtwork {
  readonly width: number;
  readonly height: number;
  readonly at: (sx: number, sy: number) => { readonly r: number; readonly g: number; readonly b: number };
}

export interface CompositedPixel {
  readonly r: number;
  readonly g: number;
  readonly b: number;
  readonly a: number;
}

function composite(
  source: { readonly r: number; readonly g: number; readonly b: number },
  page: { readonly r: number; readonly g: number; readonly b: number },
  revealAlpha: number,
  cutoutAlpha: number,
): CompositedPixel {
  // srcOver(cutout, srcOver(reveal, page)) — the two stacked canvases with
  // their element opacities over the opaque page, in one expression.
  const baseR = source.r * revealAlpha + page.r * (1 - revealAlpha);
  const baseG = source.g * revealAlpha + page.g * (1 - revealAlpha);
  const baseB = source.b * revealAlpha + page.b * (1 - revealAlpha);
  const baseA = revealAlpha + 1 * (1 - revealAlpha);
  return {
    r: source.r * cutoutAlpha + baseR * (1 - cutoutAlpha),
    g: source.g * cutoutAlpha + baseG * (1 - cutoutAlpha),
    b: source.b * cutoutAlpha + baseB * (1 - cutoutAlpha),
    a: cutoutAlpha + baseA * (1 - cutoutAlpha),
  };
}

/** The candidate renderer's final pixel: uniforms in, two-pass source-over out. */
export function renderHeroPixel(
  reveal: HeroShaderUniforms,
  cutout: HeroShaderUniforms,
  artwork: AnalyticArtwork,
  page: { readonly r: number; readonly g: number; readonly b: number },
  x: number,
  y: number,
): CompositedPixel {
  const { sx, sy } = shaderSourceCoord(cutout, x, y);
  const source = artwork.at(sx, sy);
  const cutoutAlpha = shaderMaskAlpha(cutout, x, y) * cutout.passOpacity;
  const revealAlpha = shaderMaskAlpha(reveal, x, y) * reveal.passOpacity;
  return composite(source, page, revealAlpha, cutoutAlpha);
}

/**
 * The oracle's final pixel: mask alphas from the production
 * `cutoutMaskAlpha` over `heroMaskGeometry`, the source sampled through the
 * component's own device-space `drawImage` cover-fit math
 * (new-thread-background.tsx:301-333), composited identically.
 */
export function referenceHeroPixel(
  hero: Rect,
  composer: Rect,
  artwork: AnalyticArtwork,
  page: { readonly r: number; readonly g: number; readonly b: number },
  dpr: number,
  x: number,
  y: number,
): CompositedPixel {
  const cutoutMask = heroMaskGeometry(hero, composer, true);
  const revealMask = heroMaskGeometry(hero, composer, false);
  const cutoutAlpha = cutoutMaskAlpha(cutoutMask, x, y);
  const revealAlpha = cutoutMaskAlpha(revealMask, x, y) * CUTOUT_REVEAL_OPACITY;
  const canvasWidth = Math.max(1, Math.round(hero.width * dpr));
  const canvasHeight = Math.max(1, Math.round(hero.height * dpr));
  const cover = Math.max(canvasWidth / artwork.width, canvasHeight / artwork.height);
  const destX = (canvasWidth - artwork.width * cover) / 2;
  const destY = (canvasHeight - artwork.height * cover) / 2;
  const source = artwork.at(
    ((x - hero.x) * dpr - destX) / cover,
    ((y - hero.y) * dpr - destY) / cover,
  );
  return composite(source, page, revealAlpha, cutoutAlpha);
}

// ---------------------------------------------------------------------------
// 2. The render scheduler
// ---------------------------------------------------------------------------

/** The arm/settle edge contract both motion signals satisfy post-gate. */
export interface MotionSignalLike {
  isActive(): boolean;
  subscribe(listener: (active: boolean) => void): () => void;
}

/**
 * The PROPOSED `DockGlideSignal` shape after the gate adds `subscribe()`
 * (mirroring `SidebarTweenSignal`; research §3.64 "Dock signal" sanctions a
 * shared subscription tested with ticket 65). Tests drive this stand-in;
 * the production integration swaps in the real signal.
 */
export class ProposedDockSignal implements MotionSignalLike {
  #active = false;
  #listeners = new Set<(active: boolean) => void>();

  arm(): void {
    this.#active = true;
    this.#notify();
  }

  settle(): void {
    this.#active = false;
    this.#notify();
  }

  isActive(): boolean {
    return this.#active;
  }

  subscribe(listener: (active: boolean) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  #notify(): void {
    for (const listener of this.#listeners) {
      listener(this.#active);
    }
  }
}

/** The renderer's per-frame geometry read: hero + composer measured together. */
export interface HeroGeometrySample {
  readonly hero: Rect;
  readonly composer: Rect;
  readonly dpr: number;
}

export interface HeroRenderSchedulerOptions {
  readonly sidebar: MotionSignalLike;
  readonly dock: MotionSignalLike;
  /**
   * The dock pump's post-prepaint frame hook — production: a listener
   * registry in `dock-glide.ts` invoked at the end of `writeDockGlideVars`,
   * which the pump already calls after the wrapper transform write, so the
   * sample below sees THIS frame's transform-only placement.
   */
  readonly onDockFrame: (listener: () => void) => () => void;
  readonly requestFrame: (callback: () => void) => number;
  readonly cancelFrame: (handle: number) => void;
  /** The current frame's identity (production: the rAF timestamp's frame bucket). */
  readonly frameId: () => number;
  /** Measure hero + composer together (production: two getBoundingClientRect reads). */
  readonly sample: () => HeroGeometrySample;
  readonly render: (geometry: HeroGeometrySample) => void;
  /** Artwork/effect source re-upload (production: texture upload / raster repaint). */
  readonly uploadSource?: () => void;
}

/**
 * The scheduling rules from §2.5 item 2 / §3.65b, executable:
 *
 * - Geometry notes (both ResizeObservers) COALESCE: at most one render per
 *   animation frame, and a note whose sampled geometry equals the
 *   already-rendered geometry for this frame renders nothing.
 * - While `sidebar.isActive() || dock.isActive()` the render cadence comes
 *   from the motion itself — the dock pump's post-prepaint hook when the
 *   dock glide runs (same-frame placement, transform-only moves included),
 *   otherwise the renderer's own rAF riding the sidebar's CSS tween.
 *   Geometry notes during motion are absorbed, never painted synchronously.
 * - An artwork/effect change re-uploads the source and renders at the
 *   CURRENT geometry even mid-motion (the raster re-fixes the window).
 * - When the LAST signal settles, exactly one final paint lands at the
 *   measured end geometry, coalesced with any render already done this
 *   frame; one motion settling never releases the other's cadence.
 * - `dispose()` cancels the pending frame and unsubscribes; every entry
 *   point is a no-op afterwards.
 */
export class HeroRenderScheduler {
  #options: HeroRenderSchedulerOptions;
  #raf: number | null = null;
  #disposed = false;
  #lastRender: { readonly frame: number; readonly geometry: HeroGeometrySample } | null = null;
  #unsubscribe: Array<() => void> = [];

  constructor(options: HeroRenderSchedulerOptions) {
    this.#options = options;
    this.#unsubscribe.push(options.sidebar.subscribe(this.#onEdge));
    this.#unsubscribe.push(options.dock.subscribe(this.#onEdge));
    this.#unsubscribe.push(options.onDockFrame(this.#onDockFrame));
    // A mount can land mid-glide (the hero mounts while the dock dissolves
    // a selection away) — pick up the live cadence immediately.
    this.#syncCadence();
  }

  /** ResizeObserver notes: absorbed during motion; one sampled render per frame otherwise. */
  noteGeometry(): void {
    if (this.#disposed || this.#motionActive()) {
      return;
    }
    this.#renderCoalesced();
  }

  /** Artwork/effect change: re-upload + render at the CURRENT geometry, even mid-motion. */
  noteArtwork(): void {
    if (this.#disposed) {
      return;
    }
    this.#options.uploadSource?.();
    this.#renderForced();
  }

  /** The component's flag-fall settle commit (pre-paint), beside the signal edges. */
  noteSettle(): void {
    if (this.#disposed || this.#motionActive()) {
      return;
    }
    this.#renderCoalesced();
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#cancelRaf();
    for (const unsubscribe of this.#unsubscribe.splice(0)) {
      unsubscribe();
    }
  }

  #motionActive(): boolean {
    return this.#options.sidebar.isActive() || this.#options.dock.isActive();
  }

  #onEdge = (): void => {
    if (this.#disposed) {
      return;
    }
    this.#syncCadence();
    if (!this.#motionActive()) {
      // The LAST settle: exactly one final paint at the true bounds,
      // coalesced with any render already done this frame (the dock pump's
      // converged frame renders the same geometry through the hook).
      this.#renderCoalesced();
    }
  };

  #onDockFrame = (): void => {
    if (this.#disposed || !this.#options.dock.isActive()) {
      return;
    }
    this.#renderCoalesced();
  };

  /**
   * The frame source: the dock pump's hook while the dock glide runs (it
   * samples after THIS frame's placement write); the renderer's own rAF
   * while only the sidebar's CSS tween runs (the browser interpolates the
   * width, so sampling anywhere in the frame sees the current value). Both
   * active → the hook wins and the rAF stands down; dock settles while the
   * sidebar still runs → the rAF resumes on that edge.
   */
  #syncCadence(): void {
    if (!this.#disposed && this.#options.sidebar.isActive() && !this.#options.dock.isActive()) {
      this.#ensureRaf();
    } else {
      this.#cancelRaf();
    }
  }

  #ensureRaf(): void {
    if (this.#raf !== null || this.#disposed) {
      return;
    }
    this.#raf = this.#options.requestFrame(() => {
      this.#raf = null;
      if (this.#disposed) {
        return;
      }
      this.#renderCoalesced();
      this.#syncCadence();
    });
  }

  #cancelRaf(): void {
    if (this.#raf !== null) {
      this.#options.cancelFrame(this.#raf);
      this.#raf = null;
    }
  }

  #renderCoalesced(): void {
    const geometry = this.#options.sample();
    const last = this.#lastRender;
    const frame = this.#options.frameId();
    if (
      last !== null &&
      last.frame === frame &&
      last.geometry.dpr === geometry.dpr &&
      rectEquals(last.geometry.hero, geometry.hero) &&
      rectEquals(last.geometry.composer, geometry.composer)
    ) {
      return;
    }
    this.#lastRender = { frame, geometry };
    this.#options.render(geometry);
  }

  #renderForced(): void {
    const geometry = this.#options.sample();
    this.#lastRender = { frame: this.#options.frameId(), geometry };
    this.#options.render(geometry);
  }
}

/**
 * A manual animation clock for the scheduler tests: `pump()` advances the
 * frame identity and runs the pending rAF callbacks (which may re-register).
 */
export class ManualFramePump {
  #nextHandle = 0;
  #frame = 0;
  #pending = new Map<number, () => void>();

  readonly requestFrame = (callback: () => void): number => {
    this.#nextHandle += 1;
    this.#pending.set(this.#nextHandle, callback);
    return this.#nextHandle;
  };

  readonly cancelFrame = (handle: number): void => {
    this.#pending.delete(handle);
  };

  readonly frameId = (): number => this.#frame;

  get pendingCount(): number {
    return this.#pending.size;
  }

  pump(): void {
    this.#frame += 1;
    const callbacks = [...this.#pending.values()];
    this.#pending.clear();
    for (const callback of callbacks) {
      callback();
    }
  }
}
