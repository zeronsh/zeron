import { describe, expect, it } from "vitest";
import {
  CLOSE_GRACE_MS,
  creditsOf,
  creditsPerDay,
  dayKey,
  daysBetween,
  isDayKey,
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

const row = (day: string, sandboxId: string, extra: Partial<UsageRow> = {}): UsageRow => ({
  day,
  provider: "boat",
  sandboxId,
  sandboxType: "default",
  seconds: 100,
  dollars: 0.001,
  running: false,
  reconciledAt: at(`${day}T12:00:00Z`),
  closed: false,
  ...extra
});

describe("UTC month and day math", () => {
  it("keys, starts, ends and walks months", () => {
    expect(monthKey(at("2026-09-30T23:59:59.999Z"))).toBe("2026-09");
    expect(monthKey(at("2026-10-01T00:00:00Z"))).toBe("2026-10");
    expect(monthStart("2026-12")).toBe(at("2026-12-01T00:00:00Z"));
    expect(monthEnd("2026-12")).toBe(at("2027-01-01T00:00:00Z"));
    expect(previousMonth("2027-01")).toBe("2026-12");
    expect(monthsBetween(at("2026-11-20T00:00:00Z"), at("2027-01-02T00:00:00Z"))).toEqual(["2026-11", "2026-12", "2027-01"]);
  });
  it("keys and walks days, and validates them", () => {
    expect(dayKey(at("2026-09-30T23:59:59.999Z"))).toBe("2026-09-30");
    expect(daysBetween(at("2026-12-30T10:00:00Z"), at("2027-01-01T01:00:00Z"))).toEqual(["2026-12-30", "2026-12-31", "2027-01-01"]);
    expect(isDayKey("2026-02-28")).toBe(true);
    expect(isDayKey("2026-02-30")).toBe(false);
    expect(isDayKey("2026-2-3")).toBe(false);
  });
  it("counts a credit per minute of a small machine", () => {
    expect(creditsOf(60)).toBe(1);
    expect(creditsOf(90)).toBe(1.5);
    expect(creditsOf(86_400)).toBe(1440);
  });
});

describe("planReconcile", () => {
  const sandbox: SandboxRecord = { provider: "boat", sandboxId: "bx_a", type: "default", createdAt: at("2026-09-29T22:00:00Z") };

  it("splits a sandbox that spans days into per-day windows", () => {
    const now = at("2026-09-30T00:30:00Z"); // inside the grace hour
    const tasks = planReconcile([sandbox], [], now);
    expect(tasks).toEqual([
      { day: "2026-09-29", provider: "boat", sandboxId: "bx_a", sandboxType: "default", since: at("2026-09-29T00:00:00Z"), until: at("2026-09-30T00:00:00Z"), fetch: true, close: false, deleted: false },
      { day: "2026-09-30", provider: "boat", sandboxId: "bx_a", sandboxType: "default", since: at("2026-09-30T00:00:00Z"), until: now, fetch: true, close: false, deleted: false }
    ]);
  });

  it("closes a day only after it ended plus the grace hour, with until = day end", () => {
    const now = at("2026-09-30T00:00:00Z") + CLOSE_GRACE_MS;
    const [first] = planReconcile([sandbox], [], now);
    expect(first).toMatchObject({ day: "2026-09-29", until: at("2026-09-30T00:00:00Z"), close: true, fetch: true });
  });

  it("never revisits a closed row", () => {
    const now = at("2026-09-30T12:00:00Z");
    const tasks = planReconcile([sandbox], [row("2026-09-29", "bx_a", { closed: true })], now);
    expect(tasks.map((t) => t.day)).toEqual(["2026-09-30"]);
  });

  it("treats a pre-delete capture as final and only closes it at day end", () => {
    const deleted: SandboxRecord = {
      ...sandbox,
      deletedAt: at("2026-09-30T12:00:01Z"),
      finalAt: at("2026-09-30T12:00:00Z")
    };
    const captured = [row("2026-09-29", "bx_a", { closed: true }), row("2026-09-30", "bx_a", { reconciledAt: at("2026-09-30T12:00:00Z") })];
    expect(planReconcile([deleted], captured, at("2026-09-30T20:00:00Z"))).toEqual([]);
    expect(planReconcile([deleted], captured, at("2026-10-01T02:00:00Z"))).toEqual([
      expect.objectContaining({ day: "2026-09-30", fetch: false, close: true, deleted: true })
    ]);
  });

  it("bounds a deleted sandbox's window by its deletion and stops there", () => {
    const deleted: SandboxRecord = { ...sandbox, deletedAt: at("2026-09-30T12:00:00Z") };
    const tasks = planReconcile([deleted], [], at("2026-12-01T05:00:00Z"));
    expect(tasks.map((t) => [t.day, t.until, t.close])).toEqual([
      ["2026-09-29", at("2026-09-30T00:00:00Z"), true],
      ["2026-09-30", at("2026-09-30T12:00:00Z"), true]
    ]);
    expect(meteringOpen([deleted], [row("2026-09-29", "bx_a", { closed: true }), row("2026-09-30", "bx_a", { closed: true })], at("2026-12-01T05:00:00Z"))).toBe(false);
  });
});

describe("summarizeMonth", () => {
  const sandboxes: SandboxRecord[] = [
    { provider: "boat", sandboxId: "bx_a", type: "default", createdAt: at("2026-09-29T00:00:00Z"), deletedAt: at("2026-09-30T12:00:00Z") },
    { provider: "boat", sandboxId: "bx_b", type: "large", createdAt: at("2026-09-10T00:00:00Z"), deletedAt: at("2026-09-10T06:00:00Z") }
  ];
  const rows = [
    row("2026-09-29", "bx_a", { seconds: 3600, dollars: 0.036, closed: true }),
    row("2026-09-30", "bx_a", { seconds: 1800, dollars: 0.018 }),
    row("2026-09-10", "bx_b", { seconds: 2400, dollars: 0.024, sandboxType: "large", closed: true }),
    row("2026-10-01", "bx_c", { seconds: 999 })
  ];
  it("sums a month's days per sandbox and is closed only when every day is", () => {
    const now = at("2026-10-02T00:00:00Z");
    const open = summarizeMonth("2026-09", sandboxes, rows, now);
    expect(open).toMatchObject({ month: "2026-09", seconds: 7800, dollars: 0.078, closed: false, available: true });
    expect(open.sandboxes).toEqual([
      expect.objectContaining({ sandboxId: "bx_a", seconds: 5400 }),
      expect.objectContaining({ sandboxId: "bx_b", seconds: 2400, sandboxType: "large" })
    ]);
    const done = summarizeMonth("2026-09", sandboxes, rows.map((r) => ({ ...r, closed: true })), now);
    expect(done.closed).toBe(true);
    // A day a sandbox existed on but with no row yet keeps the month open.
    expect(summarizeMonth("2026-09", sandboxes, rows.filter((r) => r.day !== "2026-09-29").map((r) => ({ ...r, closed: true })), now).closed).toBe(false);
  });
  it("is never closed before the month (plus grace) is over", () => {
    expect(summarizeMonth("2026-10", [], [], at("2026-10-31T23:00:00Z")).closed).toBe(false);
    expect(summarizeMonth("2026-10", [], [], at("2026-11-01T01:00:00Z")).closed).toBe(true);
  });
});

describe("creditsPerDay", () => {
  it("lists every day of the range, zero when nothing ran", () => {
    const rows = [row("2026-09-29", "bx_a", { seconds: 120 }), row("2026-09-29", "bx_b", { seconds: 60 }), row("2026-10-02", "bx_a", { seconds: 30 })];
    expect(creditsPerDay("2026-09-28", "2026-09-30", rows)).toEqual([
      { day: "2026-09-28", credits: 0 },
      { day: "2026-09-29", credits: 3 },
      { day: "2026-09-30", credits: 0 }
    ]);
  });
});
