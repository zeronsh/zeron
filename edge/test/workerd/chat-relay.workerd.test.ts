import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME, type Frame } from "../../src/chat-frames";
import { logStats } from "../../src/chat-log";
import { pendingChatWake } from "../../src/chat-wakes";
import { AUTH_ORG_HEADER, AUTH_USER_HEADER } from "../../src/env";

/** The relay layer's server half (docs/design/relay-layer.md), and proof that
 * with it off a room behaves exactly as it did. */

const user = "relay-owner";
const room = (name: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${name}`));
const bytes = (fill: number, len = 16) => new Uint8Array(len).fill(fill);

/** Turn the flag on for this object's lifetime. */
const enableRelay = (name: string) =>
  runInDurableObject(room(name), instance => {
    const target = instance as unknown as { env: Record<string, unknown> };
    target.env = { ...target.env, CHAT_RELAY_LAYER: "1" };
  });

const connect = async (
  name: string,
  device: string,
  opts: { relay?: boolean; host?: string; gen?: number; relaying?: boolean; org?: string } = {}
) => {
  const hint = opts.host ? `&hostDevice=${opts.host}` : "";
  const response = await room(name).fetch(`https://chat/ws?device=${device}&chatId=${name}${hint}`, {
    headers: {
      [AUTH_USER_HEADER]: user,
      upgrade: "websocket",
      ...(opts.org ? { [AUTH_ORG_HEADER]: opts.org } : {})
    }
  });
  expect(response.status).toBe(101);
  const socket = response.webSocket!;
  socket.accept();
  socket.binaryType = "arraybuffer";
  const frames: Frame[] = [];
  const raw: Uint8Array[] = [];
  const waiters: Array<() => void> = [];
  socket.addEventListener("message", event => {
    const data = new Uint8Array(event.data as ArrayBuffer);
    raw.push(data);
    const frame = decodeFrame(data);
    if (frame) frames.push(frame);
    waiters.splice(0).forEach(wake => wake());
  });
  const nextOf = async (type: Frame["type"]): Promise<Frame> => {
    for (;;) {
      const index = frames.findIndex(f => f.type === type);
      if (index >= 0) return frames.splice(index, 1)[0]!;
      await new Promise<void>(resolve => waiters.push(resolve));
    }
  };
  const sendFrame = (type: Frame["type"], header: Record<string, unknown>, payload?: Uint8Array) => {
    const frame = encodeFrame(type, header, payload);
    socket.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  };
  sendFrame(FRAME.hello, {
    cursor: 0,
    device,
    ...(opts.relay ? { relay: 1 } : {}),
    ...(opts.gen ? { gen: opts.gen } : {}),
    ...(opts.relaying ? { relaying: 1 } : {})
  });
  const state = await nextOf(FRAME.state);
  /** A probe's answer proves the DO handled everything sent before it. */
  const settled = async () => {
    sendFrame(FRAME.probe, {});
    await nextOf(FRAME.probeOk);
  };
  return { socket, frames, raw, state, nextOf, sendFrame, settled };
};

const queueCount = (name: string) =>
  runInDurableObject(room(name), (_instance, state) =>
    [...state.storage.sql.exec("SELECT COUNT(*) AS n FROM relay_queue")][0]!.n as number
  );

describe("with the flag off", () => {
  it("hello, state and an unknown frame answer byte for byte as before", async () => {
    const name = crypto.randomUUID();
    const client = await connect(name, "a", { relay: true });
    expect(client.state.header).toEqual({
      headSeq: 0, seqFloor: 0, checkpointSeq: 0, checkpointSize: 0, rowCount: 0, rowBytes: 0
    });
    for (const type of [FRAME.relay, FRAME.announce, FRAME.syncReq, FRAME.syncData, FRAME.enqueue, FRAME.queueReq, FRAME.delivered]) {
      client.sendFrame(type, { batchId: "x", to: "b" }, bytes(1));
      const error = await client.nextOf(FRAME.error);
      expect(error.header).toEqual({ code: "bad_frame", message: "malformed frame" });
    }
    client.socket.close();
  });

  it("creates no queue table and stores nothing new", async () => {
    const name = crypto.randomUUID();
    const client = await connect(name, "a", { relay: true });
    client.sendFrame(FRAME.push, { batchId: "row" }, bytes(1));
    await client.nextOf(FRAME.ack);
    const tables = await runInDurableObject(room(name), (_instance, state) =>
      [...state.storage.sql.exec("SELECT name FROM sqlite_master WHERE name='relay_queue'")]
    );
    expect(tables).toEqual([]);
    const stats = await (await room(name).fetch("https://chat/stats", { headers: { [AUTH_USER_HEADER]: user } })).json();
    expect(stats).not.toHaveProperty("relayQueue");
    client.socket.close();
  });

  it("a read without relay frames signals nobody", async () => {
    const name = crypto.randomUUID();
    const client = await connect(name, "a", { relay: true });
    const pull = await room(name).fetch("https://chat/rows?after=0&device=old", { headers: { [AUTH_USER_HEADER]: user } });
    await pull.arrayBuffer();
    await client.settled();
    expect(client.frames.filter(f => f.type === FRAME.legacyPeer)).toEqual([]);
    client.socket.close();
  });
});

describe("relay frames", () => {
  it("offers the layer only to a client that asks for it", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const asks = await connect(name, "new", { relay: true });
    expect(asks.state.header).toMatchObject({ relay: 1, legacyPeers: 0 });
    const old = await connect(name, "old");
    expect(old.state.header).not.toHaveProperty("relay");
    asks.socket.close();
    old.socket.close();
  });

  it("forwards to relay sockets without storing a row, and never to an old client", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const viewer = await connect(name, "viewer", { relay: true });
    const old = await connect(name, "old");
    host.sendFrame(FRAME.relay, {}, bytes(7));
    const relayed = await viewer.nextOf(FRAME.relay);
    expect(relayed.header).toEqual({ device: "host" });
    expect(relayed.payload).toEqual(bytes(7));
    await old.settled();
    expect(old.frames.filter(f => f.type === FRAME.relay)).toEqual([]);
    await host.settled();
    expect(host.frames.filter(f => f.type === FRAME.relay)).toEqual([]);
    const stats = await runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql));
    expect(stats).toMatchObject({ headSeq: 0, rowCount: 0 });
    for (const client of [host, viewer, old]) client.socket.close();
  });

  it("relay frames write nothing but one mark that the room has relayed", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const viewer = await connect(name, "viewer", { relay: true });
    const cursors: Array<SqlStorageCursor<Record<string, SqlStorageValue>>> = [];
    await runInDurableObject(room(name), (_instance, state) => {
      const sql = state.storage.sql;
      const exec = sql.exec.bind(sql);
      sql.exec = ((text: string, ...bindings: SqlStorageValue[]) => {
        const cursor = exec(text, ...bindings);
        cursors.push(cursor);
        return cursor;
      }) as typeof sql.exec;
    });
    for (let i = 0; i < 200; i++) host.sendFrame(FRAME.relay, {}, bytes(i % 255, 2048));
    for (let i = 0; i < 200; i++) await viewer.nextOf(FRAME.relay);
    await host.settled();
    expect(cursors.reduce((sum, cursor) => sum + cursor.rowsWritten, 0)).toBeLessThanOrEqual(2);
    host.socket.close();
    viewer.socket.close();
  });

  it("directs sync frames to their target alone, and drops them if it's gone", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const asker = await connect(name, "asker", { relay: true });
    const bystander = await connect(name, "bystander", { relay: true });
    asker.sendFrame(FRAME.syncReq, { to: "host" }, bytes(1));
    const request = await host.nextOf(FRAME.syncReq);
    expect(request.header).toEqual({ device: "asker" });
    host.sendFrame(FRAME.syncData, { to: "asker", xfer: "x", offset: 0, total: 4 }, bytes(2, 4));
    const data = await asker.nextOf(FRAME.syncData);
    expect(data.header).toEqual({ device: "host", xfer: "x", offset: 0, total: 4 });
    asker.sendFrame(FRAME.syncReq, { to: "nobody" }, bytes(1));
    await asker.settled();
    await bystander.settled();
    expect(asker.frames.filter(f => f.type === FRAME.error)).toEqual([]);
    expect(bystander.frames.filter(f => f.type === FRAME.syncReq || f.type === FRAME.syncData)).toEqual([]);
    for (const client of [host, asker, bystander]) client.socket.close();
  });

  it("refuses relay frames from a socket that didn't ask for the layer", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const old = await connect(name, "old");
    old.sendFrame(FRAME.relay, {}, bytes(1));
    expect((await old.nextOf(FRAME.error)).header.code).toBe("hello_first");
    old.socket.close();
  });

  it("drops frames past a runaway client's budget, without closing it", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const viewer = await connect(name, "viewer", { relay: true });
    const sent = 6100;
    for (let i = 0; i < sent; i++) host.sendFrame(FRAME.announce, {}, bytes(1, 1));
    await host.settled();
    await viewer.settled();
    expect(viewer.frames.filter(f => f.type === FRAME.announce).length).toBe(6000);
    host.socket.close();
    viewer.socket.close();
  });
});

describe("offline queue", () => {
  it("holds a message with the host offline, durably, and owes the host a wake", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true, host: "laptop" });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    expect((await phone.nextOf(FRAME.queued)).header).toEqual({ batchId: "m1", dup: false });
    await runInDurableObject(room(name), async (_instance, state) => {
      expect(pendingChatWake(state.storage.sql)?.host_device).toBe("laptop");
      expect(await state.storage.getAlarm()).not.toBeNull();
      expect(logStats(state.storage.sql)).toMatchObject({ headSeq: 0, rowCount: 0 });
    });
    phone.socket.close();

    // Gone from memory: the queue must come from storage.
    await runInDurableObject(room(name), (_instance, state) => state.abort("simulated eviction")).catch(() => {});
    await enableRelay(name);
    const laptop = await connect(name, "laptop", { relay: true });
    laptop.sendFrame(FRAME.queueReq, {});
    const row = await laptop.nextOf(FRAME.relay);
    expect(row.header).toEqual({ device: "phone", queued: true, batchId: "m1" });
    expect(row.payload).toEqual(bytes(5));
    expect((await laptop.nextOf(FRAME.queueDone)).header.count).toBe(1);
    laptop.socket.close();
  });

  it("a replay is a duplicate while queued, and writes nothing", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    await phone.nextOf(FRAME.queued);
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    expect((await phone.nextOf(FRAME.queued)).header).toEqual({ batchId: "m1", dup: true });
    expect(await queueCount(name)).toBe(1);
    phone.socket.close();
  });

  it("reaches a live host once, ahead of the sender's ack, and a replay doesn't go again", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const laptop = await connect(name, "laptop", { relay: true });
    const phone = await connect(name, "phone", { relay: true, host: "laptop" });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    const live = await laptop.nextOf(FRAME.relay);
    expect(live.header).toEqual({ device: "phone", queued: true, batchId: "m1" });
    await phone.nextOf(FRAME.queued);
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    expect((await phone.nextOf(FRAME.queued)).header).toEqual({ batchId: "m1", dup: true });
    await laptop.settled();
    expect(laptop.frames.filter(f => f.type === FRAME.relay), "no second copy").toHaveLength(0);
    // Rejected sends never go early.
    phone.sendFrame(FRAME.enqueue, { batchId: "empty" }, new Uint8Array(0));
    await phone.nextOf(FRAME.error);
    await laptop.settled();
    expect(laptop.frames.filter(f => f.type === FRAME.relay), "no second copy").toHaveLength(0);
    expect(await queueCount(name)).toBe(1);
    for (const client of [laptop, phone]) client.socket.close();
  });

  it("goes live to connected peers and leaves when the host confirms", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const laptop = await connect(name, "laptop", { relay: true });
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    const live = await laptop.nextOf(FRAME.relay);
    expect(live.header).toEqual({ device: "phone", queued: true, batchId: "m1" });
    expect(await queueCount(name)).toBe(1);
    laptop.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await laptop.settled();
    expect(await queueCount(name)).toBe(0);
    laptop.socket.close();
    phone.socket.close();
  });

  it("an author can't confirm its own message, and a repeat can't touch a newer one", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true });
    const laptop = await connect(name, "laptop", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(1));
    await phone.nextOf(FRAME.queued);
    phone.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await phone.settled();
    expect(await queueCount(name)).toBe(1);
    laptop.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await laptop.settled();
    phone.sendFrame(FRAME.enqueue, { batchId: "m2" }, bytes(2));
    await phone.nextOf(FRAME.queued);
    laptop.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await laptop.settled();
    expect(await queueCount(name)).toBe(1);
    phone.socket.close();
    laptop.socket.close();
  });

  it("rejects what the row log rejects, with the batch id, so the client retires it", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "empty" }, new Uint8Array(0));
    expect((await phone.nextOf(FRAME.error)).header).toMatchObject({ code: "empty", batchId: "empty" });
    phone.socket.close();
  });

  it("deletes only queue rows: stored rows and the checkpoint stay", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const laptop = await connect(name, "laptop", { relay: true });
    const old = await connect(name, "old");
    old.sendFrame(FRAME.push, { batchId: "stored" }, bytes(8));
    await old.nextOf(FRAME.ack);
    old.socket.close();
    await laptop.settled();
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "queued" }, bytes(9));
    await phone.nextOf(FRAME.queued);
    laptop.sendFrame(FRAME.delivered, { batchIds: ["queued", "stored"] });
    await laptop.settled();
    const stats = await runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql));
    expect(stats).toMatchObject({ headSeq: 1, rowCount: 1 });
    expect(await queueCount(name)).toBe(0);
    for (const client of [laptop, phone]) client.socket.close();
  });
});

describe("old clients in the room", () => {
  it("tells relay sockets when an old client says hello", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const old = await connect(name, "old");
    expect((await host.nextOf(FRAME.legacyPeer)).header).toEqual({ device: "old" });
    // A host arriving later hears it from `state` instead.
    const late = await connect(name, "late", { relay: true });
    expect(late.state.header.legacyPeers).toBe(1);
    for (const client of [host, old, late]) client.socket.close();
  });

  it("an HTTP read counts as an old reader unless it's a relay client's bootstrap", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const read = (query: string) =>
      room(name).fetch(`https://chat/rows?after=0&device=reader${query}`, { headers: { [AUTH_USER_HEADER]: user } })
        .then(r => r.arrayBuffer());
    await read("&relay=1");
    await host.settled();
    expect(host.frames.filter(f => f.type === FRAME.legacyPeer)).toEqual([]);
    await read("&relay=1&fallback=1");
    expect((await host.nextOf(FRAME.legacyPeer)).header.device).toBe("reader");
    // Rate-limited, but still counted.
    await read("");
    await host.settled();
    expect(host.frames.filter(f => f.type === FRAME.legacyPeer)).toEqual([]);
    const late = await connect(name, "late", { relay: true });
    expect(late.state.header.legacyPeers).toBe(1);
    host.socket.close();
    late.socket.close();
  });

  it("an old client's push is stored, relayed and acked as always", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const old = await connect(name, "old");
    old.sendFrame(FRAME.push, { batchId: "old-1" }, bytes(4));
    expect((await old.nextOf(FRAME.ack)).header).toMatchObject({ batchId: "old-1", seq: 1 });
    const row = await host.nextOf(FRAME.row);
    expect(row.header).toEqual({ seq: 1, device: "old", batchId: "old-1" });
    host.socket.close();
    old.socket.close();
  });

  it("an old client still gets a checkpoint and rows, with no holes", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    host.sendFrame(FRAME.push, { batchId: "r1" }, bytes(1));
    await host.nextOf(FRAME.ack);
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "q1" }, bytes(2));
    await phone.nextOf(FRAME.queued);
    host.sendFrame(FRAME.delivered, { batchIds: ["q1"] });
    await host.settled();
    host.sendFrame(FRAME.push, { batchId: "r2" }, bytes(3));
    await host.nextOf(FRAME.ack);
    const old = await connect(name, "old");
    expect(old.state.header).toMatchObject({ headSeq: 2, rowCount: 2 });
    old.sendFrame(FRAME.rowsReq, { after: 0, excludeOwn: false });
    expect((await old.nextOf(FRAME.row)).header.seq).toBe(1);
    expect((await old.nextOf(FRAME.row)).header.seq).toBe(2);
    expect((await old.nextOf(FRAME.rowsDone)).header.headSeq).toBe(2);
    for (const client of [host, phone, old]) client.socket.close();
  });
});

describe("who the rooms offer the layer to", () => {
  const withFlag = (name: string, flag: string) =>
    runInDurableObject(room(name), instance => {
      const target = instance as unknown as { env: Record<string, unknown> };
      target.env = { ...target.env, CHAT_RELAY_LAYER: flag };
    });

  it("a list offers it to the rooms of the users on it, and to nobody else", async () => {
    const mine = crypto.randomUUID();
    await withFlag(mine, `someone-else, ${user}`);
    const listed = await connect(mine, "host", { relay: true });
    expect(listed.state.header.relay).toBe(1);
    listed.socket.close();

    const theirs = crypto.randomUUID();
    await withFlag(theirs, "someone-else");
    const unlisted = await connect(theirs, "host", { relay: true });
    expect(unlisted.state.header.relay).toBeUndefined();
    unlisted.socket.close();
  });

  it("taking a user off the list sends their live sockets back to stored rows", async () => {
    const name = crypto.randomUUID();
    await withFlag(name, user);
    const host = await connect(name, "host", { relay: true });
    const closed = new Promise<number>(resolve => host.socket.addEventListener("close", e => resolve(e.code)));
    await withFlag(name, "someone-else");
    host.sendFrame(FRAME.relay, {}, bytes(1));
    expect(await closed).toBe(1012);
  });
});

describe("nobody is left waiting on a host that isn't there", () => {
  const wakeFor = (name: string) =>
    runInDurableObject(room(name), (_instance, state) => pendingChatWake(state.storage.sql)?.host_device);

  it("a reader arriving with the host away wakes it, once", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const reader = await connect(name, "reader", { relay: true, host: "laptop", gen: 3 });
    await expect.poll(() => wakeFor(name)).toBe("laptop");
    await runInDurableObject(room(name), (_instance, state) => {
      state.storage.sql.exec("DELETE FROM chat_wake");
    });
    const second = await connect(name, "second", { relay: true, host: "laptop", gen: 3 });
    await second.settled();
    expect(await wakeFor(name)).toBeUndefined();
    reader.socket.close();
    second.socket.close();
  });

  it("a reader asking a host that has left wakes it again", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const reader = await connect(name, "reader", { relay: true, host: "laptop", gen: 3 });
    await expect.poll(() => wakeFor(name)).toBe("laptop");
    await runInDurableObject(room(name), (instance, state) => {
      state.storage.sql.exec("DELETE FROM chat_wake");
      (instance as unknown as { hostWokenAt: number }).hostWokenAt = 0;
    });
    // Only a request for this chat's host wakes anyone.
    reader.sendFrame(FRAME.syncReq, { to: "elsewhere" }, bytes(1));
    await reader.settled();
    expect(await wakeFor(name)).toBeUndefined();
    reader.sendFrame(FRAME.syncReq, { to: "laptop" }, bytes(1));
    await expect.poll(() => wakeFor(name)).toBe("laptop");
    reader.socket.close();
  });

  it("an old client arriving wakes it too, so it can heal", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "laptop", { relay: true, relaying: true });
    host.socket.close();
    await host.settled().catch(() => {});
    const old = await connect(name, "old", { host: "laptop" });
    await expect.poll(() => wakeFor(name)).toBe("laptop");
    old.socket.close();
  });

  it("an old client with no hint wakes the host the registry names", async () => {
    const name = crypto.randomUUID();
    const org = crypto.randomUUID();
    await enableRelay(name);
    const registry = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${org}/${user}`));
    await runInDurableObject(registry, (_instance, state) => {
      state.storage.sql.exec(
        "INSERT INTO rows (kind, id, seq, deleted, del_hlc, fields, clocks) VALUES ('chats', ?, 1, 0, NULL, ?, '{}')",
        name,
        JSON.stringify({ deviceId: "laptop" })
      );
    });
    const host = await connect(name, "laptop", { relay: true, relaying: true });
    host.socket.close();
    await host.settled().catch(() => {});
    const phone = await connect(name, "phone", { org });
    await expect.poll(() => wakeFor(name)).toBe("laptop");
    phone.socket.close();
  });

  it("a host back on a build without the layer gets what was queued for it", async () => {
    const name = crypto.randomUUID();
    const org = crypto.randomUUID();
    await enableRelay(name);
    const registry = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${org}/${user}`));
    await runInDurableObject(registry, (_instance, state) => {
      state.storage.sql.exec(
        "INSERT INTO rows (kind, id, seq, deleted, del_hlc, fields, clocks) VALUES ('chats', ?, 1, 0, NULL, ?, '{}')",
        name,
        JSON.stringify({ deviceId: "laptop" })
      );
    });
    // The host relays, so the room remembers it, then goes away.
    const host = await connect(name, "laptop", { relay: true, relaying: true });
    host.sendFrame(FRAME.relay, {}, bytes(4));
    await host.settled();
    host.socket.close();
    const phone = await connect(name, "phone", { relay: true, host: "laptop" });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    await phone.nextOf(FRAME.queued);
    // Someone else without the layer arriving doesn't drain it: no heal has landed.
    const other = await connect(name, "old-phone", { org });
    await other.settled();
    expect(await queueCount(name)).toBe(1);
    // The host itself, downgraded, does.
    const old = await connect(name, "laptop", { org });
    const row = await old.nextOf(FRAME.row);
    expect(row.header.batchId).toBe("m1");
    expect(await queueCount(name)).toBe(0);
    for (const client of [phone, other, old]) client.socket.close();
  });

  it("the edge flag alone wakes nobody: a stored chat's readers need no host", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const old = await connect(name, "old", { host: "laptop" });
    const reader = await connect(name, "reader", { relay: true, host: "laptop", gen: 2 });
    await reader.settled();
    expect(await wakeFor(name)).toBeUndefined();
    old.socket.close();
    reader.socket.close();
  });

  it("no wake when the host is already here", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const laptop = await connect(name, "laptop", { relay: true });
    const reader = await connect(name, "reader", { relay: true, host: "laptop" });
    await reader.settled();
    expect(await wakeFor(name)).toBeUndefined();
    laptop.socket.close();
    reader.socket.close();
  });

  it("with the flag off, a reader wakes nobody", async () => {
    const name = crypto.randomUUID();
    const reader = await connect(name, "reader", { relay: true, host: "laptop" });
    await reader.settled();
    expect(await wakeFor(name)).toBeUndefined();
    reader.socket.close();
  });
});

describe("what's queued is never stranded", () => {
  const stats = (name: string) =>
    runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql));

  it("an old client arriving moves the queue into rows it can read", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    await phone.nextOf(FRAME.queued);
    const old = await connect(name, "old");
    expect(old.state.header).toMatchObject({ headSeq: 1, rowCount: 1 });
    expect(await queueCount(name)).toBe(0);
    old.sendFrame(FRAME.rowsReq, { after: 0, excludeOwn: false });
    const row = await old.nextOf(FRAME.row);
    expect(row.header).toEqual({ seq: 1, device: "phone", batchId: "m1" });
    expect(row.payload).toEqual(bytes(5));
    // A relay socket here hears it too.
    expect((await phone.nextOf(FRAME.row)).header.seq).toBe(1);
    phone.socket.close();
    old.socket.close();
  });

  it("turning the layer off moves the queue into rows, once", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const phone = await connect(name, "phone", { relay: true });
    phone.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(5));
    await phone.nextOf(FRAME.queued);
    phone.socket.close();
    // The object restarts with the flag off.
    await runInDurableObject(room(name), (_instance, state) => state.abort("redeploy")).catch(() => {});
    const host = await connect(name, "host");
    expect(host.state.header).toMatchObject({ headSeq: 1, rowCount: 1 });
    const tables = await runInDurableObject(room(name), (_instance, state) =>
      [...state.storage.sql.exec("SELECT name FROM sqlite_master WHERE name='relay_queue'")]
    );
    expect(tables).toEqual([]);
    host.socket.close();
  });

  it("a room that never had the layer is untouched by that check", async () => {
    const name = crypto.randomUUID();
    const host = await connect(name, "host");
    expect(host.state.header).toMatchObject({ headSeq: 0, rowCount: 0 });
    expect(await stats(name)).toMatchObject({ headSeq: 0 });
    host.socket.close();
  });
});

describe("relay-era checkpoints", () => {
  const post = (name: string, query: string) =>
    room(name).fetch(`https://chat/checkpoint?${query}`, {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AQ==" },
      body: bytes(9, 32)
    });

  it("prune nothing, whatever floor the host last knew", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const old = await connect(name, "old");
    for (const batch of ["a", "b", "c"]) {
      old.sendFrame(FRAME.push, { batchId: batch }, bytes(1));
      await old.nextOf(FRAME.ack);
    }
    await (await post(name, "seqCovered=1")).arrayBuffer(); // floor 1
    old.socket.close();
    const stale = await post(name, "seqCovered=0&atFloor=1");
    expect(stale.status).toBe(200);
    await stale.arrayBuffer();
    const stats = await runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql));
    expect(stats).toMatchObject({ headSeq: 3, seqFloor: 1, rowCount: 2, checkpointSeq: 1 });
  });

  it("make a connected old client look again, and only an old one", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const old = await connect(name, "old");
    const relay = await connect(name, "new", { relay: true });
    const closed = new Promise<number>(resolve =>
      old.socket.addEventListener("close", event => resolve(event.code))
    );
    await (await post(name, "seqCovered=0&atFloor=1")).arrayBuffer();
    expect(await closed).toBe(1012);
    await relay.settled();
    relay.socket.close();
  });

  it("an ordinary checkpoint closes nobody", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const old = await connect(name, "old");
    await (await post(name, "seqCovered=0")).arrayBuffer();
    await old.settled();
    old.socket.close();
  });

  it("a socket from before the flag went off is sent to say hello again", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const closed = new Promise<number>(resolve => host.socket.addEventListener("close", e => resolve(e.code)));
    await runInDurableObject(room(name), instance => {
      const target = instance as unknown as { env: Record<string, unknown> };
      target.env = { ...target.env, CHAT_RELAY_LAYER: "" };
    });
    host.sendFrame(FRAME.relay, {}, bytes(1));
    expect(await closed).toBe(1012);
    const again = await connect(name, "host", { relay: true });
    expect(again.state.header.relay).toBeUndefined();
    again.socket.close();
  });

  it("with the flag off, atFloor means nothing", async () => {
    const name = crypto.randomUUID();
    const old = await connect(name, "old");
    old.sendFrame(FRAME.push, { batchId: "a" }, bytes(1));
    await old.nextOf(FRAME.ack);
    await (await post(name, "seqCovered=1&atFloor=1")).arrayBuffer();
    await old.settled();
    const stats = await runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql));
    expect(stats).toMatchObject({ seqFloor: 1, rowCount: 0 });
    old.socket.close();
  });

  it("a host hears an old reader is still here each time it announces", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const host = await connect(name, "host", { relay: true });
    const old = await connect(name, "old");
    await host.nextOf(FRAME.legacyPeer);
    host.sendFrame(FRAME.announce, {}, bytes(1));
    await host.nextOf(FRAME.legacyPeer);
    old.socket.close();
    await host.settled();
    host.socket.close();
  });
});

describe("a stored row can't be left depending on ops no checkpoint holds", () => {
  const rowCount = (name: string) =>
    runInDurableObject(room(name), (_instance, state) => logStats(state.storage.sql).rowCount);
  const checkpoint = (name: string, query: string, frontier = "AQ==") =>
    room(name).fetch(`https://chat/checkpoint?seqCovered=0&atFloor=1${query}`, {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": frontier },
      body: bytes(7, 64)
    });

  /** A host that relayed, a viewer's message queued on top, the host gone. */
  const relayedThenQueued = async (name: string) => {
    const host = await connect(name, "laptop", { relay: true, relaying: true });
    const viewer = await connect(name, "viewer", { relay: true, host: "laptop", gen: 3 });
    host.sendFrame(FRAME.relay, {}, bytes(1));
    await viewer.nextOf(FRAME.relay);
    viewer.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await viewer.nextOf(FRAME.queued);
    host.socket.close();
    await viewer.settled();
    return viewer;
  };

  it("an old client's hello holds the queue while the host hasn't healed", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await relayedThenQueued(name);
    const old = await connect(name, "old", { host: "laptop" });
    await old.settled();
    expect(await rowCount(name)).toBe(0);
    expect(await queueCount(name)).toBe(1);
    expect(old.frames.find(frame => frame.type === FRAME.row)).toBeUndefined();
    viewer.socket.close();
    old.socket.close();
  });

  it("an HTTP read by an old client holds it too", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await relayedThenQueued(name);
    const read = await room(name).fetch(`https://chat/rows?after=0&device=old&chatId=${name}`, {
      headers: { [AUTH_USER_HEADER]: user }
    });
    await read.arrayBuffer();
    expect(await rowCount(name)).toBe(0);
    expect(await queueCount(name)).toBe(1);
    viewer.socket.close();
  });

  it("the heal stores what was held and makes old sockets rejoin", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await relayedThenQueued(name);
    const old = await connect(name, "old", { host: "laptop" });
    await old.settled();
    const closed = new Promise<number>(resolve => old.socket.addEventListener("close", e => resolve(e.code)));
    const healed = await checkpoint(name, "&heal=1");
    await healed.arrayBuffer();
    expect(healed.status).toBe(200);
    expect(await rowCount(name)).toBe(1);
    expect(await queueCount(name)).toBe(0);
    expect(await closed).toBe(1012);
    viewer.socket.close();
  });

  it("a checkpoint that isn't a heal doesn't release it", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await relayedThenQueued(name);
    const old = await connect(name, "old", { host: "laptop" });
    await old.settled();
    await (await checkpoint(name, "")).arrayBuffer();
    expect(await rowCount(name)).toBe(0);
    expect(await queueCount(name)).toBe(1);
    viewer.socket.close();
    old.socket.close();
  });

  it("with the edge flag off the stale queue waits for the heal too", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await relayedThenQueued(name);
    viewer.socket.close();
    await runInDurableObject(room(name), instance => {
      const target = instance as unknown as { env: Record<string, unknown>; staleQueueChecked: boolean };
      target.env = { ...target.env, CHAT_RELAY_LAYER: "" };
      target.staleQueueChecked = false;
    });
    const old = await connect(name, "old", { host: "laptop" });
    await old.settled();
    const held = await runInDurableObject(room(name), (_instance, state) => ({
      rows: logStats(state.storage.sql).rowCount,
      queue: [...state.storage.sql.exec("SELECT COUNT(*) AS n FROM relay_queue")][0]?.n
    }));
    expect(held).toEqual({ rows: 0, queue: 1 });
    await (await checkpoint(name, "&heal=1")).arrayBuffer();
    expect(await rowCount(name)).toBe(1);
    old.socket.close();
  });

  it("a room that never relayed still stores for an old reader at once", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    const viewer = await connect(name, "viewer", { relay: true });
    viewer.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await viewer.nextOf(FRAME.queued);
    const old = await connect(name, "old");
    await old.settled();
    expect(await rowCount(name)).toBe(1);
    expect(await queueCount(name)).toBe(0);
    viewer.socket.close();
    old.socket.close();
  });
});

describe("only the chat's host can retire a queue row", () => {
  const org = "org_1";
  const hostedBy = async (chat: string, device: string) => {
    const registry = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${org}/${user}`));
    await runInDurableObject(registry, (_instance, state) => {
      state.storage.sql.exec(
        "INSERT INTO rows (kind, id, seq, deleted, del_hlc, fields, clocks) VALUES ('chats', ?, 1, 0, NULL, ?, '{}') ON CONFLICT(kind, id) DO UPDATE SET fields = excluded.fields",
        chat,
        JSON.stringify({ deviceId: device })
      );
    });
  };

  it("a stale host's confirmation keeps the row for the new host", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    await hostedBy(name, "new-host");
    const stale = await connect(name, "old-host", { relay: true, org });
    const sender = await connect(name, "sender", { relay: true, org });
    sender.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await sender.nextOf(FRAME.queued);
    stale.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await stale.settled();
    await new Promise(resolve => setTimeout(resolve, 200));
    expect(await queueCount(name)).toBe(1);
    const fresh = await connect(name, "new-host", { relay: true, org });
    fresh.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await expect.poll(() => queueCount(name)).toBe(0);
    for (const client of [stale, sender, fresh]) client.socket.close();
  });

  it("the host can retire a row it wrote before it hosted the chat", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    await hostedBy(name, "new-host");
    const author = await connect(name, "new-host", { relay: true, org });
    author.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await author.nextOf(FRAME.queued);
    author.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await expect.poll(() => queueCount(name)).toBe(0);
    author.socket.close();
  });

  it("a host that has just confirmed loses the right the moment the chat moves", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    await hostedBy(name, "first");
    const first = await connect(name, "first", { relay: true, org });
    const sender = await connect(name, "sender", { relay: true, org });
    sender.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await sender.nextOf(FRAME.queued);
    first.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await expect.poll(() => queueCount(name)).toBe(0);
    await hostedBy(name, "second");
    sender.sendFrame(FRAME.enqueue, { batchId: "m2" }, bytes(3));
    await sender.nextOf(FRAME.queued);
    first.sendFrame(FRAME.delivered, { batchIds: ["m2"] });
    await first.settled();
    await new Promise(resolve => setTimeout(resolve, 200));
    expect(await queueCount(name)).toBe(1);
    for (const client of [first, sender]) client.socket.close();
  });

  it("the host moving is noticed for anyone else", async () => {
    const name = crypto.randomUUID();
    await enableRelay(name);
    await hostedBy(name, "first");
    const first = await connect(name, "first", { relay: true, org });
    const sender = await connect(name, "sender", { relay: true, org });
    sender.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await sender.nextOf(FRAME.queued);
    await hostedBy(name, "second");
    const second = await connect(name, "second", { relay: true, org });
    second.sendFrame(FRAME.delivered, { batchIds: ["m1"] });
    await expect.poll(() => queueCount(name)).toBe(0);
    for (const client of [first, sender, second]) client.socket.close();
  });
});

