import { describe, expect, it } from "vitest";
import { EARNED_BADGES, V3_BETA, V3_BETA_CUTOFF_MS, earnsV3Beta, isEarnedBadge } from "../../src/earned-badges";

describe("V3 BETA's cutoff", () => {
  it("is 2026-11-05 01:00 PST, which is 09:00 UTC (daylight saving ended Nov 1)", () => {
    expect(V3_BETA_CUTOFF_MS).toBe(Date.parse("2026-11-05T09:00:00.000Z"));
    expect(V3_BETA_CUTOFF_MS).toBe(Date.parse("2026-11-05T01:00:00-08:00"));
    expect(new Date(V3_BETA_CUTOFF_MS).toLocaleString("en-US", { timeZone: "America/Los_Angeles", timeZoneName: "short" })).toBe(
      "11/5/2026, 1:00:00 AM PST"
    );
  });

  it("is earned by a first claim before it, and not at or after it", () => {
    expect(earnsV3Beta(Date.parse("2026-11-05T08:59:59Z"))).toBe(true);
    expect(earnsV3Beta(V3_BETA_CUTOFF_MS - 1)).toBe(true);
    expect(earnsV3Beta(Date.parse("2026-11-05T09:00:00Z"))).toBe(false);
    expect(earnsV3Beta(V3_BETA_CUTOFF_MS + 1)).toBe(false);
    expect(earnsV3Beta(null)).toBe(false);
  });

  it("reads exactly V3 BETA, and only earned badges are off limits to operators", () => {
    expect(V3_BETA).toMatchObject({ id: "v3-beta", name: "V3 BETA", icon: null });
    expect(EARNED_BADGES.map((b) => b.id)).toEqual(["v3-beta"]);
    expect(isEarnedBadge("v3-beta")).toBe(true);
    expect(isEarnedBadge("cloud-tester")).toBe(false);
  });
});
