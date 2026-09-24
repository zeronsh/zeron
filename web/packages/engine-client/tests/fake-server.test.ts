import { afterEach, describe, expect, test } from "vitest";
import type { Chat } from "@zeron/proto";
import { ReconnectBackoff } from "../src/backoff";
import { EngineClient, type EngineStatus } from "../src/client";
import { RpcError } from "../src/rpc-error";
import { FakeEngine } from "./helpers/fake-engine";
import { delay, statusWhen, trackedFactory, waitUntil } from "./helpers/ws";

const FAST_BACKOFF = { initialMs: 15, jitterMs: 1, maxMs: 100 };

const cleanups: Array<() => Promise<void> | void> = [];

afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) {
    await cleanup();
  }
});

function newClient(fake: FakeEngine, options: Partial<ConstructorParameters<typeof EngineClient>[0]> = {}) {
  const tracked = trackedFactory();
  const client = new EngineClient({
    endpoint: fake.endpoint,
    credential: "good-credential",
    expectedDeviceId: fake.deviceId,
    backoff: FAST_BACKOFF,
    webSocket: tracked.factory,
    ...options,
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

describe("connection against a scripted fake engine", () => {
  test("authenticates with a lone first frame, survives garbage, and streams", async () => {
    const fake = new FakeEngine();
    fake.streams["WatchChats"] = (reply) => {
      reply.ack();
      reply.item([]);
    };
    fake.calls["Noisy"] = (params, reply) => {
      reply.raw(`not json\n${JSON.stringify({ id: reply.id, ok: params })}\n{`);
    };
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);

    client.connect();
    await statusWhen(client, (status) => status.state === "connected");

    // The listener authenticates the FIRST line of the first message and
    // discards the rest of that message, so the envelope must travel alone.
    expect(fake.authFrames).toEqual(['{"auth":"good-credential"}']);
    expect(fake.engineInfoCalls).toBe(1);

    const echoed = await client.call<{ value: number }>("Echo", { value: 7 });
    expect(echoed).toEqual({ value: 7 });

    const noisy = await client.call<{ noisy: boolean }>("Noisy", { noisy: true });
    expect(noisy).toEqual({ noisy: true });

    const items: Array<{ item: unknown; generation: number }> = [];
    const endings: Array<RpcError | undefined> = [];
    client.watch<Chat[]>("WatchChats", {}, {
      onItem: (item, context) => items.push({ item, generation: context.generation }),
      onEnd: (error) => endings.push(error),
    });
    await waitUntil(() => items.length === 1, 2_000, "first watch item");
    expect(items[0]!.item).toEqual([]);
    expect(items[0]!.generation).toBe(1);

    fake.connections[0]!.pushItem("WatchChats", [fakeChat("chat-9")]);
    await waitUntil(() => items.length === 2, 2_000, "pushed watch item");
    expect((items[1]!.item as Chat[])[0]!.id).toBe("chat-9");

    fake.connections[0]!.finishStream("WatchChats");
    await waitUntil(() => endings.length === 1, 2_000, "watch end after done");
    expect(endings[0]).toBeUndefined();

    // A live stream can be cancelled: the engine must see the cancel frame.
    client.watch("WatchChats", {}, { onItem: () => {} });
    await waitUntil(
      () => fake.connections[0]!.frames.filter((frame) => frame.method === "WatchChats").length === 2,
      2_000,
      "second watch invoke",
    );
    const invokes = fake.connections[0]!.frames.filter((frame) => frame.method === "WatchChats");
    const second = client.watch("WatchChats", {}, { onItem: () => {} });
    await waitUntil(
      () => fake.connections[0]!.frames.filter((frame) => frame.method === "WatchChats").length === 3,
      2_000,
      "third watch invoke",
    );
    second.cancel();
    await waitUntil(() => fake.connections[0]!.cancels.length === 1, 2_000, "cancel frame");
    const thirdInvoke = fake.connections[0]!.frames.filter((frame) => frame.method === "WatchChats")[2]!;
    expect(fake.connections[0]!.cancels[0]).toBe(thirdInvoke.id);

    await expect(client.call("Nope", {})).rejects.toMatchObject({
      kind: "unknown-method",
      method: "Nope",
    });
  });

  test("a watch whose ack never arrives ends with a timeout", async () => {
    const fake = new FakeEngine();
    fake.streams["SilentStream"] = () => {};
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake, { watchAckTimeoutMs: 60 });
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    const endings: RpcError[] = [];
    client.watch("SilentStream", {}, { onItem: () => {}, onEnd: (error) => endings.push(error!) });
    await waitUntil(() => endings.length === 1, 2_000, "ack timeout");
    expect(endings[0]!.kind).toBe("timeout");
    // The connection itself stays usable.
    expect(await client.call("Echo", {})).toEqual({});
  });

  test("a watch with the ack timeout disabled waits silently until items arrive", async () => {
    const fake = new FakeEngine();
    fake.streams["SilentStream"] = () => {};
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake, { watchAckTimeoutMs: 60 });
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    const items: unknown[] = [];
    const endings: Array<RpcError | undefined> = [];
    // SubscribeTerminal parity: no readiness frame, possibly long silence
    // before the first item — the per-watch override disables the ack
    // barrier (the desktop subscribes without one).
    client.watch(
      "SilentStream",
      {},
      { onItem: (item) => items.push(item), onEnd: (error) => endings.push(error) },
      { ackTimeoutMs: 0 },
    );
    // Well past the client-level 60ms ack window: nothing ended, nothing timed out.
    await delay(300);
    expect(endings).toEqual([]);
    fake.connections[0]!.pushItem("SilentStream", { late: true });
    await waitUntil(() => items.length === 1, 2_000, "late item still flows");
    expect(items[0]).toEqual({ late: true });
    expect(endings).toEqual([]);
  });

  test("a drop mid-call fails the call, reconnects, re-verifies identity, and resubscribes", async () => {
    const fake = new FakeEngine();
    fake.streams["WatchChats"] = (reply) => {
      reply.ack();
      reply.item([]);
    };
    fake.calls["Slow"] = () => {};
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client, tracked } = newClient(fake);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");

    const generations: number[] = [];
    client.watch("WatchChats", {}, {
      onItem: (_item, context) => generations.push(context.generation),
    });
    await waitUntil(() => generations.length === 1, 2_000, "first watch item");

    const slow = client.call("Slow", { some: "input" });
    fake.connections[0]!.socket.terminate();
    await expect(slow).rejects.toMatchObject({ kind: "closed" });

    await statusWhen(client, (status: EngineStatus) => status.state === "connected" && status.generation === 2);
    expect(tracked.sockets).toHaveLength(2);
    expect(fake.authFrames).toHaveLength(2);
    expect(fake.engineInfoCalls).toBeGreaterThanOrEqual(2);

    await waitUntil(() => generations.includes(2), 2_000, "resubscribed item on generation 2");
    expect(await client.call("Echo", { reconnect: true })).toEqual({ reconnect: true });
  });

  test("every dial emits the connecting status the catalog heal listens on", async () => {
    // Ticket 61, hole 3: only dial 1 emitted "connecting" (client #dial),
    // so a status-change heal had no event between a drop's
    // "reconnecting" and the re-dial's outcome — silent re-dials starved
    // the re-trigger lattice. Each dial must announce itself.
    const fake = new FakeEngine();
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake);
    const states: string[] = [];
    client.onStatus((status) => states.push(status.state));
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");

    fake.connections[0]!.socket.terminate();
    await statusWhen(client, (status: EngineStatus) => status.state === "connected" && status.generation === 2);
    // The drop emitted "reconnecting"; the re-dial that followed must
    // have emitted "connecting" again before it established.
    const reconnecting = states.lastIndexOf("reconnecting");
    expect(reconnecting).toBeGreaterThanOrEqual(0);
    expect(states.indexOf("connecting", reconnecting + 1)).toBeGreaterThan(reconnecting);
  });

  test("a 4401 invalid-credential close parks permanently with no re-dialling", async () => {
    const fake = new FakeEngine({ auth: (credential) => credential === "good-credential" });
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client, tracked } = newClient(fake, { credential: "revoked-credential" });

    const endings: RpcError[] = [];
    client.watch("WatchChats", {}, { onItem: () => {}, onEnd: (error) => endings.push(error!) });
    client.connect();
    const parked = await statusWhen(client, (status) => status.state === "parked");
    expect(parked).toMatchObject({ state: "parked", reason: "invalid-credential" });
    expect(endings).toHaveLength(1);
    expect(endings[0]!.kind).toBe("parked");

    await expect(client.call("Echo", {})).rejects.toMatchObject({ kind: "parked" });

    await delay(150);
    expect(tracked.sockets).toHaveLength(1);
  });

  test("an identity mismatch parks as a re-pair condition, never silent continued use", async () => {
    const fake = new FakeEngine();
    fake.engineInfo = () => ({ deviceId: "another-device", workspaceScope: "local" });
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client, tracked } = newClient(fake);
    client.connect();
    const parked = await statusWhen(client, (status) => status.state === "parked");
    expect(parked).toMatchObject({ state: "parked", reason: "identity-changed" });
    await expect(client.call("Echo", {})).rejects.toMatchObject({ kind: "parked" });
    await delay(150);
    expect(tracked.sockets).toHaveLength(1);
  });

  test("without an expected identity the first engine is pinned and a later change parks", async () => {
    const fake = new FakeEngine();
    const identities = ["engine-alpha", "engine-beta"];
    fake.engineInfo = () => ({
      deviceId: identities[Math.min(fake.engineInfoCalls, identities.length) - 1] ?? "engine-alpha",
      workspaceScope: "local",
    });
    cleanups.push(() => fake.close());
    await fake.listen();
    const { client } = newClient(fake, { expectedDeviceId: undefined });
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    expect(client.engineInfo?.deviceId).toBe("engine-alpha");

    fake.connections[0]!.socket.terminate();
    await statusWhen(client, (status) => status.state === "parked");
    expect(client.status).toMatchObject({ state: "parked", reason: "identity-changed" });
  });

  test("a single HTTP 401 at the dial is invisible to a browser and recovered by retry", async () => {
    const fake = new FakeEngine();
    cleanups.push(() => fake.close());
    await fake.listen();
    fake.refuseUpgrades = 1;
    const { client, tracked } = newClient(fake);
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    expect(tracked.sockets).toHaveLength(2);
    expect(await client.call("Echo", { recovered: true })).toEqual({ recovered: true });
  });

  test("repeated dial failures space out along the backoff curve", async () => {
    const fake = new FakeEngine();
    cleanups.push(() => fake.close());
    await fake.listen();
    fake.refuseUpgrades = 2;
    const { client, tracked } = newClient(fake, {
      backoff: { initialMs: 40, jitterMs: 0, maxMs: 5_000 },
    });
    client.connect();
    await statusWhen(client, (status) => status.state === "connected");
    expect(tracked.dialedAt).toHaveLength(3);
    expect(tracked.dialedAt[1]! - tracked.dialedAt[0]!).toBeGreaterThanOrEqual(40);
    expect(tracked.dialedAt[2]! - tracked.dialedAt[1]!).toBeGreaterThanOrEqual(80);
  });
});

describe("the backoff curve", () => {
  test("doubles from the initial delay and caps at the maximum", () => {
    const curve = new ReconnectBackoff({ initialMs: 500, maxMs: 15_000, jitterMs: 0 });
    const delays = [1, 2, 3, 4, 5, 6, 7].map(() => curve.nextDelayMs(0));
    expect(delays).toEqual([500, 1000, 2000, 4000, 8000, 15_000, 15_000]);
  });

  test("a connection that outlived the reset threshold restarts the curve", () => {
    const curve = new ReconnectBackoff({ initialMs: 100, maxMs: 10_000, jitterMs: 0, resetAfterMs: 10_000 });
    expect(curve.nextDelayMs(0)).toBe(100);
    expect(curve.nextDelayMs(0)).toBe(200);
    expect(curve.nextDelayMs(10_001)).toBe(100);
    expect(curve.nextDelayMs(0)).toBe(200);
  });

  test("jitter is uniform up to its bound", () => {
    const high = new ReconnectBackoff({ initialMs: 100, jitterMs: 256, random: () => 0.999 });
    expect(high.nextDelayMs(0)).toBe(355);
    const low = new ReconnectBackoff({ initialMs: 100, jitterMs: 256, random: () => 0 });
    expect(low.nextDelayMs(0)).toBe(100);
  });
});
