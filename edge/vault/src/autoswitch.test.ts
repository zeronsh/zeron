import { describe, expect, it } from "vitest";
import { pickAccount, readAgainAt, retryReadAt } from "./autoswitch";

// Durations are spelled out, not imported: they are the read budget
// (autoswitch.ts), and changing one should fail here.
const MINUTE = 60_000;
const now = 1_000_000;

describe("readAgainAt", () => {
  it("reads a fuller account sooner: 5 minutes empty, 3 from the threshold on", () => {
    expect(readAgainAt({ percent: 0 }, 90, now)).toBe(now + 5 * MINUTE);
    expect(readAgainAt({ percent: 45 }, 90, now)).toBe(now + 4 * MINUTE);
    expect(readAgainAt({ percent: 90 }, 90, now)).toBe(now + 3 * MINUTE);
    expect(readAgainAt({ percent: 97 }, 90, now)).toBe(now + 3 * MINUTE);
  });

  it("an exhausted account every 10 minutes, or at its reset if sooner", () => {
    expect(readAgainAt({ percent: 100, resetsAt: now + 3 * 60 * MINUTE }, 90, now)).toBe(now + 10 * MINUTE);
    expect(readAgainAt({ percent: 100, resetsAt: now + 7 * MINUTE }, 90, now)).toBe(now + 7 * MINUTE);
  });

  it("never past a window's reset, never within 3 minutes: 20 reads an hour at most", () => {
    expect(readAgainAt({ percent: 0, resetsAt: now + 4 * MINUTE }, 90, now)).toBe(now + 4 * MINUTE);
    expect(readAgainAt({ percent: 0, resetsAt: now + MINUTE }, 90, now)).toBe(now + 3 * MINUTE);
    expect(readAgainAt({ percent: 0, resetsAt: now - MINUTE }, 90, now)).toBe(now + 3 * MINUTE);
  });
});

describe("retryReadAt", () => {
  it("waits out a Retry-After, then 5 more minutes", () => {
    expect(retryReadAt(now)).toBe(now + 5 * MINUTE);
    expect(retryReadAt(now, 0)).toBe(now + 5 * MINUTE);
    expect(retryReadAt(now, 20 * MINUTE)).toBe(now + 25 * MINUTE);
  });

  it("caps a Retry-After at an hour", () => {
    expect(retryReadAt(now, 1e12)).toBe(now + 65 * MINUTE);
  });
});

describe("pickAccount", () => {
  it("picks the readable account with the most room", () => {
    expect(
      pickAccount(95, [
        { slot: "a", percent: 40 },
        { slot: "b", percent: undefined },
        { slot: "c", percent: 20 }
      ])
    ).toEqual({ slot: "c", percent: 20 });
  });

  it("only for 5 more points of room", () => {
    expect(pickAccount(95, [{ slot: "a", percent: 90 }])).toEqual({ slot: "a", percent: 90 });
    expect(pickAccount(95, [{ slot: "a", percent: 92 }])).toBeUndefined();
  });

  it("never an account it couldn't read", () => {
    expect(pickAccount(100, [{ slot: "a", percent: undefined }])).toBeUndefined();
    expect(pickAccount(100, [])).toBeUndefined();
  });
});
