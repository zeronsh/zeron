/**
 * Chunked blob storage over a DO's SQLite. Durable Object SQL caps individual
 * values at ~2MB; session snapshots and diff sidecars can exceed that, so
 * named blobs are stored as ordered chunk rows.
 */

export const CHUNK_BYTES = 1_500_000;

export interface BlobStore {
  put(name: string, bytes: Uint8Array): void;
  get(name: string): Uint8Array | undefined;
  delete(name: string): void;
  /** Its size in bytes (undefined: none), without reading it. */
  size(name: string): number | undefined;
  /** Its bytes from `start` to the end, read a chunk row at a time: a blob
   * larger than a DO can hold in memory streams through. */
  stream(name: string, start?: number): ReadableStream<Uint8Array>;
  /** `bytes` added at its end (a new blob when there's none). */
  append(name: string, bytes: Uint8Array): void;
  /** Renamed, replacing whatever `to` was: no bytes read or copied. */
  rename(from: string, to: string): void;
}

export const createBlobStore = (sql: SqlStorage): BlobStore => {
  sql.exec(
    "CREATE TABLE IF NOT EXISTS blobs (name TEXT NOT NULL, idx INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (name, idx))"
  );
  return {
    put(name, bytes) {
      sql.exec("DELETE FROM blobs WHERE name = ?", name);
      for (let i = 0, idx = 0; i === 0 || i < bytes.length; i += CHUNK_BYTES, idx++) {
        const chunk = bytes.subarray(i, Math.min(i + CHUNK_BYTES, bytes.length));
        sql.exec("INSERT INTO blobs (name, idx, bytes) VALUES (?, ?, ?)", name, idx, chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength));
      }
    },
    get(name) {
      const rows = [...sql.exec("SELECT bytes FROM blobs WHERE name = ? ORDER BY idx", name)];
      if (rows.length === 0) return undefined;
      const parts = rows.map((r) => new Uint8Array(r.bytes as ArrayBuffer));
      const total = parts.reduce((a, p) => a + p.length, 0);
      const out = new Uint8Array(total);
      let off = 0;
      for (const p of parts) {
        out.set(p, off);
        off += p.length;
      }
      return out;
    },
    delete(name) {
      sql.exec("DELETE FROM blobs WHERE name = ?", name);
    },
    size(name) {
      const row = [...sql.exec("SELECT COUNT(*) AS n, COALESCE(SUM(LENGTH(bytes)), 0) AS b FROM blobs WHERE name = ?", name)][0];
      return row && (row.n as number) > 0 ? (row.b as number) : undefined;
    },
    stream(name, start = 0) {
      // (Each row's length first, then one row read per pull: an appended
      // blob's rows aren't all CHUNK_BYTES.)
      const rows = [...sql.exec("SELECT idx, LENGTH(bytes) AS n FROM blobs WHERE name = ? ORDER BY idx", name)].map(
        (r) => ({ idx: r.idx as number, n: r.n as number })
      );
      let next = 0;
      let skip = start;
      while (next < rows.length && skip >= rows[next]!.n) {
        skip -= rows[next]!.n;
        next++;
      }
      return new ReadableStream<Uint8Array>({
        pull(controller) {
          const row = rows[next];
          if (!row) {
            controller.close();
            return;
          }
          const [hit] = [...sql.exec("SELECT bytes FROM blobs WHERE name = ? AND idx = ?", name, row.idx)];
          if (!hit) {
            controller.error(new Error(`blob ${name} changed while read`));
            return;
          }
          const bytes = new Uint8Array(hit.bytes as ArrayBuffer);
          controller.enqueue(skip > 0 ? bytes.subarray(skip) : bytes);
          skip = 0;
          next++;
        }
      });
    },
    append(name, bytes) {
      const last = [...sql.exec("SELECT COALESCE(MAX(idx), -1) AS i FROM blobs WHERE name = ?", name)][0]?.i as number;
      for (let i = 0, idx = last + 1; i < bytes.length; i += CHUNK_BYTES, idx++) {
        const chunk = bytes.subarray(i, Math.min(i + CHUNK_BYTES, bytes.length));
        sql.exec("INSERT INTO blobs (name, idx, bytes) VALUES (?, ?, ?)", name, idx, chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength));
      }
    },
    rename(from, to) {
      sql.exec("DELETE FROM blobs WHERE name = ?", to);
      sql.exec("UPDATE blobs SET name = ? WHERE name = ?", to, from);
    }
  };
};

export const textEncoder = new TextEncoder();
export const textDecoder = new TextDecoder();

export const putJsonBlob = (store: BlobStore, name: string, value: unknown): void =>
  store.put(name, textEncoder.encode(JSON.stringify(value)));

export const getJsonBlob = <T>(store: BlobStore, name: string): T | undefined => {
  const bytes = store.get(name);
  if (!bytes) return undefined;
  try {
    return JSON.parse(textDecoder.decode(bytes)) as T;
  } catch {
    return undefined;
  }
};
