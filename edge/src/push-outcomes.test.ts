import { describe, expect, it } from "vitest";
import { FLUSH_MS, PushOutcomeLog, type PushOutcome } from "./push-outcomes";

/** A realistic epoch, so "no flush yet" never reads as an ancient one. */
const T0 = 1_790_000_000_000;

/** One stored JSON slot, shared by every instance — the DO's `meta` row. */
const storage = () => {
  const slot = { value: undefined as string | undefined, writes: 0 };
  const open = () =>
    new PushOutcomeLog(
      () => slot.value,
      (value) => {
        slot.value = value;
        slot.writes += 1;
      }
    );
  const stored = () => JSON.parse(slot.value ?? "{}") as Record<string, PushOutcome>;
  return { slot, open, stored };
};

describe("PushOutcomeLog", () => {
  it("persists rejections at once", () => {
    const { open, stored } = storage();
    const log = open();
    log.record("dev", true, T0);
    log.record("dev", false, T0 + 1_000);
    expect(stored().dev).toEqual({ ok: 1, rejected: 1, lastOkAt: T0 });
  });

  it("keeps successes in memory between flushes; the snapshot is exact", () => {
    const { slot, open, stored } = storage();
    const log = open();
    for (let i = 0; i < 10; i++) log.record("dev", true, T0 + i * 1_000);
    expect(slot.writes).toBe(1);
    expect(stored().dev).toEqual({ ok: 1, rejected: 0, lastOkAt: T0 });
    expect(log.snapshot().dev).toEqual({ ok: 10, rejected: 0, lastOkAt: T0 + 9_000 });
  });

  it("bounds each device's stored lastOkAt lag when one busy device always flushes first", () => {
    // The starvation shape: each instance serves a busy device's push, then
    // a quiet device's, then hibernation recreates it. A room-wide flush
    // timer let the busy push flush and the quiet one wait in memory to be
    // dropped — forever. The decision now reads each device's STORED time.
    const { open, stored } = storage();
    const last: Record<string, number> = {};
    let rounds = 0;
    for (let t = T0; t < T0 + 10 * FLUSH_MS; t += 70_000) {
      rounds += 1;
      const log = open();
      for (const [device, now] of [["busy", t], ["quiet", t + 3_000]] as const) {
        log.record(device, true, now);
        last[device] = now;
      }
      for (const [device, real] of Object.entries(last)) {
        expect(real - (stored()[device]?.lastOkAt ?? 0)).toBeLessThan(FLUSH_MS);
      }
    }
    expect(rounds).toBe(9);
  });

  it("a recreated instance doesn't re-flush a device stored within the window", () => {
    const { slot, open } = storage();
    open().record("dev", true, T0);
    open().record("dev", true, T0 + 30_000);
    open().record("dev", true, T0 + 59_000);
    expect(slot.writes).toBe(1);
    open().record("dev", true, T0 + 61_000);
    expect(slot.writes).toBe(2);
  });

  it("reset forgets counts the wipe deleted", () => {
    const { slot, open, stored } = storage();
    const log = open();
    log.record("dev", true, T0);
    log.record("dev", true, T0 + 1_000);
    slot.value = undefined; // operator /reset: DELETE FROM meta
    log.reset();
    log.record("dev", true, T0 + 2_000);
    expect(stored().dev).toEqual({ ok: 1, rejected: 0, lastOkAt: T0 + 2_000 });
  });
});
