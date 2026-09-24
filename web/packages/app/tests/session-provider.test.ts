// @vitest-environment jsdom

/**
 * Mounted EngineSessionProvider lifecycle regressions (ticket 67). The real
 * provider, the real createEngineSession, and real PickerCatalog instances
 * run for real; the fleet module is doubled by a REAL EngineStore (in-memory
 * storage, so pair/pin produce genuine StoredEngine/engines-array identity
 * changes) plus a controllable registry exposing replaceable client/cache
 * objects and a restart that publishes ONLY a registry snapshot. Routing,
 * notifications, settings, and sounds are unrelated to session ownership and
 * are stubbed narrowly. No JSX (createElement), per-file jsdom pragma only.
 */

import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { HarnessDescriptor, Model } from "@zeron/proto";
import { EngineStore, type StoredEngine, type StorageLike } from "../src/lib/engine-store";
import type { EngineSession } from "../src/state/engine-session";
import {
  EngineSessionProvider,
  useEngineRetry,
  useEngineSession,
  useEngineSessions,
} from "../src/state/session-provider";

// ── Controllable doubles ──────────────────────────────────────────────────

const h = vi.hoisted(() => {
  interface Deferred {
    readonly method: string;
    readonly params: unknown;
    readonly resolve: (value: unknown) => void;
    readonly reject: (error: unknown) => void;
  }

  /** Fake engine client: every unary call is deferred; status/close observable. */
  class FakeClient {
    readonly calls: Array<{ method: string; params: unknown }> = [];
    readonly deferreds: Deferred[] = [];
    closeCalls = 0;
    readonly statusListeners = new Set<(status: { state: string }) => void>();

    onStatus(listener: (status: { state: string }) => void): () => void {
      this.statusListeners.add(listener);
      return () => {
        this.statusListeners.delete(listener);
      };
    }

    emitStatus(state: string): void {
      for (const listener of [...this.statusListeners]) {
        listener({ state });
      }
    }

    call(method: string, params?: unknown): Promise<unknown> {
      this.calls.push({ method, params });
      return new Promise((resolve, reject) => {
        this.deferreds.push({ method, params, resolve, reject });
      });
    }

    #take(method: string): Deferred {
      const deferred = this.deferreds.find((entry) => entry.method === method);
      if (deferred === undefined) {
        throw new Error(`no pending ${method} call`);
      }
      this.deferreds.splice(this.deferreds.indexOf(deferred), 1);
      return deferred;
    }

    resolveNext(method: string, value: unknown): void {
      this.#take(method).resolve(value);
    }

    rejectNext(method: string, error: unknown): void {
      this.#take(method).reject(error);
    }

    close(): void {
      this.closeCalls += 1;
    }
  }

  /** Fake watch cache: stable subscribe/getSnapshot for the notification driver. */
  class FakeCache {
    disposeCalls = 0;
    readonly #listeners = new Set<() => void>();

    get listenerCount(): number {
      return this.#listeners.size;
    }

    subscribe(listener: () => void): () => void {
      this.#listeners.add(listener);
      return () => {
        this.#listeners.delete(listener);
      };
    }

    getSnapshot(): null {
      return null;
    }

    dispose(): void {
      this.disposeCalls += 1;
    }
  }

  interface FakeRegistryRow {
    readonly key: string;
    readonly state: string;
  }

  interface FakeRegistrySnapshot {
    readonly engines: readonly FakeRegistryRow[];
    readonly configurationError: string | null;
  }

  interface FakeEntry {
    readonly credential: string;
    readonly client: FakeClient;
    readonly cache: FakeCache;
  }

  /**
   * Controllable registry: current client/cache per engine key are
   * replaceable. `restart` swaps them and publishes ONLY a new registry
   * snapshot — fleet.engines identity stays untouched, the ticket-67 seam.
   * `syncFrom` mirrors the production fleet module's store subscription
   * (state/fleet.ts): pairing changes adopt/replace/drop resources BEFORE
   * React observes the store commit, exactly like syncRegistry.
   */
  class FakeRegistry {
    autoSpawn = true;
    readonly restartCalls: Array<{ key: string; expectedDeviceId: string | null | undefined }> = [];
    readonly #entries = new Map<string, FakeEntry>();
    readonly #listeners = new Set<() => void>();
    #snapshot: FakeRegistrySnapshot = { engines: [], configurationError: null };

    clientFor(key: string): FakeClient | null {
      return this.#entries.get(key)?.client ?? null;
    }

    watchCacheFor(key: string): FakeCache | null {
      return this.#entries.get(key)?.cache ?? null;
    }

    getSnapshot(): FakeRegistrySnapshot {
      return this.#snapshot;
    }

    subscribe(listener: () => void): () => void {
      this.#listeners.add(listener);
      return () => {
        this.#listeners.delete(listener);
      };
    }

    /** The production store→registry wiring peer. */
    syncFrom(engines: readonly { baseUrl: string; credential: string }[]): void {
      if (!this.autoSpawn) {
        return;
      }
      let changed = false;
      for (const engine of engines) {
        const existing = this.#entries.get(engine.baseUrl);
        if (existing === undefined || existing.credential !== engine.credential) {
          this.#entries.set(engine.baseUrl, {
            credential: engine.credential,
            client: new FakeClient(),
            cache: new FakeCache(),
          });
          changed = true;
        }
      }
      for (const key of [...this.#entries.keys()]) {
        if (!engines.some((engine) => engine.baseUrl === key)) {
          this.#entries.delete(key);
          changed = true;
        }
      }
      if (changed) {
        this.publish();
      }
    }

    /** Test seam: adopt resources for an engine whose pairing is already stored. */
    adopt(key: string, credential = ""): { client: FakeClient; cache: FakeCache } {
      const entry: FakeEntry = { credential, client: new FakeClient(), cache: new FakeCache() };
      this.#entries.set(key, entry);
      this.publish();
      return entry;
    }

    /** Test seam: the engine's resources are (temporarily) unavailable. */
    drop(key: string): void {
      this.#entries.delete(key);
      this.publish();
    }

    /** Test seam: a row/status publication over UNCHANGED resource objects. */
    publishRows(): void {
      this.publish();
    }

    restart(key: string, expectedDeviceId?: string | null): void {
      this.restartCalls.push({ key, expectedDeviceId });
      const existing = this.#entries.get(key);
      if (existing === undefined) {
        return;
      }
      this.#entries.set(key, { credential: existing.credential, client: new FakeClient(), cache: new FakeCache() });
      this.publish();
    }

    publish(): void {
      this.#snapshot = {
        engines: [...this.#entries.keys()].sort().map((key) => ({ key, state: "connected" })),
        configurationError: null,
      };
      for (const listener of [...this.#listeners]) {
        listener();
      }
    }
  }

  const cells: { store: import("../src/lib/engine-store").EngineStore | null; registry: FakeRegistry } = {
    store: null,
    registry: new FakeRegistry(),
  };
  return { FakeClient, FakeCache, FakeRegistry, cells };
});

vi.mock("../src/state/fleet", async () => {
  const { useSyncExternalStore } = await import("react");
  const store = (): import("../src/lib/engine-store").EngineStore => {
    if (h.cells.store === null) {
      throw new Error("fleet store double used before beforeEach");
    }
    return h.cells.store;
  };
  const subscribeFleet = (listener: () => void) => store().subscribe(listener);
  const getFleetSnapshot = () => store().getSnapshot();
  const subscribeRegistry = (listener: () => void) => h.cells.registry.subscribe(listener);
  const getRegistrySnapshot = () => h.cells.registry.getSnapshot();
  return {
    useFleet: () => useSyncExternalStore(subscribeFleet, getFleetSnapshot, getFleetSnapshot),
    useFleetRegistry: () => useSyncExternalStore(subscribeRegistry, getRegistrySnapshot, getRegistrySnapshot),
    get engineRegistry() {
      return h.cells.registry;
    },
    get fleetStore() {
      return store();
    },
  };
});

vi.mock("@tanstack/react-router", () => {
  const navigate = (): Promise<void> => Promise.resolve();
  return {
    useRouterState: (options: { select: (state: { location: { pathname: string } }) => unknown }) =>
      options.select({ location: { pathname: "/" } }),
    useNavigate: () => navigate,
  };
});

vi.mock("../src/state/sidebar", () => ({
  useSidebar: () => ({ spaceFilter: null, lastSpaceId: null }),
}));

vi.mock("../src/state/ui-settings", () => {
  const settings = {};
  return { useUiSettings: () => settings };
});

vi.mock("../src/state/transcript-store", () => ({ echoStore: {} }));

vi.mock("../src/state/attention-gate", () => ({
  appAttentionGate: { shouldPlay: () => false },
}));

vi.mock("../src/lib/sounds", () => ({
  playSound: () => {},
  sessionSoundEnabled: () => false,
}));

vi.mock("../src/lib/notifications", () => ({
  ConnectivityNotificationState: class {
    update(): null {
      return null;
    }
  },
  chatBannerTexts: () => ({ title: "", body: "" }),
  connectivityBannerTexts: () => ({ title: "", body: "" }),
  echoSendPending: () => false,
  onChatNotificationClick: () => () => {},
  postBanner: () => {},
  sessionNotificationState: () => ({}),
  soundSince: () => null,
}));

// ── Mounted-provider harness ──────────────────────────────────────────────

const ENGINE_ONE_URL = "http://engine-one.test:8080";
const ENGINE_TWO_URL = "http://engine-two.test:8080";

const HARNESS: HarnessDescriptor = {
  id: "claude-code",
  name: "Claude",
  supportsSteering: true,
  steeringMode: "step-boundary",
  reasoningLevels: ["medium"],
  installed: true,
  enabled: true,
};
const MODEL: Model = { id: "sonnet", label: "Sonnet", reasoningLevels: ["medium"], options: [] };

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

function store(): EngineStore {
  if (h.cells.store === null) {
    throw new Error("fleet store double used before beforeEach");
  }
  return h.cells.store;
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

let signInSeq = 0;

beforeEach(() => {
  const registry = new h.FakeRegistry();
  const engineStore = new EngineStore({
    storage: memoryStorage(),
    now: () => 1_000,
  });
  // The production wiring peer: the registry follows the pairing store.
  engineStore.subscribe(() => registry.syncFrom(engineStore.getSnapshot().engines));
  h.cells.store = engineStore;
  h.cells.registry = registry;
});

interface ProbeSnapshot {
  readonly session: EngineSession | null;
  readonly sessions: ReadonlyMap<string, EngineSession>;
  readonly retry: () => void;
}

interface Mounted {
  readonly observed: { current: ProbeSnapshot };
  unmount(): void;
}

const mounted: Mounted[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

function mountProvider(strict = false): Mounted {
  const observed: { current: ProbeSnapshot } = {
    current: { session: null, sessions: new Map(), retry: () => {} },
  };
  function Probe() {
    const session = useEngineSession();
    const sessions = useEngineSessions();
    const retry = useEngineRetry();
    observed.current = { session, sessions, retry };
    return null;
  }
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const tree = createElement(EngineSessionProvider, null, createElement(Probe));
  act(() => {
    root.render(strict ? createElement(StrictMode, null, tree) : tree);
  });
  let unmounted = false;
  const handle: Mounted = {
    observed,
    unmount() {
      if (unmounted) {
        return;
      }
      unmounted = true;
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  mounted.push(handle);
  return handle;
}

/** Sign in to an engine with a fresh credential, the way the real flow would. */
async function pair(baseUrl: string): Promise<StoredEngine> {
  let engine!: StoredEngine;
  signInSeq += 1;
  await act(async () => {
    engine = await store().signInEngine({
      baseUrl,
      credential: `credential-${signInSeq}`,
      label: "Test browser",
      sessionId: `session-${signInSeq}`,
    });
  });
  return engine;
}

async function pairWithResources(baseUrl: string): Promise<{
  engine: StoredEngine;
  client: InstanceType<typeof h.FakeClient>;
  cache: InstanceType<typeof h.FakeCache>;
}> {
  const engine = await pair(baseUrl);
  const client = h.cells.registry.clientFor(engine.baseUrl);
  const cache = h.cells.registry.watchCacheFor(engine.baseUrl);
  if (client === null || cache === null) {
    throw new Error("registry did not adopt the paired engine's resources");
  }
  return { engine, client, cache };
}

function sessionOf(handle: Mounted, baseUrl: string): EngineSession {
  const session = handle.observed.current.sessions.get(baseUrl);
  if (session === undefined) {
    throw new Error(`no session for ${baseUrl}`);
  }
  return session;
}

// ── The ticket-67 ownership regressions ───────────────────────────────────

describe("EngineSessionProvider resource lifetime", () => {
  it("first identity pin preserves the live mounted picker catalog", async () => {
    const { engine, client, cache } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();

    const before = sessionOf(handle, engine.baseUrl);
    expect(before.engine.deviceId).toBeNull();
    const catalog = before.catalog;
    const disposeSpy = vi.spyOn(catalog, "dispose");
    const listener = vi.fn();
    catalog.subscribe(listener);

    // The first verified EngineInfo pins the identity: same credential, same
    // registry resources, a NEW StoredEngine object and engines array.
    await act(async () => {
      store().pinDevice(engine.baseUrl, "device-1");
    });

    const after = sessionOf(handle, engine.baseUrl);
    expect(after.engine.deviceId).toBe("device-1");
    expect(after.client).toBe(client);
    expect(after.cache).toBe(cache);
    expect(after.catalog).toBe(catalog);
    expect(disposeSpy).not.toHaveBeenCalled();

    // Real loads kicked after the pin still settle and notify.
    let harnessLoad!: Promise<void>;
    await act(async () => {
      harnessLoad = catalog.loadHarnesses();
    });
    let modelLoad!: Promise<void>;
    await act(async () => {
      modelLoad = catalog.loadModels("claude-code");
    });
    expect(client.deferreds.map((deferred) => deferred.method)).toEqual(["ListHarnesses", "ListModels"]);
    await act(async () => {
      client.resolveNext("ListHarnesses", [HARNESS]);
      client.resolveNext("ListModels", [MODEL]);
      await harnessLoad;
      await modelLoad;
    });
    expect(catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
    expect(catalog.getModels("claude-code").rows.map((row) => row.id)).toEqual(["sonnet"]);
    expect(listener).toHaveBeenCalled();
  });

  it("registry restart replaces session resources without a fleet metadata change", async () => {
    const { engine, client: oldClient, cache: oldCache } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();

    const before = sessionOf(handle, engine.baseUrl);
    const enginesBefore = store().getSnapshot().engines;
    const oldDispose = vi.spyOn(before.catalog, "dispose");

    // The engine gate's Retry: a registry restart over untouched fleet
    // metadata. The relay fleet keys engines by device id, so the restart
    // re-pins identity to the key itself.
    await act(async () => {
      handle.observed.current.retry();
    });

    expect(store().getSnapshot().engines).toBe(enginesBefore);
    expect(h.cells.registry.restartCalls).toEqual([{ key: engine.baseUrl, expectedDeviceId: engine.baseUrl }]);

    const after = sessionOf(handle, engine.baseUrl);
    expect(after.engine).toBe(before.engine);
    expect(after.client).toBe(h.cells.registry.clientFor(engine.baseUrl));
    expect(after.client).not.toBe(oldClient);
    expect(after.cache).not.toBe(oldCache);
    expect(after.catalog).not.toBe(before.catalog);
    expect(oldDispose).toHaveBeenCalledTimes(1);

    // Subsequent requests reach ONLY the replacement client; the fresh catalog is usable.
    const current = h.cells.registry.clientFor(engine.baseUrl)!;
    let load!: Promise<void>;
    await act(async () => {
      load = after.catalog.loadHarnesses();
    });
    expect(current.calls.map((call) => call.method)).toEqual(["ListHarnesses"]);
    expect(oldClient.calls).toHaveLength(0);
    await act(async () => {
      current.resolveNext("ListHarnesses", [HARNESS]);
      await load;
    });
    expect(after.catalog.getHarnesses().loaded).toBe(true);
    expect(after.catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
  });

  it("identity pin preserves pending harness and model requests", async () => {
    const { engine, client } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();
    const catalog = sessionOf(handle, engine.baseUrl).catalog;
    const harnessListener = vi.fn();
    catalog.subscribe(harnessListener);
    const modelListener = vi.fn();
    catalog.subscribeModels("claude-code", modelListener);

    // Both requests in flight when the pin lands.
    let harnessLoad!: Promise<void>;
    await act(async () => {
      harnessLoad = catalog.loadHarnesses();
    });
    let modelLoad!: Promise<void>;
    await act(async () => {
      modelLoad = catalog.loadModels("claude-code");
    });
    expect(client.deferreds.map((deferred) => deferred.method)).toEqual(["ListHarnesses", "ListModels"]);
    const callsBeforePin = client.calls.length;

    await act(async () => {
      store().pinDevice(engine.baseUrl, "device-1");
    });
    expect(sessionOf(handle, engine.baseUrl).catalog).toBe(catalog);
    // No replacement calls caused solely by the metadata refresh.
    expect(client.calls).toHaveLength(callsBeforePin);

    // The deferred responses keep their right to land after the pin.
    await act(async () => {
      client.resolveNext("ListHarnesses", [HARNESS]);
      client.resolveNext("ListModels", [MODEL]);
      await harnessLoad;
      await modelLoad;
    });
    expect(catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
    expect(catalog.getModels("claude-code").rows.map((row) => row.id)).toEqual(["sonnet"]);
    // Existing subscribers still receive updates: loading + rows, once each.
    expect(harnessListener).toHaveBeenCalledTimes(2);
    expect(modelListener).toHaveBeenCalledTimes(2);
  });

  it("registry row updates preserve unchanged sessions and catalogs", async () => {
    const { engine, cache } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();
    const before = sessionOf(handle, engine.baseUrl);
    const mapBefore = handle.observed.current.sessions;
    const disposeSpy = vi.spyOn(before.catalog, "dispose");
    expect(cache.listenerCount).toBe(1); // the notification driver's mount subscription

    await act(async () => {
      h.cells.registry.publishRows();
    });
    await act(async () => {
      h.cells.registry.publishRows();
    });
    await act(async () => {
      h.cells.registry.publishRows();
    });

    // Unchanged resources: same map, same wrapper, same catalog, no
    // disposal, no resubscription churn from wrapper recreation.
    expect(handle.observed.current.sessions).toBe(mapBefore);
    expect(sessionOf(handle, engine.baseUrl)).toBe(before);
    expect(sessionOf(handle, engine.baseUrl).catalog).toBe(before.catalog);
    expect(disposeSpy).not.toHaveBeenCalled();
    expect(cache.listenerCount).toBe(1);
  });

  it("re-pair and removal dispose only displaced catalogs", async () => {
    const one = await pairWithResources(ENGINE_ONE_URL);
    const two = await pairWithResources(ENGINE_TWO_URL);
    const handle = mountProvider();

    const firstOne = sessionOf(handle, one.engine.baseUrl);
    const firstTwo = sessionOf(handle, two.engine.baseUrl);
    const oneDispose = vi.spyOn(firstOne.catalog, "dispose");
    const twoDispose = vi.spyOn(firstTwo.catalog, "dispose");

    // Re-sign-in engine one: the store replaces its StoredEngine (new
    // credential) and the registry adopts fresh resources in the same commit.
    const repaired = await pair(ENGINE_ONE_URL);
    expect(repaired.credential).not.toBe(one.engine.credential);

    const secondOne = sessionOf(handle, one.engine.baseUrl);
    expect(secondOne.engine.credential).toBe(repaired.credential);
    expect(secondOne.client).toBe(h.cells.registry.clientFor(one.engine.baseUrl));
    expect(secondOne.catalog).not.toBe(firstOne.catalog);
    expect(oneDispose).toHaveBeenCalledTimes(1);
    expect(twoDispose).not.toHaveBeenCalled();
    expect(sessionOf(handle, two.engine.baseUrl)).toBe(firstTwo);

    // Removal: engine one's session goes; engine two's catalog stays live.
    const secondOneDispose = vi.spyOn(secondOne.catalog, "dispose");
    await act(async () => {
      store().remove(one.engine.baseUrl);
    });
    expect(handle.observed.current.sessions.has(one.engine.baseUrl)).toBe(false);
    expect(secondOneDispose).toHaveBeenCalledTimes(1);
    expect(twoDispose).not.toHaveBeenCalled();
    expect(sessionOf(handle, two.engine.baseUrl)).toBe(firstTwo);

    // …and the surviving catalog still loads from its own client only.
    let load!: Promise<void>;
    await act(async () => {
      load = firstTwo.catalog.loadHarnesses();
    });
    await act(async () => {
      two.client.resolveNext("ListHarnesses", [HARNESS]);
      await load;
    });
    expect(firstTwo.catalog.getHarnesses().loaded).toBe(true);
    expect(two.client.calls.map((call) => call.method)).toEqual(["ListHarnesses"]);
  });

  it("late results from replaced catalogs cannot affect the new session", async () => {
    const { engine, client: oldClient } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();
    const oldCatalog = sessionOf(handle, engine.baseUrl).catalog;
    const oldHarnessListener = vi.fn();
    oldCatalog.subscribe(oldHarnessListener);
    const oldModelListener = vi.fn();
    oldCatalog.subscribeModels("claude-code", oldModelListener);

    // Old work in flight when the restart lands.
    let oldHarness!: Promise<void>;
    await act(async () => {
      oldHarness = oldCatalog.loadHarnesses();
    });
    let oldModels!: Promise<void>;
    await act(async () => {
      oldModels = oldCatalog.loadModels("claude-code");
    });
    const oldHarnessCalls = oldHarnessListener.mock.calls.length;
    const oldModelCalls = oldModelListener.mock.calls.length;

    await act(async () => {
      handle.observed.current.retry();
    });
    const after = sessionOf(handle, engine.baseUrl);
    const newClient = h.cells.registry.clientFor(engine.baseUrl)!;
    expect(after.catalog).not.toBe(oldCatalog);

    // New rows load on the replacement catalog.
    let newHarness!: Promise<void>;
    await act(async () => {
      newHarness = after.catalog.loadHarnesses();
    });
    await act(async () => {
      newClient.resolveNext("ListHarnesses", [HARNESS]);
      await newHarness;
    });
    expect(after.catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);

    // The displaced catalog's late settle is dropped: no landing, no listener.
    await act(async () => {
      oldClient.resolveNext("ListHarnesses", [{ ...HARNESS, id: "codex" }]);
      oldClient.rejectNext("ListModels", new Error("late failure"));
      await oldHarness;
      await oldModels;
    });
    expect(oldCatalog.getHarnesses().rows).toEqual([]);
    expect(oldHarnessListener).toHaveBeenCalledTimes(oldHarnessCalls);
    expect(oldModelListener).toHaveBeenCalledTimes(oldModelCalls);
    expect(sessionOf(handle, engine.baseUrl).catalog.getHarnesses().rows.map((row) => row.id)).toEqual([
      "claude-code",
    ]);
  });

  it("missing registry resources recover on registry publication", async () => {
    // The engine is stored before the registry has adopted current resources.
    h.cells.registry.autoSpawn = false;
    const engine = await pair(ENGINE_ONE_URL);
    const handle = mountProvider();
    expect(handle.observed.current.sessions.size).toBe(0);
    expect(handle.observed.current.session).toBeNull();

    // The registry publishes resource availability without a fleet change.
    const enginesBefore = store().getSnapshot().engines;
    await act(async () => {
      h.cells.registry.adopt(engine.baseUrl, engine.credential);
    });
    expect(store().getSnapshot().engines).toBe(enginesBefore);

    const session = sessionOf(handle, engine.baseUrl);
    expect(session.client).toBe(h.cells.registry.clientFor(engine.baseUrl));
    let load!: Promise<void>;
    await act(async () => {
      load = session.catalog.loadHarnesses();
    });
    await act(async () => {
      h.cells.registry.clientFor(engine.baseUrl)!.resolveNext("ListHarnesses", [HARNESS]);
      await load;
    });
    expect(session.catalog.getHarnesses().loaded).toBe(true);
    expect(session.catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
  });

  it("provider unmount disposes current catalogs without closing registry clients", async () => {
    const { engine, client: oldClient } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider();
    const first = sessionOf(handle, engine.baseUrl);
    const firstDispose = vi.spyOn(first.catalog, "dispose");

    // Metadata clone (pin) then replacement (restart).
    await act(async () => {
      store().pinDevice(engine.baseUrl, "device-1");
    });
    expect(sessionOf(handle, engine.baseUrl).catalog).toBe(first.catalog);
    expect(firstDispose).not.toHaveBeenCalled();
    await act(async () => {
      handle.observed.current.retry();
    });
    const second = sessionOf(handle, engine.baseUrl);
    expect(second.catalog).not.toBe(first.catalog);
    expect(firstDispose).toHaveBeenCalledTimes(1); // displaced by the restart, once
    const secondDispose = vi.spyOn(second.catalog, "dispose");
    const newClient = h.cells.registry.clientFor(engine.baseUrl)!;

    handle.unmount();

    expect(firstDispose).toHaveBeenCalledTimes(1); // displaced catalogs are not re-disposed
    expect(secondDispose).toHaveBeenCalledTimes(1); // still-owned catalog, once
    // The provider never closes registry-owned clients/caches.
    expect(oldClient.closeCalls).toBe(0);
    expect(newClient.closeCalls).toBe(0);
  });

  it("strict lifecycle replay leaves the adopted catalog usable", async () => {
    const { engine, client } = await pairWithResources(ENGINE_ONE_URL);
    const handle = mountProvider(true);

    // StrictMode's setup→cleanup→setup replay: the published session must
    // hold a LIVE catalog, never one disposed by the replayed cleanup.
    const first = sessionOf(handle, engine.baseUrl);
    const firstDispose = vi.spyOn(first.catalog, "dispose");

    await act(async () => {
      store().pinDevice(engine.baseUrl, "device-1");
    });
    const after = sessionOf(handle, engine.baseUrl);
    expect(after.engine.deviceId).toBe("device-1");
    expect(after.catalog).toBe(first.catalog);
    expect(firstDispose).not.toHaveBeenCalled();

    // Real calls settle through the adopted catalog after the replay + pin.
    let load!: Promise<void>;
    await act(async () => {
      load = after.catalog.loadHarnesses();
    });
    await act(async () => {
      client.resolveNext("ListHarnesses", [HARNESS]);
      await load;
    });
    expect(after.catalog.getHarnesses().loaded).toBe(true);
    expect(after.catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
  });
});
