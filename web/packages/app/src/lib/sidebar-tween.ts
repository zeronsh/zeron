import { motion } from "@zeron/theme";
import { rectEquals, type Rect } from "./new-thread-background";
import { dockGlideSignal, onDockGlideFrame } from "./dock-glide";

/**
 * The sidebar tween's settle contract (ticket 57a) — the pure half of the
 * cadence that replaced the per-frame pump. The desktop evaluates
 * `sidebar_now()` INSIDE render (one scalar, GPU-side); the web's peer is a
 * CSS `width` transition the BROWSER interpolates — zero React frames, zero
 * re-rasters between the flip and `transitionend`. This module owns the two
 * pieces other code needs to coordinate with that glide:
 *
 * - the SIGNAL (`sidebarTweenSignal` / `sidebarTweenActive()`): true from the
 *   flip commit until the transition settles — the deferral flag tickets
 *   59/63 (and any later per-frame work) consult so nothing re-renders or
 *   re-rasters under the moving column, and `subscribe()` hands the same
 *   arm/settle window to boundary-work riders (63's identity freeze) with
 *   no second flag threaded through the tree;
 * - the remask cadence's ONE predicate (`remaskDue` + `HeroRemaskGate`):
 *   the hero re-rasters only on settle, artwork/effect change, or — once
 *   settled — a real geometry change. While the tween runs, the cutout hole
 *   is tracked by the raster-window CSS (the readiness layer holds the
 *   pre-flip bitmap centered on the gliding hero), so geometry ticks are
 *   absorbed, never painted.
 *
 * Pure by design (no `now`, no DOM) so the unit tests drive the timeline
 * directly; `ConversationPage` arms/settles the signal, the hero's
 * ResizeObservers route through the gate.
 */

/** `motion::RESIZE` — the 200ms curve the sidebar glide and the hero's width transition ride. */
export const SIDEBAR_GLIDE_MS =
  motion.specs.find((spec) => spec.name === "resize")?.durationMs ?? 200;

/**
 * The settle cap: `transitionend` is the settle signal, but it can be
 * swallowed (the hero unmounts mid-glide, the tab hides, a style kill
 * cancels without a successor) — the page still settles within this bound.
 */
export const SIDEBAR_SETTLE_CAP_MS = 120;

/** Why a remask was requested — the settle predicate's only input. */
export type RemaskReason = "settle" | "artwork" | "hero-geometry" | "surface-geometry";

/**
 * The remask cadence's ONE predicate (§2.2): a remask is due only for
 * settle (the `transitionend` commit — the snap to the true raster) and for
 * artwork/effect changes (which re-fix the raster window themselves). Hero
 * and composer-surface geometry changes remask only ONCE the tween has
 * settled — during the glide the raster window tracks the hole, so the
 * per-frame geometry ticks the observers report are absorbed, not painted.
 */
export function remaskDue(tweenActive: boolean, reason: RemaskReason): boolean {
  if (reason === "settle" || reason === "artwork") {
    return true;
  }
  return !tweenActive;
}

/**
 * The sidebar tween's armed/settled flag: armed at the flip commit (the one
 * state flip that starts the CSS transition), settled at `transitionend`.
 * A mid-glide reversal re-arms (the CSS transition retargets from the
 * painted width — the desktop's `sidebar_now()` capture semantics).
 */
export class SidebarTweenSignal {
  #active = false;
  #listeners = new Set<(active: boolean) => void>();

  /** The flip landed and the CSS transition is (re)starting. */
  arm(): void {
    this.#active = true;
    this.#notify();
  }

  /** `transitionend` (or the settle cap / a disarm) — the glide is over. */
  settle(): void {
    this.#active = false;
    this.#notify();
  }

  isActive(): boolean {
    return this.#active;
  }

  /**
   * Observe the arm/settle edges synchronously — the window's riders (63's
   * identity freeze) attach once and ride every settle path (transitionend,
   * the cap, the drag/reduce disarms) for free, since they all funnel
   * through `settle()`. Fires on every arm/settle call, flip or re-arm
   * alike: a mid-glide reversal re-arms, and a listener that re-captures
   * from the painted state on it matches the signal's own retarget
   * semantics. Returns the unsubscribe.
   */
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

/** The page-scoped sidebar tween signal (tickets 59/63 defer work while it is active). */
export const sidebarTweenSignal = new SidebarTweenSignal();

/**
 * `true` while the sidebar's 200ms glide is running (the hero's cutout and
 * any deferrable per-frame work should ride it out, not fight it).
 */
export function sidebarTweenActive(): boolean {
  return sidebarTweenSignal.isActive();
}

/**
 * The hero remask scheduler: every remask request (dep-change, hero
 * ResizeObserver, composer-surface ResizeObserver, settle) routes through
 * the settle predicate against the LIVE tween signal, so the component's
 * observers can report every frame they see and still paint nothing during
 * the tween.
 */
export class HeroRemaskGate {
  #remask: () => void;

  constructor(remask: () => void) {
    this.#remask = remask;
  }

  note(reason: RemaskReason): void {
    if (remaskDue(sidebarTweenSignal.isActive(), reason)) {
      this.#remask();
    }
  }
}

// ---------------------------------------------------------------------------
// The hero render scheduler (ticket 65 — the HeroRemaskGate successor)
// ---------------------------------------------------------------------------

/**
 * The arm/settle edge contract both motion signals satisfy (ticket 65 §3.1).
 * `SidebarTweenSignal` and `DockGlideSignal` both implement it.
 */
export interface MotionSignalLike {
  isActive(): boolean;
  subscribe(listener: (active: boolean) => void): () => void;
}

/** The renderer's per-frame geometry read: hero + composer measured together. */
export interface HeroGeometrySample {
  readonly hero: Rect;
  readonly composer: Rect;
  readonly dpr: number;
}

export interface HeroRenderSchedulerOptions {
  /** Measure hero + composer together (production: two getBoundingClientRect reads). */
  readonly sample: () => HeroGeometrySample;
  /** Paint at the sampled geometry (production: the hero background renderer). */
  readonly render: (geometry: HeroGeometrySample) => void;
  /** Artwork/effect source re-upload (production: texture upload) — optional. */
  readonly uploadSource?: () => void;
  /** Defaults to the page-scoped `sidebarTweenSignal`. */
  readonly sidebar?: MotionSignalLike;
  /** Defaults to the page-scoped `dockGlideSignal`. */
  readonly dock?: MotionSignalLike;
  /** Defaults to `onDockGlideFrame` — the pump's post-prepaint hook. */
  readonly onDockFrame?: (listener: () => void) => () => void;
  /** Defaults to `requestAnimationFrame`/`cancelAnimationFrame`. */
  readonly requestFrame?: (callback: () => void) => number;
  readonly cancelFrame?: (handle: number) => void;
  /**
   * The current frame's identity for same-frame dedupe. The production
   * default is a constant: the sampled geometry IS the dedupe key — a
   * render is skipped only when the same geometry was already rendered, so
   * coalescing holds across the settle (the converged frame's render
   * satisfies the settle paint) while any real change repaints. The
   * prototype's tests inject a frame counter to prove the same-frame rule
   * explicitly.
   */
  readonly frameId?: () => number;
}

/**
 * Ticket 65's render scheduler — the gate's lifecycle generalized to BOTH
 * motion signals (§2.5 item 2 / §3.65b):
 *
 * - Geometry notes (both ResizeObservers) COALESCE: at most one render per
 *   animation frame, and a note whose sampled geometry equals the
 *   already-rendered geometry renders nothing.
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
  #options: Required<Omit<HeroRenderSchedulerOptions, "uploadSource">> & Pick<HeroRenderSchedulerOptions, "uploadSource">;
  #raf: number | null = null;
  #disposed = false;
  #lastRender: { readonly frame: number; readonly geometry: HeroGeometrySample } | null = null;
  #unsubscribe: Array<() => void> = [];

  constructor(options: HeroRenderSchedulerOptions) {
    this.#options = {
      sidebar: options.sidebar ?? sidebarTweenSignal,
      dock: options.dock ?? dockGlideSignal,
      onDockFrame: options.onDockFrame ?? onDockGlideFrame,
      // Arrows, never bare natives: a default captured as `requestAnimationFrame`
      // itself would be invoked as a method of this options object (receiver ≠
      // window), which the native function rejects with `TypeError: Illegal
      // invocation` through the signal's notify chain — the sidebar-click crash.
      requestFrame: options.requestFrame ?? ((callback: () => void) => window.requestAnimationFrame(callback)),
      cancelFrame: options.cancelFrame ?? ((handle: number) => window.cancelAnimationFrame(handle)),
      frameId: options.frameId ?? (() => 0),
      sample: options.sample,
      render: options.render,
      uploadSource: options.uploadSource,
    };
    this.#unsubscribe.push(this.#options.sidebar.subscribe(this.#onEdge));
    this.#unsubscribe.push(this.#options.dock.subscribe(this.#onEdge));
    this.#unsubscribe.push(this.#options.onDockFrame(this.#onDockFrame));
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
