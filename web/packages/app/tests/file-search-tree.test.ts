import { describe, expect, it } from "vitest";
import type { WorkspaceEntryKind, WorkspaceFileSearchMatch } from "@zeron/proto";
import {
  buildSearchTree,
  isSearchNodeExpanded,
  sortSearchPaths,
  toggleSearchNode,
} from "../src/lib/file-search-tree";

/**
 * Ports of `crates/ui/src/files/search.rs`'s `SearchTreeModel` tests — the
 * synthetic ancestor tree keyed by path components, its ordering, and the
 * own-subtree-only collapse.
 */

function searchMatch(path: string, kind: WorkspaceEntryKind, score: number): WorkspaceFileSearchMatch {
  return { path, name: path.split("/").pop() ?? path, kind, score };
}

describe("buildSearchTree (search.rs)", () => {
  it("search_tree_groups_matches_under_real_ancestors", () => {
    const tree = buildSearchTree([
      searchMatch("archive-renamed/docs/docs-1.txt", "file", 100),
      searchMatch("archive-renamed/logs/file-2.txt", "file", 90),
      searchMatch("README.md", "file", 80),
    ]);

    expect(tree.rows.map((row) => row.path)).toEqual([
      "archive-renamed",
      "archive-renamed/docs",
      "archive-renamed/docs/docs-1.txt",
      "archive-renamed/logs",
      "archive-renamed/logs/file-2.txt",
      "README.md",
    ]);
    expect(tree.rows.map((row) => row.depth)).toEqual([0, 1, 2, 1, 2, 0]);
    // Ancestors are directories even though they are not matches.
    expect(tree.rows[0]?.kind).toBe("directory");
  });

  it("groupsByPathComponentsNotNamePrefixes", () => {
    const tree = buildSearchTree([searchMatch("THIRD_PARTY_NOTICES/THIRD_PARTY_NOTICES.md", "file", 100)]);

    expect(tree.rows.map((row) => row.path)).toEqual([
      "THIRD_PARTY_NOTICES",
      "THIRD_PARTY_NOTICES/THIRD_PARTY_NOTICES.md",
    ]);
  });

  it("search_tree_collapses_and_restores_matching_branches", () => {
    const tree = buildSearchTree([
      searchMatch("src/main.rs", "file", 100),
      searchMatch("src/lib.rs", "file", 90),
    ]);

    const collapsed = toggleSearchNode(tree, "src");
    expect(collapsed).not.toBeNull();
    expect(collapsed!.rows).toHaveLength(1);
    expect(isSearchNodeExpanded(collapsed!, "src")).toBe(false);

    const restored = toggleSearchNode(collapsed!, "src");
    expect(restored).not.toBeNull();
    expect(restored!.rows).toHaveLength(3);
    expect(isSearchNodeExpanded(restored!, "src")).toBe(true);
  });

  it("toggling a node collapses only its own subtree", () => {
    const tree = buildSearchTree([
      searchMatch("a/deep/one.txt", "file", 100),
      searchMatch("b/other.txt", "file", 100),
    ]);

    const collapsed = toggleSearchNode(tree, "a")!;
    expect(collapsed.rows.map((row) => row.path)).toEqual(["a", "b", "b/other.txt"]);
    // `a/deep` vanished with its parent, `b` stayed expanded.
    expect(collapsed.nodes.has("a/deep")).toBe(true);
  });

  it("a matching directory keeps its match kind and name", () => {
    const tree = buildSearchTree([searchMatch("src", "directory", 70)]);
    expect(tree.rows[0]).toMatchObject({ path: "src", kind: "directory", name: "src", hasChildren: false });
  });

  it("sorts by descendant-propagated best score, then directories, then name, then path", () => {
    const matches = [
      searchMatch("zeta/low.txt", "file", 10),
      searchMatch("zeta", "directory", 5),
      searchMatch("Beta/mid.txt", "file", 50),
      searchMatch("alpha/hi.txt", "file", 100),
      searchMatch("alpha/hi2.txt", "file", 100),
    ];
    const tree = buildSearchTree(matches);
    // `zeta` carries its subtree's best (10), `alpha`'s 100 leads; equal
    // scores break on the case-insensitive name then the raw path.
    expect(tree.roots).toEqual(["alpha", "Beta", "zeta"]);
    expect(tree.rows.map((row) => row.path)).toEqual([
      "alpha",
      "alpha/hi.txt",
      "alpha/hi2.txt",
      "Beta",
      "Beta/mid.txt",
      "zeta",
      "zeta/low.txt",
    ]);
  });

  it("sortSearchPaths orders directories before files at equal score", () => {
    const tree = buildSearchTree([
      searchMatch("dir", "directory", 50),
      searchMatch("file.txt", "file", 50),
    ]);
    const paths = [...tree.roots];
    sortSearchPaths(paths, tree.nodes);
    expect(paths).toEqual(["dir", "file.txt"]);
  });
});
