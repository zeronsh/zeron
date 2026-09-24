import { describe, expect, it, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import {
  HeroRenderScheduler,
  HeroRemaskGate,
  SIDEBAR_GLIDE_MS,
  SIDEBAR_SETTLE_CAP_MS,
  SidebarTweenSignal,
  remaskDue,
  sidebarTweenActive,
  sidebarTweenSignal,
  type HeroGeometrySample,
} from "../src/lib/sidebar-tween";
import { dockGlideSignal, writeDockGlideVars } from "../src/lib/dock-glide";
import { ManualFramePump, ProposedDockSignal } from "./helpers/hero-renderer-prototype";
import { ColumnWidthPublication } from "../src/routes/chat-page";

/**
 * The sidebar tween's settle contract (ticket 57a), the same three layers
 * the page wires: the SIGNAL (arm at the flip, settle at `transitionend`),
 * the remask cadence's ONE predicate (`remaskDue` — geometry ticks are
 * absorbed while the tween runs; settle and artwork changes always remask),
 * and the CSS artifact the glide actually rides (the hero's width
 * transition + the readiness layer's raster window, both scoped to the
 * tween flag and killed by the reduce/resizing blocks).
 */

afterEach(() => {
  sidebarTweenSignal.settle();
});

describe("remaskDue — the settle predicate (ticket 57 §2.2)", () => {
  it("settle and artwork/effect changes always remask", () => {
    expect(remaskDue(false, "settle")).toBe(true);
    expect(remaskDue(true, "settle")).toBe(true);
    expect(remaskDue(false, "artwork")).toBe(true);
    expect(remaskDue(true, "artwork")).toBe(true);
  });

  it("geometry changes remask only once the tween has settled", () => {
    expect(remaskDue(true, "hero-geometry")).toBe(false);
    expect(remaskDue(true, "surface-geometry")).toBe(false);
    expect(remaskDue(false, "hero-geometry")).toBe(true);
    expect(remaskDue(false, "surface-geometry")).toBe(true);
  });
});

describe("SidebarTweenSignal", () => {
  it("arms at the flip and settles at transitionend (idempotent both ways)", () => {
    const signal = new SidebarTweenSignal();
    expect(signal.isActive()).toBe(false);
    signal.arm();
    signal.arm();
    expect(signal.isActive()).toBe(true);
    signal.settle();
    signal.settle();
    expect(signal.isActive()).toBe(false);
  });

  it("the page singleton backs sidebarTweenActive()", () => {
    expect(sidebarTweenActive()).toBe(false);
    sidebarTweenSignal.arm();
    expect(sidebarTweenActive()).toBe(true);
    sidebarTweenSignal.settle();
    expect(sidebarTweenActive()).toBe(false);
  });

  it("subscribe() hands the arm/settle edges to window riders (ticket 63)", () => {
    const signal = new SidebarTweenSignal();
    const edges: boolean[] = [];
    const unsubscribe = signal.subscribe((active) => {
      edges.push(active);
    });
    signal.arm();
    // A mid-glide reversal re-arms — the rider sees the edge and
    // re-captures from the painted state, the signal's retarget semantics.
    signal.arm();
    signal.settle();
    signal.settle();
    expect(edges).toEqual([true, true, false, false]);
    unsubscribe();
    signal.arm();
    expect(edges).toEqual([true, true, false, false]);
  });
});

describe("HeroRemaskGate — zero remasks during the tween, exactly one on settle", () => {
  it("absorbs every per-frame geometry tick the observers report, then snaps once", () => {
    let remasks = 0;
    const gate = new HeroRemaskGate(() => {
      remasks += 1;
    });
    sidebarTweenSignal.arm();
    // One tick per animation frame of the 200ms glide, from BOTH observers
    // (the hero's box animates; a narrow viewport also re-widths the pill).
    const frames = Math.ceil(SIDEBAR_GLIDE_MS / 16);
    for (let frame = 0; frame < frames; frame += 1) {
      gate.note("hero-geometry");
      gate.note("surface-geometry");
    }
    expect(remasks).toBe(0);
    // transitionend: the page settles the signal and the settle commit
    // re-rasters exactly once (same commit, order between the two is the
    // commit's own).
    gate.note("settle");
    sidebarTweenSignal.settle();
    expect(remasks).toBe(1);
    // Settled: real geometry changes paint again.
    gate.note("hero-geometry");
    expect(remasks).toBe(2);
  });

  it("a drag takeover mid-tween re-opens the geometry path immediately", () => {
    let remasks = 0;
    const gate = new HeroRemaskGate(() => {
      remasks += 1;
    });
    sidebarTweenSignal.arm();
    gate.note("hero-geometry");
    expect(remasks).toBe(0);
    // The disarm: the page settles the signal, the flag-fall commit
    // remasks (the snap to the drag geometry), and the hero's observer tick
    // that follows paints freely.
    sidebarTweenSignal.settle();
    gate.note("settle");
    gate.note("hero-geometry");
    expect(remasks).toBe(2);
  });

  it("an artwork/effect change remasks even mid-tween (the raster re-fixes the window)", () => {
    let remasks = 0;
    const gate = new HeroRemaskGate(() => {
      remasks += 1;
    });
    sidebarTweenSignal.arm();
    gate.note("artwork");
    expect(remasks).toBe(1);
    // Geometry stays absorbed around it.
    gate.note("hero-geometry");
    expect(remasks).toBe(1);
  });

  it("a reduce/phone flip never arms, so geometry remasks at once", () => {
    let remasks = 0;
    const gate = new HeroRemaskGate(() => {
      remasks += 1;
    });
    expect(sidebarTweenActive()).toBe(false);
    gate.note("hero-geometry");
    expect(remasks).toBe(1);
  });
});

/**
 * Ticket 64 §2.4 — the column width's publication policy: every observer
 * tick updates the live channels outside React (the publisher's measured
 * width and the clamped composer target the dock pump reads per frame),
 * while the page's React publication defers exactly one window — the blank
 * canvas under an active sidebar tween — and re-publishes the current final
 * measurement once on settle. The widths below stay under the 768px
 * composer cap so the clamped live target tracks them exactly.
 */
describe("column geometry ticks do not publish page state during shell motion (ticket 64)", () => {
  function harness(hasSelection: boolean) {
    const signal = new SidebarTweenSignal();
    const published: number[] = [];
    const liveTarget = { current: 0 };
    const publication = new ColumnWidthPublication({
      signal,
      hasSelection,
      liveTarget,
      publish: (width) => {
        published.push(width);
      },
    });
    return { signal, published, liveTarget, publication };
  }

  it("defers the blank canvas's ticks while the tween runs, then publishes the final width once on settle", () => {
    const h = harness(false);
    // The initial measurement publishes (idle signal) — the baseline width.
    h.publication.note(700);
    expect(h.published).toEqual([700]);
    expect(h.liveTarget.current).toBe(700);
    h.signal.arm();
    // One tick per animation frame of the 200ms glide: EVERY tick reaches
    // the live target; NONE reaches React.
    const frames = Math.ceil(SIDEBAR_GLIDE_MS / 16);
    for (let frame = 1; frame <= frames; frame += 1) {
      h.publication.note(700 - frame * 10);
    }
    expect(h.liveTarget.current).toBe(700 - frames * 10);
    expect(h.published).toEqual([700]);
    // `transitionend`: exactly one publication, the current final width.
    h.signal.settle();
    expect(h.published).toEqual([700, 700 - frames * 10]);
    // Settled: publication is responsive again.
    h.publication.note(1100);
    expect(h.published).toEqual([700, 700 - frames * 10, 1100]);
    expect(h.liveTarget.current).toBe(768); // clamped to the composer cap
    h.publication.dispose();
  });

  it("a mid-glide reversal keeps the deferral; one settle publishes the current measurement", () => {
    const h = harness(false);
    h.publication.note(700);
    h.signal.arm();
    h.publication.note(660);
    h.publication.note(620);
    // The reversal re-arms — the window stays open, the deferral continues.
    h.signal.arm();
    h.publication.note(650);
    expect(h.liveTarget.current).toBe(650);
    expect(h.published).toEqual([700]);
    h.signal.settle();
    expect(h.published).toEqual([700, 650]);
    h.publication.dispose();
  });

  it("a drag takeover's settle publishes the drag geometry at once, then tracks responsively", () => {
    const h = harness(false);
    h.publication.note(700);
    h.signal.arm();
    h.publication.note(660);
    // The seam drag takes the clock over: the page settles the signal...
    h.signal.settle();
    expect(h.published).toEqual([700, 660]);
    // ...and the drag's own ticks publish as they land.
    h.publication.note(648);
    h.publication.note(636);
    expect(h.published).toEqual([700, 660, 648, 636]);
    h.publication.dispose();
  });

  it("selected-chat publication stays responsive under the armed signal (QueuePanel's feed)", () => {
    const h = harness(true);
    h.publication.note(600);
    h.signal.arm();
    h.publication.note(560);
    h.publication.note(520);
    expect(h.published).toEqual([600, 560, 520]);
    // The settle adds nothing — nothing was deferred.
    h.signal.settle();
    expect(h.published).toEqual([600, 560, 520]);
    h.publication.dispose();
  });

  it("redundant ticks never publish — and a glide back to the published width flushes nothing", () => {
    const h = harness(false);
    h.publication.note(700);
    // The observer's initial duplicate (ResizeObserver fires on observe()).
    h.publication.note(700);
    expect(h.published).toEqual([700]);
    h.signal.arm();
    h.publication.note(660);
    // Reversed all the way back before settle: the endpoint IS the
    // published width, so the settle has nothing to flush.
    h.publication.note(700);
    h.signal.settle();
    expect(h.published).toEqual([700]);
    h.publication.dispose();
  });
});

describe("the settle cap bounds a swallowed transitionend", () => {
  it("is a small slack over the glide, never shorter than it", () => {
    expect(SIDEBAR_GLIDE_MS).toBe(200);
    expect(SIDEBAR_SETTLE_CAP_MS).toBeGreaterThan(0);
    expect(SIDEBAR_GLIDE_MS + SIDEBAR_SETTLE_CAP_MS).toBeGreaterThanOrEqual(SIDEBAR_GLIDE_MS);
  });
});

describe("the CSS artifact (the glide the browser actually runs)", () => {
  const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

  it("the hero's width transition rides the motion tokens, scoped to the tween flag", () => {
    const block = css.match(/\.new-thread-hero\[data-sidebar-tween="1"\]\s*\{[^}]*\}/)?.[0];
    expect(block).toBeDefined();
    expect(block).toMatch(
      /transition:\s*width\s+var\(--rb-motion-resize\)\s+var\(--rb-ease-ease-out\)/,
    );
  });

  it("the readiness layer becomes the fixed raster window during the tween — CPU fallback only", () => {
    // Ticket 65: the raster window is gated to `data-renderer="cpu-fallback"`
    // — the GPU path re-renders per frame at the sampled live geometry, so
    // the bitmap tracks the glide and the frozen window never engages (the
    // growth bands and the settle re-crop snap are the 65b fix).
    const block = css.match(
      /\.new-thread-hero\[data-sidebar-tween="1"\]\[data-renderer="cpu-fallback"\]\s+\.new-thread-hero-readiness\s*\{[^}]*\}/,
    )?.[0];
    expect(block).toBeDefined();
    expect(block).toMatch(/left:\s*50%;/);
    expect(block).toMatch(/right:\s*auto;/);
    expect(block).toMatch(/width:\s*var\(--rb-hero-raster-width,\s*100%\);/);
    expect(block).toMatch(/margin-left:\s*calc\(var\(--rb-hero-raster-width,\s*100%\)\s*\/\s*-2\);/);
    // The un-gated rule (which froze the GPU path's window too) is gone.
    expect(
      css.match(/\.new-thread-hero\[data-sidebar-tween="1"\]\s+\.new-thread-hero-readiness\s*\{/),
    ).toBeNull();
  });

  it("the seam-drag freeze kills the hero's transition with the column's", () => {
    expect(css).toMatch(/:root\[data-rb-resizing\]\s+\.new-thread-hero,/);
  });

  it("the reduced-motion block snaps the hero with the sidebar column", () => {
    // The hero joins the column's kill list; this selector adjacency only
    // exists inside the `prefers-reduced-motion` block.
    expect(css).toMatch(/\.new-thread-hero,\s*\r?\n\s*\.sidebar,\s*\r?\n\s*\.right-pane,/);
  });
});

describe("chat-page carries no per-frame flush path (the grep proof)", () => {
  const source = readFileSync(new URL("../src/routes/chat-page.tsx", import.meta.url), "utf8");

  it("no flushSync import or call remains in the tween path", () => {
    expect(source.includes("flushSync")).toBe(false);
  });

  it("no per-frame pump state remains", () => {
    expect(source.includes("sidebarPump")).toBe(false);
    expect(source.includes("animatedSidebar")).toBe(false);
  });
});

/** A style stand-in for the dock channel writes (the frame hook's target). */
function channelStyle(): { setProperty: (n: string, v: string) => void; removeProperty: (n: string) => void } {
  return {
    setProperty: () => {},
    removeProperty: () => {},
  };
}

describe("HeroRenderScheduler (ticket 65 � the gate's successor, production wiring)", () => {
  // The prototype suite (tests/new-thread-background.test.ts, "stage-A
  // render scheduler prototype") proved the ALGORITHM against stand-in
  // signals; these drive the PRODUCTION class. Only the frame clock is
  // injected (node has no rAF) � the signal/hook options below exercise the
  // production default binding: the singleton sidebar signal, the singleton
  // dock signal, and the real post-prepaint frame registry.

  interface Frame {
    heroWidth: number;
    composerX: number;
  }

  function makeScheduler(overrides: {
    sidebar?: { isActive(): boolean; subscribe(l: (a: boolean) => void): () => void };
    dock?: { isActive(): boolean; subscribe(l: (a: boolean) => void): () => void };
    onDockFrame?: (l: () => void) => () => void;
    frame: { value: Frame };
    renders: Frame[];
    uploads: number[];
    pump: ManualFramePump;
  }): HeroRenderScheduler {
    return new HeroRenderScheduler({
      sample: (): HeroGeometrySample => ({
        hero: { x: 0, y: 0, width: overrides.frame.value.heroWidth, height: 600 },
        composer: { x: overrides.frame.value.composerX, y: 400, width: 160, height: 120 },
        dpr: 1,
      }),
      render: (geometry) => {
        overrides.renders.push({
          heroWidth: geometry.hero.width,
          composerX: geometry.composer.x,
        });
      },
      uploadSource: () => {
        overrides.uploads.push(overrides.uploads.length);
      },
      requestFrame: overrides.pump.requestFrame,
      cancelFrame: overrides.pump.cancelFrame,
      frameId: overrides.pump.frameId,
      sidebar: overrides.sidebar,
      dock: overrides.dock,
      onDockFrame: overrides.onDockFrame,
    });
  }

  it("rides the production singleton signals and the real dock frame hook", () => {
    const pump = new ManualFramePump();
    const frame = { value: { heroWidth: 1200, composerX: 520 } };
    const renders: Frame[] = [];
    const uploads: number[] = [];
    const scheduler = makeScheduler({ frame, renders, uploads, pump });
    try {
      // Sidebar-only glide: the scheduler's own rAF owns the cadence.
      sidebarTweenSignal.arm();
      frame.value = { heroWidth: 1230, composerX: 520 };
      pump.pump();
      frame.value = { heroWidth: 1260, composerX: 520 };
      pump.pump();
      expect(renders).toEqual([
        { heroWidth: 1230, composerX: 520 },
        { heroWidth: 1260, composerX: 520 },
      ]);
      // A geometry note mid-glide is absorbed � the frame cadence owns it.
      scheduler.noteGeometry();
      expect(renders).toHaveLength(2);
      // The dock glide joins: the hook takes over, the rAF stands down.
      dockGlideSignal.arm();
      frame.value = { heroWidth: 1300, composerX: 540 };
      writeDockGlideVars(channelStyle(), {
        transcriptOpacity: 0.5,
        transcriptRise: 4,
        paneOpacity: 1,
        composerWidth: 640,
        heroOpacity: 1,
        chromeNewThread: 0.5,
        chromeSession: 0.5,
      });
      expect(renders).toHaveLength(3);
      expect(renders[2]).toEqual({ heroWidth: 1300, composerX: 540 });
      pump.pump(); // the rAF was cancelled on the dock arm � no double render
      expect(renders).toHaveLength(3);
      // The sidebar settles first � the dock still owns the cadence.
      sidebarTweenSignal.settle();
      frame.value = { heroWidth: 1320, composerX: 540 };
      writeDockGlideVars(channelStyle(), {
        transcriptOpacity: 0.6,
        transcriptRise: 3,
        paneOpacity: 1,
        composerWidth: 640,
        heroOpacity: 1,
        chromeNewThread: 0.6,
        chromeSession: 0.4,
      });
      expect(renders).toHaveLength(4);
      // The LAST settle coalesces: the converged frame already painted this
      // geometry this frame � exactly zero extra renders.
      dockGlideSignal.settle();
      expect(renders).toHaveLength(4);
      // A settled geometry change (a viewport resize) renders once; the
      // redundant duplicate note is deduped.
      frame.value = { heroWidth: 1440, composerX: 620 };
      scheduler.noteGeometry();
      scheduler.noteGeometry();
      expect(renders).toHaveLength(5);
      expect(renders[4]).toEqual({ heroWidth: 1440, composerX: 620 });
    } finally {
      scheduler.dispose();
      dockGlideSignal.settle();
      sidebarTweenSignal.settle();
    }
  });

  it("a mid-flight dock teardown still lands one final paint at the moved geometry", () => {
    const dock = new ProposedDockSignal();
    const pump = new ManualFramePump();
    const frame = { value: { heroWidth: 1200, composerX: 520 } };
    const renders: Frame[] = [];
    const uploads: number[] = [];
    const hookRef: { current: (() => void) | null } = { current: null };
    const registerHook = (listener: () => void): (() => void) => {
      hookRef.current = listener;
      return () => {
        hookRef.current = null;
      };
    };
    const scheduler = makeScheduler({
      dock,
      frame,
      renders,
      uploads,
      pump,
      onDockFrame: registerHook,
    });
    try {
      dock.arm();
      hookRef.current?.(); // one glide frame renders at this frame's placement
      expect(renders).toHaveLength(1);
      // The transform moves again with NO hook call (the loop is torn down
      // mid-flight � the chat page's cleanup settle) � the settle edge must
      // still paint the moved geometry exactly once.
      frame.value = { heroWidth: 1200, composerX: 700 };
      dock.settle();
      expect(renders).toHaveLength(2);
      expect(renders[1]).toEqual({ heroWidth: 1200, composerX: 700 });
    } finally {
      scheduler.dispose();
    }
  });

  it("an artwork change re-uploads and renders even mid-motion, bypassing the coalescer", () => {
    const pump = new ManualFramePump();
    const frame = { value: { heroWidth: 1200, composerX: 520 } };
    const renders: Frame[] = [];
    const uploads: number[] = [];
    const scheduler = makeScheduler({ frame, renders, uploads, pump });
    try {
      sidebarTweenSignal.arm();
      frame.value = { heroWidth: 1230, composerX: 520 };
      pump.pump();
      expect(renders).toHaveLength(1);
      // The same geometry, but a NEW source: the forced render must not be
      // swallowed by the same-geometry dedupe.
      scheduler.noteArtwork();
      expect(uploads).toHaveLength(1);
      expect(renders).toHaveLength(2);
      expect(renders[1]).toEqual({ heroWidth: 1230, composerX: 520 });
    } finally {
      scheduler.dispose();
      sidebarTweenSignal.settle();
    }
  });

  it("dispose cancels the pending frame and turns every entry point into a no-op", () => {
    const pump = new ManualFramePump();
    const frame = { value: { heroWidth: 1200, composerX: 520 } };
    const renders: Frame[] = [];
    const uploads: number[] = [];
    const scheduler = makeScheduler({ frame, renders, uploads, pump });
    sidebarTweenSignal.arm();
    // A frame is pending.
    expect(pump.pendingCount).toBe(1);
    scheduler.dispose();
    expect(pump.pendingCount).toBe(0);
    pump.pump();
    scheduler.noteGeometry();
    scheduler.noteArtwork();
    scheduler.noteSettle();
    expect(renders).toHaveLength(0);
    expect(uploads).toHaveLength(0);
    sidebarTweenSignal.settle();
  });
});
