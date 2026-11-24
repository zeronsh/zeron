import { describe, expect, test } from "vitest";
import { age, ago, bytes, compatibility, installs } from "./format.ts";

test("installs", () => {
  expect(installs(0)).toBe("No installs yet");
  expect(installs(1)).toBe("1 install");
  expect(installs(999)).toBe("999 installs");
  expect(installs(12_840)).toBe("12.8k installs");
});

test("compatibility in words", () => {
  expect(compatibility(">=0.4.0")).toBe("Zeron 0.4.0 or later");
  expect(compatibility("^0.3.0")).toBe("Zeron 0.3.0 or later, before 0.4.0");
  expect(compatibility("^1.2.0")).toBe("Zeron 1.2.0 or later, before 2.0.0");
  expect(compatibility(">=0.3.0 <0.5.0")).toBe("Zeron 0.3.0 or later, before 0.5.0");
  expect(compatibility("*")).toBe("Any version of Zeron");
  expect(compatibility(undefined)).toBe("Any version of Zeron");
  expect(compatibility("~0.3.1 || >=1.0.0")).toBe("Zeron ~0.3.1 || >=1.0.0");
});

test("times and sizes", () => {
  const now = Date.UTC(2026, 10, 18);
  expect(ago(now - 3 * 86_400_000, now)).toBe("3 days ago");
  expect(ago(now - 10_000, now)).toBe("just now");
  expect(age(now - 400 * 86_400_000, now)).toBe("13 months old");
  expect(bytes(512)).toBe("512 B");
  expect(bytes(9_700)).toBe("9.5 KB");
});

describe("sinceInSentence", () => {
  const now = Date.UTC(2026, 10, 24, 12);
  test("its own words in lower case, inside a sentence; a day or a count as it was", async () => {
    const { since, sinceInSentence } = await import("./format.ts");
    expect(sinceInSentence(now - 30_000, now, true)).toBe("just now");
    expect(sinceInSentence(now - 2 * 3_600_000, now, true)).toBe("2 hours ago");
    expect(sinceInSentence(now - 3_600_000, now)).toBe("today");
    expect(sinceInSentence(now - 86_400_000, now)).toBe("yesterday");
    expect(sinceInSentence(now - 3 * 86_400_000, now)).toBe("3 days ago");
    expect(sinceInSentence(now - 30 * 86_400_000, now)).toBe(since(now - 30 * 86_400_000, now));
    // (At a line's start, as before.)
    expect(since(now - 86_400_000, now)).toBe("Yesterday");
  });
});
