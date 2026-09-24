import { describe, expect, it, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import {
  DOCK_GLIDE_DOCK_SECONDS,
  DOCK_GLIDE_UNDOCK_SECONDS,
  DockState,
  dockFrameSettled,
  dockVisualsSettled,
  type DockFrame,
} from "../src/lib/composer-dock";
import { newThreadBackgroundElementOpacity } from "../src/lib/new-thread-background";
import { SidebarTweenSignal } from "../src/lib/sidebar-tween";
import { ColumnWidthPublication } from "../src/routes/chat-page";
import {
  DOCK_GLIDE_VARS,
  DockGlideSignal,
  DockMountSequencer,
  clearDockGlideVars,
  dockGlideActive,
  dockGlideChannels,
  dockGlideSignal,
  onDockGlideFrame,
  writeDockGlideVars,
} from "../src/lib/dock-glide";

/**
 * The dock pump's de-Reacted contract (ticket 57b, §2.3), the same layers
 * the page wires: the CHANNEL SET (one frame in, the CSS-var values out —
 * the DOM writes the loop performs per animation frame), the PHASE
 * SEQUENCER (the chrome rows' mount crossings — the glide's ONLY React
 * state writes, discrete per-navigation events that keep the rows
 * unmounted at channel 0), and the settle handoff (the vars the converged
 * values ride until the settle commit clears them). The glide simulation
 * is the node-env-honest render-count proof: the DOM writes scale with the
 * frame count, the state writes do not.
 */

afterEach(() => {
  dockGlideSignal.settle();
});

/** A recording stand-in for the column's style (the DOM half's target). */
function recordingStyle(): { ops: string[]; setProperty: (n: string, v: string) => void; removeProperty: (n: string) => void } {
  const ops: string[] = [];
  return {
    ops,
    setProperty: (name, value) => {
      ops.push(`${name}=${value}`);
    },
    removeProperty: (name) => {
      ops.push(`-${name}`);
    },
  };
}

/** One 60 Hz frame in ms. */
const FRAME_MS = 1000 / 60;

/**
 * Run one full glide the way the page's loop does (the desktop's per-paint
 * order, minus the DOM): a settling tick + prepaint so the clock and the
 * position exist, the flip tick that arms the choreography, then one tick
 * per frame with the channel writes and the mount-crossing detection —
 * publishing the live frame exactly like the page's `setDockFrameState`.
 */
function runGlide(docked: boolean): {
  frames: number;
  stateWrites: number;
  domWrites: number;
  publishFrames: number[];
  mountedSelectors: boolean[];
  mountedFooter: boolean[];
} {
  const dock = new DockState();
  const mounts = new DockMountSequencer();
  const style = recordingStyle();
  let stateWrites = 0;
  let domWrites = 0;
  const publishFrames: number[] = [];
  const mountedSelectors: boolean[] = [];
  const mountedFooter: boolean[] = [];
  let t = 1000;
  // The shell's first pass: the clock stamps and prepaint initializes the
  // position (the page's first commit) — the choreography needs both.
  dock.tick(!docked, false, t);
  dock.prepaint({ left: 100, top: docked ? 640 : 300, height: 76 }, 800, false, t);
  t += FRAME_MS;
  // The navigation commit's render-phase tick (ticket 35) — arms the glide.
  dock.tick(docked, false, t);
  let index = 0;
  for (;;) {
    t += FRAME_MS;
    const frame = dock.tick(docked, false, t);
    mountedSelectors.push(frame.visuals.selectors > 0);
    mountedFooter.push(frame.visuals.footer > 0);
    writeDockGlideVars(style, dockGlideChannels(frame, 1, 600, "opaque"));
    domWrites += DOCK_GLIDE_VARS.length;
    if (mounts.crossing(frame)) {
      stateWrites += 1;
      publishFrames.push(index);
    }
    index += 1;
    if (!frame.active && dock.paneProgress() === null) {
      // The settle publish — the glide's LAST state write.
      stateWrites += 1;
      break;
    }
  }
  return { frames: index, stateWrites, domWrites, publishFrames, mountedSelectors, mountedFooter };
}

describe("dockGlideChannels — the parity formulas the DOM writes carry", () => {
  it("maps the frame's channels exactly (the rise, the hero product, the chrome pair)", () => {
    const frame: DockFrame = {
      amount: 0.5,
      docked: true,
      active: true,
      visuals: { transcript: 0.25, selectors: 0.5, footer: 0, dissolve: 0.6 },
    };
    const channels = dockGlideChannels(frame, 0.8, 640, "frosted");
    expect(channels.transcriptOpacity).toBe(0.25);
    expect(channels.transcriptRise).toBeCloseTo(8 * (1 - 0.25));
    expect(channels.paneOpacity).toBe(0.8);
    expect(channels.composerWidth).toBe(640);
    expect(channels.heroOpacity).toBeCloseTo(newThreadBackgroundElementOpacity(0.6, 1, "frosted"));
    expect(channels.chromeNewThread).toBe(0.5);
    expect(channels.chromeSession).toBe(0);
  });

  it("the settled frames map to the settled channels (the fallbacks the JSX holds between glides)", () => {
    for (const docked of [false, true]) {
      const frame = dockFrameSettled(docked);
      const channels = dockGlideChannels(frame, 1, 600, "opaque");
      expect(channels.transcriptOpacity).toBe(docked ? 1 : 0);
      expect(channels.transcriptRise).toBeCloseTo(docked ? 0 : 8);
      expect(channels.heroOpacity).toBeCloseTo(docked ? 0 : 1);
      expect(channels.chromeNewThread).toBe(docked ? 0 : 1);
      expect(channels.chromeSession).toBe(docked ? 1 : 0);
    }
  });
});

describe("the DOM half — write/clear roundtrip", () => {
  it("writes every var with its value (units on the lengths) in one frame", () => {
    const style = recordingStyle();
    const frame = dockFrameSettled(true);
    writeDockGlideVars(style, dockGlideChannels(frame, 0.5, 612.25, "opaque"));
    expect(style.ops).toEqual([
      "--rb-dock-transcript-opacity=1",
      "--rb-dock-transcript-rise=0px",
      "--rb-dock-pane-opacity=0.5",
      "--rb-dock-composer-width=612.25px",
      "--rb-dock-hero-opacity=0",
      "--rb-dock-chrome-new=0",
      "--rb-dock-chrome-session=1",
    ]);
  });

  it("clear removes exactly the vars the glide wrote (the settle handoff)", () => {
    const style = recordingStyle();
    writeDockGlideVars(style, dockGlideChannels(dockFrameSettled(false), 1, 600, "opaque"));
    style.ops.length = 0;
    clearDockGlideVars(style);
    expect(style.ops).toEqual(DOCK_GLIDE_VARS.map((name) => `-${name}`));
  });

  it("a null target is a no-op both ways (the column can be unmounted)", () => {
    expect(() => {
      writeDockGlideVars(null, dockGlideChannels(dockFrameSettled(true), 1, 600, "opaque"));
      clearDockGlideVars(null);
    }).not.toThrow();
  });
});

describe("DockMountSequencer — the phase events (the glide's only state writes)", () => {
  it("the first feed initializes from the pre-glide mounts and never emits", () => {
    const mounts = new DockMountSequencer();
    const settled = dockFrameSettled(false);
    expect(mounts.crossing(settled)).toBe(false);
    // The flip frame's visuals equal the pre-glide settled ones — no event.
    expect(mounts.crossing({ ...settled, active: true })).toBe(false);
  });

  it("emits exactly when a chrome mount boolean flips — never on value-only frames", () => {
    const mounts = new DockMountSequencer();
    const settled = dockFrameSettled(false);
    mounts.crossing(settled);
    // A value-only frame (the channel animates but stays > 0): no event.
    expect(
      mounts.crossing({
        ...settled,
        active: true,
        visuals: { ...settled.visuals, selectors: 0.42, transcript: 0.3, dissolve: 0.2 },
      }),
    ).toBe(false);
    // The selectors channel hits 0 — the row unmounts: ONE event.
    expect(
      mounts.crossing({
        ...settled,
        active: true,
        visuals: { ...settled.visuals, selectors: 0, transcript: 0.8, dissolve: 0.4 },
      }),
    ).toBe(true);
    // Staying unmounted: no event.
    expect(
      mounts.crossing({
        ...settled,
        active: true,
        visuals: { ...settled.visuals, selectors: 0, transcript: 0.9, dissolve: 0.5 },
      }),
    ).toBe(false);
    // The footer channel leaves 0 — the row mounts: ONE event.
    expect(
      mounts.crossing({
        ...settled,
        docked: true,
        active: true,
        visuals: { ...settled.visuals, selectors: 0, footer: 0.02, transcript: 0.95, dissolve: 0.6 },
      }),
    ).toBe(true);
  });
});

describe("the state-write counter — zero per-frame React writes across a whole glide", () => {
  it("DOCKING (new-thread → chat): the DOM writes scale with frames, the state writes stay discrete", () => {
    const glide = runGlide(true);
    // A real 420 ms glide at 60 Hz: plenty of frames.
    expect(glide.frames).toBeGreaterThanOrEqual(20);
    // Every frame wrote the full channel set to the DOM.
    expect(glide.domWrites).toBe(glide.frames * DOCK_GLIDE_VARS.length);
    // The state writes: the chrome mount crossings + the ONE settle — the
    // old pump wrote one setState pair per frame here (frames × 2).
    expect(glide.publishFrames.length).toBeLessThanOrEqual(2);
    expect(glide.stateWrites).toBe(glide.publishFrames.length + 1);
    expect(glide.stateWrites).toBeLessThanOrEqual(3);
    expect(glide.stateWrites).toBeLessThan(glide.frames / 6);
    // The phase matrix (ticket 36): the rows mount only while their channel
    // is > 0, and the ramps never leave both mounted.
    expect(glide.mountedSelectors.some((mounted) => mounted)).toBe(true);
    expect(glide.mountedFooter.some((mounted) => mounted)).toBe(true);
    for (let i = 0; i < glide.frames; i += 1) {
      expect(glide.mountedSelectors[i] && glide.mountedFooter[i]).toBe(false);
    }
    // The choreography direction: selectors start mounted (the hero), the
    // footer ends mounted (the session).
    expect(glide.mountedSelectors[0]).toBe(true);
    expect(glide.mountedFooter.at(-1)).toBe(true);
    expect(glide.mountedSelectors.at(-1)).toBe(false);
  });

  it("UNDOCKING (chat → new-thread): the footer hands off to the selectors mid-glide", () => {
    const glide = runGlide(false);
    expect(glide.frames).toBeGreaterThanOrEqual(20);
    expect(glide.domWrites).toBe(glide.frames * DOCK_GLIDE_VARS.length);
    // Undocking's crossings are well separated (footer out at 0.18, the
    // selectors in at 0.5 of the 0.47 s glide): exactly two, plus settle.
    expect(glide.publishFrames.length).toBe(2);
    expect(glide.stateWrites).toBe(3);
    // The ordering: the footer unmounts BEFORE the selectors mount — the
    // crossfade's never-duplicate window.
    const footerOut = glide.publishFrames[0] ?? -1;
    const selectorsIn = glide.publishFrames[1] ?? -1;
    expect(footerOut).toBeLessThan(selectorsIn);
    expect(glide.mountedFooter[0]).toBe(true);
    expect(glide.mountedSelectors.at(-1)).toBe(true);
    expect(glide.mountedFooter.at(-1)).toBe(false);
    for (let i = 0; i < glide.frames; i += 1) {
      expect(glide.mountedSelectors[i] && glide.mountedFooter[i]).toBe(false);
    }
  });

  it("the durations the simulation rode match the desktop's shipped constants", () => {
    expect(DOCK_GLIDE_DOCK_SECONDS).toBe(0.42);
    expect(DOCK_GLIDE_UNDOCK_SECONDS).toBe(0.47);
  });
});

describe("DockGlideSignal — the 59/63 defer signal", () => {
  it("arms at the loop's start and settles at the glide's end (idempotent both ways)", () => {
    const signal = new DockGlideSignal();
    expect(signal.isActive()).toBe(false);
    signal.arm();
    signal.arm();
    expect(signal.isActive()).toBe(true);
    signal.settle();
    signal.settle();
    expect(signal.isActive()).toBe(false);
  });

  it("the page singleton backs dockGlideActive()", () => {
    expect(dockGlideActive()).toBe(false);
    dockGlideSignal.arm();
    expect(dockGlideActive()).toBe(true);
    dockGlideSignal.settle();
    expect(dockGlideActive()).toBe(false);
  });
});

/**
 * Ticket 64 §2.4 — the observer/publication integration at the dock pump's
 * width read. The column observer feeds the pump's live composer target on
 * EVERY tick (no React commit), the page publication defers to the sidebar
 * signal's settle edge, and the glide's discrete-write contract (mount
 * crossings + one settle publish) is unchanged by the moving target. The
 * sidebar's 200ms window rides INSIDE the 420ms dock glide here, the
 * co-occurring case the live-ref feed exists for.
 */
describe("the observer-fed composer target (ticket 64 §2.4)", () => {
  it("the pump reads the live target per frame while page publication defers to one settle publish", () => {
    const signal = new SidebarTweenSignal();
    const published: number[] = [];
    const liveTarget = { current: 0 };
    const publication = new ColumnWidthPublication({
      signal,
      hasSelection: false,
      liveTarget,
      publish: (width) => {
        published.push(width);
      },
    });
    const dock = new DockState();
    const mounts = new DockMountSequencer();
    let stateWrites = 0;
    let t = 1000;
    // The shell's first pass (the page's first commit), then the flip.
    dock.tick(false, false, t);
    dock.prepaint({ left: 100, top: 370, height: 76 }, 800, false, t);
    let measured = 640;
    publication.note(measured); // the initial measurement publishes (idle signal)
    expect(published).toEqual([640]);
    t += FRAME_MS;
    dock.tick(true, false, t); // the navigation commit arms the glide
    signal.arm(); // the sidebar flip rides the same window
    let width = dock.layoutWidth(liveTarget.current, false, t);
    const sidebarFrames = Math.ceil(200 / FRAME_MS); // the sidebar's 200ms window
    let frameCount = 0;
    for (;;) {
      t += FRAME_MS;
      frameCount += 1;
      if (frameCount <= sidebarFrames) {
        // The column observer ticks mid-glide: the live target moves with
        // EVERY frame, with zero React publication while the signal is armed.
        measured -= 6;
        publication.note(measured);
      } else if (signal.isActive()) {
        // The sidebar's `transitionend`: ONE flush of the final measurement.
        signal.settle();
      }
      const frame = dock.tick(true, false, t);
      // The pump's width read (chat-page.tsx): the live ref, this frame.
      width = dock.layoutWidth(liveTarget.current, false, t);
      if (mounts.crossing(frame)) {
        stateWrites += 1;
      }
      if (!frame.active && dock.paneProgress() === null) {
        stateWrites += 1; // the settle publish
        break;
      }
    }
    // The sidebar window deferred every one of its ticks...
    expect(signal.isActive()).toBe(false);
    expect(published).toEqual([640, 640 - sidebarFrames * 6]);
    // ...while the pump's per-frame read saw each one...
    expect(liveTarget.current).toBe(measured);
    // ...and the width glide visibly followed the moving target (a stale,
    // render-fed target would have pinned the width at 640).
    expect(width).toBeLessThan(640);
    // The glide ran its full course with the moving target, and the React
    // writes stayed discrete (the 57b contract): crossings + ONE settle.
    expect(frameCount).toBeGreaterThanOrEqual(20);
    expect(stateWrites).toBeLessThanOrEqual(3);
    // Settled with a static target, the width has converged onto it.
    expect(dock.layoutWidth(liveTarget.current, false, t + FRAME_MS)).toBe(measured);
    publication.dispose();
  });
});

describe("the settled visuals the fallbacks hold (36's matrix stays green)", () => {
  it("docked and undocked settle to the non-overlapping chrome pair", () => {
    const undocked = dockVisualsSettled(false);
    expect(undocked.selectors).toBe(1);
    expect(undocked.footer).toBe(0);
    const docked = dockVisualsSettled(true);
    expect(docked.selectors).toBe(0);
    expect(docked.footer).toBe(1);
  });
});

describe("the source carries no per-frame pump state (the grep proof)", () => {
  const page = readFileSync(new URL("../src/routes/chat-page.tsx", import.meta.url), "utf8");
  const composer = readFileSync(new URL("../src/components/composer.tsx", import.meta.url), "utf8");
  const hero = readFileSync(new URL("../src/components/new-thread-background.tsx", import.meta.url), "utf8");

  it("no dockPump per-frame state remains in the page", () => {
    expect(page.includes("dockPump")).toBe(false);
    expect(page.includes("setDockPump")).toBe(false);
  });

  it("the page's setDockFrameState call sites are exactly the four sanctioned publishes", () => {
    // The render-phase tick (35), the safety-net tick, the mount-crossing
    // publish, and the settle publish — nothing per frame.
    expect(page.match(/setDockFrameState\(/g)?.length).toBe(4);
  });

  it("the loop drives the composer imperatively and writes the channel vars", () => {
    expect(page).toMatch(/dockEvaluateRef\.current\?\.\(\)/);
    expect(page).toMatch(/writeDockGlideVars\(/);
    expect(page).toMatch(/new DockMountSequencer\(\)/);
  });

  it("the per-frame channels are consumed as var-with-fallback inline styles", () => {
    expect(page).toMatch(/var\(--rb-dock-transcript-opacity,/);
    expect(page).toMatch(/var\(--rb-dock-transcript-rise,/);
    expect(page).toMatch(/var\(--rb-dock-pane-opacity,/);
    expect(page).toMatch(/var\(--rb-dock-composer-width,/);
    expect(hero).toMatch(/var\(--rb-dock-hero-opacity,/);
    expect(composer).toMatch(/var\(--rb-dock-chrome-new,/);
    expect(composer).toMatch(/var\(--rb-dock-chrome-session,/);
    expect(composer).toMatch(/var\(--rb-dock-pill-height,/);
    expect(composer).toMatch(/var\(--rb-dock-pill-radius,/);
    expect(composer).toMatch(/var\(--rb-dock-box-height,/);
  });

  it("the composer's inner geometry channels are written, consumed with fallbacks, and cleared (ticket 74)", () => {
    // The route-clock inner values ride the same live-var mechanism as the
    // heights, so an active frame never mixes live heights with stale
    // published padding/inset/glide — and the settle commit removes every
    // temporary override (settle, reversal, wizard takeover, reduced motion
    // all funnel through the same [layout]-commit cleanup).
    for (const channel of [
      "--rb-dock-text-pad",
      "--rb-dock-text-glide",
      "--rb-dock-cluster-dy",
      "--rb-dock-cluster-inset",
    ]) {
      expect(composer).toContain(`setProperty("${channel}"`);
      expect(composer).toContain(`var(${channel},`);
      expect(composer).toContain(`removeProperty("${channel}"`);
    }
  });

  it("the composer parks its evaluate pass and reads the live frame (the pump's channels)", () => {
    expect(composer).toMatch(/dockEvaluateRef\.current = \(\) => evaluateRef\.current\(\)/);
    expect(composer).toMatch(/liveDockFrame/);
  });
});

describe("DockGlideSignal.subscribe + the post-prepaint frame hook (ticket 65)", () => {
  it("subscribe hands the arm/settle edges to riders (mirrors SidebarTweenSignal)", () => {
    const signal = new DockGlideSignal();
    const edges: boolean[] = [];
    const unsubscribe = signal.subscribe((active) => edges.push(active));
    signal.arm();
    signal.arm(); // a re-arm notifies too (retarget semantics, like the sidebar's)
    signal.settle();
    signal.settle();
    expect(edges).toEqual([true, true, false, false]);
    unsubscribe();
    signal.arm();
    expect(edges).toEqual([true, true, false, false]);
    signal.settle();
  });

  it("onDockGlideFrame fires at the end of writeDockGlideVars, never on clear, and unsubscribes", () => {
    let calls = 0;
    const unsubscribe = onDockGlideFrame(() => {
      calls += 1;
    });
    const style = recordingStyle();
    const channels = dockGlideChannels(dockFrameSettled(true), 1, 600, "opaque");
    writeDockGlideVars(style, channels);
    expect(calls).toBe(1);
    // A second frame in the same glide fires again � the hook is per write.
    writeDockGlideVars(style, channels);
    expect(calls).toBe(2);
    // The settle handoff never fires it (the glide has converged).
    clearDockGlideVars(style);
    expect(calls).toBe(2);
    unsubscribe();
    writeDockGlideVars(style, channels);
    expect(calls).toBe(2);
  });
});
