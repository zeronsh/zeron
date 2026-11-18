import { describe, expect, test } from "vitest";
import { HUE_STEPS, hueClass, hueStep } from "./hue.ts";

describe("a plugin's hue, from its id", () => {
  test("is FNV-1a over the id's bytes, a step of 24", () => {
    // (Known FNV-1a 32-bit values: "" → 0x811c9dc5, "a" → 0xe40c292c.)
    expect(hueStep("")).toBe(0x811c9dc5 % HUE_STEPS);
    expect(hueStep("a")).toBe(0xe40c292c % HUE_STEPS);
    expect(hueClass("focus-timer")).toBe(`h${hueStep("focus-timer")}`);
  });

  test("spreads: ids a letter apart land apart, and every step is used", () => {
    const steps = new Set<number>();
    for (let i = 0; i < 2000; i++) steps.add(hueStep(`plugin-${i}`));
    expect(steps.size).toBe(HUE_STEPS);
    let same = 0;
    for (let i = 0; i < 500; i++) if (hueStep(`plugin-${i}`) === hueStep(`plugin-${i}s`)) same++;
    // (About one in 24 by chance; never most.)
    expect(same).toBeLessThan(60);
  });
});
