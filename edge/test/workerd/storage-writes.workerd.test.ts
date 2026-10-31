import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import { CHUNK_BYTES, createBlobStore } from "../../src/blobs";
import { ensureChatLog, getMeta, setMeta } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";
import { PushDiagnostics } from "../../src/push-diagnostics";

/** Measure actual SQLite row writes, including index maintenance, rather
 * than counting SQL statements (one statement can write many rows). */
function meter(sql: SqlStorage) {
  let writes = 0;
  const measured = new Proxy(sql, {
    get(target, key) {
      if (key === "exec") return (...args: Parameters<SqlStorage["exec"]>) => {
        const cursor = target.exec(...args);
        writes += cursor.rowsWritten;
        return cursor;
      };
      return Reflect.get(target, key, target);
    }
  });
  return { sql: measured, writes: () => writes };
}

function inSql(fn: (sql: SqlStorage) => void) {
  const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName(crypto.randomUUID()));
  return runInDurableObject(stub, (_instance, state) => fn(state.storage.sql));
}

describe("Durable Object write budgets", () => {
  it.each(["chat", "registry"])("keeps %s receipts, backup deadlines and live attribution while skipping repeated metadata writes", async (kind) => {
    const ns = kind === "chat" ? env.CHAT_ROOMS : env.REGISTRY_ROOMS;
    const stub = ns.get(ns.idFromName(crypto.randomUUID()));
    const headers = { [AUTH_USER_HEADER]: "user" };
    const push = async (batch: number) => {
      const response = kind === "chat"
        ? await stub.fetch(`https://room/rows?device=host&batchId=b${batch}`, { method: "POST", headers, body: new Uint8Array([batch]) })
        : await stub.fetch("https://room/push?device=host", { method: "POST", headers, body: JSON.stringify({
          batch: `b${batch}`,
          ops: [{ kind: "chats", id: "chat", op: "upsert", hlc: `${String(batch).padStart(13, "0")}-000000-host`, set: { title: `Title ${batch}` } }]
        }) });
      expect(response.status).toBe(200);
      expect(await response.json()).toMatchObject({ seq: batch });
    };
    await push(1);
    const deadline = await runInDurableObject(stub, async (_instance, state) => {
      const sql = state.storage.sql;
      // Audit updates at the real request boundary, including the registry's
      // separate metadata setter. A no-op UPDATE would still fire this.
      sql.exec("CREATE TABLE meta_updates (key TEXT NOT NULL)");
      sql.exec("CREATE TRIGGER audit_meta AFTER UPDATE ON meta BEGIN INSERT INTO meta_updates VALUES (NEW.key); END");
      return state.storage.getAlarm();
    });
    expect(deadline).not.toBeNull();
    await push(2);
    await push(3);
    const response = await stub.fetch("https://room/stats", { headers });
    expect(await response.json()).toMatchObject({ pushOutcomes: { host: { ok: 3, rejected: 0 } } });
    await runInDurableObject(stub, async (_instance, state) => {
      expect(await state.storage.getAlarm()).toBe(deadline);
      const sql = state.storage.sql;
      expect(getMeta(sql, "backupDirty")).toBe("1");
      expect([...sql.exec("SELECT key FROM meta_updates WHERE key IN ('backupDirty', 'pushOutcomes')")]).toEqual([]);
      expect(JSON.parse(getMeta(sql, "pushOutcomes")!).host.ok).toBe(1);
    });
    const reset = await stub.fetch("https://room/reset", { method: "POST", headers });
    expect(reset.status).toBe(200);
    await reset.text();
    const empty = await stub.fetch("https://room/stats", { headers });
    expect(await empty.json()).toMatchObject({ pushOutcomes: {} });
    await push(1);
    const reseeded = await stub.fetch("https://room/stats", { headers });
    expect(await reseeded.json()).toMatchObject({ pushOutcomes: { host: { ok: 1, rejected: 0 } } });
  });

  it("does not rewrite dirty flags or unchanged metadata, but persists transitions", async () => {
    await inSql((sql) => {
      ensureChatLog(sql);
      const m = meter(sql);
      setMeta(m.sql, "backupDirty", "1");
      const first = m.writes();
      expect(first).toBeGreaterThan(0);
      for (let i = 0; i < 100; i++) setMeta(m.sql, "backupDirty", "1");
      expect(m.writes()).toBe(first);
      setMeta(m.sql, "backupDirty", "0");
      expect(getMeta(sql, "backupDirty")).toBe("0");
      setMeta(m.sql, "backupDirty", "1");
      expect(getMeta(sql, "backupDirty")).toBe("1");
      expect(m.writes()).toBeGreaterThan(first);
    });
  });

  it("updates only changed blob chunks, removes a shrinking suffix and preserves empty blobs", async () => {
    await inSql((sql) => {
      const m = meter(sql);
      const blobs = createBlobStore(m.sql);
      const original = new Uint8Array(CHUNK_BYTES + 3).fill(7);
      blobs.put("tail", original);
      blobs.put("other", new Uint8Array([9]));
      const first = m.writes();
      blobs.put("tail", original);
      expect(m.writes()).toBe(first);
      const changed = original.slice();
      changed[changed.length - 1] = 8;
      blobs.put("tail", changed);
      const updateWrites = m.writes() - first;
      expect(updateWrites).toBe(1);
      expect(blobs.get("tail")).toEqual(changed);
      blobs.put("tail", new Uint8Array([1, 2]));
      expect(blobs.get("tail")).toEqual(new Uint8Array([1, 2]));
      expect([...sql.exec("SELECT idx FROM blobs WHERE name='tail'")]).toHaveLength(1);
      blobs.put("tail", new Uint8Array(0));
      expect(blobs.get("tail")).toEqual(new Uint8Array(0));
      const emptyWrites = m.writes();
      blobs.put("tail", new Uint8Array(0));
      expect(m.writes()).toBe(emptyWrites);
      blobs.put("tail", original);
      expect(blobs.get("tail")).toEqual(original);
      expect(blobs.get("other")).toEqual(new Uint8Array([9]));
    });
  });

  it("checkpoints success attribution in bounded batches and immediately persists failures", async () => {
    await inSql((sql) => {
      ensureChatLog(sql);
      setMeta(sql, "pushOutcomes", JSON.stringify({ old: { ok: 5, rejected: 2, lastOkAt: 100 } }));
      const m = meter(sql);
      const clock = vi.spyOn(Date, "now").mockReturnValue(1000);
      try {
        const diagnostics = new PushDiagnostics(m.sql);
        diagnostics.record("old", true);
        const first = m.writes();
        for (let i = 0; i < 63; i++) diagnostics.record("new", true);
        expect(m.writes()).toBe(first);
        expect(diagnostics.snapshot()).toEqual({
          old: { ok: 6, rejected: 2, lastOkAt: 1000 },
          new: { ok: 63, rejected: 0, lastOkAt: 1000 }
        });
        // Simulated eviction retains the last checkpoint, without claiming
        // unflushed diagnostic counts were durable.
        expect(new PushDiagnostics(sql).snapshot().new).toBeUndefined();
        diagnostics.record("new", true);
        expect(m.writes()).toBeGreaterThan(first);
        expect(new PushDiagnostics(sql).snapshot().new.ok).toBe(64);
        diagnostics.record("new", true);
        diagnostics.record("new", false);
        expect(new PushDiagnostics(sql).snapshot().new).toEqual({ ok: 65, rejected: 1, lastOkAt: 1000 });
        diagnostics.record("new", true);
        const beforeTime = m.writes();
        clock.mockReturnValue(61_000);
        diagnostics.record("new", true);
        expect(m.writes()).toBeGreaterThan(beforeTime);
        expect(new PushDiagnostics(sql).snapshot().new.ok).toBe(67);
        diagnostics.record("new", true);
        sql.exec("DELETE FROM meta");
        diagnostics.reset();
        expect(diagnostics.snapshot()).toEqual({});
        diagnostics.record("", false);
        expect(new PushDiagnostics(sql).snapshot()["(unknown)"].rejected).toBe(1);
      } finally {
        clock.mockRestore();
      }
    });
  });

});
