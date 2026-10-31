import { describe, expect, it } from "vitest";
import { sizeMultiplier } from "./metering";
import { machineName, parseMachine } from "./policy";

describe("machine sizes", () => {
  it("knows every provider size, xlarge included", () => {
    expect(machineName("xlarge")).toBe("xlarge");
    expect(machineName("huge")).toBeUndefined();
    expect(parseMachine("xlarge")).toBe("xlarge");
    expect(parseMachine(undefined)).toBe("default");
  });

  it("estimates a bigger machine's spend at its provider multiplier", () => {
    expect(sizeMultiplier("xlarge")).toBe(5.56);
    expect(sizeMultiplier("small")).toBe(0.5);
    expect(sizeMultiplier(undefined)).toBe(1);
    expect(sizeMultiplier("unknown")).toBe(1);
  });
});
