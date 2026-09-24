import { describe, expect, it } from "vitest";
import type { ContextUsage } from "@zeron/proto";
import { hasWindow, usageDetails, withSeparators } from "../src/components/context-usage";

function usage(fields: Partial<ContextUsage>): ContextUsage {
  return { tokens: null, window: null, ...fields };
}

/**
 * The 787f5831 context-UI parity port: `has_window` (harnesses that never
 * report a window get no indicator), `with_separators` (counts grouped by
 * thousands), and the four `details` cases rewritten with separators.
 */

describe("withSeparators (context_usage.rs:85-101)", () => {
  it("groupsByThousands", () => {
    expect(withSeparators(0)).toBe("0");
    expect(withSeparators(999)).toBe("999");
    expect(withSeparators(5417)).toBe("5,417");
    expect(withSeparators(1_048_576)).toBe("1,048,576");
  });
});

describe("hasWindow (context_usage.rs:105-111)", () => {
  it("indicatorNeedsAReportedWindow", () => {
    expect(hasWindow(null)).toBe(false);
    expect(hasWindow(usage({ tokens: 1200, window: null }))).toBe(false);
    expect(hasWindow(usage({ tokens: 1200, window: 0 }))).toBe(false);
    expect(hasWindow(usage({ tokens: null, window: 200_000 }))).toBe(true);
  });
});

describe("usageDetails (context_usage.rs:85-108)", () => {
  it("missingUsageIsDistinctFromZeroAndOverflow", () => {
    expect(usageDetails(null)).toContain("not reported");
    expect(usageDetails(usage({ tokens: 0, window: 1000 }))).toContain("0 / 1,000 tokens");
  });

  it("tokenCountsAreGroupedByThousands", () => {
    expect(usageDetails(usage({ tokens: 5417, window: 1_048_576 }))).toBe(
      "5,417 / 1,048,576 tokens\n1,043,159 tokens remaining",
    );
    expect(usageDetails(usage({ tokens: 1200, window: null }))).toBe(
      "1,200 tokens used\nContext limit not reported",
    );
    expect(usageDetails(usage({ tokens: null, window: 200_000 }))).toBe(
      "200,000 token capacity\nWaiting for context usage",
    );
  });
});
