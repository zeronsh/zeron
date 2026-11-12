import { env, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME, type Frame } from "../../src/chat-frames";
import { getMeta, headSeq, logStats, rowsAfter, setMeta } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";

/** What a ChatRoom push costs in billed SQLite rows, measured against the real
 * runtime: every statement's `rowsWritten` (which counts index entries), plus
 * `setAlarm`, which Cloudflare bills as a row. */

const user = "writes-owner";
const room = (name: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${name}`));

class Meter {
  statements: Array<{ sql: string; rows: number }> = [];
  alarms = 0;
  settle = () => {};
  /** Billed rows since the last reset. */
  total(): number {
    this.settle();
    return this.statements.reduce((sum, s) => sum + s.rows, 0) + this.alarms;
  }
  reset(): void {
    this.settle();
    this.statements.length = 0;
    this.alarms = 0;
  }
}

/** Wrap this DO instance's storage so every write lands in the meter. */
const meter = async (name: string): Promise<Meter> => {
  const taken = new Meter();
  // The object lives in the test isolate, so the closure sees the live meter.
  await runInDurableObject(room(name), (_instance, state) => {
    const sql = state.storage.sql;
    const exec = sql.exec.bind(sql);
    const pending: Array<{ text: string; cursor: SqlStorageCursor<Record<string, SqlStorageValue>> }> = [];
    taken.settle = () => {
      for (const { text, cursor } of pending.splice(0)) {
        const rows = cursor.rowsWritten;
        if (rows > 0) taken.statements.push({ sql: text, rows });
      }
    };
    sql.exec = ((text: string, ...bindings: SqlStorageValue[]) => {
      taken.settle();
      const cursor = exec(text, ...bindings);
      pending.push({ text, cursor });
      return cursor;
    }) as typeof sql.exec;
    const setAlarm = state.storage.setAlarm.bind(state.storage);
    state.storage.setAlarm = ((...args: Parameters<typeof setAlarm>) => {
      taken.alarms += 1;
      return setAlarm(...args);
    }) as typeof state.storage.setAlarm;
  });
  return taken;
};

const connect = async (name: string, device: string) => {
  const response = await room(name).fetch(`https://chat/ws?device=${device}&chatId=${name}`, {
    headers: { [AUTH_USER_HEADER]: user, upgrade: "websocket" }
  });
  expect(response.status).toBe(101);
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
  const nextOf = async (type: Frame["type"]): Promise<Frame> => {
    for (;;) {
      const index = frames.findIndex(f => f.type === type);
      if (index >= 0) return frames.splice(index, 1)[0]!;
      await new Promise<void>(resolve => waiters.push(resolve));
    }
  };
  const sendFrame = (frame: Uint8Array) =>
    socket.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  sendFrame(encodeFrame(FRAME.hello, { cursor: 0, device }));
  const state = await nextOf(FRAME.state);
  return {
    socket,
    state,
    nextOf,
    push: async (batchId: string, bytes = new Uint8Array(400).fill(7)) => {
      sendFrame(encodeFrame(FRAME.push, { batchId }, bytes));
      return nextOf(FRAME.ack);
    }
  };
};

/** A room created before the batch id lost its index, mid-life: the head only
 * in `meta`, as every existing room has it. */
const seedLegacyRoom = (name: string, rows: number) =>
  runInDurableObject(room(name), (_instance, state) => {
    const sql = state.storage.sql;
    sql.exec("DROP TABLE rows");
    sql.exec(
      "CREATE TABLE rows (seq INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL UNIQUE, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)"
    );
    for (let seq = 1; seq <= rows; seq++) {
      sql.exec("INSERT INTO rows VALUES (?, 'host', ?, x'01', 0)", seq, `legacy-${seq}`);
    }
    setMeta(sql, "headSeq", String(rows));
    setMeta(sql, "backupDirty", "1");
    setMeta(sql, "backupDue", String(Date.now() + 86_400_000));
    setMeta(sql, "pushOutcomes", JSON.stringify({ host: { ok: rows, rejected: 0, lastOkAt: 1 } }));
  });

describe("rows written per push", () => {
  it("a new room pays one row per push and one per pruned row", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    const host = await connect(name, "host");
    // The first push pays one-off setup (first backup deadline, flush alarm).
    await host.push("first");
    taken.reset();
    const pushes = 100;
    for (let i = 0; i < pushes; i++) await host.push(`batch-${i}`);
    expect(taken.total() / pushes).toBe(1);
    expect(taken.alarms).toBe(0);

    taken.reset();
    const checkpoint = await room(name).fetch(`https://chat/checkpoint?seqCovered=${pushes + 1}`, {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AQ==" },
      body: new Uint8Array(32).fill(9)
    });
    expect(checkpoint.status).toBe(200);
    await checkpoint.arrayBuffer();
    taken.settle();
    const pruned = taken.statements.filter(s => s.sql.startsWith("DELETE FROM rows"));
    expect(pruned.reduce((sum, s) => sum + s.rows, 0) / (pushes + 1)).toBe(1);
    host.socket.close();
  });

  it("an existing room keeps its index and pays two rows per push", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 3);
    const taken = await meter(name);
    const host = await connect(name, "host");
    const ack = await host.push("after-upgrade");
    expect(ack.header.seq).toBe(4);
    taken.reset();
    const pushes = 50;
    for (let i = 0; i < pushes; i++) await host.push(`batch-${i}`);
    expect(taken.total() / pushes).toBe(2);
    host.socket.close();
  });

  it("the deferred bookkeeping costs a few rows per flush, however many pushes it covers", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    const host = await connect(name, "host");
    await host.push("first");
    for (let i = 0; i < 50; i++) await host.push(`batch-${i}`);
    await runDurableObjectAlarm(room(name)); // first flush also inserts the meta keys
    // A full cycle: pushes, the alarm they arm, the flush, re-arming for backup.
    taken.reset();
    const pushes = 50;
    for (let i = 0; i < pushes; i++) await host.push(`cycle-${i}`);
    await runDurableObjectAlarm(room(name));
    expect(taken.total() - pushes).toBe(4);
    // Nothing changed since: the next alarm has nothing to flush.
    taken.reset();
    await runDurableObjectAlarm(room(name));
    taken.settle();
    expect(taken.statements).toEqual([]);
    host.socket.close();
  });
});

describe("a cheaper push is no weaker a push", () => {
  it("an acked row, its head and its dedupe survive the object being torn down", async () => {
    const name = crypto.randomUUID();
    const first = await connect(name, "host");
    expect((await first.push("a")).header.seq).toBe(1);
    expect((await first.push("b")).header.seq).toBe(2);
    // Stored head is deliberately behind: no flush has run.
    await runInDurableObject(room(name), (_instance, state) => {
      expect(getMeta(state.storage.sql, "headSeq")).toBeUndefined();
    });
    await runInDurableObject(room(name), (_instance, state) => state.abort("simulated eviction")).catch(() => {});

    const second = await connect(name, "host");
    expect(second.state.header.headSeq).toBe(2);
    expect(second.state.header.rowCount).toBe(2);
    expect((await second.push("b")).header).toMatchObject({ seq: 2, dup: true });
    expect((await second.push("c")).header).toMatchObject({ seq: 3, dup: false });
    second.socket.close();
  });

  it("a checkpoint that prunes every row leaves the head at the floor", async () => {
    const name = crypto.randomUUID();
    const host = await connect(name, "host");
    await host.push("a");
    await host.push("b");
    const checkpoint = await room(name).fetch("https://chat/checkpoint?seqCovered=2", {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AQ==" },
      body: new Uint8Array(8).fill(1)
    });
    await checkpoint.arrayBuffer();
    expect(checkpoint.status).toBe(200);
    expect((await host.push("c")).header.seq).toBe(3);
    await runInDurableObject(room(name), (_instance, state) => {
      expect(headSeq(state.storage.sql)).toBe(3);
      expect([...rowsAfter(state.storage.sql, 0)].map(r => r.seq)).toEqual([3]);
    });
    host.socket.close();
  });

  it("a partial checkpoint keeps later rows and the head", async () => {
    const name = crypto.randomUUID();
    const host = await connect(name, "host");
    for (const batch of ["a", "b", "c"]) await host.push(batch);
    const checkpoint = await room(name).fetch("https://chat/checkpoint?seqCovered=1", {
      method: "POST",
      headers: { [AUTH_USER_HEADER]: user, "x-chat2-frontier": "AQ==" },
      body: new Uint8Array(8).fill(1)
    });
    await checkpoint.arrayBuffer();
    await runInDurableObject(room(name), (_instance, state) => {
      expect(logStats(state.storage.sql)).toMatchObject({ headSeq: 3, seqFloor: 1, rowCount: 2 });
    });
    host.socket.close();
  });

  it("an existing room continues from its stored head, even with every row pruned", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 2);
    await runInDurableObject(room(name), (_instance, state) => {
      state.storage.sql.exec("DELETE FROM rows");
      setMeta(state.storage.sql, "seqFloor", "2");
    });
    const host = await connect(name, "host");
    expect(host.state.header.headSeq).toBe(2);
    expect((await host.push("next")).header.seq).toBe(3);
    host.socket.close();
  });

  it("/stats keeps per-device attribution, before and after the flush and across eviction", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 2);
    const host = await connect(name, "host");
    await host.push("a");
    await host.push("b");
    const stats = async () => {
      const response = await room(name).fetch("https://chat/stats", { headers: { [AUTH_USER_HEADER]: user } });
      return ((await response.json()) as { pushOutcomes: Record<string, { ok: number }> }).pushOutcomes;
    };
    expect((await stats()).host!.ok).toBe(4);
    await runDurableObjectAlarm(room(name));
    expect((await stats()).host!.ok).toBe(4);
    await runInDurableObject(room(name), (_instance, state) => {
      expect(JSON.parse(getMeta(state.storage.sql, "pushOutcomes")!).host.ok).toBe(4);
    });
    await runInDurableObject(room(name), (_instance, state) => state.abort("simulated eviction")).catch(() => {});
    expect((await stats()).host!.ok).toBe(4);
  });

  it("a rejected push still counts, without storing anything per rejection", async () => {
    const name = crypto.randomUUID();
    const host = await connect(name, "host");
    await host.push("a");
    const taken = await meter(name);
    const empty = host.push("empty", new Uint8Array(0));
    void empty;
    const error = await host.nextOf(FRAME.error);
    expect(error.header.code).toBe("empty");
    taken.settle();
    expect(taken.statements).toEqual([]);
    const response = await room(name).fetch("https://chat/stats", { headers: { [AUTH_USER_HEADER]: user } });
    const stats = (await response.json()) as { pushOutcomes: Record<string, { rejected: number }> };
    expect(stats.pushOutcomes.host!.rejected).toBe(1);
    host.socket.close();
  });
});

describe("rolling back past this release", () => {
  /** What the previous release's appendRow computes. */
  const oldNextSeq = (sql: SqlStorage) => Number(getMeta(sql, "headSeq") ?? "0") + 1;

  it("the alarm brings the stored head current, so older code can't collide", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name, 5);
    const host = await connect(name, "host");
    await host.push("a");
    await host.push("b");
    await runDurableObjectAlarm(room(name));
    await runInDurableObject(room(name), (_instance, state) => {
      const sql = state.storage.sql;
      expect(getMeta(sql, "headSeq")).toBe("7");
      expect([...sql.exec("SELECT seq FROM rows WHERE seq = ?", oldNextSeq(sql))]).toEqual([]);
    });
    host.socket.close();
  });

  it("the write-through gate keeps the stored head current on every push", async () => {
    const name = crypto.randomUUID();
    await runInDurableObject(room(name), instance => {
      const target = instance as unknown as { env: Record<string, unknown> };
      target.env = { ...target.env, CHAT_ROOM_WRITE_THROUGH: "1" };
    });
    const host = await connect(name, "host");
    await host.push("a");
    await host.push("b");
    await runInDurableObject(room(name), (_instance, state) => {
      expect(getMeta(state.storage.sql, "headSeq")).toBe("2");
    });
    host.socket.close();
  });

  it("the new table shape works for older code's insert", async () => {
    const name = crypto.randomUUID();
    const host = await connect(name, "host");
    await host.push("a");
    await runInDurableObject(room(name), (_instance, state) => {
      const sql = state.storage.sql;
      expect([...sql.exec("SELECT seq FROM rows WHERE batch_id = ?", "a")]).toHaveLength(1);
      sql.exec("INSERT INTO rows (seq, device, batch_id, bytes, received_at) VALUES (2, 'host', 'b', x'01', 0)");
      setMeta(sql, "headSeq", "2");
      expect(headSeq(sql)).toBe(2);
    });
    host.socket.close();
  });

  describe("a room from before the bound keeps its UNIQUE batch index", () => {
    const seedLegacyRoom = (name: string, rows: number) =>
      runInDurableObject(room(name), (_instance, state) => {
        const sql = state.storage.sql;
        sql.exec("DROP TABLE rows");
        sql.exec(
          "CREATE TABLE rows (seq INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL UNIQUE, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)"
        );
        for (let seq = 1; seq <= rows; seq++) {
          sql.exec("INSERT INTO rows VALUES (?, 'viewer', ?, x'01', 0)", seq, `legacy-${seq}`);
        }
        setMeta(sql, "headSeq", String(rows));
      });

    it("acks a replay older than the dedupe window over HTTP, with its original seq", async () => {
      const name = crypto.randomUUID();
      await seedLegacyRoom(name, 4200);
      const response = await room(name).fetch(
        `https://chat/rows?device=viewer&batchId=legacy-1&chatId=${name}`,
        { method: "POST", headers: { [AUTH_USER_HEADER]: user }, body: new Uint8Array(16).fill(1) }
      );
      expect(response.status).toBe(200);
      expect(((await response.json()) as { seq: number }).seq).toBe(1);
      const head = await runInDurableObject(room(name), (_instance, state) => headSeq(state.storage.sql));
      expect(head).toBe(4200);
    });

    it("acks the same replay over a socket", async () => {
      const name = crypto.randomUUID();
      await seedLegacyRoom(name, 4200);
      const response = await room(name).fetch(`https://chat/ws?device=viewer&chatId=${name}`, {
        headers: { [AUTH_USER_HEADER]: user, upgrade: "websocket" }
      });
      const socket = response.webSocket!;
      socket.accept();
      socket.binaryType = "arraybuffer";
      const frames: Frame[] = [];
      socket.addEventListener("message", event => {
        const frame = decodeFrame(new Uint8Array(event.data as ArrayBuffer));
        if (frame) frames.push(frame);
      });
      const send = (type: Frame["type"], header: Record<string, unknown>, payload?: Uint8Array) => {
        const frame = encodeFrame(type, header, payload);
        socket.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
      };
      const until = async (type: Frame["type"]) => {
        for (let i = 0; i < 300; i++) {
          const found = frames.find(f => f.type === type);
          if (found) return found;
          await new Promise(resolve => setTimeout(resolve, 10));
        }
        throw new Error(`no frame ${type}`);
      };
      send(FRAME.hello, { cursor: 0, device: "viewer" });
      await until(FRAME.state);
      send(FRAME.push, { batchId: "legacy-1" }, new Uint8Array(16).fill(1));
      const ack = await until(FRAME.ack);
      expect(ack.header.seq).toBe(1);
      socket.close();
    });
  });
});
