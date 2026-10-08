/**
 * chat2 log storage (docs/chat2-sync.md workstream B) — the dumb-relay data
 * model, pure over a DO's SqlStorage + BlobStore so the workerd test tier
 * exercises it against real SQLite (the ~2MB value-cap runtime).
 *
 * The room is an append-only log of opaque Loro update blobs plus one
 * client-written checkpoint blob. No wasm, no replay, no materialization:
 * cold start is a table read, so the s2 wedge class (CPU-limited history
 * replay in the DO) cannot exist here by construction.
 */

import type { BlobStore } from "./blobs";

/** Per-row byte cap, rejected at the frame header. Post-strip updates are
 * KB-scale; a full checkpoint travels over HTTP, never as a row. Well under
 * the ~2MB SQL value cap, so rows are never chunked (unlike s2's update-log,
 * whose silent SQLITE_TOOBIG overflow was the 2026-08-05 whale freeze). */
export const MAX_ROW_BYTES = 1024 * 1024;

/** Blob-store names for the checkpoint payload and its frontier. */
export const CHECKPOINT_BLOB = "checkpoint";
export const FRONTIER_BLOB = "checkpoint-frontier";

export interface LogRow {
  seq: number;
  device: string;
  batchId: string;
  bytes: Uint8Array;
}

export const ensureChatLog = (sql: SqlStorage): void => {
  sql.exec(
    "CREATE TABLE IF NOT EXISTS rows (seq INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL UNIQUE, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)"
  );
  sql.exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
};

export const getMeta = (sql: SqlStorage, key: string): string | undefined => {
  const rows = [...sql.exec("SELECT value FROM meta WHERE key = ?", key)];
  return rows[0]?.value as string | undefined;
};

export const setMeta = (sql: SqlStorage, key: string, value: string): void => {
  sql.exec(
    "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    key,
    value
  );
};

/** The newest seq ever issued, derived from the log itself. `seq` is the
 * rowid, so `MAX` is one b-tree probe; rows only ever leave through a
 * checkpoint, which records the floor, so an empty log's head is `seqFloor`.
 * The stored `headSeq` is a lower bound on top: reissuing a seq a client
 * already holds would make it skip the new row.
 *
 * Reading this way is what makes it safe to STOP storing `headSeq` (a row
 * write per append) in a later release: this release is that one's rollback
 * target, and it reads correctly whether or not the stored copy kept up. */
export const headSeq = (sql: SqlStorage): number => {
  const max = [...sql.exec("SELECT MAX(seq) AS m FROM rows")][0]?.m as number | null;
  return Math.max(max ?? 0, seqFloor(sql), Number(getMeta(sql, "headSeq") ?? "0"));
};

/** Rows with `seq <= seqFloor` are gone — covered by the checkpoint. A cursor
 * below the floor must load the checkpoint before requesting rows. */
export const seqFloor = (sql: SqlStorage): number => Number(getMeta(sql, "seqFloor") ?? "0");

export type AppendOutcome =
  | { ok: true; seq: number; dup: boolean }
  | { ok: false; error: "too_large" | "empty" };

/** Append one update row. `batch_id` UNIQUE dedupes reconnect re-pushes
 * server-side: a replay acks the ORIGINAL seq and appends nothing (Loro
 * re-import is a no-op client-side, so duplicates are safe end-to-end — this
 * just keeps the log tight). */
export const appendRow = (
  sql: SqlStorage,
  device: string,
  batchId: string,
  bytes: Uint8Array,
  receivedAt: number
): AppendOutcome => {
  if (bytes.byteLength === 0) return { ok: false, error: "empty" };
  if (bytes.byteLength > MAX_ROW_BYTES) return { ok: false, error: "too_large" };
  const existing = [...sql.exec("SELECT seq FROM rows WHERE batch_id = ?", batchId)];
  if (existing.length > 0) {
    return { ok: true, seq: existing[0]!.seq as number, dup: true };
  }
  const seq = headSeq(sql) + 1;
  sql.exec(
    "INSERT INTO rows (seq, device, batch_id, bytes, received_at) VALUES (?, ?, ?, ?, ?)",
    seq,
    device,
    batchId,
    bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    receivedAt
  );
  // Still stored for releases that read ONLY this key (everything before
  // the derived `headSeq`): rolling back to one must not reissue a seq.
  setMeta(sql, "headSeq", String(seq));
  return { ok: true, seq, dup: false };
};

/** Rows `seq > after`, in order, optionally excluding one device's own writes
 * (the reconnect-after-offline-work path never re-downloads itself). */
export function* rowsAfter(
  sql: SqlStorage,
  after: number,
  excludeDevice?: string
): Generator<LogRow> {
  const cursor = excludeDevice
    ? sql.exec(
        "SELECT seq, device, batch_id, bytes FROM rows WHERE seq > ? AND device != ? ORDER BY seq",
        after,
        excludeDevice
      )
    : sql.exec("SELECT seq, device, batch_id, bytes FROM rows WHERE seq > ? ORDER BY seq", after);
  for (const raw of cursor) {
    yield {
      seq: raw.seq as number,
      device: raw.device as string,
      batchId: raw.batch_id as string,
      bytes: new Uint8Array(raw.bytes as ArrayBuffer)
    };
  }
}

export type CheckpointOutcome =
  | { ok: true; seqFloor: number; pruned: number }
  | { ok: false; error: "floor_regression" | "ahead_of_head" | "empty" };

/** Commit a client-built checkpoint covering rows `seq <= seqCovered`.
 *
 * The guard is floor-monotonic — the dumb replacement for s2's VV-monotonic
 * R2 guard: a checkpoint may only move the floor forward, and may not claim
 * to cover rows that don't exist yet. Within those bounds the SERVER trusts
 * the payload blindly (owner-only rooms contain semantic garbage per-user;
 * the next checkpoint erases it). Rows covered by the checkpoint are deleted
 * in the same event — DO events are single-threaded and commit atomically,
 * so a crash never leaves the floor and the rows disagreeing. */
export const commitCheckpoint = (
  sql: SqlStorage,
  blobs: BlobStore,
  seqCovered: number,
  frontier: Uint8Array,
  bytes: Uint8Array,
  committedAt: number
): CheckpointOutcome => {
  if (bytes.byteLength === 0) return { ok: false, error: "empty" };
  if (seqCovered < seqFloor(sql)) return { ok: false, error: "floor_regression" };
  if (seqCovered > headSeq(sql)) return { ok: false, error: "ahead_of_head" };
  blobs.put(CHECKPOINT_BLOB, bytes);
  blobs.put(FRONTIER_BLOB, frontier);
  const before = rowCount(sql);
  sql.exec("DELETE FROM rows WHERE seq <= ?", seqCovered);
  const pruned = before - rowCount(sql);
  setMeta(sql, "seqFloor", String(seqCovered));
  setMeta(sql, "checkpointSeq", String(seqCovered));
  setMeta(sql, "checkpointSize", String(bytes.byteLength));
  setMeta(sql, "checkpointAt", String(committedAt));
  return { ok: true, seqFloor: seqCovered, pruned };
};

const rowCount = (sql: SqlStorage): number =>
  [...sql.exec("SELECT COUNT(*) AS n FROM rows")][0]?.n as number;

export interface LogStats {
  headSeq: number;
  seqFloor: number;
  rowCount: number;
  rowBytes: number;
  checkpointSeq: number;
  checkpointSize: number;
  checkpointAt: number;
}

/** The hello/stats surface — also what the host's checkpoint policy reads
 * (`rowBytes > 512KB || rowCount > 200` → post a checkpoint) and what the
 * fleet alert watches (unbounded growth is this design's failure mode). */
export const logStats = (sql: SqlStorage): LogStats => {
  const agg = [
    ...sql.exec("SELECT COUNT(*) AS n, COALESCE(SUM(LENGTH(bytes)), 0) AS b FROM rows")
  ][0]!;
  return {
    headSeq: headSeq(sql),
    seqFloor: seqFloor(sql),
    rowCount: agg.n as number,
    rowBytes: agg.b as number,
    checkpointSeq: Number(getMeta(sql, "checkpointSeq") ?? "0"),
    checkpointSize: Number(getMeta(sql, "checkpointSize") ?? "0"),
    checkpointAt: Number(getMeta(sql, "checkpointAt") ?? "0")
  };
};
