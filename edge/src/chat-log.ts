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

/** How far back a push looks for its own batch id. Rooms created before this
 * bound keep their `batch_id` index and answer it from there; newer rooms have
 * no index (one billed row per push instead of two) and walk this many rows at
 * most. Rows are read at a thousandth of a write's price, and a replay older
 * than this is what a replay after a checkpoint prune already is: a new row
 * whose re-import is a Loro no-op. */
export const DEDUPE_WINDOW = 4096;

export const ensureChatLog = (sql: SqlStorage): void => {
  sql.exec(
    "CREATE TABLE IF NOT EXISTS rows (seq INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)"
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

/** The log's head comes from what the log holds, so it can never disagree with
 * the rows: the highest row, or the floor when a checkpoint pruned them all.
 * A checkpoint only deletes at or below its floor, so neither can regress.
 * `meta.headSeq` is a lagging copy (see `flushHeadSeq`). */
export const headSeq = (sql: SqlStorage): number => {
  const top = [...sql.exec("SELECT MAX(seq) AS top FROM rows")][0]?.top as number | null;
  return Math.max(top ?? 0, seqFloor(sql), Number(getMeta(sql, "headSeq") ?? "0"));
};

/** Bring the stored copy of the head up to date. Nothing here reads it for
 * correctness; releases before the head was derived do, and would collide on
 * `rows.seq` if they read a stale one (see `CHAT_ROOM_WRITE_THROUGH`). */
export const flushHeadSeq = (sql: SqlStorage): void => {
  const head = headSeq(sql);
  if (head !== Number(getMeta(sql, "headSeq") ?? "0")) setMeta(sql, "headSeq", String(head));
};

/** Rows with `seq <= seqFloor` are gone — covered by the checkpoint. A cursor
 * below the floor must load the checkpoint before requesting rows. */
export const seqFloor = (sql: SqlStorage): number => Number(getMeta(sql, "seqFloor") ?? "0");

export type AppendOutcome =
  | { ok: true; seq: number; dup: boolean }
  | { ok: false; error: "too_large" | "empty" };

/** Append one update row. The batch id dedupes reconnect re-pushes server-side:
 * a replay acks the ORIGINAL seq and appends nothing (Loro re-import is a no-op
 * client-side, so duplicates are safe end-to-end — this just keeps the log
 * tight). The row is the only thing written: its presence IS the new head, so
 * an ack can't outrun or disagree with it. */
export const appendRow = (
  sql: SqlStorage,
  device: string,
  batchId: string,
  bytes: Uint8Array,
  receivedAt: number
): AppendOutcome => {
  if (bytes.byteLength === 0) return { ok: false, error: "empty" };
  if (bytes.byteLength > MAX_ROW_BYTES) return { ok: false, error: "too_large" };
  const head = headSeq(sql);
  const existing = [
    ...sql.exec("SELECT seq FROM rows WHERE seq > ? AND batch_id = ?", head - DEDUPE_WINDOW, batchId)
  ];
  if (existing.length > 0) {
    return { ok: true, seq: existing[0]!.seq as number, dup: true };
  }
  const seq = head + 1;
  // OR IGNORE: a room from before the bound still has its UNIQUE index on
  // batch_id, and the window above doesn't reach a replay older than it. There
  // the index answers; a plain INSERT would throw and the batch would never ack.
  const insert = sql.exec(
    "INSERT OR IGNORE INTO rows (seq, device, batch_id, bytes, received_at) VALUES (?, ?, ?, ?, ?)",
    seq,
    device,
    batchId,
    bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    receivedAt
  );
  if (insert.rowsWritten === 0) {
    const original = [...sql.exec("SELECT seq FROM rows WHERE batch_id = ?", batchId)];
    return { ok: true, seq: original[0]!.seq as number, dup: true };
  }
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
  | { ok: false; error: "floor_regression" | "ahead_of_head" | "empty" | "incomplete" };

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
  return landCheckpoint(sql, blobs, seqCovered, frontier, bytes.byteLength, committedAt);
};

/** Where a checkpoint sent in parts is gathered (`/checkpoint/part`) until
 * its commit takes it whole: one at a time per room. */
export const STAGED_CHECKPOINT_BLOB = "checkpoint-upload";

/** `commitCheckpoint` for one sent in parts: the staged blob becomes the
 * checkpoint as it is, renamed (its bytes never held in memory at once),
 * once it is the `size` it was said to be. Same guards, same outcome. */
export const commitStagedCheckpoint = (
  sql: SqlStorage,
  blobs: BlobStore,
  seqCovered: number,
  frontier: Uint8Array,
  size: number,
  committedAt: number
): CheckpointOutcome => {
  const staged = blobs.size(STAGED_CHECKPOINT_BLOB) ?? 0;
  if (size <= 0 || staged === 0) return { ok: false, error: "empty" };
  if (staged !== size) return { ok: false, error: "incomplete" };
  if (seqCovered < seqFloor(sql)) return { ok: false, error: "floor_regression" };
  if (seqCovered > headSeq(sql)) return { ok: false, error: "ahead_of_head" };
  blobs.rename(STAGED_CHECKPOINT_BLOB, CHECKPOINT_BLOB);
  return landCheckpoint(sql, blobs, seqCovered, frontier, size, committedAt);
};

const landCheckpoint = (
  sql: SqlStorage,
  blobs: BlobStore,
  seqCovered: number,
  frontier: Uint8Array,
  size: number,
  committedAt: number
): CheckpointOutcome => {
  blobs.put(FRONTIER_BLOB, frontier);
  const before = rowCount(sql);
  sql.exec("DELETE FROM rows WHERE seq <= ?", seqCovered);
  const pruned = before - rowCount(sql);
  setMeta(sql, "seqFloor", String(seqCovered));
  setMeta(sql, "checkpointSeq", String(seqCovered));
  setMeta(sql, "checkpointSize", String(size));
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
