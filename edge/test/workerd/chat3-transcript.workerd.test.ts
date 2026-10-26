import { env, runInDurableObject, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME, type Frame } from "../../src/chat-frames";
import { ensureTranscript, putItem } from "../../src/chat-transcript";
import { meterSql, newSqlMeter } from "../../src/sql-meter";

/** chat3 transcript plane (docs/chat3-sync.md) through the production route,
 * on a real SQLite-backed ChatRoom. */

const owner = "chat3-owner";
const auth = { authorization: `Bearer ${owner}@org` };
const enc = new TextEncoder();
const dec = new TextDecoder();

const framed = (frames: Uint8Array[]): Uint8Array => {
  const out = new Uint8Array(frames.reduce((n, f) => n + 4 + f.length, 0));
  const view = new DataView(out.buffer);
  let off = 0;
  for (const f of frames) {
    view.setUint32(off, f.length, true);
    out.set(f, off + 4);
    off += 4 + f.length;
  }
  return out;
};

const unframe = (bytes: Uint8Array): Frame[] => {
  const frames: Frame[] = [];
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (let off = 0; off < bytes.length; ) {
    const len = view.getUint32(off, true);
    off += 4;
    frames.push(decodeFrame(bytes.subarray(off, off + len))!);
    off += len;
  }
  return frames;
};

interface Put {
  ord: number;
  idx: number;
  entry: string;
  json: unknown;
}

const putItems = async (chat: string, items: Put[], device = "host") => {
  const body = framed(
    items.map((i) => encodeFrame(FRAME.itemPut, { ord: i.ord, idx: i.idx, entry: i.entry }, enc.encode(JSON.stringify(i.json))))
  );
  const response = await SELF.fetch(`https://edge/chat3/${chat}/items?device=${device}`, {
    method: "POST",
    headers: auth,
    body
  });
  expect(response.status, await response.clone().text()).toBe(200);
  return (await response.json()) as { headRev: number; results: { ord: number; idx: number; rev?: number; changed?: boolean; error?: string }[] };
};

const getItems = async (chat: string, query: string) => {
  const response = await SELF.fetch(`https://edge/chat3/${chat}/items?${query}`, { headers: auth });
  expect(response.status).toBe(200);
  return unframe(new Uint8Array(await response.arrayBuffer()));
};

const keys = (frames: Frame[]) =>
  frames.filter((f) => f.type === FRAME.item).map((f) => [f.header.ord, f.header.idx]);

const header = (id: string, ord: number, status = "complete") => ({
  ord,
  idx: -1,
  entry: id,
  json: { id, ord, role: "assistant", createdAt: 1, deviceId: "host", status }
});
const part = (entry: string, ord: number, idx: number, text: string) => ({
  ord,
  idx,
  entry,
  json: { id: `p${idx}`, kind: "text", text }
});

async function socket(chat: string, device: string) {
  const response = await SELF.fetch(`https://edge/chat3/${chat}/ws?device=${device}`, {
    headers: { ...auth, upgrade: "websocket" }
  });
  expect(response.status).toBe(101);
  const ws = response.webSocket!;
  ws.accept();
  ws.binaryType = "arraybuffer";
  const frames: Frame[] = [];
  ws.addEventListener("message", (event) => {
    frames.push(decodeFrame(new Uint8Array(event.data as ArrayBuffer))!);
  });
  ws.send(encodeFrame(FRAME.hello, { device, cursor: 0 }));
  await expect.poll(() => frames.some((f) => f.type === FRAME.state)).toBe(true);
  return { ws, frames };
}

describe("chat3 transcript plane", () => {
  it("pages newest-first, returns rows ascending, and prepends a cut entry's header", async () => {
    const chat = crypto.randomUUID();
    const puts: Put[] = [];
    for (let e = 1; e <= 3; e++) {
      puts.push(header(`e${e}`, e));
      for (let p = 0; p < 4; p++) puts.push(part(`e${e}`, e, p, `entry ${e} part ${p}`));
    }
    const ack = await putItems(chat, puts);
    expect(ack.results.every((r) => r.changed)).toBe(true);
    expect(ack.headRev).toBe(15);

    // Newest 3 rows: e3 parts 1..3, plus e3's header because the cut is mid-entry.
    const last = await getItems(chat, "limit=3");
    expect(keys(last)).toEqual([[3, -1], [3, 1], [3, 2], [3, 3]]);
    const done = last.find((f) => f.type === FRAME.itemsDone)!;
    expect(done.header).toEqual({ headRev: 15, more: true });

    // The page before e3/p1 continues backwards without overlap.
    const older = await getItems(chat, "limit=5&before=3:1");
    expect(keys(older)).toEqual([[2, -1], [2, 1], [2, 2], [2, 3], [3, -1], [3, 0]]);
    // ...and the oldest page reports there is nothing left.
    const first = await getItems(chat, "limit=50&before=2:-1");
    expect(keys(first)).toEqual([[1, -1], [1, 0], [1, 1], [1, 2], [1, 3]]);
    expect(first.find((f) => f.type === FRAME.itemsDone)!.header.more).toBe(false);
  });

  it("orders entries by ord even when one is inserted between two others", async () => {
    const chat = crypto.randomUUID();
    await putItems(chat, [header("a", 1), header("c", 2)]);
    await putItems(chat, [header("b", 1.5)]); // the provider-switch seam
    const page = await getItems(chat, "limit=10");
    expect(page.filter((f) => f.type === FRAME.item).map((f) => f.header.entry)).toEqual(["a", "b", "c"]);
  });

  it("an identical rewrite keeps its rev; a change takes the next one and shows in the feed", async () => {
    const chat = crypto.randomUUID();
    await putItems(chat, [header("e1", 1, "streaming"), part("e1", 1, 0, "hello")]);
    const replay = await putItems(chat, [part("e1", 1, 0, "hello")]);
    expect(replay.results[0]).toMatchObject({ rev: 2, changed: false });
    expect(replay.headRev).toBe(2);

    const settle = await putItems(chat, [part("e1", 1, 0, "hello world"), header("e1", 1, "complete")]);
    expect(settle.results.map((r) => r.rev)).toEqual([3, 4]);
    const feed = await getItems(chat, "since=2");
    expect(feed.filter((f) => f.type === FRAME.item).map((f) => [f.header.idx, f.header.rev])).toEqual([[0, 3], [-1, 4]]);
    const text = JSON.parse(dec.decode(feed.find((f) => f.header.idx === 0)!.payload)).text;
    expect(text).toBe("hello world");
  });

  it("refuses malformed items per row without failing the batch", async () => {
    const chat = crypto.randomUUID();
    const ack = await putItems(chat, [
      { ord: Number.NaN, idx: 0, entry: "e1", json: {} },
      { ord: 1, idx: -2, entry: "e1", json: {} },
      { ord: 1, idx: 0, entry: "bad entry id", json: {} },
      part("e1", 1, 0, "fine")
    ]);
    expect(ack.results.map((r) => r.error ?? "ok")).toEqual(["bad_ord", "bad_idx", "bad_entry", "ok"]);
  });

  it("bills a new item as two rows (row + rev index) and a no-op replay as none", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName(`chat3-bill-${crypto.randomUUID()}`));
    await runInDurableObject(stub, (_i, state) => {
      const meter = newSqlMeter();
      const sql = meterSql(state.storage.sql, meter);
      ensureTranscript(sql);
      const bytes = enc.encode('{"kind":"text","text":"x"}');
      let before = meter.rowsWritten;
      putItem(sql, { ord: 1, idx: 0, entry: "e" }, bytes);
      expect(meter.rowsWritten - before).toBe(2);
      before = meter.rowsWritten;
      putItem(sql, { ord: 1, idx: 0, entry: "e" }, bytes);
      expect(meter.rowsWritten - before).toBe(0);
      before = meter.rowsWritten;
      putItem(sql, { ord: 1, idx: 0, entry: "e" }, enc.encode('{"kind":"text","text":"xy"}'));
      // An update moves the row's rev, so the index entry moves with it.
      expect(meter.rowsWritten - before).toBeLessThanOrEqual(3);
    });
  });

  it("relays stored rows and live frames, and a late viewer rebuilds the open part", async () => {
    const chat = crypto.randomUUID();
    const host = await socket(chat, "host");
    const viewer = await socket(chat, "viewer");

    // Live: a part frame then an append, relayed to the viewer, not echoed.
    const live = { entry: "e1", ord: 1, idx: 0 };
    host.ws.send(encodeFrame(FRAME.live, { ...live, op: "part" }, enc.encode('{"id":"p0","kind":"text","text":"Hel"}')));
    host.ws.send(encodeFrame(FRAME.live, { ...live, op: "append", at: 3 }, enc.encode("lo")));
    await expect.poll(() => viewer.frames.filter((f) => f.type === FRAME.live).length).toBe(2);
    expect(host.frames.some((f) => f.type === FRAME.live)).toBe(false);

    // A viewer arriving now gets the open part on its first read.
    const late = await getItems(chat, "limit=10");
    const rebuilt = late.filter((f) => f.type === FRAME.live);
    expect(rebuilt.map((f) => f.header.op)).toEqual(["part", "append"]);
    expect(dec.decode(rebuilt[1]!.payload)).toBe("lo");
    const stats = (await (await SELF.fetch(`https://edge/chat3/${chat}/stats`, { headers: auth })).json()) as {
      transcript: { liveParts: number; items: number };
      billing: { rowsWritten: number };
    };
    expect(stats.transcript.liveParts).toBe(1);
    // Live frames are never stored.
    expect(stats.transcript.items).toBe(0);

    // The part settles: the stored row reaches the viewer (not the host
    // device that wrote it) and replaces the live copy.
    await putItems(chat, [header("e1", 1), part("e1", 1, 0, "Hello")], "host");
    await expect.poll(() => viewer.frames.filter((f) => f.type === FRAME.item).length).toBe(2);
    expect(host.frames.some((f) => f.type === FRAME.item)).toBe(false);
    const after = await getItems(chat, "limit=10");
    expect(after.some((f) => f.type === FRAME.live)).toBe(false);
    host.ws.close();
    viewer.ws.close();
  });

  it("stores full tool outputs apart from the rows and serves them on demand", async () => {
    const chat = crypto.randomUUID();
    const output = "line\n".repeat(50_000); // 250 KB
    const put = await SELF.fetch(`https://edge/chat3/${chat}/outputs/toolu_01abc`, {
      method: "PUT",
      headers: auth,
      body: output
    });
    expect(put.status).toBe(200);
    await put.arrayBuffer();
    const got = await SELF.fetch(`https://edge/chat3/${chat}/outputs/toolu_01abc`, { headers: auth });
    expect(got.status).toBe(200);
    expect(await got.text()).toBe(output);
    const missing = await SELF.fetch(`https://edge/chat3/${chat}/outputs/nope`, { headers: auth });
    expect(missing.status).toBe(404);
    await missing.arrayBuffer();
    const bad = await SELF.fetch(`https://edge/chat3/${chat}/outputs/${encodeURIComponent("a/b")}`, { headers: auth });
    expect(bad.status).toBe(404);
    await bad.arrayBuffer();
    const tooBig = await SELF.fetch(`https://edge/chat3/${chat}/outputs/big`, {
      method: "PUT",
      headers: auth,
      body: new Uint8Array(1024 * 1024 + 1)
    });
    expect(tooBig.status).toBe(413);
    await tooBig.arrayBuffer();
  });

  it("keeps the generations apart: chat2 has no transcript routes, and the rooms differ", async () => {
    const chat = crypto.randomUUID();
    const chat2Items = await SELF.fetch(`https://edge/chat2/${chat}/items?limit=1`, { headers: auth });
    expect(chat2Items.status).toBe(404);
    await chat2Items.arrayBuffer();
    // Same id, different rooms: a chat3 push never lands in the chat2 log.
    const push = await SELF.fetch(`https://edge/chat3/${chat}/rows?device=d&batchId=b1`, {
      method: "POST",
      headers: auth,
      body: new Uint8Array([1])
    });
    expect(push.status).toBe(200);
    await push.arrayBuffer();
    const chat2Stats = (await (await SELF.fetch(`https://edge/chat2/${chat}/stats`, { headers: auth })).json()) as {
      headSeq: number;
      transcript?: unknown;
    };
    expect(chat2Stats.headSeq).toBe(0);
    expect(chat2Stats.transcript).toBeUndefined();
  });

  it("only the owner may read a chat3 room", async () => {
    const chat = crypto.randomUUID();
    await putItems(chat, [header("e1", 1)]);
    const other = await SELF.fetch(`https://edge/chat3/${chat}/items?limit=1`, {
      headers: { authorization: "Bearer someone-else@org" }
    });
    expect(other.status).toBe(403);
    await other.arrayBuffer();
  });
});
