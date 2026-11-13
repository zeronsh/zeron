import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { creditRate, isDayKey, NO_CUTOVER } from "../../src/cloud/metering";
import { CREDIT_PACK, PLANS } from "../../src/cloud/stripe";

/** What the sandbox provider bills: billable seconds per dollar. */
const BILLABLE_SECONDS_PER_DOLLAR = 100_000;

const wrangler = readFileSync(new URL("../../wrangler.jsonc", import.meta.url), "utf8");
/** A var's value in each environment (production, staging), comments aside. */
const values = (name: string) =>
  [...wrangler.matchAll(new RegExp(`^\\s*"${name}":\\s*"([^"]*)"`, "gm"))].map((match) => match[1]!);

const rates = values("CLOUD_CREDIT_SECONDS");
const cutovers = values("CLOUD_CREDIT_RATE_FROM");

const dollars = (credits: number, secondsPerCredit: number) =>
  (credits * secondsPerCredit) / BILLABLE_SECONDS_PER_DOLLAR;

describe("credit pricing", () => {
  it("names the rate and the cutover in production and staging", () => {
    expect(rates).toHaveLength(2);
    expect(cutovers).toHaveLength(2);
  });

  it.each(PLANS.map((plan) => [plan.name, plan] as const))("%s costs less at full use than its price", (_name, plan) => {
    for (const seconds of rates) {
      expect(dollars(plan.monthlyCredits, Number(seconds))).toBeLessThan(plan.priceCents / 100);
    }
  });

  it("a credit pack costs less than its price", () => {
    for (const seconds of rates) {
      expect(dollars(CREDIT_PACK.credits, Number(seconds))).toBeLessThan(CREDIT_PACK.priceCents / 100);
    }
  });

  it("has a real day or the placeholder for a cutover, and the placeholder changes nothing", () => {
    for (const from of cutovers) {
      if (!isDayKey(from)) {
        expect(from).toBe("SET-AT-DEPLOY");
        expect(creditRate(rates[0], from)).toEqual(NO_CUTOVER);
      }
    }
  });
});
