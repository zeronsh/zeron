import { describe, expect, it } from "vitest";
import {
  ACCENT_BAR_WIDTH,
  bodyHeight,
  bodyHeightWith,
  bodyRowCount,
  cleanMessage,
  defaultBaseRef,
  diffPhase,
  DIFF_SCOPE_CHIPS,
  fileCounts,
  fileNotices,
  flattenFiles,
  gutterWidth,
  GUTTER_WIDTH,
  horizontalGeometry,
  MARKER_WIDTH,
  parseKey,
  parsePatch,
  resolveDiff,
  scopeLabel,
  scopeMode,
  splitContentWidth,
  splitPairs,
  splitPairsUpto,
  truncateFileLines,
  unifiedContentWidth,
  upsertDiffFrame,
  visualColumns,
  BODY_BOTTOM_PAD,
  DIFF_LINE_HEIGHT,
  HUNK_HEADER_HEIGHT,
  NOTICE_HEIGHT,
  type DiffLine,
  type DiffRow,
} from "../src/lib/diff";
import { FilePlaneScroll } from "../src/components/diff-view";

/**
 * The diff model, against the desktop's (`crates/ui/src/changes.rs` tests,
 * 5033-5928). Tests mirror the Rust names (snake_case → camelCase); the
 * shared PATCH fixture is the Rust one verbatim so row counts and heights
 * assert identical values.
 */

const PATCH = [
  "diff --git a/src/main.rs b/src/main.rs",
  "index 111..222 100644",
  "--- a/src/main.rs",
  "+++ b/src/main.rs",
  "@@ -1,4 +1,5 @@ fn main",
  " fn main() {",
  "-    println!(\"old\");",
  "+    println!(\"new\");",
  "+    let x = 1;",
  " }",
  "@@ -10,2 +11,2 @@",
  " // tail",
  "-old_line",
  "+new_line",
  "diff --git a/added.txt b/added.txt",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/added.txt",
  "@@ -0,0 +1,2 @@",
  "+first",
  "+second",
  "\\ No newline at end of file",
  "diff --git a/gone.txt b/gone.txt",
  "deleted file mode 100644",
  "--- a/gone.txt",
  "+++ /dev/null",
  "@@ -1,1 +0,0 @@",
  "-bye",
  "diff --git a/img.png b/img.png",
  "new file mode 100644",
  "Binary files /dev/null and b/img.png differ",
  "diff --git a/old_name.rs b/new_name.rs",
  "similarity index 90%",
  "rename from old_name.rs",
  "rename to new_name.rs",
].join("\n");

describe("parsePatch", () => {
  it("parsesFilesHunksAndLines", () => {
    const files = parsePatch(PATCH);
    expect(files).toHaveLength(5);

    const main = files[0]!;
    expect(main.path).toBe("src/main.rs");
    expect(main.status).toBe("modified");
    expect(main.hunks).toHaveLength(2);
    expect(main.additions).toBe(3);
    expect(main.deletions).toBe(2);
    const h0 = main.hunks[0]!;
    expect(h0.header).toBe("@@ -1,4 +1,5 @@ fn main");
    expect(h0.lines).toHaveLength(5);
    expect(h0.lines[0]!.kind).toBe("context");
    expect(h0.lines[0]!.oldNo).toBe(1);
    expect(h0.lines[0]!.newNo).toBe(1);
    expect(h0.lines[1]!.kind).toBe("del");
    expect(h0.lines[1]!.oldNo).toBe(2);
    expect(h0.lines[1]!.newNo).toBeNull();
    expect(h0.lines[2]!.kind).toBe("add");
    expect(h0.lines[2]!.newNo).toBe(2);
    expect(h0.lines[3]!.kind).toBe("add");
    expect(h0.lines[3]!.newNo).toBe(3);
    // Closing context line: numbering advanced past the add/del block.
    expect(h0.lines[4]!.oldNo).toBe(3);
    expect(h0.lines[4]!.newNo).toBe(4);
    // Second hunk restarts numbering from its header.
    expect(main.hunks[1]!.lines[0]!.oldNo).toBe(10);
    expect(main.hunks[1]!.lines[0]!.newNo).toBe(11);
  });

  it("detectsNewDeletedBinaryAndRenamed", () => {
    const files = parsePatch(PATCH);
    const added = files[1]!;
    expect(added.status).toBe("added");
    expect(added.additions).toBe(2);
    // The no-newline marker rides as a Meta line.
    const last = added.hunks[0]!.lines[added.hunks[0]!.lines.length - 1]!;
    expect(last.kind).toBe("meta");
    expect(last.text).toContain("No newline");
    expect(fileNotices(added).some((n) => n === "New file")).toBe(true);

    const deleted = files[2]!;
    expect(deleted.status).toBe("deleted");
    expect(deleted.deletions).toBe(1);
    expect(fileNotices(deleted).some((n) => n === "Deleted file")).toBe(true);

    const binary = files[3]!;
    expect(binary.binary).toBe(true);
    expect(binary.status).toBe("added");
    expect(binary.hunks).toHaveLength(0);
    expect(fileNotices(binary).some((n) => n.includes("Binary"))).toBe(true);

    const renamed = files[4]!;
    expect(renamed.status).toBe("renamed");
    expect(renamed.path).toBe("new_name.rs");
    expect(renamed.oldPath).toBe("old_name.rs");
    expect(fileNotices(renamed).some((n) => n.includes("old_name.rs"))).toBe(true);
  });

  it("emptyAndGarbagePatchesParseToNothing", () => {
    expect(parsePatch("")).toHaveLength(0);
    expect(parsePatch("not a diff\nat all\n")).toHaveLength(0);
    // Truncated mid-hunk: keeps what parsed.
    const files = parsePatch("diff --git a/x b/x\n@@ -1,9 +1,9 @@\n ctx\n+add");
    expect(files).toHaveLength(1);
    expect(files[0]!.hunks[0]!.lines).toHaveLength(2);
    expect(files[0]!.additions).toBe(1);
  });

  it("quotedAndSpacedPaths", () => {
    // Quoted paths resolve through the `diff --git` line's splitter (the
    // desktop tests `parse_git_paths` directly; the `+++` arm never
    // unquotes on either client).
    const quoted = parsePatch('diff --git "a/with space.rs" "b/with space.rs"\n');
    expect(quoted[0]!.path).toBe("with space.rs");
    expect(quoted[0]!.oldPath).toBeNull();
    const spaced = parsePatch(
      ["diff --git a/dir with spaces/a.rs b/dir with spaces/b.rs",
       "--- a/dir with spaces/a.rs",
       "+++ b/dir with spaces/b.rs"].join("\n"),
    );
    expect(spaced[0]!.path).toBe("dir with spaces/b.rs");
    expect(spaced[0]!.oldPath).toBe("dir with spaces/a.rs");
  });

  it("hunkHeadersParseWithAndWithoutCounts", () => {
    const withCounts = parsePatch(
      ["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -1,4 +2,5 @@", " ctx"].join("\n"),
    );
    expect(withCounts[0]!.hunks[0]!.lines[0]).toMatchObject({ oldNo: 1, newNo: 2 });
    const withoutCounts = parsePatch(
      ["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -7 +9 @@ fn ctx", " ctx"].join("\n"),
    );
    expect(withoutCounts[0]!.hunks[0]!.lines[0]).toMatchObject({ oldNo: 7, newNo: 9 });
    const garbage = parsePatch(
      ["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ garbage"].join("\n"),
    );
    expect(garbage[0]!.hunks).toHaveLength(0);
  });
});

describe("resolveDiff", () => {
  type Diff = { readonly checkoutId: string; readonly deviceId: string; readonly cwd: string };
  const diffs: readonly Diff[] = [
    { checkoutId: "co-1", deviceId: "dev-a", cwd: "/repo/one" },
    { checkoutId: "co-2", deviceId: "dev-b", cwd: "/repo/two" },
  ];

  it("diffResolutionPrefersCheckoutIdThenCwd", () => {
    // checkout_id match wins even when cwd points elsewhere.
    expect(
      resolveDiff(diffs, { checkoutId: "co-2", deviceId: "dev-a", cwd: "/repo/one" })?.checkoutId,
    ).toBe("co-2");
    // Unknown checkout falls back to device+cwd.
    expect(
      resolveDiff(diffs, { checkoutId: "co-9", deviceId: "dev-a", cwd: "/repo/one" })?.checkoutId,
    ).toBe("co-1");
    // Wrong device still matches by cwd alone.
    expect(
      resolveDiff(diffs, { checkoutId: null, deviceId: "dev-z", cwd: "/repo/two" })?.checkoutId,
    ).toBe("co-2");
    // Nothing to go on.
    expect(resolveDiff(diffs, { checkoutId: null, deviceId: "dev-a", cwd: null })).toBeNull();
    expect(resolveDiff(diffs, { checkoutId: null, deviceId: "dev-a", cwd: "/elsewhere" })).toBeNull();
  });
});

describe("phases", () => {
  it("preparing clean and list follow the active diff", () => {
    expect(diffPhase(null)).toBe("preparing");
    expect(diffPhase({ patch: "  \n", files: [] })).toBe("clean");
    expect(diffPhase({ patch: "diff --git a/x b/x\n", files: [] })).toBe("list");
    // Engine may report files without patch text (truncation edge).
    expect(diffPhase({ patch: "", files: [{ path: "x" }] })).toBe("list");
  });
});

describe("scope helpers", () => {
  it("headerLabelPluralizes", () => {
    expect(scopeLabel({ scope: "workingTree", count: 0 })).toBe("0 Uncommitted changes");
    expect(scopeLabel({ scope: "workingTree", count: 1 })).toBe("1 Uncommitted change");
    expect(scopeLabel({ scope: "workingTree", count: 4 })).toBe("4 Uncommitted changes");
  });

  it("scopeLabelsAndCleanMessages", () => {
    expect(scopeLabel({ scope: "branch", count: 1, base: "main" })).toBe("1 Changed file vs main");
    expect(scopeLabel({ scope: "branch", count: 3 })).toBe("3 Changed files");
    expect(scopeLabel({ scope: "turn", count: 2 })).toBe("2 Changed files this turn");
    expect(scopeLabel({ scope: "commit", count: 2 })).toBe("2 Changed files in this commit");
    expect(cleanMessage("workingTree", null)).toBe("No uncommitted changes");
    expect(cleanMessage("branch", "develop")).toBe("No changes vs develop");
    expect(cleanMessage("branch", null)).toBe("No branch changes");
    expect(cleanMessage("turn", null)).toBe("No changes this turn");
    expect(cleanMessage("commit", null)).toBe("No changes in this commit");
  });

  it("baseRefDefaultsToRepoDefaultThenMain", () => {
    // Engine order puts the repo default first — take it when it isn't the
    // checked-out branch itself.
    expect(defaultBaseRef(["main", "feature"], "feature")).toBe("main");
    // No origin/HEAD: engine "default" is the current branch — fall
    // through to main/master.
    expect(defaultBaseRef(["feature", "main"], "feature")).toBe("main");
    expect(defaultBaseRef(["feature", "master"], "feature")).toBe("master");
    // No main/master: any branch that isn't the current one.
    expect(defaultBaseRef(["feature", "develop"], "feature")).toBe("develop");
    // Checked out ON main: comparing main with itself is the honest default.
    expect(defaultBaseRef(["main", "feature"], "main")).toBe("main");
    // Single-branch repo, and empty list.
    expect(defaultBaseRef(["main"], "main")).toBe("main");
    expect(defaultBaseRef([], "main")).toBeNull();
  });

  it("scopeModesAreWireStable", () => {
    // `mode` is the GetCheckoutDiff wire contract — engine matches on it.
    expect(scopeMode("workingTree")).toBe("workingTree");
    expect(scopeMode("branch")).toBe("branch");
    expect(scopeMode("turn")).toBe("turn");
    expect(scopeMode("commit")).toBe("commit");
    // The scope chips expose the three selectable values; commit is
    // tab-mounted only, history is its own surface (ticket 27).
    expect(DIFF_SCOPE_CHIPS).toEqual(["workingTree", "branch", "turn"]);
  });
});

describe("splitPairs", () => {
  type L = { kind: "context" | "add" | "del" | "meta"; oldNo: number | null; newNo: number | null; text: string };
  const lines = (kinds: readonly L["kind"][]): L[] => kinds.map((kind) => ({ kind, oldNo: null, newNo: null, text: kind }));

  it("splitPairsAlignEditsAndStrandTheRest", () => {
    const files = parsePatch(PATCH);
    // src/main.rs hunk 0: context, −1, +1, +1, context. The edited line
    // pairs across; the extra addition is stranded on the right.
    expect(splitPairs(files[0]!.hunks[0]!.lines)).toEqual([
      [0, 0],
      [1, 2],
      [null, 3],
      [4, 4],
    ]);
    // A pure add: every row is right-only, including the trailing
    // no-newline Meta line — it belongs to the side it follows.
    expect(splitPairs(files[1]!.hunks[0]!.lines)).toEqual([
      [null, 0],
      [null, 1],
      [null, 2],
    ]);
    // A pure delete strands the left.
    expect(splitPairs(files[2]!.hunks[0]!.lines)).toEqual([[0, null]]);
    expect(splitPairs([])).toEqual([]);

    // `-a +b -c +d` is two one-line edits, not one four-line one: a
    // deletion arriving after additions opens a new block.
    expect(splitPairs(lines(["del", "add", "del", "add"]))).toEqual([
      [0, 1],
      [2, 3],
    ]);
  });

  it("noNewlineMarkersKeepTheirEditPaired", () => {
    // Both files lost their final newline: git writes the marker twice,
    // once per side. Neither may split the edit into one-sided rows.
    const both = parsePatch(
      ["diff --git a/a.txt b/a.txt",
       "--- a/a.txt",
       "+++ b/a.txt",
       "@@ -1 +1 @@",
       "-old",
       "\\ No newline at end of file",
       "+new",
       "\\ No newline at end of file"].join("\n"),
    );
    const bothLines = both[0]!.hunks[0]!.lines;
    expect(bothLines.map((line) => line.kind)).toEqual(["del", "meta", "add", "meta"]);
    // One aligned old/new row, then the two markers on one row of their
    // own — four lines read as two rows, not four.
    expect(splitPairs(bothLines)).toEqual([
      [0, 2],
      [1, 3],
    ]);
    const full = splitPairs(bothLines);
    for (let cap = 0; cap <= full.length + 2; cap += 1) {
      expect(splitPairsUpto(bothLines, cap)).toEqual(full.slice(0, Math.min(cap, full.length)));
    }

    // Only the old file lacked one: the edit still pairs, and the lone
    // marker takes a row on its own side.
    const oldOnly = parsePatch(
      ["diff --git a/a.txt b/a.txt",
       "--- a/a.txt",
       "+++ b/a.txt",
       "@@ -1 +1 @@",
       "-old",
       "\\ No newline at end of file",
       "+new"].join("\n"),
    );
    expect(splitPairs(oldOnly[0]!.hunks[0]!.lines)).toEqual([
      [0, 2],
      [1, null],
    ]);
  });

  it("cappedPairingAgreesWithTheFullPairingAndStaysBounded", () => {
    // The fold tween re-renders its stand-in every frame, so the capped
    // walk must be a true prefix of the full one — not an approximation.
    const lines = parsePatch(PATCH)[0]!.hunks[0]!.lines;
    const full = splitPairs(lines);
    for (let cap = 0; cap <= full.length + 2; cap += 1) {
      expect(splitPairsUpto(lines, cap)).toEqual(full.slice(0, Math.min(cap, full.length)));
    }

    // A huge single-sided run must not be materialized to yield a few
    // rows: 20k deletions, 5 rows asked for, 5 rows built.
    const many: DiffLine[] = Array.from({ length: 20_000 }, (_, n) => ({
      kind: "del",
      oldNo: n + 1,
      newNo: null,
      text: "",
    }));
    const capped = splitPairsUpto(many, 5);
    expect(capped).toHaveLength(5);
    expect(capped[4]).toEqual([4, null]);
  });
});

describe("rows", () => {
  it("rowsFlattenToLineGranularity", () => {
    const files = parsePatch(PATCH);
    const rows = flattenFiles(files, "unified", new Map());
    // Every file's span starts with its header…
    const starts = rows.filter((row) => row.kind === "fileHeader").map((row) => rows.indexOf(row));
    expect(starts).toEqual([0, 12, 19, 24, 28]);
    // …and spans exactly header + analytic body rows.
    for (let ix = 0; ix < files.length; ix += 1) {
      const start = starts[ix]!;
      const end = ix + 1 < starts.length ? starts[ix + 1]! : rows.length;
      expect(end - start).toBe(1 + bodyRowCount(files[ix]!));
    }

    // src/main.rs: header, 2 hunk headers, 8 lines, pad.
    const mainRows = rows.slice(0, 12);
    expect(mainRows.map((row) => row.kind)).toEqual([
      "fileHeader", "hunkHeader", "line", "line", "line", "line", "line",
      "hunkHeader", "line", "line", "line", "bodyPad",
    ]);
    // Notices lead the body: the added file carries "New file".
    expect(rows[13]!.kind).toBe("notice");
    expect(bodyNotices(rows, 13)).toEqual(["New file"]);

    // A collapsed file contributes its header row only.
    const folds = new Map([[files[0]!.path, { collapsed: true, epoch: 0, from: 0, to: 0, toggledAt: null, folding: false }]]);
    const collapsedRows = flattenFiles(files, "unified", folds);
    expect(collapsedRows.filter((row) => row.fileIx === 0).map((row) => row.kind)).toEqual(["fileHeader"]);
    expect(collapsedRows.filter((row) => row.fileIx === 1)[0]!.kind).toBe("fileHeader");
  });

  it("splitFlatteningPairsRowsAndKeepsHeightsAnalytic", () => {
    const files = parsePatch(PATCH);
    const rows = flattenFiles(files, "split", new Map());
    // src/main.rs: header, 2 hunk headers, 4 + 2 paired rows, pad — the
    // same 8 lines, two columns.
    const mainRows = rows.filter((row) => row.fileIx === 0);
    expect(mainRows.map((row) => row.kind)).toEqual([
      "fileHeader", "hunkHeader", "splitLine", "splitLine", "splitLine", "splitLine",
      "hunkHeader", "splitLine", "splitLine", "bodyPad",
    ]);
    const paired = mainRows[2] as Extract<DiffRow, { kind: "splitLine" }>;
    expect(paired.left).toBe(0);
    expect(paired.right).toBe(0);
    const stranded = mainRows[4] as Extract<DiffRow, { kind: "splitLine" }>;
    expect(stranded.left).toBeNull();
    expect(stranded.right).toBe(3);
    // Pairing only ever merges rows, so split is never the taller layout.
    expect(mainRows.length).toBeLessThan(1 + bodyRowCount(files[0]!));

    // Heights stay analytic — the fold tween needs no measurement.
    expect(bodyHeightWith(files[0]!, "split")).toBe(
      2 * HUNK_HEADER_HEIGHT + 6 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD,
    );
  });

  it("bodyHeightIsAnalytic", () => {
    const files = parsePatch(PATCH);
    const main = files[0]!;
    const lines = main.hunks.reduce((sum, hunk) => sum + hunk.lines.length, 0);
    expect(bodyHeight(main)).toBe(
      2 * HUNK_HEADER_HEIGHT + lines * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD,
    );
    // Notices add height (added file: 1 notice + meta line inside hunk).
    const added = files[1]!;
    expect(bodyHeight(added)).toBe(
      NOTICE_HEIGHT + HUNK_HEADER_HEIGHT + 3 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD,
    );
  });

  it("truncateCapsLinesAndAppendsNotice", () => {
    const main = parsePatch(PATCH)[0]!; // 2 hunks, 8 lines
    expect(truncateFileLines(main, 10)).toBe(main);

    const capped = truncateFileLines(main, 6);
    expect(capped.hunks.reduce((sum, hunk) => sum + hunk.lines.length, 0)).toBe(6);
    expect(capped.hunks).toHaveLength(2);
    expect(fileNotices(capped).some((n) => n.includes("first 6 of 8 lines"))).toBe(true);
    // body_height stays consistent with what actually renders.
    expect(bodyHeight(capped)).toBe(
      NOTICE_HEIGHT + 2 * HUNK_HEADER_HEIGHT + 6 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD,
    );

    // A cap below the first hunk's length drops later hunks entirely.
    const tighter = truncateFileLines(main, 3);
    expect(tighter.hunks).toHaveLength(1);
    expect(tighter.hunks[0]!.lines).toHaveLength(3);
  });
});

function bodyNotices(rows: readonly DiffRow[], noticeRow: number): readonly string[] {
  const row = rows[noticeRow] as Extract<DiffRow, { kind: "notice" }>;
  return fileNotices(row.file);
}

describe("gutterWidth + fileCounts", () => {
  it("guttersFitTheLargestLineNumber", () => {
    const files = parsePatch(PATCH);
    // src/main.rs second hunk ends at old 11 / new 12.
    expect(files[0]!.maxLine).toBe(12);
    expect(gutterWidth(files[0]!)).toBe(GUTTER_WIDTH);

    // Every digit count keeps ≥6px clear of the accent bar on the left
    // of the number (digits×6.6 + 8px right pad + 6px gap), and the
    // column never shrinks below the classic 36px.
    const file = { ...files[0]! };
    for (let digits = 1; digits <= 7; digits += 1) {
      file.maxLine = 10 ** digits - 1;
      const w = gutterWidth(file);
      expect(w).toBeGreaterThanOrEqual(GUTTER_WIDTH);
      const leftGap = w - (digits * 6.6 + 8);
      expect(leftGap).toBeGreaterThanOrEqual(6);
    }
    // 4 digits outgrow the classic column now.
    file.maxLine = 9999;
    expect(gutterWidth(file)).toBeGreaterThan(GUTTER_WIDTH);
    file.maxLine = 27404;
    expect(gutterWidth(file)).toBeGreaterThan(gutterWidth({ ...file, maxLine: 9999 }));

    // Truncation refits the gutter to what actually renders: the first
    // 3 lines are ctx(1,1) / del(2,·) / add(·,2) — max line 2.
    expect(truncateFileLines(files[0]!, 3).maxLine).toBe(2);
  });

  it("renders the +/- summary and a binary marker", () => {
    expect(fileCounts({ ...baseFile(), additions: 12, deletions: 4 })).toBe("+12 -4");
    expect(fileCounts({ ...baseFile(), additions: 5, deletions: 0 })).toBe("+5");
    expect(fileCounts({ ...baseFile(), additions: 0, deletions: 5 })).toBe("-5");
    expect(fileCounts({ ...baseFile(), binary: true })).toBe("Binary");
    expect(fileCounts({ ...baseFile(), additions: 0, deletions: 0 })).toBe("");
  });
});

describe("horizontal geometry", () => {
  it("horizontalGeometryCountsTabsAndUnicodeColumns", () => {
    expect(visualColumns("ab\tc")).toBe(5);
    expect(visualColumns("界")).toBe(2);
    expect(visualColumns("e\u0301")).toBe(1);

    const files = parsePatch("diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+ab\t界\n");
    const geometry = horizontalGeometry(files[0]!);
    expect(geometry.maxCodeColumns).toBe(6);
    expect(geometry.maxGutterWidth).toBe(GUTTER_WIDTH);
  });

  it("horizontalContentWidthCompensatesForLocalGutters", () => {
    const maxTextWidth = 240;
    const maxGutterWidth = 52;
    const narrow = 36;
    const wide = 52;

    const unifiedTotal = (gutter: number): number =>
      ACCENT_BAR_WIDTH + 2 * gutter + MARKER_WIDTH + unifiedContentWidth(maxTextWidth, maxGutterWidth, gutter);
    expect(unifiedTotal(narrow)).toBe(unifiedTotal(wide));

    const splitTotal = (gutter: number): number =>
      ACCENT_BAR_WIDTH + gutter + 18 + splitContentWidth(maxTextWidth, maxGutterWidth, gutter);
    expect(splitTotal(narrow)).toBe(splitTotal(wide));
  });

  it("horizontalScrollAndWidthAreIndependentPerFile", () => {
    const files = parsePatch(
      "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+short\n" +
      "diff --git a/b b/b\n@@ -1 +1 @@\n-old\n+a much longer source line\n",
    );
    expect(files).toHaveLength(2);
    expect(horizontalGeometry(files[0]!).maxCodeColumns).toBe(5);
    expect(horizontalGeometry(files[1]!).maxCodeColumns).toBe(25);

    // The web's scroll state: one offset per file, shared by every row
    // and slot of that file, never crossing files.
    const scroll = new FilePlaneScroll();
    const aOne = fakeElement();
    const bOld = fakeElement();
    const bNew = fakeElement();
    const unregisterA = scroll.register("a", aOne);
    scroll.register("b", bOld);
    scroll.register("b", bNew);

    aOne.scrollLeft = 96;
    scroll.onScroll("a", aOne);
    expect(scroll.offset("a")).toBe(96);
    expect(scroll.offset("b")).toBe(0);
    expect(bNew.scrollLeft).toBe(0);

    bNew.scrollLeft = 48;
    scroll.onScroll("b", bNew);
    // The other slot of the same file follows; the other file does not.
    expect(bOld.scrollLeft).toBe(48);
    expect(scroll.offset("a")).toBe(96);
    unregisterA();
  });

  it("horizontalScrollResetReturnsToOrigin", () => {
    const scroll = new FilePlaneScroll();
    const el = fakeElement();
    scroll.register("a", el);
    el.scrollLeft = 120;
    scroll.onScroll("a", el);
    expect(scroll.offset("a")).toBe(120);

    scroll.reset();

    expect(scroll.offset("a")).toBe(0);
    expect(el.scrollLeft).toBe(0);
  });
});

function fakeElement(): HTMLElement {
  return { scrollLeft: 0 } as unknown as HTMLElement;
}

describe("parseKey", () => {
  it("folds every input that affects parse identity", () => {
    expect(parseKey("co", "ck", "branch", "main")).not.toBe(parseKey("co", "ck", "branch", "dev"));
    expect(parseKey("co", "ck", "branch", "main")).not.toBe(parseKey("co", "ck", "workingTree", "main"));
    expect(parseKey("co", "ck", "branch", "main")).not.toBe(parseKey("co", "other", "branch", "main"));
  });
});

describe("diff frames", () => {
  it("diffFramesReplaceListsAndUpsertSingles", () => {
    type Diff = { readonly checkoutId: string; readonly patch: string };
    let diffs: readonly Diff[] = [];
    const one: Diff = { checkoutId: "co-1", patch: "p1" };
    // Single frame inserts.
    diffs = upsertDiffFrame(diffs, one);
    expect(diffs).toHaveLength(1);
    // Identical frame is a no-op (same list identity).
    expect(upsertDiffFrame(diffs, one)).toBe(diffs);
    // Same checkout upserts in place.
    diffs = upsertDiffFrame(diffs, { checkoutId: "co-1", patch: "p2" });
    expect(diffs).toHaveLength(1);
    expect(diffs[0]!.patch).toBe("p2");
  });
});

function baseFile() {
  return {
    path: "x.rs",
    oldPath: null,
    status: "modified" as const,
    binary: false,
    notices: [],
    hunks: [] as { header: string; lines: ReturnType<typeof linesFor> }[],
    additions: 0,
    deletions: 0,
    maxLine: 1,
  };
}

function linesFor(): { kind: "context" | "add" | "del" | "meta"; oldNo: number | null; newNo: number | null; text: string }[] {
  return [];
}
