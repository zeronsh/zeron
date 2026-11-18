import { expect, test } from "vitest";
import { appUrl, linkable, fileUrl, httpsUrl, signInUrl, sitePath } from "./urls.ts";

test.each([
  ["/account", "/account"],
  ["/plugins?q=a#x", "/plugins?q=a#x"],
  ["/a/../b", "/b"],
  ["//evil.example", "/"],
  ["/\\evil.example", "/"],
  ["https://evil.example/", "/"],
  ["javascript:alert(1)", "/"],
  ["/\tjavascript", "/"],
  ["/.//evil.example", "/"],
  ["/a/..//evil.example", "/"],
  ["/%2e//evil.example", "/"],
  ["", "/"],
  [null, "/"],
])("a way back to %j stays on the site: %j", (input, out) => expect(sitePath(input)).toBe(out));

test("sign in comes back only to a path here", () => {
  expect(signInUrl("/account/plugins/x")).toBe("/auth/web/login?return_to=%2Faccount%2Fplugins%2Fx");
  expect(signInUrl("https://evil.example")).toBe("/auth/web/login?return_to=%2F");
});

test("the app's link names only a store id", () => {
  expect(appUrl("focus-timer")).toBe("zeron://open/plugin/focus-timer");
  // (The link can name a version: the contract's "The link can name a version".)
  expect(appUrl("focus-timer", "1.2.0-beta.1")).toBe("zeron://open/plugin/focus-timer?version=1.2.0-beta.1");
  // (As the app reads it: unescaped, a build's "+" as it is; anything else left out.)
  expect(appUrl("focus-timer", "1.2.0+build.7")).toBe("zeron://open/plugin/focus-timer?version=1.2.0+build.7");
  expect(appUrl("focus-timer", "1.2.0&x=1")).toBe("zeron://open/plugin/focus-timer");
  expect(appUrl("focus-timer", "1".repeat(33))).toBe("zeron://open/plugin/focus-timer");
  expect(linkable("1.0.0-rc.1+build.20261118.0123456789")).toBe(false);
  expect(linkable("1.0.0-rc.1+build.7")).toBe(true);
});

test("a file's address encodes each part", () => {
  expect(fileUrl("a", "1.0.0", "shots/a b.png")).toBe("/plugins/a/files/1.0.0/shots/a%20b.png");
});

test.each([
  ["https://example.com", "https://example.com/"],
  ["https://example.com/a%20b", "https://example.com/a%20b"],
  ["https://example.com/a b", null],
  ["http://example.com", null],
  ["javascript:alert(1)", null],
  ["https://user:pw@example.com", null],
  ["https:\\\\evil.example", null],
  ["  https://example.com", null],
  ["", null],
  [null, null],
])("an author's link %j is followed as %j", (input, out) => expect(httpsUrl(input)).toBe(out));
