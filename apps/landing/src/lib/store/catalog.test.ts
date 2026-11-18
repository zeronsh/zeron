import { describe, expect, test } from "vitest";
import { canonicalSearch, catalogHref, effectiveSort, isFront, pageHref, readCatalog } from "./catalog.ts";

const CATEGORIES = [{ id: "git", name: "Git" }, { id: "agents", name: "Agents" }];
const read = (search: string) => readCatalog(new URLSearchParams(search), CATEGORIES);

describe("reading the catalog's address", () => {
  test("the front page", () => {
    const state = read("");
    expect(state).toEqual({ q: "", category: null, keyword: null, sort: null, cursor: null });
    expect(effectiveSort(state)).toBe("popular");
    expect(isFront(state)).toBe(true);
  });

  test("words are tidied and bounded", () => {
    expect(read(`q=${encodeURIComponent("  focus \n\t timer ")}`).q).toBe("focus timer");
    expect(read(`q=${"a".repeat(300)}`).q).toHaveLength(100);
    expect(read("keyword=GitHub").keyword).toBe("github");
  });

  test("only known categories and sorts are kept", () => {
    expect(read("category=git").category).toBe("git");
    expect(read("category=nope").category).toBeNull();
    expect(read("category=%3Cscript%3E").category).toBeNull();
    expect(read("sort=new").sort).toBe("new");
    expect(read("sort=drop table").sort).toBeNull();
    // (Without the edge's list, any well-formed category is passed on for the edge to judge.)
    expect(readCatalog(new URLSearchParams("category=git"), null).category).toBe("git");
  });

  test("a cursor is opaque but bounded and url-safe", () => {
    expect(read("cursor=eyJrIjoxfQ").cursor).toBe("eyJrIjoxfQ");
    expect(read("cursor=a+b").cursor).toBeNull();
    expect(read("cursor=a/b").cursor).toBeNull();
    expect(read(`cursor=${"a".repeat(513)}`).cursor).toBeNull();
    expect(read("cursor=").cursor).toBeNull();
  });

  test("relevance needs words", () => {
    expect(effectiveSort({ q: "", sort: "relevance" })).toBe("popular");
    expect(effectiveSort({ q: "x", sort: null })).toBe("relevance");
    expect(effectiveSort({ q: "x", sort: "new" })).toBe("new");
  });
});

describe("the catalog's addresses", () => {
  test("defaults are left out, the rest in a fixed order", () => {
    expect(catalogHref({})).toBe("/plugins/all");
    expect(catalogHref({ sort: "popular" })).toBe("/plugins/all");
    expect(catalogHref({ q: "x", sort: "relevance" })).toBe("/plugins/all?q=x");
    expect(catalogHref({ cursor: "c", sort: "new", keyword: "k", category: "git", q: "a b" })).toBe("/plugins/all?q=a+b&category=git&keyword=k&sort=new&cursor=c");
  });

  test("an address that says more than it means has a plain one", () => {
    const params = new URLSearchParams("sort=popular&utm_source=x");
    expect(canonicalSearch(params, read("sort=popular&utm_source=x"))).toBe("/plugins/all");
    const clean = new URLSearchParams("q=x&sort=new");
    expect(canonicalSearch(clean, read("q=x&sort=new"))).toBeNull();
  });

  test("Previous and Next keep the search and carry the cursor", () => {
    const state = read("q=timer&category=git");
    expect(pageHref(state, "NEXT")).toBe("/plugins/all?q=timer&category=git&cursor=NEXT");
    expect(pageHref(state, undefined)).toBeNull();
  });
});
