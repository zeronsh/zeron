import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME, type Frame } from "../../src/chat-frames";
import { getMeta, headSeq, setMeta } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";

const user = "regressions-owner";
const room = (name: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${name}`));
const bytes = (fill: number, len = 16) => new Uint8Array(len).fill(fill);

const withEnv = (name: string, vars: Record<string, string>) =>
  runInDurableObject(room(name), instance => {
    const target = instance as unknown as { env: Record<string, unknown> };
    target.env = { ...target.env, ...vars };
  });

const seedLegacyRoom = (name: string, rows: number) =>
  runInDurableObject(room(name), (_instance, state) => {
    const sql = state.storage.sql;
    sql.exec("DROP TABLE rows");
    sql.exec("CREATE TABLE rows (seq INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL UNIQUE, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)");
    for (let seq = 1; seq <= rows; seq++) {
      sql.exec("INSERT INTO rows VALUES (?, 'viewer', ?, x'01', 0)", seq, `legacy-${seq}`);
    }
    setMeta(sql, "headSeq", String(rows));
  });

const connect = async (name: string, device: string, relay = false) => {
  const response = await room(name).fetch(`https://chat/ws?device=${device}&chatId=${name}`, {
    headers: { [AUTH_USER_HEADER]: user, upgrade: "websocket" }
  });
  const socket = response.webSocket!;
  socket.accept();
  socket.binaryType = "arraybuffer";
  const frames: Frame[] = [];
  const waiters: Array<() => void> = [];
  socket.addEventListener("message", event => {
    const frame = decodeFrame(new Uint8Array(event.data as ArrayBuffer));
    if (frame) frames.push(frame);
    waiters.splice(0).forEach(wake => wake());
  });
  const nextOf = async (type: Frame["type"], ms = 3000): Promise<Frame | "timeout"> => {
    const deadline = Date.now() + ms;
    for (;;) {
      const index = frames.findIndex(f => f.type === type);
      if (index >= 0) return frames.splice(index, 1)[0]!;
      const left = deadline - Date.now();
      if (left <= 0) return "timeout";
      await Promise.race([
        new Promise<void>(resolve => waiters.push(resolve)),
        new Promise<void>(resolve => setTimeout(resolve, left))
      ]);
    }
  };
  const sendFrame = (type: Frame["type"], header: Record<string, unknown>, payload?: Uint8Array) => {
    const frame = encodeFrame(type, header, payload);
    socket.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  };
  sendFrame(FRAME.hello, { cursor: 0, device, ...(relay ? { relay: 1 } : {}) });
  return { socket, frames, nextOf, sendFrame };
};

describe("relay layer regressions", () => {
  // #1251: the comment says existing rooms answer an old replay from their index,
  // but the query's `seq > head - DEDUPE_WINDOW` excludes it, and the INSERT then
  // hits the UNIQUE constraint that room still has. Before #1251: a dup ack.
  it("a pre-#1251 room still acks a replay older than the dedupe window (HTTP)", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 4200);
    let status: number | string;
    try {
      const response = await room(name).fetch(
        `https://chat/rows?device=viewer&batchId=legacy-1&chatId=${name}`,
        { method: "POST", headers: { [AUTH_USER_HEADER]: user }, body: bytes(1) }
      );
      status = response.status;
      await response.arrayBuffer();
    } catch (error) {
      status = String(error);
    }
    expect(status).toBe(200);
  });

  it("a pre-#1251 room still acks a replay older than the dedupe window (socket)", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 4200);
    const client = await connect(name, "viewer");
    await client.nextOf(FRAME.state);
    client.sendFrame(FRAME.push, { batchId: "legacy-1" }, bytes(1));
    const ack = await client.nextOf(FRAME.ack);
    expect(ack).not.toBe("timeout");
  });

  // Same collision through the drain: every later hello dies before `state`.
  it("an old client's hello survives a queued batch already stored past the window", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 4200);
    await withEnv(name, { CHAT_RELAY_LAYER: "1" });
    const viewer = await connect(name, "viewer", true);
    await viewer.nextOf(FRAME.state);
    viewer.sendFrame(FRAME.enqueue, { batchId: "legacy-1" }, bytes(2));
    expect(await viewer.nextOf(FRAME.queued)).not.toBe("timeout");
    const old = await connect(name, "old");
    expect(await old.nextOf(FRAME.state)).not.toBe("timeout");
  });

  // Rollback gate: a release that reads meta.headSeq collides on rows.seq.
  it("with write-through on, a drained row keeps the stored head current", async () => {
    const name = crypto.randomUUID();
    await withEnv(name, { CHAT_RELAY_LAYER: "1", CHAT_ROOM_WRITE_THROUGH: "1" });
    const viewer = await connect(name, "viewer", true);
    await viewer.nextOf(FRAME.state);
    viewer.sendFrame(FRAME.enqueue, { batchId: "q1" }, bytes(2));
    await viewer.nextOf(FRAME.queued);
    const old = await connect(name, "old");
    await old.nextOf(FRAME.state);
    const heads = await runInDurableObject(room(name), (_instance, state) => ({
      derived: headSeq(state.storage.sql),
      stored: Number(getMeta(state.storage.sql, "headSeq") ?? "0")
    }));
    expect(heads.stored).toBe(heads.derived);
  });

  // The room's checkpoint is the only stored copy; rows at or below the floor are
  // gone. Nothing stops an at-floor checkpoint from a doc that lacks them.
  it("an at-floor checkpoint can't replace a newer one with an older or empty doc", async () => {
    const name = crypto.randomUUID();
    await withEnv(name, { CHAT_RELAY_LAYER: "1" });
    const writer = await connect(name, "other-device");
    await writer.nextOf(FRAME.state);
    for (const batch of ["a", "b"]) {
      writer.sendFrame(FRAME.push, { batchId: batch }, bytes(1));
      await writer.nextOf(FRAME.ack);
    }
    const full = await room(name).fetch("https://chat/checkpoint?seqCovered=2", {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AQ==" },
      body: bytes(7, 64) // the transcript
    });
    await full.arrayBuffer();
    expect(full.status).toBe(200);
    // seqCovered=0: exactly what spawn_chat2_checkpoint sends with no client yet.
    const stale = await room(name).fetch("https://chat/checkpoint?seqCovered=0&atFloor=1", {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AA==" }, // encoded empty VV
      body: bytes(0, 8) // an empty doc's snapshot
    });
    await stale.arrayBuffer();
    const served = await room(name).fetch("https://chat/checkpoint", { headers: { [AUTH_USER_HEADER]: user } });
    const body = new Uint8Array(await served.arrayBuffer());
    expect(body.byteLength).toBe(64);
  });
  // A reader on a socket with no relay frames never hears of a row queued after
  // its hello: the drain only runs on a legacy hello or read. Same for a host on
  // an older build that a sender's registry still lists as queue-capable.
  it("a connected old socket gets a message queued after it said hello", async () => {
    const name = crypto.randomUUID();
    await withEnv(name, { CHAT_RELAY_LAYER: "1" });
    const old = await connect(name, "old-build-host");
    await old.nextOf(FRAME.state);
    const viewer = await connect(name, "viewer", true);
    await viewer.nextOf(FRAME.state);
    viewer.sendFrame(FRAME.enqueue, { batchId: "m1" }, bytes(2));
    await viewer.nextOf(FRAME.queued);
    expect(await old.nextOf(FRAME.row, 1000)).not.toBe("timeout");
  });
});

