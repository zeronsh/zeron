import { env, runDurableObjectAlarm, runInDurableObject, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME, type Frame } from "../../src/chat-frames";
import { logStats } from "../../src/chat-log";
import {
  CLOSE_DRAFT_DISCARDED,
  DRAFT_IDLE_TTL_MS,
  draftRoomName,
  MAX_CHECKPOINT_BYTES,
  MAX_ROW_BYTES
} from "../../src/draft-protocol";
import { AUTH_USER_HEADER } from "../../src/env";

/** DraftRoom (docs/draft-sync.md §1) on a real SQLite-backed DO, reached
 * through the production route seam (draft-route.ts via the test fixture,
 * whose bearer is `user@org`). loro-wasm is not needed: the room only relays
 * opaque bytes. */

const ORG = "org1";
const bearer = (user: string, org = ORG) => ({ authorization: `Bearer ${user}@${org}` });
const url = (chat: string, action: string, query = "", org = ORG) =>
  `https://edge/draft/${org}/${chat}/${action}${query}`;
const call = (
  user: string,
  chat: string,
  action: string,
  query = "",
  init: RequestInit = {},
  org = ORG
) =>
  SELF.fetch(url(chat, action, query, org), {
    ...init,
    headers: { ...bearer(user, org), ...(init.headers as Record<string, string> | undefined) }
  });
const epochOf = async (user: string, chat: string): Promise<number> =>
  ((await (await call(user, chat, "epoch")).json()) as { epoch: number }).epoch;
const push = (user: string, chat: string, epoch: number, batchId: string, bytes: Uint8Array, device = "dev") =>
  call(user, chat, "rows", `?epoch=${epoch}&device=${device}&batchId=${batchId}`, {
    method: "POST",
    body: bytes
  });
const stubFor = (user: string, chat: string, org = ORG) =>
  env.DRAFT_ROOMS.get(env.DRAFT_ROOMS.idFromName(draftRoomName(org, user, chat)));
const bytes = (len: number, fill = 7) => new Uint8Array(len).fill(fill);

/** Decode a `/rows` GET body (u32-LE length-prefixed frames). */
const decodeRowsBody = (body: Uint8Array): Frame[] => {
  const out: Frame[] = [];
  const view = new DataView(body.buffer, body.byteOffset);
  for (let off = 0; off < body.length; ) {
    const len = view.getUint32(off, true);
    out.push(decodeFrame(body.subarray(off + 4, off + 4 + len))!);
    off += 4 + len;
  }
  return out;
};

class Sock {
  readonly inbox: Frame[] = [];
  closed?: { code: number; reason: string };
  private constructor(readonly ws: WebSocket) {}
  static async dial(user: string, chat: string, epoch: number, device: string): Promise<Sock> {
    const res = await SELF.fetch(url(chat, "ws", `?epoch=${epoch}&device=${device}`), {
      headers: { ...bearer(user), upgrade: "websocket" }
    });
    expect(res.status).toBe(101);
    const ws = res.webSocket!;
    const sock = new Sock(ws);
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (ev) => {
      sock.inbox.push(decodeFrame(new Uint8Array(ev.data as ArrayBuffer))!);
    });
    ws.addEventListener("close", (ev) => {
      sock.closed = { code: ev.code, reason: ev.reason };
    });
    ws.accept();
    return sock;
  }
  send(type: (typeof FRAME)[keyof typeof FRAME], header: Record<string, unknown>, payload?: Uint8Array) {
    const f = encodeFrame(type, header, payload);
    this.ws.send(f.buffer.slice(f.byteOffset, f.byteOffset + f.byteLength) as ArrayBuffer);
  }
  async next(type: number): Promise<Frame> {
    for (let i = 0; i < 200; i++) {
      const at = this.inbox.findIndex((f) => f.type === type);
      if (at >= 0) return this.inbox.splice(at, 1)[0]!;
      await new Promise((r) => setTimeout(r, 10));
    }
    throw new Error(`timeout waiting for frame ${type}`);
  }
  async waitClosed(): Promise<{ code: number; reason: string }> {
    for (let i = 0; i < 200 && !this.closed; i++) await new Promise((r) => setTimeout(r, 10));
    if (!this.closed) throw new Error("socket did not close");
    return this.closed;
  }
}

describe("DraftRoom epoch", () => {
  it("starts at epoch 1", async () => {
    const res = await call("alice", "epoch-start", "epoch");
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ epoch: 1 });
  });

  it("rejects rows, checkpoint and ws on a missing or mismatching epoch with 409", async () => {
    const chat = "epoch-gate";
    for (const q of ["", "?epoch=2", "?epoch=0", "?epoch=junk", "?epoch=01"]) {
      const get = await call("alice", chat, "rows", q);
      expect(get.status, `GET rows ${q}`).toBe(409);
      expect(await get.json()).toEqual({ error: "epoch_mismatch", epoch: 1 });
      const post = await call("alice", chat, "rows", `${q}${q ? "&" : "?"}batchId=b1`, {
        method: "POST",
        body: bytes(4)
      });
      expect(post.status, `POST rows ${q}`).toBe(409);
      await post.arrayBuffer();
      const cget = await call("alice", chat, "checkpoint", q);
      expect(cget.status, `GET checkpoint ${q}`).toBe(409);
      await cget.arrayBuffer();
      const cpost = await call("alice", chat, "checkpoint", `${q}${q ? "&" : "?"}seqCovered=0`, {
        method: "POST",
        body: bytes(4)
      });
      expect(cpost.status, `POST checkpoint ${q}`).toBe(409);
      expect(await cpost.json()).toEqual({ error: "epoch_mismatch", epoch: 1 });
      const ws = await SELF.fetch(url(chat, "ws", q), {
        headers: { ...bearer("alice"), upgrade: "websocket" }
      });
      expect(ws.status, `ws ${q}`).toBe(409);
      expect(ws.webSocket).toBeNull();
      await ws.arrayBuffer();
    }
    // Nothing was written by any rejected request.
    const stats = await runInDurableObject(stubFor("alice", chat), (_i, s) => logStats(s.storage.sql));
    expect(stats.headSeq).toBe(0);
    expect(stats.rowCount).toBe(0);
  });

  it("accepts rows and checkpoint at the current epoch", async () => {
    const chat = "epoch-ok";
    const ack = await push("alice", chat, 1, "b1", bytes(10));
    expect(ack.status).toBe(200);
    expect(await ack.json()).toEqual({ batchId: "b1", seq: 1, dup: false });
    const dup = await push("alice", chat, 1, "b1", bytes(10));
    expect(await dup.json()).toEqual({ batchId: "b1", seq: 1, dup: true });

    const rows = await call("alice", chat, "rows", "?epoch=1&after=0");
    expect(rows.status).toBe(200);
    const frames = decodeRowsBody(new Uint8Array(await rows.arrayBuffer()));
    expect(frames.map((f) => f.type)).toEqual([FRAME.state, FRAME.row, FRAME.rowsDone]);
    expect(frames[1]!.payload).toEqual(bytes(10));

    const cp = await call("alice", chat, "checkpoint", "?epoch=1&seqCovered=1", {
      method: "POST",
      headers: { "x-chat2-frontier": "AQ==" },
      body: bytes(100, 9)
    });
    expect(await cp.json()).toEqual({ ok: true, seqFloor: 1, pruned: 1 });
    const got = await call("alice", chat, "checkpoint", "?epoch=1", { headers: { range: "bytes=90-" } });
    expect(got.status).toBe(206);
    expect(got.headers.get("x-chat2-checkpoint-seq")).toBe("1");
    expect(new Uint8Array(await got.arrayBuffer())).toEqual(bytes(10, 9));
  });

  it("has no tail/diff/reset sidecars and enforces the method allow-list", async () => {
    for (const [method, action] of [
      ["GET", "tail"],
      ["PUT", "diff"],
      ["POST", "reset"],
      ["POST", "epoch"],
      ["GET", "discard"],
      ["POST", "stats"],
      ["PUT", "rows"]
    ] as const) {
      const res = await call("alice", "sidecars", action, "?epoch=1", { method });
      expect(res.status, `${method} ${action}`).toBe(404);
      await res.arrayBuffer();
    }
  });
});

describe("DraftRoom discard", () => {
  it("wipes rows and checkpoint, bumps the epoch, and rejects the old epoch", async () => {
    const chat = "discard-wipe";
    await push("alice", chat, 1, "b1", bytes(10));
    await push("alice", chat, 1, "b2", bytes(10));
    await call("alice", chat, "checkpoint", "?epoch=1&seqCovered=1", {
      method: "POST",
      headers: { "x-chat2-frontier": "AQ==" },
      body: bytes(50)
    });

    const res = await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ epoch: 2, discarded: true });
    expect(await epochOf("alice", chat)).toBe(2);

    // Storage really is empty (no history left), only the epoch meta remains.
    await runInDurableObject(stubFor("alice", chat), (_i, s) => {
      const sql = s.storage.sql;
      expect([...sql.exec("SELECT COUNT(*) AS n FROM rows")][0]!.n).toBe(0);
      expect([...sql.exec("SELECT COUNT(*) AS n FROM blobs")][0]!.n).toBe(0);
      const meta = Object.fromEntries([...sql.exec("SELECT key, value FROM meta")].map((r) => [r.key, r.value]));
      expect(meta.epoch).toBe("2");
      expect(meta.headSeq).toBeUndefined();
      expect(meta.checkpointSeq).toBeUndefined();
      expect(meta.pushOutcomes).toBeUndefined();
    });

    const stale = await call("alice", chat, "rows", "?epoch=1");
    expect(stale.status).toBe(409);
    expect(await stale.json()).toEqual({ error: "epoch_mismatch", epoch: 2 });
    const cp = await call("alice", chat, "checkpoint", "?epoch=2");
    expect(cp.status).toBe(404);
    await cp.arrayBuffer();
    const fresh = await push("alice", chat, 2, "b1", bytes(5));
    expect(await fresh.json()).toEqual({ batchId: "b1", seq: 1, dup: false });
  });

  it("is a no-op returning the current epoch for a stale epoch (idempotent)", async () => {
    const chat = "discard-stale";
    await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
    await push("alice", chat, 2, "keep", bytes(5));
    for (let i = 0; i < 2; i++) {
      const res = await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({ epoch: 2, discarded: false });
    }
    // A future epoch is also "not equal": no-op.
    const future = await call("alice", chat, "discard", "?epoch=9", { method: "POST" });
    expect(await future.json()).toEqual({ epoch: 2, discarded: false });
    const stats = await runInDurableObject(stubFor("alice", chat), (_i, s) => logStats(s.storage.sql));
    expect(stats.rowCount).toBe(1);
    expect(await epochOf("alice", chat)).toBe(2);
  });

  it("rejects a missing or invalid epoch with 400", async () => {
    for (const q of ["", "?epoch=", "?epoch=0", "?epoch=-1", "?epoch=abc"]) {
      const res = await call("alice", "discard-bad", "discard", q, { method: "POST" });
      expect(res.status, q).toBe(400);
      await res.arrayBuffer();
    }
    expect(await epochOf("alice", "discard-bad")).toBe(1);
  });

  it("closes every open socket with 4411 'draft discarded'", async () => {
    const chat = "discard-sockets";
    const a = await Sock.dial("alice", chat, 1, "devA");
    const b = await Sock.dial("alice", chat, 1, "devB");
    a.send(FRAME.hello, { cursor: 0, device: "devA" });
    b.send(FRAME.hello, { cursor: 0, device: "devB" });
    await a.next(FRAME.state);
    await b.next(FRAME.state);

    const res = await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
    expect(await res.json()).toEqual({ epoch: 2, discarded: true });
    expect(await a.waitClosed()).toEqual({ code: CLOSE_DRAFT_DISCARDED, reason: "draft discarded" });
    expect(await b.waitClosed()).toEqual({ code: CLOSE_DRAFT_DISCARDED, reason: "draft discarded" });

    // A redial on the old epoch is refused; on the new epoch it works.
    const old = await SELF.fetch(url(chat, "ws", "?epoch=1&device=devA"), {
      headers: { ...bearer("alice"), upgrade: "websocket" }
    });
    expect(old.status).toBe(409);
    await old.arrayBuffer();
    const again = await Sock.dial("alice", chat, 2, "devA");
    again.send(FRAME.hello, { cursor: 0, device: "devA" });
    const state = await again.next(FRAME.state);
    expect(state.header.headSeq).toBe(0);
    again.ws.close(1000, "done");
  });
});

describe("DraftRoom websocket protocol", () => {
  it("hello/state, push/ack, relay to the second socket, backfill, probe", async () => {
    const chat = "ws-flow";
    const a = await Sock.dial("alice", chat, 1, "devA");
    const b = await Sock.dial("alice", chat, 1, "devB");
    a.send(FRAME.hello, { cursor: 0, device: "devA" });
    b.send(FRAME.hello, { cursor: 0, device: "devB" });
    expect((await a.next(FRAME.state)).header.headSeq).toBe(0);
    await b.next(FRAME.state);

    a.send(FRAME.push, { batchId: "p1" }, bytes(20, 3));
    expect((await a.next(FRAME.ack)).header).toEqual({ batchId: "p1", seq: 1, dup: false });
    const relayed = await b.next(FRAME.row);
    expect(relayed.header).toEqual({ seq: 1, device: "devA", batchId: "p1" });
    expect(relayed.payload).toEqual(bytes(20, 3));
    expect(a.inbox.filter((f) => f.type === FRAME.row)).toHaveLength(0);

    b.send(FRAME.rowsReq, { after: 0 });
    expect((await b.next(FRAME.row)).header.seq).toBe(1);
    expect((await b.next(FRAME.rowsDone)).header.headSeq).toBe(1);
    b.send(FRAME.probe, {});
    expect((await b.next(FRAME.probeOk)).header.headSeq).toBe(1);

    a.ws.close(1000, "done");
    b.ws.close(1000, "done");
  });
});

describe("DraftRoom privacy and routing", () => {
  it("gives two users isolated rooms for the same org and chat id", async () => {
    const chat = "shared-id";
    await push("alice", chat, 1, "a1", bytes(10));
    await push("alice", chat, 1, "a2", bytes(10));
    await call("alice", chat, "discard", "?epoch=1", { method: "POST" });

    // bob's room for the same chat id is untouched: still epoch 1, empty.
    expect(await epochOf("bob", chat)).toBe(1);
    const bobPush = await push("bob", chat, 1, "b1", bytes(10));
    expect(await bobPush.json()).toEqual({ batchId: "b1", seq: 1, dup: false });

    const aliceRows = await call("alice", chat, "rows", "?epoch=2");
    const aliceFrames = decodeRowsBody(new Uint8Array(await aliceRows.arrayBuffer()));
    expect(aliceFrames.filter((f) => f.type === FRAME.row)).toHaveLength(0);
    const bobRows = await call("bob", chat, "rows", "?epoch=1");
    const bobFrames = decodeRowsBody(new Uint8Array(await bobRows.arrayBuffer()));
    expect(bobFrames.filter((f) => f.type === FRAME.row)).toHaveLength(1);

    // Separate Durable Objects underneath.
    expect(stubFor("alice", chat).id.toString()).not.toBe(stubFor("bob", chat).id.toString());
    const bobStats = await runInDurableObject(stubFor("bob", chat), (_i, s) => logStats(s.storage.sql));
    expect(bobStats.rowCount).toBe(1);
  });

  it("does not let a caller pick their own room via spoofed headers or query", async () => {
    const chat = "spoof";
    await push("alice", chat, 1, "a1", bytes(10));
    const res = await SELF.fetch(url(chat, "rows", "?epoch=1&user=alice&batchId=x"), {
      method: "POST",
      headers: { ...bearer("mallory"), [AUTH_USER_HEADER]: "alice" },
      body: bytes(3)
    });
    expect(res.status).toBe(200);
    await res.arrayBuffer();
    const alice = await runInDurableObject(stubFor("alice", chat), (_i, s) => logStats(s.storage.sql));
    expect(alice.rowCount).toBe(1);
    const mallory = await runInDurableObject(stubFor("mallory", chat), (_i, s) => logStats(s.storage.sql));
    expect(mallory.rowCount).toBe(1);
  });

  it("answers 403 when the org claim differs from the URL org, on every route", async () => {
    for (const action of ["ws", "checkpoint", "rows", "epoch", "discard", "stats"]) {
      const res = await SELF.fetch(url("c1", action, "?epoch=1", "org2"), {
        method: action === "discard" ? "POST" : "GET",
        headers: { ...bearer("alice", "org1"), upgrade: "websocket" }
      });
      expect(res.status, action).toBe(403);
      await res.arrayBuffer();
    }
    const noOrg = await SELF.fetch("https://edge/draft/org1/c1/epoch", {
      headers: { authorization: "Bearer alice" }
    });
    expect(noOrg.status).toBe(403);
    await noOrg.arrayBuffer();
    // The forbidden request never created a room.
    const stats = await runInDurableObject(stubFor("alice", "c1", "org2"), (_i, s) => logStats(s.storage.sql));
    expect(stats.headSeq).toBe(0);
  });

  it("validates the chat id, requires an upgrade for ws, and 404s unknown routes", async () => {
    const bad = await call("alice", "bad!id", "epoch");
    expect(bad.status).toBe(400);
    await bad.arrayBuffer();
    const noUpgrade = await call("alice", "c1", "ws", "?epoch=1");
    expect(noUpgrade.status).toBe(426);
    await noUpgrade.arrayBuffer();
    const unknown = await call("alice", "c1", "nope");
    expect(unknown.status).toBe(404);
    await unknown.arrayBuffer();
    const short = await SELF.fetch("https://edge/draft/org1/c1", { headers: bearer("alice") });
    expect(short.status).toBe(404);
    await short.arrayBuffer();
  });

  it("reports stats, including the epoch", async () => {
    const chat = "stats";
    await push("alice", chat, 1, "s1", bytes(10), "devS");
    const res = await call("alice", chat, "stats");
    const stats = (await res.json()) as Record<string, unknown> & { pushOutcomes: Record<string, { ok: number }> };
    expect(stats.epoch).toBe(1);
    expect(stats.headSeq).toBe(1);
    expect(stats.pushOutcomes.devS!.ok).toBe(1);
    expect(stats.idleExpiresAt as number).toBeGreaterThan(Date.now() + DRAFT_IDLE_TTL_MS - 60_000);
  });
});

describe("DraftRoom idle expiry", () => {
  it("arms the alarm on first contact, so a merely opened chat still expires", async () => {
    const chat = "alarm-first-contact";
    await call("alice", chat, "epoch");
    const at = await runInDurableObject(stubFor("alice", chat), (_i, s) => s.storage.getAlarm());
    expect(at).not.toBeNull();
    expect(Math.abs(at! - (Date.now() + DRAFT_IDLE_TTL_MS))).toBeLessThan(60_000);
  });

  it("arms an alarm ~30 days out on writes", async () => {
    const chat = "alarm-arm";
    await push("alice", chat, 1, "b1", bytes(10));
    const at = await runInDurableObject(stubFor("alice", chat), (_i, s) => s.storage.getAlarm());
    expect(at).not.toBeNull();
    expect(Math.abs(at! - (Date.now() + DRAFT_IDLE_TTL_MS))).toBeLessThan(60_000);
  });

  it("re-arms instead of deleting when the room is not yet idle", async () => {
    const chat = "alarm-early";
    await push("alice", chat, 1, "b1", bytes(10));
    expect(await runDurableObjectAlarm(stubFor("alice", chat))).toBe(true);
    const stats = await runInDurableObject(stubFor("alice", chat), (_i, s) => logStats(s.storage.sql));
    expect(stats.rowCount).toBe(1);
    const at = await runInDurableObject(stubFor("alice", chat), (_i, s) => s.storage.getAlarm());
    expect(at).not.toBeNull();
  });

  it("deletes all storage once idle for the TTL, resets the epoch and closes sockets", async () => {
    const chat = "alarm-expire";
    await push("alice", chat, 1, "b1", bytes(10));
    await call("alice", chat, "checkpoint", "?epoch=1&seqCovered=1", {
      method: "POST",
      headers: { "x-chat2-frontier": "AQ==" },
      body: bytes(50)
    });
    await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
    await push("alice", chat, 2, "b2", bytes(10));
    const sock = await Sock.dial("alice", chat, 2, "devA");
    sock.send(FRAME.hello, { cursor: 0, device: "devA" });
    await sock.next(FRAME.state);
    expect(await epochOf("alice", chat)).toBe(2);
    sock.ws.addEventListener("close", () => undefined);

    const stub = stubFor("alice", chat);
    // Age the room past the TTL (the alarm decides from the recorded last write).
    await runInDurableObject(stub, (_i, s) => {
      s.storage.sql.exec(
        "UPDATE meta SET value = ? WHERE key = 'lastWriteAt'",
        String(Date.now() - DRAFT_IDLE_TTL_MS - 1000)
      );
    });
    expect(await runDurableObjectAlarm(stub)).toBe(true);

    expect((await sock.waitClosed()).code).toBe(CLOSE_DRAFT_DISCARDED);
    await runInDurableObject(stub, async (_i, s) => {
      const sql = s.storage.sql;
      expect([...sql.exec("SELECT COUNT(*) AS n FROM rows")][0]!.n).toBe(0);
      expect([...sql.exec("SELECT COUNT(*) AS n FROM blobs")][0]!.n).toBe(0);
      expect([...sql.exec("SELECT COUNT(*) AS n FROM meta")][0]!.n).toBe(0);
      expect(await s.storage.getAlarm()).toBeNull();
    });
    // The room is gone: epoch restarts at 1 and it accepts writes again.
    expect(await epochOf("alice", chat)).toBe(1);
    const again = await push("alice", chat, 1, "b1", bytes(10));
    expect(await again.json()).toEqual({ batchId: "b1", seq: 1, dup: false });
  });

  it("a discard also re-arms the alarm", async () => {
    const chat = "alarm-discard";
    await call("alice", chat, "discard", "?epoch=1", { method: "POST" });
    const at = await runInDurableObject(stubFor("alice", chat), (_i, s) => s.storage.getAlarm());
    expect(at).not.toBeNull();
    expect(Math.abs(at! - (Date.now() + DRAFT_IDLE_TTL_MS))).toBeLessThan(60_000);
  });
});

describe("DraftRoom limits", () => {
  it("rejects an oversize row over HTTP (413) and WS (too_large error, socket stays open)", async () => {
    const chat = "oversize-row";
    const ok = await push("alice", chat, 1, "fits", bytes(MAX_ROW_BYTES));
    expect(ok.status).toBe(200);
    await ok.arrayBuffer();
    const big = await push("alice", chat, 1, "big", bytes(MAX_ROW_BYTES + 1));
    expect(big.status).toBe(413);
    expect(await big.json()).toEqual({ error: "too_large" });
    const huge = await push("alice", chat, 1, "huge", bytes(MAX_ROW_BYTES + 8192));
    expect(huge.status).toBe(413);
    await huge.arrayBuffer();

    const sock = await Sock.dial("alice", chat, 1, "devA");
    sock.send(FRAME.hello, { cursor: 0, device: "devA" });
    await sock.next(FRAME.state);
    sock.send(FRAME.push, { batchId: "ws-big" }, bytes(MAX_ROW_BYTES + 1));
    const err = await sock.next(FRAME.error);
    expect(err.header.code).toBe("too_large");
    expect(err.header.batchId).toBe("ws-big");
    sock.send(FRAME.probe, {});
    await sock.next(FRAME.probeOk);
    const stats = await runInDurableObject(stubFor("alice", chat), (_i, s) => logStats(s.storage.sql));
    expect(stats.rowCount).toBe(1);
    sock.ws.close(1000, "done");
  });

  it("closes a websocket that sends a frame beyond the frame budget with 1009", async () => {
    const chat = "oversize-frame";
    const sock = await Sock.dial("alice", chat, 1, "devA");
    sock.send(FRAME.hello, { cursor: 0, device: "devA" });
    await sock.next(FRAME.state);
    sock.send(FRAME.push, { batchId: "frame-big" }, bytes(MAX_ROW_BYTES + 16 * 1024));
    expect((await sock.waitClosed()).code).toBe(1009);
  });

  it("rejects an oversize checkpoint with 413 and keeps the existing one", async () => {
    const chat = "oversize-checkpoint";
    const fits = await call("alice", chat, "checkpoint", "?epoch=1&seqCovered=0", {
      method: "POST",
      body: bytes(MAX_CHECKPOINT_BYTES, 4)
    });
    expect(fits.status).toBe(200);
    await fits.arrayBuffer();
    const big = await call("alice", chat, "checkpoint", "?epoch=1&seqCovered=0", {
      method: "POST",
      body: bytes(MAX_CHECKPOINT_BYTES + 1, 5)
    });
    expect(big.status).toBe(413);
    expect(await big.json()).toEqual({ error: "too_large" });
    const got = await call("alice", chat, "checkpoint", "?epoch=1");
    expect(new Uint8Array(await got.arrayBuffer())[0]).toBe(4);
  });

  it("still enforces the 300 pushes / 60s per-device quota", async () => {
    const chat = "quota";
    for (let i = 0; i < 300; i++) {
      const res = await push("alice", chat, 1, `q${i}`, bytes(3), "devQ");
      expect(res.status, `push ${i}`).toBe(200);
      await res.arrayBuffer();
    }
    const over = await push("alice", chat, 1, "q300", bytes(3), "devQ");
    expect(over.status).toBe(429);
    expect(await over.json()).toEqual({ error: "quota" });
    // Quota is per device.
    const other = await push("alice", chat, 1, "other", bytes(3), "devOther");
    expect(other.status).toBe(200);
    await other.arrayBuffer();

    const sock = await Sock.dial("alice", chat, 1, "devQ");
    sock.send(FRAME.hello, { cursor: 0, device: "devQ" });
    await sock.next(FRAME.state);
    sock.send(FRAME.push, { batchId: "ws-q" }, bytes(3));
    const err = await sock.next(FRAME.error);
    expect(err.header).toMatchObject({ code: "quota", batchId: "ws-q" });
    sock.ws.close(1000, "done");
  }, 30_000);
});

describe("draft route hardening", () => {
  it("404s actions that name Object.prototype members instead of throwing", async () => {
    for (const action of ["constructor", "toString", "__proto__", "hasOwnProperty"]) {
      const res = await call("alice", "proto-actions", action, "", { method: "POST", body: "x" });
      expect(res.status, action).toBe(404);
      await res.arrayBuffer();
    }
  });

  it("rejects a POST that declares an oversized body before buffering it", async () => {
    const res = await call("alice", "declared-big", "checkpoint", "?epoch=1&seqCovered=1", {
      method: "POST",
      headers: { "content-length": String(50 * 1024 * 1024) },
      body: new Uint8Array(4)
    });
    expect(res.status).toBe(413);
    await res.arrayBuffer();
  });
});
