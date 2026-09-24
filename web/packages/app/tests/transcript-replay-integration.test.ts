// @vitest-environment jsdom

/**
 * Ticket 69 — the mounted cache→reset baseline regression. The REAL
 * TranscriptView mounts against a REAL TranscriptStore (through the public
 * `store` prop) driven by a controllable fake client and a deferred offline
 * cache, so row derivation, the ToolGroupMotionStore, the baseline effect,
 * and the scroller/StickController all run for real. Spies on the motion
 * store's `sync`/`noteRendered` and the controller's
 * `attach`/`snapToEnd`/`restoreViewport` call through — they record what the
 * mounted consumer actually did (the render-consumed baseline decisions), not
 * just what the store published. The store's own listener log rides along as
 * supplementary evidence.
 *
 * jsdom gaps are stubbed per-suite (the session-provider/composer-reasoning
 * idiom): matchMedia, a deterministic rAF queue, a controllable
 * ResizeObserver, and a mocked performance.now. Scroller geometry
 * (clientHeight/scrollHeight/scrollTop) is stubbed on the element itself —
 * these stubs prove lifecycle/state transitions only, never browser layout.
 * No JSX (createElement), per-file jsdom pragma only.
 */

import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineClient, EngineStatus } from "@zeron/engine-client";
import type { MessagePart, SessionMessageEntry, TranscriptUpdate } from "@zeron/proto";
import { TranscriptView } from "../src/components/transcript";
import { StickController } from "../src/components/stick-controller";
import { ToolGroupMotionStore, type AutomaticFoldTransition, type FoldState } from "../src/lib/tool-motion";
import { ChatArrivalWindow } from "../src/lib/chat-arrival";
import { transcriptFoldCache } from "../src/state/transcript-fold-state";
import { uiSettings } from "../src/state/ui-settings";
import type { OwnTurnAnchor, TranscriptRow } from "../src/lib/transcript";
import {
  echoStore,
  LIVE_SEED_STREAMING_MS,
  savedViewportCache,
  TranscriptStore,
  type TranscriptCache,
  type TranscriptSeed,
} from "../src/state/transcript-store";

// The transcript subtree reads only the resolved appearance (tool chips,
// markdown assets). The real module boots a shell-scoped artwork store whose
// prewarm rides Image.decode — absent in jsdom — so this suite stubs the one
// hook it consumes; baseline synchronization and the motion store are never
// mocked.
vi.mock("../src/state/appearance", () => ({
  useResolvedAppearance: () => "dark" as const,
}));

const CHAT = "chat-replay";

// ── Entries ─────────────────────────────────────────────────────────────────

function toolPart(id: string, command: string): MessagePart {
  return { kind: "tool", id, call: { kind: "exec", command }, isError: false, resolved: true };
}

function userEntry(id: string, text = "hello"): SessionMessageEntry {
  return { id, role: "user", parts: [{ kind: "text", id: `${id}#t`, text }], createdAt: 1_000, deviceId: "dev" };
}

/** A settled assistant entry whose parts are one collapsible tool group. */
function toolEntry(id: string, commands: string[]): SessionMessageEntry {
  return {
    id,
    role: "assistant",
    parts: commands.map((command, ix) => toolPart(`${id}#t${ix}`, command)),
    createdAt: 2_000,
    deviceId: "dev",
    status: null,
  };
}

// ── Controllable client + deferred cache ────────────────────────────────────

interface WatchSlot {
  onItem: (item: TranscriptUpdate, ctx: { generation: number }) => void;
  onEnd?: (error: unknown) => void;
}

class FakeClient {
  readonly status = { state: "connected" } as unknown as EngineStatus;
  readonly watches: WatchSlot[] = [];
  readonly #statusListeners = new Set<(status: EngineStatus) => void>();

  onStatus(listener: (status: EngineStatus) => void): () => void {
    this.#statusListeners.add(listener);
    return () => {
      this.#statusListeners.delete(listener);
    };
  }

  watch(_method: string, _params: unknown, handlers: WatchSlot): { cancel: () => void } {
    this.watches.push(handlers);
    return { cancel: () => {} };
  }

  call(): Promise<never> {
    return Promise.resolve({} as never);
  }

  /** Deliver a frame on the LATEST watch (a resubscribe replaces it). */
  emit(update: TranscriptUpdate, generation = 1): void {
    const slot = this.watches[this.watches.length - 1];
    if (slot === undefined) {
      throw new Error("no watch registered");
    }
    slot.onItem(update, { generation });
  }
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

// ── Deterministic time, rAF, ResizeObserver, matchMedia ─────────────────────

let now = 10_000;
const rafQueue = new Map<number, FrameRequestCallback>();
let rafSeq = 0;

/** Run every queued animation frame once (callbacks may re-arm). */
function pumpRaf(frames = 1): void {
  for (let frame = 0; frame < frames; frame += 1) {
    const callbacks = [...rafQueue.values()];
    rafQueue.clear();
    for (const callback of callbacks) {
      callback(now);
    }
  }
}

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  readonly callback: ResizeObserverCallback;
  readonly observed = new Set<Element>();

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    FakeResizeObserver.instances.push(this);
  }

  observe(el: Element): void {
    this.observed.add(el);
  }

  unobserve(el: Element): void {
    this.observed.delete(el);
  }

  disconnect(): void {
    this.observed.clear();
  }
}

/** Explicitly deliver one measurement batch to observed rows (jsdom has no layout). */
function deliverHeights(heights: Record<string, number>): void {
  for (const observer of FakeResizeObserver.instances) {
    const entries: ResizeObserverEntry[] = [];
    for (const el of observer.observed) {
      const rid = (el as HTMLElement).dataset?.rid;
      const height = rid === undefined ? undefined : heights[rid];
      if (height !== undefined) {
        entries.push({ target: el, borderBoxSize: [{ blockSize: height, inlineSize: 700 }] } as unknown as ResizeObserverEntry);
      }
    }
    if (entries.length > 0) {
      observer.callback(entries, observer as unknown as ResizeObserver);
    }
  }
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
  globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
    rafSeq += 1;
    rafQueue.set(rafSeq, callback);
    return rafSeq;
  }) as typeof requestAnimationFrame;
  globalThis.cancelAnimationFrame = ((handle: number) => {
    rafQueue.delete(handle);
  }) as typeof cancelAnimationFrame;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

// ── Call-through spies on the real consumer seams ───────────────────────────

const realSync = ToolGroupMotionStore.prototype.sync;
const realNoteRendered = ToolGroupMotionStore.prototype.noteRendered;
const realAttach = StickController.prototype.attach;
const realSnapToEnd = StickController.prototype.snapToEnd;
const realRestoreViewport = StickController.prototype.restoreViewport;
const realArrivalArm = ChatArrivalWindow.prototype.arm;

type ArrivalTrace =
  | { event: "sync"; now: number; baseline: boolean; replaying: boolean; groups: [string, number][] }
  | { event: "arm"; now: number }
  | { event: "render"; now: number; rowId: string; open: boolean; bodyHeight: number;
      before: boolean | null | undefined; fold: FoldState | null };

interface SyncRecord {
  readonly baseline: boolean;
  readonly replaying: boolean;
  /** [rowId, toolCount] for each collapsible tool group the surface synced. */
  readonly groups: readonly (readonly [string, number])[];
}

const probe = {
  syncs: [] as SyncRecord[],
  flips: [] as { rowId: string; open: boolean }[],
  snaps: 0,
  restores: 0,
  motion: null as ToolGroupMotionStore | null,
  stick: null as StickController | null,
  el: null as HTMLElement | null,
  trace: [] as ArrivalTrace[],
};

beforeEach(() => {
  now = 10_000;
  probe.syncs = [];
  probe.flips = [];
  probe.snaps = 0;
  probe.restores = 0;
  probe.motion = null;
  probe.stick = null;
  probe.el = null;
  probe.trace = [];
  transcriptFoldCache.clear();
  FakeResizeObserver.instances = [];
  savedViewportCache.clear();
  echoStore.reset();
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.spyOn(ChatArrivalWindow.prototype, "arm").mockImplementation(function (this: ChatArrivalWindow, time: number) {
    probe.trace.push({ event: "arm", now: time });
    realArrivalArm.call(this, time);
  });
  vi.spyOn(ToolGroupMotionStore.prototype, "sync").mockImplementation(function (
    this: ToolGroupMotionStore,
    rows: readonly TranscriptRow[],
    baseline: boolean,
    replaying = false,
  ) {
    probe.motion = this;
    const groups: [string, number][] = [];
    for (const row of rows) {
      if (row.rowKind.kind === "toolGroup") {
        groups.push([row.id, row.rowKind.tools.length]);
      }
    }
    probe.syncs.push({ baseline, replaying, groups });
    probe.trace.push({ event: "sync", now, baseline, replaying, groups });
    return realSync.call(this, rows, baseline, replaying);
  });
  vi.spyOn(ToolGroupMotionStore.prototype, "noteRendered").mockImplementation(function (
    this: ToolGroupMotionStore,
    rowId: string,
    open: boolean,
    bodyHeight: number,
  ) {
    if (rowId.includes("#g")) {
      probe.flips.push({ rowId, open });
    }
    const before = this.revealOf(rowId)?.renderedOpen;
    realNoteRendered.call(this, rowId, open, bodyHeight);
    probe.trace.push({ event: "render", now, rowId, open, bodyHeight, before, fold: this.groupFold(rowId) });
  });
  vi.spyOn(StickController.prototype, "attach").mockImplementation(function (this: StickController, el: HTMLElement) {
    probe.stick = this;
    probe.el = el;
    return realAttach.call(this, el);
  });
  vi.spyOn(StickController.prototype, "snapToEnd").mockImplementation(function (this: StickController) {
    probe.snaps += 1;
    return realSnapToEnd.call(this);
  });
  vi.spyOn(StickController.prototype, "restoreViewport").mockImplementation(function (
    this: StickController,
    scrollTop: number,
    ownTurn: OwnTurnAnchor | null,
    distanceFromBottom: number,
  ) {
    probe.restores += 1;
    return realRestoreViewport.call(this, scrollTop, ownTurn, distanceFromBottom);
  });
});

// ── Mounted harness ─────────────────────────────────────────────────────────

interface Mounted {
  readonly store: TranscriptStore;
  readonly client: FakeClient;
  readonly cacheLoad: { promise: Promise<TranscriptSeed | null>; resolve: (value: TranscriptSeed | null) => void };
  /** The scroller element, captured via the controller's attach. */
  el(): HTMLElement;
  unmount(disposeStore?: boolean): void;
}

const mounted: Mounted[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  vi.restoreAllMocks();
  document.body.replaceChildren();
  rafQueue.clear();
  echoStore.reset();
  savedViewportCache.clear();
});

function mountTranscript(options: { strict?: boolean; reuse?: Mounted } = {}): Mounted {
  const client = options.reuse?.client ?? new FakeClient();
  const cacheLoad = deferred<TranscriptSeed | null>();
  const cache: TranscriptCache = { load: () => cacheLoad.promise, save: () => Promise.resolve() };
  const store = options.reuse?.store ?? new TranscriptStore(client as unknown as EngineClient, CHAT, { cache });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const tree = createElement(TranscriptView, {
    client: client as unknown as EngineClient,
    docId: CHAT,
    deviceId: "dev",
    store,
  });
  act(() => {
    root.render(options.strict === true ? createElement(StrictMode, null, tree) : tree);
  });
  let unmounted = false;
  const handle: Mounted = {
    store,
    client,
    cacheLoad,
    el() {
      if (probe.el === null) {
        throw new Error("the stick controller never attached a scroller");
      }
      return probe.el;
    },
    unmount(disposeStore = true) {
      if (unmounted) {
        return;
      }
      unmounted = true;
      act(() => {
        root.unmount();
      });
      container.remove();
      if (disposeStore) store.dispose();
    },
  };
  mounted.push(handle);
  return handle;
}

/** Deterministic scroller geometry on the element itself (jsdom lays out nothing). */
function stubScrollerGeometry(el: HTMLElement, dims: { clientHeight: number; scrollHeight: number }): void {
  let top = 0;
  Object.defineProperty(el, "clientHeight", { configurable: true, get: () => dims.clientHeight });
  Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => dims.scrollHeight });
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => top,
    set: (value: number) => {
      top = value;
    },
  });
}

/**
 * Settle the deferred cache load (the offline seed) through React. A bare
 * entry array is the stale shape (`savedAtMs: 0`, ticket 80); a seed object
 * passes through verbatim (ticket 81's live case stamps `Date.now()`).
 */
async function settleCache(
  handle: Mounted,
  seed: readonly SessionMessageEntry[] | TranscriptSeed | null,
): Promise<void> {
  const value: TranscriptSeed | null = seed === null || "savedAtMs" in seed ? seed : { entries: seed, savedAtMs: 0 };
  await act(async () => {
    handle.cacheLoad.resolve(value);
  });
}

/** Raw store publications (supplementary; the sync log is the consumer evidence). */
function publicationLog(store: TranscriptStore): { replay: string; generation: number }[] {
  const publications: { replay: string; generation: number }[] = [];
  store.subscribe(() => {
    const snap = store.getSnapshot();
    publications.push({ replay: snap.replay, generation: snap.generation });
  });
  return publications;
}

function startsOf(rowId: string): (number | null)[] {
  const reveal = probe.motion?.revealOf(rowId);
  if (reveal === null || reveal === undefined) {
    throw new Error(`no reveal recorded for ${rowId}`);
  }
  return reveal.starts;
}

// ── The regressions ─────────────────────────────────────────────────────────

function runCachedResetCase(strict: boolean): void {
  it(`cached_transcript_generation_reset_rebaselines_without_rendering_pending${strict ? " (StrictMode)" : ""}`, async () => {
    const cached = [toolEntry("A", ["pwd"])];
    const handle = mountTranscript({ strict });
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    await settleCache(handle, cached);

    // The cache seed painted as the (first) baseline: closed history, one
    // initial restore, no arrival starts.
    expect(probe.snaps).toBe(1);
    expect(startsOf("A#g0").every((start) => start === null)).toBe(true);
    const syncsAfterSeed = probe.syncs.length;

    // The reported sequence: the new generation's pending commit and its
    // reset's populated commit land in ONE task — React coalesces them into a
    // single render, so the intermediate pending snapshot is never rendered.
    const publications = publicationLog(handle.store);
    const reset: SessionMessageEntry[] = [toolEntry("A", ["pwd"]), toolEntry("B", ["ls", "cat"])];
    act(() => {
      handle.client.emit({ contextUsage: null, reset }, 2);
    });

    // Raw evidence: the store DID publish pending then populated on the new
    // generation (supplementary — the gate is the consumer behavior below).
    expect(publications.map((p) => p.replay)).toEqual(["pending", "populated"]);
    expect(publications.map((p) => p.generation)).toEqual([2, 2]);
    // No render ever consumed the pending snapshot: not one sync ran in the
    // transient replaying mode after the seed settled.
    expect(probe.syncs.slice(syncsAfterSeed).every((call) => !call.replaying)).toBe(true);

    // The contract: the reset is recognized as a replay baseline ANYWAY —
    // one baseline sync carrying the reset's own rows, then the live pass.
    const baselineCalls = probe.syncs.filter((call) => call.baseline);
    expect(baselineCalls.length).toBe(2); // the seed, then the reset
    expect(baselineCalls[1]!.groups).toEqual([
      ["A#g0", 1],
      ["B#g0", 2],
    ]);
    // Replayed history gained no arrival starts and no new-group header —
    // and no group ever rendered open because of the reset.
    expect(startsOf("A#g0").every((start) => start === null)).toBe(true);
    expect(startsOf("B#g0").every((start) => start === null)).toBe(true);
    expect(probe.motion!.revealOf("B#g0")!.headerStartedAt).toBeNull();
    expect(probe.flips.filter((flip) => flip.rowId === "B#g0").every((flip) => flip.open === false)).toBe(true);
    // The same-chat reset did not re-run the initial viewport restore.
    expect(probe.snaps).toBe(1);
    expect(probe.restores).toBe(0);
  });
}

describe("mounted cache→reset replay baseline (ticket 69)", () => {
  runCachedResetCase(false);
  runCachedResetCase(true);

  function runSameGenerationCase(strict: boolean): void {
    it(`same_generation_reset_rebaselines_once_and_preserves_post_reset_arrivals${strict ? " (StrictMode)" : ""}`, async () => {
      const handle = mountTranscript({ strict });
      stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
      await settleCache(handle, null);

      // The first live frame: a new-generation reset with one tool of history.
      act(() => {
        handle.client.emit({ contextUsage: null, reset: [toolEntry("A", ["pwd"])] }, 1);
      });
      expect(startsOf("A#g0").every((start) => start === null)).toBe(true);

      // A same-generation resubscribe (the desync recovery shape), its reset
      // — carrying GREW history (a second replayed tool on A) — and a genuine
      // post-reset arrival, all in ONE React batch.
      act(() => {
        handle.store.resubscribe();
        handle.client.emit({ contextUsage: null, reset: [toolEntry("A", ["pwd", "ls"])] }, 1);
        handle.client.emit(
          {
            contextUsage: null,
            upsert: [{ after: "A", entry: toolEntry("C", ["cat"]) }],
            append: [],
            remove: [],
            count: 2,
          },
          1,
        );
      });

      // Exactly one NEW baseline, and it carried the RESET's rows — the
      // coalesced delta's group is not part of the baseline.
      const baselineCalls = probe.syncs.filter((call) => call.baseline);
      expect(baselineCalls.length).toBe(2); // first live reset, then this one
      expect(baselineCalls[1]!.groups).toEqual([["A#g0", 2]]);
      // The replayed growth on A is history: no arrival start for the tool
      // that appeared while resubscribing.
      expect(startsOf("A#g0").every((start) => start === null)).toBe(true);
      // The genuinely post-reset group still arrives as live content.
      const revealC = probe.motion!.revealOf("C#g0")!;
      expect(revealC.headerStartedAt).not.toBeNull();
      expect(revealC.starts.some((start) => start !== null)).toBe(true);
    });
  }

  runSameGenerationCase(false);
  runSameGenerationCase(true);

  it("late_replay_preserves_escaped_anchor_and_own_turn", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    await settleCache(handle, [userEntry("U"), toolEntry("A", ["pwd"])]);
    expect(probe.snaps).toBe(1);
    const el = handle.el();

    // An own send installs the runway; the entry glide runs its frames.
    act(() => {
      echoStore.pushEcho({ messageId: "m1", chatId: CHAT, startedAtMs: Date.now(), text: "follow up", attachmentPaths: [] });
    });
    act(() => {
      pumpRaf(30);
    });
    expect(probe.stick!.ownTurn?.messageId).toBe("m1");

    // The user wheels up: the hold releases, the pin drops, the anchor lands.
    act(() => {
      el.scrollTop = 100;
      el.dispatchEvent(new Event("scroll"));
      pumpRaf(1);
    });
    expect(probe.stick!.pinned).toBe(false);
    expect(probe.stick!.ownTurn?.held).toBe(false);

    // The reset arrives LATE — long past the chat-switch arrival hard cap.
    now += 60_000;
    act(() => {
      handle.store.resubscribe();
      handle.client.emit({ contextUsage: null, reset: [userEntry("U"), toolEntry("A", ["pwd", "ls"])] }, 1);
    });
    act(() => {
      pumpRaf(3);
    });

    // No second initial restore, no re-engaged pin, the released runway
    // intact, and the escaped anchor pixel-stationary.
    expect(probe.snaps).toBe(1);
    expect(probe.restores).toBe(0);
    expect(probe.stick!.pinned).toBe(false);
    expect(probe.stick!.ownTurn?.messageId).toBe("m1");
    expect(probe.stick!.ownTurn?.held).toBe(false);
    expect(el.scrollTop).toBe(100);
    // The late reset baselined the grown history — no arrival starts.
    expect(startsOf("A#g0").every((start) => start === null)).toBe(true);

    // A late measurement batch changes nothing: no restore, no reclassify.
    act(() => {
      deliverHeights({ "A#g0": 60 });
      pumpRaf(2);
    });
    expect(el.scrollTop).toBe(100);
    expect(probe.snaps).toBe(1);
    expect(startsOf("A#g0").every((start) => start === null)).toBe(true);
  });
});

describe("mounted canvas-arrival artifacts (ticket 80)", () => {
  it("cache_seed_downgrades_stale_streaming_so_history_renders_closed", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    // A chat left mid-run: the cache saved the assistant entry with
    // `status: "streaming"`. A previous session's save cannot still be
    // streaming — the stale status would render the last tool group
    // auto-opened ("old tool calls opening") and then visibly close when
    // the authoritative reset settles it.
    const stale: SessionMessageEntry = { ...toolEntry("S", ["pwd", "ls"]), status: "streaming" };
    await settleCache(handle, [userEntry("U"), stale]);

    // The seed the surface received is downgraded to the interrupted-run
    // status, and the snapshot never claims streaming.
    const snap = handle.store.getSnapshot();
    expect(snap.entries[snap.entries.length - 1]!.status).toBe("aborted");
    expect(snap.streaming).toBe(false);
    // The group rendered CLOSED (no autoOpen): the rendered-open records
    // carry open=false, so there is no flip for the live reset to visibly
    // close either.
    expect(probe.flips.filter((flip) => flip.rowId === "S#g0").length).toBeGreaterThan(0);
    expect(
      probe.flips.filter((flip) => flip.rowId === "S#g0").every((flip) => !flip.open),
    ).toBe(true);

    // The authoritative reset lands later and settles the same closed state
    // — no fold tween is seeded (the thought-completion tracker saw the
    // resolved status from the seed already).
    act(() => {
      handle.client.emit(
        { contextUsage: null, reset: [userEntry("U"), toolEntry("S", ["pwd", "ls"])] },
        2,
      );
    });
    expect(
      probe.flips.filter((flip) => flip.rowId === "S#g0").every((flip) => !flip.open),
    ).toBe(true);
  });

  it("reset_after_the_seed_window_fell_settles_pinned_growth_atomically", async () => {
    const handle = mountTranscript();
    const el = handle.el();
    const dims = { clientHeight: 600, scrollHeight: 4000 };
    stubScrollerGeometry(el, dims);
    await settleCache(handle, [userEntry("U"), toolEntry("A", ["pwd"])]);
    // The seed's restore pinned the surface at the end.
    expect(probe.snaps).toBe(1);
    expect(el.scrollTop).toBe(3400);

    // The seed's arrival window has long fallen (past the hard cap).
    now += 60_000;

    // The authoritative reset replaces the seed with grown history: the
    // baseline consume re-arms the arrival window, so the reset's own
    // commit hard-writes the end instead of arming the stick spring (the
    // canvas route's visible "scrolling down" glide).
    act(() => {
      handle.client.emit(
        { contextUsage: null, reset: [userEntry("U"), toolEntry("A", ["pwd", "ls", "cat", "grep"])] },
        2,
      );
    });
    // The grown content measures taller (the scroll height grows with it).
    dims.scrollHeight = 4600;
    act(() => {
      deliverHeights({ "A#g0": 480 });
      pumpRaf(2);
    });
    // The measurement kick stayed inside the re-armed window: the new end
    // landed in ONE hard write — no eased partial steps toward it.
    expect(el.scrollTop).toBe(4600 - 600);
    expect(probe.snaps).toBe(1);
    expect(probe.restores).toBe(0);
  });
});

describe("mounted live-seed streaming (ticket 81)", () => {
  it("a_live_seed_keeps_its_streaming_tail_open_like_the_desktops_state_preserved_switch", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    // A LIVE mid-run switch: the cache saved seconds ago while the run was
    // still streaming. The desktop reads the live doc and mounts the chat
    // with the last tool call OPEN, state preserved, no animation — the
    // seed must match that until the live frame confirms. (Ticket 80's
    // unconditional downgrade made this render closed for ~the live
    // roundtrip, then visibly re-open.)
    const liveTail: SessionMessageEntry = { ...toolEntry("S", ["pwd", "ls"]), status: "streaming" };
    await settleCache(handle, { entries: [userEntry("U"), liveTail], savedAtMs: Date.now() });

    // The seed's streaming status survives verbatim: the snapshot claims
    // streaming (the live-end anchor's tail-follow) and the entries keep it.
    const snap = handle.store.getSnapshot();
    expect(snap.entries[snap.entries.length - 1]!.status).toBe("streaming");
    expect(snap.streaming).toBe(true);
    // The tail group rendered OPEN (autoOpen rides the streaming status).
    expect(probe.flips.filter((flip) => flip.rowId === "S#g0").length).toBeGreaterThan(0);
    expect(probe.flips.filter((flip) => flip.rowId === "S#g0").every((flip) => flip.open)).toBe(true);

    // The authoritative live reset re-affirms the still-streaming tail —
    // still open, no closed→open flip to see.
    act(() => {
      handle.client.emit(
        { contextUsage: null, reset: [userEntry("U"), { ...toolEntry("S", ["pwd", "ls"]), status: "streaming" }] },
        2,
      );
    });
    expect(handle.store.getSnapshot().streaming).toBe(true);
    expect(probe.flips.filter((flip) => flip.rowId === "S#g0").every((flip) => flip.open)).toBe(true);
  });

  it("a_seed_older_than_the_live_window_falls_back_to_the_stale_downgrade", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    // Freshly stamped, but past the LIVE window (a long silent tool call):
    // the seed is treated as a dead session's leftover, ticket 80's rule.
    const streamingTail: SessionMessageEntry = { ...toolEntry("S", ["pwd"]), status: "streaming" };
    await settleCache(handle, {
      entries: [userEntry("U"), streamingTail],
      savedAtMs: Date.now() - (LIVE_SEED_STREAMING_MS + 1),
    });
    const snap = handle.store.getSnapshot();
    expect(snap.entries[snap.entries.length - 1]!.status).toBe("aborted");
    expect(snap.streaming).toBe(false);
  });
});

/**
 * Ticket 82 diagnosis characterizations, promoted from scratch probes.
 * Synthetic protocol inputs prove these mechanisms, not which frames the
 * user's live run delivered. DOM endpoints and effect order are observable;
 * jsdom does not prove browser paint, dock opacity, or measured scroll geometry.
 * The late-reset event is CURRENT behavior, not the desired parity contract.
 */
describe("mounted live-arrival diagnosis (ticket 82)", () => {
  function groupEndpoint(): { open: string | null; height: string } {
    const header = document.getElementById("S#g0-hdr");
    const body = document.querySelector<HTMLElement>('[data-rid="S#g0"] .tool-group-fold');
    expect(header).not.toBeNull();
    expect(body).not.toBeNull();
    return { open: header!.getAttribute("aria-expanded"), height: body!.style.height };
  }

  const textPart: MessagePart = { kind: "text", id: "text", text: "Still working" };

  for (const resetKind of ["text-tail", "settled-entry", "thought-completed"] as const) {
    for (const delay of [25, 1_000]) {
      it(`${resetKind} reset at +${delay}ms closes the seed, with a pre-baseline event only after expiry`, async () => {
        const handle = mountTranscript();
        stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 600 });
        const commits: ReturnType<TranscriptStore["getSnapshot"]>[] = [];
        handle.store.subscribe(() => commits.push(handle.store.getSnapshot()));
        const tail: SessionMessageEntry = {
          ...toolEntry("S", ["pwd", "ls"]), status: "streaming",
          ...(resetKind === "thought-completed"
            ? { parts: [{ kind: "reasoning" as const, id: "r", text: "Thinking through the next step" }] }
            : {}),
        };
        await settleCache(handle, { entries: [tail], savedAtMs: Date.now() });
        expect(groupEndpoint().open).toBe("true");
        expect(parseFloat(groupEndpoint().height)).toBeGreaterThan(0);
        const transitions: AutomaticFoldTransition[] = [];
        probe.motion!.onAutomaticFoldTransition(t => transitions.push(t));
        probe.trace = [];
        now += delay;
        const reset: SessionMessageEntry[] = resetKind === "settled-entry"
          ? [{ ...tail, status: "complete" }, { ...toolEntry("T", ["next"]), status: "streaming" }]
          : [{ ...tail, parts: [...tail.parts, textPart] }];
        act(() => { handle.client.emit({ contextUsage: null, reset }, 1); });

        expect(commits.map(s => [s.replay, s.baseline?.epoch, s.baseline?.provenance, s.streaming])).toEqual([
          ["populated", 1, "seed", true], ["pending", 1, "seed", true], ["populated", 2, "reset", true],
        ]);
        expect(commits.at(-1)!.entries).toEqual(reset);
        expect(probe.syncs.at(-2)?.baseline).toBe(true);
        expect(groupEndpoint()).toEqual({ open: "false", height: "0px" });
        expect(handle.client.watches).toHaveLength(1); // no desync/resubscribe needed
        expect(transitions).toEqual(delay > 500 ? [{ rowId: "S#g0", key: null, toggledAt: now }] : []);
        // The child row's layout effect precedes the parent's arm + sync.
        const closedIx = probe.trace.findIndex(e => e.event === "render" && e.rowId === "S#g0" && !e.open);
        const armIx = probe.trace.findIndex(e => e.event === "arm");
        const baselineIx = probe.trace.findIndex(e => e.event === "sync" && e.baseline);
        expect(closedIx).toBeGreaterThanOrEqual(0);
        expect(armIx).toBeGreaterThan(closedIx);
        expect(baselineIx).toBeGreaterThan(armIx);
        const firstClosed = probe.trace[closedIx];
        if (firstClosed?.event !== "render") { throw new Error("missing close render"); }
        expect(firstClosed.fold?.toggledAt ?? null).toBe(delay > 500 ? now : null);
        // The baseline strips that clock before act returns; it is NOT a
        // surviving 140ms reset-close tween. No thought-detail completion event.
        expect(probe.motion!.groupFold("S#g0")?.toggledAt ?? null).toBeNull();
        expect(probe.motion!.detailFold("S#g0#d0")?.toggledAt ?? null).toBeNull();
        expect(probe.motion!.captureExplicitFolds().groups.size).toBe(0);
        expect(probe.snaps).toBe(1);
      });
    }
  }

  it("identical late reset and tail-tool growth stay open beyond the arrival cap", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 600 });
    const tail = { ...toolEntry("S", ["pwd"]), status: "streaming" as const };
    await settleCache(handle, { entries: [tail], savedAtMs: Date.now() });
    const transitions: AutomaticFoldTransition[] = [];
    probe.motion!.onAutomaticFoldTransition(t => transitions.push(t));
    now += 1_000;
    act(() => { handle.client.emit({ contextUsage: null, reset: [tail] }, 1); });
    now += 100;
    act(() => { handle.client.emit({ contextUsage: null, upsert: [{ after: null,
      entry: { ...tail, parts: [...tail.parts, toolPart("new", "ls")] },
    }], append: [], remove: [], count: 1 }, 1); });
    now += 1_000;
    act(() => { pumpRaf(); });
    expect(groupEndpoint()).toEqual({ open: "true", height: "66px" });
    expect(transitions).toEqual([]);
    expect(probe.flips.filter(f => f.rowId === "S#g0").every(f => f.open)).toBe(true);
  });

  it("post-reset tool growth followed by text stays open until reveal expiry, then tweens closed without a store commit", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 600 });
    const tail = { ...toolEntry("S", ["pwd"]), status: "streaming" as const };
    await settleCache(handle, { entries: [tail], savedAtMs: Date.now() });
    act(() => { handle.client.emit({ contextUsage: null, reset: [tail] }, 1); });
    const transitions: AutomaticFoldTransition[] = [];
    probe.motion!.onAutomaticFoldTransition(t => transitions.push(t));
    now += 100;
    act(() => { handle.client.emit({ contextUsage: null, upsert: [{ after: null, entry: {
      ...tail, parts: [...tail.parts, toolPart("new", "ls"), textPart],
    } }], append: [], remove: [], count: 1 }, 1); });
    expect(groupEndpoint().open).toBe("true");
    expect(startsOf("S#g0")[1]).toBe(now);
    // Ordinary animation frames (not a 600ms main-thread stall).
    for (let i = 0; i < 29; i += 1) {
      now += 16;
      act(() => { pumpRaf(); });
    }
    expect(groupEndpoint()).toEqual({ open: "true", height: "66px" });
    const beforeExpiry = handle.store.getSnapshot();
    now += 16; // connector reveal ends at 480ms; reset window is now 580ms old
    act(() => { pumpRaf(); });
    expect(handle.store.getSnapshot()).toBe(beforeExpiry);
    expect(beforeExpiry.streaming).toBe(true);
    expect(transitions).toEqual([{ rowId: "S#g0", key: null, toggledAt: now }]);
    expect(groupEndpoint()).toEqual({ open: "false", height: "66px" });
    now += 70;
    act(() => { pumpRaf(); });
    expect(parseFloat(groupEndpoint().height)).toBeGreaterThan(0);
    expect(parseFloat(groupEndpoint().height)).toBeLessThan(66);
    now += 80;
    act(() => { pumpRaf(); });
    expect(groupEndpoint()).toEqual({ open: "false", height: "0px" });
  });

  it("remembered closed pins apply on the first seed render, not after a delay", async () => {
    transcriptFoldCache.capture("", CHAT, { groups: new Map([["S#g0", false]]), details: new Map() });
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 600 });
    const tail = { ...toolEntry("S", ["pwd"]), status: "streaming" as const };
    await settleCache(handle, { entries: [tail], savedAtMs: Date.now() });
    expect(groupEndpoint()).toEqual({ open: "false", height: "0px" });
    now += 1_000;
    act(() => { handle.client.emit({ contextUsage: null, reset: [tail] }, 1); });
    expect(probe.flips.filter(f => f.rowId === "S#g0").every(f => !f.open)).toBe(true);
  });

  for (const elapsed of [25, 501]) {
    it(`measured growth at +${elapsed}ms ${elapsed > 500 ? "springs" : "hard-writes"} even with streaming true`, async () => {
      const handle = mountTranscript();
      const dims = { clientHeight: 600, scrollHeight: 4000 };
      stubScrollerGeometry(handle.el(), dims);
      const tail = { ...toolEntry("S", ["pwd"]), status: "streaming" as const };
      await settleCache(handle, { entries: [tail], savedAtMs: Date.now() });
      act(() => { handle.client.emit({ contextUsage: null, reset: [tail] }, 1); });
      expect(handle.el().scrollTop).toBe(3400);
      now += elapsed;
      dims.scrollHeight += 600;
      act(() => { deliverHeights({ "S#g0": 480 }); pumpRaf(); });
      expect(handle.store.getSnapshot().streaming).toBe(true);
      expect(probe.stick!.pinned).toBe(true);
      if (elapsed > 500) {
        expect(handle.el().scrollTop).toBeGreaterThan(3400);
        expect(handle.el().scrollTop).toBeLessThan(4000);
      } else {
        expect(handle.el().scrollTop).toBe(4000);
      }
    });
  }
});

describe("mounted warm-chat return", () => {
  for (const strict of [false, true]) {
    it(`baselines updates received while away, then reveals new live tools (StrictMode=${strict})`, async () => {
      const first = mountTranscript();
      stubScrollerGeometry(first.el(), { clientHeight: 600, scrollHeight: 600 });
      await settleCache(first, null);
      act(() => first.client.emit({ contextUsage: null, reset: [toolEntry("A", ["pwd"])] }));
      first.unmount(false);
      now += 1_000;
      act(() => first.client.emit({ contextUsage: null, upsert: [
        { after: null, entry: toolEntry("A", ["pwd", "ls"]) },
        { after: "A", entry: toolEntry("B", ["cat"]) },
      ], append: [], remove: [], count: 2 }));
      probe.flips = [];
      const returned = mountTranscript({ reuse: first, strict });
      expect(returned.client.watches).toHaveLength(1);
      expect(startsOf("A#g0").every(start => start === null)).toBe(true);
      expect(startsOf("B#g0").every(start => start === null)).toBe(true);
      expect(probe.motion!.revealOf("B#g0")!.headerStartedAt).toBeNull();
      expect(probe.flips.every(flip => !flip.open)).toBe(true);
      now += 1_000;
      act(() => returned.client.emit({ contextUsage: null,
        upsert: [{ after: "B", entry: toolEntry("C", ["new live tool"]) }],
        append: [], remove: [], count: 3 }));
      expect(probe.motion!.revealOf("C#g0")!.headerStartedAt).not.toBeNull();
    });
  }
});

describe("mounted momentum-safe anchor preserve (ticket 85)", () => {
  it("a measurement correction during a fast fling adds the content delta only — never a teleport back", async () => {
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    await settleCache(handle, [userEntry("U"), toolEntry("A", ["pwd"]), toolEntry("B", ["ls", "cat"])]);
    const el = handle.el();
    // Mount the whole (short) list and give it known geometry:
    // U [0,200), A [200,600), B [600,2000).
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
    act(() => {
      pumpRaf(1);
    });
    act(() => {
      deliverHeights({ U: 200, "A#g0": 400, "B#g0": 1400 });
      pumpRaf(2);
    });

    // The user escapes upward; the scroll event captures the escape anchor
    // at scrollTop 700 (row B, offset 100).
    el.scrollTop = 700;
    el.dispatchEvent(new Event("scroll"));
    act(() => {
      pumpRaf(1);
    });

    // A touch fling's compositor momentum advances the viewport PAST the
    // capture before the next scroll event — the commit (a measurement
    // batch landing mid-fling) sees the advanced position.
    el.scrollTop = 900;
    // A row ABOVE the anchor re-measures taller: B's position shifts
    // 600 → 800 (content delta +200).
    act(() => {
      deliverHeights({ "A#g0": 600 });
      pumpRaf(2);
    });

    // The preserve added ONLY the content delta (900 + 200 = 1100): the
    // fling's own motion was never yanked back to the capture-time
    // position (the old absolute write landed at 800 + 100 = 900 — erasing
    // the user's momentum; at real fling speeds this was the mobile
    // "content jumping up and down" every frame).
    expect(el.scrollTop).toBe(1100);
  });
});

describe("conversation width reflow (upstream cbf2ad84)", () => {
  it("a width change re-wraps rows in place — no remount, no re-snap, stale heights drop", async () => {
    // `conversation_width_reflows_streaming_text_without_restarting_animations`
    // (transcript.rs, upstream cbf2ad84), jsdom edition: the row ELEMENT
    // identity is the web's veil Rc — row components own the reveal/fade
    // state, so a remount is exactly a restarted animation — and the
    // observer's re-measure is the reflow. No browser layout exists here, so
    // heights arrive by batch and the cache's lifecycle is the evidence:
    // `noteMeasure` fires only for a batch that REGISTERS (the observer's
    // 0.5px threshold against the cache), which is exactly what a dropped
    // cache lets the reflow do again.
    const noteMeasure = vi.spyOn(ChatArrivalWindow.prototype, "noteMeasure");
    const handle = mountTranscript();
    stubScrollerGeometry(handle.el(), { clientHeight: 600, scrollHeight: 4000 });
    const text = "Streaming content should wrap at the configured conversation width. ".repeat(80);
    const streaming: SessionMessageEntry = {
      id: "reply",
      role: "assistant",
      parts: [{ kind: "text", id: "body", text }],
      createdAt: 2_000,
      deviceId: "dev",
      status: "streaming",
    };
    await settleCache(handle, [userEntry("U"), streaming]);
    const el = handle.el();
    // The user escapes the seed's pin to the top and the rows measure.
    act(() => {
      el.scrollTop = 0;
      el.dispatchEvent(new Event("scroll"));
      pumpRaf(1);
    });
    act(() => {
      deliverHeights({ U: 200, "reply#body.0": 400 });
      pumpRaf(2);
    });
    const row = el.querySelector<HTMLElement>('[data-rid="reply#body.0"]');
    expect(row).not.toBeNull();
    // A re-delivery of the SAME batch is a no-op: the cache holds it.
    noteMeasure.mockClear();
    act(() => {
      deliverHeights({ U: 200, "reply#body.0": 400 });
      pumpRaf(2);
    });
    expect(noteMeasure).not.toHaveBeenCalled();

    // The column narrows: the surface re-renders off the settings store,
    // the streaming row keeps its DOM node, the stick surface and scroll
    // position stand, and no snap fires (the spring is not restarted).
    const stick = probe.stick;
    const snapsBefore = probe.snaps;
    await act(async () => {
      uiSettings.updateImmediate({ transcriptWidth: 560 });
    });
    act(() => {
      pumpRaf(2);
    });
    expect(el.querySelector('[data-rid="reply#body.0"]')).toBe(row);
    expect(probe.stick).toBe(stick);
    expect(probe.snaps).toBe(snapsBefore);
    expect(el.scrollTop).toBe(0);

    // Heights measured under the old column were dropped (the web
    // `list.remeasure()`): the same batch now REGISTERS — the reflow
    // re-measures every row it touches.
    noteMeasure.mockClear();
    act(() => {
      deliverHeights({ U: 200, "reply#body.0": 400 });
      pumpRaf(2);
    });
    expect(noteMeasure).toHaveBeenCalled();

    // Restore the ladder default for the suites that follow.
    await act(async () => {
      uiSettings.updateImmediate({ transcriptWidth: 736 });
    });
  });
});
