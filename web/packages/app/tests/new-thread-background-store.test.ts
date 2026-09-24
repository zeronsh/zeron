import { describe, expect, it, vi } from "vitest";
import { DockState, heroLayerMounted } from "../src/lib/composer-dock";
import {
  NEW_THREAD_BACKGROUND_IDB_PATH,
  newThreadBackgroundElementOpacity,
  resolveNewThreadBackground,
} from "../src/lib/new-thread-background";
import { idbBackgroundBlobStore } from "../src/lib/background-blob-store";
import { NewThreadArtworkStore, type NewThreadArtwork } from "../src/state/appearance";
import { UiSettingsStore } from "../src/state/ui-settings";
import type { StorageLike } from "../src/lib/engine-store";

/**
 * Ticket 35 — the shell-scoped new-thread background store and the hero
 * layer's survival across a route change. The store-level cases mirror the
 * desktop's `cold_artwork_fades_in_once_and_warm_navigation_does_not_restart_it`
 * (effects.rs:319-336); the host-level case drives a real `DockState`
 * through ConversationPage's render sequence (`/` → `/chat/$id` → `/`)
 * with the same-render tick (shell.rs:5865-5895). The vitest environment is
 * node, so "mounted ConversationPage" is its pure render body — the mount
 * decision (`heroLayerMounted`) plus the dock's mutable frame, exactly the
 * pieces the page consults per render.
 */

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

interface ArtworkHarness {
  readonly store: NewThreadArtworkStore;
  readonly settings: UiSettingsStore;
  readonly clock: { now: number };
  readonly frames: Array<() => void>;
  readonly prewarms: ReadonlyArray<{ readonly effect: string; readonly appearance: string; readonly url: string }>;
  resolveCount(): number;
}

/** A store wired to fake settings, clock, frame queue, resolve and prewarm. */
function createArtworkHarness(options: { reduced?: boolean } = {}): ArtworkHarness {
  const settings = new UiSettingsStore({ storage: memoryStorage() });
  settings.updateImmediate({
    newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" },
  });
  const clock = { now: 0 };
  const frames: Array<() => void> = [];
  const prewarms: Array<{ effect: string; appearance: string; url: string }> = [];
  const calls = { resolves: 0 };
  const store = new NewThreadArtworkStore({
    settings,
    resolve: (setting) => {
      calls.resolves += 1;
      return Promise.resolve(setting === null ? "/default.png" : `blob:${setting.name}`);
    },
    prewarm: (effect, appearance, url) => {
      prewarms.push({ effect, appearance, url });
    },
    nowMs: () => clock.now,
    schedule: (callback) => {
      frames.push(callback);
    },
    reducedMotion: () => options.reduced ?? false,
  });
  return {
    store,
    settings,
    clock,
    frames,
    prewarms,
    resolveCount: () => calls.resolves,
  };
}

/** Let the store's async resolve land (one macrotask). */
function settle(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

/** Run ONE scheduled frame (the clock advances between calls, not inside). */
function runFrame(frames: Array<() => void>): void {
  frames.shift()?.();
}

describe("cold_artwork_fades_in_once_and_warm_navigation_does_not_restart_it (effects.rs:319-336)", () => {
  it("cold id → 0.5 at 60 ms, 1 at ≥120; the fade loop stops at 1", async () => {
    const { store, clock, frames } = createArtworkHarness();
    await settle();
    // The arrival starts the clock: value 0, not ready, and the frame loop
    // is live (shell.rs:5884-5886 — rAF while artwork && opacity < 1).
    expect(store.readinessValue()).toBe(0);
    expect(store.getSnapshot()).toMatchObject({ url: "blob:wall.png", id: "blob:wall.png", ready: false });
    expect(frames.length).toBe(1);
    clock.now = 60;
    runFrame(frames);
    expect(store.readinessValue()).toBeCloseTo(0.5, 6);
    expect(store.getSnapshot().ready).toBe(true);
    expect(frames.length).toBe(1);
    clock.now = 120;
    runFrame(frames);
    expect(store.readinessValue()).toBe(1);
    expect(store.getSnapshot().ready).toBe(true);
    // The rAF-while-fading rule: no frames once the value reaches 1.
    expect(frames.length).toBe(0);
  });

  it("the same id across a warm navigation stays ready — no re-fade, no re-resolve", async () => {
    const { store, clock, frames, resolveCount } = createArtworkHarness();
    await settle();
    // Warm the artwork fully.
    clock.now = 60;
    runFrame(frames);
    clock.now = 120;
    runFrame(frames);
    expect(store.readinessValue()).toBe(1);
    // A "navigation": hero remounts are subscribe/unsubscribe cycles around
    // the same shell-scoped store — the snapshot never resets.
    const first = store.getSnapshot();
    const unsubscribe = store.subscribe(() => {});
    expect(store.getSnapshot()).toBe(first);
    unsubscribe();
    expect(store.getSnapshot()).toBe(first);
    expect(store.getSnapshot().ready).toBe(true);
    expect(resolveCount()).toBe(1);
    // Still no clock restart far later in the session.
    clock.now = 5000;
    expect(store.readinessValue()).toBe(1);
  });

  it("a NEW id restarts the clock from 0", async () => {
    const { store, settings, clock } = createArtworkHarness();
    await settle();
    clock.now = 120;
    expect(store.readinessValue()).toBe(1);
    settings.updateImmediate({
      newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "other.png" },
    });
    await settle();
    // The new artwork's arrival (a new blob URL identity) starts cold.
    expect(store.getSnapshot().url).toBe("blob:other.png");
    expect(store.readinessValue()).toBe(0);
    expect(store.getSnapshot().ready).toBe(false);
    clock.now += 60;
    expect(store.readinessValue()).toBeCloseTo(0.5, 6);
  });

  it("reduced motion snaps to 1 with no frame loop", async () => {
    const { store, frames } = createArtworkHarness({ reduced: true });
    await settle();
    expect(store.readinessValue()).toBe(1);
    expect(store.getSnapshot().ready).toBe(true);
    expect(frames.length).toBe(0);
  });
});

describe("the prewarm (shell.rs:5846-5859 — decode + raster, both routes)", () => {
  it("runs once per arrival with the current effect and appearance", async () => {
    const { prewarms, settings, resolveCount } = createArtworkHarness();
    await settle();
    expect(prewarms).toEqual([{ effect: "none", appearance: "dark", url: "blob:wall.png" }]);
    // An effect flip re-prewarms and publishes the effect without
    // re-resolving — and the readiness never restarts on an effect change.
    settings.updateImmediate({ newThreadBackgroundEffect: "dither" });
    expect(prewarms.length).toBe(2);
    expect(prewarms[1]).toMatchObject({ effect: "dither", url: "blob:wall.png" });
    expect(resolveCount()).toBe(1);
  });
});

describe("idbBackgroundBlobStore singleton (G19 — stable URL identity)", () => {
  it("hands every resolve the same store instance", () => {
    expect(idbBackgroundBlobStore()).toBe(idbBackgroundBlobStore());
  });

  it("keeps one url per blob revision and retires it on replace", async () => {
    // Node has no IndexedDB: the singleton is the memory stand-in, whose
    // urls stand in for the browser's object URLs (size-keyed).
    const blobs = idbBackgroundBlobStore();
    await blobs.put(new Blob(["a"], { type: "image/png" }));
    const first = await blobs.url();
    // Repeated resolves — the hero's, the Appearance row's, a remount's —
    // all share the ONE cachedUrl, so the artwork id is stable per revision.
    expect(await blobs.url()).toBe(first);
    const setting = { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" };
    expect(await resolveNewThreadBackground(setting, "/default.png", blobs)).toBe(first);
    // Replacing the blob retires the old URL exactly once and mints a new
    // identity — no unrevoked-URL growth across resolutions.
    const revoke = vi.spyOn(URL, "revokeObjectURL");
    await blobs.put(new Blob(["bbbb"], { type: "image/png" }));
    expect(revoke).toHaveBeenCalledWith(first);
    const second = await blobs.url();
    expect(second).not.toBe(first);
    expect(await resolveNewThreadBackground(setting, "/default.png", blobs)).toBe(second);
    revoke.mockRestore();
  });
});

describe("the hero layer survives the route change (shell.rs:5865-5895)", () => {
  it("never unmounts across `/` → `/chat/$id` → `/`; the artwork id is continuous", async () => {
    const { store, clock, frames } = createArtworkHarness();
    await settle();
    // Warm the artwork fully (the fade ran out long before navigating).
    clock.now = 60;
    runFrame(frames);
    clock.now = 120;
    runFrame(frames);
    expect(store.getSnapshot()).toMatchObject({ url: "blob:wall.png", ready: true });

    // ConversationPage's render body (chat-page.tsx): the same-render tick
    // precedes the mount decision, so `heroLayerMounted` consumes the frame
    // the tick just produced — never the previous render's state. The commit
    // then runs the layout effect's prepaint (mirrored below as `commit`).
    const dock = new DockState();
    const BOUNDS = { left: 100, top: 600, height: 124 };
    let now = 0;
    const render = (hasSelection: boolean): { heroVisible: boolean; artwork: NewThreadArtwork } => {
      if (dock.frame.docked !== hasSelection) {
        dock.tick(hasSelection, false, now);
      }
      return { heroVisible: heroLayerMounted(hasSelection, dock.frame), artwork: store.getSnapshot() };
    };
    const commit = (hasSelection: boolean): { heroVisible: boolean; artwork: NewThreadArtwork } => {
      const rendered = render(hasSelection);
      dock.prepaint(BOUNDS, 800, false, now);
      return rendered;
    };

    // Boot at `/`: the first-pass tick + prepaint initialize the clock.
    dock.tick(false, false, now);
    dock.prepaint(BOUNDS, 800, false, now);
    expect(render(false).heroVisible).toBe(true);

    // Navigate to /chat/$id — the FIRST navigation render. The pre-fix
    // defect read the stale settled frame here and unmounted the hero.
    const first = render(true);
    expect(first.heroVisible).toBe(true);
    expect(first.artwork.url).not.toBe(null);

    // The pump's per-frame ticks through the 0.420 s docking choreography:
    // while the dock is active the layer is held mounted, the artwork id
    // never changes, and the readiness is already 1 — the element opacity
    // is the dissolve product alone (one fade, not two).
    let framesHeld = 0;
    for (now = 16; now <= 640; now += 16) {
      dock.tick(true, false, now);
      dock.prepaint(BOUNDS, 800, false, now);
      expect(heroLayerMounted(true, dock.frame)).toBe(dock.frame.active);
      if (dock.frame.active) {
        framesHeld += 1;
        const frame = render(true);
        expect(frame.heroVisible).toBe(true);
        expect(frame.artwork.id).toBe(first.artwork.id);
        expect(store.readinessValue()).toBe(1);
        const dissolve = dock.frame.visuals.dissolve;
        expect(newThreadBackgroundElementOpacity(dissolve, 1, "opaque")).toBeCloseTo(1 - dissolve, 6);
      }
    }
    expect(framesHeld).toBeGreaterThan(0);
    // The dissolve finished: the layer drops by the dock's own clock.
    expect(dock.frame.active).toBe(false);
    expect(render(true).heroVisible).toBe(false);

    // Back to `/` (undock): the first navigation render mounts the hero
    // again — with the SAME warm artwork id and readiness 1, so it paints
    // immediately, no readiness fade replaying under the dissolve-in.
    const back = commit(false);
    expect(back.heroVisible).toBe(true);
    expect(back.artwork.id).toBe(first.artwork.id);
    expect(back.artwork.ready).toBe(true);
  });

  it("the stale pre-tick frame is what the same-render tick removes", () => {
    // The old mount decision read the one-render-stale settled frame: on
    // the first navigation render that yields FALSE — the one-commit
    // unmount/remount (and the artwork state loss) this ticket kills.
    const dock = new DockState();
    dock.tick(false, false, 0);
    expect(heroLayerMounted(true, dock.frame)).toBe(false);
  });
});
