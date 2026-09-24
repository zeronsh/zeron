import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SurfaceTreatment } from "@zeron/theme";
import type { NewThreadBackgroundEffect } from "../state/ui-settings";
import {
  newThreadBackgroundElementOpacity,
  newThreadBackgroundHeight,
} from "../lib/new-thread-background";
import { effectRaster, type RasterData } from "../lib/new-thread-background-effects";
import {
  createHeroBackgroundRenderer,
  type HeroArtworkSource,
  type HeroBackgroundRenderer,
} from "../lib/new-thread-background-renderer";
import { HeroRenderScheduler, type HeroGeometrySample } from "../lib/sidebar-tween";
import { useResolvedAppearance } from "../state/appearance";

/**
 * The new-thread background hero — the web port of `shell.rs:857-914`
 * (`new_thread_background`) over `new_thread_background_mask.rs`'s two-pass
 * feathered cutout.
 *
 * Two stacked CANVAS passes paint the same cover-fit artwork (the desktop's
 * `paint` fit math: max scale, centered, `corner_radii` 0, no grayscale —
 * drawn at the device-pixel ratio): the REVEAL pass at opacity 0.5 whose
 * alpha grid carries only the shared bottom fade, and the CUTOUT pass at
 * opacity 1 whose grid is the single combined `min(hole, fade)` mask — the
 * desktop's one shader, so the hole and the fade never multiply. The hole
 * is the exact smoothstep-over-SDF ramp (a hard 8px transparent margin,
 * then the 120–280px one-sided dome), not a Gaussian. No blend mode, no
 * theme overlay: the masks multiply source alpha and nothing else, and this
 * file references no theme roles.
 *
 * When a background effect is installed, the painted image is the
 * RASTERIZED artwork (`lib/new-thread-background-effects.ts` — still
 * cover-fit, still masked); `none` paints the raw artwork.
 *
 * The hero's frost leg reads the RESOLVED surface treatment off the
 * document root (`data-surface`): 0.84 under `"frosted"`, 1.0 under
 * `"opaque"` — the hero consumes the resolution instead of pre-deciding it,
 * while the defrost decision itself stays where it lives
 * (`lib/appearance-store.ts`).
 *
 * The mask is regenerated from the measured `#composer-surface` rect only
 * on the settle conditions (ticket 57a, §2.2): an artwork/effect change
 * (scheduled on the animation frame AFTER the commit's layout effects, so
 * it consumes the same frame's dock-transform write, never last frame's
 * geometry — the web peer of "all prepaint completes before any paint"), a
 * real geometry change (the hero's or the surface's ResizeObserver — the
 * 180 ms typing morph grows the pill, viewport resizes re-crop the artwork),
 * and the sidebar tween's settle commit (the snap to the true raster after
 * the CSS glide). While the tween runs, nothing re-rasters: the readiness
 * layer's raster-window CSS (app.css) holds the pre-flip bitmap centered on
 * the gliding hero so the dome hole tracks the pill with no JS per frame.
 *
 * The hero uses the full conversation-canvas width even while the right pane
 * clips it: navigation must never rescale the artwork.
 */

export interface NewThreadBackgroundProps {
  /**
   * The resolved artwork to paint, or null while nothing is resolved at all
   * (the store's `url === null` — the only null-paint case, ticket 35);
   * `ready` is the shell-scoped readiness clock's `data-ready` flag.
   */
  readonly artwork: { readonly url: string; readonly id: string | number; readonly ready: boolean } | null;
  readonly viewportHeight: number;
  /** `viewport_width − sidebar_now` — the full conversation canvas. */
  readonly heroWidth: number;
  /** The dock's `dissolve` channel: 1 = the established thread, 0 = the hero. */
  readonly dissolve: number;
  /** The settings-store effect — `none` paints the raw artwork; the others paint their raster. */
  readonly effect: NewThreadBackgroundEffect;
  /**
   * True while the sidebar's 200ms CSS glide runs (ticket 57a): renders the
   * hero's `data-sidebar-tween` flag (its width transition + the raster
   * window) and, on the fall to false, re-rasters at the settled geometry
   * inside the same commit.
   */
  readonly sidebarTween: boolean;
}

/** `prefers-reduced-motion` at first paint, reactive afterwards. */
function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    () =>
      typeof window !== "undefined" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduced(query.matches);
    onChange();
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

function readRootSurfaceTreatment(): SurfaceTreatment {
  if (typeof document === "undefined") {
    return "opaque";
  }
  return document.documentElement.dataset.surface === "frosted" ? "frosted" : "opaque";
}

/**
 * The resolved surface treatment, read off the document root's
 * `data-surface` (installed by `theme.ts`) — reactive to the attribute
 * itself, so a flipped resolution (or a forced screenshot state) re-renders
 * the hero's 0.84 frost leg without any store coupling.
 */
function useRootSurfaceTreatment(): SurfaceTreatment {
  const [surface, setSurface] = useState(readRootSurfaceTreatment);
  useEffect(() => {
    const observer = new MutationObserver(() => setSurface(readRootSurfaceTreatment()));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-surface"],
    });
    return () => observer.disconnect();
  }, []);
  return surface;
}

/** The raw artwork, decoded and ready for `drawImage`. */
function useDecodedImage(url: string | null): HTMLImageElement | null {
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  useEffect(() => {
    if (url === null) {
      setImage(null);
      return;
    }
    let cancelled = false;
    const element = new Image();
    element.src = url;
    void element.decode().then(
      () => {
        if (!cancelled) {
          setImage(element);
        }
      },
      () => {
        if (!cancelled) {
          setImage(null);
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [url]);
  return image;
}

/**
 * The rasterized artwork for the installed effect, keyed on the resolved
 * appearance (Dither shares one raster across appearances). An appearance
 * flip or artwork swap keeps painting the CURRENT raster until the new one
 * resolves — no flash of empty hero.
 */
function useEffectRaster(
  url: string | null,
  effect: NewThreadBackgroundEffect,
  light: boolean,
): RasterData | null {
  const [entry, setEntry] = useState<{ url: string; raster: RasterData } | null>(null);
  useEffect(() => {
    if (url === null || effect === "none") {
      setEntry(null);
      return;
    }
    let cancelled = false;
    effectRaster(url, effect, light).then(
      (raster) => {
        if (!cancelled) {
          setEntry({ url, raster });
        }
      },
      () => {
        if (!cancelled) {
          setEntry(null);
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [url, effect, light]);
  return entry !== null && entry.url === url ? entry.raster : null;
}

/** The raster as a drawable — painted once per raster, then blitted cover-fit. */
function rasterToCanvas(raster: RasterData): HTMLCanvasElement | null {
  if (typeof document === "undefined") {
    return null;
  }
  const canvas = document.createElement("canvas");
  canvas.width = raster.width;
  canvas.height = raster.height;
  const context = canvas.getContext("2d");
  if (context === null) {
    return null;
  }
  context.putImageData(new ImageData(raster.data, raster.width, raster.height), 0, 0);
  return canvas;
}

export function NewThreadBackground({
  artwork,
  viewportHeight,
  heroWidth,
  dissolve,
  effect,
  sidebarTween,
}: NewThreadBackgroundProps) {
  const reduced = useReducedMotion();
  const heroRef = useRef<HTMLDivElement | null>(null);
  const revealCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const cutoutCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const surface = useRootSurfaceTreatment();
  const appearance = useResolvedAppearance();
  const image = useDecodedImage(artwork === null ? null : artwork.url);
  const raster = useEffectRaster(artwork === null ? null : artwork.url, effect, appearance === "light");
  const rasterCanvas = useMemo(() => (raster === null ? null : rasterToCanvas(raster)), [raster]);
  const rendererRef = useRef<HeroBackgroundRenderer | null>(null);
  const schedulerRef = useRef<HeroRenderScheduler | null>(null);
  // The renderer kind lands on the hero root as `data-renderer` — the CPU
  // fallback's raster-window CSS (ticket 57a) is gated on it; the GPU path
  // re-renders per frame, so the bitmap is never frozen behind a window.
  const [rendererKind, setRendererKind] = useState<"webgl" | "cpu-fallback" | null>(null);

  const height = newThreadBackgroundHeight(viewportHeight);
  // shell.rs:5893 — artwork_opacity × new_thread_background_opacity(is_frost);
  // the readiness leg rides the CSS wrapper (nested opacities multiply back
  // to the element formula).
  const heroOpacity = newThreadBackgroundElementOpacity(dissolve, 1, surface);
  // The readiness fade (effects.rs:11-32) is OWNED by the shell-scoped
  // artwork store (ticket 35): `artwork.ready` is its clock's past-arrival
  // flag, so a remount with the SAME id mounts ready (no re-fade — the
  // 120 ms ramp rides the store's own frame source, never this component's
  // lifecycle) and only a NEW id starts cold. `reduced` still snaps the
  // transition off here.

  // The renderer + render scheduler (ticket 65): the renderer paints both
  // passes at a SAMPLED geometry (hero + composer measured together); the
  // scheduler owns WHEN — geometry notes (both ResizeObservers) coalesce to
  // one render per frame, motion (the sidebar tween OR the dock glide — the
  // old gate knew only the sidebar) owns the cadence: the dock pump's
  // post-prepaint hook samples this frame's composer placement (transform-
  // only moves included, the exact frames the ResizeObservers cannot see),
  // the renderer's own rAF rides the sidebar's CSS tween, and the last
  // settle paints exactly once at the measured end geometry. Mount once:
  // the canvases commit their context mode here (WebGL vs 2d), so the
  // renderer lives for the mount, not per source change.
  useLayoutEffect(() => {
    const hero = heroRef.current;
    const revealCanvas = revealCanvasRef.current;
    const cutoutCanvas = cutoutCanvasRef.current;
    if (hero === null || revealCanvas === null || cutoutCanvas === null) {
      return;
    }
    const renderer = createHeroBackgroundRenderer(revealCanvas, cutoutCanvas, {
      // The raster window's width (ticket 57a, CPU fallback only — the GPU
      // path re-renders per frame, so the bitmap is never frozen).
      setRasterWidth: (cssWidth) => {
        hero.style.setProperty("--rb-hero-raster-width", `${cssWidth}px`);
      },
    });
    rendererRef.current = renderer;
    setRendererKind(renderer.kind);
    const sample = (): HeroGeometrySample => {
      const composerSurface = document.getElementById("composer-surface");
      const dpr = Math.max(1, window.devicePixelRatio || 1);
      if (composerSurface === null) {
        return { hero: { x: 0, y: 0, width: 0, height: 0 }, composer: { x: 0, y: 0, width: 0, height: 0 }, dpr };
      }
      const heroRect = hero.getBoundingClientRect();
      const surfaceRect = composerSurface.getBoundingClientRect();
      return {
        hero: { x: heroRect.x, y: heroRect.y, width: heroRect.width, height: heroRect.height },
        composer: { x: surfaceRect.x, y: surfaceRect.y, width: surfaceRect.width, height: surfaceRect.height },
        dpr,
      };
    };
    const scheduler = new HeroRenderScheduler({
      sample,
      render: (geometry) => renderer.render(geometry),
    });
    schedulerRef.current = scheduler;
    // Same-frame contract: rAF fires after this commit's layout effects (the
    // dock prepaint writes the wrapper transform there) and before paint —
    // the source effect below has already uploaded by then.
    const raf = requestAnimationFrame(() => scheduler.noteArtwork());
    const observer =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => scheduler.noteGeometry())
        : null;
    const composerSurface = document.getElementById("composer-surface");
    if (observer !== null && composerSurface !== null) {
      observer.observe(composerSurface);
    }
    // The hero's own geometry (viewport resizes, the seam-drag takeover):
    // absorbed while any motion runs, one coalesced render once settled.
    const heroObserver =
      typeof ResizeObserver !== "undefined" && heroRef.current !== null
        ? new ResizeObserver(() => scheduler.noteGeometry())
        : null;
    if (heroObserver !== null && heroRef.current !== null) {
      heroObserver.observe(heroRef.current);
    }
    return () => {
      rendererRef.current = null;
      schedulerRef.current = null;
      scheduler.dispose();
      renderer.dispose();
      cancelAnimationFrame(raf);
      observer?.disconnect();
      heroObserver?.disconnect();
    };
    // The mount owns the renderer/scheduler/observers — source changes ride
    // the effect below, never a re-mount (the canvases' context modes are
    // committed for the mount's lifetime).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Source changes (artwork, its decoded/rasterized sources, the effect,
  // the surface treatment — the same deps the old remask keyed on): upload
  // the new source into the renderer, then note it — the scheduler renders
  // at the CURRENT geometry, even mid-motion (the raster re-fixes the
  // window), with the rAF deferral preserving the same-frame transform
  // contract. An installed effect paints its RASTER only — the desktop's
  // hero is `Empty` while the raster is cold, never the raw artwork
  // swapping mid-view; `none` paints the decoded raw artwork.
  useLayoutEffect(() => {
    const renderer = rendererRef.current;
    if (renderer === null) {
      return;
    }
    const drawable = effect === "none" ? image : rasterCanvas;
    const source: HeroArtworkSource | null =
      drawable === null || artwork === null
        ? null
        : {
            drawable,
            width: drawable instanceof HTMLImageElement ? drawable.naturalWidth : drawable.width,
            height: drawable instanceof HTMLImageElement ? drawable.naturalHeight : drawable.height,
          };
    renderer.setSource(source);
    const raf = requestAnimationFrame(() => schedulerRef.current?.noteArtwork());
    return () => {
      cancelAnimationFrame(raf);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [artwork, image, rasterCanvas, effect, surface]);

  // The tween's settle (or a drag takeover's disarm): the commit that
  // returns the readiness layer to `inset: 0` has already restored the
  // canvases' boxes to the hero's settled box, so the settle note HERE —
  // inside the same commit, before paint — snaps the raster to the true
  // geometry with no stretched intermediate frame.
  const wasSidebarTweenRef = useRef(false);
  useLayoutEffect(() => {
    if (wasSidebarTweenRef.current && !sidebarTween) {
      schedulerRef.current?.noteSettle();
    }
    wasSidebarTweenRef.current = sidebarTween;
  }, [sidebarTween]);

  if (artwork === null) {
    return null;
  }

  return (
    <div
      className="new-thread-hero"
      ref={heroRef}
      data-effect={effect}
      data-sidebar-tween={sidebarTween ? "1" : "0"}
      data-renderer={rendererKind ?? undefined}
      style={{
        width: `${heroWidth}px`,
        height: `${height}px`,
        // Ticket 57b: during a route glide the dock pump writes the
        // dissolve element opacity as a CSS var on the conversation column
        // (the parity formula, computed per frame); the fallback is this
        // render's value — the last published frame — so a mid-glide render
        // can never clobber the live fade.
        opacity: `var(--rb-dock-hero-opacity, ${heroOpacity})`,
      }}
      aria-hidden="true"
    >
      {/*
        The readiness wrapper keyed on the artwork id: a NEW id mounts with
        data-ready="false" and the store-owned clock flips it a frame later
        (the 120 ms CSS ramp); the SAME id mounts ready and never re-fades —
        a remount keeps its identity, so no transition runs (ticket 35).
        During a sidebar tween this layer becomes the RASTER WINDOW (app.css,
        ticket 57a): fixed at the current raster's width, centered on the
        hero, so the cutout dome (painted at the pill's center = the raster's
        center) tracks the pill for the whole glide with zero re-rasters.
      */}
      <div
        className="new-thread-hero-readiness"
        key={String(artwork.id)}
        data-ready={artwork.ready ? "true" : "false"}
        data-reduced={reduced ? "true" : "false"}
      >
        {/*
          The reveal pass: the same cover-fit artwork at
          CUTOUT_REVEAL_OPACITY (0.5) with only the shared bottom fade — its
          exclusion sits below the image, so it fills the cutout's hole
          without changing its shape.
        */}
        <canvas className="new-thread-hero-art new-thread-hero-art--reveal" ref={revealCanvasRef} />
        {/*
          The cutout pass: the single combined min(hole, fade) mask applied
          per pixel — the hole is the smoothstep-over-SDF ramp at the
          composer's live box (radius 26, a hard 8px transparent margin,
          feathered over clamp(0.52·heroHeight, 120, 280), extending from the
          composer's top past the hero's bottom).
        */}
        <canvas className="new-thread-hero-art new-thread-hero-art--cutout" ref={cutoutCanvasRef} />
      </div>
    </div>
  );
}
