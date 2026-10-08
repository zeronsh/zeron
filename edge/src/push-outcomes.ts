/**
 * Per-device push bookkeeping for ChatRoom/RegistryRoom `/stats` — the only
 * per-device attribution surface (2026-08-05 incident tooling), kept without
 * paying a SQLite row write on every push.
 *
 * Rows written are the billed dimension once a month's free allowance is
 * spent, and rewriting this JSON blob per push was a fifth of a push's cost.
 * Rejections are rare and are what an incident needs, so they persist at
 * once. A success persists only when that DEVICE's stored `lastOkAt` is
 * [`FLUSH_MS`] old or more; the decision reads the stored value, so it holds
 * across hibernation and instance recreation, and one busy device can't
 * starve another's flushes. Unflushed successes live in memory, which
 * hibernation drops: the stored `ok` is a lower bound (the live room's
 * `snapshot()` is exact), and the stored `lastOkAt` trails a device's real
 * last success by less than [`FLUSH_MS`].
 */

export interface PushOutcome {
  ok: number;
  rejected: number;
  lastOkAt: number;
}

export const FLUSH_MS = 60_000;

export class PushOutcomeLog {
  private readonly read: () => string | undefined;
  private readonly write: (value: string) => void;
  private outcomes: Record<string, PushOutcome> | undefined;
  /** device → `lastOkAt` as last written to storage. */
  private stored = new Map<string, number>();

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
    const storedAt = this.stored.get(key);
    if (!ok || storedAt === undefined || now - storedAt >= FLUSH_MS) this.flush(outcomes);
  }

  /** Drop the in-memory copy after an operator wipe deleted the stored one,
   * so the next flush doesn't resurrect pre-wipe counts. */
  reset(): void {
    this.outcomes = undefined;
    this.stored = new Map();
  }

  /** Freshest view: the in-memory counts, which include unflushed successes. */
  snapshot(): Record<string, PushOutcome> {
    return this.load();
  }

  private load(): Record<string, PushOutcome> {
    if (this.outcomes === undefined) {
      this.outcomes = JSON.parse(this.read() ?? "{}") as Record<string, PushOutcome>;
      for (const [key, entry] of Object.entries(this.outcomes)) this.stored.set(key, entry.lastOkAt);
    }
    return this.outcomes;
  }

  private flush(outcomes: Record<string, PushOutcome>): void {
    this.write(JSON.stringify(outcomes));
    for (const [key, entry] of Object.entries(outcomes)) this.stored.set(key, entry.lastOkAt);
  }
}
