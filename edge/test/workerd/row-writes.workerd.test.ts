import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { appendRow, ensureChatLog, headSeq, setMeta } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";
import type { Op } from "../../src/registry-core";

/** Rows written are what a DO bills once the month's free allowance is spent
 * (2026-10 bill: ~9M/day, 72% ChatRoom pushes). A steady-state push must pay
 * for its data only — the row and its unique index — not for bookkeeping.
 * Counts come from the runtime's own cursors, not from a model of SQLite. */

/** Wrap `sql.exec` to sum what every statement in `fn` wrote. */
const rowsWrittenBy = async (sql: SqlStorage, fn: () => Promise<unknown>): Promise<number> => {
  const exec = sql.exec.bind(sql);
  const cursors: Array<{ rowsWritten: number }> = [];
  sql.exec = ((query: string, ...bindings: unknown[]) => {
    const cursor = exec(query, ...bindings);
    cursors.push(cursor);
    return cursor;
  }) as typeof sql.exec;
  try {
    await fn();
  } finally {
    sql.exec = exec;
  }
  return cursors.reduce((sum, cursor) => sum + cursor.rowsWritten, 0);
};

const authed = (url: string, init: RequestInit = {}): Request =>
  new Request(url, { ...init, headers: { [AUTH_USER_HEADER]: "user", ...init.headers } });

describe("chat log", () => {
  it("an append writes only the row and its batch_id index entry", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName("append-writes"));
    await runInDurableObject(stub, async (_instance, state) => {
      const sql = state.storage.sql;
      ensureChatLog(sql);
      appendRow(sql, "dev", "warm", new Uint8Array([1]), 0);
      const written = await rowsWrittenBy(sql, async () => {
        appendRow(sql, "dev", "b2", new Uint8Array([2]), 1);
      });
      expect(written).toBe(2);
      expect(headSeq(sql)).toBe(2);
    });
  });

  it("a room's stored legacy headSeq stays a lower bound, so no seq is reissued", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName("legacy-head"));
    await runInDurableObject(stub, (_instance, state) => {
      const sql = state.storage.sql;
      ensureChatLog(sql);
      setMeta(sql, "headSeq", "7");
      expect(appendRow(sql, "dev", "b1", new Uint8Array([1]), 0)).toEqual({ ok: true, seq: 8, dup: false });
      expect(headSeq(sql)).toBe(8);
    });
  });
});

describe("ChatRoom push", () => {
  const push = (batchId: string) =>
    authed(`https://chat/rows?device=dev&batchId=${batchId}`, { method: "POST", body: new Uint8Array([1, 2, 3]) });

  it("a steady-state HTTP push writes only its row", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("push-writes"));
    await runInDurableObject(stub, async (instance, state) => {
      expect((await instance.fetch(push("warm"))).status).toBe(200);
      const written = await rowsWrittenBy(state.storage.sql, async () => {
        const response = await instance.fetch(push("b2"));
        expect(await response.json()).toEqual({ batchId: "b2", seq: 2, dup: false });
      });
      expect(written).toBe(2);
    });
  });

  it("/stats counts successes that have not been flushed yet", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("push-stats"));
    for (const batch of ["a", "b", "c"]) expect((await stub.fetch(push(batch))).status).toBe(200);
    const stats = await (await stub.fetch(authed("https://chat/stats"))).json<{
      pushOutcomes: Record<string, { ok: number; rejected: number }>;
    }>();
    expect(stats.pushOutcomes.dev).toMatchObject({ ok: 3, rejected: 0 });
  });

  it("the nightly alarm backs up once per new head, then lets the chain stop", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("push-backup"));
    expect((await stub.fetch(push("a"))).status).toBe(200);
    await runInDurableObject(stub, async (instance, state) => {
      const key = `backup/chat2/${state.id.toString()}/latest.json`;
      await instance.alarm();
      expect(await env.BLOBS.head(key)).not.toBeNull();
      await env.BLOBS.delete(key);
      // Nothing new since the last backup: no rewrite.
      await instance.alarm();
      expect(await env.BLOBS.head(key)).toBeNull();
    });
  });
});

describe("RegistryRoom push", () => {
  const op = (tick: number): Op => ({
    kind: "chats",
    id: "chat-a",
    op: "upsert",
    set: { title: `t${tick}` },
    hlc: `${String(tick).padStart(13, "0")}-000000-test`
  });
  const push = (tick: number) =>
    authed("https://registry/push?device=dev", {
      method: "POST",
      body: JSON.stringify({ batch: crypto.randomUUID(), ops: [op(tick)] })
    });

  it("a steady-state one-row push writes only that row", async () => {
    const stub = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName("push-writes"));
    await runInDurableObject(stub, async (instance, state) => {
      expect((await instance.fetch(push(1))).status).toBe(200);
      const written = await rowsWrittenBy(state.storage.sql, async () => {
        const response = await instance.fetch(push(2));
        expect(await response.json()).toMatchObject({ seq: 2, applied: 1 });
      });
      expect(written).toBe(2);
    });
  });

  it("a room's stored legacy seq stays a lower bound, so no seq is reissued", async () => {
    const stub = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName("legacy-seq"));
    await runInDurableObject(stub, async (instance, state) => {
      state.storage.sql.exec("INSERT INTO meta (key, value) VALUES ('seq', '41')");
      const response = await instance.fetch(push(1));
      expect(await response.json()).toMatchObject({ seq: 42, applied: 1 });
    });
  });
});
