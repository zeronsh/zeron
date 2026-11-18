import { describe, expect, test } from "vitest";
import { isPluginId, isPrerelease, isUsername, isVersion, packagePath, resolvePackagePath } from "./names.ts";

describe("plugin ids", () => {
  test.each(["focus-timer", "a1", "x".repeat(40), "git"])("%s is one", (id) => expect(isPluginId(id)).toBe(true));
  test.each(["", "a", "x".repeat(41), "Focus", "-a", "a-", "a_b", "a/b", "..", "a.b", "a b", "é1", null, 3])("%s isn't", (id) => expect(isPluginId(id)).toBe(false));
});

describe("versions", () => {
  test.each(["1.0.0", "0.3.0-beta.1", "1.2.3+build.5", "10.20.30-rc.1+x"])("%s is strict semver", (v) => expect(isVersion(v)).toBe(true));
  test.each(["1.0", "01.0.0", "1.0.0-", "v1.0.0", "1.0.0-01", "..", "1.0.0/../x", `1.0.0-${"a".repeat(70)}`])("%s isn't", (v) => expect(isVersion(v)).toBe(false));
  test("pre-releases", () => {
    expect(isPrerelease("1.0.0-beta.1")).toBe(true);
    expect(isPrerelease("1.0.0+build-5")).toBe(false);
  });
});

describe("usernames", () => {
  test.each(["ada", "mo-dev", "a_b_c"])("%s", (u) => expect(isUsername(u)).toBe(true));
  test.each(["ab", "Ada", "-ada", "a".repeat(21), "ad a"])("not %s", (u) => expect(isUsername(u)).toBe(false));
});

describe("paths in a package", () => {
  test.each([
    ["icon.svg", "icon.svg"],
    ["shots/a.png", "shots/a.png"],
    ["", null],
    ["/etc/passwd", null],
    ["a/../b", null],
    ["./a", null],
    ["a//b", null],
    ["a\\b", null],
    ["a\nb", null],
    ["x".repeat(513), null],
  ])("%j → %j", (input, out) => expect(packagePath(input)).toBe(out));

  test.each([
    ["./shots/a.png", "shots/a.png"],
    ["docs/../shots/a.png", "shots/a.png"],
    ["shots/a.png?raw=1#top", "shots/a.png"],
    ["shots/a%20b.png", "shots/a b.png"],
    ["../a.png", null],
    ["docs/../../a.png", null],
    ["/a.png", null],
    ["%2e%2e/a.png", null],
    ["shots%2F..%2F..%2Fa.png", null],
    ["%zz", null],
  ])("resolves %j → %j", (input, out) => expect(resolvePackagePath(input)).toBe(out));
});
