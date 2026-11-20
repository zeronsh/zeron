/**
 * Counts the SQLite rows a Durable Object writes, as Cloudflare bills them:
 * each statement's `rowsWritten` (index entries included), plus every
 * `setAlarm`. Memory-only and reset when the object restarts; a room puts it
 * on `/stats` so its write cost can be read off a live object.
 */
export class WriteMeter {
  private written = 0;
  private alarms = 0;
  /** `rowsWritten` is only final once the statement ran, which a later
   * statement (or a read of the meter) guarantees for a write. */
  private last?: SqlStorageCursor<Record<string, SqlStorageValue>>;

  constructor(private readonly inner: SqlStorage) {}

  exec<T extends Record<string, SqlStorageValue>>(query: string, ...bindings: unknown[]): SqlStorageCursor<T> {
    this.settle();
    const cursor = this.inner.exec<T>(query, ...bindings);
    this.last = cursor as unknown as SqlStorageCursor<Record<string, SqlStorageValue>>;
    return cursor;
  }

  /** Count one `setAlarm`. */
  alarm(): void {
    this.alarms += 1;
  }

  snapshot(): { rows: number; alarms: number } {
    this.settle();
    return { rows: this.written, alarms: this.alarms };
  }

  private settle(): void {
    if (this.last === undefined) return;
    this.written += this.last.rowsWritten;
    this.last = undefined;
  }
}
