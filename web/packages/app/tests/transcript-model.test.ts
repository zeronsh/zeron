// @vitest-environment jsdom

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { MessagePart, SessionMessageEntry, ToolCall, TranscriptFrame } from "@zeron/proto";
import { parseMarkdown, type InlineRun } from "../src/lib/markdown";
import { bodyHeight } from "../src/lib/diff";
import {
  CHIPS_TOP_PAD,
  CHIP_HEIGHT,
  CHIP_GAP,
  TranscriptDesync,
  applyTranscriptFrame,
  assistantCopyText,
  blobDetail,
  callBlock,
  chipsHeight,
  detailHeight,
  diffRows,
  fileBadgeName,
  formatKb,
  formatTimestamp,
  rowsForEntry,
  singleLine,
  stripSpawnPrefix,
  subagentTabTitle,
  thoughtLines,
  toolChipContent,
  toolDetail,
  toolGroupSummary,
  toolGroupTitle,
  topGapFor,
  userMessageNeedsCollapse,
  visibleRowWindow,
  type TranscriptRow,
} from "../src/lib/transcript";
import {
  ACTIVITY_BEND_RADIUS,
  ACTIVITY_BRANCH_END_X,
  ACTIVITY_TRUNK_X,
  activityBranchPoints,
  railPath,
  toolConnectorContinuation,
  toolConnectorParts,
  toolTitleShimmerAmount,
  toolTitleShimmerPhase,
} from "../src/lib/tool-motion";
import {
  jumpVisibility,
  shouldAnchorLiveStream,
  shouldRestick,
} from "../src/lib/stick-spring";

const parse = (_key: string, text: string, live: boolean) => parseMarkdown(text, live);

function entry(id: string, parts: MessagePart[], fields: Partial<SessionMessageEntry> = {}): SessionMessageEntry {
  return { id, role: "assistant", parts, createdAt: 1758000000000, deviceId: "dev", status: null, ...fields };
}

function textPart(id: string, text: string): MessagePart {
  return { kind: "text", id, text };
}

function toolPart(id: string, call: ToolCall, fields: Partial<Extract<MessagePart, { kind: "tool" }>> = {}): MessagePart {
  return { kind: "tool", id, call, isError: false, resolved: true, ...fields };
}

function exec(command: string): ToolCall {
  return { kind: "exec", command };
}

// ---------------------------------------------------------------------------
// Delta application (port of doc/src/transcript_delta.rs apply tests)
// ---------------------------------------------------------------------------

describe("applyTranscriptFrame", () => {
  it("reset replaces the transcript", () => {
    const a = entry("a", [textPart("t0", "hello")]);
    expect(applyTranscriptFrame([], { reset: [a] })).toEqual([a]);
  });

  it("a streaming tick appends text without re-sending the entry", () => {
    const a = entry("a", [textPart("t0", "prompt")]);
    const b0 = entry("b", [textPart("t0", "streaming…")]);
    const frame: TranscriptFrame = { upsert: [], append: [{ entry: "b", part: "t0", text: " more", len: 15 }], remove: [], count: 2 };
    const next = applyTranscriptFrame([a, b0], frame);
    const part = next[1]!.parts[0];
    if (part === undefined) {
      throw new Error("expected part");
    }
    expect(part.kind).toBe("text");
    if (part.kind !== "text") {
      throw new Error("expected text part");
    }
    expect(part.text).toBe("streaming… more");
    // The untouched entry keeps its identity (React memoization holds).
    expect(next[0]).toBe(a);
  });

  it("upserts anchor after the given entry and replace in place", () => {
    const a = entry("a", [textPart("t0", "1")]);
    const b = entry("b", [textPart("t0", "2")]);
    const c = entry("c", [textPart("t0", "3")]);
    // Mid-list insert (a Loro merge landing b between a and c).
    const inserted = applyTranscriptFrame([a, c], { upsert: [{ after: "a", entry: b }], append: [], remove: [], count: 3 });
    expect(inserted.map((e) => e.id)).toEqual(["a", "b", "c"]);
    // Replace: same id re-upserts at the same position.
    const b2 = entry("b", [textPart("t0", "2!")]);
    const replaced = applyTranscriptFrame(inserted, { upsert: [{ after: "a", entry: b2 }], append: [], remove: [], count: 3 });
    expect(replaced.map((e) => e.id)).toEqual(["a", "b", "c"]);
    expect(replaced[1]).toBe(b2);
  });

  it("removes entries and validates the count tripwire", () => {
    const a = entry("a", [textPart("t0", "1")]);
    const b = entry("b", [textPart("t0", "2")]);
    const c = entry("c", [textPart("t0", "3")]);
    const next = applyTranscriptFrame([a, b, c], { upsert: [], append: [], remove: ["b"], count: 2 });
    expect(next.map((e) => e.id)).toEqual(["a", "c"]);
    expect(() => applyTranscriptFrame([a], { upsert: [], append: [], remove: [], count: 5 })).toThrow(TranscriptDesync);
  });

  it("append length mismatch is a desync (resubscribe tripwire)", () => {
    const a = entry("a", [textPart("t0", "hello")]);
    const frame: TranscriptFrame = { upsert: [], append: [{ entry: "a", part: "t0", text: "x", len: 99 }], remove: [], count: 1 };
    expect(() => applyTranscriptFrame([a], frame)).toThrow(TranscriptDesync);
  });

  it("a missing anchor is a desync", () => {
    const x = entry("x", [textPart("t0", "1")]);
    const frame: TranscriptFrame = { upsert: [{ after: "missing", entry: x }], append: [], remove: [], count: 2 };
    expect(() => applyTranscriptFrame([], frame)).toThrow(TranscriptDesync);
  });

  it("a reset preserves identities of unchanged entries (cache-swap case)", () => {
    const a = entry("a", [textPart("t0", "hello")]);
    const b = entry("b", [textPart("t0", "world")]);
    const current = [a, b];
    const again = applyTranscriptFrame(current, { reset: [entry("a", [textPart("t0", "hello")]), b] });
    expect(again[0]).toBe(a);
    expect(again[1]).toBe(b);
    expect(applyTranscriptFrame(current, { reset: [a, b] })).toBe(current);
  });
});

// ---------------------------------------------------------------------------
// Row model (port of transcript.rs rows_for_entry)
// ---------------------------------------------------------------------------

describe("rowsForEntry", () => {
  it("a user entry is one bubble row with the timestamp strip", () => {
    const user = entry("u1", [textPart("t0", "hello there")], { role: "user" });
    const rows = rowsForEntry(user, { parse });
    expect(rows.length).toBe(1);
    expect(rows[0]!.id).toBe("u1");
    expect(rows[0]!.turnStart).toBe(true);
    expect(rows[0]!.timestamp).toBe(user.createdAt);
    expect(rows[0]!.copyText).toBe("hello there");
    expect(rows[0]!.rowKind).toMatchObject({ kind: "user", text: "hello there", pending: false });
  });

  it("assistant text splits one row per top-level markdown block", () => {
    const e = entry("a1", [textPart("p0", "# Title\n\nsome text\n\n```ts\nconst x = 1;\n```")]);
    const rows = rowsForEntry(e, { parse });
    expect(rows.map((row) => row.id)).toEqual(["a1#p0.0", "a1#p0.1", "a1#p0.2"]);
    expect(rows.map((row) => row.rowKind.kind)).toEqual(["markdown", "markdown", "markdown"]);
    // Only the first row of the entry opens the turn.
    expect(rows.map((row) => row.turnStart)).toEqual([true, false, false]);
    // The settled entry's last row carries the timestamp strip and copy text.
    expect(rows[2]!.timestamp).toBe(e.createdAt);
    expect(rows[2]!.copyText).toContain("some text");
    expect(rows[0]!.timestamp).toBeNull();
  });

  it("streaming text rows are liveMarkdown with identical ids", () => {
    const live = entry("a1", [textPart("p0", "one\n\ntwo")], { status: "streaming" });
    const rows = rowsForEntry(live, { parse });
    expect(rows.map((row) => row.id)).toEqual(["a1#p0.0", "a1#p0.1"]);
    expect(rows.every((row) => row.rowKind.kind === "liveMarkdown")).toBe(true);
    // No timestamp hover mid-stream.
    expect(rows.every((row) => row.timestamp === null)).toBe(true);
  });

  it("consecutive tools fold into one group; text flushes it", () => {
    const e = entry("a1", [
      textPart("t0", "looking"),
      toolPart("c0", exec("ls")),
      toolPart("c1", exec("pwd")),
      textPart("t1", "done"),
    ]);
    const rows = rowsForEntry(e, { parse });
    const group = rows.find((row) => row.rowKind.kind === "toolGroup");
    expect(group).toBeDefined();
    expect(group!.id).toBe("a1#g0");
    if (group!.rowKind.kind === "toolGroup") {
      expect(group!.rowKind.tools.length).toBe(2);
      expect(group!.rowKind.autoOpen).toBe(false);
    }
    // Text around the tools splits into separate markdown rows.
    expect(rows[0]!.rowKind.kind).toBe("markdown");
    expect(rows[rows.length - 1]!.rowKind.kind).toBe("markdown");
  });

  it("a streaming entry's tail tool group auto-opens", () => {
    const e = entry("a1", [toolPart("c0", exec("ls"), { resolved: false })], { status: "streaming" });
    const rows = rowsForEntry(e, { parse });
    const group = rows[0]!;
    expect(group.rowKind.kind === "toolGroup" && group.rowKind.autoOpen).toBe(true);
  });

  it("agent spawn chips never share a fold with ordinary tools", () => {
    const spawn: ToolCall = { kind: "unknown", name: "Agent: scan repo" };
    const e = entry("a1", [toolPart("c0", exec("ls")), toolPart("c1", spawn), toolPart("c2", exec("pwd"))]);
    const rows = rowsForEntry(e, { parse });
    // The genus flips twice: [exec] · [spawn] · [exec].
    const groups = rows.filter((row) => row.rowKind.kind === "toolGroup");
    expect(groups.length).toBe(3);
    const agentGroup = groups[1]!;
    if (agentGroup.rowKind.kind === "toolGroup") {
      expect(agentGroup.rowKind.tools.length).toBe(1);
      expect(agentGroup.rowKind.tools[0]!.call).toEqual(spawn);
    }
  });

  it("reasoning rides the tool group as a thought chip", () => {
    const e = entry("a1", [
      toolPart("c0", exec("ls")),
      { kind: "reasoning", id: "r0", text: "thinking about it" },
    ]);
    const rows = rowsForEntry(e, { parse });
    const group = rows[0]!;
    expect(group.rowKind.kind).toBe("toolGroup");
    if (group.rowKind.kind === "toolGroup") {
      expect(group.rowKind.tools.length).toBe(2);
      expect(group.rowKind.tools[1]!.isThought).toBe(true);
      expect(group.rowKind.tools[1]!.resolved).toBe(true);
    }
  });

  it("a live tail thought is unresolved and keeps the tail when truncated", () => {
    const long = Array.from({ length: 40 }, (_, ix) => `line ${ix} of thinking`).join("\n\n");
    const e = entry("a1", [{ kind: "reasoning", id: "r0", text: long }], { status: "streaming" });
    const rows = rowsForEntry(e, { parse });
    const group = rows[0]!;
    if (group.rowKind.kind !== "toolGroup") {
      throw new Error("expected a tool group");
    }
    const thought = group.rowKind.tools[0]!;
    expect(thought.isThought).toBe(true);
    expect(thought.resolved).toBe(false);
    expect(thought.detail).not.toBeNull();
    if (thought.detail !== null && thought.detail.kind === "thought") {
      expect(thought.detail.lines.length).toBeLessThanOrEqual(24);
      expect(thought.detail.truncatedBy).toBeGreaterThan(0);
      // Live thoughts keep the TAIL (the fresh thinking is the signal).
      const flat = thought.detail.lines.map((line) => line.map((run) => run.text).join("")).join("\n");
      expect(flat).toContain("line 39");
    }
  });

  it("input and error parts render as chips", () => {
    const e = entry("a1", [
      { kind: "input", id: "i0", requestId: "req", questions: [{ id: "q", header: "Pick one", question: "?", options: ["a"], multiSelect: false }], resolved: false },
      { kind: "error", id: "e0", message: "boom\nstack" },
    ]);
    const rows = rowsForEntry(e, { parse });
    expect(rows[0]!.rowKind).toEqual({ kind: "inputChip", header: "Pick one", resolved: false });
    expect(rows[1]!.rowKind).toEqual({ kind: "errorChip", message: "boom stack" });
  });

  it("empty text parts produce no rows", () => {
    const e = entry("a1", [textPart("t0", "   ")]);
    expect(rowsForEntry(e, { parse })).toEqual([]);
  });
});

describe("topGapFor / diffRows", () => {
  const row = (id: string, kind: TranscriptRow["rowKind"]["kind"], turnStart = false): TranscriptRow => {
    const rowKind: TranscriptRow["rowKind"] =
      kind === "markdown" || kind === "liveMarkdown"
        ? { kind, tree: { blocks: [] }, blockIx: 0 }
        : kind === "toolGroup"
          ? { kind, tools: [], autoOpen: false }
          : kind === "user"
            ? { kind, text: "", mentions: [], badges: [], pending: false, attachments: [] }
            : kind === "inputChip"
              ? { kind, header: "", resolved: false }
              : { kind, message: "" };
    return {
      id,
      version: 0,
      turnStart,
      rowKind,
      entryId: id,
      timestamp: null,
      copyText: null,
    };
  };

  it("turn starts get the large gap; same-part blocks the block gap", () => {
    expect(topGapFor(null, row("a", "errorChip", true))).toBe(16);
    // split_sibling_gaps_match_live_internal_spacing: the markdown clause
    // requires BOTH rows to be markdown kinds — a chip after a block keeps
    // the small step even with a shared part prefix.
    const mdA = row("e#p.0", "markdown");
    const mdB = row("e#p.1", "markdown");
    expect(topGapFor(mdA, mdB)).toBe(12);
    expect(topGapFor(row("e#p.0", "errorChip"), row("e#p.1", "errorChip"))).toBe(8);
    expect(topGapFor(mdA, row("e#p.1", "errorChip"))).toBe(8);
    expect(topGapFor(row("x", "toolGroup"), row("y", "errorChip"))).toBe(12);
    expect(topGapFor(row("x", "errorChip"), row("y", "toolGroup"))).toBe(12);
  });

  it("diffRows finds the minimal splice", () => {
    const a = row("a", "errorChip");
    const b = row("b", "errorChip");
    const c = row("c", "errorChip");
    expect(diffRows([a, b, c], [a, b, c])).toBeNull();
    expect(diffRows([a, b, c], [a, c])).toEqual([1, 1, 0]);
    expect(diffRows([a], [a, b])).toEqual([1, 0, 1]);
    const b2 = { ...b, version: 9 };
    expect(diffRows([a, b, c], [a, b2, c])).toEqual([1, 1, 1]);
  });
});

// ---------------------------------------------------------------------------
// Tool chips (port of view.rs tool_chip_content / tool_group_summary)
// ---------------------------------------------------------------------------

describe("toolChipContent", () => {
  it("tool_chip_labels_per_kind", () => {
    expect(toolChipContent(exec("ls -la"))).toEqual({ label: "Run", detail: "ls -la" });
    expect(toolChipContent({ kind: "readFile", path: "a/b.ts" })).toEqual({ label: "Read", detail: "a/b.ts" });
    expect(toolChipContent({ kind: "writeFile", path: "a/b.ts" })).toEqual({ label: "Write", detail: "a/b.ts" });
    expect(toolChipContent({ kind: "editFile", path: "a/b.ts" })).toEqual({ label: "Edit", detail: "a/b.ts" });
    expect(toolChipContent({ kind: "applyPatch", path: null })).toEqual({ label: "Patch", detail: "workspace" });
    expect(toolChipContent({ kind: "search", pattern: "foo", path: "src" })).toEqual({ label: "Search", detail: "foo in src" });
    expect(toolChipContent({ kind: "search", pattern: "foo", path: null })).toEqual({ label: "Search", detail: "foo" });
    expect(toolChipContent({ kind: "glob", pattern: "**/*.ts" })).toEqual({ label: "Glob", detail: "**/*.ts" });
    expect(toolChipContent({ kind: "webFetch", url: "https://x.dev", prompt: null })).toEqual({ label: "Fetch", detail: "https://x.dev" });
    expect(toolChipContent({ kind: "webSearch", query: "zeron" })).toEqual({ label: "Web", detail: "zeron" });
    expect(toolChipContent({ kind: "todo", items: [{ text: "a", done: true }, { text: "b", done: false }] })).toEqual({ label: "Todo", detail: "1/2 done" });
    expect(toolChipContent({ kind: "mcp", server: "fs", tool: "read", input: null })).toEqual({ label: "MCP", detail: "fs · read" });
    expect(toolChipContent({ kind: "unknown", name: "Agent: scan repo" })).toEqual({ label: "Agent", detail: "scan repo" });
    expect(toolChipContent({ kind: "unknown", name: "Agent" })).toEqual({ label: "Agent", detail: "" });
    expect(toolChipContent({ kind: "unknown", name: "custom_tool" })).toEqual({ label: "Tool", detail: "custom_tool" });
  });

  it("single_line_collapses_all_whitespace_runs", () => {
    expect(singleLine(" a\n\tb  c ")).toBe("a b c");
    expect(singleLine("plain")).toBe("plain");
    expect(singleLine("")).toBe("");
    expect(singleLine("\n\n")).toBe("");
  });

  it("multiline_command_flattens_to_one_chip_line", () => {
    // The user's breaker: a multi-line script in a Run chip. The detail must
    // come out as ONE sanitized line — the chip's fixed card then truncates
    // it with an ellipsis.
    const { label, detail } = toolChipContent(exec('set -e\nfixture_in_original=0\n\tgrep -c  "x"'));
    expect(label).toBe("Run");
    expect(detail).toBe('set -e fixture_in_original=0 grep -c "x"');
    expect(detail.includes("\n")).toBe(false);
    // The chip row height is a constant, independent of content shape.
    expect(chipsHeight(1)).toBe(CHIPS_TOP_PAD + CHIP_HEIGHT);
    // Every detail kind is sanitized (MCP inputs / queries are model text).
    expect(toolChipContent({ kind: "webSearch", query: "line one\nline two" }).detail).toBe("line one line two");
  });

  it("file_action_badges_show_only_the_file_name", () => {
    expect(fileBadgeName("/Users/me/project/src/main.rs")).toBe("main.rs");
    expect(fileBadgeName("crates/ui/src/transcript.rs")).toBe("transcript.rs");
    expect(fileBadgeName("C:\\project\\src\\main.rs")).toBe("main.rs");
    expect(fileBadgeName("src/components/")).toBe("components");
    expect(fileBadgeName("main.rs")).toBe("main.rs");
    expect(fileBadgeName("")).toBe("");
  });
});

describe("toolGroupSummary", () => {
  const pair = (call: ToolCall, isError = false) => ({ call, isError });

  it("tool_group_summaries", () => {
    expect(toolGroupSummary([pair(exec("ls")), pair(exec("pwd")), pair(exec("cd"))])).toBe("Ran 3 commands");
    expect(
      toolGroupSummary([
        pair(exec("ls")),
        pair({ kind: "editFile", path: "a" }),
        pair({ kind: "writeFile", path: "b" }),
        pair({ kind: "editFile", path: "a" }),
      ]),
    ).toBe("Ran 1 command · edited 2 files");
    // Distinct-path dedupe: editing one file twice counts once.
    expect(toolGroupSummary([pair({ kind: "editFile", path: "a" }), pair({ kind: "editFile", path: "a" })])).toBe("Edited 1 file");
    expect(toolGroupSummary([pair({ kind: "readFile", path: "x" })])).toBe("Read 1 file");
    expect(toolGroupSummary([pair({ kind: "glob", pattern: "*" }), pair({ kind: "webSearch", query: "q" })])).toBe("Searched 2 times");
    expect(toolGroupSummary([pair({ kind: "webFetch", url: "u", prompt: null })])).toBe("Fetched 1 page");
    expect(toolGroupSummary([pair({ kind: "todo", items: [] })])).toBe("Updated todos");
    expect(toolGroupSummary([pair(exec("ls"), true)])).toBe("Ran 1 command · 1 failed");
    expect(toolGroupSummary([])).toBe("0 tools");
  });

  it("names thought chips on the collapsed line", () => {
    const thought = { isThought: true } as never;
    const tool = { isThought: false, call: exec("ls"), isError: false } as never;
    expect(toolGroupTitle([thought])).toBe("Thought process");
    expect(toolGroupTitle([thought, thought])).toBe("Thought 2 times");
    expect(toolGroupTitle([thought, tool])).toBe("Thought · Ran 1 command");
    expect(toolGroupTitle([thought, thought, tool])).toBe("Thought 2 times · Ran 1 command");
    expect(toolGroupTitle([tool])).toBe("Ran 1 command");
  });
});

describe("callBlock", () => {
  it("call_block_carries_the_full_invocation", () => {
    // Multi-line command: verbatim lines, not the flattened chip line.
    const block = callBlock(exec("set -e\ncargo test"));
    expect(block!.kind === "output" && block!.truncatedBy).toBe(0);
    expect(block!.kind === "output" && block!.lines).toEqual(["set -e", "cargo test"]);
    const long = callBlock(exec("x".repeat(200)));
    expect(long!.kind === "output" && long!.lines.every((line) => [...line].length <= 80)).toBe(true);
    expect(long!.kind === "output" && long!.lines.join("")).toBe("x".repeat(200));
    // A long single-line command soft-wraps instead of ellipsizing: 200
    // chars at 80 columns is 3 chunks.
    expect(long!.kind === "output" && long!.lines.length).toBe(3);
  });

  it("renders todo items one per line and mcp input as JSON", () => {
    const todo = callBlock({ kind: "todo", items: [{ text: "a", done: true }, { text: "b", done: false }] });
    expect(todo!.kind === "output" && todo!.lines).toEqual(["[x] a", "[ ] b"]);
    const mcp = callBlock({ kind: "mcp", server: "fs", tool: "read", input: { path: "/x" } });
    expect(mcp!.kind === "output" && mcp!.lines[0]).toBe("fs · read");
    expect(mcp!.kind === "output" && mcp!.lines.join("\n")).toContain('"path": "/x"');
  });
});

// ---------------------------------------------------------------------------
// Tool groups (ticket 19 — transcript.rs tests :10662-11200)
// ---------------------------------------------------------------------------

describe("tool details (ticket 19)", () => {
  it("tool_diff_builds_real_hunks_with_context_and_numbers", () => {
    const oldLines = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`);
    const newLines = [...oldLines];
    newLines[9] = "LINE 10";
    const diff = { path: "/w/a.rs", oldText: oldLines.join("\n") + "\n", newText: newLines.join("\n") + "\n" };
    const detail = toolDetail(null, diff, null);
    expect(detail).not.toBeNull();
    expect(detail!.kind).toBe("diff");
    const file = detail!.kind === "diff" ? detail!.file : null;
    expect(file).not.toBeNull();
    // One hunk: the change plus 3 context lines each side, real numbers.
    expect(file!.hunks.length).toBe(1);
    const hunk = file!.hunks[0]!;
    expect(hunk.header).toBe("@@ -7,7 +7,7 @@");
    expect(hunk.lines.length).toBe(8); // 6 context + 1 del + 1 add
    const del = hunk.lines.find((line) => line.kind === "del");
    expect(del).toBeDefined();
    expect(del!.oldNo).toBe(10);
    expect(del!.newNo).toBeNull();
    expect(del!.text).toBe("line 10");
    const add = hunk.lines.find((line) => line.kind === "add");
    expect(add).toBeDefined();
    expect(add!.newNo).toBe(10);
    expect(add!.oldNo).toBeNull();
    expect(add!.text).toBe("LINE 10");
    expect(file!.additions).toBe(1);
    expect(file!.deletions).toBe(1);
    // New files carry Added status (and no old numbers).
    const created = toolDetail(null, { path: "/w/new.txt", oldText: null, newText: "only\n" }, null);
    expect(created!.kind === "diff" && created!.file.status).toBe("added");
    expect(created!.kind === "diff" && created!.file.hunks[0]!.lines.every((line) => line.oldNo === null)).toBe(true);
    // Output: verbatim lines (indentation intact), counted-tail cap.
    const output = Array.from({ length: 40 }, (_, i) => `    indented ${i}`).join("\n");
    const outDetail = toolDetail(output, null, null);
    expect(outDetail!.kind === "output" && outDetail!.lines.length).toBe(24);
    expect(outDetail!.kind === "output" && outDetail!.truncatedBy).toBe(16);
    expect(outDetail!.kind === "output" && outDetail!.lines[0]).toBe("    indented 0");
    // Diff wins over stats wins over output.
    const stats = [{ path: "a", additions: 1, deletions: 2 }];
    expect(toolDetail("out", diff, stats)!.kind).toBe("diff");
    expect(toolDetail("out", null, stats)!.kind).toBe("stats");
    // Nothing → no detail.
    expect(toolDetail(null, null, null)).toBeNull();
    expect(toolDetail("\n\n", null, null)).toBeNull();
  });

  it("blob_detail parses diff JSON and renders uncapped output", () => {
    const diffJson = JSON.stringify({ path: "/w/a.rs", oldText: null, newText: "a\nb\n" });
    const fromBlob = blobDetail(diffJson, true);
    expect(fromBlob!.kind).toBe("diff");
    // Output blobs cap at the defensive FULL_OUTPUT_MAX_LINES ceiling.
    const longOutput = Array.from({ length: 500 }, (_, i) => `line ${i}`).join("\n");
    const out = blobDetail(longOutput, false);
    expect(out!.kind === "output" && out!.lines.length).toBe(400);
    expect(out!.kind === "output" && out!.truncatedBy).toBe(100);
    expect(blobDetail("  \n\n ", false)).toBeNull();
    expect(blobDetail("{not json", true)).toBeNull();
  });

  it("chips_height_is_analytic", () => {
    expect(chipsHeight(0)).toBe(0);
    expect(chipsHeight(1)).toBe(CHIPS_TOP_PAD + CHIP_HEIGHT);
    expect(chipsHeight(3)).toBe(CHIPS_TOP_PAD + 3 * CHIP_HEIGHT + 2 * CHIP_GAP);
  });

  it("detail_height is analytic per kind", () => {
    const output = toolDetail("a\nb", null, null)!;
    // 2 lines + no tail row → 2·18 + py(6)×2, plus the separator.
    expect(detailHeight(output)).toBe(1 + 2 * 18 + 12);
    const truncated = toolDetail(Array.from({ length: 30 }, () => "x").join("\n"), null, null)!;
    expect(detailHeight(truncated)).toBe(1 + (24 + 1) * 18 + 12);
    const stats = toolDetail(null, null, [{ path: "a", additions: 1, deletions: 0 }])!;
    expect(detailHeight(stats)).toBe(1 + 1 * 18 + 12);
    const diff = toolDetail(null, { path: "a.rs", oldText: null, newText: "one\ntwo\n" }, null)!;
    expect(detailHeight(diff)).toBe(1 + bodyHeight(diff.kind === "diff" ? diff.file : null!));
  });

  it("formatKb never shows decimals", () => {
    expect(formatKb(512)).toBe("512 B");
    expect(formatKb(0)).toBe("0 B");
    expect(formatKb(1023)).toBe("1023 B");
    expect(formatKb(1024)).toBe("1 KB");
    expect(formatKb(12288)).toBe("12 KB");
    expect(formatKb(12500)).toBe("13 KB");
  });
});

describe("subagent tab titles (transcript.rs :7188)", () => {
  it("subagent_tab_title fallbacks and caps", () => {
    // The tab is the BARE task — the "Agent:" genus is stripped.
    expect(subagentTabTitle({ kind: "unknown", name: "Agent: scan repo", input: null })).toBe("scan repo");
    // A bare "Task" digs the description out of the call input.
    expect(
      subagentTabTitle({
        kind: "unknown",
        name: "Task",
        input: { description: "Agent: audit the auth flow", prompt: "very long instructions…" },
      }),
    ).toBe("audit the auth flow");
    // Word boundaries only — a name that merely STARTS with the genus keeps
    // itself.
    expect(subagentTabTitle({ kind: "unknown", name: "Taskmaster", input: null })).toBe("Taskmaster");
    // A bare "agent" strips to "" and falls through to the generic label.
    expect(subagentTabTitle({ kind: "unknown", name: "agent", input: null })).toBe("Subagent");
    // Absurd lengths cap with an ellipsis.
    const long = subagentTabTitle({ kind: "unknown", name: "x".repeat(120), input: null });
    expect([...long].length).toBe(41);
    expect(long.endsWith("…")).toBe(true);
    // Multiline names keep only their first line.
    expect(subagentTabTitle({ kind: "unknown", name: "Agent: one\ntwo", input: null })).toBe("one");
    // Non-spawn-shaped calls stay generic.
    expect(subagentTabTitle(exec("ls"))).toBe("Subagent");
  });

  it("strip_spawn_prefix only strips real word boundaries", () => {
    expect(stripSpawnPrefix("Agent: scan")).toBe("scan");
    expect(stripSpawnPrefix("task  cleanup")).toBe("cleanup");
    expect(stripSpawnPrefix("Taskmaster")).toBe("Taskmaster");
    expect(stripSpawnPrefix("Agent")).toBe("");
    expect(stripSpawnPrefix("plain")).toBe("plain");
  });
});

// ---------------------------------------------------------------------------
// Thought details (transcript.rs :8721-8818)
// ---------------------------------------------------------------------------

describe("thought details (ticket 19)", () => {
  const thoughtOf = (text: string): InlineRun[][] => thoughtLines(parseMarkdown(text, false));
  const lineString = (line: InlineRun[]): string => line.map((run) => run.text).join("");
  const lineChars = (line: InlineRun[]): number => [...lineString(line)].length;

  it("codex_summary_paragraphs_render_as_separate_styled_lines", () => {
    const lines = thoughtOf("**Implementing file badges**\n\n**Preparing fixture screenshots**");
    expect(lines.map(lineString)).toEqual(["Implementing file badges", "", "Preparing fixture screenshots"]);
    for (const ix of [0, 2]) {
      expect(lines[ix]!.every((run) => run.text.trim().length === 0 || run.style.bold === true)).toBe(true);
    }
  });

  it("thought_wrap_is_word_aware_and_bounded", () => {
    const lines = thoughtOf("one two three");
    expect(lines.length).toBe(1);
    expect(lineString(lines[0]!)).toBe("one two three");
    const long = "word ".repeat(200);
    const wrapped = thoughtOf(long);
    expect(wrapped.every((line) => lineChars(line) <= 96)).toBe(true);
    expect(wrapped.length).toBeGreaterThan(5);
    const pathological = "x".repeat(300);
    expect(thoughtOf(pathological).every((line) => lineChars(line) <= 96)).toBe(true);
    // A word glued across style boundaries wraps as ONE unit — no line may
    // split inside `**bold**tail`.
    const glued = `${"word ".repeat(30)} **bold**tail`;
    const joined = thoughtOf(glued).map(lineString);
    expect(joined.some((line) => line.endsWith("boldtail"))).toBe(true);
  });

  it("thought_markdown_styles_instead_of_literal_markers", () => {
    // The exact user report: `**bold**` markers showed as glyphs.
    const lines = thoughtOf("**Planning rollback** then *checking* `parse` [docs](https://d)");
    expect(lines.length).toBe(1);
    const flat = lineString(lines[0]!);
    expect(flat.includes("*")).toBe(false);
    expect(flat.includes("`")).toBe(false);
    expect(flat.includes("[")).toBe(false);
    const line = lines[0]!;
    expect(line.some((run) => run.style.bold === true && run.text.includes("Planning rollback"))).toBe(true);
    expect(line.some((run) => run.style.italic === true && run.text.includes("checking"))).toBe(true);
    expect(line.some((run) => run.style.code === true && run.text.includes("parse"))).toBe(true);
    expect(line.some((run) => run.style.link !== null && run.text.includes("docs"))).toBe(true);
  });

  it("thought_blocks_flatten_structurally", () => {
    const lines = thoughtOf("# Head\n\npara\n\n- one\n- two\n\n```rust\nlet x = 1;\n```");
    const flat = lines.map(lineString);
    // Heading renders bold, same size (one 18px row).
    expect(lines[0]!.some((run) => run.style.bold === true && run.text.includes("Head"))).toBe(true);
    // Blank separator rows between top-level blocks; tight list inside.
    expect(flat[1]).toBe("");
    expect(flat[2]).toBe("para");
    expect(flat[4]).toBe("• one");
    expect(flat[5]).toBe("• two");
    // Code lines verbatim, styled as code (mono at render).
    const last = lines[lines.length - 1]!;
    expect(last.some((run) => run.style.code === true && run.text === "let x = 1;")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Tool motion + rail geometry (transcript.rs :7960-8000, :11127-11200)
// ---------------------------------------------------------------------------

describe("tool motion (ticket 19)", () => {
  it("connector_intersection_is_tessellated_only_once", () => {
    // The web contract: trunk + branch ride ONE <path> with fill-rule
    // nonzero — two contours in a single `d`, never two strokes (stroke
    // tessellation would double-blend the fork).
    const d = railPath({
      bendRowHeight: 32,
      canvasHeight: 32,
      hasPredecessor: false,
      continues: true,
      connectorReveal: 1,
      continuationReveal: 1,
    });
    expect(d).not.toBeNull();
    const starts = d!.split(" ").filter((token) => token === "M").length;
    expect(starts).toBe(2);
    expect(d!.includes("Z")).toBe(true);
    // A row that has not started reveals nothing.
    expect(
      railPath({ bendRowHeight: 32, canvasHeight: 32, hasPredecessor: true, continues: true, connectorReveal: 0, continuationReveal: 0 }),
    ).toBeNull();
  });

  it("tool_branch_reveal_tracks_distance_through_the_bend", () => {
    const length = (points: readonly { x: number; y: number }[]): number => {
      let total = 0;
      for (let ix = 1; ix < points.length; ix += 1) {
        total += Math.hypot(points[ix]!.x - points[ix - 1]!.x, points[ix]!.y - points[ix - 1]!.y);
      }
      return total;
    };
    const full = activityBranchPoints(1);
    expect(activityBranchPoints(0)).toEqual([{ x: 0, y: 0 }]);
    const end = full[full.length - 1]!;
    expect(end.x).toBeCloseTo(ACTIVITY_BRANCH_END_X - ACTIVITY_TRUNK_X, 4);
    expect(end.y).toBeCloseTo(ACTIVITY_BEND_RADIUS, 4);
    for (const progress of [0.1, 0.25, 0.5, 0.75, 0.9]) {
      const partial = activityBranchPoints(progress);
      expect(length(partial) / length(full)).toBeCloseTo(progress, 4);
      for (let ix = 1; ix < partial.length; ix += 1) {
        expect(partial[ix]!.x).toBeGreaterThanOrEqual(partial[ix - 1]!.x);
        expect(partial[ix]!.y).toBeGreaterThanOrEqual(partial[ix - 1]!.y);
      }
    }
  });

  it("tool_connector_parts split one arrival into phases", () => {
    expect(toolConnectorParts(0, false)).toEqual({ incoming: 0, branch: 0 });
    expect(toolConnectorParts(0.44, true)).toEqual({ incoming: 0, branch: 0 });
    expect(toolConnectorContinuation(0)).toBe(0);
    expect(toolConnectorContinuation(0.3)).toBeGreaterThan(0);
    expect(toolConnectorContinuation(0.45)).toBe(1);
    expect(toolConnectorContinuation(null)).toBe(0);
    const { incoming, branch } = toolConnectorParts(0.6, true);
    expect(incoming).toBeGreaterThan(0);
    expect(incoming).toBeLessThan(1);
    expect(branch).toBe(0);
    expect(toolConnectorParts(1, true)).toEqual({ incoming: 1, branch: 1 });
  });

  it("tool_title_shimmer_crosses_the_title_without_a_loop_seam", () => {
    expect(toolTitleShimmerAmount(0.5, 0.5)).toBe(1);
    expect(toolTitleShimmerAmount(0, 0.5)).toBe(0);
    expect(toolTitleShimmerAmount(1, 0.5)).toBe(0);
    expect(toolTitleShimmerAmount(0.3, 0.5)).toBeGreaterThan(0.4);
    expect(toolTitleShimmerAmount(0.7, 0.5)).toBeGreaterThan(0.4);
    for (const x of [0, 0.25, 0.5, 0.75, 1]) {
      expect(toolTitleShimmerAmount(x, 0)).toBe(toolTitleShimmerAmount(x, 1));
    }
    expect(toolTitleShimmerPhase(0)).toBe(0);
    expect(toolTitleShimmerPhase(3400)).toBe(0);
    expect(toolTitleShimmerPhase(1700)).toBeCloseTo(0.5, 6);
  });
});

// ---------------------------------------------------------------------------
// Entry helpers
// ---------------------------------------------------------------------------

describe("entry helpers", () => {
  it("assistantCopyText joins text parts and excludes tools", () => {
    const e = entry("a", [textPart("t0", "one"), toolPart("c0", exec("ls")), textPart("t1", "two")]);
    expect(assistantCopyText(e)).toBe("one\n\ntwo");
    expect(assistantCopyText(entry("b", [toolPart("c0", exec("ls"))]))).toBeNull();
  });

  it("userMessageNeedsCollapse mirrors the desktop proxy", () => {
    expect(userMessageNeedsCollapse("short")).toBe(false);
    expect(userMessageNeedsCollapse(Array.from({ length: 6 }, (_, i) => `line ${i}`).join("\n"))).toBe(true);
    expect(userMessageNeedsCollapse("x".repeat(401))).toBe(true);
  });

  it("formatTimestamp matches the desktop shape", () => {
    // Local-time rendering: assert the shape, not a zone-specific hour.
    expect(formatTimestamp(Date.parse("2026-07-01T15:45:00"))).toMatch(/^[A-Z][a-z]{2} 1, \d{1,2}:45 [AP]M$/);
    expect(formatTimestamp(Number.NaN)).toBe("");
  });
});

// ---------------------------------------------------------------------------
// Virtualizer window
describe("visibleRowWindow", () => {
  // Five 100px rows: positions 0,100,200,300,400; total 500.
  const positions = [0, 100, 200, 300, 400];
  const heights = [100, 100, 100, 100, 100];
  const OVERDRAW = 320;

  it("mounts every row while the whole list fits the window", () => {
    // top 0, height 600: window [-320, 920] covers everything.
    expect(visibleRowWindow(positions, heights, 0, 600, OVERDRAW)).toEqual({ first: 0, last: 4 });
  });

  it("skips rows entirely above the overdraw window", () => {
    // top 500, height 200: window [180, 1020]. Row 0's bottom (100) is above
    // 180, so first is 1 — the top spacer replaces row 0. The old dead
    // `ix < first` condition mounted ALL rows from 0 on every render.
    expect(visibleRowWindow(positions, heights, 500, 200, OVERDRAW)).toEqual({ first: 1, last: 4 });
  });

  it("treats a bottom exactly at the window start as crossing", () => {
    // top 520: windowStart 200 — row 1's bottom is exactly 200, which counts
    // as crossing (>=), so first is 1, not 2.
    expect(visibleRowWindow(positions, heights, 520, 200, OVERDRAW)).toEqual({ first: 1, last: 4 });
  });

  it("skips rows entirely below the window plus overdraw", () => {
    // top 0, height 10: window [-320, 330]. Row 4's top (400) is past 330,
    // so last is 3.
    expect(visibleRowWindow(positions, heights, 0, 10, OVERDRAW)).toEqual({ first: 0, last: 3 });
  });

  it("returns an empty window for an empty list", () => {
    expect(visibleRowWindow([], [], 0, 600, OVERDRAW)).toEqual({ first: 0, last: -1 });
  });

  it("pins first to the last row when a stale top sits past the content", () => {
    // A view.top the scroller has not clamped yet (content shrank): every
    // row is above the window. first pins to the last row so the spacer
    // math stays inside positions[]; the next scroll event corrects.
    expect(visibleRowWindow(positions, heights, 2000, 600, OVERDRAW)).toEqual({ first: 4, last: 4 });
  });

  it("treats the first row whose bottom crosses the window start as first", () => {
    // top 450, height 200: window [130, 970]. Row 0's bottom (100) is above
    // 130, row 1's (200) is not — first is 1.
    expect(visibleRowWindow(positions, heights, 450, 200, OVERDRAW)).toEqual({ first: 1, last: 4 });
  });

  it("uses per-row heights, not a uniform estimate", () => {
    // One tall row then short rows: positions 0,300,340,380,420.
    const uneven = [0, 300, 340, 380, 420];
    const unevenHeights = [300, 40, 40, 40, 40];
    // top 650, height 200: window [330, 1170]. The tall row's bottom (300)
    // is above 330, so first is 1 — with uniform 100px rows the same window
    // would start at 3.
    expect(visibleRowWindow(uneven, unevenHeights, 650, 200, OVERDRAW)).toEqual({ first: 1, last: 4 });
    const uniform = [0, 100, 200, 300, 400];
    expect(visibleRowWindow(uniform, [100, 100, 100, 100, 100], 650, 200, OVERDRAW)).toEqual({ first: 3, last: 4 });
  });
});

// ---------------------------------------------------------------------------
// Ticket 18 � transcript rows (ports of the desktop test names in �6)
// ---------------------------------------------------------------------------

import {
  FLAVOUR_WORDS,
  PendingQueuedTurns,
  SavedViewportCache,
  captureSavedViewport,
  captureViewportAnchor,
  flavourSeed,
  flavourWord,
  formatElapsed,
  ownTurnObservesPrompt,
  ownTurnReleasedForRestore,
  parseForRow,
  resolveViewportAnchor,
  selectionDragAutoscrolls,
  SelectionDragTracker,
  selectionScrollStep,
  sendingBridge,
  sentMentionDisplay,
  userResizeCurve,
  userResizeDurationMs,
  type OwnTurnAnchor,
  type SavedViewport,
  type TranscriptRow as Row18,
} from "../src/lib/transcript";

describe("working trailer helpers (transcript.rs:1886-1952)", () => {
  it("flavour_words_rotate_every_seven_seconds", () => {
    const seed = flavourSeed("chat-1");
    // The index advances one step per FLAVOUR_ROTATE_SECS of elapsed time.
    expect(flavourWord(seed, 0)).toBe(flavourWord(seed, 6));
    expect(flavourWord(seed, 7)).not.toBe(flavourWord(seed, 0));
    expect(flavourWord(seed, 7)).toBe(FLAVOUR_WORDS[(((seed + 1) % 21) + 21) % 21]);
    // The full 21-word cycle repeats after 147s.
    expect(flavourWord(seed, 147)).toBe(flavourWord(seed, 0));
    // Negative elapsed clamps to zero, never wraps backwards.
    expect(flavourWord(seed, -30)).toBe(flavourWord(seed, 0));
  });

  it("elapsed_format_scales_from_seconds_to_days (transcript.rs)", () => {
    const cases: Array<[number, string]> = [
      [-5, "0s"],
      [0, "0s"],
      [59, "59s"],
      [60, "1m 0s"],
      [92, "1m 32s"],
      [3_599, "59m 59s"],
      [3_600, "1h 0m"],
      [4_800, "1h 20m"],
      [6_000, "1h 40m"],
      [86_399, "23h 59m"],
      [86_400, "1d 0h"],
      [183_845, "2d 3h"],
    ];
    for (const [secs, expected] of cases) {
      expect(formatElapsed(secs), `elapsed seconds: ${secs}`).toBe(expected);
    }
  });

  it("sending_bridge_holds_until_the_turn_outdates_the_send", () => {
    // No send in flight: never sending.
    expect(sendingBridge(null, null)).toBe(false);
    expect(sendingBridge(null, 1000)).toBe(false);
    // A send with no turn row: the round-trip window � sending.
    expect(sendingBridge(1000, null)).toBe(true);
    // The turn predates the send (the row still carries the PREVIOUS turn):
    // sending until the new turn actually begins.
    expect(sendingBridge(1000, 999)).toBe(true);
    expect(sendingBridge(1000, 1000)).toBe(true);
    expect(sendingBridge(1000, 1001)).toBe(false);
  });
});

describe("selectionScrollStep (transcript.rs:142)", () => {
  const bounds = { top: 0, bottom: 600 };

  it("selection_scroll_ramps_at_viewport_edges", () => {
    // Dead centre: no scroll.
    expect(selectionScrollStep(bounds, { x: 10, y: 300 })).toBe(0);
    // Top edge: negative (toward the document top), ramping as t�.
    expect(selectionScrollStep(bounds, { x: 10, y: 0 })).toBe(-24);
    expect(selectionScrollStep(bounds, { x: 10, y: 18 })).toBe(-24 * 0.25);
    expect(selectionScrollStep(bounds, { x: 10, y: 36 })).toBe(0);
    // Bottom edge: positive.
    expect(selectionScrollStep(bounds, { x: 10, y: 600 })).toBe(24);
    expect(selectionScrollStep(bounds, { x: 10, y: 582 })).toBe(24 * 0.25);
    // The edge band is capped at a third of the viewport.
    expect(selectionScrollStep({ top: 0, bottom: 60 }, { x: 0, y: 0 })).toBe(-24);
    expect(selectionScrollStep({ top: 0, bottom: 0 }, { x: 0, y: 0 })).toBe(0);
  });
});

describe("selection drag tracker (ticket 78)", () => {
  it("window pointermove never arms — only a scroller press arms", () => {
    const drag = new SelectionDragTracker();
    // The live bug: a hold with micro-drift on the titlebar/composer fires
    // window pointermoves with buttons held; those must not arm the tracker.
    drag.move(1, 10, 10);
    drag.move(1, 12, 14);
    expect(drag.position).toBeNull();
    // A primary press on non-interactive content inside the scroller arms.
    drag.press(5, 5);
    expect(drag.position).toEqual({ x: 5, y: 5 });
  });

  it("tracks an armed drag, then clears on release, cancel, and interactive press", () => {
    const drag = new SelectionDragTracker();
    drag.press(5, 5);
    drag.move(1, 40, 40);
    expect(drag.position).toEqual({ x: 40, y: 40 });
    // Buttons released: disarm (a stray later move cannot resurrect it).
    drag.move(0, 41, 41);
    expect(drag.position).toBeNull();
    drag.move(1, 60, 60);
    expect(drag.position).toBeNull();
    // pointerup / pointercancel clear an armed drag.
    drag.press(5, 5);
    drag.clear();
    expect(drag.position).toBeNull();
    // A press on interactive content disarms instead of arming.
    drag.press(5, 5);
    drag.pressInteractive();
    expect(drag.position).toBeNull();
    drag.move(1, 60, 60);
    expect(drag.position).toBeNull();
  });

  it("gates the tick on a non-collapsed selection", () => {
    expect(selectionDragAutoscrolls(null)).toBe(false);
    expect(selectionDragAutoscrolls({ isCollapsed: true, rangeCount: 1 })).toBe(false);
    expect(selectionDragAutoscrolls({ isCollapsed: false, rangeCount: 0 })).toBe(false);
    expect(selectionDragAutoscrolls({ isCollapsed: false, rangeCount: 1 })).toBe(true);
    expect(selectionDragAutoscrolls({ isCollapsed: false, rangeCount: 2 })).toBe(true);
  });
});

describe("user fold resize spec (transcript.rs:1178-1192)", () => {
  it("user_resize_duration_scales_with_distance_and_stays_bounded", () => {
    expect(userResizeDurationMs(0)).toBe(220);
    expect(userResizeDurationMs(100)).toBe(252);
    expect(userResizeDurationMs(-50)).toBe(220);
    expect(userResizeDurationMs(2000)).toBe(850);
    // Short folds ease-out; large folds ease-in-out.
    expect(userResizeCurve(400)).toBe("easeOut");
    expect(userResizeCurve(501)).toBe("easeInOut");
  });

  it("long_prompts_collapse_and_short_ones_do_not", () => {
    expect(userMessageNeedsCollapse("short")).toBe(false);
    expect(userMessageNeedsCollapse("five\nlines\nexactly\nhere\nnow")).toBe(false);
    expect(userMessageNeedsCollapse("six\nlines\nright\nhere\nnow\nok")).toBe(true);
    expect(userMessageNeedsCollapse("x".repeat(401))).toBe(true);
  });
});

describe("jump / restick / live-anchor gates (transcript.rs:74, :199, :3113)", () => {
  it("jump_button_stays_available_when_scrolling_down_until_near_bottom", () => {
    // Hidden: not yet past the 320px offering threshold.
    expect(jumpVisibility(false, 320)).toBe(false);
    expect(jumpVisibility(false, 500)).toBe(true);
    // Hysteresis: once shown, it stays until AT_BOTTOM_PX (2).
    expect(jumpVisibility(true, 320)).toBe(true);
    expect(jumpVisibility(true, 3)).toBe(true);
    expect(jumpVisibility(true, 2)).toBe(false);
    expect(jumpVisibility(true, 1)).toBe(false);
  });

  it("restick_is_direction_aware", () => {
    // Returning toward the bottom inside the 70px band re-sticks.
    expect(shouldRestick(60, 100)).toBe(true);
    expect(shouldRestick(0, 5)).toBe(true);
    // A small wheel-up notch NEAR the bottom stays inside the band but moves
    // AWAY from it: re-sticking would make the pin unbreakable.
    expect(shouldRestick(20, 10)).toBe(false);
    expect(shouldRestick(2, 0)).toBe(false);
    // Past the band, direction is irrelevant.
    expect(shouldRestick(71, 100)).toBe(false);
  });

  it("only_a_stream_at_the_bottom_gets_a_hard_end_anchor", () => {
    expect(shouldAnchorLiveStream(true, 2, true)).toBe(true);
    // Unpinned, or gliding back toward the bottom: the normal spring.
    expect(shouldAnchorLiveStream(false, 2, true)).toBe(false);
    expect(shouldAnchorLiveStream(true, 3, true)).toBe(false);
    // Not streaming: no hard anchor.
    expect(shouldAnchorLiveStream(true, 2, false)).toBe(false);
  });
});

describe("diffRows (transcript.rs:1651)", () => {
  const mk = (id: string, version = 0): Row18 => ({
    id,
    version,
    turnStart: false,
    rowKind: { kind: "errorChip", message: id },
    entryId: "e",
    timestamp: null,
    copyText: null,
  });

  it("diff_rows_appends_and_middle_edits", () => {
    expect(diffRows([mk("a")], [mk("a")])).toBeNull();
    expect(diffRows([mk("a")], [mk("a"), mk("b")])).toEqual([1, 0, 1]);
    expect(diffRows([mk("a"), mk("b"), mk("c")], [mk("a"), mk("c")])).toEqual([1, 1, 0]);
    // An in-place content change: same ids, new version ? splice of one.
    expect(diffRows([mk("a"), mk("b"), mk("c")], [mk("a"), mk("b", 7), mk("c")])).toEqual([1, 1, 1]);
    // Same id/version but different timestamp content (the settle bit):
    const settled = { ...mk("b"), version: mk("b").version ^ 0x40000000 };
    expect(diffRows([mk("a"), mk("b"), mk("c")], [mk("a"), settled, mk("c")])).toEqual([1, 1, 1]);
  });

  it("diff_handles_live_to_split_growth", () => {
    // A streaming tail splits into block rows while the prefix stays put:
    // live rows e#p.0 / e#p.0.0, then one more block appears.
    const live = [mk("e#p.0"), mk("e#p.0.0")];
    const grown = [mk("e#p.0"), mk("e#p.0.0"), mk("e#p.0.1")];
    expect(diffRows(live, grown)).toEqual([2, 0, 1]);
    // The live?complete flip changes every version but no id: an in-place
    // remeasure, not a splice.
    const flipped = grown.map((row) => ({ ...row, version: row.version + 1 }));
    expect(diffRows(grown, flipped)).toEqual([0, 3, 3]);
  });

  it("timestamp_strip_lands_on_the_last_settled_row", () => {
    const source = "# T\n\nbody";
    const streaming = entry("a1", [textPart("p0", source)], { status: "streaming" });
    const live = rowsForEntry(streaming, { parse });
    expect(live.every((row) => row.timestamp === null)).toBe(true);
    expect(live.every((row) => row.copyText === null)).toBe(true);
    const settled = rowsForEntry(entry("a1", [textPart("p0", source)]), { parse });
    // Only the LAST settled row carries the timestamp + copy affordance.
    expect(settled.map((row) => row.timestamp === null)).toEqual([true, false]);
    expect(settled[settled.length - 1]!.copyText).toBe(source);
    // Identical settles are diff-stable (the cache case).
    const settledAgain = rowsForEntry(entry("a1", [textPart("p0", source)]), { parse });
    expect(diffRows(settled, settledAgain)).toBeNull();
    // The live→settled flip keeps every id but changes the diff keys (the
    // streaming bit, the settle bit): an in-place remeasure, never a splice.
    const flipped = diffRows(live, settled);
    expect(flipped).toEqual([0, live.length, settled.length]);
  });
});

describe("sentMentionDisplay (composer.rs:1316, projected chips)", () => {
  const link = (path: string, label = path.split("/").pop()!) =>
    `[${label}](zeron-file:${encodeURIComponent(path).replaceAll("%2F", "/")})`;

  it("user_rows_project_file_mentions_into_chips", () => {
    // Plain prompts take the zero-work path.
    expect(sentMentionDisplay("no mentions here")).toBeNull();
    const raw = `look at ${link("src/lib/foo.ts")} please`;
    const projected = sentMentionDisplay(raw);
    expect(projected).not.toBeNull();
    // The chip: non-breaking side bearings around `@basename`.
    expect(projected!.display).toBe("look at \u00a0@foo.ts\u00a0 please");
    expect(projected!.mentions).toHaveLength(1);
    expect(projected!.mentions[0]).toMatchObject({ path: "src/lib/foo.ts", isDir: false });
    // The chip's range covers exactly the projected label run.
    const span = projected!.mentions[0]!;
    expect(projected!.display.slice(span.start, span.end)).toBe("\u00a0@foo.ts\u00a0");
    // The row model carries the projection.
    const user = entry("u1", [textPart("t0", raw)], { role: "user" });
    const [row] = rowsForEntry(user, { parse });
    expect(row!.rowKind).toMatchObject({ kind: "user" });
    if (row!.rowKind.kind === "user") {
      expect(row!.rowKind.text).toBe(projected!.display);
      expect(row!.rowKind.mentions).toHaveLength(1);
    }
  });

  it("duplicate basenames take the shortest unique suffix; dirs keep their slash", () => {
    const raw = `${link("a/util.ts")} and ${link("b/util.ts")}`;
    const projected = sentMentionDisplay(raw);
    expect(projected!.display).toContain("\u00a0@a/util.ts\u00a0");
    expect(projected!.display).toContain("\u00a0@b/util.ts\u00a0");
    const dir = sentMentionDisplay(link("src/lib/", "lib"));
    expect(dir!.mentions[0]).toMatchObject({ path: "src/lib/", isDir: true });
    // Non-mention links and unsafe paths are left untouched.
    expect(sentMentionDisplay("see [docs](https://x.dev)")).toBeNull();
    expect(sentMentionDisplay("[evil](zeron-file:..%2F..%2Fetc)")).toBeNull();
  });

  it("message_copy_keeps_authored_text_and_excludes_tool_traces", () => {
    const e = entry("a", [textPart("t0", "one"), toolPart("c0", exec("ls")), textPart("t1", "  two  ")]);
    // Authored bytes preserved, blank parts dropped, tools excluded.
    expect(assistantCopyText(e)).toBe("one\n\n  two  ");
    expect(assistantCopyText(entry("b", [toolPart("c0", exec("ls"))]))).toBeNull();
    // A mention-carrying prompt copies its PROJECTED text.
    const user = entry("u1", [textPart("t0", `hi ${link("src/a.ts")}`)], { role: "user" });
    const [row] = rowsForEntry(user, { parse });
    expect(row!.copyText).toBe(row!.rowKind.kind === "user" ? row!.rowKind.text : null);
  });
});

describe("parseForRow (transcript.rs:1557-1650)", () => {
  it("streaming parses mended, settling hands off the exact live tree", () => {
    const state = new Map();
    // A fresh live part: one full parse, nothing stable yet.
    const first = parseForRow(state, "k", "one\n\ntwo", true);
    expect(first.outcome.kind).toBe("incremental");
    if (first.outcome.kind === "incremental") {
      expect(first.outcome.parsedBytes).toBe("one\n\ntwo".length);
      expect(first.outcome.stablePrefixBlocks).toBe(0);
    }
    // A prefix extension: the reparse tail is the appended bytes, and the
    // leading paragraph block survives untouched.
    const second = parseForRow(state, "k", "one\n\ntwo three", true);
    expect(second.outcome.kind).toBe("incremental");
    if (second.outcome.kind === "incremental") {
      expect(second.outcome.parsedBytes).toBe(" three".length);
      expect(second.outcome.stablePrefixBlocks).toBe(1);
    }
    // Identical re-delivery keeps tree identity (no re-render).
    expect(parseForRow(state, "k", "one\n\ntwo three", true).tree).toBe(second.tree);
    // The live→complete handoff ADOPTS the live tree when sources match —
    // the split rows then share the tree the unsplit row painted.
    const settled = parseForRow(state, "k", "one\n\ntwo three", false);
    expect(settled.outcome.kind).toBe("handoff");
    expect(settled.tree).toBe(second.tree);
    // And the settled entry then serves from cache.
    expect(parseForRow(state, "k", "one\n\ntwo three", false).outcome.kind).toBe("cached");
    // A changed settled source re-parses in full, unmended.
    const fresh = parseForRow(state, "k", "plain *now*", false);
    expect(fresh.outcome.kind).toBe("full");
    // The streaming path mends hanging markers (display-only closers).
    const mended = parseForRow(new Map(), "m", "**bold", true);
    const runs = mended.tree.blocks[0]?.block;
    expect(runs !== undefined && runs.kind === "paragraph").toBe(true);
    if (runs !== undefined && runs.kind === "paragraph") {
      expect(runs.runs.some((run) => run.style.bold === true)).toBe(true);
    }
  });
});

describe("ViewportAnchor resolve (transcript.rs:2349-2395)", () => {
  const rows: Row18[] = [
    { id: "a#0", entryId: "a", turnStart: true, rowKind: { kind: "errorChip", message: "" }, version: 0, timestamp: null, copyText: null },
    { id: "a#1", entryId: "a", turnStart: false, rowKind: { kind: "errorChip", message: "" }, version: 0, timestamp: null, copyText: null },
    { id: "b#0", entryId: "b", turnStart: true, rowKind: { kind: "errorChip", message: "" }, version: 0, timestamp: null, copyText: null },
    { id: "c#0", entryId: "c", turnStart: true, rowKind: { kind: "errorChip", message: "" }, version: 0, timestamp: null, copyText: null },
  ];

  it("captures the first row crossing the viewport top, then resolves exactly", () => {
    const anchor = captureViewportAnchor(rows, 150, [0, 100, 200, 300], [100, 100, 100, 100]);
    expect(anchor).toMatchObject({ rowId: "a#1", fallbackIx: 1, offsetInRow: 50 });
    const resolved = resolveViewportAnchor(anchor!, rows, false);
    expect(resolved).toEqual({ itemIx: 1, offsetInItem: 50 });
  });

  it("falls back to the same entry's nearest row, then the clamped index", () => {
    // The anchored row disappeared (a streaming block reshaped).
    const anchor = { rowId: "a#1", entryId: "a", fallbackIx: 1, offsetInRow: 50 };
    const reshaped = rows.filter((row) => row.id !== "a#1");
    // While the replay is pending, fallbacks are disabled: no restore.
    expect(resolveViewportAnchor(anchor, reshaped, false)).toBeNull();
    const fallback = resolveViewportAnchor(anchor, reshaped, true);
    expect(fallback).toEqual({ itemIx: 0, offsetInItem: 0 });
    // Entry gone entirely: the clamped index.
    const gone = resolveViewportAnchor({ rowId: "x", entryId: "zz", fallbackIx: 9, offsetInRow: 4 }, rows, true);
    expect(gone).toEqual({ itemIx: 3, offsetInItem: 0 });
    expect(resolveViewportAnchor({ rowId: "x", entryId: "zz", fallbackIx: 0, offsetInRow: 0 }, [], true)).toBeNull();
  });

  it("saved viewports follow the tail when pinned, anchor otherwise", () => {
    const positions = [0, 100, 200, 300];
    const heights = [100, 100, 100, 100];
    expect(captureSavedViewport([], 0, positions, heights, true, 0, null)).toBeNull();
    const pinned = captureSavedViewport(rows, 0, positions, heights, true, 0, null);
    expect(pinned).toEqual({ kind: "followTail" });
    const own: OwnTurnAnchor = { chatId: "c1", messageId: "m1", held: true, positioned: true, seenPrompt: true };
    const escaped = captureSavedViewport(rows, 150, positions, heights, false, 700, own);
    expect(escaped).toMatchObject({ kind: "anchored", distanceFromBottom: 700, ownTurn: own });
    if (escaped !== null && escaped.kind === "anchored") {
      // Restoring releases the hold � the reservation, not the auto-follow.
      expect(ownTurnReleasedForRestore(escaped.ownTurn!)).toMatchObject({
        held: false,
        positioned: false,
        seenPrompt: true,
      });
    }
  });

  it("own_turn keeps the runway while the prompt was never seen", () => {
    const anchor: OwnTurnAnchor = { chatId: "c", messageId: "m", held: true, positioned: false, seenPrompt: false };
    expect(ownTurnObservesPrompt(anchor, false)).toBe(true);
    const seen: OwnTurnAnchor = { ...anchor, seenPrompt: true };
    expect(ownTurnObservesPrompt(seen, true)).toBe(true);
    // Once seen, a later disappearance is terminal.
    expect(ownTurnObservesPrompt(seen, false)).toBe(false);
  });
});

describe("PendingQueuedTurns (transcript.rs:2307)", () => {
  const rowOf = (entryId: string, turnStart: boolean): Row18 => ({
    id: entryId,
    version: 0,
    turnStart,
    rowKind: { kind: "errorChip", message: "" },
    entryId,
    timestamp: null,
    copyText: null,
  });

  it("registers inert and consumes the newest materialized row", () => {
    const turns = new PendingQueuedTurns();
    turns.register("c1", "m1");
    turns.register("c1", "m2");
    expect(turns.size).toBe(2);
    // Nothing materialized yet: inert — the visible turn is untouched.
    expect(turns.takeLatestMaterialized("c1", [rowOf("other", true)])).toBeNull();
    expect(turns.size).toBe(2);
    // Both land in one doc frame: the newest (the last registered) owns the
    // runway, matching consecutive immediate sends.
    const rows = [rowOf("m1", true), rowOf("m2", true)];
    expect(turns.takeLatestMaterialized("c1", rows)).toBe("m2");
    expect(turns.size).toBe(0);
    // Re-registering an existing id refreshes it to the back (newest).
    turns.register("c1", "m1");
    turns.register("c1", "m2");
    turns.register("c1", "m1");
    expect(turns.takeLatestMaterialized("c1", rows)).toBe("m1");
    // A non-turn-start row with the same entry id does not materialize.
    turns.register("c2", "q1");
    expect(turns.takeLatestMaterialized("c2", [rowOf("q1", false)])).toBeNull();
    expect(turns.size).toBe(1);
    // Another chat's rows never match.
    expect(turns.takeLatestMaterialized("c3", [rowOf("q1", true)])).toBeNull();
  });

  it("bounds itself to MAX_PENDING_QUEUED_TURNS", () => {
    const turns = new PendingQueuedTurns();
    for (let ix = 0; ix < 300; ix++) {
      turns.register("c", `m${ix}`);
    }
    expect(turns.size).toBe(256);
  });
});

describe("SavedViewportCache (transcript.rs:2401)", () => {
  it("is per-chat, LRU-bounded, and refreshes on save", () => {
    const cache = new SavedViewportCache();
    const tail: SavedViewport = { kind: "followTail" };
    for (let ix = 0; ix < 256; ix++) {
      cache.save(`chat-${ix}`, tail);
    }
    cache.save("chat-old", tail);
    expect(cache.get("chat-old")).toEqual(tail);
    // Saving enough to overflow evicts the least-recently-used — chat-0
    // goes while chat-old (saved most recently) stays.
    for (let ix = 0; ix < 255; ix++) {
      cache.save(`other-${ix}`, tail);
    }
    expect(cache.get("chat-0")).toBeUndefined();
    expect(cache.get("chat-old")).toEqual(tail);
    cache.clear();
    expect(cache.get("chat-old")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// Ticket 40 — transcript stability: replay baseline re-arms, rows never
// empty mid-session, the open-group estimate, the reservation's slack +
// fold-expansion terms (ports of the desktop tests at transcript.rs:7851,
// :7862, :7894, :10392)
// ---------------------------------------------------------------------------

import {
  estimateRowHeight,
  ownTurnReservationFloor,
  userFoldExpansionHeight,
  type UserFoldState,
} from "../src/components/transcript";
import { StickController } from "../src/components/stick-controller";
import {
  OWN_SEND_SCROLL_SLACK_PX,
  OWN_SEND_TOP_INSET_PX,
  SPACE_LG,
  TITLEBAR_HEIGHT,
  TOOL_GROUP_HEADER_HEIGHT,
  type ToolDetail,
  type ToolItem,
} from "../src/lib/transcript";
import { ToolGroupMotionStore, TOOL_CONNECTOR_REVEAL_MS, type FoldState } from "../src/lib/tool-motion";
import {
  computeToolMeasurementKeys,
  effectiveToolDetail,
  EMPTY_TOOL_GEOMETRY_STATE,
  pruneStaleToolMeasurements,
  toolGroupGeometry,
  type ToolGroupEstimateContext,
  type ToolGroupGeometryState,
} from "../src/lib/tool-group-geometry";

/** One collapsible tool-group row, built through the real row model. */
function toolGroupRow(entryId: string, commands: string[], streamingTail = false): TranscriptRow {
  const e = entry(
    entryId,
    commands.map((command, ix) => toolPart(`c${ix}`, exec(command))),
    streamingTail ? { status: "streaming" } : {},
  );
  const rows = rowsForEntry(e, { parse });
  const group = rows.find((row) => row.rowKind.kind === "toolGroup");
  if (group === undefined) {
    throw new Error("expected a tool group row");
  }
  return group;
}

/**
 * The component's open resolution (tool-group.tsx:119-127), mirrored so the
 * tests assert what the row would actually render. The test env has no
 * `matchMedia`, so reduced motion is false — the same branch the component
 * takes there.
 */
function resolvedGroupOpen(motion: ToolGroupMotionStore, row: TranscriptRow, at: number): boolean {
  if (row.rowKind.kind !== "toolGroup") {
    throw new Error("expected toolGroup");
  }
  const starts = motion.revealOf(row.id)?.starts ?? [];
  const arrivalPending = starts.some((start) => start != null && at - start < TOOL_CONNECTOR_REVEAL_MS);
  return motion.groupFold(row.id)?.open ?? (row.rowKind.autoOpen || arrivalPending);
}

describe("replay baseline re-arms per replay (ticket 40)", () => {
  it("tool_groups_stay_closed_on_populated_chat_attach", () => {
    const motion = new ToolGroupMotionStore();
    const row = toolGroupRow("tools", ["pwd"]);
    // Repeated attach/replay (chat-b, chat-a, chat-b on the desktop): each
    // cycle is the resubscribe shape — populated baseline → transient empty
    // window (replay pending) → the reset frame lands with the baseline
    // re-armed.
    for (let cycle = 0; cycle < 3; cycle += 1) {
      motion.sync([row], true);
      motion.sync([], false, true);
      motion.sync([row], true);
      const at = performance.now();
      expect(resolvedGroupOpen(motion, row, at)).toBe(false);
      motion.noteRendered(row.id, false, 0);
      const reveal = motion.revealOf(row.id);
      expect(reveal).not.toBeNull();
      // History flashed open on its first render — never.
      expect(reveal!.renderedOpen).toBe(false);
      // History replayed a stale closing tween — never.
      expect(reveal!.renderedHeight).toBe(0);
      expect(reveal!.headerStartedAt).toBeNull();
      expect([...reveal!.starts].every((start) => start == null)).toBe(true);
    }
  });

  it("tool_groups_stay_closed_after_rapid_new_chat_navigation", () => {
    const motion = new ToolGroupMotionStore();
    const row = toolGroupRow("tools", ["pwd"]);
    motion.sync([row], true);
    // Navigate away during the previous close animation: a fold pinned
    // closed with live tween clocks (a stale 120px closing tween).
    motion.toggleGroupFold(row.id, 0, false);
    motion.toggleGroupFold(row.id, 120, false);
    expect(motion.groupFold(row.id)?.open).toBe(false);
    expect(motion.groupFold(row.id)?.toggledAt).not.toBeNull();
    // Rapid navigation lands the replay without waiting the tween out.
    motion.sync([], false, true);
    motion.sync([row], true);
    const fold = motion.groupFold(row.id);
    expect(fold).not.toBeNull();
    // The user pin survives; the stale closing tween never resumes.
    expect(fold!.open).toBe(false);
    expect(fold!.toggledAt).toBeNull();
    expect(fold!.disclosureAt).toBeNull();
    const at = performance.now();
    expect(resolvedGroupOpen(motion, row, at)).toBe(false);
    motion.noteRendered(row.id, false, 0);
    const reveal = motion.revealOf(row.id)!;
    expect(reveal.renderedOpen).toBe(false);
    expect(reveal.renderedHeight).toBe(0);
    expect([...reveal.starts].every((start) => start == null)).toBe(true);
  });

  it("tool_group_navigation_keeps_user_pins_and_new_arrivals", () => {
    const motion = new ToolGroupMotionStore();
    const row = toolGroupRow("tools", ["pwd"]);
    motion.sync([row], true);
    // The user EXPANDED the group.
    motion.toggleGroupFold(row.id, 0, false);
    // Cached replay lands without an intervening pending frame.
    motion.sync([row], true);
    expect(motion.groupFold(row.id)?.open).toBe(true);
    const at = performance.now();
    expect(resolvedGroupOpen(motion, row, at)).toBe(true);
    motion.noteRendered(row.id, true, 34);
    const reveal = motion.revealOf(row.id)!;
    expect(reveal.renderedOpen).toBe(true);
    expect([...reveal.starts].every((start) => start == null)).toBe(true);

    // A genuinely NEW streamed arrival staggers in: the same group grows a
    // chip and only the new chip gets a start (old_count comes from the
    // previous LIVE rows, never a wiped map).
    const grown = toolGroupRow("tools", ["pwd", "ls"]);
    motion.sync([grown], false);
    const grownStarts = motion.revealOf(row.id)!.starts;
    expect(grownStarts[0] == null).toBe(true);
    expect(grownStarts[1]).not.toBeNull();
    expect(grownStarts[1]!).toBeGreaterThan(at - 1);

    // A brand-new group on a new streaming entry: every chip animates.
    const fresh = toolGroupRow("live-tools", ["ls"], true);
    motion.sync([grown, fresh], false);
    const freshReveal = motion.revealOf("live-tools#g0");
    expect(freshReveal).not.toBeNull();
    expect(freshReveal!.headerStartedAt).not.toBeNull();
    expect([...freshReveal!.starts].every((start) => start != null)).toBe(true);
  });

  it("a transient empty window never wipes the live sets; an authoritative empty does", () => {
    const motion = new ToolGroupMotionStore();
    const row = toolGroupRow("tools", ["pwd"]);
    motion.sync([row], false);
    expect(motion.revealOf(row.id)).not.toBeNull();
    // The resubscribe window: rows momentarily empty while replaying — the
    // reset has not landed; counts and reveals must survive it.
    motion.sync([], false, true);
    expect(motion.revealOf(row.id)).not.toBeNull();
    // An authoritative empty (replay "empty") is genuine: cleanup fires.
    motion.sync([], false, false);
    expect(motion.revealOf(row.id)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Ticket 70 — the shared tool-group geometry contract: the estimator and the
// renderer resolve ONE analytic height (32px rail rows, never the 38px
// standalone chip), and cached tool-row measurements are bounded by the
// semantic inputs they were taken under.
// ---------------------------------------------------------------------------

/** A fabricated tool item — the shared resolver's input grain. */
function toolItem(fields: Partial<ToolItem> = {}): ToolItem {
  return {
    call: exec("pwd"),
    isError: false,
    resolved: true,
    detail: null,
    invocation: null,
    outputRef: null,
    outputBytes: null,
    diffRef: null,
    subagentRef: null,
    subagentStatus: null,
    subagentTail: null,
    isThought: false,
    ...fields,
  };
}

function outputDetail(lines: readonly string[]): ToolDetail {
  return { kind: "output", lines, truncatedBy: 0 };
}

/** A fabricated collapsible tool-group row. */
function toolGroupRowFrom(id: string, tools: readonly ToolItem[], autoOpen: boolean): TranscriptRow {
  return {
    id,
    version: 1,
    turnStart: false,
    rowKind: { kind: "toolGroup", tools, autoOpen },
    entryId: id,
    timestamp: null,
    copyText: null,
  };
}

/** An aged fold (no tween clock) — the settled pin. */
function settledFold(open: boolean): FoldState {
  return { open, epoch: 1, from: 0, toggledAt: null, disclosureAt: null };
}

/** A geometry-state stub with the group fold pinned (aged — no tween clock). */
function pinnedGroupState(open: boolean, overrides: Partial<ToolGroupGeometryState> = {}): ToolGroupGeometryState {
  return { ...EMPTY_TOOL_GEOMETRY_STATE, groupFold: () => settledFold(open), ...overrides };
}

function estimateWith(row: TranscriptRow, state: ToolGroupGeometryState, now = 0, reduced = false): number {
  const ctx: ToolGroupEstimateContext = { state, now, reduced };
  return estimateRowHeight(row, ctx);
}

describe("open-group height estimate (tickets 40 + 70 — transcript.rs:96-136, :6000-6023)", () => {
  it("a closed group estimates its header only", () => {
    expect(estimateRowHeight(toolGroupRow("t", ["pwd", "ls"]))).toBe(TOOL_GROUP_HEADER_HEIGHT);
  });

  it("an auto-open group estimates the open height (header + rail rows)", () => {
    // Independent rail geometry: 26 header + 2 body top pad + 32 per rail
    // row — the old chipsHeight-based expectation (38px standalone rows)
    // self-confirmed the +6px/tool error this ticket removes.
    expect(estimateRowHeight(toolGroupRow("t", ["pwd"], true))).toBe(60);
    expect(estimateRowHeight(toolGroupRow("t", ["pwd", "ls"], true))).toBe(92);
  });

  it("rail_group_estimate_matches_rendered_geometry", () => {
    // The settled open collapsible group is header 26 + body pad 2 + 32 per
    // rail row; closed is the header. The renderer's own resolver agrees.
    const oneOpen = toolGroupRow("t", ["pwd"], true);
    const threeOpen = toolGroupRow("t", ["pwd", "ls", "cat"], true);
    expect(estimateRowHeight(oneOpen)).toBe(60); // 26 + 2 + 32
    expect(estimateRowHeight(threeOpen)).toBe(124); // 26 + 2 + 96
    expect(estimateRowHeight(toolGroupRow("t", ["pwd", "ls", "cat"]))).toBe(26);
    if (threeOpen.rowKind.kind !== "toolGroup") {
      throw new Error("expected toolGroup");
    }
    expect(
      toolGroupGeometry({
        rowId: threeOpen.id,
        tools: threeOpen.rowKind.tools,
        autoOpen: threeOpen.rowKind.autoOpen,
        state: EMPTY_TOOL_GEOMETRY_STATE,
        now: 0,
        reduced: false,
      }).totalHeight,
    ).toBe(124);
  });

  it("a user-pinned-open group estimates the open height; a pinned-closed one the header", () => {
    const row = toolGroupRow("t", ["pwd", "ls"]);
    expect(estimateWith(row, pinnedGroupState(true))).toBe(92);
    // A user pin CLOSED overrides the auto-open rule.
    expect(estimateWith(toolGroupRow("t", ["pwd", "ls"], true), pinnedGroupState(false))).toBe(
      TOOL_GROUP_HEADER_HEIGHT,
    );
  });

  it("a live thought chip's detail rides the open estimate", () => {
    const e = entry(
      "t",
      [toolPart("c0", exec("pwd")), { kind: "reasoning", id: "r0", text: "thinking\nharder" }],
      { status: "streaming" },
    );
    const rows = rowsForEntry(e, { parse });
    const group = rows.find((row) => row.rowKind.kind === "toolGroup")!;
    if (group.rowKind.kind !== "toolGroup") {
      throw new Error("expected toolGroup");
    }
    const thought = group.rowKind.tools[1]!;
    expect(thought.isThought).toBe(true);
    expect(thought.resolved).toBe(false);
    expect(thought.detail).not.toBeNull();
    // A live thought opens its detail by default; the rail rows are 32px.
    const expected = 26 + 2 + 32 * 2 + detailHeight(thought.detail!);
    expect(estimateWith(group, pinnedGroupState(true))).toBe(expected);
    // Without the pin the group still auto-opens (streaming tail).
    expect(estimateRowHeight(group)).toBe(expected);
  });

  it("a spawn-only group still estimates its unwrapped chips", () => {
    const spawn: ToolCall = { kind: "unknown", name: "Agent: scan repo" };
    const e = entry("t", [toolPart("c0", spawn)]);
    const rows = rowsForEntry(e, { parse });
    expect(estimateRowHeight(rows[0]!)).toBe(chipsHeight(1));
  });
});

describe("tool-group shared geometry (ticket 70)", () => {
  it("group_geometry_resolves_pins_arrivals_and_effective_details", () => {
    const one = [toolItem()];
    const base = { rowId: "g", tools: one, autoOpen: false };

    // Default: closed header only.
    const closed = toolGroupGeometry({ ...base, state: EMPTY_TOOL_GEOMETRY_STATE, now: 0, reduced: false });
    expect(closed.open).toBe(false);
    expect(closed.totalHeight).toBe(26);

    // Explicit pins win over row data, both ways.
    expect(
      toolGroupGeometry({ ...base, state: pinnedGroupState(true), now: 0, reduced: false }).totalHeight,
    ).toBe(60); // 26 + 2 + 32
    expect(
      toolGroupGeometry({ ...base, autoOpen: true, state: pinnedGroupState(false), now: 0, reduced: false })
        .totalHeight,
    ).toBe(26);

    // Detail pin open: invocation (1 line → 31) + doc detail (1 line → 31) +
    // the unfetched blob affordance (24) ride the 32px rail row.
    const withRefs = [
      toolItem({ invocation: outputDetail(["pwd"]), detail: outputDetail(["summary"]), outputRef: "g/out" }),
    ];
    const detailPinned = pinnedGroupState(true, {
      detailFold: (key) => (key === "g#d0" ? settledFold(true) : null),
    });
    const openDetail = toolGroupGeometry({ rowId: "g", tools: withRefs, autoOpen: false, state: detailPinned, now: 0, reduced: false });
    expect(openDetail.detailOpens).toEqual([true]);
    expect(openDetail.rowHeights).toEqual([32 + 31 + 31 + 24]);
    expect(openDetail.totalHeight).toBe(26 + 2 + 118);
    // The estimator resolves the SAME state to the SAME height.
    expect(estimateWith(toolGroupRowFrom("g", withRefs, false), detailPinned)).toBe(openDetail.totalHeight);

    // Unresolved thought: detail open by default; a pinned-closed detail
    // fold overrides the default (the 2-line thought detail adds 49).
    const thought = [toolItem({ isThought: true, resolved: false, detail: outputDetail(["a", "b"]) })];
    const thoughtOpen = toolGroupGeometry({ rowId: "g", tools: thought, autoOpen: false, state: pinnedGroupState(true), now: 0, reduced: false });
    expect(thoughtOpen.detailOpens).toEqual([true]);
    expect(thoughtOpen.totalHeight).toBe(26 + 2 + 32 + 49);
    const thoughtClosed = toolGroupGeometry({
      rowId: "g",
      tools: thought,
      autoOpen: false,
      state: pinnedGroupState(true, { detailFold: () => settledFold(false) }),
      now: 0,
      reduced: false,
    });
    expect(thoughtClosed.detailOpens).toEqual([false]);
    expect(thoughtClosed.totalHeight).toBe(60);

    // Fetched payload + affordance: the ready blob is the effective detail
    // (3 lines → 67) and the shown affordance slot empties.
    const fetched = outputDetail(["l1", "l2", "l3"]);
    const fetchedState = pinnedGroupState(true, {
      detailFold: () => settledFold(true),
      blobFetchOf: (ref) => (ref === "g/out" ? { state: "ready", detail: fetched } : null),
      blobOrderOf: (ref) => (ref === "g/out" ? 1 : 0),
    });
    const fetchedGeometry = toolGroupGeometry({ rowId: "g", tools: withRefs, autoOpen: false, state: fetchedState, now: 0, reduced: false });
    expect(effectiveToolDetail(withRefs[0]!, fetchedState)).toBe(fetched);
    expect(fetchedGeometry.affordances).toEqual([null]);
    expect(fetchedGeometry.totalHeight).toBe(26 + 2 + 32 + 31 + 67);

    // Arrival-only open: no autoOpen, no pin, but a live reveal start holds
    // the group open (here past the 360ms row reveal, mid connector draw).
    const arrivalState: ToolGroupGeometryState = {
      ...EMPTY_TOOL_GEOMETRY_STATE,
      revealOf: () => ({ headerStartedAt: null, starts: [1000], shimmerStartedAt: null, renderedOpen: null, renderedHeight: 0 }),
    };
    const arrival = toolGroupGeometry({ ...base, state: arrivalState, now: 1400, reduced: false });
    expect(arrival.arrivalPending).toBe(true);
    expect(arrival.open).toBe(true);
    expect(arrival.totalHeight).toBe(60);
    expect(arrival.motionActive).toBe(true);
    // A FUTURE start still reads as pending, with the row reveal at 0.
    const future = toolGroupGeometry({ ...base, state: arrivalState, now: 500, reduced: false });
    expect(future.open).toBe(true);
    expect(future.totalHeight).toBe(26 + 2);
    // Reduced motion: no arrival, no reveal — the pin-less group stays shut.
    const reducedArrival = toolGroupGeometry({ ...base, state: arrivalState, now: 1400, reduced: true });
    expect(reducedArrival.arrivalPending).toBe(false);
    expect(reducedArrival.open).toBe(false);
    expect(reducedArrival.totalHeight).toBe(26);

    // Timestamped mid-tween: the fold clock lerps the body toward the open
    // target; past TOOL_FOLD it saturates. Reduced motion snaps.
    const tweenState = pinnedGroupState(true);
    const tweenFold: FoldState = { open: true, epoch: 1, from: 0, toggledAt: 1000, disclosureAt: null };
    const liveTween: ToolGroupGeometryState = { ...tweenState, groupFold: () => tweenFold };
    const mid = toolGroupGeometry({ ...base, state: liveTween, now: 1070, reduced: false });
    expect(mid.totalHeight).toBeGreaterThan(26);
    expect(mid.totalHeight).toBeLessThan(60);
    expect(mid.motionActive).toBe(true);
    expect(toolGroupGeometry({ ...base, state: liveTween, now: 1140, reduced: false }).totalHeight).toBe(60);
    expect(toolGroupGeometry({ ...base, state: liveTween, now: 1070, reduced: true }).totalHeight).toBe(60);
  });

  it("fetched_detail_geometry_is_read_only_and_respects_request_recency", async () => {
    // beginBlobFetch's 20s timeout timer reads `window`.
    vi.stubGlobal("window", globalThis);
    try {
      const motion = new ToolGroupMotionStore();
      const tool = toolItem({
        detail: outputDetail(["summary"]),
        outputRef: "g/out",
        outputBytes: 2048,
        diffRef: "g/d.diff",
      });
      const input = { rowId: "g", tools: [tool], autoOpen: true, now: 0, reduced: false };

      // Repeated geometry reads of unchanged state: no fetch, no order churn,
      // no version bump, and the doc detail stays effective. The affordance
      // offers the DIFF first (the richer upgrade).
      const version0 = motion.getVersion();
      const g0 = toolGroupGeometry({ ...input, state: motion });
      toolGroupGeometry({ ...input, state: motion });
      toolGroupGeometry({ ...input, state: motion });
      expect(motion.getVersion()).toBe(version0);
      expect(motion.blobFetchOf("g/out")).toBeNull();
      expect(effectiveToolDetail(tool, motion)).toBe(tool.detail);
      expect(g0.affordances[0]).toEqual({ ref: "g/d.diff", label: "Show full diff", loading: false });

      // The user's fetch is IN FLIGHT: the doc detail still shows, the
      // affordance reports loading — still no geometry side effects.
      let releaseDiff!: (text: string) => void;
      motion.beginBlobFetch("g/d.diff", () => new Promise<string>((resolve) => { releaseDiff = resolve; }));
      const loading = toolGroupGeometry({ ...input, state: motion });
      expect(loading.affordances[0]).toEqual({ ref: "g/d.diff", label: "Loading full diff…", loading: true });
      expect(effectiveToolDetail(tool, motion)).toBe(tool.detail);

      // Deferred completion: the fetched payload becomes effective, the
      // affordance slot hands over to the unfetched output — and reads STILL
      // do not mutate the store.
      releaseDiff(JSON.stringify({ path: "a.ts", oldText: "before", newText: "after" }));
      await new Promise((resolve) => setTimeout(resolve, 0));
      const diffFetch = motion.blobFetchOf("g/d.diff");
      if (diffFetch?.state !== "ready") {
        throw new Error("expected ready diff");
      }
      const versionReady = motion.getVersion();
      const g1 = toolGroupGeometry({ ...input, state: motion });
      toolGroupGeometry({ ...input, state: motion });
      expect(motion.getVersion()).toBe(versionReady);
      expect(effectiveToolDetail(tool, motion)).toBe(diffFetch.detail);
      expect(g1.affordances[0]).toEqual({ ref: "g/out", label: "Show full output (2 KB)", loading: false });

      // The output fetch completes: requested LATER, it wins the effective
      // detail; the ready-but-not-shown diff re-arms as the recency toggle.
      let releaseOutput!: (text: string) => void;
      motion.beginBlobFetch("g/out", () => new Promise<string>((resolve) => { releaseOutput = resolve; }));
      releaseOutput("l1\nl2\nl3");
      await new Promise((resolve) => setTimeout(resolve, 0));
      const outFetch = motion.blobFetchOf("g/out");
      if (outFetch?.state !== "ready") {
        throw new Error("expected ready output");
      }
      expect(effectiveToolDetail(tool, motion)).toBe(outFetch.detail);
      const toggled = toolGroupGeometry({ ...input, state: motion });
      expect(toggled.affordances[0]).toEqual({ ref: "g/d.diff", label: "Show full diff", loading: false });

      // The ready-recency click: re-"fetching" the READY diff ref re-orders
      // it WITHOUT a new fetch, and the effective detail follows the recency.
      let fetchedAgain = false;
      motion.beginBlobFetch("g/d.diff", () => {
        fetchedAgain = true;
        return Promise.resolve("unused");
      });
      expect(fetchedAgain).toBe(false);
      expect(motion.blobOrderOf("g/d.diff")).toBeGreaterThan(motion.blobOrderOf("g/out"));
      expect(effectiveToolDetail(tool, motion)).toBe(diffFetch.detail);
      expect(toolGroupGeometry({ ...input, state: motion }).affordances[0]).toEqual({
        ref: "g/out",
        label: "Show full output",
        loading: false,
      });
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("late_blob_completion_invalidates_only_affected_tool_measurement", async () => {
    vi.stubGlobal("window", globalThis);
    try {
      const motion = new ToolGroupMotionStore();
      const rowA = toolGroupRowFrom("a#g0", [toolItem({ detail: outputDetail(["summary"]), outputRef: "a/out" })], true);
      const rowB = toolGroupRowFrom("b#g0", [toolItem()], true);
      const mdRow: TranscriptRow = {
        id: "m",
        version: 1,
        turnStart: false,
        rowKind: { kind: "markdown", tree: { blocks: [] }, blockIx: 0 },
        entryId: "m",
        timestamp: null,
        copyText: null,
      };
      const rows = [rowA, rowB, mdRow];

      // Both tool rows + the markdown row hold cached measurements, tagged
      // under the CURRENT semantic inputs (the observer's write shape).
      const heights = new Map<string, number>([["a#g0", 150], ["b#g0", 92], ["m", 33]]);
      const measuredKeys = new Map<string, string>();
      for (const [id, key] of computeToolMeasurementKeys(rows, motion)) {
        measuredKeys.set(id, key);
      }

      // Unchanged inputs: nothing is invalidated, nothing is notified.
      const version0 = motion.getVersion();
      expect(pruneStaleToolMeasurements(heights, measuredKeys, computeToolMeasurementKeys(rows, motion))).toEqual([]);
      expect(heights.get("a#g0")).toBe(150);
      expect(motion.getVersion()).toBe(version0);

      // The UNMOUNTED row's in-flight fetch completes: no DOM, no observer —
      // the semantic key is what notices. Only row A's cached height drops.
      let release!: (text: string) => void;
      motion.beginBlobFetch("a/out", () => new Promise<string>((resolve) => { release = resolve; }));
      release("l1\nl2\nl3");
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(motion.blobFetchOf("a/out")?.state).toBe("ready");
      const dropped = pruneStaleToolMeasurements(heights, measuredKeys, computeToolMeasurementKeys(rows, motion));
      expect(dropped).toEqual(["a#g0"]);
      expect(heights.has("a#g0")).toBe(false);
      expect(heights.get("b#g0")).toBe(92); // unrelated tool row survives
      expect(heights.get("m")).toBe(33); // non-tool rows never carry keys
      // The analytic replacement already reflects the fetched payload
      // selection (the detail stays closed until pinned — renderer parity).
      expect(estimateRowHeight(rowA, { state: motion, now: 0, reduced: false })).toBe(60);

      // Unchanged follow-up inputs stop the invalidation: a second pass is a
      // no-op and the store was never notified by any of it.
      expect(pruneStaleToolMeasurements(heights, measuredKeys, computeToolMeasurementKeys(rows, motion))).toEqual([]);

      // The remount's fresh measurement re-tags under the current inputs and
      // is retained from then on.
      heights.set("a#g0", 127);
      measuredKeys.set("a#g0", computeToolMeasurementKeys(rows, motion).get("a#g0")!);
      expect(pruneStaleToolMeasurements(heights, measuredKeys, computeToolMeasurementKeys(rows, motion))).toEqual([]);
      expect(heights.get("a#g0")).toBe(127);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("own-send reservation terms (ticket 40 — transcript.rs:3483-3513)", () => {
  const fold = (fields: Partial<UserFoldState>): UserFoldState => ({
    open: false,
    epoch: 1,
    toggledAt: 0,
    durationMs: 400,
    from: 0,
    expansion: 220,
    ...fields,
  });

  it("the reservation value is inset − slack − expansion", () => {
    // The 2px slack reads as the app's bottom (below perception); the
    // expansion term carries the anchor row's live fold tween.
    const floor = ownTurnReservationFloor({ anchorTop: 100, lastTop: 260, viewport: 800, inset: 48, expansion: 0 });
    expect(floor).toBe(100 + 800 - (48 - OWN_SEND_SCROLL_SLACK_PX) - 260);
  });

  it("folding_releases_sent_turn_hold_without_removing_reservation (shape)", () => {
    // The anchor row's fold toggles open: the tween's height lands in the
    // anchor row (lastTop grows with it) while the expansion term grows in
    // lockstep — the floor still binds instead of collapsing with the tween.
    const viewport = 800;
    const inset = 48;
    const anchorTop = 100;
    const lastTop = 260;
    const expansion = 220;
    const before = ownTurnReservationFloor({ anchorTop, lastTop, viewport, inset, expansion: 0 });
    expect(before).toBeGreaterThan(0);
    const after = ownTurnReservationFloor({
      anchorTop,
      lastTop: lastTop + expansion,
      viewport,
      inset,
      expansion,
    });
    expect(after).toBeGreaterThan(0);
    expect(after).toBeCloseTo(before, 6);
  });

  it("the expansion term tracks the fold tween (transcript.rs:3490-3504)", () => {
    const opening = fold({ open: true, toggledAt: 0 });
    // A degenerate duration or reduced motion snaps to the target (the
    // desktop's `_` arm).
    expect(userFoldExpansionHeight(fold({ open: true, durationMs: 0, toggledAt: 0 }), 5000, false)).toBe(220);
    expect(userFoldExpansionHeight(opening, 5000, true)).toBe(220);
    // Opening: 0 → full expansion over the duration.
    expect(userFoldExpansionHeight(opening, 0, false)).toBe(0);
    const mid = userFoldExpansionHeight(opening, 200, false);
    expect(mid).toBeGreaterThan(0);
    expect(mid).toBeLessThan(220);
    expect(userFoldExpansionHeight(opening, 400, false)).toBe(220);
    // Show less decays the term back to zero.
    const closing = fold({ open: false, toggledAt: 0 });
    expect(userFoldExpansionHeight(closing, 0, false)).toBe(220);
    expect(userFoldExpansionHeight(closing, 400, false)).toBe(0);
  });

  it("unmeasured_rail_group_does_not_prematurely_fill_runway", () => {
    // The reservation's fill predicate (transcript.tsx's `reservationFilled`):
    // the held runway retires once the reply's natural content end reaches
    // `anchorTop + viewport − inset + 0.5`. For an UNMEASURED rail group the
    // estimate feeds that arithmetic — near the threshold, the removed
    // +6px/tool excess alone decides between retiring and holding. (This is
    // the ARITHMETIC proof; runtime retirement evidence needs a browser —
    // see the ticket's Comments.)
    const viewport = 800;
    const inset = StickController.ownSendInset(3);
    const anchorTop = 100;
    const threshold = anchorTop + viewport - inset + 0.5;
    const tools = ["t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10"];
    const group = toolGroupRow("reply", tools, true); // auto-open, unmeasured
    const lastNatural = estimateRowHeight(group);
    expect(lastNatural).toBe(26 + 2 + 32 * tools.length); // 348
    // Near the threshold: the corrected estimate does NOT fill the runway.
    const lastTop = threshold - lastNatural - 10;
    expect(lastTop + lastNatural >= threshold).toBe(false);
    // …where the old 38px/chip estimate (408) would have read as filled.
    const oldEstimate = 26 + 2 + 38 * tools.length;
    expect(lastTop + oldEstimate >= threshold).toBe(true);
    // After measurement the estimate IS the settled render height, so the
    // fill state does not flip when the measurement lands.
    expect(lastTop + 348 >= threshold).toBe(false);
  });

  it("a first send in an empty chat parks row 0 at 64 (echo start)", () => {
    // Row 0's own gap carries the chrome: the inset for row 0 is 0 and the
    // first row's gap is the 64px titlebar clearance (transcript.rs:3435-3445
    // — adding both parked a first prompt ~66px low, user report).
    expect(StickController.ownSendInset(0)).toBe(0);
    expect(StickController.ownSendInset(3)).toBe(OWN_SEND_TOP_INSET_PX);
    expect(TITLEBAR_HEIGHT + SPACE_LG + 10).toBe(64);
  });
});

// ---------------------------------------------------------------------------
// Ticket 68 — chat-switch fold memory (decision option 1): the explicit
// group/detail pins of a chat survive a REAL A→B→A surface destroy/restore
// through the bounded per-engine cache; restored pins carry no tween clocks
// and replay no arrival motion; genuine live arrivals still animate. The
// regressions mount the REAL TranscriptView against the REAL TranscriptStore
// (two chat identities, two engines), unmounting between visits — never
// sync(true) on one retained store (the ticket-40 suite above owns that
// level). Spies on the motion store and the stick controller record what the
// mounted consumer actually did; jsdom gaps are stubbed per-suite (the
// mounted replay suite's idiom): matchMedia, a deterministic rAF queue, a
// fake ResizeObserver, and a mocked performance.now. No JSX (createElement).
// ---------------------------------------------------------------------------

vi.mock("../src/state/appearance", () => ({
  useResolvedAppearance: () => "dark" as const,
}));

import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { EngineClient, EngineStatus } from "@zeron/engine-client";
import type { TranscriptUpdate } from "@zeron/proto";
import { TranscriptView } from "../src/components/transcript";
import { echoStore, savedViewportCache, TranscriptStore } from "../src/state/transcript-store";
import { transcriptFoldCache } from "../src/state/transcript-fold-state";

describe("chat-switch fold memory (ticket 68)", () => {
  /** A settled entry whose parts are one collapsible tool group. */
  function toolEntry(id: string, commands: string[]): SessionMessageEntry {
    return entry(
      id,
      commands.map((command, ix) => toolPart(`${id}#c${ix}`, exec(command))),
    );
  }

  /** A streaming entry — its single group row carries `autoOpen: true`. */
  function liveToolEntry(id: string, command: string): SessionMessageEntry {
    return entry(id, [toolPart(`${id}#c0`, exec(command))], { status: "streaming" });
  }

  /** An exec tool plus an unresolved-look thought (settled: resolved default). */
  function thoughtToolEntry(id: string, command: string): SessionMessageEntry {
    return entry(id, [
      toolPart(`${id}#c0`, exec(command)),
      { kind: "reasoning", id: `${id}#r0`, text: "thinking\nharder" },
    ]);
  }

  /** Resolve one chat's group row through the REAL row model. */
  function groupRowOf(chat: readonly SessionMessageEntry[], rowId: string): { tools: readonly ToolItem[]; autoOpen: boolean } {
    const rows: TranscriptRow[] = [];
    for (const item of chat) {
      rows.push(...rowsForEntry(item, { parse }));
    }
    const row = rows.find((candidate) => candidate.id === rowId);
    if (row === undefined || row.rowKind.kind !== "toolGroup") {
      throw new Error(`expected tool group row ${rowId}`);
    }
    return { tools: row.rowKind.tools, autoOpen: row.rowKind.autoOpen };
  }

  /** The shared resolver's verdict for one group row under the mounted store. */
  function geometryOf(rowId: string, chat: readonly SessionMessageEntry[], motion: ToolGroupMotionStore) {
    const group = groupRowOf(chat, rowId);
    return toolGroupGeometry({
      rowId,
      tools: group.tools,
      autoOpen: group.autoOpen,
      state: motion,
      now,
      reduced: false,
    });
  }

  // ── Controllable client + jsdom stubs (the mounted replay suite's idiom) ──

  interface WatchSlot {
    onItem: (item: TranscriptUpdate, ctx: { generation: number }) => void;
    onEnd?: (error: unknown) => void;
  }

  class FakeClient {
    readonly status = { state: "connected" } as unknown as EngineStatus;
    readonly engineKey: string | null;
    readonly watches: WatchSlot[] = [];
    readonly #statusListeners = new Set<(status: EngineStatus) => void>();

    constructor(engineKey: string | null) {
      this.engineKey = engineKey;
    }

    onStatus(listener: (status: EngineStatus) => void): () => void {
      this.#statusListeners.add(listener);
      return () => {
        this.#statusListeners.delete(listener);
      };
    }

    watch(_method: string, _params: unknown, handlers: WatchSlot): { cancel: () => void } {
      this.watches.push(handlers);
      return { cancel: () => {} };
    }

    call(): Promise<never> {
      return Promise.resolve({} as never);
    }

    emit(update: TranscriptUpdate, generation = 1): void {
      const slot = this.watches[this.watches.length - 1];
      if (slot === undefined) {
        throw new Error("no watch registered");
      }
      slot.onItem(update, { generation });
    }
  }

  class FakeResizeObserver {
    readonly callback: ResizeObserverCallback;
    readonly observed = new Set<Element>();

    constructor(callback: ResizeObserverCallback) {
      this.callback = callback;
    }

    observe(el: Element): void {
      this.observed.add(el);
    }

    unobserve(el: Element): void {
      this.observed.delete(el);
    }

    disconnect(): void {
      this.observed.clear();
    }
  }

  let now = 10_000;
  const rafQueue = new Map<number, FrameRequestCallback>();
  let rafSeq = 0;

  const realSync = ToolGroupMotionStore.prototype.sync;
  const realRestoreFolds = ToolGroupMotionStore.prototype.restoreExplicitFolds;
  const realNoteRendered = ToolGroupMotionStore.prototype.noteRendered;
  const realAttach = StickController.prototype.attach;
  const realSnapToEnd = StickController.prototype.snapToEnd;

  const probe = {
    motion: null as ToolGroupMotionStore | null,
    stick: null as StickController | null,
    el: null as HTMLElement | null,
    snaps: 0,
    /** Call-through order log: "restore" (fold pins) vs "snap" (viewport). */
    order: [] as string[],
  };

  beforeAll(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      rafSeq += 1;
      rafQueue.set(rafSeq, callback);
      return rafSeq;
    }) as typeof requestAnimationFrame;
    globalThis.cancelAnimationFrame = ((handle: number) => {
      rafQueue.delete(handle);
    }) as typeof cancelAnimationFrame;
  });

  afterAll(() => {
    delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
    delete (globalThis as { matchMedia?: unknown }).matchMedia;
    delete (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
  });

  beforeEach(() => {
    now = 10_000;
    probe.motion = null;
    probe.stick = null;
    probe.el = null;
    probe.snaps = 0;
    probe.order = [];
    transcriptFoldCache.clear();
    savedViewportCache.clear();
    echoStore.reset();
    vi.spyOn(performance, "now").mockImplementation(() => now);
    vi.spyOn(ToolGroupMotionStore.prototype, "sync").mockImplementation(function (
      this: ToolGroupMotionStore,
      rows: readonly TranscriptRow[],
      baseline: boolean,
      replaying = false,
    ) {
      probe.motion = this;
      return realSync.call(this, rows, baseline, replaying);
    });
    vi.spyOn(ToolGroupMotionStore.prototype, "restoreExplicitFolds").mockImplementation(function (
      this: ToolGroupMotionStore,
      saved: Parameters<ToolGroupMotionStore["restoreExplicitFolds"]>[0],
    ) {
      probe.order.push("restore");
      return realRestoreFolds.call(this, saved);
    });
    vi.spyOn(ToolGroupMotionStore.prototype, "noteRendered").mockImplementation(function (
      this: ToolGroupMotionStore,
      rowId: string,
      open: boolean,
      bodyHeight: number,
    ) {
      return realNoteRendered.call(this, rowId, open, bodyHeight);
    });
    vi.spyOn(StickController.prototype, "attach").mockImplementation(function (this: StickController, el: HTMLElement) {
      probe.stick = this;
      probe.el = el;
      return realAttach.call(this, el);
    });
    vi.spyOn(StickController.prototype, "snapToEnd").mockImplementation(function (this: StickController) {
      probe.order.push("snap");
      probe.snaps += 1;
      return realSnapToEnd.call(this);
    });
  });

  afterEach(() => {
    while (mounted.length > 0) {
      mounted.pop()!.unmount();
    }
    vi.restoreAllMocks();
    document.body.replaceChildren();
    rafQueue.clear();
    transcriptFoldCache.clear();
    savedViewportCache.clear();
    echoStore.reset();
  });

  // ── The mounted harness ─────────────────────────────────────────────────

  interface MountedChat {
    readonly store: TranscriptStore;
    readonly client: FakeClient;
    el(): HTMLElement;
    unmount(): void;
  }

  const mounted: MountedChat[] = [];

  function mountChat(options: {
    docId: string;
    engineKey: string | null;
    /** Pre-seeded history: the outlet hands the view the store only once its
     *  first frame has landed, so the FIRST render is already the loaded
     *  commit — the strongest ordering case for restore-before-viewport. */
    entries?: readonly SessionMessageEntry[];
    strict?: boolean;
  }): MountedChat {
    const client = new FakeClient(options.engineKey);
    const store = new TranscriptStore(client as unknown as EngineClient, options.docId);
    if (options.entries !== undefined) {
      client.emit({ contextUsage: null, reset: [...options.entries] });
    }
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const tree = createElement(TranscriptView, {
      client: client as unknown as EngineClient,
      docId: options.docId,
      deviceId: "dev",
      store,
    });
    act(() => {
      root.render(options.strict === true ? createElement(StrictMode, null, tree) : tree);
    });
    let unmounted = false;
    const handle: MountedChat = {
      store,
      client,
      el() {
        if (probe.el === null) {
          throw new Error("the stick controller never attached a scroller");
        }
        return probe.el;
      },
      unmount() {
        if (unmounted) {
          return;
        }
        unmounted = true;
        act(() => {
          root.unmount();
        });
        container.remove();
        store.dispose();
      },
    };
    mounted.push(handle);
    // Deterministic scroller geometry on the element (jsdom lays out nothing).
    const el = handle.el();
    let top = 0;
    Object.defineProperty(el, "clientHeight", { configurable: true, get: () => 600 });
    Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => 4000 });
    Object.defineProperty(el, "scrollTop", {
      configurable: true,
      get: () => top,
      set: (value: number) => {
        top = value;
      },
    });
    return handle;
  }

  it("explicit_fold_choices_survive_real_chat_switch", () => {
    const chatA = [thoughtToolEntry("tools-a", "pwd"), liveToolEntry("live-a", "ls")];
    const chatB = [thoughtToolEntry("tools-b", "cat")];

    // ── Visit A: pins are made through the real mounted motion store. ─────
    const visitA = mountChat({ docId: "chat-a", engineKey: "e1", entries: chatA });
    const motionA = probe.motion!;
    expect(motionA).not.toBeNull();
    // Before any pin, the streaming tail group follows live auto-open.
    expect(geometryOf("live-a#g0", chatA, motionA).open).toBe(true);
    // The user closes the streaming group (a live click clock exists) and
    // opens the settled thought's detail (its resolved default is closed).
    act(() => {
      motionA.toggleGroupFold("live-a#g0", 92, true);
      motionA.toggleDetailFold("tools-a#g0#d1", 80, false);
    });
    expect(motionA.groupFold("live-a#g0")!.open).toBe(false);
    expect(motionA.groupFold("live-a#g0")!.toggledAt).not.toBeNull();
    expect(motionA.detailFold("tools-a#g0#d1")!.open).toBe(true);
    expect(motionA.detailFold("tools-a#g0#d1")!.toggledAt).not.toBeNull();

    // ── A→B: the surface state is destroyed; pins are captured at teardown.
    visitA.unmount();
    const savedA = transcriptFoldCache.restore("e1", "chat-a");
    expect(savedA).not.toBeNull();
    expect(savedA!.groups.get("live-a#g0")).toBe(false);
    expect(savedA!.details.get("tools-a#g0#d1")).toBe(true);

    const visitB = mountChat({ docId: "chat-b", engineKey: "e1", entries: chatB });
    const motionB = probe.motion!;
    expect(motionB).not.toBe(motionA);
    act(() => {
      motionB.toggleGroupFold("tools-b#g0", 26, false);
    });
    visitB.unmount();
    expect(transcriptFoldCache.restore("e1", "chat-b")!.groups.get("tools-b#g0")).toBe(true);

    // ── B→A: a FRESH store and surface restore only A's pins. ────────────
    // Watch only this mount's call order: the restored pins land BEFORE the
    // viewport restore's height estimates (the render-phase restore precedes
    // the scroller's first snap — the outlet's hand-off means the first
    // render is already the loaded commit).
    probe.order = [];
    const revisitA = mountChat({ docId: "chat-a", engineKey: "e1", entries: chatA });
    expect(probe.order.indexOf("restore")).toBeGreaterThanOrEqual(0);
    expect(probe.order.indexOf("restore")).toBeLessThan(probe.order.indexOf("snap"));
    const motionA2 = probe.motion!;
    expect(motionA2).not.toBe(motionA);
    const groupFold = motionA2.groupFold("live-a#g0")!;
    expect(groupFold.open).toBe(false);
    expect(groupFold.toggledAt).toBeNull();
    expect(groupFold.disclosureAt).toBeNull();
    const detailFold = motionA2.detailFold("tools-a#g0#d1")!;
    expect(detailFold.open).toBe(true);
    expect(detailFold.toggledAt).toBeNull();
    // The remembered closed pin overrides the streaming auto-open default
    // through the ONE shared resolver — no second resolution path.
    expect(geometryOf("live-a#g0", chatA, motionA2).open).toBe(false);
    // The restored detail pin overrides the thought's resolved default.
    expect(geometryOf("tools-a#g0", chatA, motionA2).detailOpens[1]).toBe(true);
    // History the user never pinned stays auto-following — the replay
    // baseline manufactures no explicit pin.
    expect(motionA2.groupFold("tools-a#g0")).toBeNull();
    expect(geometryOf("tools-a#g0", chatA, motionA2).open).toBe(false);
    // B's pins never leak into A's namespace.
    expect(motionA2.groupFold("tools-b#g0")).toBeNull();
    revisitA.unmount();

    // ── The same doc under another engine: no collision. ──────────────────
    const otherEngine = mountChat({ docId: "chat-a", engineKey: "e2", entries: chatA });
    const motionE2 = probe.motion!;
    expect(motionE2.groupFold("live-a#g0")).toBeNull();
    expect(motionE2.detailFold("tools-a#g0#d1")).toBeNull();
    otherEngine.unmount();
  });

  it("restored_folds_do_not_replay_arrival_motion", () => {
    const chatA = [toolEntry("tools-a", ["pwd"])];

    // ── Visit A: pin the group closed mid-tween (rapid navigation case). ──
    const visitA = mountChat({ docId: "chat-a", engineKey: "e1", entries: chatA });
    const motionA = probe.motion!;
    act(() => {
      motionA.toggleGroupFold("tools-a#g0", 0, false); // opens
      motionA.toggleGroupFold("tools-a#g0", 120, false); // closes with a live tween
    });
    expect(motionA.groupFold("tools-a#g0")!.open).toBe(false);
    expect(motionA.groupFold("tools-a#g0")!.toggledAt).not.toBeNull();
    visitA.unmount();
    // The capture stripped the tween clocks.
    const savedA = transcriptFoldCache.restore("e1", "chat-a")!;
    expect(savedA.groups.get("tools-a#g0")).toBe(false);

    // ── Revisit A: the pin returns settled; no arrival motion replays. ────
    const revisitA = mountChat({ docId: "chat-a", engineKey: "e1", entries: chatA });
    const motion = probe.motion!;
    const fold = motion.groupFold("tools-a#g0")!;
    expect(fold.open).toBe(false);
    expect(fold.toggledAt).toBeNull();
    expect(fold.disclosureAt).toBeNull();
    const reveal = motion.revealOf("tools-a#g0")!;
    expect(reveal.headerStartedAt).toBeNull();
    expect(reveal.starts.every((start) => start === null)).toBe(true);
    expect(reveal.shimmerStartedAt).toBeNull();
    // The mounted render's rendered-open flip seeds no tween (the first
    // noteRendered only records; the arrival window holds the flip path).
    act(() => {
      motion.noteRendered("tools-a#g0", false, 0);
    });
    expect(motion.groupFold("tools-a#g0")!.toggledAt).toBeNull();

    // ── A genuinely live arrival after the restore still animates. ───────
    const grownChat = [toolEntry("tools-a", ["pwd", "ls"]), liveToolEntry("fresh-a", "cat")];
    act(() => {
      revisitA.client.emit(
        {
          contextUsage: null,
          upsert: [
            { after: null, entry: toolEntry("tools-a", ["pwd", "ls"]) },
            { after: "tools-a", entry: liveToolEntry("fresh-a", "cat") },
          ],
          append: [],
          remove: [],
          count: 2,
        },
        1,
      );
    });
    const starts = motion.revealOf("tools-a#g0")!.starts;
    // (A baseline start is an array hole — the file's loose `== null`
    // convention, the same one the resolver normalizes with `?? null`.)
    expect(starts[0] == null).toBe(true); // the replayed chip stays history
    expect(starts[1]).not.toBeNull(); // the live-grown chip staggered in
    expect(starts[1]!).toBeGreaterThan(now - 1);
    const freshReveal = motion.revealOf("fresh-a#g0")!;
    expect(freshReveal.headerStartedAt).not.toBeNull();
    expect(freshReveal.starts.every((start) => start !== null)).toBe(true);
    // The restored closed pin still governs openness while the arrival is
    // pending — a remembered false pin overrides the arrival default.
    const grownGeometry = geometryOf("tools-a#g0", grownChat, motion);
    expect(grownGeometry.arrivalPending).toBe(true);
    expect(grownGeometry.open).toBe(false);
    // An unpinned new group follows the live auto-open behavior.
    expect(geometryOf("fresh-a#g0", grownChat, motion).open).toBe(true);
    revisitA.unmount();
  });
});

// ---------------------------------------------------------------------------
// Ticket 71 — tool-fold scroll ownership + the animated thought close. The
// selected policy (recorded 2026-09-20): explicit group/detail clicks own
// the viewport (preserve the clicked header, release follow/hold, retain
// the reservation, cancel on wheel/touch/navigation); an automatic thought
// completion closes over the EXISTING 140ms EASE_OUT fold tween with the
// same ownership anchoring the viewport; automatic outer-group closure
// never moves a manually escaped viewport; the own-send reservation's
// lifecycle is unchanged. The pure policy lives in lib/tool-fold-scroll.ts;
// the store's seeding (sync) never fires under a pin, an armed arrival, or
// a replay baseline.
// ---------------------------------------------------------------------------

import { ChatArrivalWindow } from "../src/lib/chat-arrival";
import { TOOL_FOLD_MS, type AutomaticFoldTransition } from "../src/lib/tool-motion";
import {
  armToolFoldCompensation,
  automaticToolFoldCompensationArms,
  toolFoldCompensationDone,
  toolFoldCompensationWrite,
  type ToolFoldCompensation,
} from "../src/lib/tool-fold-scroll";
import { CHIP_CARD_HEIGHT, TOOL_TREE_ROW_HEIGHT } from "../src/lib/transcript";

describe("tool-fold scroll ownership + animated thought close (ticket 71)", () => {
  /** A minimal scroller: only the fields the controller reads and writes. */
  type MutableScroller = HTMLElement & {
    scrollTop: number;
    scrollHeight: number;
    clientHeight: number;
    addEventListener: () => void;
    removeEventListener: () => void;
    parentElement: null;
    fire: (type: string) => void;
  };

  function fakeScroller(scrollHeight: number, clientHeight: number): MutableScroller {
    const listeners = new Map<string, Set<() => void>>();
    const el = {
      scrollTop: 0,
      scrollHeight,
      clientHeight,
      addEventListener: (type: string, listener: () => void) => {
        const set = listeners.get(type) ?? new Set();
        set.add(listener);
        listeners.set(type, set);
      },
      removeEventListener: (type: string, listener: () => void) => {
        listeners.get(type)?.delete(listener);
      },
      parentElement: null,
      fire: (type: string) => {
        for (const listener of [...(listeners.get(type) ?? [])]) {
          listener();
        }
      },
    };
    return el as unknown as MutableScroller;
  }

  /** One thought-chip group row, through the real row model. */
  function thoughtRow(entryId: string, text: string, streaming: boolean): TranscriptRow {
    const e = entry(
      entryId,
      [{ kind: "reasoning", id: "r0", text }],
      streaming ? { status: "streaming" } : {},
    );
    const rows = rowsForEntry(e, { parse });
    const group = rows.find((row) => row.rowKind.kind === "toolGroup");
    if (group === undefined) {
      throw new Error("expected a tool group row");
    }
    return group;
  }

  /** The geometry of one thought row under a motion store, at `now`. */
  function thoughtGeometry(
    motion: ToolGroupMotionStore,
    row: TranscriptRow,
    now: number,
    reduced = false,
  ) {
    if (row.rowKind.kind !== "toolGroup") {
      throw new Error("expected toolGroup");
    }
    return toolGroupGeometry({
      rowId: row.id,
      tools: row.rowKind.tools,
      autoOpen: row.rowKind.autoOpen,
      state: motion,
      now,
      reduced,
    });
  }

  /** A settled entry whose parts are one collapsible tool group. */
  function ticket71ToolEntry(id: string, commands: string[]): SessionMessageEntry {
    return entry(
      id,
      commands.map((command, ix) => toolPart(`${id}#c${ix}`, exec(command))),
    );
  }

  it("tool_fold_preserves_clicked_header_and_reservation", () => {
    // ── The anchor math (pure): the clicked header stays at its screen
    // position across the fold tween's drift. ───────────────────────────
    const comp = armToolFoldCompensation({ rowId: "tools#g0", offsetInRow: 12, screenY: 300, now: 1000 });
    expect(comp.endsAt).toBe(1000 + TOOL_FOLD_MS);
    // At the click the anchor already sits where the user clicked it.
    expect(toolFoldCompensationWrite(comp, { rowTop: 500, scrollTop: 212 })).toBeNull();
    // A browser clamp near the scroll end moved the viewport 90px: the
    // write restores the header's position exactly.
    expect(toolFoldCompensationWrite(comp, { rowTop: 500, scrollTop: 302 })).toBe(212);
    // Rows spliced above re-target the anchor in content space; the screen
    // position is still the invariant.
    expect(toolFoldCompensationWrite(comp, { rowTop: 400, scrollTop: 112 })).toBeNull();
    expect(toolFoldCompensationWrite(comp, { rowTop: 400, scrollTop: 302 })).toBe(112);
    // The compensation stands down with the tween, not before.
    expect(toolFoldCompensationDone(comp, 1000 + TOOL_FOLD_MS - 1)).toBe(false);
    expect(toolFoldCompensationDone(comp, 1000 + TOOL_FOLD_MS)).toBe(true);

    // ── The runway state matrix on the REAL controller: the click releases
    // the hold and the pin while RETAINING any live reservation. ─────────
    const raf = vi.fn(() => 1);
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      for (const state of ["held", "released", "retired+pinned", "escaped"] as const) {
        const el = fakeScroller(4000, 600);
        const stick = new StickController({ onJumpVisibility: () => {} });
        stick.setGeometry(() => ({ anchor: null, filled: false, positions: [], transient: true }));
        stick.attach(el);
        if (state === "held" || state === "released") {
          stick.onOwnSend("chat", "prompt");
          expect(stick.ownTurn).not.toBeNull();
          if (state === "released") {
            stick.releaseOwnTurnHold();
            expect(stick.ownTurnHeld).toBe(false);
          } else {
            expect(stick.ownTurnHeld).toBe(true);
          }
        } else if (state === "retired+pinned") {
          stick.snapToEnd();
          expect(stick.ownTurn).toBeNull();
          expect(stick.pinned).toBe(true);
        } else {
          // Escaped: pinned at the end, then a scroll away breaks the pin.
          stick.snapToEnd();
          el.scrollTop = 100;
          el.fire("scroll");
          expect(stick.pinned).toBe(false);
        }

        // The click path's controller sequence (transcript.tsx's
        // onToolFoldNav): release follow/hold first, then arm.
        stick.beginScrollNavigation();
        const armed = armToolFoldCompensation({
          rowId: "tools#g0",
          offsetInRow: 12,
          screenY: 300,
          now: 2000,
        });
        expect(armed).not.toBeNull();
        expect(stick.ownTurnHeld).toBe(false);
        expect(stick.pinned).toBe(false);
        expect(
          stick.ownTurn !== null,
          `${state}: the reservation survives any release of its hold`,
        ).toBe(state === "held" || state === "released");
      }
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("thought_completion_obeys_explicit_pin_and_scroll_policy", () => {
    // ── The animated close: unresolved→resolved with no pin seeds the
    // detail fold's tween from the renderer-reported card height. ────────
    const motion = new ToolGroupMotionStore();
    const transitions: AutomaticFoldTransition[] = [];
    motion.onAutomaticFoldTransition((transition) => transitions.push(transition));
    const live = thoughtRow("think", "thinking hard", true);
    motion.sync([live], false);
    const openGeometry = thoughtGeometry(motion, live, 1000);
    expect(openGeometry.detailOpens[0]).toBe(true);
    const openCardHeight = openGeometry.rowHeights[0]! - TOOL_TREE_ROW_HEIGHT + CHIP_CARD_HEIGHT;
    expect(openCardHeight).toBeGreaterThan(CHIP_CARD_HEIGHT);
    // The renderer's per-paint report (the tween's `from`).
    motion.noteDetailRendered("think#g0#d0", openCardHeight);

    // The completion: the entry settles — the thought chip resolves.
    const completed = thoughtRow("think", "thinking hard", false);
    motion.sync([completed], false);
    const fold = motion.detailFold("think#g0#d0");
    expect(fold).not.toBeNull();
    expect(fold!.open, "no pin was invented").toBeNull();
    expect(fold!.toggledAt, "the close is animated, not a snap").not.toBeNull();
    expect(fold!.epoch, "the chip body stays mounted through the shrink").toBe(1);
    expect(fold!.from, "the tween starts from the rendered open height").toBe(openCardHeight);
    expect(transitions).toEqual([{ rowId: "think#g0", key: "think#g0#d0", toggledAt: fold!.toggledAt }]);

    // Mid-flight the shared geometry tweens; the end state is closed.
    const at = fold!.toggledAt!;
    const mid = thoughtGeometry(motion, completed, at + TOOL_FOLD_MS / 2);
    expect(mid.rowHeights[0]!).toBeGreaterThan(TOOL_TREE_ROW_HEIGHT);
    expect(mid.rowHeights[0]!).toBeLessThan(openGeometry.rowHeights[0]!);
    expect(mid.detailOpens[0], "the FINAL open state is closed mid-tween").toBe(false);
    const end = thoughtGeometry(motion, completed, at + 10_000);
    expect(end.rowHeights[0]!).toBe(TOOL_TREE_ROW_HEIGHT);
    expect(end.detailOpens[0]).toBe(false);

    // ── Explicit pins win in every case. ─────────────────────────────────
    for (const pinnedOpen of [true, false]) {
      const pinned = new ToolGroupMotionStore();
      pinned.sync([live], false);
      pinned.noteDetailRendered("think#g0#d0", openCardHeight);
      // The default is open (unresolved); one click pins it closed, a
      // second pins it back open — either way the pin exists.
      pinned.toggleDetailFold("think#g0#d0", openCardHeight, true);
      if (pinnedOpen) {
        pinned.toggleDetailFold("think#g0#d0", openCardHeight, false);
      }
      expect(pinned.detailFold("think#g0#d0")!.open).toBe(pinnedOpen);
      pinned.sync([completed], false);
      const after = pinned.detailFold("think#g0#d0")!;
      expect(after.open, "the completion never overrides a pin").toBe(pinnedOpen);
      expect(after.toggledAt, "the pin keeps its own click clock").not.toBeNull();
      const geometry = thoughtGeometry(pinned, completed, at + 10_000);
      expect(geometry.detailOpens[0]).toBe(pinnedOpen);
    }

    // ── Replay never impersonates a completion: the baseline resets the
    // tracker, so the resolved frame only re-records. ────────────────────
    const baseline = new ToolGroupMotionStore();
    baseline.sync([live], false);
    baseline.noteDetailRendered("think#g0#d0", openCardHeight);
    baseline.sync([completed], true);
    expect(baseline.detailFold("think#g0#d0"), "the replay baseline seeds no tween").toBeNull();

    // ── An armed chat-switch arrival renders the endpoint without a tween. ──
    const arrival = new ChatArrivalWindow();
    const arrived = new ToolGroupMotionStore(arrival);
    arrival.arm(performance.now());
    arrived.sync([live], false);
    arrived.noteDetailRendered("think#g0#d0", openCardHeight);
    arrived.sync([completed], false);
    expect(arrived.detailFold("think#g0#d0"), "an arrival frame never animates").toBeNull();

    // ── Reduced motion is deterministic: the seeded close snaps. ─────────
    const snap = thoughtGeometry(motion, completed, at + 1, true);
    expect(snap.rowHeights[0]!).toBe(TOOL_TREE_ROW_HEIGHT);
    expect(snap.detailOpens[0]).toBe(false);

    // ── The ownership gate (§2.3): an automatic transition arms only while
    // no other owner is live. ────────────────────────────────────────────
    const unowned = {
      pinned: false,
      ownTurnHeld: false,
      userFoldCompensating: false,
      escapeAnchor: false,
      pendingViewportRestore: false,
    };
    expect(automaticToolFoldCompensationArms(unowned)).toBe(true);
    for (const owner of [
      "pinned",
      "ownTurnHeld",
      "userFoldCompensating",
      "escapeAnchor",
      "pendingViewportRestore",
    ] as const) {
      expect(automaticToolFoldCompensationArms({ ...unowned, [owner]: true })).toBe(false);
    }
  });

  it("user_scroll_cancels_tool_fold_compensation", () => {
    const raf = vi.fn(() => 1);
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      const el = fakeScroller(4000, 600);
      // The surface's cancellation wiring: user input and navigation both
      // clear the armed compensation (transcript.tsx's onUserInput /
      // onNavigation).
      let comp: ToolFoldCompensation | null = null;
      const stick = new StickController({
        onJumpVisibility: () => {},
        onUserInput: () => {
          comp = null;
        },
        onNavigation: () => {
          comp = null;
        },
      });
      stick.attach(el);
      // Reading near the end, escaped (the state with no other owner):
      // pinned at the end first, then a scroll away breaks the pin.
      stick.snapToEnd();
      el.scrollTop = 3200;
      el.fire("scroll");
      expect(stick.pinned).toBe(false);

      // An automatic completion armed the reading anchor mid-flight.
      comp = armToolFoldCompensation({ rowId: "tools#g0", offsetInRow: 20, screenY: 500, now: performance.now() });
      // The shrink clamped the scroll end (content drifted down 90px): the
      // step's write restores the anchor exactly.
      el.scrollTop = 3350;
      expect(toolFoldCompensationWrite(comp!, { rowTop: 1500, scrollTop: el.scrollTop })).toBe(1020);

      // Wheel mid-fold: the user scroll cancels the compensation
      // synchronously and does not re-engage the pin.
      el.scrollTop = 3000;
      el.fire("scroll");
      expect(comp, "wheel/touch cancels the compensation immediately").toBeNull();
      expect(stick.pinned, "cancellation does not re-engage the pin").toBe(false);
      // A frame queued before the input neither writes nor resurrects: the
      // loop reads a cleared compensation and stands down before it ticks.
      expect(comp).toBeNull();

      // Re-arm, then explicit navigation (rail glide / a user fold toggle)
      // takes the viewport the same way.
      comp = armToolFoldCompensation({ rowId: "tools#g0", offsetInRow: 20, screenY: 500, now: performance.now() });
      expect(comp).not.toBeNull();
      stick.beginScrollNavigation();
      expect(comp, "navigation cancels the compensation").toBeNull();
      expect(stick.pinned).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("a mounted group click owns the viewport and retains the reservation (integration)", () => {
    // The mounted replay suite's jsdom stubs, scoped to this test: a
    // deterministic rAF queue (never auto-run — the compensation's loop
    // arms but never ticks), matchMedia, and the act environment.
    const rafQueue = new Map<number, FrameRequestCallback>();
    let rafSeq = 0;
    const actEnvironment = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    const priorMatchMedia = window.matchMedia;
    const priorRaf = globalThis.requestAnimationFrame;
    const priorResizeObserver = globalThis.ResizeObserver;
    actEnvironment.IS_REACT_ACT_ENVIRONMENT = true;
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      rafSeq += 1;
      rafQueue.set(rafSeq, callback);
      return rafSeq;
    }) as typeof requestAnimationFrame;
    class FakeResizeObserver {
      readonly callback: ResizeObserverCallback;
      constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
      }
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    }
    globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

    // The mounted harness's probe holder (assignment inside the spy closure
    // must stay visible to the test body).
    const probe: { stick: StickController | null; el: HTMLElement | null } = { stick: null, el: null };
    const realBeginNavigation = StickController.prototype.beginScrollNavigation;
    const beginNavigation = vi
      .spyOn(StickController.prototype, "beginScrollNavigation")
      .mockImplementation(function (this: StickController) {
        return realBeginNavigation.call(this);
      });
    const realAttach = StickController.prototype.attach;
    vi.spyOn(StickController.prototype, "attach").mockImplementation(function (
      this: StickController,
      el: HTMLElement,
    ) {
      probe.stick = this;
      probe.el = el;
      return realAttach.call(this, el);
    });
    let container: HTMLDivElement | null = null;
    let mountedRoot: ReturnType<typeof createRoot> | null = null;
    let mountedStore: TranscriptStore | null = null;
    try {
      // Fresh cross-chat caches: a saved viewport for this docId would
      // restore over the runway this test installs.
      echoStore.reset();
      savedViewportCache.clear();
      transcriptFoldCache.clear();
      const client = new FakeWatchClient("e1");
      const store = new TranscriptStore(client as unknown as EngineClient, "chat-a");
      mountedStore = store;
      client.emit({ contextUsage: null, reset: [ticket71ToolEntry("tools", ["pwd"])] });
      container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      mountedRoot = root;
      act(() => {
        root.render(
          createElement(TranscriptView, {
            client: client as unknown as EngineClient,
            docId: "chat-a",
            deviceId: "dev",
            store,
          }),
        );
      });
      if (probe.stick === null || probe.el === null) {
        throw new Error("the stick controller never attached a scroller");
      }
      const stick = probe.stick;
      const el = probe.el;
      let scrollTop = 0;
      Object.defineProperty(el, "clientHeight", { configurable: true, get: () => 600 });
      Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => 4000 });
      Object.defineProperty(el, "scrollTop", {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => {
          scrollTop = value;
        },
      });
      // The send lands on the MOUNTED surface (the real flow: the echo
      // arrives after the scroller, not with the first frame): the pending
      // echo row installs the own-send runway (the reservation).
      act(() => {
        echoStore.pushEcho({
          chatId: "chat-a",
          messageId: "prompt",
          startedAtMs: performance.now() - 100,
          text: "prompt",
          attachmentPaths: [],
        });
      });
      expect(stick.ownTurn, "the pending echo installed the runway").not.toBeNull();
      expect(stick.ownTurnHeld).toBe(true);

      // The click: the settled group's header (auto-open false) opens, and
      // the click owns the viewport before the fold state flips.
      const header = document.getElementById("tools#g0-hdr");
      expect(header).not.toBeNull();
      expect(beginNavigation).not.toHaveBeenCalled();
      act(() => {
        header!.click();
      });
      expect(beginNavigation).toHaveBeenCalledTimes(1);
      // The hold released; the reservation survived; the pin dropped.
      expect(stick.ownTurn, "the reservation survives the click").not.toBeNull();
      expect(stick.ownTurnHeld).toBe(false);
      expect(stick.pinned).toBe(false);
    } finally {
      const root = mountedRoot;
      if (root !== null) {
        act(() => {
          root.unmount();
        });
      }
      mountedStore?.dispose();
      vi.restoreAllMocks();
      if (container !== null) {
        container.remove();
      }
      document.body.replaceChildren();
      delete actEnvironment.IS_REACT_ACT_ENVIRONMENT;
      window.matchMedia = priorMatchMedia;
      globalThis.requestAnimationFrame = priorRaf;
      globalThis.ResizeObserver = priorResizeObserver;
      echoStore.reset();
      savedViewportCache.clear();
      transcriptFoldCache.clear();
      rafQueue.clear();
    }
  });
});

/** A minimal watch client for the mounted ticket-71 harness (jsdom). */
class FakeWatchClient {
  readonly status = { state: "connected" } as unknown as EngineStatus;
  readonly engineKey: string | null;
  #handlers: { onItem: (item: TranscriptUpdate, ctx: { generation: number }) => void } | null = null;

  constructor(engineKey: string | null) {
    this.engineKey = engineKey;
  }

  onStatus(): () => void {
    return () => {};
  }

  watch(_method: string, _params: unknown, handlers: { onItem: (item: TranscriptUpdate, ctx: { generation: number }) => void }): { cancel: () => void } {
    this.#handlers = handlers;
    return { cancel: () => {} };
  }

  call(): Promise<never> {
    return Promise.resolve({} as never);
  }

  emit(update: TranscriptUpdate, generation = 1): void {
    if (this.#handlers === null) {
      throw new Error("no watch registered");
    }
    this.#handlers.onItem(update, { generation });
  }
}

