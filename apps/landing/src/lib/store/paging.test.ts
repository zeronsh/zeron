import { describe, expect, test } from "vitest";
import { pagerOf, placeOf } from "./paging.ts";

const at = (query: string) => placeOf(new URL(`https://zeron.sh/admin/plugins${query}`));

describe("an admin list's pages", () => {
  test("the first page: no Previous, Next carries page 2", () => {
    const pager = pagerOf("/admin/plugins", {}, at(""), { next_cursor: "b", total: 128 }, 50);
    expect(pager).toEqual({ previous: null, next: "/admin/plugins?cursor=b&page=2", where: "1–50 of 128" });
  });

  test("the second page goes back to the list's own address; the last says how far it is", () => {
    expect(pagerOf("/admin/plugins", { status: "rejected" }, at("?cursor=b&page=2"), { next_cursor: "c", prev_cursor: "a", total: 128 }, 50)).toEqual({
      previous: "/admin/plugins?status=rejected",
      next: "/admin/plugins?status=rejected&cursor=c&page=3",
      where: "51–100 of 128",
    });
    expect(pagerOf("/admin/plugins", {}, at("?cursor=c&page=3"), { prev_cursor: "b", total: 128 }, 28)).toEqual({ previous: "/admin/plugins?cursor=b&page=2", next: null, where: "101–128 of 128" });
  });

  test("a cursor without its page (or a page that can't be): where it is in the list goes unsaid", () => {
    expect(at("?cursor=b")).toEqual({ cursor: "b", page: null });
    expect(at("?cursor=b&page=1")).toEqual({ cursor: "b", page: null });
    expect(at("?cursor=<b>&page=2")).toEqual({ cursor: undefined, page: 1 });
    expect(pagerOf("/admin/plugins", {}, at("?cursor=b"), { prev_cursor: "a", next_cursor: "c", total: 128 }, 50).where).toBe("of 128");
  });
});
