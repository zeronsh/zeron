import { describe, expect, it } from "vitest";
import type { Chat, Device, EngineInfo, Session, Space } from "@zeron/proto";
import {
  EngineRegistry,
  MemoryEngineCache,
  projectRegistrySnapshot,
  wireParams,
  encodeScopedId,
  parseScopedId,
  type EngineRegistrySnapshot,
  type WebSocketFactory,
  type WsSocket,
} from "@zeron/engine-client";
import { EngineStore, engineWsEndpoint, type StorageLike } from "../src/lib/engine-store";
import { deviceOnline, spaceDeviceTag } from "../src/lib/view";

/**
 * Ticket 31's registry suite — the ports of `engine_registry/tests.rs`
 * (tests.rs:39-227) plus the web-specific cache/offline/routing tests the
 * ticket's §3 names. The desktop's real-engine tests pair an in-process
 * `EngineCore` over a TCP listener; the web's engines are always remote,
 * so the stand-in is a scripted in-memory transport speaking the real
 * wire protocol (first-frame Auth, ndjson frames, 4401 refusal) — the
 * same contract `FakeEngine` drives in the engine-client suite, but with
 * dial-level control so reconnect TIMING is measured, not wall-clocked.
 */

const FAST_BACKOFF = { initialMs: 25, jitterMs: 1, maxMs: 80, resetAfterMs: 300 };
/** The registry's row-cache save debounce; tests wait one window for saves. */
const SAVE_DEBOUNCE_MS = 300;

function chat(id: string, spaceId: string | null, deviceId: string): Chat {
  return {
    id,
    deviceId,
    title: `chat ${id}`,
    archived: false,
    cwd: null,
    branch: null,
    checkoutId: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: null,
    createdAt: "2026-09-19T10:00:00Z",
    ...(spaceId === null ? {} : { spaceId }),
  };
}

function space(id: string, deviceId: string): Space {
  return {
    id,
    deviceId,
    path: `/tmp/${id}`,
    name: null,
    gitDetected: true,
    gitCheckedAt: null,
    checkoutId: null,
    createdAt: "2026-09-19T10:00:00Z",
  };
}

function device(id: string, name: string, lastSeenAt: string | null): Device {
  return { id, name, platform: "windows", lastSeenAt, createdAt: null, version: null, capabilities: [] };
}

function statusRow(chatId: string, deviceId: string): Session {
  return { chatId, deviceId, status: "idle", updatedAt: "2026-09-19T10:00:00Z", startedAt: null, lastCompletedTurn: null };
}

// ---------------------------------------------------------------------------
// The scripted transport
// ---------------------------------------------------------------------------

interface ServerRowSet {
  chats: Chat[];
  spaces: Space[];
  devices: Device[];
  sessions: Session[];
}

/**
 * One scripted engine endpoint. `up` accepts dials and serves the four
 * watch streams plus `EngineInfo`; `down` fails every dial (a close, like
 * a refused TCP connect); `refuseAuth` closes with 4401 so the client
 * parks. Every dial is timestamped through the factory.
 */
class ScriptedEngine {
  readonly endpoint: string;
  readonly credential: string;
  deviceId: string;
  up = true;
  refuseAuth = false;
  rows: ServerRowSet;
  readonly #connections: FakeSocket[] = [];
  dialedAt: number[] = [];

  constructor(endpoint: string, credential: string, deviceId: string, rows: ServerRowSet) {
    this.endpoint = endpoint;
    this.credential = credential;
    this.deviceId = deviceId;
    this.rows = rows;
  }

  factory(): WebSocketFactory {
    return (url: string) => {
      this.dialedAt.push(Date.now());
      return new FakeSocket(this, url);
    };
  }

  /** Hard-drop every live connection (a network cut, not a refusal). */
  drop(): void {
    for (const socket of [...this.#connections]) {
      socket.networkDrop();
    }
  }

  /** Adopt a socket that finished opening. */
  attach(socket: FakeSocket): void {
    this.#connections.push(socket);
  }

  /** Release a socket that closed. */
  detach(socket: FakeSocket): void {
    const index = this.#connections.indexOf(socket);
    if (index >= 0) {
      this.#connections.splice(index, 1);
    }
  }

  get connections(): readonly FakeSocket[] {
    return this.#connections;
  }

  handle(socket: FakeSocket, text: string): void {
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (trimmed.length === 0) {
        continue;
      }
      let frame: { id?: number; method?: string; params?: unknown; auth?: string };
      try {
        frame = JSON.parse(trimmed) as typeof frame;
      } catch {
        continue;
      }
      if (typeof frame.auth === "string") {
        if (this.refuseAuth || frame.auth !== this.credential) {
          socket.serverClose(4401, "invalid credential");
          return;
        }
        socket.authed = true;
        return;
      }
      if (typeof frame.id !== "number" || typeof frame.method !== "string" || !socket.authed) {
        return;
      }
      this.dispatch(socket, frame.id, frame.method);
    }
  }

  dispatch(socket: FakeSocket, id: number, method: string): void {
    if (method === "EngineInfo") {
      const info: EngineInfo = {
        deviceId: this.deviceId,
        workspaceScope: "local",
        capabilities: ["web-client"],
      };
      socket.reply(id, { ok: info });
      return;
    }
    const rows = streamRows(this, method);
    if (rows !== undefined) {
      socket.reply(id, { ok: { stream: true } });
      socket.reply(id, { item: rows });
      return;
    }
    socket.reply(id, { err: `unknown method: ${method}` });
  }
}

function streamRows(server: ScriptedEngine, method: string): unknown[] | undefined {
  switch (method) {
    case "WatchChats":
      return server.rows.chats;
    case "WatchSpaces":
      return server.rows.spaces;
    case "WatchDevices":
      return server.rows.devices;
    case "WatchSessions":
      return server.rows.sessions;
    default:
      return undefined;
  }
}

class FakeSocket implements WsSocket {
  readonly #server: ScriptedEngine;
  authed = false;
  #open = false;
  #closed = false;
  readonly #openListeners: (() => void)[] = [];
  readonly #messageListeners: ((event: { data: unknown }) => void)[] = [];
  readonly #closeListeners: ((event: { code: number; reason: string; wasClean: boolean }) => void)[] = [];

  constructor(server: ScriptedEngine, url: string) {
    this.#server = server;
    void url;
    if (server.up) {
      // The connect lands on the next macrotask, like a real socket.
      setTimeout(() => {
        if (this.#closed) {
          return;
        }
        this.#open = true;
        this.#server.attach(this);
        for (const listener of this.#openListeners) {
          listener();
        }
      }, 0);
    } else {
      // A refused connect: the socket closes without opening.
      setTimeout(() => this.emitClose(4000, "connect failed"), 0);
    }
  }

  send(data: string): void {
    if (!this.#open) {
      return;
    }
    this.#server.handle(this, data);
  }

  close(): void {
    this.emitClose(1000, "client closed");
  }

  addEventListener(
    type: "open" | "message" | "close" | "error",
    listener: (() => void) | ((event: { data: unknown }) => void) | ((event: { code: number; reason: string; wasClean: boolean }) => void),
  ): void {
    switch (type) {
      case "open":
        this.#openListeners.push(listener as () => void);
        break;
      case "message":
        this.#messageListeners.push(listener as (event: { data: unknown }) => void);
        break;
      case "close":
        this.#closeListeners.push(listener as (event: { code: number; reason: string; wasClean: boolean }) => void);
        break;
      default:
        break;
    }
  }

  /** A server-originated reply frame. */
  reply(id: number, frame: Record<string, unknown>): void {
    this.deliver(JSON.stringify({ id, ...frame }));
  }

  /** A hard network cut (the peer vanished). */
  networkDrop(): void {
    this.emitClose(1006, "");
  }

  serverClose(code: number, reason: string): void {
    this.emitClose(code, reason);
  }

  private deliver(text: string): void {
    for (const listener of this.#messageListeners) {
      listener({ data: text });
    }
  }

  private emitClose(code: number, reason: string): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#open = false;
    this.#server.detach(this);
    for (const listener of this.#closeListeners) {
      listener({ code, reason, wasClean: code === 1000 });
    }
  }
}

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

function memoryStorage(): StorageLike & { raw(): string | null } {
  let value: string | null = null;
  return {
    getItem: () => value,
    setItem: (_key, next) => void (value = next),
    removeItem: () => void (value = null),
    raw: () => value,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitUntil(predicate: () => boolean, message = "condition", timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${message}`);
    }
    await delay(10);
  }
}

/**
 * The fleet wiring under test — the same shape `state/fleet.ts` installs:
 * the pairing store drives the registry's supervised set, and the registry
 * owns one client + watch cache per paired engine.
 */
function createFleet(
  storage: StorageLike,
  cache: MemoryEngineCache,
  servers: readonly ScriptedEngine[],
) {
  const byEndpoint = new Map(servers.map((server) => [server.endpoint, server]));
  const factory: WebSocketFactory = (url) => {
    const server = byEndpoint.get(url);
    if (server === undefined) {
      throw new Error(`no scripted engine for ${url}`);
    }
    return server.factory()(url);
  };
  const store = new EngineStore({ storage });
  // Sign-in carries the credential directly (the AuthKit/dev sign-in flow
  // ran before the store saw it); this helper wires each scripted engine's
  // own credential to its origin, the way the real flow would.
  const signIn = (server: ScriptedEngine) =>
    store.signInEngine({
      baseUrl: server.endpoint.replace(/^ws/, "http"),
      credential: server.credential,
      label: "Test web",
      sessionId: `s-${server.deviceId}`,
    });
  const registry = new EngineRegistry({ cache, webSocket: factory, backoff: FAST_BACKOFF });
  const sync = (): void => {
    const state = store.getSnapshot();
    registry.sync(
      state.engines.map((engine) => ({
        key: engine.baseUrl,
        endpoint: engineWsEndpoint(engine.baseUrl),
        credential: engine.credential,
        expectedDeviceId: engine.deviceId,
      })),
      state.configurationError,
    );
  };
  store.subscribe(sync);
  sync();
  return { store, registry, sync, signIn };
}

const ENDPOINT_A = "ws://127.0.0.1:27699/";
const ENDPOINT_B = "ws://192.168.1.20:27699/";
const URL_A = "http://127.0.0.1:27699";
const URL_B = "http://192.168.1.20:27699";

function engineA(): ScriptedEngine {
  return new ScriptedEngine(ENDPOINT_A, "cred-a", "device-a", {
    chats: [chat("same-chat", "same-space", "device-a")],
    spaces: [space("same-space", "device-a")],
    devices: [device("device-a", "Laptop", "2026-09-19T10:00:00Z")],
    sessions: [statusRow("same-chat", "device-a")],
  });
}

function engineB(): ScriptedEngine {
  return new ScriptedEngine(ENDPOINT_B, "cred-b", "device-b", {
    chats: [chat("same-chat", "same-space", "device-b")],
    spaces: [space("same-space", "device-b")],
    devices: [device("device-b", "Desktop", "2026-09-19T10:00:00Z")],
    sessions: [statusRow("same-chat", "device-b")],
  });
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

describe("EngineRegistry (ticket 31)", () => {
  it("realEngineReconnectRetainsRowsPersistsPairingAndForgets", async () => {
    const cache = new MemoryEngineCache();
    const storage = memoryStorage();
    const a = engineA();
    const b = engineB();
    // Sign in to the FIRST engine while its transport is up — sign-in
    // starts supervision immediately, no switch step.
    let fleet = createFleet(storage, cache, [a, b]);
    await fleet.signIn(a);
    await fleet.signIn(b);
    const registry = fleet.registry;

    // Rows flow from BOTH engines simultaneously, merged under scoped ids.
    await waitUntil(
      () =>
        registry.getSnapshot().engines.length === 2 &&
        registry.getSnapshot().engines.every((engine) => engine.state === "connected" && engine.chats.loaded),
      "both engines connected",
    );
    let projected = projectRegistrySnapshot(registry.getSnapshot());
    expect(projected.chats).toHaveLength(2);
    expect(projected.chats[0]!.id).not.toBe(projected.chats[1]!.id);
    expect(projected.spaces).toHaveLength(2);
    // Same raw chat id on both engines resolves to distinct scoped ids.
    for (const row of projected.chats) {
      const parsed = parseScopedId(row.id);
      expect(parsed.rawId).toBe("same-chat");
      expect([URL_A, URL_B]).toContain(parsed.engine);
    }

    // Let the debounced cache saves land before the outage.
    await delay(SAVE_DEBOUNCE_MS + 80);

    // Drop engine B's connection: its rows keep rendering (frozen
    // last-known), the state moves to reconnecting, and its calls fail
    // while engine A stays live.
    b.drop();
    await waitUntil(
      () =>
        registry
          .getSnapshot()
          .engines.some((engine) => engine.key === URL_B && engine.state === "reconnecting"),
      "engine B reconnecting",
    );
    projected = projectRegistrySnapshot(registry.getSnapshot());
    expect(projected.chats).toHaveLength(2);
    expect(projected.spaces).toHaveLength(2);
    const clientB = registry.clientFor(URL_B);
    const clientA = registry.clientFor(URL_A);
    expect(clientB).not.toBeNull();
    expect(clientA).not.toBeNull();
    await expect(clientB!.call("Echo", {})).rejects.toThrow();
    await expect(clientA!.call("EngineInfo", {})).resolves.toBeTruthy();

    // Reconnect: the dropped engine returns on its own (backoff curve —
    // exercised precisely by the dedicated timing test below) with a NEW
    // generation, and its rows are live again.
    const dialsAfterDrop = b.dialedAt.length;
    await waitUntil(() => b.dialedAt.length >= dialsAfterDrop + 1, "engine B redials");
    await waitUntil(
      () => registry.getSnapshot().engines.some((engine) => engine.key === URL_B && engine.generation >= 2),
      "engine B reconnects with a new generation",
    );
    const entryB = registry.getSnapshot().engines.find((engine) => engine.key === URL_B)!;
    expect(entryB.state).toBe("connected");
    expect(entryB.generation).toBeGreaterThanOrEqual(2);
    await expect(clientB!.call("EngineInfo", {})).resolves.toBeTruthy();

    // Forgetting: the persisted list, the live connection, AND the cache
    // entries all drop — task-cancel → flush → delete, no race.
    await delay(SAVE_DEBOUNCE_MS + 80);
    expect(cache.hasRows(URL_B)).toBe(true);
    fleet.store.remove(URL_B);
    await waitUntil(() => registry.getSnapshot().engines.every((engine) => engine.key !== URL_B), "B forgotten");
    await waitUntil(() => !cache.hasRows(URL_B), "B cache deleted");
    projected = projectRegistrySnapshot(registry.getSnapshot());
    expect(projected.chats).toHaveLength(1);
    const forgotten = registry.clientFor(URL_B);
    expect(forgotten).toBeNull();
    expect(projected.chats[0]!).toBeDefined();

    // "Persists pairing": reopen the whole client (fresh store + fresh
    // registry, same storage + cache) with BOTH transports down — the
    // cached history keeps the sidebar rendering (engine A's rows).
    b.drop();
    a.drop();
    a.up = false;
    b.up = false;
    await registry.shutdown();
    fleet = createFleet(storage, cache, [a, b]);
    const reopened = fleet.registry;
    await waitUntil(() => reopened.getSnapshot().engines.length === 1, "reopened registry loads engine A");
    await delay(SAVE_DEBOUNCE_MS);
    projected = projectRegistrySnapshot(reopened.getSnapshot());
    expect(projected.chats).toHaveLength(1);
    expect(projected.chats[0]!.id).toBe(encodeScopedId(URL_A, "same-chat"));
    // The offline entry reads reconnecting, never connected — but its
    // cached rows render with `loaded: true`.
    const offline = reopened.getSnapshot().engines[0]!;
    expect(offline.state).not.toBe("connected");
    expect(offline.chats.loaded).toBe(true);
    await fleet.registry.shutdown();
  }, 20_000);

  it("reconnectBackoffDoublesAndResetsAfterALongLivedConnection", async () => {
    // A dedicated timing pass: short-lived connections double the delay
    // (25 → 50), a long-lived one (past `resetAfterMs`) resets it back to
    // the initial. Delays are measured DROP→REDIAL — the close handler
    // schedules the reconnect synchronously, so the window between the
    // drop and the next dial IS the backoff curve, not wall clock.
    const server = new ScriptedEngine(ENDPOINT_A, "cred-a", "device-a", {
      chats: [],
      spaces: [],
      devices: [],
      sessions: [],
    });
    const cache = new MemoryEngineCache();
    const registry = new EngineRegistry({ cache, webSocket: server.factory(), backoff: FAST_BACKOFF });
    registry.sync(
      [{ key: URL_A, endpoint: ENDPOINT_A, credential: "cred-a", expectedDeviceId: null }],
      null,
    );
    const dials = server.dialedAt;
    const dropAndAwaitRedial = async (label: string): Promise<number> => {
      await waitUntil(() => server.connections.length > 0, `${label} connection`);
      const droppedAt = Date.now();
      server.drop();
      const before = dials.length;
      await waitUntil(() => dials.length > before, `${label} redial`);
      return dials[before]! - droppedAt;
    };
    // Two short-lived connections: the first retry pays the initial delay,
    // the second pays the DOUBLED one.
    await waitUntil(() => dials.length >= 1, "first dial");
    await waitUntil(
      () => registry.getSnapshot().engines[0]?.generation === 1,
      "generation 1",
    );
    const delay1 = await dropAndAwaitRedial("short-lived 1");
    await waitUntil(
      () => registry.getSnapshot().engines[0]?.generation === 2,
      "generation 2",
    );
    const delay2 = await dropAndAwaitRedial("short-lived 2");
    expect(delay1).toBeGreaterThanOrEqual(FAST_BACKOFF.initialMs - 5);
    expect(delay1).toBeLessThan(FAST_BACKOFF.initialMs + 15);
    expect(delay2).toBeGreaterThanOrEqual(FAST_BACKOFF.initialMs * 2 - 5);
    expect(delay2).toBeLessThan(FAST_BACKOFF.initialMs * 2 + 15);
    // A long-lived connection (past resetAfterMs) resets the curve: the
    // next redial is back at the INITIAL delay.
    await waitUntil(
      () => registry.getSnapshot().engines[0]?.generation === 3,
      "generation 3",
    );
    await delay(FAST_BACKOFF.resetAfterMs + 60);
    const delay3 = await dropAndAwaitRedial("post-reset");
    expect(delay3).toBeGreaterThanOrEqual(FAST_BACKOFF.initialMs - 5);
    expect(delay3).toBeLessThan(FAST_BACKOFF.initialMs + 15);
    await registry.shutdown();
  }, 20_000);

  it("parkedEnginesNeverRetryUntilRepaired", async () => {
    const server = engineA();
    server.refuseAuth = true;
    const registry = new EngineRegistry({
      cache: new MemoryEngineCache(),
      webSocket: server.factory(),
      backoff: FAST_BACKOFF,
    });
    registry.sync([{ key: URL_A, endpoint: ENDPOINT_A, credential: "wrong", expectedDeviceId: null }], null);
    await waitUntil(
      () => registry.getSnapshot().engines[0]?.state === "off",
      "engine parks on refused credential",
    );
    const dials = server.dialedAt.length;
    await delay(300);
    // Park-on-revoked: no further dials for that engine key, ever.
    expect(server.dialedAt.length).toBe(dials);
    // An explicit re-pair (new credential) replaces the entry and dials again.
    server.refuseAuth = false;
    registry.sync([{ key: URL_A, endpoint: ENDPOINT_A, credential: "cred-a", expectedDeviceId: null }], null);
    await waitUntil(
      () => registry.getSnapshot().engines[0]?.state === "connected",
      "re-paired engine connects",
    );
    await registry.shutdown();
  });

  it("damagedPairingConfigPreservesLocalAccessAndOriginalBytes", async () => {
    // The desktop's local-engine half ("preserves local access") maps to
    // the web's no-local-engine reality: the app still runs with the
    // damaged list read as empty, the configuration error surfaces, and
    // the raw bytes are never overwritten.
    const storage = memoryStorage();
    storage.setItem("zeron.fleet.v1", "damaged config");
    const a = engineA();
    const cache = new MemoryEngineCache();
    const fleet = createFleet(storage, cache, [a]);
    expect(fleet.store.getSnapshot().configurationError).not.toBe(null);
    expect(fleet.store.getSnapshot().engines).toHaveLength(0);
    expect(fleet.registry.getSnapshot().configurationError).not.toBe(null);
    // Sign-in refuses while damaged.
    await expect(fleet.signIn(a)).rejects.toThrow();
    // The original bytes survive every refusal untouched.
    expect(storage.raw()).toBe("damaged config");
    expect(cache.hasRows(URL_A)).toBe(false);
    await fleet.registry.shutdown();
  });

  it("projectedMergesRowsFromEveryPairedEngineUnderScopedIds", () => {
    // A registry snapshot shaped by hand (the pure merge, no transport):
    // two engines carrying the SAME raw ids, one connected, one offline
    // with a cached identity but no device rows.
    const infoA: EngineInfo = { deviceId: "device-a", workspaceScope: "local", capabilities: ["web-client"] };
    const infoB: EngineInfo = { deviceId: "device-b", workspaceScope: "local", capabilities: ["web-client"] };
    const snapshot: EngineRegistrySnapshot = {
      engines: [
        {
          key: URL_A,
          info: infoA,
          state: "connected",
          lastError: null,
          generation: 2,
          chats: { rows: [chat("same-chat", "same-space", "device-a")], loaded: true, error: null },
          spaces: { rows: [space("same-space", "device-a")], loaded: true, error: null },
          devices: { rows: [device("device-a", "Laptop", "2026-09-19T10:00:00Z")], loaded: true, error: null },
          sessions: { rows: [statusRow("same-chat", "device-a")], loaded: true, error: null },
        },
        {
          key: URL_B,
          info: infoB,
          state: "reconnecting",
          lastError: "Engine connection closed (1006)",
          generation: 1,
          chats: { rows: [chat("same-chat", "same-space", "device-b")], loaded: true, error: null },
          spaces: { rows: [space("same-space", "device-b")], loaded: true, error: null },
          devices: { rows: [], loaded: false, error: null },
          sessions: { rows: [statusRow("same-chat", "device-b")], loaded: true, error: null },
        },
      ],
      configurationError: null,
    };
    const projected = projectRegistrySnapshot(snapshot);
    // One flat list: same raw ids from two engines, never colliding.
    expect(projected.chats).toHaveLength(2);
    expect(new Set(projected.chats.map((row) => row.id)).size).toBe(2);
    expect(new Set(projected.spaces.map((row) => row.id)).size).toBe(2);
    expect(new Set(projected.sessions.map((row) => row.chatId)).size).toBe(2);
    for (const row of projected.chats) {
      expect(parseScopedId(row.id).rawId).toBe("same-chat");
      expect(parseScopedId(row.deviceId).rawId).toBe(parseScopedId(row.id).engine === URL_A ? "device-a" : "device-b");
    }
    // Spaces scope their ids and their device ids too.
    expect(parseScopedId(projected.spaces[0]!.id).rawId).toBe("same-space");
    // An engine with NO device rows still contributes its own synthesized
    // device ("Remote engine"), scoped like any other.
    expect(projected.devices).toHaveLength(2);
    const deviceB = projected.devices.find((row) => parseScopedId(row.id).engine === URL_B)!;
    expect(deviceB.name).toBe("Remote engine");
    expect(parseScopedId(deviceB.id).rawId).toBe("device-b");
    // A non-connected engine's cached device timestamps never imply live.
    expect(deviceB.lastSeenAt).toBeNull();
    // Connected engines keep their heartbeat.
    const deviceA = projected.devices.find((row) => parseScopedId(row.id).engine === URL_A)!;
    expect(deviceA.lastSeenAt).toBe("2026-09-19T10:00:00Z");
  });

  it("wireParamsRejectsRequestForAnotherEngine", () => {
    const owner = "http://engine-b:27699";
    const foreign = encodeScopedId("http://engine-a:27699", "same-chat");
    const localReserved = encodeScopedId("http://engine-b:27699", "engine:v1:literal-id");
    expect(() => wireParams(owner, "QueueCommand", { chatId: foreign })).toThrow(
      /Request identity belongs to another engine/u,
    );
    // A scoped id under the OWNER passes and decodes to raw.
    const wired = wireParams(owner, "QueueCommand", {
      chatId: encodeScopedId(owner, "same-chat"),
    }) as Record<string, unknown>;
    expect(wired.chatId).toBe("same-chat");
    // Unscoped ids pass through untouched.
    const plain = wireParams(owner, "QueueCommand", { chatId: "same-chat" }) as Record<string, unknown>;
    expect(plain.chatId).toBe("same-chat");
    // Every identity field, `target.*`, and Mutate's `id` decode.
    const mut = wireParams(owner, "Mutate", {
      id: encodeScopedId(owner, "chat-1"),
      op: "renameChat",
      target: { chatId: encodeScopedId(owner, "chat-1") },
    }) as { id: string; target: { chatId: string } };
    expect(mut.id).toBe("chat-1");
    expect(mut.target.chatId).toBe("chat-1");
    expect(() => wireParams(owner, "Mutate", { id: foreign, op: "deleteChat" })).toThrow(
      /Request identity belongs to another engine/u,
    );
    void localReserved;
  });

  it("wireParamsAlwaysStripsTargetDeviceId", () => {
    const owner = "http://engine-b:27699";
    const wired = wireParams(owner, "ListFolders", {
      query: "notes",
      targetDeviceId: encodeScopedId(owner, "device-b"),
      path: "/tmp",
    }) as Record<string, unknown>;
    // Routing ends at the socket: the field decodes (same engine — no
    // rejection) and is REMOVED before the request leaves the client.
    expect(wired.targetDeviceId).toBeUndefined();
    expect(wired.query).toBe("notes");
    expect(wired.path).toBe("/tmp");
    // Also stripped from nested `target` objects, and when unscoped.
    const nested = wireParams(owner, "ListModels", {
      harness: "opencode",
      target: { targetDeviceId: "device-b" },
    }) as { target: Record<string, unknown> };
    expect(nested.target.targetDeviceId).toBeUndefined();
    // Non-identity fields stay byte-for-byte.
    const foreign = encodeScopedId("http://engine-a:27699", "x");
    const untouched = wireParams(owner, "QueueCommand", {
      chatId: encodeScopedId(owner, "chat-1"),
      text: foreign,
      options: { anything: foreign },
      path: foreign,
    }) as Record<string, unknown>;
    expect(untouched.text).toBe(foreign);
    expect((untouched.options as Record<string, unknown>).anything).toBe(foreign);
    expect(untouched.path).toBe(foreign);
  });

  it("spaceDeviceTagReflectsLiveEngineState", () => {
    // A device backed by a known registry engine reports that engine's
    // LIVE connection state instead of the heartbeat heuristic — even a
    // fresh last-seen reads offline while its engine reconnects.
    const engineStates = new Map([
      ["http://a:27699", "connected" as const],
      ["http://b:27699", "reconnecting" as const],
    ]);
    const devices = [
      { ...device(encodeScopedId("http://a:27699", "device-a"), "Laptop", "2000-01-01T00:00:00Z") },
      { ...device(encodeScopedId("http://b:27699", "device-b"), "Desktop", new Date().toISOString()) },
    ];
    const now = Date.now();
    // Connected engine: online regardless of the stale heartbeat.
    expect(spaceDeviceTag({ deviceId: devices[0]!.id }, devices, now, engineStates)).toEqual({
      tag: "@ Laptop",
      offline: false,
    });
    // Reconnecting engine: offline regardless of the fresh heartbeat.
    const tag = spaceDeviceTag({ deviceId: devices[1]!.id }, devices, now, engineStates);
    expect(tag.tag).toBe("@ Desktop");
    expect(tag.offline).toBe(true);
    // The deviceOnline override is what the tag consumes.
    expect(deviceOnline(devices[1], now, engineStates)).toBe(false);
  });

  it("spaceDeviceTagFallsBackToLastSeenWindow", () => {
    // No registry engine for the device (or none supplied): the 70s
    // last-seen window decides, exactly `settings/devices.rs`.
    const now = Date.parse("2026-09-19T12:00:00Z");
    const fresh = device("d1", "Laptop", "2026-09-19T11:59:50Z");
    const stale = device("d2", "Desktop", "2026-09-19T11:58:00Z");
    const never = device("d3", "Tablet", null);
    expect(spaceDeviceTag({ deviceId: "d1" }, [fresh, stale, never], now).offline).toBe(false);
    expect(spaceDeviceTag({ deviceId: "d2" }, [fresh, stale, never], now).offline).toBe(true);
    expect(spaceDeviceTag({ deviceId: "d3" }, [fresh, stale, never], now).offline).toBe(true);
    // A device row that is MISSING reads online (never a spurious glyph).
    expect(spaceDeviceTag({ deviceId: "missing" }, [fresh], now).offline).toBe(false);
    // Unknown-name fallback: "@ Unknown device".
    expect(spaceDeviceTag({ deviceId: "missing" }, [fresh], now).tag).toBe("@ Unknown device");
    // An engine KNOWN to the registry wins even over the window.
    const engineStates = new Map([["http://b:27699", "off" as const]]);
    const offlineEngineDevice = device(encodeScopedId("http://b:27699", "device-b"), "Desktop", "2026-09-19T11:59:50Z");
    expect(spaceDeviceTag({ deviceId: offlineEngineDevice.id }, [offlineEngineDevice], now, engineStates).offline).toBe(true);
  });
});
