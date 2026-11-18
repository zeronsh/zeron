import { describe, expect, test } from "vitest";
import { ANDROID_APK } from "../data/releases.ts";
import { acceptVersion, buildUrl, detectTarget, targetUrl } from "./downloads.ts";

describe("detectTarget", () => {
  test.each([
    [{ platform: "MacIntel", userAgent: "Macintosh" }, "macos"],
    [{ platform: "Win32", userAgent: "Windows NT" }, "windows"],
    [{ platform: "Linux x86_64", userAgent: "Linux" }, "linux"],
    [{ platform: "Linux aarch64", userAgent: "Linux" }, "linux-arm"],
    [{ userAgentData: { platform: "macOS" } }, "macos"],
  ])("desktop %j", (client, build) => {
    expect(detectTarget(client)).toEqual({ kind: "desktop", build });
  });

  test.each([
    [{ platform: "Linux armv8l", userAgent: "Mozilla/5.0 (Linux; Android 16; Pixel 9)" }],
    [{ platform: "Linux", userAgent: "Android" }],
  ])("android %j", (client) => {
    expect(detectTarget(client)).toEqual({ kind: "android" });
  });

  test.each([[{ platform: "MacIntel", maxTouchPoints: 5 }], [{ platform: "iPhone" }], [{ userAgent: "Mozilla/5.0 (iPad)" }], [{}]])(
    "a phone, a tablet or an unknown system keeps the table: %j",
    (client) => {
      expect(detectTarget(client)).toEqual({ kind: "table" });
    },
  );
});

describe("urls", () => {
  test("a desktop build's file under the release", () => {
    expect(buildUrl("0.2.98", "windows")).toBe("https://zeron.sh/releases/zeron-0.2.98-windows-x86_64-setup.exe");
    expect(targetUrl({ kind: "desktop", build: "linux-arm" }, "1.0.0")).toBe("https://zeron.sh/releases/zeron-1.0.0-linux-aarch64.tar.gz");
  });
  test("android has one stable file; the table has none", () => {
    expect(targetUrl({ kind: "android" }, "0.2.98")).toBe(ANDROID_APK);
    expect(targetUrl({ kind: "table" }, "0.2.98")).toBeNull();
  });
});

describe("acceptVersion", () => {
  test("takes a newer or equal release", () => {
    expect(acceptVersion("0.3.7\n", "0.2.97")).toBe("0.3.7");
    expect(acceptVersion("0.2.97", "0.2.97")).toBe("0.2.97");
    expect(acceptVersion("1.0.0", "0.9.99")).toBe("1.0.0");
  });
  test.each(["<script>alert(1)</script>", "0.2.96", "", "0.3", "v0.3.7", "0.3.7-beta"])("refuses %j", (text) => {
    expect(acceptVersion(text, "0.2.97")).toBeNull();
  });
});
