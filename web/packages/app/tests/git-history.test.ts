import { describe, expect, it } from "vitest";
import type { GitHistoryCommit, GitHistoryRef, GitHistoryRefKind } from "@zeron/proto";
import {
  branchRefKey,
  collapseBranchRuns,
  compactCommitsToVisible,
  compactGeometry,
  decodeHistoryAvatar,
  estimatedRefBadgeWidth,
  fittedGeometry,
  formatDate,
  gitHistoryMatches,
  graphColor,
  historyAuthorInitial,
  historyAuthorName,
  historyColumnDropIndex,
  historyColumnWidth,
  historyListSplice,
  historyTransitionRows,
  hoveredGraphPath,
  interpolateGraphGeometry,
  laneX,
  naturalGeometry,
  reorderedHistoryColumns,
  refAreaWidth,
  refDescription,
  resolveHistoryScrollAnchor,
  resizedHistoryColumnWidths,
  responsiveGraphGeometry,
  shouldUseCompactGraph,
  stabilizedGraphGeometry,
  visibleHistoryColumns,
  visibleRefCount,
  layoutGraph,
  type GraphRow,
  type HistoryColumnWidths,
} from "../src/lib/git-history";

/**
 * The History pane's pure-logic port, mirroring the desktop's test names
 * (`crates/ui/src/history.rs:4498+` and `crates/engine/src/repos.rs`). Each
 * assertion matches its Rust counterpart; the avatar test asserts the mime
 * sniff (§3's web adaptation) instead of GPUI's `ImageFormat`.
 */

function commit(sha: string, parents: readonly string[]): GitHistoryCommit {
  return {
    sha,
    parentShas: [...parents],
    subject: sha,
    authorName: "Test",
    authorEmail: "test@example.com",
    authoredAt: "2026-08-12T12:00:00Z",
    refs: [],
  };
}

function withBranch(
  source: GitHistoryCommit,
  kind: GitHistoryRefKind,
  label: string,
): GitHistoryCommit {
  return { ...source, refs: [{ kind, label }] };
}

const ref = (kind: GitHistoryRefKind, label: string): GitHistoryRef => ({ kind, label });

describe("git-history layout", () => {
  it("graph splits and rejoins merge lanes", () => {
    const commits = [
      commit("merge", ["main", "feature"]),
      commit("main", ["base"]),
      commit("feature", ["base"]),
      commit("base", []),
    ];
    const graph = layoutGraph(commits, "merge");
    expect(graph.rows).toHaveLength(commits.length);
    expect(graph.maxLaneCount).toBeGreaterThanOrEqual(2);
    expect(graph.rows[0]!.isHead).toBe(true);
    expect(graph.rows[0]!.segments).toHaveLength(2);
    expect(graph.rows[3]!.segments).toHaveLength(2);
    expect(graph.rows[3]!.nodeLane).toBe(0);
  });

  it("appending older commits preserves the loaded prefix layout", () => {
    const prefix = [commit("tip", ["parent"]), commit("parent", ["root"])];
    const before = layoutGraph(prefix, "tip");
    const all = [...prefix, commit("root", [])];
    const after = layoutGraph(all, "tip");
    expect(after.rows.slice(0, before.rows.length)).toEqual(before.rows);
  });
});

describe("git-history collapse and compaction", () => {
  it("collapsing a branch contracts linear parents and keeps junctions", () => {
    const commits = [
      withBranch(commit("feature", ["middle"]), "branch", "feature"),
      commit("middle", ["base"]),
      withBranch(commit("main", ["base"]), "branch", "main"),
      commit("base", ["root"]),
      commit("root", []),
    ];
    const { visible, hiddenCounts } = collapseBranchRuns(commits, new Set(["local:feature"]), "main");
    expect(visible.map((entry) => entry.sha)).toEqual(["feature", "main", "base", "root"]);
    expect(visible[0]!.parentShas).toEqual(["base"]);
    expect(hiddenCounts.get("local:feature")).toBe(1);
  });

  it("search compaction connects matches across hidden commits", () => {
    const commits = [
      commit("tip", ["middle"]),
      commit("middle", ["base"]),
      commit("base", ["root"]),
      commit("root", []),
    ];
    const compact = compactCommitsToVisible(commits, new Set(["tip", "base"]));
    expect(compact.map((entry) => entry.sha)).toEqual(["tip", "base"]);
    expect(compact[0]!.parentShas).toEqual(["base"]);
    expect(compact[1]!.parentShas).toEqual([]);
  });

  it("search compaction handles a twenty thousand commit gap", () => {
    const depth = 20_000;
    const commits: GitHistoryCommit[] = [];
    for (let index = depth - 1; index >= 0; index -= 1) {
      const sha = `c${String(index).padStart(5, "0")}`;
      const parent = `c${String(index - 1).padStart(5, "0")}`;
      commits.push(index === 0 ? commit(sha, []) : commit(sha, [parent]));
    }
    const newest = `c${String(depth - 1).padStart(5, "0")}`;
    const oldest = "c00000";
    const compact = compactCommitsToVisible(commits, new Set([newest, oldest]));
    expect(compact).toHaveLength(2);
    expect(compact[0]!.sha).toBe(newest);
    expect(compact[0]!.parentShas).toEqual([oldest]);
    expect(compact[1]!.sha).toBe(oldest);
    expect(compact[1]!.parentShas).toEqual([]);
  });

  it("branch fold keys keep local and remote identity", () => {
    expect(branchRefKey(ref("branch", "main"))).toBe("local:main");
    expect(branchRefKey(ref("remote", "origin/main"))).toBe("remote:origin/main");
    expect(branchRefKey(ref("tag", "v1"))).toBeNull();
  });
});

describe("git-history search matching", () => {
  it("history search matches fuzzy subject terms and sha prefix", () => {
    const candidate = { ...commit("a1b2c3d4", []), subject: "Polish the history graph" };
    expect(gitHistoryMatches("plsh grph", candidate)).toBe(true);
    expect(gitHistoryMatches("A1B2", candidate)).toBe(true);
    expect(gitHistoryMatches("terminal", candidate)).toBe(false);
    expect(gitHistoryMatches("   ", candidate)).toBe(true);
  });

  it("git history matches unicode case insensitively", () => {
    const candidate = { ...commit("a1b2c3d4", []), subject: "RÉPARER la recherche" };
    expect(gitHistoryMatches("réparer", candidate)).toBe(true);
    expect(gitHistoryMatches("RÉPARER", candidate)).toBe(true);
  });

  it("history search keeps unicode engine result visible", () => {
    // Simulate the page the engine returned for this query: the UI's local
    // re-filter must not discard a result the shared matcher accepted.
    const candidate = { ...commit("a1b2c3d4", []), subject: "RÉPARER la recherche" };
    const query = "réparer";
    const enginePage = [candidate];
    const locallyVisible = enginePage.filter((entry) => gitHistoryMatches(query, entry));
    expect(locallyVisible.map((entry) => entry.subject)).toEqual(["RÉPARER la recherche"]);
  });
});

describe("git-history transitions", () => {
  it("transition rows fold old commits beside their stable anchor", () => {
    const old = [commit("tip", ["one"]), commit("one", ["two"]), commit("two", ["base"]), commit("base", [])];
    const target = [commit("tip", ["base"]), commit("base", [])];
    const { commits, transitions } = historyTransitionRows(old, target);
    expect(commits.map((entry) => entry.sha)).toEqual(["tip", "one", "two", "base"]);
    expect(transitions).toEqual(["stable", "exiting", "exiting", "stable"]);
  });

  it("transition rows expand new commits in their final order", () => {
    const old = [commit("tip", ["base"]), commit("base", [])];
    const target = [commit("tip", ["one"]), commit("one", ["two"]), commit("two", ["base"]), commit("base", [])];
    const { commits, transitions } = historyTransitionRows(old, target);
    expect(commits.map((entry) => entry.sha)).toEqual(["tip", "one", "two", "base"]);
    expect(transitions).toEqual(["stable", "entering", "entering", "stable"]);
  });

  it("list splice preserves the unchanged prefix around a fold", () => {
    const old = [commit("tip", ["one"]), commit("one", ["two"]), commit("two", ["base"]), commit("base", [])];
    const target = [commit("tip", ["base"]), commit("base", [])];
    expect(historyListSplice(old, true, target, true)).toEqual({ start: 1, end: 3, count: 0 });
    expect(historyListSplice(old, false, old, false)).toBeNull();
  });

  it("removed scroll anchor moves to the next surviving commit", () => {
    const old = [commit("tip", ["one"]), commit("one", ["two"]), commit("two", ["base"]), commit("base", [])];
    const target = [commit("tip", ["base"]), commit("base", [])];
    const resolved = resolveHistoryScrollAnchor({ sha: "one", offsetInItem: 12 }, old, target);
    expect(resolved).toEqual({ sha: "base", offsetInItem: 0 });
    // An anchor that survives is kept verbatim, sub-item offset included.
    expect(resolveHistoryScrollAnchor({ sha: "tip", offsetInItem: 7 }, old, target)).toEqual({
      sha: "tip",
      offsetInItem: 7,
    });
  });
});

describe("git-history geometry", () => {
  it("responsive graph keeps natural spacing when it fits", () => {
    expect(responsiveGraphGeometry(8, 900, 240)).toEqual(naturalGeometry(8));
  });

  it("responsive graph compresses lanes to preserve commit space", () => {
    const geometry = responsiveGraphGeometry(20, 400, 240);
    expect(geometry.width).toBe(80);
    expect(geometry.laneSpacing).toBeLessThan(12);
    expect(laneX(geometry, 0)).toBe(laneX(naturalGeometry(20), 0));
    expect(laneX(geometry, 19)).toBeLessThan(laneX(naturalGeometry(20), 19));
  });

  it("narrow commit space switches to a compact rail with hysteresis", () => {
    const target = responsiveGraphGeometry(10, 500, 240);
    expect(shouldUseCompactGraph(target, naturalGeometry(10), 500, 240)).toBe(true);

    const compact = compactGeometry(10);
    const stillNarrow = responsiveGraphGeometry(10, 550, 240);
    expect(shouldUseCompactGraph(stillNarrow, compact, 550, 240)).toBe(true);

    const wideAgain = responsiveGraphGeometry(10, 570, 240);
    expect(shouldUseCompactGraph(wideAgain, compact, 570, 240)).toBe(false);
  });

  it("responsive graph geometry ignores sub step resize jitter", () => {
    const previous = fittedGeometry(20, 80);
    const stable = stabilizedGraphGeometry(fittedGeometry(20, 81.9), previous, false);
    expect(stable.width).toBe(previous.width);
    expect(stable.laneSpacing).toBe(previous.laneSpacing);

    const next = stabilizedGraphGeometry(fittedGeometry(20, 82.1), stable, false);
    expect(next.width).toBe(82);
    expect(next.laneSpacing).not.toBe(stable.laneSpacing);
  });

  it("graph geometry morph converges lanes before entering the compact rail", () => {
    const full = naturalGeometry(8);
    const compact = compactGeometry(8);
    const halfway = interpolateGraphGeometry(full, compact, 0.5);
    expect(halfway.compact).toBe(false);
    expect(halfway.width).toBeLessThan(full.width);
    expect(halfway.width).toBeGreaterThan(compact.width);
    expect(halfway.laneSpacing).toBeLessThan(full.laneSpacing);
    expect(halfway.laneSpacing).toBeGreaterThan(compact.laneSpacing);

    const settled = interpolateGraphGeometry(full, compact, 1.0);
    expect(settled.compact).toBe(true);
    expect(settled.width).toBe(compact.width);
    expect(settled.laneSpacing).toBe(0);
  });

  it("graph geometry morph expands the rail from its compact start", () => {
    const compact = compactGeometry(8);
    const full = naturalGeometry(8);
    expect(interpolateGraphGeometry(compact, full, 0).compact).toBe(true);
    const halfway = interpolateGraphGeometry(compact, full, 0.5);
    expect(halfway.compact).toBe(false);
    expect(halfway.laneSpacing).toBeGreaterThan(0);
    expect(halfway.laneSpacing).toBeLessThan(full.laneSpacing);
  });
});

describe("git-history hover hit testing", () => {
  const row = (
    sha: string,
    nodeLane: number,
    nodeColorId: number,
    segments: GraphRow["segments"],
  ): GraphRow => ({ sha, nodeLane, nodeColorId, segments, isHead: false });

  it("graph hover detects vertical and curved paths", () => {
    const geometry = naturalGeometry(2);
    const vertical = row("vertical", 0, 1, [
      { fromLane: 1, toLane: 1, colorId: 7, shape: "through" },
    ]);
    expect(hoveredGraphPath(vertical, laneX(geometry, 1), 5, geometry)).toBe(7);

    const middle = 36 / 2;
    const curveX = cubicAt(laneX(geometry, 0), laneX(geometry, 0), laneX(geometry, 1), laneX(geometry, 1), 0.5);
    const curveY = cubicAt(middle, middle * 1.45, middle * 1.45, 36 + 0.75, 0.5);
    const curved = row("curved", 0, 1, [
      { fromLane: 0, toLane: 1, colorId: 9, shape: "outgoing" },
    ]);
    expect(hoveredGraphPath(curved, curveX, curveY, geometry)).toBe(9);
  });

  it("graph hover prefers the node and ignores empty space", () => {
    const geometry = naturalGeometry(4);
    const target = row("node", 1, 11, [
      { fromLane: 0, toLane: 1, colorId: 4, shape: "incoming" },
    ]);
    expect(hoveredGraphPath(target, laneX(geometry, 1), 18, geometry)).toBe(11);
    expect(hoveredGraphPath(target, geometry.width - 1, 2, geometry)).toBeNull();
  });

  it("compact graph keeps each rows color as its hover identity", () => {
    const geometry = compactGeometry(20);
    const target = row("compact", 14, 9, []);
    expect(laneX(geometry, 0)).toBe(laneX(geometry, 14));
    expect(hoveredGraphPath(target, laneX(geometry, 0), 2, geometry)).toBe(9);
  });

  it("compressed graph hit testing uses the fitted lane positions", () => {
    const geometry = fittedGeometry(20, 80);
    const target = row("compact", 19, 7, []);
    expect(hoveredGraphPath(target, laneX(geometry, 19), 18, geometry)).toBe(7);
  });
});

function cubicAt(start: number, c1: number, c2: number, end: number, t: number): number {
  const inverse = 1 - t;
  return (
    inverse ** 3 * start + 3 * inverse ** 2 * t * c1 + 3 * inverse * t ** 2 * c2 + t ** 3 * end
  );
}

describe("git-history palette", () => {
  it("graph palette only reduces saturation", () => {
    const source = "hsla(223, 80%, 55%, 0.9)";
    const muted = graphColor(source);
    const match = /^hsla\(([\d.]+), ([\d.]+)%, ([\d.]+)%, ([\d.]+)\)$/.exec(muted);
    expect(match).not.toBeNull();
    const [, h, s, l, a] = match!;
    expect(Number(h)).toBeCloseTo(223, 5);
    expect(Number(s)).toBeCloseTo(57.6, 3);
    expect(Number(l)).toBeCloseTo(55, 5);
    expect(Number(a)).toBeCloseTo(0.9, 5);

    // A hex token input takes the same trip: hue/lightness preserved.
    const red = graphColor("#ff0000");
    const redMatch = /^hsla\(([\d.]+), ([\d.]+)%, ([\d.]+)%, ([\d.]+)\)$/.exec(red);
    expect(redMatch).not.toBeNull();
    expect(Number(redMatch![1])).toBeCloseTo(0, 5);
    expect(Number(redMatch![2])).toBeCloseTo(72, 3);
    expect(Number(redMatch![3])).toBeCloseTo(50, 5);
    expect(Number(redMatch![4])).toBeCloseTo(1, 5);
  });
});

describe("git-history ref badges", () => {
  it("ref badges expand with the available width", () => {
    const refs = [ref("branch", "main"), ref("branch", "tag"), ref("branch", "origin")];
    expect(visibleRefCount([], 100)).toBe(0);
    expect(visibleRefCount(refs, 70)).toBe(1);
    expect(visibleRefCount(refs, 115)).toBe(2);
    expect(visibleRefCount(refs, 200)).toBe(3);
  });

  it("ref badges preserve overflow when the first badge is too wide", () => {
    const refs = [
      ref("branch", "feature/accessibility-polish"),
      ref("branch", "main"),
      ref("branch", "origin/main"),
      ref("branch", "upstream/main"),
      ref("branch", "v0.1.53"),
      ref("branch", "HEAD"),
    ];
    expect(visibleRefCount(refs, 120)).toBe(0);
    // The estimate itself is capped at the badge's max width.
    expect(estimatedRefBadgeWidth(refs[0]!)).toBe(112);
  });

  it("ref area preserves subject space and caps at forty five percent", () => {
    expect(refAreaWidth(80)).toBe(0);
    expect(refAreaWidth(200)).toBeCloseTo(86.4, 3);
    expect(refAreaWidth(400)).toBeCloseTo(176.4, 3);
  });

  it("ref tooltip describes each reference kind", () => {
    expect(refDescription(ref("branch", "main"))).toBe("Branch: main");
    expect(refDescription(ref("remote", "origin/main"))).toBe("Remote branch: origin/main");
    expect(refDescription(ref("tag", "v0.1.52"))).toBe("Tag: v0.1.52");
  });
});

describe("git-history authors and dates", () => {
  it("author avatar fallback uses the first visible initial", () => {
    expect(historyAuthorName("")).toBe("Unknown");
    expect(historyAuthorName("   ")).toBe("Unknown");
    expect(historyAuthorInitial("  josé")).toBe("J");
    expect(historyAuthorInitial("   ")).toBe("?");
  });

  it("format date renders month day year and falls back to an em dash", () => {
    expect(formatDate("2026-08-12T12:00:00Z")).toBe("Aug 12, 2026");
    expect(formatDate("2026-01-05T00:00:00Z")).toBe("Jan 5, 2026");
    expect(formatDate("not a date")).toBe("—");
  });
});

describe("git-history avatar decoding", () => {
  it("github avatar payload decodes into a data url with the right mime", () => {
    const jpeg = Buffer.from([0xff, 0xd8, 0xff, 0x70, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64]).toString("base64");
    expect(decodeHistoryAvatar(jpeg)).toBe(`data:image/jpeg;base64,${jpeg}`);

    const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]).toString("base64");
    expect(decodeHistoryAvatar(png)).toBe(`data:image/png;base64,${png}`);

    const gif = Buffer.from("GIF89a0000").toString("base64");
    expect(decodeHistoryAvatar(gif)).toBe(`data:image/gif;base64,${gif}`);

    const webp = Buffer.from("RIFF0000WEBPVP8 ").toString("base64");
    expect(decodeHistoryAvatar(webp)).toBe(`data:image/webp;base64,${webp}`);

    expect(decodeHistoryAvatar("not base64!")).toBeNull();
    const unknown = Buffer.from([0x00, 0x01, 0x02, 0x03]).toString("base64");
    expect(decodeHistoryAvatar(unknown)).toBeNull();
  });
});

describe("git-history column math", () => {
  const widths: HistoryColumnWidths = { author: 88, date: 88, sha: 74 };

  it("commit divider resizes the first visible fixed column", () => {
    const resized = resizedHistoryColumnWidths(
      widths,
      { left: "commit", right: "author", leftWidth: 80, rightWidth: widths.author },
      20,
    );
    expect(resized.author).toBe(68);
    expect(resized.date).toBe(widths.date);
    expect(resized.sha).toBe(widths.sha);
  });

  it("interior column divider preserves width and clamps both sides", () => {
    const anchor = { left: "author" as const, right: "date" as const, leftWidth: widths.author, rightWidth: widths.date };
    const resized = resizedHistoryColumnWidths(widths, anchor, 10);
    expect(resized.author).toBe(98);
    expect(resized.date).toBe(78);
    expect(resized.author + resized.date).toBe(widths.author + widths.date);

    const clamped = resizedHistoryColumnWidths(widths, anchor, 1000);
    expect(clamped.date).toBe(68);
    expect(clamped.author).toBe(108);
  });

  it("visible columns follow persisted order and skip hidden entries", () => {
    const order = ["sha", "author", "date"] as const;
    const columns = { author: false, date: true, sha: true };
    expect(visibleHistoryColumns(order, columns)).toEqual(["sha", "date"]);
  });

  it("reordering visible columns preserves hidden columns", () => {
    const order = ["author", "date", "sha"] as const;
    expect(reorderedHistoryColumns(order, "sha", "date")).toEqual(["author", "sha", "date"]);
  });

  it("reorder drop index respects uneven column widths", () => {
    const columns = ["author", "date", "sha"] as const;
    const rendered = widths.author + widths.date + widths.sha;
    expect(historyColumnDropIndex(10, rendered, columns, widths)).toBe(0);
    expect(historyColumnDropIndex(100, rendered, columns, widths)).toBe(1);
    expect(historyColumnDropIndex(240, rendered, columns, widths)).toBe(2);
  });

  it("history column width and limits match the settings contract", () => {
    expect(historyColumnWidth("commit", widths)).toBe(80);
    expect(historyColumnWidth("author", widths)).toBe(88);
  });
});
