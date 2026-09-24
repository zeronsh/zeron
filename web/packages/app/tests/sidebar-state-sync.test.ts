import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { encodeScopedId, RpcError } from "@zeron/engine-client";
import { methods } from "@zeron/engine-client";
import { UiSettingsStore, UI_SETTINGS_STORAGE_KEY, type UiSettings } from "../src/state/ui-settings";
import type { StorageLike } from "../src/lib/engine-store";
import {
  SidebarStateSync,
  reconcileOwnedIds,
  type SidebarStateClient,
} from "../src/lib/sidebar-state-sync";
import type { SidebarStateSnapshot } from "@zeron/proto";

/**
 * Ticket 11's web bridge, scripted like the queue-store suites: a fake
 * engine store holds the engine-side state and fans every mutation out to
 * the attached watchers, so two `SidebarStateSync` instances over the same
 * store are the scripted two-client mirror (no dev server, no live
 * pairing — the verification budget's rule).
 */

type WatchHandlers = {
  onItem: (item: unknown, context: { generation: number }) => void;
  onEnd?: (error: RpcError | undefined) => void;
};

interface FakeWatch {
  readonly method: string;
  readonly params: unknown;
  readonly handlers: WatchHandlers;
  cancelled: boolean;
}

interface Call {
  readonly method: string;
  readonly params: unknown;
}

function scoped(engineKey: string, rawId: string): string {
  return encodeScopedId(engineKey, rawId);
}

/** The engine's own store — `crates/engine/src/sidebar_state.rs` in miniature. */
class FakeEngineStore {
  pinsByProfile: Record<string, string[]> = {};
  sectionsByProfile: Record<string, { id: string; name: string; sessionIds: string[]; collapsed: boolean }[]> = {};
  readonly watchers: FakeWatch[] = [];

  snapshot(): SidebarStateSnapshot {
    return {
      pinsByProfile: JSON.parse(JSON.stringify(this.pinsByProfile)) as Record<string, string[]>,
      sectionsByProfile: JSON.parse(JSON.stringify(this.sectionsByProfile)),
    };
  }

  /** The mutation surface: ordered-list replace, reply with the fresh snapshot. */
  setPins(profileKey: string, sessionIds: string[]): SidebarStateSnapshot {
    if (sessionIds.length === 0) {
      delete this.pinsByProfile[profileKey];
    } else {
      this.pinsByProfile[profileKey] = [...sessionIds];
    }
    return this.publish();
  }

  setSections(
    profileKey: string,
    sections: { id: string; name: string; sessionIds: string[]; collapsed: boolean }[],
  ): SidebarStateSnapshot {
    if (sections.length === 0) {
      delete this.sectionsByProfile[profileKey];
    } else {
      this.sectionsByProfile[profileKey] = sections.map((section) => ({ ...section, sessionIds: [...section.sessionIds] }));
    }
    return this.publish();
  }

  /** `watch` fan-out: every attached stream sees every change. */
  publish(): SidebarStateSnapshot {
    const snapshot = this.snapshot();
    for (const watch of this.watchers) {
      if (!watch.cancelled) {
        watch.handlers.onItem(snapshot, { generation: 1 });
      }
    }
    return snapshot;
  }
}

class FakeClient {
  readonly calls: Call[] = [];
  generation = 1;
  offline = false;
  /** Settles of pending calls for these methods stay parked (LWW scripting). */
  readonly parked = new Set<string>();
  private readonly parkedResolvers: (() => void)[] = [];
  readonly engine: FakeEngineStore;

  constructor(engine: FakeEngineStore) {
    this.engine = engine;
  }

  async call<T>(method: string, params?: unknown): Promise<T> {
    // A real EngineClient never executes a request in the same task as
    // the store write that triggered it — the frame is serialized and
    // sent on a later tick. Defer the same way so the test's synchronous
    // setup (offline flip, parked set) applies before the call runs.
    await new Promise((resolve) => setTimeout(resolve, 0));
    this.calls.push({ method, params });
    if (this.offline) {
      throw new RpcError("transport", "Engine is offline; reconnecting");
    }
    if (this.parked.has(method)) {
      await new Promise<void>((resolve) => {
        this.parkedResolvers.push(resolve);
      });
    }
    const { profileKey } = params as { profileKey: string };
    if (method === methods.SET_SIDEBAR_PINS) {
      const { sessionIds } = params as { sessionIds: string[] };
      return this.engine.setPins(profileKey, sessionIds) as T;
    }
    if (method === methods.SET_SIDEBAR_SECTIONS) {
      const { sections } = params as { sections: { id: string; name: string; sessionIds: string[]; collapsed: boolean }[] };
      return this.engine.setSections(profileKey, sections) as T;
    }
    throw new RpcError("unknown-method", `unknown method: ${method}`, method);
  }

  watch(_method: string, params: unknown, handlers: WatchHandlers): { method: string; cancel: () => void } {
    const fake: FakeWatch = { method: _method, params, handlers, cancelled: false };
    this.engine.watchers.push(fake);
    // The stream's first item is the current value — engine parity.
    handlers.onItem(this.engine.snapshot(), { generation: this.generation });
    return { method: _method, cancel: () => { fake.cancelled = true; } };
  }

  /** Release parked calls; a new generation = a fresh stream instance. */
  release(): void {
    this.generation += 1;
    const resolvers = [...this.parkedResolvers];
    this.parkedResolvers.length = 0;
    for (const resolve of resolvers) {
      resolve();
    }
  }

  /** Deliver a watch frame (a reconnect's first item carries the new generation). */
  frame(snapshot: SidebarStateSnapshot): void {
    for (const watch of this.engine.watchers) {
      if (!watch.cancelled) {
        watch.handlers.onItem(snapshot, { generation: this.generation });
      }
    }
  }
}

function memoryStorage(): StorageLike & { dump(): Record<string, string> } {
  const map = new Map<string, string>();
  return {
    getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
    dump: () => Object.fromEntries(map),
  };
}

function settingsStore(storage: StorageLike = memoryStorage()): UiSettingsStore {
  return new UiSettingsStore({ storage });
}

function setPins(store: UiSettingsStore, profileKey: string, ids: readonly string[]): void {
  const current = { ...store.getSnapshot().sidebarPinnedSessionIdsByProfile };
  if (ids.length === 0) {
    delete current[profileKey];
  } else {
    current[profileKey] = [...ids];
  }
  store.update({ sidebarPinnedSessionIdsByProfile: current }, "immediate");
}

function setSections(
  store: UiSettingsStore,
  profileKey: string,
  sections: readonly { id: string; name: string; sessionIds: string[]; collapsed: boolean }[],
): void {
  const current = { ...store.getSnapshot().sidebarSectionsByProfile };
  if (sections.length === 0) {
    delete current[profileKey];
  } else {
    current[profileKey] = sections.map((section) => ({ ...section }));
  }
  store.update({ sidebarSectionsByProfile: current }, "immediate");
}

/** Let the self-driving write loops settle (each round is one macrotask). */
async function settle(rounds = 12): Promise<void> {
  for (let i = 0; i < rounds; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

describe("SidebarStateSync", () => {
  let engine: FakeEngineStore;
  let client: FakeClient;
  let store: UiSettingsStore;
  let sync: SidebarStateSync;

  beforeEach(() => {
    engine = new FakeEngineStore();
    client = new FakeClient(engine);
    store = settingsStore();
    sync = new SidebarStateSync(store);
    sync.attach("eng-1", client as unknown as SidebarStateClient, "local");
  });

  afterEach(() => {
    sync.dispose();
  });

  it("applies the first frame into the offline cache with scoped ids", () => {
    engine.setPins("local", ["chat-a", "chat-b"]);
    const cache = store.getSnapshot().sidebarPinnedSessionIdsByProfile;
    expect(cache["local"]).toEqual([scoped("eng-1", "chat-a"), scoped("eng-1", "chat-b")]);
  });

  it("mirrors desktop writes live (the watch fan-out)", () => {
    engine.setPins("local", ["chat-a"]);
    engine.setPins("local", ["chat-a", "chat-b"]);
    const cache = store.getSnapshot().sidebarPinnedSessionIdsByProfile;
    expect(cache["local"]).toEqual([scoped("eng-1", "chat-a"), scoped("eng-1", "chat-b")]);
  });

  it("writes local pins through the RPC as raw ids in order", async () => {
    setPins(store, "local", [scoped("eng-1", "b"), scoped("eng-1", "a")]);
    await settle();

    expect(client.calls.map((call) => call.method)).toContain(methods.SET_SIDEBAR_PINS);
    const write = client.calls.find((call) => call.method === methods.SET_SIDEBAR_PINS)!;
    expect(write.params).toEqual({ profileKey: "local", sessionIds: ["b", "a"] });
    // The engine holds the raw ids — the desktop peer of this surface reads
    // exactly this bucket.
    expect(engine.pinsByProfile["local"]).toEqual(["b", "a"]);
  });

  it("keeps localStorage as the cache: writes land in zeron.ui-settings.v1", async () => {
    const storage = memoryStorage();
    const store2 = settingsStore(storage);
    const sync2 = new SidebarStateSync(store2);
    try {
      sync2.attach("eng-1", client as unknown as SidebarStateClient, "local");
      engine.setPins("local", ["chat-a"]);
      const persisted = JSON.parse(storage.dump()[UI_SETTINGS_STORAGE_KEY]!) as UiSettings;
      expect(persisted.sidebarPinnedSessionIdsByProfile["local"]).toEqual([
        scoped("eng-1", "chat-a"),
      ]);
    } finally {
      sync2.dispose();
    }
  });

  it("imports the cached state when the engine has no bucket (pre-ticket migration)", async () => {
    const fresh = settingsStore();
    const freshSync = new SidebarStateSync(fresh);
    try {
      // Pre-ticket localStorage: pins exist, the engine store is empty.
      setPins(fresh, "local", [scoped("eng-1", "legacy-1"), scoped("eng-1", "legacy-2")]);
      freshSync.attach("eng-1", client as unknown as SidebarStateClient, "local");
      await settle();
      // The first (empty) frame imports the cached bucket (LWW: push).
      expect(engine.pinsByProfile["local"]).toEqual(["legacy-1", "legacy-2"]);
    } finally {
      freshSync.dispose();
    }
  });

  it("imports cached sections the same way", async () => {
    const fresh = settingsStore();
    const freshSync = new SidebarStateSync(fresh);
    try {
      setSections(fresh, "local", [
        { id: "s1", name: "Focus", sessionIds: [scoped("eng-1", "chat-a")], collapsed: false },
      ]);
      freshSync.attach("eng-1", client as unknown as SidebarStateClient, "local");
      await settle();
      expect(engine.sectionsByProfile["local"]).toEqual([
        { id: "s1", name: "Focus", sessionIds: ["chat-a"], collapsed: false },
      ]);
    } finally {
      freshSync.dispose();
    }
  });

  it("writes sections through the RPC and mirrors them back", async () => {
    setSections(store, "local", [
      { id: "s1", name: "Focus", sessionIds: [scoped("eng-1", "chat-a")], collapsed: true },
      { id: "s2", name: "Later", sessionIds: [], collapsed: false },
    ]);
    await settle();

    const write = client.calls.find((call) => call.method === methods.SET_SIDEBAR_SECTIONS)!;
    expect(write.params).toEqual({
      profileKey: "local",
      sections: [
        { id: "s1", name: "Focus", sessionIds: ["chat-a"], collapsed: true },
        { id: "s2", name: "Later", sessionIds: [], collapsed: false },
      ],
    });
    // Empty sections are retained engine-side (the drop target survives).
    expect(engine.sectionsByProfile["local"]).toHaveLength(2);
  });

  it("falls back offline: the cache keeps serving, the push retries after reconnect", async () => {
    setPins(store, "local", [scoped("eng-1", "offline-pin")]);
    client.offline = true;
    await settle();
    // The offline fallback: the cache holds the value, the engine never saw it.
    expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "offline-pin"),
    ]);
    expect(engine.pinsByProfile["local"]).toBeUndefined();

    // The engine comes back: a fresh stream (new generation) retries the push.
    client.offline = false;
    client.generation += 1;
    client.frame(engine.snapshot());
    await settle();
    expect(engine.pinsByProfile["local"]).toEqual(["offline-pin"]);
  });

  it("adopts the engine's list when clean and never fights a pending write", async () => {
    // Clean adoption: a desktop-side change arrives, no local mutation pending.
    engine.setPins("local", ["desktop-pin"]);
    expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "desktop-pin"),
    ]);

    // A pending local write (the parked call) is newer than any frame: a
    // frame arriving mid-flight never clobbers the local value.
    setPins(store, "local", [scoped("eng-1", "mine")]);
    client.parked.add(methods.SET_SIDEBAR_PINS);
    await settle();
    engine.setPins("local", ["theirs"]);
    expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "mine"),
    ]);

    // Our write lands last: LWW — the engine ends with our list, and the
    // cache mirrors the engine (our write won).
    client.release();
    await settle();
    expect(engine.pinsByProfile["local"]).toEqual(["mine"]);
    expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "mine"),
    ]);
  });

  it("degrades to local-only on an engine too old to know the surface", async () => {
    sync.dispose();
    engine.watchers.length = 0;
    const failing = new FakeClient(new FakeEngineStore());
    const sync2 = new SidebarStateSync(store);
    try {
      // The watch ends with the engine's unknown-method error before any item.
      const originalWatch = failing.watch.bind(failing);
      failing.watch = (method: string, params: unknown, handlers: WatchHandlers) => {
        const handle = originalWatch(method, params, {
          ...handlers,
          onItem: () => {},
        });
        handlers.onEnd?.(new RpcError("unknown-method", "unknown method: WatchSidebarState", method));
        return handle;
      };
      sync2.attach("eng-1", failing as unknown as SidebarStateClient, "local");
      setPins(store, "local", [scoped("eng-1", "pin")]);
      await settle();
      expect(failing.calls).toEqual([]);
      // The cache still holds the value — the pre-ticket behavior.
      expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
        scoped("eng-1", "pin"),
      ]);
    } finally {
      sync2.dispose();
    }
  });
});

describe("SidebarStateSync two-client mirror over one engine", () => {
  let engine: FakeEngineStore;
  let clientA: FakeClient;
  let clientB: FakeClient;
  let storeA: UiSettingsStore;
  let storeB: UiSettingsStore;
  let syncA: SidebarStateSync;
  let syncB: SidebarStateSync;

  beforeEach(() => {
    engine = new FakeEngineStore();
    clientA = new FakeClient(engine);
    clientB = new FakeClient(engine);
    storeA = settingsStore();
    storeB = settingsStore();
    syncA = new SidebarStateSync(storeA);
    syncB = new SidebarStateSync(storeB);
    syncA.attach("eng-1", clientA as unknown as SidebarStateClient, "local");
    syncB.attach("eng-1", clientB as unknown as SidebarStateClient, "local");
  });

  afterEach(() => {
    syncA.dispose();
    syncB.dispose();
  });

  it("mirrors pins from A to B and back (the scripted two-client run)", async () => {
    // A pins two chats (the web store writes scoped ids of engine eng-1).
    setPins(storeA, "local", [scoped("eng-1", "chat-a"), scoped("eng-1", "chat-b")]);
    await settle();

    // B's cache mirrors the engine's list without any B-side action.
    expect(storeB.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "chat-a"),
      scoped("eng-1", "chat-b"),
    ]);

    // B reorders (an ordered-list replace); A mirrors the new order.
    setPins(storeB, "local", [scoped("eng-1", "chat-b"), scoped("eng-1", "chat-a")]);
    await settle();
    expect(engine.pinsByProfile["local"]).toEqual(["chat-b", "chat-a"]);
    expect(storeA.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "chat-b"),
      scoped("eng-1", "chat-a"),
    ]);
  });

  it("resolves concurrent writes last-write-wins on the engine", async () => {
    // Both clients write; the last call to land owns the engine's bucket.
    clientA.parked.add(methods.SET_SIDEBAR_PINS);
    setPins(storeA, "local", [scoped("eng-1", "a-only")]);
    await settle();
    setPins(storeB, "local", [scoped("eng-1", "b-1"), scoped("eng-1", "b-2")]);
    await settle();

    // B's write landed while A's was parked: the engine holds B's list, and
    // A (its write still pending) keeps its own newer intent.
    expect(engine.pinsByProfile["local"]).toEqual(["b-1", "b-2"]);
    expect(storeA.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "a-only"),
    ]);

    // A's write lands after: LWW — the engine holds A's list, B adopts it.
    clientA.release();
    await settle();
    expect(engine.pinsByProfile["local"]).toEqual(["a-only"]);
    expect(storeB.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
      scoped("eng-1", "a-only"),
    ]);
  });

  it("mirrors sections between clients, scoped on both sides", async () => {
    setSections(storeA, "local", [
      { id: "s1", name: "Focus", sessionIds: [scoped("eng-1", "chat-a")], collapsed: false },
    ]);
    await settle();
    expect(storeB.getSnapshot().sidebarSectionsByProfile["local"]).toEqual([
      { id: "s1", name: "Focus", sessionIds: [scoped("eng-1", "chat-a")], collapsed: false },
    ]);
    // B renames: the engine holds the new name and A mirrors it.
    setSections(storeB, "local", [
      { id: "s1", name: "Deep work", sessionIds: [scoped("eng-1", "chat-a")], collapsed: true },
    ]);
    await settle();
    expect(storeA.getSnapshot().sidebarSectionsByProfile["local"]).toEqual([
      { id: "s1", name: "Deep work", sessionIds: [scoped("eng-1", "chat-a")], collapsed: true },
    ]);
  });
});

describe("SidebarStateSync multi-engine slices", () => {
  it("each engine holds only its own ids; frames never clobber the other slice", async () => {
    const engineA = new FakeEngineStore();
    const engineB = new FakeEngineStore();
    const clientA = new FakeClient(engineA);
    const clientB = new FakeClient(engineB);
    const store = settingsStore();
    const sync = new SidebarStateSync(store);
    try {
      sync.attach("eng-a", clientA as unknown as SidebarStateClient, "local");
      sync.attach("eng-b", clientB as unknown as SidebarStateClient, "local");
      await settle();

      // One merged bucket, ids from two engines (the fleet's shared
      // "local" profile).
      setPins(store, "local", [
        scoped("eng-a", "a1"),
        scoped("eng-b", "b1"),
        scoped("eng-a", "a2"),
      ]);
      await settle();

      // Each engine's store holds exactly its own ids, in merged order —
      // no cross-engine merge.
      expect(engineA.pinsByProfile["local"]).toEqual(["a1", "a2"]);
      expect(engineB.pinsByProfile["local"]).toEqual(["b1"]);

      // A desktop client writes engine A's bucket: the web reconciles A's
      // slice (A's ids take A's positions, in engine order) and never
      // touches B's ids.
      engineA.setPins("local", ["a2", "desktop-a"]);
      expect(store.getSnapshot().sidebarPinnedSessionIdsByProfile["local"]).toEqual([
        scoped("eng-a", "a2"),
        scoped("eng-b", "b1"),
        scoped("eng-a", "desktop-a"),
      ]);
      // And B's engine still holds exactly its own slice.
      expect(engineB.pinsByProfile["local"]).toEqual(["b1"]);
    } finally {
      sync.dispose();
    }
  });
});

describe("reconcileOwnedIds", () => {
  const owner = (id: string) => id.startsWith("a:");

  it("replaces owned positions in order and keeps foreign slots", () => {
    expect(reconcileOwnedIds(["a:1", "f:1", "a:2"], ["a:x", "a:y"], owner)).toEqual([
      "a:x",
      "f:1",
      "a:y",
    ]);
  });

  it("drops owned slots the engine no longer lists", () => {
    expect(reconcileOwnedIds(["a:1", "f:1", "a:2"], ["a:9"], owner)).toEqual(["a:9", "f:1"]);
  });

  it("appends leftover arrivals and is idempotent", () => {
    const once = reconcileOwnedIds(["f:1"], ["a:1", "a:2"], owner);
    expect(once).toEqual(["f:1", "a:1", "a:2"]);
    expect(reconcileOwnedIds(once, ["a:1", "a:2"], owner)).toEqual(once);
  });
});

describe("SidebarStateSync rpc param shapes", () => {
  it("routes through one engine's client with no targetDeviceId", async () => {
    const engine = new FakeEngineStore();
    const client = new FakeClient(engine);
    const store = settingsStore();
    const sync = new SidebarStateSync(store);
    try {
      sync.attach("eng-1", client as unknown as SidebarStateClient, "local");
      setPins(store, "local", [scoped("eng-1", "chat-a")]);
      await settle();
      for (const call of client.calls) {
        expect(Object.keys(call.params as Record<string, unknown>)).not.toContain("targetDeviceId");
      }
    } finally {
      sync.dispose();
    }
  });
});
