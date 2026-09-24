import { describe, expect, it } from "vitest";
import {
  classifyKey,
  filterAndSort,
  filterIndices,
  matchRank,
  menuStep,
} from "../src/lib/picker-search";

/**
 * Ports of the desktop's `popover.rs` test module: `menu_step_wraps_and_enters`
 * (`:1587`), `filter_ranks_prefix_before_substring` (`:1601`),
 * `match_rank_kinds` (`:1615`), and `key_classification` (`:1623`) — plus
 * coverage of `filterAndSort`, the wrapper ticket 10 keeps calling.
 */

describe("menuStep (popover.rs:1587)", () => {
  it("entering an empty menu stays out", () => {
    expect(menuStep(null, 0, 1)).toBe(null);
    expect(menuStep(3, 0, 1)).toBe(null);
  });

  it("entering from nothing lands on the matching edge", () => {
    expect(menuStep(null, 3, 1)).toBe(0);
    expect(menuStep(null, 3, -1)).toBe(2);
  });

  it("stepping wraps both ways", () => {
    expect(menuStep(2, 3, 1)).toBe(0);
    expect(menuStep(0, 3, -1)).toBe(2);
    expect(menuStep(1, 3, 1)).toBe(2);
  });
});

describe("filterIndices (popover.rs:1601)", () => {
  const labels = ["main", "feature/main-sync", "master", "dev"];

  it("prefix matches come before the substring match", () => {
    expect(filterIndices("ma", labels)).toEqual([0, 2, 1]);
  });

  it("is case-insensitive", () => {
    expect(filterIndices("MA", labels)).toEqual([0, 2, 1]);
  });

  it("no matches yields empty", () => {
    expect(filterIndices("zzz", labels)).toEqual([]);
  });

  it("empty and whitespace queries keep input order", () => {
    expect(filterIndices("", labels)).toEqual([0, 1, 2, 3]);
    expect(filterIndices("   ", labels)).toEqual([0, 1, 2, 3]);
  });
});

describe("matchRank (popover.rs:1615)", () => {
  it("ranks prefix 0, substring 1, miss null", () => {
    expect(matchRank("re", "release")).toBe(0);
    expect(matchRank("lease", "release")).toBe(1);
    expect(matchRank("x", "release")).toBe(null);
    expect(matchRank("", "anything")).toBe(1);
  });

  it("all substring hits share rank 1 — position never orders", () => {
    expect(matchRank("ma", "feature/main-sync")).toBe(1);
    expect(matchRank("a", "banana")).toBe(1);
  });
});

describe("filterAndSort", () => {
  it("orders prefix matches before substring matches, stable on ties", () => {
    const sorted = filterAndSort(
      ["main", "feature/main-sync", "master", "dev"],
      (item) => item,
      "ma",
    );
    expect(sorted).toEqual(["main", "master", "feature/main-sync"]);
  });

  it("returns the full list for a whitespace query", () => {
    expect(filterAndSort(["a", "b", "c"], (item) => item, "  ")).toEqual(["a", "b", "c"]);
  });

  it("is case-insensitive", () => {
    const sorted = filterAndSort(["Claude-Code", "claude-code"], (item) => item, "CLAUDE");
    expect(sorted).toEqual(["Claude-Code", "claude-code"]);
  });

  it("returns an empty array when nothing matches", () => {
    expect(filterAndSort(["alpha", "beta"], (item) => item, "zz")).toEqual([]);
  });
});

describe("classifyKey (popover.rs:1623)", () => {
  it("accepts both the DOM and gpui key spellings", () => {
    expect(classifyKey("ArrowUp", false, false)).toBe("up");
    expect(classifyKey("ArrowDown", false, false)).toBe("down");
    expect(classifyKey("up", false, false)).toBe("up");
    expect(classifyKey("down", false, false)).toBe("down");
    expect(classifyKey("Enter", false, false)).toBe("enter");
    expect(classifyKey("enter", false, false)).toBe("enter");
    expect(classifyKey("Escape", false, false)).toBe("escape");
    expect(classifyKey("Backspace", false, false)).toBe("backspace");
    expect(classifyKey("q", false, false)).toBe("other");
  });

  it("plain n/p are other — the modifier must be ctrl, not cmd", () => {
    expect(classifyKey("n", false, false)).toBe("other");
    expect(classifyKey("p", true, false)).toBe("other");
  });

  it("ctrl n/p mirror down/up (readline/emacs motion)", () => {
    expect(classifyKey("n", false, true)).toBe("down");
    expect(classifyKey("p", false, true)).toBe("up");
  });

  it("enter with cmd or ctrl is mod-enter", () => {
    expect(classifyKey("enter", true, false)).toBe("mod-enter");
    expect(classifyKey("enter", false, true)).toBe("mod-enter");
  });
});
