import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";

// What the store's SQL leans on, checked in the runtime's own D1.
describe("store D1", () => {
  it("has FTS5 and the schema", async () => {
    const tables = await env.STORE_DB.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").all();
    expect(tables.results.map((r) => r.name)).toContain("plugin_search");
  });
  it("changes() inside a batch sees the previous statement", async () => {
    const db = env.STORE_DB;
    await db.exec("CREATE TABLE IF NOT EXISTS t1 (k TEXT PRIMARY KEY, v INTEGER)");
    await db.exec("CREATE TABLE IF NOT EXISTS t2 (n INTEGER)");
    await db.batch([
      db.prepare("INSERT INTO t1 (k, v) VALUES ('a', 1) ON CONFLICT DO NOTHING"),
      db.prepare("INSERT INTO t2 (n) SELECT 1 WHERE changes() = 1"),
      db.prepare("INSERT INTO t1 (k, v) VALUES ('a', 2) ON CONFLICT DO NOTHING"),
      db.prepare("INSERT INTO t2 (n) SELECT 2 WHERE changes() = 1"),
      db.prepare("UPDATE t1 SET v = 5 WHERE k = 'a' AND v = 1"),
      db.prepare("INSERT INTO t2 (n) SELECT 3 WHERE changes() = 1")
    ]);
    const rows = await db.prepare("SELECT n FROM t2 ORDER BY n").all();
    expect(rows.results.map((r) => r.n)).toEqual([1, 3]);
  });
});
