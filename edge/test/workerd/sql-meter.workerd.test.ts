import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { meterSql, newSqlMeter } from "../../src/sql-meter";

/** The meter must agree with the runtime's own billing counters, so these
 * pin the platform behaviour it relies on: writes are counted inside `exec`,
 * an index costs a row per write, and a delete is a billed write. */

const inRoom = <T>(name: string, fn: (sql: SqlStorage) => T): Promise<T> => {
  const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName(`meter-${name}`));
  return runInDurableObject(stub, (_instance, state) => fn(state.storage.sql));
};

describe("sql meter on real DO SQLite", () => {
  it("counts a plain insert as one row and an indexed insert as two", async () => {
    await inRoom("insert", (raw) => {
      const meter = newSqlMeter();
      const sql = meterSql(raw, meter);
      sql.exec("CREATE TABLE plain (k INTEGER PRIMARY KEY, v TEXT)");
      sql.exec("CREATE TABLE indexed (k INTEGER PRIMARY KEY, v TEXT UNIQUE)");
      const before = meter.rowsWritten;
      sql.exec("INSERT INTO plain (k, v) VALUES (1, 'a')");
      expect(meter.rowsWritten - before).toBe(1);
      const mid = meter.rowsWritten;
      sql.exec("INSERT INTO indexed (k, v) VALUES (1, 'a')");
      expect(meter.rowsWritten - mid).toBe(2);
    });
  });

  it("counts deletes as written rows and reads as they are consumed", async () => {
    await inRoom("delete", (raw) => {
      const meter = newSqlMeter();
      const sql = meterSql(raw, meter);
      sql.exec("CREATE TABLE t (k INTEGER PRIMARY KEY, v TEXT)");
      for (let i = 0; i < 10; i++) sql.exec("INSERT INTO t (k, v) VALUES (?, 'x')", i);
      const written = meter.rowsWritten;
      sql.exec("DELETE FROM t WHERE k < 4");
      expect(meter.rowsWritten - written).toBe(4);
      const read = meter.rowsRead;
      const rows = [...sql.exec("SELECT k FROM t ORDER BY k")];
      expect(rows.length).toBe(6);
      expect(meter.rowsRead - read).toBe(6);
    });
  });

  it("an upsert that changes nothing still bills its row", async () => {
    await inRoom("upsert", (raw) => {
      const meter = newSqlMeter();
      const sql = meterSql(raw, meter);
      sql.exec("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
      const upsert = "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value";
      sql.exec(upsert, "backupDirty", "1");
      const before = meter.rowsWritten;
      sql.exec(upsert, "backupDirty", "1");
      expect(meter.rowsWritten - before).toBeGreaterThanOrEqual(1);
    });
  });
});
