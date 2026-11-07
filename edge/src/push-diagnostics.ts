export interface PushOutcome {
  ok: number;
  rejected: number;
  lastOkAt: number;
}

/** Best-effort attribution, never part of the durable sync receipt. Checkpoint
 * successes on activity (no timers that prevent hibernation); rejections flush
 * immediately. Eviction can discard at most 63 successes per room. */
export class PushDiagnostics {
  private pending: Record<string, PushOutcome> = Object.create(null);
  private successes = 0;
  private lastFlush: number | undefined;

  constructor(private readonly sql: SqlStorage) {}

  snapshot(): Record<string, PushOutcome> {
    const raw = [...this.sql.exec<{ value: string }>("SELECT value FROM meta WHERE key = 'pushOutcomes'")][0];
    const outcomes: Record<string, PushOutcome> = Object.assign(Object.create(null), JSON.parse(raw?.value ?? "{}"));
    for (const [device, delta] of Object.entries(this.pending)) {
      const before = outcomes[device] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
      outcomes[device] = {
        ok: before.ok + delta.ok,
        rejected: before.rejected + delta.rejected,
        lastOkAt: Math.max(before.lastOkAt, delta.lastOkAt)
      };
    }
    return outcomes;
  }

  record(device: string, ok: boolean): void {
    const now = Date.now();
    const key = device || "(unknown)";
    const delta = this.pending[key] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
    if (ok) {
      delta.ok += 1;
      delta.lastOkAt = now;
      this.successes += 1;
    } else delta.rejected += 1;
    this.pending[key] = delta;
    if (!ok || this.lastFlush === undefined || this.successes >= 64 || now - this.lastFlush >= 60_000) {
      this.sql.exec(
        "INSERT INTO meta (key, value) VALUES ('pushOutcomes', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        JSON.stringify(this.snapshot())
      );
      this.reset();
      this.lastFlush = now;
    }
  }

  reset(): void {
    this.pending = Object.create(null);
    this.successes = 0;
    this.lastFlush = undefined;
  }
}
