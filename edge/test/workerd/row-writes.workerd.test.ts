import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { appendRow, ensureChatLog, getMeta, headSeq, setMeta } from "../../src/chat-log";
import { AUTH_USER_HEADER } from "../../src/env";
import type { Op } from "../../src/registry-core";

/** Rows written are what a DO bills once the month's free allowance is spent
 * (2026-10 bill: ~10M/day, ~94% ChatRoom + RegistryRoom pushes). Counts come
 * from the runtime's own cursors, not from a model of SQLite.
 *
 * Rollout is two-stage so every deploy keeps a safe rollback target:
 * - this release derives the head from the log and gates backups on it, but
 *   still STORES `headSeq`/`seq` and the `backupDirty` flag that earlier
 *   releases read — a push costs 3 rows (data 2 + stored head 1);
 * - the next release stops storing them (2 rows) with this one as its
 *   rollback target. The "rollback" cases below pin both directions. */

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

/** The pre-derivation release's append, verbatim in effect: it trusts ONLY
 * the stored `headSeq`. */
const legacyAppend = (sql: SqlStorage, batchId: string): number => {
  const seq = Number(getMeta(sql, "headSeq") ?? "0") + 1;
  sql.exec(
    "INSERT INTO rows (seq, device, batch_id, bytes, received_at) VALUES (?, ?, ?, ?, ?)",
    seq, "legacy", batchId, new Uint8Array([9]).buffer, 0
  );
  setMeta(sql, "headSeq", String(seq));
  return seq;
};

/** A release that no longer stores the head: appends rows, leaves meta be. */
const unstoredAppend = (sql: SqlStorage, batchId: string): number => {
  const seq = headSeq(sql) + 1;
  sql.exec(
    "INSERT INTO rows (seq, device, batch_id, bytes, received_at) VALUES (?, ?, ?, ?, ?)",
    seq, "next", batchId, new Uint8Array([9]).buffer, 0
  );
  return seq;
};

describe("chat log", () => {
  it("an append writes the row, its batch_id index entry and the stored head", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName("append-writes"));
    await runInDurableObject(stub, async (_instance, state) => {
      const sql = state.storage.sql;
      ensureChatLog(sql);
      appendRow(sql, "dev", "warm", new Uint8Array([1]), 0);
      const written = await rowsWrittenBy(sql, async () => {
        appendRow(sql, "dev", "b2", new Uint8Array([2]), 1);
      });
      expect(written).toBe(3);
    });
  });

  it("rollback: old → this → old keeps seqs dense and unique", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName("rollback-old"));
    await runInDurableObject(stub, (_instance, state) => {
      const sql = state.storage.sql;
      ensureChatLog(sql);
      expect(legacyAppend(sql, "o1")).toBe(1);
      expect(appendRow(sql, "dev", "n1", new Uint8Array([1]), 0)).toMatchObject({ seq: 2 });
      expect(appendRow(sql, "dev", "n2", new Uint8Array([1]), 0)).toMatchObject({ seq: 3 });
      // Had this release stopped storing `headSeq`, it would still read 1
      // and the old release would reissue seq 2 (UNIQUE constraint failed).
      expect(legacyAppend(sql, "o2")).toBe(4);
    });
  });

  it("rollback: next → this reads a head the stored copy never saw", async () => {
    const stub = env.TEST_LOG.get(env.TEST_LOG.idFromName("rollback-next"));
    await runInDurableObject(stub, (_instance, state) => {
      const sql = state.storage.sql;
      ensureChatLog(sql);
      appendRow(sql, "dev", "a", new Uint8Array([1]), 0);
      expect(unstoredAppend(sql, "b")).toBe(2);
      expect(unstoredAppend(sql, "c")).toBe(3);
      expect(getMeta(sql, "headSeq")).toBe("1");
      expect(appendRow(sql, "dev", "d", new Uint8Array([1]), 0)).toMatchObject({ seq: 4 });
    });
  });
});

describe("ChatRoom push", () => {
  const push = (batchId: string, device = "dev") =>
    authed(`https://chat/rows?device=${device}&batchId=${batchId}`, {
      method: "POST",
      body: new Uint8Array([1, 2, 3])
    });

  it("a steady-state HTTP push writes its row and the stored head — no bookkeeping", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("push-writes"));
    await runInDurableObject(stub, async (instance, state) => {
      expect((await instance.fetch(push("warm"))).status).toBe(200);
      const written = await rowsWrittenBy(state.storage.sql, async () => {
        const response = await instance.fetch(push("b2"));
        expect(await response.json()).toEqual({ batchId: "b2", seq: 2, dup: false });
      });
      expect(written).toBe(3);
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

  it("rollback: the stored flag still marks unbacked rows for an old alarm", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("backup-flag"));
    await runInDurableObject(stub, async (instance, state) => {
      const sql = state.storage.sql;
      await instance.fetch(push("a"));
      expect(getMeta(sql, "backupDirty")).toBe("1");
      await instance.alarm();
      expect(getMeta(sql, "backupDirty")).toBe("0");
      await instance.fetch(push("b"));
      // An old release's alarm gates on exactly this.
      expect(getMeta(sql, "backupDirty")).toBe("1");
    });
  });

  it("rollback: next → this backs up rows the flag never marked", async () => {
    const stub = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName("backup-next"));
    await runInDurableObject(stub, async (instance, state) => {
      const sql = state.storage.sql;
      const key = `backup/chat2/${state.id.toString()}/latest.json`;
      await instance.fetch(push("a"));
      await instance.alarm();
      expect(await env.BLOBS.head(key)).not.toBeNull();
      await env.BLOBS.delete(key);
      // Nothing new since the last backup: no rewrite.
      await instance.alarm();
      expect(await env.BLOBS.head(key)).toBeNull();
      // The next release appends without touching backupDirty.
      unstoredAppend(sql, "b");
      expect(getMeta(sql, "backupDirty")).toBe("0");
      await instance.alarm();
      expect(await env.BLOBS.head(key)).not.toBeNull();
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
  const meta = (sql: SqlStorage, key: string) =>
    [...sql.exec("SELECT value FROM meta WHERE key = ?", key)][0]?.value as string | undefined;

  it("a steady-state one-row push writes that row and the stored seq — no bookkeeping", async () => {
    const stub = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName("push-writes"));
    await runInDurableObject(stub, async (instance, state) => {
      expect((await instance.fetch(push(1))).status).toBe(200);
      const written = await rowsWrittenBy(state.storage.sql, async () => {
        const response = await instance.fetch(push(2));
        expect(await response.json()).toMatchObject({ seq: 2, applied: 1 });
      });
      expect(written).toBe(3);
    });
  });

  it("rollback: the stored seq and flag keep tracking for an old release", async () => {
    const stub = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName("rollback-old"));
    await runInDurableObject(stub, async (instance, state) => {
      const sql = state.storage.sql;
      await instance.fetch(push(1));
      await instance.fetch(push(2));
      // An old release reads ONLY these: its next push must be seq 3, and its
      // alarm must see the unbacked change.
      expect(meta(sql, "seq")).toBe("2");
      expect(meta(sql, "backupDirty")).toBe("1");
    });
  });

  it("rollback: next → this continues past a seq the stored copy never saw", async () => {
    const stub = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName("rollback-next"));
    await runInDurableObject(stub, async (instance, state) => {
      const sql = state.storage.sql;
      await instance.fetch(push(1));
      // The next release applied a batch at seq 2 without storing it.
      sql.exec("UPDATE rows SET seq = 2");
      expect(meta(sql, "seq")).toBe("1");
      // Trusting the stored seq alone would reissue seq 2, and a client
      // already at cursor 2 would get an empty incremental response.
      const response = await instance.fetch(push(3));
      expect(await response.json()).toMatchObject({ seq: 3, applied: 1 });
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
