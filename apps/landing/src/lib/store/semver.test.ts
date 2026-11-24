import { describe, expect, test } from "vitest";
import { satisfies } from "./semver.ts";

describe("a Zeron version in a plugin's range", () => {
  test.each([
    ["0.3.4", ">=0.3.0", true],
    ["0.2.97", ">=0.3.0", false],
    ["0.3.4", "^0.3.0", true],
    ["0.4.0", "^0.3.0", false],
    ["1.2.0", "^1.0.0", true],
    ["2.0.0", "^1.0.0", false],
    ["0.3.9", "~0.3.1", true],
    ["0.4.0", "~0.3.1", false],
    ["0.3.4", ">=0.3.0 <0.4.0", true],
    ["0.4.0", ">=0.3.0 <0.4.0", false],
    ["0.5.0", ">=0.3.0 <0.4.0 || >=0.5.0", true],
    ["0.3.4", "0.3.x", true],
    ["0.4.0", "0.3", false],
    ["0.3.4", "*", true],
    ["0.3.4", "", true],
    ["0.3.4", "0.3.0 - 0.3.9", true],
    ["0.3.0-beta.1", ">=0.3.0", false],
    ["0.3.4", "=0.3.4", true],
  ] as const)("%s in %s: %s", (version, range, expected) => expect(satisfies(version, range)).toBe(expected));
  test("a range or a version it can't read is no answer", () => {
    expect(satisfies("0.3.4", "latest")).toBeNull();
    expect(satisfies("nightly", ">=0.3.0")).toBeNull();
  });
});
