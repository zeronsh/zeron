/**
 * Per-device push bookkeeping for ChatRoom/RegistryRoom `/stats` — the only
 * per-device attribution surface (2026-08-05 incident tooling), kept without
 * paying a SQLite row write on every push.
 *
 * Rows written are the billed dimension once a month's free allowance is
 * spent, and rewriting this JSON blob per push was a fifth of a ChatRoom
 * push's cost. Rejections are rare and are what an incident needs, so they
 * persist at once; successes persist at most once per [`FLUSH_MS`]. Between
 * flushes they live in memory, which hibernation drops — `ok` is a lower
 * bound and `lastOkAt` may trail by up to a flush interval.
 */

export interface PushOutcome {
  ok: number;
  rejected: number;
  lastOkAt: number;
}

const FLUSH_MS = 60_000;

export class PushOutcomeLog {
  private readonly read: () => string | undefined;
  private readonly write: (value: string) => void;
  private outcomes: Record<string, PushOutcome> | undefined;
  private lastFlushAt = 0;

  constructor(read: () => string | undefined, write: (value: string) => void) {
    this.read = read;
    this.write = write;
  }

  record(device: string, ok: boolean, now = Date.now()): void {
    const outcomes = this.load();
    const key = device === "" ? "(unknown)" : device;
    const entry = outcomes[key] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
    if (ok) {
      entry.ok += 1;
      entry.lastOkAt = now;
    } else {
      entry.rejected += 1;
    }
    outcomes[key] = entry;
    if (!ok || now - this.lastFlushAt >= FLUSH_MS) {
      this.write(JSON.stringify(outcomes));
      this.lastFlushAt = now;
    }
  }

  /** Drop the in-memory copy after an operator wipe deleted the stored one,
   * so the next flush doesn't resurrect pre-wipe counts. */
  reset(): void {
    this.outcomes = undefined;
    this.lastFlushAt = 0;
  }

  /** Freshest view: the in-memory counts, which include unflushed successes. */
  snapshot(): Record<string, PushOutcome> {
    return this.load();
  }

  private load(): Record<string, PushOutcome> {
    this.outcomes ??= JSON.parse(this.read() ?? "{}") as Record<string, PushOutcome>;
    return this.outcomes;
  }
}
