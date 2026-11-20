import { describe, expect, it } from "vitest";
import {
  CLOSE_GRACE_MS,
  NO_CUTOVER,
  creditBreakdown,
  creditRate,
  creditSeconds,
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
  secondsPerCredit,
  sizeMultiplier,
  summarizeMonth,
  unmeteredCredits,
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
  it("counts a credit per minute of a small machine, which bills 30 seconds a minute", () => {
    const billed = 60 * sizeMultiplier("small");
    expect(billed).toBe(30);
    expect(creditsOf(billed, 30)).toBe(1);
    expect(creditsOf(45, 30)).toBe(1.5);
    expect(creditsOf(86_400, 30)).toBe(2880);
    // Days before the cutover keep their rate.
    expect(creditsOf(86_400, 60)).toBe(1440);
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

describe("creditBreakdown", () => {
  const period = (invoice: string, start: string, end: string, credits: number) => ({
    invoice,
    plan: "base",
    start: at(start),
    end: at(end),
    credits
  });
  const use = (day: string, minutes: number) => row(day, `bx_${day}`, { seconds: minutes * 60 });

  it("with no plan, everything comes out of credits that never expire", () => {
    expect(creditBreakdown(1000, [], [use("2026-10-01", 300)], at("2026-10-02T00:00:00Z"))).toEqual({
      monthly: 0,
      monthlyTotal: 0,
      extra: 700
    });
  });

  it("spends the period's plan credits first, then extra; unused plan credits lapse", () => {
    const periods = [
      period("in_sep", "2026-09-05T00:00:00Z", "2026-10-05T00:00:00Z", 1000),
      period("in_oct", "2026-10-05T00:00:00Z", "2026-11-05T00:00:00Z", 1000)
    ];
    const rows = [
      use("2026-09-10", 400), // September: 600 of its 1000 lapse
      use("2026-10-06", 900),
      use("2026-10-07", 300), // October: 200 past its 1000 come out of extra
      use("2026-08-01", 50) // before any plan: extra
    ];
    expect(creditBreakdown(500, periods, rows, at("2026-10-08T00:00:00Z"))).toEqual({
      monthly: 0,
      monthlyTotal: 1000,
      monthlyEndsAt: at("2026-11-05T00:00:00Z"),
      extra: 250
    });
    // Mid-October with less use: plan credits left this period.
    expect(creditBreakdown(0, periods, [use("2026-10-06", 100)], at("2026-10-08T00:00:00Z"))).toMatchObject({
      monthly: 900,
      extra: 0
    });
    // After the period ends with nothing renewed, the plan credits are gone.
    expect(creditBreakdown(0, periods, [], at("2026-11-06T00:00:00Z"))).toEqual({ monthly: 0, monthlyTotal: 0, extra: 0 });
  });

  it("after a mid-period upgrade, the new plan's credits cover the rest of the period", () => {
    const periods = [
      period("in_base", "2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z", 1000),
      period("in_upgrade", "2026-10-10T12:00:00Z", "2026-11-01T00:00:00Z", 5000)
    ];
    const rows = [use("2026-10-05", 800), use("2026-10-11", 2000)];
    expect(creditBreakdown(0, periods, rows, at("2026-10-12T00:00:00Z"))).toMatchObject({
      monthly: 3000,
      monthlyTotal: 5000,
      extra: 0
    });
  });

  it("scales by the configured billable seconds per credit", () => {
    expect(creditBreakdown(100, [], [use("2026-10-01", 60)], at("2026-10-02T00:00:00Z"), { seconds: 120, from: "2026-10-01" }).extra).toBe(70);
    expect(creditSeconds("120")).toBe(120);
    expect(creditSeconds(undefined)).toBe(60);
    expect(creditSeconds("-5")).toBe(60);
  });
});

describe("the credit rate cutover", () => {
  const CUTOVER = "2026-11-10";
  const rate = creditRate("30", CUTOVER);
  const use = (day: string, seconds: number, sandboxId = "bx_a") => row(day, sandboxId, { seconds });
  const period = (start: string, end: string, credits: number) => ({ invoice: start, plan: "base", start: at(start), end: at(end), credits });

  it("goes by the UTC day: 60 before the cutover, the new rate from it on", () => {
    expect(secondsPerCredit("2026-11-09", rate)).toBe(60);
    expect(secondsPerCredit(CUTOVER, rate)).toBe(30);
    expect(secondsPerCredit("2026-12-01", rate)).toBe(30);
    expect(secondsPerCredit("2027-01-01", rate)).toBe(30);
  });

  it("changes nothing without a real cutover day, whatever the rate says", () => {
    for (const from of [undefined, "", "SET-AT-DEPLOY", "2026-02-30", "2026-11", "11/10/2026"]) {
      expect(creditRate("30", from), String(from)).toEqual(NO_CUTOVER);
    }
    expect(secondsPerCredit("2030-01-01", creditRate("30", "SET-AT-DEPLOY"))).toBe(60);
    // A cutover with no usable rate stays at 60 too.
    expect(secondsPerCredit("2030-01-01", creditRate(undefined, CUTOVER))).toBe(60);
    expect(secondsPerCredit("2030-01-01", creditRate("-5", CUTOVER))).toBe(60);

    const rows = [use("2026-11-09", 6000), use(CUTOVER, 6000)];
    const now = at("2026-11-11T00:00:00Z");
    const placeholder = creditRate("30", "SET-AT-DEPLOY");
    expect(creditBreakdown(1000, [], rows, now, placeholder)).toEqual(creditBreakdown(1000, [], rows, now));
    expect(creditBreakdown(1000, [], rows, now, placeholder).extra).toBe(800);
    expect(creditsPerDay("2026-11-09", CUTOVER, rows, placeholder)).toEqual(creditsPerDay("2026-11-09", CUTOVER, rows));
    expect(unmeteredCredits(now - 60_000, now, 1, placeholder)).toBe(unmeteredCredits(now - 60_000, now, 1));
  });

  it("doesn't re-price past usage: a balance stays put on deploy, even one a flat rate would sink", () => {
    const rows = [use("2026-11-01", 45_000), use("2026-11-09", 3000)];
    const deployed = at("2026-11-09T12:00:00Z"); // the cutover is still ahead
    expect(creditBreakdown(1000, [], rows, deployed)).toEqual({ monthly: 0, monthlyTotal: 0, extra: 200 });
    expect(creditBreakdown(1000, [], rows, deployed, rate)).toEqual({ monthly: 0, monthlyTotal: 0, extra: 200 });
    // Long after the cutover, those days still haven't moved.
    expect(creditBreakdown(1000, [], rows, at("2026-12-01T00:00:00Z"), rate).extra).toBe(200);
    // What the cutover guards against.
    expect(creditBreakdown(1000, [], rows, deployed, { seconds: 30, from: "2020-01-01" }).extra).toBe(-600);
  });

  it("re-reads a day that closes after the cutover at that day's own rate", () => {
    const late = [use("2026-11-09", 6000, "bx_late")];
    expect(creditsPerDay("2026-11-09", "2026-11-09", late, rate)).toEqual([{ day: "2026-11-09", credits: 100 }]);
  });

  it("charges a machine running across the cutover by day", () => {
    const rows = [use("2026-11-09", 86_400), use(CUTOVER, 86_400), use("2026-11-11", 3600)];
    expect(creditsPerDay("2026-11-09", "2026-11-11", rows, rate)).toEqual([
      { day: "2026-11-09", credits: 1440 },
      { day: CUTOVER, credits: 2880 },
      { day: "2026-11-11", credits: 120 }
    ]);
    expect(creditBreakdown(10_000, [], rows, at("2026-11-12T00:00:00Z"), rate).extra).toBe(10_000 - 1440 - 2880 - 120);
  });

  it("adds up a plan period straddling the cutover at each day's rate", () => {
    const periods = [period("2026-11-01T00:00:00Z", "2026-12-01T00:00:00Z", 1000)];
    const rows = [use("2026-11-05", 12_000), use("2026-11-15", 6000)]; // 200 + 200
    const now = at("2026-11-20T00:00:00Z");
    expect(creditBreakdown(0, periods, rows, now, rate)).toMatchObject({ monthly: 600, monthlyTotal: 1000, extra: 0 });
    // Past the plan's credits, the rest comes out of extra at the day's rate too.
    const more = [...rows, use("2026-11-16", 24_000)]; // + 800
    expect(creditBreakdown(500, periods, more, now, rate)).toMatchObject({ monthly: 0, extra: 300 });
  });

  it("splits a running machine's estimate at the day boundary", () => {
    const midnight = at(`${CUTOVER}T00:00:00Z`);
    const half = 30 * 60_000;
    // A default machine bills a second a second: 30 credits at 60, then 60 at 30.
    expect(unmeteredCredits(midnight - half, midnight + half, 1, rate)).toBeCloseTo(90);
    expect(unmeteredCredits(midnight - 2 * half, midnight, 1, rate)).toBeCloseTo(60);
    expect(unmeteredCredits(midnight, midnight + 2 * half, 1, rate)).toBeCloseTo(120);
    // Without a cutover it's what it always was.
    expect(unmeteredCredits(midnight - half, midnight + half, 1)).toBeCloseTo(60);
    expect(unmeteredCredits(midnight + half, midnight, 1, rate)).toBe(0);
    // Spanning whole days.
    expect(unmeteredCredits(midnight - 86_400_000, midnight + 86_400_000, 1, rate)).toBeCloseTo(1440 + 2880);
  });

  it("spends 1, 2, 4 and about 11 credits a minute by size at the new rate", () => {
    const start = at(`${CUTOVER}T12:00:00Z`);
    const perMinute = (machine: string) => unmeteredCredits(start, start + 60_000, sizeMultiplier(machine), rate);
    expect(perMinute("small")).toBeCloseTo(1);
    expect(perMinute("default")).toBeCloseTo(2);
    expect(perMinute("large")).toBeCloseTo(4);
    expect(perMinute("xlarge")).toBeCloseTo(11.12);
    // The same from the provider's meter, once it has read them.
    for (const [machine, credits] of [["small", 1], ["default", 2], ["large", 4], ["xlarge", 11.12]] as const) {
      const billed = use(CUTOVER, 60 * sizeMultiplier(machine));
      expect(creditsPerDay(CUTOVER, CUTOVER, [billed], rate)[0]!.credits).toBe(credits);
    }
    // Before the cutover, half that.
    const earlier = at("2026-11-09T12:00:00Z");
    expect(unmeteredCredits(earlier, earlier + 60_000, sizeMultiplier("default"), rate)).toBeCloseTo(1);
  });

  it("says the same in operator figures: seconds and dollars don't depend on the rate", () => {
    const rows = [use("2026-11-09", 3600), use(CUTOVER, 3600)];
    const month = summarizeMonth("2026-11", [], rows, at("2026-11-11T00:00:00Z"));
    expect(month).toMatchObject({ seconds: 7200, dollars: 0.002 });
    expect(Object.keys(month)).not.toContain("credits");
  });
});
