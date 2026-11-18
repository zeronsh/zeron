import { describe, expect, test } from "vitest";
import { canonicalPath, isStaffPath, isStorePath } from "./paths.ts";
import { fileUrl } from "./urls.ts";

describe("an address in its one canonical form", () => {
  test("encoding of a letter, a digit, - _ ~, once or twice over, is written plainly", () => {
    expect(canonicalPath("/%61dmin/plugins")).toBe("/admin/plugins");
    expect(canonicalPath("/%2561dmin/plugins")).toBe("/admin/plugins");
    expect(canonicalPath("/%252561dmin")).toBe("/admin");
    expect(canonicalPath("/admin//plugins")).toBe("/admin/plugins");
    expect(canonicalPath("/plugins/focus%2Dtimer")).toBe("/plugins/focus-timer");
  });

  test("what changes a path's shape stays as sent: an encoded dot is a name, an encoded slash part of one", () => {
    expect(canonicalPath("/plugins/x/files/1.0.0/%2e%2e/secret")).toBe("/plugins/x/files/1.0.0/%2e%2e/secret");
    expect(canonicalPath("/plugins/x/files/1.0.0/..%2f..%2fsecret.png")).toBe("/plugins/x/files/1.0.0/..%2f..%2fsecret.png");
    expect(canonicalPath("/admin%2Fplugins")).toBe("/admin%2Fplugins");
  });

  test("the site's own addresses are canonical already, whatever a file is named", () => {
    // (A file named as if encoded, "%61.md", never was reachable: the router reads its address twice decoded, "a.md".)
    for (const path of ["docs/<b>notes</b> #1 100%.md", "shots/a b#1 100%.svg", "a%20b.txt", "ünïcode/ファイル.md"]) {
      const url = fileUrl("xss-probe", "1.1.0-beta.1", path);
      expect(canonicalPath(url), path).toBe(url);
    }
  });

  test("the staff's paths and the store's", () => {
    expect(isStaffPath("/admin")).toBe(true);
    expect(isStaffPath("/admin/x")).toBe(true);
    expect(isStaffPath("/administrator")).toBe(false);
    expect(isStorePath("/plugins/all")).toBe(true);
    expect(isStorePath("/auth/web/login")).toBe(true);
    expect(isStorePath("/privacy")).toBe(false);
    expect(isStorePath("/")).toBe(false);
  });
});
