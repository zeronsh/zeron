/**
 * Billed-row metering for a DO's SQLite.
 *
 * Cloudflare bills SQLite-backed Durable Objects per row read and per row
 * written, and the numbers it bills are the final `rowsRead`/`rowsWritten`
 * of each `SqlStorageCursor` (index maintenance and deletes included). This
 * wrapper reads those same counters off every cursor the room opens, so
 * `/stats` reports what the room actually costs instead of an estimate.
 *
 * Counters are in memory only: persisting them would itself be billed writes.
 * They reset when the DO is evicted, which is fine for their two uses —
 * per-room telemetry while a chat is busy, and the chat3 cost benchmark.
 */

export interface SqlMeter {
  rowsRead: number;
  rowsWritten: number;
  statements: number;
}

export const newSqlMeter = (): SqlMeter => ({ rowsRead: 0, rowsWritten: 0, statements: 0 });

/** A `SqlStorage` whose `exec` tallies into `meter`. Writes complete inside
 * `exec`, so their counts are final at once; a read's `rowsRead` grows as the
 * cursor is consumed, so it is re-read when iteration finishes. */
export const meterSql = (sql: SqlStorage, meter: SqlMeter): SqlStorage =>
  new Proxy(sql, {
    get(target, prop, receiver) {
      if (prop !== "exec") {
        const value = Reflect.get(target, prop, receiver);
        return typeof value === "function" ? value.bind(target) : value;
      }
      return (query: string, ...bindings: unknown[]) => {
        const cursor = target.exec(query, ...bindings);
        meter.statements += 1;
        meter.rowsWritten += cursor.rowsWritten;
        return trackReads(cursor, meter);
      };
    }
  });

/** Wrap a cursor so the rows it reads are counted once it is drained (or
 * abandoned after `toArray`/`one`). Counted by delta, so partial reads that
 * resume are never double counted. */
const trackReads = <T extends Record<string, SqlStorageValue>>(
  cursor: SqlStorageCursor<T>,
  meter: SqlMeter
): SqlStorageCursor<T> => {
  let counted = 0;
  const settle = () => {
    const now = cursor.rowsRead;
    meter.rowsRead += now - counted;
    counted = now;
  };
  settle();
  return new Proxy(cursor, {
    get(target, prop, receiver) {
      if (prop === Symbol.iterator) {
        return function* () {
          try {
            for (const row of target) yield row;
          } finally {
            settle();
          }
        };
      }
      if (prop === "toArray" || prop === "one" || prop === "next" || prop === "raw") {
        const fn = Reflect.get(target, prop, receiver) as (...args: unknown[]) => unknown;
        return (...args: unknown[]) => {
          const out = fn.apply(target, args);
          settle();
          return out;
        };
      }
      const value = Reflect.get(target, prop, receiver);
      return typeof value === "function" ? value.bind(target) : value;
    }
  });
};
