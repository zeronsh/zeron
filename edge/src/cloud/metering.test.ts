import { describe, expect, it } from "vitest";
import {
  CLOSE_GRACE_MS,
  meteringOpen,
  monthEnd,
  monthKey,
  monthStart,
  monthsBetween,
  planReconcile,
  previousMonth,
  summarizeMonth,
  type SandboxRecord,
  type UsageRow
} from "./metering";

const at = (iso: string) => Date.parse(iso);

const row = (month: string, sandboxId: string, extra: Partial<UsageRow> = {}): UsageRow => ({
  month,
  provider: "boat",
  sandboxId,
  sandboxType: "default",
  seconds: 100,
  dollars: 0.001,
  running: false,
  reconciledAt: at(`${month}-15T00:00:00Z`),
  closed: false,
  ...extra
});

describe("UTC month math", () => {
  it("keys, starts, ends and walks months", () => {
    expect(monthKey(at("2026-09-30T23:59:59.999Z"))).toBe("2026-09");
    expect(monthKey(at("2026-10-01T00:00:00Z"))).toBe("2026-10");
    expect(monthStart("2026-12")).toBe(at("2026-12-01T00:00:00Z"));
    expect(monthEnd("2026-12")).toBe(at("2027-01-01T00:00:00Z"));
    expect(previousMonth("2027-01")).toBe("2026-12");
    expect(monthsBetween(at("2026-11-20T00:00:00Z"), at("2027-01-02T00:00:00Z"))).toEqual(["2026-11", "2026-12", "2027-01"]);
  });
});

describe("planReconcile", () => {
  const sandbox: SandboxRecord = { provider: "boat", sandboxId: "bx_a", type: "default", createdAt: at("2026-09-20T10:00:00Z") };

  it("splits a sandbox that spans a month boundary into per-month windows", () => {
    const now = at("2026-10-01T00:30:00Z"); // inside the grace hour
    const tasks = planReconcile([sandbox], [], now);
    expect(tasks).toEqual([
      { month: "2026-09", provider: "boat", sandboxId: "bx_a", sandboxType: "default", since: at("2026-09-01T00:00:00Z"), until: at("2026-10-01T00:00:00Z"), fetch: true, close: false, deleted: false },
      { month: "2026-10", provider: "boat", sandboxId: "bx_a", sandboxType: "default", since: at("2026-10-01T00:00:00Z"), until: now, fetch: true, close: false, deleted: false }
    ]);
  });

  it("closes a month only after it ended plus the grace hour, with until = month end", () => {
    const now = at("2026-10-01T00:00:00Z") + CLOSE_GRACE_MS;
    const [september] = planReconcile([sandbox], [], now);
    expect(september).toMatchObject({ month: "2026-09", until: at("2026-10-01T00:00:00Z"), close: true, fetch: true });
  });

  it("never revisits a closed row", () => {
    const now = at("2026-10-05T00:00:00Z");
    const tasks = planReconcile([sandbox], [row("2026-09", "bx_a", { closed: true })], now);
    expect(tasks.map((t) => t.month)).toEqual(["2026-10"]);
  });

  it("treats a pre-delete capture as final and only closes it at month end", () => {
    const deleted: SandboxRecord = {
      ...sandbox,
      deletedAt: at("2026-10-03T12:00:01Z"),
      finalAt: at("2026-10-03T12:00:00Z")
    };
    const captured = [row("2026-09", "bx_a", { closed: true }), row("2026-10", "bx_a", { reconciledAt: at("2026-10-03T12:00:00Z") })];
    expect(planReconcile([deleted], captured, at("2026-10-20T00:00:00Z"))).toEqual([]);
    expect(planReconcile([deleted], captured, at("2026-11-01T02:00:00Z"))).toEqual([
      expect.objectContaining({ month: "2026-10", fetch: false, close: true, deleted: true })
    ]);
  });

  it("bounds a deleted sandbox's window by its deletion and stops there", () => {
    const deleted: SandboxRecord = { ...sandbox, deletedAt: at("2026-10-03T12:00:00Z") };
    const tasks = planReconcile([deleted], [], at("2026-12-01T05:00:00Z"));
    expect(tasks.map((t) => [t.month, t.until, t.close])).toEqual([
      ["2026-09", at("2026-10-01T00:00:00Z"), true],
      ["2026-10", at("2026-10-03T12:00:00Z"), true]
    ]);
    expect(meteringOpen([deleted], [row("2026-09", "bx_a", { closed: true }), row("2026-10", "bx_a", { closed: true })], at("2026-12-01T05:00:00Z"))).toBe(false);
  });
});

describe("summarizeMonth", () => {
  const sandboxes: SandboxRecord[] = [
    { provider: "boat", sandboxId: "bx_a", type: "default", createdAt: at("2026-09-02T00:00:00Z") },
    { provider: "boat", sandboxId: "bx_b", type: "large", createdAt: at("2026-09-10T00:00:00Z"), deletedAt: at("2026-09-11T00:00:00Z") }
  ];
  it("sums the month and is closed only when every sandbox row is", () => {
    const now = at("2026-10-02T00:00:00Z");
    const rows = [row("2026-09", "bx_a", { seconds: 3600, dollars: 0.036, closed: true }), row("2026-09", "bx_b", { seconds: 2400, dollars: 0.024, sandboxType: "large" })];
    const open = summarizeMonth("2026-09", sandboxes, rows, now);
    expect(open).toMatchObject({ month: "2026-09", seconds: 6000, dollars: 0.06, closed: false, available: true });
    const done = summarizeMonth("2026-09", sandboxes, rows.map((r) => ({ ...r, closed: true })), now);
    expect(done.closed).toBe(true);
    // A sandbox that existed in the month but has no row yet keeps it open.
    expect(summarizeMonth("2026-09", sandboxes, [rows[0]!], now).closed).toBe(false);
  });
  it("is never closed before the month (plus grace) is over", () => {
    expect(summarizeMonth("2026-10", [], [], at("2026-10-31T23:00:00Z")).closed).toBe(false);
    expect(summarizeMonth("2026-10", [], [], at("2026-11-01T01:00:00Z")).closed).toBe(true);
  });
});
