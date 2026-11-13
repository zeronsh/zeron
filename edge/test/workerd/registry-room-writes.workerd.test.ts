import { env, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";
import type { Op, Row } from "../../src/registry-core";

/** What a RegistryRoom push costs in billed SQLite rows, measured against the
 * real runtime: every statement's `rowsWritten` (which counts index entries),
 * plus `setAlarm`, which Cloudflare bills as a row. */

const H = { [AUTH_USER_HEADER]: "user" };
const room = (name: string) => env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/org/${name}`));

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

/** Wrap this object's storage so every write lands in the meter (beneath the
 * room's own meter, so the two can be compared). */
const meter = async (name: string): Promise<Meter> => {
  const taken = new Meter();
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

let tick = 0;
const hlc = (ms = Date.now()) => `${String(ms).padStart(13, "0")}-${String(++tick % 1_000_000).padStart(6, "0")}-mac`;
/** The host's session row as a turn streams: only `updatedAt` moves. */
const heartbeat = (chat = "c1"): Op => ({
  kind: "sessions",
  id: chat,
  op: "upsert",
  hlc: hlc(),
  set: { chatId: chat, deviceId: "mac", status: "working", lastCompletedTurn: null, updatedAt: Date.now(), runningSubagents: 0 }
});
const chat = (id: string, set: Record<string, string | number | null> = {}): Op => ({
  kind: "chats",
  id,
  op: "upsert",
  hlc: hlc(),
  set: { id, deviceId: "mac", title: "Fix login", ...set }
});

const push = async (name: string, ops: Op[], device = "mac") => {
  const response = await room(name).fetch(`https://registry/push?device=${device}`, {
    method: "POST",
    headers: H,
    body: JSON.stringify({ batch: crypto.randomUUID(), ops })
  });
  expect(response.status).toBe(200);
  return response.json<{ seq: number; applied: number }>();
};

interface Stats {
  seq: number;
  pushOutcomes: Record<string, { ok: number; rejected: number }>;
  pushLog: Array<{ chatId: string; category: string }>;
  writesSinceStart: { rows: number; alarms: number };
}
const stats = async (name: string): Promise<Stats> => (await room(name).fetch("https://registry/stats", { headers: H })).json();

const pull = async (name: string, since?: number) => {
  const query = since === undefined ? "" : `?since=${since}`;
  return (await room(name).fetch(`https://registry/rows${query}`, { headers: H })).json<{ seq: number; full: boolean; rows: Row[] }>();
};

const meta = (name: string, key: string) =>
  runInDurableObject(room(name), (_instance, state) => {
    const rows = [...state.storage.sql.exec("SELECT value FROM meta WHERE key = ?", key)];
    return rows[0]?.value as string | undefined;
  });

const evict = (name: string) =>
  runInDurableObject(room(name), (_instance, state) => state.abort("simulated eviction")).catch(() => {});

/** A room as the release before this one left it mid-life: the seq stored
 * per push, the backup marked with no deadline, push counts stored. */
const seedLegacyRoom = async (name: string) => {
  await push(name, [chat("c1"), heartbeat()]);
  await push(name, [heartbeat()]);
  await runInDurableObject(room(name), (_instance, state) => {
    const sql = state.storage.sql;
    sql.exec("DELETE FROM meta");
    for (const [key, value] of [
      ["seq", "2"],
      ["backupDirty", "1"],
      ["pushOutcomes", JSON.stringify({ mac: { ok: 2, rejected: 0, lastOkAt: 1 } })]
    ]) {
      sql.exec("INSERT INTO meta (key, value) VALUES (?, ?)", key, value);
    }
  });
  // A fresh object: nothing this release keeps in memory.
  await evict(name);
};

describe("rows written per registry push", () => {
  it("a push that moves one row writes that row and its seq index entry", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    // The first push pays one-off setup (backup mark, flush alarm).
    await push(name, [chat("c1"), heartbeat()]);
    taken.reset();
    const pushes = 50;
    for (let i = 0; i < pushes; i++) await push(name, [heartbeat()]);
    expect(taken.total() / pushes).toBe(2);
    expect(taken.alarms).toBe(0);
    taken.settle();
    expect(new Set(taken.statements.map(s => s.sql.split(" (")[0]))).toEqual(new Set(["INSERT INTO rows"]));
  });

  it("a batch pays per row it moves, nothing per batch", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    await push(name, [chat("c1"), heartbeat()]);
    taken.reset();
    // The end of a turn: preview and status together.
    await push(name, [chat("c1", { lastMessagePreview: "done", lastMessageAt: Date.now() }), heartbeat()]);
    expect(taken.total()).toBe(4);
    // A replay changes nothing and writes nothing.
    taken.reset();
    const replay = await push(name, [{ ...heartbeat(), hlc: "0000000000001-000000-mac" }]);
    expect(replay.applied).toBe(0);
    expect(taken.total()).toBe(0);
  });

  it("the deferred bookkeeping costs a few rows per flush, however many pushes it covers", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    await push(name, [chat("c1"), heartbeat()]);
    await runDurableObjectAlarm(room(name)); // the first flush also inserts the meta keys
    taken.reset();
    // A full cycle: pushes, the alarm they arm, the flush, re-arming for the backup.
    const pushes = 50;
    for (let i = 0; i < pushes; i++) await push(name, [heartbeat()]);
    await runDurableObjectAlarm(room(name));
    expect(taken.total() - 2 * pushes).toBe(4);
    // Nothing changed since: the next run stores nothing, only re-arms the backup.
    taken.reset();
    await runDurableObjectAlarm(room(name));
    taken.settle();
    expect(taken.statements).toEqual([]);
    expect(taken.alarms).toBeLessThanOrEqual(1);
  });

  it("the room's own meter agrees with the runtime's", async () => {
    const name = crypto.randomUUID();
    const taken = await meter(name);
    const before = (await stats(name)).writesSinceStart;
    taken.reset();
    for (let i = 0; i < 10; i++) await push(name, [heartbeat()]);
    const after = (await stats(name)).writesSinceStart;
    expect(after.rows + after.alarms - (before.rows + before.alarms)).toBe(taken.total());
  });
});

describe("a cheaper push is no weaker a push", () => {
  it("acked rows and the head survive the object being torn down", async () => {
    const name = crypto.randomUUID();
    expect((await push(name, [chat("c1")])).seq).toBe(1);
    expect((await push(name, [heartbeat()])).seq).toBe(2);
    // The stored copy is deliberately behind: no flush has run.
    expect(await meta(name, "seq")).toBeUndefined();
    await evict(name);
    const all = await pull(name, 0);
    expect(all.seq).toBe(2);
    expect(all.rows.map(r => [r.kind, r.seq])).toEqual([["chats", 1], ["sessions", 2]]);
    const delta = await pull(name, 1);
    expect(delta.full).toBe(false);
    expect(delta.rows.map(r => r.kind)).toEqual(["sessions"]);
    expect((await push(name, [heartbeat()])).seq).toBe(3);
  });

  it("a cursor ahead of the head still reads as a wiped room", async () => {
    const name = crypto.randomUUID();
    await push(name, [chat("c1")]);
    expect((await pull(name, 5)).full).toBe(true);
  });

  it("a probe answers the derived head", async () => {
    const name = crypto.randomUUID();
    await push(name, [chat("c1")]);
    await push(name, [heartbeat()]);
    const response = await room(name).fetch("https://registry/ws?device=mac", { headers: { ...H, upgrade: "websocket" } });
    const socket = response.webSocket!;
    socket.accept();
    const frames: Array<Record<string, unknown>> = [];
    socket.addEventListener("message", event => frames.push(JSON.parse(event.data as string)));
    socket.send(JSON.stringify({ t: "hello", cursor: 2, device: "mac" }));
    socket.send(JSON.stringify({ t: "probe" }));
    for (let i = 0; i < 200 && !frames.some(f => f.t === "probe-ok"); i++) await new Promise(r => setTimeout(r, 5));
    expect(frames.find(f => f.t === "state")).toMatchObject({ seq: 2, full: false, rows: [] });
    expect(frames.find(f => f.t === "probe-ok")).toMatchObject({ seq: 2 });
    socket.close();
  });

  it("tombstone GC that removes the top rows leaves the head at the floor", async () => {
    const name = crypto.randomUUID();
    await push(name, [chat("c1")]);
    const old = Date.now() - 40 * 86_400_000;
    await push(name, [{ kind: "chats", id: "gone", op: "delete", hlc: hlc(old) }]);
    await runInDurableObject(room(name), (_instance, state) => {
      state.storage.sql.exec("UPDATE meta SET value = '1' WHERE key = 'backupDue'");
    });
    await runDurableObjectAlarm(room(name));
    // The floor alone holds the head, with no stored copy to lean on.
    await runInDurableObject(room(name), (_instance, state) => {
      state.storage.sql.exec("DELETE FROM meta WHERE key = 'seq'");
    });
    const after = await pull(name, 0);
    expect(after.rows.map(r => r.id)).toEqual(["c1"]);
    expect(after.seq).toBe(2);
    expect((await pull(name, 2)).full).toBe(false);
    expect((await push(name, [heartbeat()])).seq).toBe(3);
  });

  it("/stats keeps per-device attribution, before and after the flush and across eviction", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name);
    await push(name, [heartbeat()]);
    await push(name, [heartbeat()], "phone");
    expect((await stats(name)).pushOutcomes).toMatchObject({ mac: { ok: 3 }, phone: { ok: 1 } });
    await runDurableObjectAlarm(room(name));
    expect(JSON.parse((await meta(name, "pushOutcomes"))!)).toMatchObject({ mac: { ok: 3 }, phone: { ok: 1 } });
    await evict(name);
    expect((await stats(name)).pushOutcomes).toMatchObject({ mac: { ok: 3 }, phone: { ok: 1 } });
  });

  it("a rejected push is stored at once, as one row", async () => {
    const name = crypto.randomUUID();
    await push(name, [chat("c1")]);
    await runDurableObjectAlarm(room(name));
    const taken = await meter(name);
    const response = await room(name).fetch("https://registry/push?device=mac", {
      method: "POST",
      headers: H,
      body: JSON.stringify({ batch: "b", ops: [{ kind: "chats", id: "bad id!", op: "upsert", hlc: hlc(), set: {} }] })
    });
    expect(response.status).toBe(400);
    expect(taken.total()).toBe(1);
    // The rejection survives eviction: it's what an incident is debugged by.
    await evict(name);
    expect((await stats(name)).pushOutcomes.mac).toMatchObject({ ok: 1, rejected: 1 });
  });

  it("a notification's log is stored once per notifying push", async () => {
    const name = crypto.randomUUID();
    await room(name).fetch("https://registry/push-target?device=ios-1", {
      method: "POST",
      headers: H,
      body: JSON.stringify({ token: "ab".repeat(32), environment: "production" })
    });
    const done = (turn: string): Op => ({ ...heartbeat(), set: { ...heartbeat().set, status: "idle", lastCompletedTurn: turn } });
    await push(name, [chat("c1"), heartbeat()]);
    await push(name, [done("t1")]);
    await push(name, [heartbeat()]);
    const taken = await meter(name);
    await push(name, [done("t2")]);
    // The session row (2) and the log (1).
    expect(taken.total()).toBe(3);
    await evict(name);
    expect((await stats(name)).pushLog.map(e => [e.chatId, e.category])).toEqual([
      ["c1", "done"],
      ["c1", "done"]
    ]);
  });
});

describe("a room from the release before", () => {
  it("continues from its stored seq and pays for the backup mark once", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name);
    const taken = await meter(name);
    expect((await push(name, [heartbeat()])).seq).toBe(3);
    // The one-off deadline for the backup the old release marked.
    expect(taken.statements.map(s => s.sql.split(" (")[0])).toContain("INSERT INTO meta");
    taken.reset();
    for (let i = 0; i < 10; i++) await push(name, [heartbeat()]);
    expect(taken.total()).toBe(20);
  });

  it("backs up a table the old release marked when its alarm fires", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name);
    await runDurableObjectAlarm(room(name));
    expect(await meta(name, "backupDirty")).toBe("0");
    expect(await meta(name, "backupSeq")).toBe("2");
  });
});

describe("rolling back past this release", () => {
  /** What the previous release's applyPushBatch computes. */
  const oldNextSeq = async (name: string) => Number((await meta(name, "seq")) ?? "0") + 1;

  it("the alarm brings the stored seq current, so older code can't stamp below a cursor", async () => {
    const name = crypto.randomUUID();
    await seedLegacyRoom(name);
    await push(name, [heartbeat()]);
    await push(name, [chat("c1", { title: "renamed" })]);
    await runDurableObjectAlarm(room(name));
    expect(await meta(name, "seq")).toBe("4");
    expect(await oldNextSeq(name)).toBe(5);
  });

  it("the write-through gate keeps the stored seq current on every push", async () => {
    const name = crypto.randomUUID();
    await runInDurableObject(room(name), instance => {
      const target = instance as unknown as { env: Record<string, unknown> };
      target.env = { ...target.env, REGISTRY_ROOM_WRITE_THROUGH: "1" };
    });
    await push(name, [chat("c1")]);
    await push(name, [heartbeat()]);
    expect(await meta(name, "seq")).toBe("2");
  });
});
