import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { encodeFrame, FRAME } from "../../src/chat-frames";
import traces from "./fixtures/chat3-cost-traces.json";

/**
 * The chat3 cost proof (docs/chat3-sync.md, "Proving the cost").
 *
 * `crates/doc/examples/chat3_trace.rs` replays agent events through the real
 * fold, SegmentWriter and both doc layouts, and records what each
 * generation sends its room. This replays those traces into a real gen-2 and
 * a real gen-3 ChatRoom (real DO SQLite), calling the room's own `fetch` in
 * process (the routing hop bills nothing and costs ~50 ms a request under
 * miniflare — thousands of them would not fit a test), then
 * reads what each room was billed — the rooms meter their own SQL with the
 * runtime's `rowsWritten`/`rowsRead` counters — and what a cold open
 * downloads.
 */

/** Compact trace events (see chat3_trace.rs): `["p", t, bytes]` a state-log
 * push, `["c", t, bytes]` a gen-2 checkpoint, `["i", t, rows]` a gen-3 upsert
 * batch (each row `[ord, idx, entry, length, hash]`), `["o", t, part, bytes]`
 * a gen-3 tool output. */
type Row = [number, number, string, number, string];
type TraceEvent =
  | ["p", number, number]
  | ["c", number, number]
  | ["i", number, Row[]]
  | ["o", number, string, number];

interface Workload {
  name: string;
  description: string;
  durationMs: number;
  entries: number;
  parts: number;
  commits: number;
  gen2: { events: TraceEvent[]; snapshotBytes: number };
  gen3: {
    events: TraceEvent[];
    stateSnapshotBytes: number;
    transcriptSnapshotBytes: number;
    liveFrames: number;
    liveBytes: number;
  };
}

interface Stats {
  headSeq: number;
  rowCount: number;
  rowBytes: number;
  checkpointSize: number;
  billing: { rowsRead: number; rowsWritten: number; statements: number };
  transcript?: { items: number; itemBytes: number; outputs: number; outputBytes: number };
}

const USER = "chat3-cost";
const auth = { "x-zeron-auth-user": USER };

type Room = { fetch(request: Request): Promise<Response> };

const room = (gen: string, chat: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`${gen}/${chat}`));

/** Run `fn` with the room's live instance (its `fetch`, its metered SQL). */
const inRoom = <T>(gen: string, chat: string, fn: (room: Room) => Promise<T>): Promise<T> =>
  runInDurableObject(room(gen, chat), (instance) => fn(instance as unknown as Room));

const call = (r: Room, path: string, init: RequestInit = {}) =>
  r.fetch(new Request(`https://chat${path}`, { ...init, headers: { ...auth, ...(init.headers ?? {}) } }));

const filler = (n: number): Uint8Array => {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = (i * 31 + 7) & 0xff;
  return out;
};

/** A row's bytes: deterministic from its content hash, so an identical
 * rewrite is byte-identical (the room skips it) and any change differs. */
const rowBytes = (length: number, hash: string): Uint8Array => {
  const out = new Uint8Array(length);
  let seed = parseInt(hash.slice(0, 8), 16) ^ parseInt(hash.slice(8), 16);
  for (let i = 0; i < length; i++) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    out[i] = 32 + (seed >>> 24) % 90;
  }
  return out;
};

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

const ok = async (response: Response): Promise<Response> => {
  if (response.status !== 200) throw new Error(`${response.status}: ${await response.text()}`);
  return response;
};

/** POST one state-log row; returns the seq the room assigned. Device ids
 * rotate so the room's per-device runaway quota (300 pushes a minute)
 * doesn't refuse a replay that runs faster than real time. */
const push = async (r: Room, n: number, bytes: number): Promise<number> => {
  const device = `host-${Math.floor(n / 250)}`;
  const response = await ok(await call(r, `/rows?device=${device}&batchId=b${n}`, { method: "POST", body: filler(bytes) }));
  return ((await response.json()) as { seq: number }).seq;
};

/** Replay, then read the room's meter in the SAME instance call: an idle DO
 * is evicted after ~10 s and its in-memory counters with it. */
const roomStats = async (r: Room): Promise<Stats> => (await (await ok(await call(r, "/stats"))).json()) as Stats;

const replayGen2 = (chat: string, w: Workload): Promise<Stats> =>
  inRoom("chat2", chat, async (r) => {
    let n = 0;
    let head = 0;
    for (const e of w.gen2.events) {
      if (e[0] === "p") {
        head = await push(r, n++, e[2]);
      } else if (e[0] === "c") {
        // The host covers what the room has acknowledged.
        await (
          await ok(
            await call(r, `/checkpoint?seqCovered=${head}`, {
              method: "POST",
              headers: { "x-chat2-frontier": "AQ==" },
              body: filler(e[2])
            })
          )
        ).arrayBuffer();
      }
    }
    return roomStats(r);
  });

const replayGen3 = (chat: string, w: Workload): Promise<Stats> =>
  inRoom("chat3", chat, async (r) => {
    let n = 0;
    for (const e of w.gen3.events) {
      if (e[0] === "p") {
        await push(r, n++, e[2]);
      } else if (e[0] === "i") {
        const body = framed(
          e[2].map(([ord, idx, entry, length, hash]) =>
            encodeFrame(FRAME.itemPut, { ord, idx, entry }, rowBytes(length, hash))
          )
        );
        await (await ok(await call(r, "/items?device=host", { method: "POST", body }))).arrayBuffer();
      } else if (e[0] === "o") {
        await (
          await ok(await call(r, `/outputs/${encodeURIComponent(e[2])}`, { method: "PUT", body: filler(e[3]) }))
        ).arrayBuffer();
      }
    }
    return roomStats(r);
  });

const download = async (r: Room, path: string): Promise<number> => {
  const response = await call(r, path);
  if (response.status === 404) {
    await response.arrayBuffer();
    return 0;
  }
  return (await (await ok(response)).arrayBuffer()).byteLength;
};

/** What a device with nothing cached downloads before it can paint, and the
 * rows the room reads to serve it — one instance call, like the replay. */
const coldOpen = (gen: string, chat: string): Promise<{ bytes: number; rowsRead: number }> =>
  inRoom(gen, chat, async (r) => {
    const before = await roomStats(r);
    // The checkpoint, then the rows above it (`/rows` serves everything
    // after `after`), then — gen 3 — the newest transcript page.
    const checkpointSeq = (before as unknown as { checkpointSeq: number }).checkpointSeq;
    let bytes = await download(r, "/checkpoint");
    bytes += await download(r, `/rows?after=${checkpointSeq}`);
    if (gen === "chat3") bytes += await download(r, "/items?limit=200");
    const after = await roomStats(r);
    // `/stats` scans tables itself (COUNT(*) bills every row it reads): one
    // more reading measures that overhead, which is left out.
    const again = await roomStats(r);
    const overhead = again.billing.rowsRead - after.billing.rowsRead;
    return { bytes, rowsRead: after.billing.rowsRead - before.billing.rowsRead - overhead };
  });

/** Busiest real-time minute of state-log pushes: chat2's room refuses a
 * device past 300 (QUOTA_MAX_PUSHES). */
const peakPushesPerMinute = (events: TraceEvent[]): number => {
  const times = events.filter((e) => e[0] === "p").map((e) => e[1]);
  let peak = 0;
  for (let i = 0, j = 0; j < times.length; j++) {
    while (times[j]! - times[i]! >= 60_000) i++;
    peak = Math.max(peak, j - i + 1);
  }
  return peak;
};

const kb = (n: number) => `${(n / 1024).toFixed(1)} KB`;

describe("chat3 cost on real DO SQLite", () => {
  const workloads = (traces as unknown as { workloads: Workload[] }).workloads;
  const results: Record<string, unknown>[] = [];

  for (const w of workloads) {
    it(`${w.name}: ${w.description}`, async () => {
      const chat = `cost-${w.name}-${crypto.randomUUID().slice(0, 8)}`;
      const s2 = await replayGen2(chat, w);
      const s3 = await replayGen3(chat, w);
      const cold2 = await coldOpen("chat2", chat);
      const cold3 = await coldOpen("chat3", chat);
      const stored2 = s2.rowBytes + s2.checkpointSize;
      const stored3 =
        s3.rowBytes + s3.checkpointSize + (s3.transcript?.itemBytes ?? 0) + (s3.transcript?.outputBytes ?? 0);
      const row = {
        workload: w.name,
        minutes: +(w.durationMs / 60_000).toFixed(1),
        entries: w.entries,
        parts: w.parts,
        gen2Pushes: w.gen2.events.filter((e) => e[0] === "p").length,
        gen2PeakPushesPerMin: peakPushesPerMinute(w.gen2.events),
        gen2RowsWritten: s2.billing.rowsWritten,
        gen3RowsWritten: s3.billing.rowsWritten,
        writeRatio: +(s3.billing.rowsWritten / s2.billing.rowsWritten).toFixed(3),
        gen3StoredRows: s3.transcript?.items ?? 0,
        gen3Outputs: s3.transcript?.outputs ?? 0,
        gen3LiveFrames: w.gen3.liveFrames,
        gen3LiveKB: +(w.gen3.liveBytes / 1024).toFixed(1),
        gen2Stored: kb(stored2),
        gen3Stored: kb(stored3),
        gen2ColdOpen: kb(cold2.bytes),
        gen3ColdOpen: kb(cold3.bytes),
        gen2ColdRowsRead: cold2.rowsRead,
        gen3ColdRowsRead: cold3.rowsRead
      };
      results.push(row);
      console.log(`chat3-cost ${JSON.stringify(row)}`);

      // The claim, pinned: gen 3 bills a fraction of gen 2's writes for the
      // same turns, and a cold open of a long chat downloads less.
      expect(s3.billing.rowsWritten).toBeLessThan(s2.billing.rowsWritten * 0.5);
      // Every stored row of the transcript plane is a part or a header —
      // never a streaming tick.
      expect(s3.transcript!.items).toBeLessThanOrEqual(w.parts + 2 * w.entries + w.commits / 20);
      // Cold open: the newest page plus the small state doc, never the whole
      // transcript's checkpoint.
      if (stored2 > 64 * 1024) expect(cold3.bytes).toBeLessThan(cold2.bytes);
    }, 300_000);
  }
});
