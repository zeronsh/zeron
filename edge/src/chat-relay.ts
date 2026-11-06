/**
 * The relay layer's offline queue (docs/design/relay-layer.md): updates from a
 * device to a chat's host, held until the host has saved them, then deleted.
 *
 * A separate table from `rows` on purpose. Deleting from the middle of `rows`
 * would leave holes in the seq space, and older clients redial forever on a
 * hole. This table can't affect them, and its deletes can't touch stored rows.
 *
 * Created on first use, so a room with the layer off never has one.
 */

import { appendRow, MAX_ROW_BYTES } from "./chat-log";

export interface QueueRow {
  device: string;
  batchId: string;
  bytes: Uint8Array;
}

export type EnqueueOutcome =
  | { ok: true; dup: boolean }
  | { ok: false; error: "too_large" | "empty" };

export const ensureRelayQueue = (sql: SqlStorage): void => {
  sql.exec(
    "CREATE TABLE IF NOT EXISTS relay_queue (id INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id TEXT NOT NULL, bytes BLOB NOT NULL, received_at INTEGER NOT NULL)"
  );
};

/** Hold one update. A replay of a batch still queued writes nothing. A replay
 * of one already delivered is stored again, and importing it is a no-op. */
export const enqueueRow = (
  sql: SqlStorage,
  device: string,
  batchId: string,
  bytes: Uint8Array,
  receivedAt: number
): EnqueueOutcome => {
  if (bytes.byteLength === 0) return { ok: false, error: "empty" };
  if (bytes.byteLength > MAX_ROW_BYTES) return { ok: false, error: "too_large" };
  if ([...sql.exec("SELECT 1 FROM relay_queue WHERE batch_id = ?", batchId)].length > 0) {
    return { ok: true, dup: true };
  }
  sql.exec(
    "INSERT INTO relay_queue (device, batch_id, bytes, received_at) VALUES (?, ?, ?, ?)",
    device,
    batchId,
    bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    receivedAt
  );
  return { ok: true, dup: false };
};

export function* queueRows(sql: SqlStorage): Generator<QueueRow> {
  for (const raw of sql.exec("SELECT device, batch_id, bytes FROM relay_queue ORDER BY id")) {
    yield {
      device: raw.device as string,
      batchId: raw.batch_id as string,
      bytes: new Uint8Array(raw.bytes as ArrayBuffer)
    };
  }
}

/** Delete rows the host has saved. Named by batch id, never by position, so a
 * late or repeated `delivered` can't delete a newer row. A device can't confirm
 * its own message: that would drop it with the host never having seen it. */
export const markDelivered = (sql: SqlStorage, device: string, batchIds: string[]): void => {
  for (const batchId of batchIds) {
    sql.exec("DELETE FROM relay_queue WHERE batch_id = ? AND device != ?", batchId, device);
  }
};

/** Whether this room ever held a queue. */
export const hasRelayQueue = (sql: SqlStorage): boolean =>
  [...sql.exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'relay_queue'")].length > 0;

/** Move every queued row into the stored log: a reader that can't use the queue
 * is here, or the layer has been turned off. Rows and deletes commit together,
 * so an acked message is always in one or the other. Returns what was stored. */
export const drainQueueIntoRows = (
  sql: SqlStorage,
  receivedAt: number
): Array<QueueRow & { seq: number }> => {
  const moved: Array<QueueRow & { seq: number }> = [];
  for (const row of [...queueRows(sql)]) {
    const outcome = appendRow(sql, row.device, row.batchId, row.bytes, receivedAt);
    if (outcome.ok && !outcome.dup) moved.push({ ...row, seq: outcome.seq });
  }
  sql.exec("DELETE FROM relay_queue");
  return moved;
};

export const queueLength = (sql: SqlStorage): number =>
  [...sql.exec("SELECT COUNT(*) AS n FROM relay_queue")][0]?.n as number;
