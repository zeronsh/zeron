import { expect, test } from "vitest";
import { appUrl, fileUrl, signInUrl, sitePath } from "./urls.ts";

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
});

test("a file's address encodes each part", () => {
  expect(fileUrl("a", "1.0.0", "shots/a b.png")).toBe("/plugins/a/files/1.0.0/shots/a%20b.png");
});
