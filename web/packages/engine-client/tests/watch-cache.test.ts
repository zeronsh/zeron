import { afterEach, describe, expect, test } from "vitest";
import type { Chat, Connectivity, ConnectivityState, Device, SessionStatus, Space } from "@zeron/proto";
import { EngineClient, type EngineStatus } from "../src/client";
import { WATCH_CHATS, WATCH_CONNECTIVITY, WATCH_DEVICES, WATCH_SESSIONS, WATCH_SPACES } from "../src/methods";
import { EngineWatchCache, type ChatStatus, type WatchCacheSnapshot } from "../src/watch-cache";
import { FakeEngine } from "./helpers/fake-engine";
import { delay, statusWhen, trackedFactory, waitUntil } from "./helpers/ws";

const FAST_BACKOFF = { initialMs: 15, jitterMs: 1, maxMs: 100 };

const cleanups: Array<() => Promise<void> | void> = [];

afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) {
    await cleanup();
  }
});

function newClient(fake: FakeEngine) {
  const tracked = trackedFactory();
  const client = new EngineClient({
    endpoint: fake.endpoint,
    credential: "good-credential",
    expectedDeviceId: fake.deviceId,
    backoff: FAST_BACKOFF,
    webSocket: tracked.factory,
  });
  cleanups.push(() => client.close());
  return { client, tracked };
}

const fakeChat = (id: string): Chat => ({
  id,
  deviceId: "fake-device-1",
  title: `Chat ${id}`,
  archived: false,
  cwd: null,
  branch: null,
  checkoutId: null,
  config: null,
  lastMessagePreview: null,
  lastMessageAt: null,
  createdAt: "2026-01-01T00:00:00Z",
});

const fakeSpace = (id: string): Space => ({
  id,
  deviceId: "fake-device-1",
  path: `C:\\spaces\\${id}`,
  gitDetected: false,
  createdAt: "2026-01-01T00:00:00Z",
});

const fakeDevice = (id: string): Device => ({
  id,
  name: `Device ${id}`,
  platform: "windows",
  lastSeenAt: null,
  createdAt: null,
});

const fakeStatus = (chatId: string, status: SessionStatus = "working"): ChatStatus => ({
  chatId,
  deviceId: "fake-device-1",
  status,
  startedAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:01Z",
  lastCompletedTurn: null,
});

const fakeConnectivity = (state: ConnectivityState): Connectivity => ({
  state,
  retryAtMs: 0,
  lastFailure: null,
  chats: [],
});

interface EngineRows {
  chats: Chat[];
  spaces: Space[];
  devices: Device[];
  statuses: ChatStatus[];
}

/** The scripted engine's server-side truth; each (re)subscribe delivers the current lists. */
function scriptStreams(fake: FakeEngine, rows: EngineRows): void {
  fake.streams[WATCH_CHATS] = (reply) => reply.item(rows.chats);
  fake.streams[WATCH_SPACES] = (reply) => reply.item(rows.spaces);
  fake.streams[WATCH_DEVICES] = (reply) => reply.item(rows.devices);
  fake.streams[WATCH_SESSIONS] = (reply) => reply.item(rows.statuses);
}

function allLoaded(cache: EngineWatchCache): boolean {
  const snapshot = cache.getSnapshot();
  return (
    snapshot.chats.loaded &&
    snapshot.spaces.loaded &&
    snapshot.devices.loaded &&
    snapshot.statuses.loaded
  );
}

/**
 * A re-render loop reader in the shape React's `useSyncExternalStore`
 * creates: read, and keep reading as long as the identity moves. With a
 * snapshot-stable store it converges immediately; with a store that
 * re-allocates per read it spins until the guard trips.
 */
async function renderUntilStable(
  cache: EngineWatchCache,
  maxReads = 50,
): Promise<{ snapshot: WatchCacheSnapshot; reads: number }> {
  let reads = 0;
  let last = cache.getSnapshot();
  for (;;) {
    const next = cache.getSnapshot();
    reads += 1;
    if (next === last) {
      return { snapshot: next, reads };
    }
    last = next;
    if (reads >= maxReads) {
      throw new Error("render loop: getSnapshot identity never settled");
    }
    await delay(1);
  }
}

describe("the watch cache against a scripted fake engine", () => {
  test("watch streams populate the cache and updates apply incrementally", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");

    await waitUntil(() => allLoaded(cache), 2_000, "every collection loaded");
    const snapshot = cache.getSnapshot();
    expect(snapshot.generation).toBe(1);
    expect(snapshot.chats.rows.map((row) => row.id)).toEqual(["chat-a"]);
    expect(snapshot.spaces.rows.map((row) => row.id)).toEqual(["space-a"]);
    expect(snapshot.devices.rows.map((row) => row.id)).toEqual(["device-a"]);
    expect(snapshot.statuses.rows.map((row) => row.chatId)).toEqual(["chat-a"]);
    expect(snapshot.capabilities).toContain("web-client");
    expect(cache.supports("web-client")).toBe(true);

    rows.chats = [fakeChat("chat-a"), fakeChat("chat-b")];
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await waitUntil(() => cache.getSnapshot().chats.rows.length === 2, 2_000, "live update lands");

    const before = cache.getSnapshot();
    rows.chats = [fakeChat("chat-a"), { ...fakeChat("chat-b"), title: "Renamed" }];
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await waitUntil(
      () => cache.getSnapshot().chats.rows[1]?.title === "Renamed",
      2_000,
      "renamed row lands",
    );
    const after = cache.getSnapshot();
    expect(after.chats.rows[0]).toBe(before.chats.rows[0]);
    expect(after.chats.rows[1]).not.toBe(before.chats.rows[1]);
  });

  test("reads are snapshot-stable: repeated getSnapshot calls return the same identity until a change", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => allLoaded(cache), 2_000, "every collection loaded");

    const before = cache.getSnapshot();
    expect(cache.getSnapshot()).toBe(before);
    expect(cache.getSnapshot()).toBe(before);
    const stable = await renderUntilStable(cache);
    expect(stable.snapshot).toBe(before);
    expect(stable.reads).toBe(1);

    let notifications = 0;
    const unsubscribe = cache.subscribe(() => {
      notifications += 1;
    });

    // The same list object re-delivered: no new identity, no notification.
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await delay(120);
    expect(cache.getSnapshot()).toBe(before);
    expect(notifications).toBe(0);

    // Deep-equal but distinct row objects: still no new identity, no notification.
    fake.connections[0]!.pushItem(WATCH_CHATS, [fakeChat("chat-a")]);
    await delay(120);
    expect(cache.getSnapshot()).toBe(before);
    expect(notifications).toBe(0);

    // An actual change swaps the identity exactly once.
    rows.chats = [fakeChat("chat-a"), fakeChat("chat-b")];
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await waitUntil(() => notifications === 1, 2_000, "one change notification");
    const changed = cache.getSnapshot();
    expect(changed).not.toBe(before);
    expect(cache.getSnapshot()).toBe(changed);
    expect(changed.chats.rows.length).toBe(2);
    expect(changed.spaces).toBe(before.spaces);
    unsubscribe();
  });

  test("reconnect resubscribes and swaps caches without ghost state or lost rows", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a"), fakeChat("chat-b")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(
      () => allLoaded(cache) && cache.getSnapshot().chats.rows.length === 2,
      2_000,
      "cache populated",
    );

    rows.chats = [fakeChat("chat-a"), fakeChat("chat-b"), fakeChat("chat-c")];
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await waitUntil(() => cache.getSnapshot().chats.rows.length === 3, 2_000, "mid-stream update");

    // Registered after the cache's own listeners, so each records the state
    // the cache had already reached when the status was emitted.
    const whileReconnecting: WatchCacheSnapshot[] = [];
    const atConnected: WatchCacheSnapshot[] = [];
    client.onStatus((status: EngineStatus) => {
      if (status.state === "reconnecting") {
        whileReconnecting.push(cache.getSnapshot());
      }
      if (status.state === "connected" && status.generation === 2) {
        atConnected.push(cache.getSnapshot());
      }
    });

    // Server-side truth changes while the client is offline: chat-b is
    // deleted, chat-d is added.
    rows.chats = [fakeChat("chat-a"), fakeChat("chat-c"), fakeChat("chat-d")];

    fake.connections[0]!.socket.terminate();
    await statusWhen(client, (status) => status.state === "reconnecting");

    // While offline the reads keep returning the frozen last-known rows.
    const offline = whileReconnecting.at(-1);
    expect(offline?.generation).toBe(1);
    expect(offline?.chats.rows.map((row) => row.id)).toEqual(["chat-a", "chat-b", "chat-c"]);

    await statusWhen(client, (status) => status.state === "connected" && status.generation === 2);

    // The swap landed at the generation bump, BEFORE any generation-2 item:
    // no old-generation row survives into the new generation as a ghost.
    expect(atConnected).toHaveLength(1);
    expect(atConnected[0]!.generation).toBe(2);
    expect(atConnected[0]!.chats.rows).toEqual([]);
    expect(atConnected[0]!.chats.loaded).toBe(false);

    await waitUntil(
      () => {
        const snapshot = cache.getSnapshot();
        return snapshot.chats.loaded && snapshot.spaces.loaded && snapshot.statuses.loaded;
      },
      2_000,
      "generation-2 resubscription",
    );
    const swapped = cache.getSnapshot();
    expect(swapped.generation).toBe(2);
    expect(swapped.chats.rows.map((row) => row.id)).toEqual(["chat-a", "chat-c", "chat-d"]);
    expect(swapped.spaces.rows.map((row) => row.id)).toEqual(["space-a"]);
    expect(swapped.statuses.rows.map((row) => row.chatId)).toEqual(["chat-a"]);
  });

  test("the connectivity stream fills the single-value slot and is identity-stable while unchanged", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    fake.streams[WATCH_CONNECTIVITY] = (reply) => reply.item(fakeConnectivity("connected"));
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => cache.getSnapshot().connectivity.loaded, 2_000, "connectivity frame arrives");

    const before = cache.getSnapshot();
    expect(before.connectivity.value?.state).toBe("connected");
    expect(before.connectivity.error).toBeNull();

    let notifications = 0;
    const unsubscribe = cache.subscribe(() => {
      notifications += 1;
    });

    // An unchanged frame re-delivered: no new slot identity, no notification.
    fake.connections[0]!.pushItem(WATCH_CONNECTIVITY, fakeConnectivity("connected"));
    await delay(120);
    expect(cache.getSnapshot().connectivity).toBe(before.connectivity);
    expect(notifications).toBe(0);

    // A degradation swaps the slot identity exactly once.
    fake.connections[0]!.pushItem(WATCH_CONNECTIVITY, fakeConnectivity("offline"));
    await waitUntil(() => notifications === 1, 2_000, "one connectivity change notification");
    const degraded = cache.getSnapshot();
    expect(degraded.connectivity).not.toBe(before.connectivity);
    expect(degraded.connectivity.value?.state).toBe("offline");
    expect(degraded.chats).toBe(before.chats);
    unsubscribe();
  });

  test("a generation swap resets the connectivity slot until the stream re-delivers", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    // The server-side truth, resubscription delivering the current value.
    let truth: Connectivity = fakeConnectivity("connected");
    fake.streams[WATCH_CONNECTIVITY] = (reply) => reply.item(truth);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => cache.getSnapshot().connectivity.loaded, 2_000, "connectivity frame arrives");
    expect(cache.getSnapshot().connectivity.value?.state).toBe("connected");

    // The engine degrades while the client is offline.
    truth = fakeConnectivity("offline");
    fake.connections[0]!.socket.terminate();
    await statusWhen(client, (status) => status.state === "reconnecting");
    // Frozen reads keep the last-known value while offline.
    expect(cache.getSnapshot().connectivity.value?.state).toBe("connected");
    expect(cache.getSnapshot().connectivity.loaded).toBe(true);

    await statusWhen(client, (status) => status.state === "connected" && status.generation === 2);
    // The swap reset the slot: unloaded (the connectivity-observed reset),
    // no stale value from generation 1.
    const swapped = cache.getSnapshot();
    expect(swapped.connectivity.loaded).toBe(false);
    expect(swapped.connectivity.value).toBeNull();

    await waitUntil(() => cache.getSnapshot().connectivity.loaded, 2_000, "connectivity resubscribed");
    expect(cache.getSnapshot().connectivity.value?.state).toBe("offline");
  });

  test("a stream the engine cannot serve degrades that collection without crashing the rest", async () => {
    const fake = new FakeEngine();
    fake.streams[WATCH_CHATS] = (reply) => reply.item([fakeChat("chat-a")]);
    fake.streams[WATCH_DEVICES] = (reply) => reply.item([fakeDevice("device-a")]);
    // WatchSpaces and WatchSessions stay unscripted: the fake engine answers
    // `unknown method: …` the way an older engine would.
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => cache.getSnapshot().chats.loaded, 2_000, "chats loaded");

    const snapshot = cache.getSnapshot();
    expect(snapshot.chats.rows.map((row) => row.id)).toEqual(["chat-a"]);
    expect(snapshot.devices.rows.map((row) => row.id)).toEqual(["device-a"]);
    expect(snapshot.spaces.error?.kind).toBe("unknown-method");
    expect(snapshot.spaces.error?.method).toBe(WATCH_SPACES);
    expect(snapshot.spaces.loaded).toBe(false);
    expect(snapshot.statuses.error?.kind).toBe("unknown-method");
    expect(snapshot.statuses.error?.method).toBe(WATCH_SESSIONS);
    expect(snapshot.connectivity.error?.kind).toBe("unknown-method");
    expect(snapshot.connectivity.error?.method).toBe(WATCH_CONNECTIVITY);
    expect(snapshot.connectivity.loaded).toBe(false);
    expect(snapshot.connectivity.value).toBeNull();

    expect(cache.getSnapshot()).toBe(snapshot);
    expect(cache.getSnapshot()).toBe(snapshot);
    expect(await client.call("Echo", { fine: true })).toEqual({ fine: true });
  });

  test("an engine that reports no capabilities degrades queries, not the cache", async () => {
    const fake = new FakeEngine();
    fake.engineInfo = () => ({ deviceId: fake.deviceId, workspaceScope: "local" });
    fake.streams[WATCH_CHATS] = (reply) => reply.item([fakeChat("chat-a")]);
    fake.streams[WATCH_SPACES] = (reply) => reply.item([fakeSpace("space-a")]);
    fake.streams[WATCH_DEVICES] = (reply) => reply.item([fakeDevice("device-a")]);
    fake.streams[WATCH_SESSIONS] = (reply) => reply.item([]);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => allLoaded(cache), 2_000, "every collection loaded");

    const snapshot = cache.getSnapshot();
    expect(snapshot.capabilities).toEqual([]);
    expect(cache.supports("message-queue-v1")).toBe(false);
    expect(snapshot.chats.rows.map((row) => row.id)).toEqual(["chat-a"]);
    expect(snapshot.chats.error).toBeNull();
  });

  test("dispose cancels every stream and freezes the snapshot", async () => {
    const fake = new FakeEngine();
    const rows: EngineRows = {
      chats: [fakeChat("chat-a")],
      spaces: [fakeSpace("space-a")],
      devices: [fakeDevice("device-a")],
      statuses: [fakeStatus("chat-a")],
    };
    scriptStreams(fake, rows);
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const cache = new EngineWatchCache(client);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    await waitUntil(() => allLoaded(cache), 2_000, "every collection loaded");

    cache.dispose();
    await waitUntil(() => fake.connections[0]!.cancels.length === 4, 2_000, "four cancel frames");

    const frozen = cache.getSnapshot();
    rows.chats = [fakeChat("chat-a"), fakeChat("chat-b")];
    fake.connections[0]!.pushItem(WATCH_CHATS, rows.chats);
    await delay(120);
    expect(cache.getSnapshot()).toBe(frozen);
    expect(frozen.chats.rows.map((row) => row.id)).toEqual(["chat-a"]);
  });
});
