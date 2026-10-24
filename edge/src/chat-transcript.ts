/**
 * chat3 transcript plane (docs/chat3-sync.md) — host-published transcript
 * rows and tool outputs, pure over a DO's SqlStorage like chat-log.ts.
 *
 * An item is one row keyed by `(ord, idx)`: `idx = -1` is an entry header,
 * `idx >= 0` is part `idx` of that entry. `ord` is the entry's position,
 * assigned once by the host. Rows are opaque JSON bytes to the DO — it orders
 * and serves them, never parses them. Every write takes the next `rev`, the
 * change feed a reader resumes from.
 *
 * Both tables are `WITHOUT ROWID`, so the primary key IS the table: one
 * b-tree per row write instead of a rowid table plus a key index. `rev` is
 * the only secondary index (the change feed needs it).
 */

/** One item's byte cap — a part's doc JSON. Well under the ~2MB SQL value
 * cap; a part bigger than this is the host's bug to split, never ours to chunk. */
export const MAX_ITEM_BYTES = 1024 * 1024;
/** Full tool output cap (the harness keeps head + tail under 256 KiB). */
export const MAX_OUTPUT_BYTES = 1024 * 1024;
/** Output chunk size, under the SQL value cap like blobs.ts. */
const OUTPUT_CHUNK_BYTES = 1_000_000;
/** Page bounds: enough to paint a screen, small enough for a 1.2 Mbps link. */
export const DEFAULT_PAGE_LIMIT = 200;
export const MAX_PAGE_LIMIT = 1000;
/** Response cap for a buffered page/feed body (mirrors `/rows`). */
export const ITEMS_BODY_CAP = 4 * 1024 * 1024;

const ENTRY_RE = /^[A-Za-z0-9_.:#~-]{1,200}$/;
export const PART_RE = /^[A-Za-z0-9_.:#~-]{1,200}$/;
/** Part indexes are list positions; a turn can hold tens of thousands. */
const MAX_IDX = 10_000_000;

export interface ItemKey {
  ord: number;
  idx: number;
}

export interface TranscriptItem extends ItemKey {
  entry: string;
  rev: number;
  bytes: Uint8Array;
}

export const ensureTranscript = (sql: SqlStorage): void => {
  sql.exec(
    "CREATE TABLE IF NOT EXISTS items (ord REAL NOT NULL, idx INTEGER NOT NULL, entry TEXT NOT NULL, rev INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (ord, idx)) WITHOUT ROWID"
  );
  sql.exec("CREATE INDEX IF NOT EXISTS items_rev ON items(rev)");
  sql.exec(
    "CREATE TABLE IF NOT EXISTS outputs (part TEXT NOT NULL, chunk INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (part, chunk)) WITHOUT ROWID"
  );
};

/** Header validation for a pushed item. Returns an error code or null. */
export const validateItemKey = (header: Record<string, unknown>): string | null => {
  const { ord, idx, entry } = header;
  if (typeof ord !== "number" || !Number.isFinite(ord)) return "bad_ord";
  if (typeof idx !== "number" || !Number.isInteger(idx) || idx < -1 || idx > MAX_IDX) return "bad_idx";
  if (typeof entry !== "string" || !ENTRY_RE.test(entry)) return "bad_entry";
  return null;
};

/** Highest rev written so far (0 = empty plane). One index probe. */
export const headRev = (sql: SqlStorage): number =>
  Number([...sql.exec("SELECT MAX(rev) AS r FROM items")][0]?.r ?? 0);

export type PutItemOutcome = { ok: true; rev: number; changed: boolean } | { ok: false; error: string };

/** Upsert one item. An identical rewrite (a host outbox replay) is answered
 * with the stored rev and writes nothing — reads are a thousandth of the
 * price of a write, so the comparison always pays for itself. */
export const putItem = (
  sql: SqlStorage,
  key: ItemKey & { entry: string },
  bytes: Uint8Array
): PutItemOutcome => {
  if (bytes.byteLength === 0) return { ok: false, error: "empty" };
  if (bytes.byteLength > MAX_ITEM_BYTES) return { ok: false, error: "too_large" };
  const existing = [
    ...sql.exec("SELECT rev, entry, bytes FROM items WHERE ord = ? AND idx = ?", key.ord, key.idx)
  ][0];
  if (existing && existing.entry === key.entry && sameBytes(new Uint8Array(existing.bytes as ArrayBuffer), bytes)) {
    return { ok: true, rev: existing.rev as number, changed: false };
  }
  const rev = headRev(sql) + 1;
  sql.exec(
    "INSERT INTO items (ord, idx, entry, rev, bytes) VALUES (?, ?, ?, ?, ?) ON CONFLICT(ord, idx) DO UPDATE SET entry = excluded.entry, rev = excluded.rev, bytes = excluded.bytes",
    key.ord,
    key.idx,
    key.entry,
    rev,
    toBuffer(bytes)
  );
  return { ok: true, rev, changed: true };
};

export interface ItemsPage {
  items: TranscriptItem[];
  /** Rows exist before the first returned row. */
  more: boolean;
}

/** The newest `limit` rows strictly before `before` (or the end), returned
 * ascending. When the window starts inside an entry, that entry's header is
 * prepended so a reader can render the parts it got. */
export const itemsPage = (sql: SqlStorage, before: ItemKey | undefined, limit: number): ItemsPage => {
  const n = Math.max(1, Math.min(MAX_PAGE_LIMIT, Math.floor(limit)));
  const raw = before
    ? [
        ...sql.exec(
          "SELECT ord, idx, entry, rev, bytes FROM items WHERE ord < ? OR (ord = ? AND idx < ?) ORDER BY ord DESC, idx DESC LIMIT ?",
          before.ord,
          before.ord,
          before.idx,
          n + 1
        )
      ]
    : [...sql.exec("SELECT ord, idx, entry, rev, bytes FROM items ORDER BY ord DESC, idx DESC LIMIT ?", n + 1)];
  const more = raw.length > n;
  const items = raw.slice(0, n).map(toItem).reverse();
  const first = items[0];
  if (first && first.idx > -1) {
    const header = [
      ...sql.exec("SELECT ord, idx, entry, rev, bytes FROM items WHERE ord = ? AND idx = -1", first.ord)
    ][0];
    if (header) items.unshift(toItem(header));
  }
  return { items, more };
};

/** Rows written after `rev`, in rev order (the reconnect change feed). */
export function* itemsSince(sql: SqlStorage, rev: number): Generator<TranscriptItem> {
  for (const row of sql.exec("SELECT ord, idx, entry, rev, bytes FROM items WHERE rev > ? ORDER BY rev", rev)) {
    yield toItem(row);
  }
}

export interface TranscriptStats {
  headRev: number;
  items: number;
  itemBytes: number;
  outputs: number;
  outputBytes: number;
}

export const transcriptStats = (sql: SqlStorage): TranscriptStats => {
  const items = [...sql.exec("SELECT COUNT(*) AS n, COALESCE(SUM(LENGTH(bytes)), 0) AS b FROM items")][0]!;
  const outputs = [
    ...sql.exec("SELECT COUNT(DISTINCT part) AS n, COALESCE(SUM(LENGTH(bytes)), 0) AS b FROM outputs")
  ][0]!;
  return {
    headRev: headRev(sql),
    items: items.n as number,
    itemBytes: items.b as number,
    outputs: outputs.n as number,
    outputBytes: outputs.b as number
  };
};

/** Replace a part's full output. Chunked so the 1 MiB cap stays under the
 * SQL value cap with room to spare; the typical output is one row. */
export const putOutput = (sql: SqlStorage, part: string, bytes: Uint8Array): void => {
  sql.exec("DELETE FROM outputs WHERE part = ?", part);
  for (let i = 0, chunk = 0; i === 0 || i < bytes.length; i += OUTPUT_CHUNK_BYTES, chunk++) {
    sql.exec(
      "INSERT INTO outputs (part, chunk, bytes) VALUES (?, ?, ?)",
      part,
      chunk,
      toBuffer(bytes.subarray(i, Math.min(i + OUTPUT_CHUNK_BYTES, bytes.length)))
    );
  }
};

export const getOutput = (sql: SqlStorage, part: string): Uint8Array | undefined => {
  const rows = [...sql.exec("SELECT bytes FROM outputs WHERE part = ? ORDER BY chunk", part)];
  if (rows.length === 0) return undefined;
  const chunks = rows.map((r) => new Uint8Array(r.bytes as ArrayBuffer));
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
};

export const resetTranscript = (sql: SqlStorage): void => {
  sql.exec("DELETE FROM items");
  sql.exec("DELETE FROM outputs");
};

/** `ord:idx` as sent in `?before=` (the key of a reader's oldest row). */
export const parseItemKey = (text: string | null): ItemKey | undefined => {
  const match = text?.match(/^(-?\d+(?:\.\d+)?(?:e-?\d+)?):(-1|\d+)$/i);
  if (!match) return undefined;
  const ord = Number(match[1]);
  const idx = Number(match[2]);
  return Number.isFinite(ord) && Number.isInteger(idx) ? { ord, idx } : undefined;
};

const toItem = (row: Record<string, SqlStorageValue>): TranscriptItem => ({
  ord: row.ord as number,
  idx: row.idx as number,
  entry: row.entry as string,
  rev: row.rev as number,
  bytes: new Uint8Array(row.bytes as ArrayBuffer)
});

const toBuffer = (bytes: Uint8Array): ArrayBuffer =>
  bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;

const sameBytes = (a: Uint8Array, b: Uint8Array): boolean => {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
};
