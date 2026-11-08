import { env, runInDurableObject } from "cloudflare:test";
import { expect, it } from "vitest";
import { appendRow, logStats } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";
import { MAX_CHECKPOINT_BYTES, MAX_CHECKPOINT_PART_BYTES } from "../../src/chat-room";

const user = { [AUTH_USER_HEADER]: "user" };

/** A large synthetic checkpoint, made a piece at a time (never whole in the
 * test's memory either): each byte says where it is, so a misplaced part or
 * a wrong range shows. */
const byteAt = (i: number): number => (i * 31 + (i >>> 16)) & 0xff;
const slice = (from: number, to: number): Uint8Array => {
  const bytes = new Uint8Array(to - from);
  for (let i = from; i < to; i++) bytes[i - from] = byteAt(i);
  return bytes;
};

const sendInParts = async (stub: DurableObjectStub, size: number): Promise<number[]> => {
  const statuses: number[] = [];
  for (let offset = 0; offset < size; offset += MAX_CHECKPOINT_PART_BYTES) {
    const part = slice(offset, Math.min(offset + MAX_CHECKPOINT_PART_BYTES, size));
    const res = await stub.fetch(`https://chat/checkpoint-part?offset=${offset}`, { method: "POST", headers: user, body: part });
    statuses.push(res.status);
    await res.arrayBuffer();
  }
  return statuses;
};

/** A response's body read as it streams, checked against `byteAt` from
 * `from`: its length, and the first byte that's wrong (-1: none). */
const check = async (res: Response, from: number): Promise<{ length: number; wrongAt: number }> => {
  const reader = res.body!.getReader();
  let at = from;
  let wrongAt = -1;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    for (let i = 0; i < value.length && wrongAt < 0; i++) if (value[i] !== byteAt(at + i)) wrongAt = at + i;
    at += value.length;
  }
  return { length: at - from, wrongAt };
};

it("takes a checkpoint past one request's cap in parts, and serves it whole and from a range", async () => {
  const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("parts-checkpoint"));
  await runInDurableObject(stub, (_instance, state) => {
    for (let i = 1; i <= 3; i++) appendRow(state.storage.sql, "host", `row-${i}`, new Uint8Array([i]), 0);
  });
  // Past what one request may carry (that stays refused: the 413 test in
  // chat-checkpoint), sent in parts: each taken, then committed by its size.
  const size = MAX_CHECKPOINT_BYTES + 3 * 1024 * 1024 + 12345;
  expect(new Set(await sendInParts(stub, size))).toEqual(new Set([200]));
  const commit = await stub.fetch(`https://chat/checkpoint?seqCovered=2&staged=${size}`, {
    method: "POST", headers: { ...user, "x-chat2-frontier": "AQ==" }
  });
  expect(commit.status).toBe(200);
  expect(await commit.json()).toMatchObject({ ok: true, seqFloor: 2, pruned: 2 });
  const stats = await runInDurableObject(stub, (_instance, state) => logStats(state.storage.sql));
  expect(stats.checkpointSeq).toBe(2);
  expect(stats.checkpointSize).toBe(size);
  expect(stats.rowCount).toBe(1);
  const read = await stub.fetch("https://chat/checkpoint", { headers: user });
  expect(read.status).toBe(200);
  expect(read.headers.get("content-length")).toBe(String(size));
  expect(await check(read, 0)).toEqual({ length: size, wrongAt: -1 });
  // Resumed mid-row (a chunk row is 1.5 MB): the rest, exactly.
  const at = 5_000_001;
  const resumed = await stub.fetch("https://chat/checkpoint", { headers: { ...user, range: `bytes=${at}-` } });
  expect(resumed.status).toBe(206);
  expect(resumed.headers.get("content-range")).toBe(`bytes ${at}-${size - 1}/${size}`);
  expect(await check(resumed, at)).toEqual({ length: size - at, wrongAt: -1 });
});

it("keeps a checkpoint sent in parts whole: out of order, cut short or too large is refused", async () => {
  const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("parts-guards"));
  await runInDurableObject(stub, (_instance, state) => {
    appendRow(state.storage.sql, "host", "row-1", new Uint8Array([1]), 0);
  });
  const post = async (path: string, body?: Uint8Array) => {
    const res = await stub.fetch(`https://chat${path}`, { method: "POST", headers: { ...user, "x-chat2-frontier": "AQ==" }, body });
    const out = { status: res.status, body: await res.json() as Record<string, unknown> };
    return out;
  };
  expect((await post("/checkpoint-part?offset=0", new Uint8Array(10).fill(1))).status).toBe(200);
  // A part not where the staged one ends.
  expect(await post("/checkpoint-part?offset=4", new Uint8Array(10))).toEqual({ status: 409, body: { error: "offset", staged: 10 } });
  // Committed short of its size: refused, the old checkpoint (none) stands.
  expect(await post("/checkpoint?seqCovered=1&staged=20")).toEqual({ status: 409, body: { error: "incomplete" } });
  // Offset 0 starts it again.
  expect((await post("/checkpoint-part?offset=0", new Uint8Array(4).fill(2))).status).toBe(200);
  expect((await post("/checkpoint?seqCovered=1&staged=4")).status).toBe(200);
  const read = await stub.fetch("https://chat/checkpoint", { headers: user });
  expect(new Uint8Array(await read.arrayBuffer())).toEqual(new Uint8Array(4).fill(2));
  // Committed again with nothing staged: refused.
  expect(await post("/checkpoint?seqCovered=1&staged=4")).toEqual({ status: 409, body: { error: "empty" } });
  // A part over the part cap.
  expect((await post("/checkpoint-part?offset=0", new Uint8Array(MAX_CHECKPOINT_PART_BYTES + 1))).status).toBe(413);
});

it("backs a large checkpoint up as an object of its own, streamed", async () => {
  const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("parts-backup"));
  await runInDurableObject(stub, (_instance, state) => {
    appendRow(state.storage.sql, "host", "row-1", new Uint8Array([1]), 0);
    appendRow(state.storage.sql, "host", "row-2", new Uint8Array([2]), 0);
  });
  const size = MAX_CHECKPOINT_PART_BYTES * 2 + 77;
  await sendInParts(stub, size);
  const commit = await stub.fetch(`https://chat/checkpoint?seqCovered=1&staged=${size}`, {
    method: "POST", headers: { ...user, "x-chat2-frontier": "AQ==" }
  });
  expect(commit.status).toBe(200);
  await commit.arrayBuffer();
  const id = await runInDurableObject(stub, async (instance, state) => {
    // (The nightly backup, run now.)
    await (instance as unknown as { backup(): Promise<void> }).backup();
    return state.id.toString();
  });
  const latest = await env.BLOBS.get(`backup/chat2/${id}/latest.json`);
  const json = (await latest!.json()) as Record<string, unknown>;
  expect(json.checkpoint).toBeNull();
  expect(json.checkpointObject).toBe(`backup/chat2/${id}/checkpoint.bin`);
  expect(json.checkpointSize).toBe(size);
  const object = await env.BLOBS.get(`backup/chat2/${id}/checkpoint.bin`);
  expect(await check(new Response(object!.body), 0)).toEqual({ length: size, wrongAt: -1 });
});
