/**
 * The transcript's pure model, ported 1:1 from the desktop:
 * - row model (`crates/ui/src/transcript.rs` `rows_for_entry`, `RowKind`,
 *   `top_gap_for`, `diff_rows`) — block-granularity rows with stable ids, so
 *   the virtualizer's row identity is continuous across the live→settled flip;
 * - tool chips (`zeron_proto::view` `tool_chip_content`/`tool_group_summary`,
 *   `ToolCall::is_subagent_spawn` from `crates/proto/src/agent.rs`);
 * - delta application (`crates/doc/src/transcript_delta.rs`
 *   `apply_transcript_frame`), made immutable so React can memoize per entry.
 */

import type {
  MessagePart,
  SessionMessageEntry,
  ToolCall,
  ToolDiff,
  ToolDiffStat,
  TranscriptFrame,
} from "@zeron/proto";
import type { IconName } from "@zeron/icons";
import { layout } from "@zeron/theme";
import { bodyHeight, DIFF_LINE_HEIGHT, truncateFileLines, type FileDiff } from "./diff";
import { blockFlatText, parseMarkdown, type Block, type BlockTree, type InlineRun, type InlineStyle } from "./markdown";
import { parseUserMessageImages, type UserImageAttachment } from "./attachments";
import { sentMentionDisplay, type SentMentionSpan } from "./mentions";
import { splitBadges, type MessageBadge } from "./badges";

/** `Theme::TITLEBAR_HEIGHT` (proto/layout.rs:44) — the overlay bar's height. */
export const TITLEBAR_HEIGHT = layout.chrome.titlebarHeight;
/** `Theme::TRANSCRIPT_FADE_BAND` (proto/layout.rs) — the edge fade ramp. */
export const TRANSCRIPT_FADE_BAND = layout.chrome.transcriptFadeBand;

// ---------------------------------------------------------------------------
// Shared view helpers (crates/proto/src/view.rs)
// ---------------------------------------------------------------------------

/** Collapse model-generated text onto ONE line for single-line surfaces. */
export function singleLine(text: string): string {
  return text.split(/\s+/).filter((piece) => piece.length > 0).join(" ");
}

function plural(n: number, one: string, many: string): string {
  return n === 1 ? `${n} ${one}` : `${n} ${many}`;
}

/** Per-kind chip label + one-line detail (view.rs `tool_chip_content`). */
export function toolChipContent(call: ToolCall): { label: string; detail: string } {
  const [label, detail] = toolChipContentRaw(call);
  return { label, detail: singleLine(detail) };
}

function toolChipContentRaw(call: ToolCall): [string, string] {
  switch (call.kind) {
    case "exec":
      return ["Run", call.command];
    case "readFile":
      return ["Read", call.path];
    case "writeFile":
      return ["Write", call.path];
    case "editFile":
      return ["Edit", call.path];
    case "applyPatch":
      return ["Patch", call.path ?? "workspace"];
    case "search":
      return ["Search", call.path !== null && call.path !== undefined ? `${call.pattern} in ${call.path}` : call.pattern];
    case "glob":
      return ["Glob", call.pattern];
    case "webFetch":
      return ["Fetch", call.url];
    case "webSearch":
      return ["Web", call.query];
    case "todo": {
      const done = call.items.filter((item) => item.done).length;
      return ["Todo", `${done}/${call.items.length} done`];
    }
    case "mcp":
      return ["MCP", `${call.server} · ${call.tool}`];
    case "unknown": {
      // Subagent spawns decode as Unknown named "Agent[: <description>]":
      // label them "Agent" with the description as the detail.
      if (call.name.startsWith("Agent: ")) {
        return ["Agent", call.name.slice("Agent: ".length)];
      }
      if (call.name === "Agent") {
        return ["Agent", ""];
      }
      return ["Tool", call.name];
    }
  }
}

/** The ToolGroup summary line — "Ran 3 commands · edited 2 files". */
export function toolGroupSummary(tools: readonly { call: ToolCall; isError: boolean }[]): string {
  let commands = 0;
  const edited: string[] = [];
  let reads = 0;
  let searches = 0;
  let fetches = 0;
  let todos = 0;
  let other = 0;
  let failed = 0;
  for (const { call, isError } of tools) {
    if (isError) {
      failed++;
    }
    switch (call.kind) {
      case "exec":
        commands++;
        break;
      case "writeFile":
      case "editFile":
        if (!edited.includes(call.path)) {
          edited.push(call.path);
        }
        break;
      case "applyPatch": {
        const path = call.path ?? "patch";
        if (!edited.includes(path)) {
          edited.push(path);
        }
        break;
      }
      case "readFile":
        reads++;
        break;
      case "search":
      case "glob":
      case "webSearch":
        searches++;
        break;
      case "webFetch":
        fetches++;
        break;
      case "todo":
        todos++;
        break;
      case "mcp":
      case "unknown":
        other++;
        break;
    }
  }
  const segments: string[] = [];
  if (commands > 0) {
    segments.push(`ran ${plural(commands, "command", "commands")}`);
  }
  if (edited.length > 0) {
    segments.push(`edited ${plural(edited.length, "file", "files")}`);
  }
  if (reads > 0) {
    segments.push(`read ${plural(reads, "file", "files")}`);
  }
  if (searches > 0) {
    segments.push(`searched ${plural(searches, "time", "times")}`);
  }
  if (fetches > 0) {
    segments.push(`fetched ${plural(fetches, "page", "pages")}`);
  }
  if (todos > 0) {
    segments.push("updated todos");
  }
  if (other > 0) {
    segments.push(`called ${plural(other, "tool", "tools")}`);
  }
  if (segments.length === 0) {
    segments.push(plural(tools.length, "tool", "tools"));
  }
  if (failed > 0) {
    segments.push(`${failed} failed`);
  }
  const summary = segments.join(" · ");
  // Capitalize the first segment only (zeron's style).
  return summary.length > 0 ? summary[0]!.toUpperCase() + summary.slice(1) : summary;
}

/**
 * The group header line (transcript.rs `tool_group_summary`): thought chips
 * are UI-synthesized, so the shared summary never sees them — name them on
 * the collapsed line instead ("Thought · Ran 2 commands").
 */
export function toolGroupTitle(tools: readonly ToolItem[]): string {
  const pairs = tools.filter((tool) => !tool.isThought);
  const thoughts = tools.length - pairs.length;
  const base = pairs.length === 0 ? "" : toolGroupSummary(pairs);
  if (thoughts === 0) {
    return base;
  }
  if (base.length === 0) {
    return thoughts === 1 ? "Thought process" : `Thought ${thoughts} times`;
  }
  return thoughts === 1 ? `Thought · ${base}` : `Thought ${thoughts} times · ${base}`;
}

// ---------------------------------------------------------------------------
// Subagent genus (crates/proto/src/agent.rs)
// ---------------------------------------------------------------------------
/**
 * A subagent SPAWN call — the `Agent[: <description>]` naming convention
 * every driver decodes its spawn tool into. The single genus gate for
 * subagent binding; see the Rust doc for why the call, never the ref, decides.
 */
export function isSubagentSpawn(call: ToolCall): boolean {
  const name = call.kind === "unknown" ? call.name : call.kind === "mcp" ? call.tool : null;
  return name !== null && (name === "Agent" || name.startsWith("Agent: "));
}

/** Spawn-input keys that carry a child model, in precedence order. */
const SUBAGENT_MODEL_KEYS = ["model", "model_id", "modelId", "subagent_model"];

/** The model a subagent spawn was given, when the spawn named one. */
export function subagentModel(call: ToolCall): string | null {
  if (!isSubagentSpawn(call)) {
    return null;
  }
  const input = call.kind === "unknown" || call.kind === "mcp" ? call.input : null;
  if (typeof input !== "object" || input === null) {
    return null;
  }
  for (const key of SUBAGENT_MODEL_KEYS) {
    const value = (input as Record<string, unknown>)[key];
    if (typeof value === "string" && value.trim().length > 0) {
      return value.trim();
    }
  }
  return null;
}

/**
 * `is_agent_call` (transcript.rs:359) — the genus is the call itself, never
 * the ref: docs written before the claude-driver fix carry stray
 * `subagent_ref`s on ordinary Run chips, and honoring the ref alone turned
 * those Runs into spawn chips that opened empty, never-created docs.
 */
export function isAgentCall(call: ToolCall): boolean {
  return isSubagentSpawn(call);
}

/** `is_agent_tool` (:368) — the item's call is an agent call. */
export function isAgentTool(item: ToolItem): boolean {
  return isAgentCall(item.call);
}

/**
 * `is_spawn_link` (:374) — an agent call that actually links to a spawned
 * doc. These render as links, never accordions: their `detail`/`invocation`
 * are suppressed and the whole chip opens the subagent's transcript.
 */
export function isSpawnLink(item: ToolItem): boolean {
  return isAgentCall(item.call) && item.subagentRef !== null;
}

/**
 * `tool_group_collapses` (:380) — ordinary tool groups fold behind a summary
 * header; an all-agent group renders as standalone, always-open rows.
 */
export function toolGroupCollapses(tools: readonly ToolItem[]): boolean {
  return tools.some((tool) => !isAgentTool(tool));
}

/**
 * First line of `text`, trimmed, capped at `max` chars with an ellipsis
 * (transcript.rs:7152).
 */
export function titleLine(text: string, max: number): string | null {
  const line = text.split("\n").find((l) => l.trim().length > 0);
  if (line === undefined) {
    return null;
  }
  const trimmed = line.trim();
  const chars = [...trimmed];
  if (chars.length > max) {
    return `${chars.slice(0, max).join("")}…`;
  }
  return trimmed;
}

/**
 * Drop a leading "Agent"/"Task" genus (with its `:` and spacing) from a
 * spawn-title candidate (transcript.rs:7164). Only a real word boundary
 * strips — "Taskmaster" keeps its name; a bare "Agent"/"Task" strips to "".
 */
export function stripSpawnPrefix(text: string): string {
  const t = text.trim();
  for (const prefix of ["agent", "task"]) {
    if (t.length >= prefix.length && t.slice(0, prefix.length).toLowerCase() === prefix) {
      const rest = t.slice(prefix.length);
      if (rest.length === 0) {
        return "";
      }
      if (rest.startsWith(":") || /^\s/.test(rest)) {
        return rest.replace(/^:+/, "").trim();
      }
    }
  }
  return t;
}

/**
 * `subagent_tab_title` (transcript.rs:7188) — the BARE task description.
 * Candidates in order: the tool name, then `input.description`, then
 * `input.prompt`; each stripped of its spawn prefix and capped at 40 chars;
 * "Subagent" only as the last resort.
 */
export function subagentTabTitle(call: ToolCall): string {
  const [name, input] =
    call.kind === "unknown"
      ? [call.name, call.input]
      : call.kind === "mcp"
        ? [call.tool, call.input]
        : [null, null];
  if (name === null) {
    return "Subagent";
  }
  const candidates: (string | null)[] = [
    name,
    typeof input === "object" && input !== null ? readString(input, "description") : null,
    typeof input === "object" && input !== null ? readString(input, "prompt") : null,
  ];
  for (const text of candidates) {
    if (text === null) {
      continue;
    }
    const title = titleLine(stripSpawnPrefix(text), SUBAGENT_TITLE_MAX);
    if (title !== null) {
      return title;
    }
  }
  return "Subagent";
}

function readString(input: object, key: string): string | null {
  const value = (input as Record<string, unknown>)[key];
  return typeof value === "string" ? value : null;
}

// ---------------------------------------------------------------------------
// Tool detail payloads (transcript.rs tool_detail / call_block / thought_item)
// ---------------------------------------------------------------------------

/** Max verbatim output lines per chip before the counted tail row. */
export const OUTPUT_DETAIL_MAX_LINES = 24;
/**
 * Max diff lines an inline tool-diff detail renders — the detail is one
 * stacked element inside its transcript row, so it must stay bounded
 * (transcript.rs:742).
 */
export const DIFF_DETAIL_MAX_LINES = 600;
/** Columns at which an invocation line soft-wraps into continuation lines. */
export const CALL_WRAP_COLS = 80;
/** Column budget for soft-wrapping thought text into detail lines. */
export const THOUGHT_WRAP_COLS = 96;

/** A chip's expandable detail payload. */
export type ToolDetail =
  | { readonly kind: "output"; readonly lines: readonly string[]; readonly truncatedBy: number }
  | { readonly kind: "thought"; readonly lines: readonly (readonly InlineRun[])[]; readonly truncatedBy: number }
  | { readonly kind: "diff"; readonly file: FileDiff }
  | { readonly kind: "stats"; readonly stats: readonly ToolDiffStat[] };

/** One tool invocation (or a reasoning part riding the group) inside a row. */
export interface ToolItem {
  readonly call: ToolCall;
  readonly isError: boolean;
  readonly resolved: boolean;
  readonly detail: ToolDetail | null;
  /** The full-invocation block: complete command/pattern/URL/input JSON. */
  readonly invocation: ToolDetail | null;
  /** Sidecar key of the full output (`{chatId}/{partId}`). */
  readonly outputRef: string | null;
  /** Full-output size, for "Show full output (12 KB)". */
  readonly outputBytes: number | null;
  /** Sidecar key of the full diff JSON. */
  readonly diffRef: string | null;
  /** The spawned subagent's doc id — the chip IS the index. */
  readonly subagentRef: string | null;
  /** Subagent lifecycle, distinct from `resolved` (eager-done). */
  readonly subagentStatus: "running" | "done" | "failed" | null;
  /** One-line live tail — LEGACY docs only; fingerprinted, never rendered. */
  readonly subagentTail: string | null;
  /** A reasoning part riding the tool group as a chip. */
  readonly isThought: boolean;
}

// ---------------------------------------------------------------------------
// Tool-chip geometry (transcript.rs:96-166) — analytic, so fold heights need
// no measurement. Ordinary tools place their icon on the rail; subagents
// retain a 30px card. Rows stack without a gap so the rail continues
// alongside expanded output.
// ---------------------------------------------------------------------------

/** `CHIP_HEIGHT` — a standalone (non-rail) chip row. */
export const CHIP_HEIGHT = 38;
/** `CHIP_GAP` — rail rows stack flush. */
export const CHIP_GAP = 0;
/** `CHIP_CARD_HEIGHT` — the chip card's border-box height. */
export const CHIP_CARD_HEIGHT = 30;
/**
 * `CHIP_HEADER_HEIGHT` — the card's inner header height (`CARD − 2`: a 30px
 * header inside a 30px bordered card clips 2px and every glyph reads high,
 * transcript.rs:102-106).
 */
export const CHIP_HEADER_HEIGHT = CHIP_CARD_HEIGHT - 2;
/** `TOOL_TEXT_SIZE` / `TOOL_LABEL_SIZE` / `TOOL_LABEL_LINE_HEIGHT` (:117-119). */
export const TOOL_TEXT_SIZE = 12;
export const TOOL_LABEL_SIZE = 12;
export const TOOL_LABEL_LINE_HEIGHT = 18;
/** `TOOL_GROUP_HEADER_HEIGHT` — the collapsed summary line (:120). */
export const TOOL_GROUP_HEADER_HEIGHT = 26;
/** `TOOL_TREE_ROW_HEIGHT` — a rail (activity) chip row (:122). */
export const TOOL_TREE_ROW_HEIGHT = 32;
/** `OUTPUT_LINE_HEIGHT` — one output/thought detail row (:746). */
export const OUTPUT_LINE_HEIGHT = 18;
/** `OUTPUT_BODY_PAD` — the detail body's py(6) × 2 (:749). */
export const OUTPUT_BODY_PAD = 12;
/** `DETAIL_SEPARATOR` — the hairline between an open chip's blocks (:752). */
export const DETAIL_SEPARATOR = 1;
/** `BLOB_AFFORDANCE_HEIGHT` — the "Show full output" row (:1838). */
export const BLOB_AFFORDANCE_HEIGHT = 24;
/** Line cap for a FETCHED full output (a defensive ceiling, :1851). */
export const FULL_OUTPUT_MAX_LINES = 400;
/** `SUBAGENT_TITLE_MAX` — chars a subagent tab title keeps (:7149). */
export const SUBAGENT_TITLE_MAX = 40;

/** A reasoning part flattened into styled, wrapped detail lines. */
export function thoughtDetail(text: string, live: boolean): ToolDetail | null {
  let lines = thoughtLines(parseMarkdown(text, live));
  const truncatedBy = Math.max(0, lines.length - OUTPUT_DETAIL_MAX_LINES);
  if (truncatedBy > 0) {
    if (live) {
      // Keep the TAIL while streaming (the fresh thinking is the signal);
      // settled thoughts keep the head like tool outputs do.
      lines = lines.slice(truncatedBy);
      // The cut can land on a block separator — drop the orphan blank.
      while (lines.length > 0 && lines[0]!.every((run) => run.text.trim().length === 0)) {
        lines = lines.slice(1);
      }
    } else {
      lines = lines.slice(0, OUTPUT_DETAIL_MAX_LINES);
    }
  }
  return lines.length > 0 ? { kind: "thought", lines, truncatedBy } : null;
}

/** Flatten a thought's parsed markdown into wrapped, styled detail lines. */
export function thoughtLines(tree: BlockTree): InlineRun[][] {
  const out: InlineRun[][] = [];
  for (const top of tree.blocks) {
    if (out.length > 0) {
      // One blank separator row between top-level blocks.
      out.push([]);
    }
    thoughtBlockLines(top.block, 0, out);
  }
  while (out.length > 0 && out[out.length - 1]!.every((run) => run.text.trim().length === 0)) {
    out.pop();
  }
  return out;
}

function indentRun(indent: number): InlineRun[] {
  return [{ text: " ".repeat(indent), style: {} }];
}

function pushStyled(line: InlineRun[], text: string, style: InlineStyle): void {
  if (text.length === 0) {
    return;
  }
  const last = line[line.length - 1];
  if (last !== undefined && styleEqual(last.style, style)) {
    line[line.length - 1] = { text: last.text + text, style: last.style };
    return;
  }
  line.push({ text, style });
}

function styleEqual(a: InlineStyle, b: InlineStyle): boolean {
  return (
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.code === b.code &&
    a.strikethrough === b.strikethrough &&
    a.link === b.link &&
    a.image === b.image
  );
}

function finishLine(indent: number, line: InlineRun[]): InlineRun[] {
  return [...indentRun(indent), ...line];
}

/** Word-wrap styled runs at the thought column budget (port of wrap_styled_runs). */
function wrapStyledRuns(runs: readonly InlineRun[], indent: number, out: InlineRun[][]): void {
  const budget = Math.max(THOUGHT_WRAP_COLS - indent, 16);
  // Segments split at hard breaks (`\n` runs).
  const segments: InlineRun[][] = [[]];
  for (const run of runs) {
    const pieces = run.text.split("\n");
    for (let ix = 0; ix < pieces.length; ix++) {
      if (ix > 0) {
        segments.push([]);
      }
      if (pieces[ix]!.length > 0) {
        segments[segments.length - 1]!.push({ text: pieces[ix]!, style: run.style });
      }
    }
  }
  for (const segment of segments) {
    // Tokens: maximal non-whitespace piece lists, glued across run boundaries.
    const tokens: InlineRun[][] = [];
    let inToken = false;
    for (const run of segment) {
      const text = run.text;
      let pos = 0;
      while (pos < text.length) {
        const rest = text.slice(pos);
        const ws = /^\s/.test(rest);
        const match = /^\s+|\S+/.exec(rest)!;
        const end = pos + match[0].length;
        if (ws) {
          inToken = false;
        } else {
          if (!inToken) {
            tokens.push([]);
            inToken = true;
          }
          pushStyled(tokens[tokens.length - 1]!, text.slice(pos, end), run.style);
        }
        pos = end;
      }
    }
    let line: InlineRun[] = [];
    let len = 0;
    for (const token of tokens) {
      const tokLen = [...token.map((r) => r.text).join("")].length;
      if (tokLen > budget) {
        // Hard-split a pathological token at the budget.
        if (len > 0) {
          out.push(finishLine(indent, line));
          line = [];
          len = 0;
        }
        for (const piece of token) {
          let chars = [...piece.text];
          while (chars.length > 0) {
            const chunk = chars.slice(0, budget - len).join("");
            if (chunk.length === 0) {
              break;
            }
            chars = chars.slice([...chunk].length);
            len += [...chunk].length;
            pushStyled(line, chunk, piece.style);
            if (len === budget) {
              out.push(finishLine(indent, line));
              line = [];
              len = 0;
            }
          }
        }
        continue;
      }
      if (len > 0 && len + 1 + tokLen > budget) {
        out.push(finishLine(indent, line));
        line = [];
        len = 0;
      }
      if (len > 0) {
        const last = line[line.length - 1];
        if (last !== undefined) {
          line[line.length - 1] = { text: last.text + " ", style: last.style };
        }
        len++;
      }
      for (const piece of token) {
        pushStyled(line, piece.text, piece.style);
      }
      len += tokLen;
    }
    if (len > 0) {
      out.push(finishLine(indent, line));
    }
  }
}

/** Soft-wrap one raw line into `cols`-char chunks (port of wrap_cols). */
function wrapCols(line: string, cols: number): string[] {
  const chars = [...line];
  if (chars.length <= cols) {
    return [line];
  }
  const out: string[] = [];
  for (let i = 0; i < chars.length; i += cols) {
    out.push(chars.slice(i, i + cols).join(""));
  }
  return out;
}

/** One markdown block into thought detail lines, `indent` spaces deep. */
function thoughtBlockLines(block: Block, indent: number, out: InlineRun[][]): void {
  switch (block.kind) {
    case "paragraph":
      wrapStyledRuns(block.runs, indent, out);
      break;
    case "heading":
      // Headings keep the detail's single type size — bold is the cue.
      wrapStyledRuns(block.runs.map((run) => ({ ...run, style: { ...run.style, bold: true } })), indent, out);
      break;
    case "codeBlock": {
      const style: InlineStyle = { code: true };
      for (const line of block.code.split("\n")) {
        for (const chunk of wrapCols(line, Math.max(THOUGHT_WRAP_COLS - indent, 16))) {
          const row = indentRun(indent);
          if (chunk.length > 0) {
            row.push({ text: chunk, style });
          }
          out.push(row);
        }
      }
      break;
    }
    case "list": {
      // Tight rendering: no blank rows inside a list.
      block.items.forEach((item, ix) => {
        const marker = block.orderedStart !== null ? `${block.orderedStart + ix}. ` : "• ";
        const inner = indent + [...marker].length;
        const mark = out.length;
        for (const child of item.blocks) {
          thoughtBlockLines(child, inner, out);
        }
        if (out.length === mark) {
          out.push(indentRun(inner));
        }
        // The item's first line trades its indent spaces for the marker —
        // the slot-0 run is REPLACED (its remainder is the item's own inner
        // indent, which the marker supplants; transcript.rs:599-601).
        const first = out[mark]![0];
        if (first !== undefined) {
          out[mark]![0] = { text: `${" ".repeat(indent)}${marker}`, style: first.style };
        }
      });
      break;
    }
    case "blockQuote": {
      const mark = out.length;
      block.children.forEach((child, ix) => {
        if (ix > 0) {
          out.push([]);
        }
        thoughtBlockLines(child, indent + 2, out);
      });
      // Trade the two quote-indent spaces for the bar on every quoted line.
      for (let k = mark; k < out.length; k++) {
        const first = out[k]![0];
        if (first !== undefined && first.text.length >= indent + 2) {
          out[k]![0] = {
            text: `${first.text.slice(0, indent)}│ ${first.text.slice(indent + 2)}`,
            style: first.style,
          };
        }
      }
      break;
    }
    case "table": {
      // A thought is a record, not a layout surface: cells joined with a dot
      // separator, header bold — no column machinery.
      const join = (cells: readonly (readonly InlineRun[])[], bold: boolean): InlineRun[] => {
        const line: InlineRun[] = [];
        cells.forEach((cell, ix) => {
          if (ix > 0) {
            pushStyled(line, " · ", {});
          }
          for (const run of cell) {
            pushStyled(line, run.text, bold ? { ...run.style, bold: true } : run.style);
          }
        });
        return line;
      };
      wrapStyledRuns(join(block.header, true), indent, out);
      for (const row of block.rows) {
        wrapStyledRuns(join(row, false), indent, out);
      }
      break;
    }
    case "rule": {
      const row = indentRun(indent);
      row.push({ text: "———", style: {} });
      out.push(row);
      break;
    }
  }
}

/**
 * Build a tool part's expandable detail (transcript.rs:757). An inline diff
 * wins (it is the more structured record of the same action) → then
 * non-empty diff stats → then raw output, with trailing blank lines trimmed
 * so the block hugs its content. The diff is capped at
 * `DIFF_DETAIL_MAX_LINES` so a whole-file rewrite can't build tens of
 * thousands of rows inside one transcript row.
 */
export function toolDetail(
  output: string | null | undefined,
  diff: ToolDiff | null | undefined,
  diffStats: readonly ToolDiffStat[] | null | undefined,
): ToolDetail | null {
  if (diff !== null && diff !== undefined) {
    const file = diffToFile(diff);
    if (file.hunks.length === 0) {
      return null;
    }
    return { kind: "diff", file: truncateFileLines(file, DIFF_DETAIL_MAX_LINES) };
  }
  if (diffStats !== null && diffStats !== undefined && diffStats.length > 0) {
    return { kind: "stats", stats: diffStats };
  }
  if (output === null || output === undefined) {
    return null;
  }
  const lines = output.split("\n");
  while (lines.length > 0 && lines[lines.length - 1]!.trim().length === 0) {
    lines.pop();
  }
  if (lines.length === 0) {
    return null;
  }
  const truncatedBy = Math.max(0, lines.length - OUTPUT_DETAIL_MAX_LINES);
  return { kind: "output", lines: lines.slice(0, OUTPUT_DETAIL_MAX_LINES), truncatedBy };
}

// ---------------------------------------------------------------------------
// diff_to_file (transcript.rs:890) — reduce an inline ToolDiff to the changes
// pane's FileDiff: hunks grouped with 3 context lines, dual 1-based line
// numbers, unified-diff hunk headers, and add/del counts.
// ---------------------------------------------------------------------------

/** One line-level edit from the Myers walk, in document order. */
interface LineOp {
  readonly tag: "equal" | "del" | "ins";
  readonly oldNo: number;
  readonly newNo: number;
}

/** Lines of `text` the way Rust's `str::lines()` splits (no trailing `""`). */
function splitLines(text: string): string[] {
  if (text.length === 0) {
    return [];
  }
  const lines = text.split("\n");
  if (text.endsWith("\n")) {
    lines.pop();
  }
  return lines;
}

/**
 * The minimal edit script between two line lists — Myers' O(ND) greedy
 * algorithm with a per-d V trace, backtracked to line-level ops. This is the
 * web stand-in for `similar::TextDiff::from_lines`; the huge-input guard
 * (a >4M-cell pairing) degrades to one whole-file hunk, matching similar's
 * bounded behavior on pathological inputs.
 */
function myersLineOps(a: readonly string[], b: readonly string[]): LineOp[] {
  const n = a.length;
  const m = b.length;
  const ops: LineOp[] = [];
  if (n + m > 20_000) {
    // Pathological input: all deletes then all inserts, one pairing.
    for (let ix = 0; ix < n; ix += 1) {
      ops.push({ tag: "del", oldNo: ix, newNo: -1 });
    }
    for (let ix = 0; ix < m; ix += 1) {
      ops.push({ tag: "ins", oldNo: -1, newNo: ix });
    }
    return ops;
  }
  if (n === 0 && m === 0) {
    return ops;
  }
  const max = n + m;
  const offset = max;
  // V indexed by k + offset; one snapshot per d for the backtrack. Reads
  // clamp to 0 — the guards below keep the read indexes in-range, and a
  // never-written cell reads as its initial 0.
  const at = (arr: readonly number[], k: number): number => arr[k + offset] ?? 0;
  const trace: number[][] = [];
  let v = new Array<number>(2 * max + 1).fill(0);
  let found = -1;
  outer: for (let d = 0; d <= max; d += 1) {
    trace.push([...v]);
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && at(v, k - 1) < at(v, k + 1))) {
        x = at(v, k + 1);
      } else {
        x = at(v, k - 1) + 1;
      }
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x += 1;
        y += 1;
      }
      v[k + offset] = x;
      if (x >= n && y >= m) {
        found = d;
        break outer;
      }
    }
  }
  if (found < 0) {
    return ops;
  }
  // Backtrack: emit line ops in REVERSE, then flip.
  let x = n;
  let y = m;
  const reversed: LineOp[] = [];
  for (let d = found; d > 0; d -= 1) {
    const vPrev = trace[d]!;
    const k = x - y;
    let prevK: number;
    if (k === -d || (k !== d && at(vPrev, k - 1) < at(vPrev, k + 1))) {
      prevK = k + 1;
    } else {
      prevK = k - 1;
    }
    const prevX = at(vPrev, prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x -= 1;
      y -= 1;
      reversed.push({ tag: "equal", oldNo: x, newNo: y });
    }
    if (x === prevX) {
      y -= 1;
      reversed.push({ tag: "ins", oldNo: -1, newNo: y });
    } else {
      x -= 1;
      reversed.push({ tag: "del", oldNo: x, newNo: -1 });
    }
  }
  while (x > 0 && y > 0) {
    x -= 1;
    y -= 1;
    reversed.push({ tag: "equal", oldNo: x, newNo: y });
  }
  ops.push(...reversed.reverse());
  return ops;
}

/** Group line ops into hunks with `context` equal lines around the changes. */
function groupHunks(ops: readonly LineOp[], context: number): { startIx: number; ops: LineOp[] }[] {
  const groups: { startIx: number; ops: LineOp[] }[] = [];
  const changeIx: number[] = [];
  for (let ix = 0; ix < ops.length; ix += 1) {
    if (ops[ix]!.tag !== "equal") {
      changeIx.push(ix);
    }
  }
  if (changeIx.length === 0) {
    return groups;
  }
  const gapLimit = 2 * context;
  let groupStart = Math.max(0, changeIx[0]! - context);
  let groupEnd = Math.min(ops.length - 1, changeIx[0]! + context);
  for (let ix = 1; ix < changeIx.length; ix += 1) {
    const at = changeIx[ix]!;
    if (at - groupEnd > gapLimit + 1) {
      groups.push({ startIx: groupStart, ops: ops.slice(groupStart, groupEnd + 1) });
      groupStart = Math.max(0, at - context);
    }
    groupEnd = Math.min(ops.length - 1, at + context);
  }
  groups.push({ startIx: groupStart, ops: ops.slice(groupStart, groupEnd + 1) });
  return groups;
}

/**
 * `diff_to_file` (transcript.rs:890) — one inline `ToolDiff` to the Changes
 * pane's `FileDiff`, grouped with 3 context lines like
 * `similar::TextDiff::grouped_ops(3)`. `oldText: null` means a new file
 * (status `added`).
 */
export function diffToFile(diff: ToolDiff): FileDiff {
  const oldText = diff.oldText ?? "";
  const oldLines = splitLines(oldText);
  const newLines = splitLines(diff.newText);
  const ops = myersLineOps(oldLines, newLines);
  // Each op's document-order start positions (an insert's old-side start is
  // where it lands, a delete's new-side start where it lands) — the source
  // of the hunk header's 0-based starts, exactly like similar's
  // `old_range()/new_range()` starts.
  const startOld: number[] = [];
  const startNew: number[] = [];
  let o = 0;
  let nw = 0;
  for (const op of ops) {
    startOld.push(o);
    startNew.push(nw);
    if (op.tag !== "ins") {
      o += 1;
    }
    if (op.tag !== "del") {
      nw += 1;
    }
  }
  const hunks: { header: string; lines: { kind: "context" | "add" | "del"; oldNo: number | null; newNo: number | null; text: string }[] }[] = [];
  let additions = 0;
  let deletions = 0;
  let maxLine = 0;
  for (const group of groupHunks(ops, 3)) {
    const oldStart = startOld[group.startIx] ?? 0;
    const newStart = startNew[group.startIx] ?? 0;
    let oldCount = 0;
    let newCount = 0;
    for (const op of group.ops) {
      if (op.tag === "equal" || op.tag === "del") {
        oldCount += 1;
      }
      if (op.tag === "equal" || op.tag === "ins") {
        newCount += 1;
      }
    }
    const header = `@@ -${oldStart + 1},${oldCount} +${newStart + 1},${newCount} @@`;
    const lines: { kind: "context" | "add" | "del"; oldNo: number | null; newNo: number | null; text: string }[] = [];
    for (const op of group.ops) {
      if (op.tag === "del") {
        deletions += 1;
      } else if (op.tag === "ins") {
        additions += 1;
      }
      const oldNo = op.tag === "ins" ? null : op.oldNo + 1;
      const newNo = op.tag === "del" ? null : op.newNo + 1;
      maxLine = Math.max(maxLine, oldNo ?? 0, newNo ?? 0);
      lines.push({
        kind: op.tag === "equal" ? "context" : op.tag === "ins" ? "add" : "del",
        oldNo,
        newNo,
        text: op.tag === "ins" ? newLines[op.newNo]! : oldLines[op.oldNo]!,
      });
    }
    hunks.push({ header, lines });
  }
  return {
    path: diff.path,
    oldPath: null,
    status: diff.oldText === null || diff.oldText === undefined ? "added" : "modified",
    binary: false,
    notices: [],
    hunks,
    additions,
    deletions,
    maxLine,
  };
}

/**
 * Build the upgraded detail from a fetched sidecar blob
 * (transcript.rs:1856). Diff blobs parse the `ToolDiff` JSON through the
 * same pipeline as inline diffs; output blobs render (near-)uncapped —
 * fetching past the summary was the point.
 */
export function blobDetail(text: string, isDiff: boolean): ToolDetail | null {
  if (isDiff) {
    let diff: ToolDiff | null = null;
    try {
      diff = JSON.parse(text) as ToolDiff;
    } catch {
      return null;
    }
    if (typeof diff !== "object" || diff === null || typeof diff.newText !== "string" || typeof diff.path !== "string") {
      return null;
    }
    return toolDetail(null, diff, null);
  }
  const lines = text.split("\n");
  while (lines.length > 0 && lines[lines.length - 1]!.trim().length === 0) {
    lines.pop();
  }
  if (lines.length === 0) {
    return null;
  }
  const truncatedBy = Math.max(0, lines.length - FULL_OUTPUT_MAX_LINES);
  return { kind: "output", lines: lines.slice(0, FULL_OUTPUT_MAX_LINES), truncatedBy };
}

/**
 * Compact byte size for the fetch affordance label ("812 B", "12 KB")
 * (transcript.rs:1880) — `ceil`, never decimals.
 */
export function formatKb(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  return `${Math.ceil(bytes / 1024)} KB`;
}

/**
 * The glyph for a tool call — `tool_icon_path` (transcript.rs:6656) mapped
 * to `@zeron/icons` names.
 */
export function toolIconName(call: ToolCall): IconName {
  switch (call.kind) {
    case "exec":
      return "terminal";
    case "readFile":
    case "applyPatch":
      return "document";
    case "writeFile":
      return "documentAdd";
    case "editFile":
      return "pen";
    case "search":
      return "magnifer";
    case "glob":
      return "folderWithFiles";
    case "webFetch":
    case "webSearch":
      return "global";
    case "todo":
      return "checklist";
    case "mcp":
      return isSubagentSpawn(call) ? "bot" : "widget";
    case "unknown":
      if (isSubagentSpawn(call) || call.name === "Wait for agents") {
        return "bot";
      }
      return "widget";
  }
}

/**
 * `file_badge_name` (transcript.rs:6676) — compact file-action chips show
 * only the final path component, accepting `/` AND `\` (remote tools can
 * report Windows paths even when the UI runs elsewhere).
 */
export function fileBadgeName(path: string): string {
  for (const part of path.split(/[/\\]/).reverse()) {
    if (part.length > 0) {
      return part;
    }
  }
  return path;
}

/**
 * Analytic expanded-chips height (transcript.rs:1803) — no measurement
 * needed for the fold tween.
 */
export function chipsHeight(count: number): number {
  if (count === 0) {
    return 0;
  }
  return CHIPS_TOP_PAD + count * CHIP_HEIGHT + (count - 1) * CHIP_GAP;
}

/**
 * Analytic height an open detail adds to its chip's card (separator + body)
 * (transcript.rs:1814) — output/thought by line count, diff via the changes
 * pane's own `body_height`, stats one row each. `diffLine` is the
 * code-size-scaled diff row (`diff_line_height`); it defaults to the
 * 12.5px-code setting's 21px.
 */
export function detailHeight(detail: ToolDetail, diffLine: number = DIFF_LINE_HEIGHT): number {
  let body: number;
  switch (detail.kind) {
    case "output":
    case "thought":
      body = (detail.lines.length + (detail.truncatedBy > 0 ? 1 : 0)) * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD;
      break;
    case "diff":
      body = bodyHeight(detail.file, diffLine);
      break;
    case "stats":
      body = detail.stats.length * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD;
      break;
  }
  return DETAIL_SEPARATOR + body;
}

/**
 * Build a chip's full-invocation block — the complete tool call the header
 * truncates to one line: the whole command, pattern, or URL, todo items one
 * per line, MCP/unknown input as pretty-printed JSON (port of `call_block`).
 */
export function callBlock(call: ToolCall): ToolDetail | null {
  let text: string;
  switch (call.kind) {
    case "exec":
      text = call.command;
      break;
    case "readFile":
      text = call.path;
      break;
    case "writeFile":
      text = call.content !== null && call.content !== undefined ? `${call.path}\n${call.content}` : call.path;
      break;
    case "editFile":
      text = call.path;
      break;
    case "applyPatch":
      text = call.path ?? "workspace";
      break;
    case "search":
      text = call.path !== null && call.path !== undefined ? `${call.pattern} in ${call.path}` : call.pattern;
      break;
    case "glob":
      text = call.pattern;
      break;
    case "webFetch":
      text = call.prompt !== null && call.prompt !== undefined ? `${call.url}\n${call.prompt}` : call.url;
      break;
    case "webSearch":
      text = call.query;
      break;
    case "todo":
      text = call.items.map((item) => `${item.done ? "[x]" : "[ ]"} ${item.text}`).join("\n");
      break;
    case "mcp": {
      const pretty = call.input !== null && call.input !== undefined ? safePretty(call.input) : null;
      text = pretty !== null ? `${call.server} · ${call.tool}\n${pretty}` : `${call.server} · ${call.tool}`;
      break;
    }
    case "unknown": {
      const pretty = call.input !== null && call.input !== undefined ? safePretty(call.input) : null;
      text = pretty !== null ? `${call.name}\n${pretty}` : call.name;
      break;
    }
  }
  const lines = text.split("\n").flatMap((line) => wrapCols(line, CALL_WRAP_COLS));
  while (lines.length > 0 && lines[lines.length - 1]!.trim().length === 0) {
    lines.pop();
  }
  if (lines.length === 0) {
    return null;
  }
  const truncatedBy = Math.max(0, lines.length - OUTPUT_DETAIL_MAX_LINES);
  return { kind: "output", lines: lines.slice(0, OUTPUT_DETAIL_MAX_LINES), truncatedBy };
}

function safePretty(input: unknown): string | null {
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Entry helpers
// ---------------------------------------------------------------------------

/** User prompts clamp to this many wrapped lines until expanded. */
export const USER_COLLAPSED_LINES = 5;
/** Conservative char-count proxy for the fold affordance. */
export const USER_COLLAPSE_CHARS = 400;

// ---------------------------------------------------------------------------
// Shared constants (transcript.rs:70-218; the spring pair lives in
// lib/stick-spring.ts, listed in the ticket for reference only)
// ---------------------------------------------------------------------------

/** Per-chat saved viewports, LRU-bounded (transcript.rs MAX_SAVED_VIEWPORTS). */
export const MAX_SAVED_VIEWPORTS = 256;
/** Locally-authored queue rows awaiting materialization (transcript.rs:87). */
export const MAX_PENDING_QUEUED_TURNS = 256;
/** Cadence of the selection drag's edge auto-scroll (transcript.rs:91). */
export const SELECTION_SCROLL_TICK_MS = 24;
/** Distance from an edge where a selection drag starts auto-scrolling (:92). */
export const SELECTION_SCROLL_EDGE_PX = 36;
/** Max px per selection auto-scroll tick, at full penetration (:93). */
export const SELECTION_SCROLL_MAX_STEP_PX = 24;
/** Wrapped line height of the user bubble (transcript.rs USER_LINE_HEIGHT). */
export const USER_LINE_HEIGHT = 22;
/** Collapsed prompt text height: 5 lines * 22 (transcript.rs:1771). */
export const USER_COLLAPSED_TEXT_HEIGHT = USER_COLLAPSED_LINES * USER_LINE_HEIGHT;
/** Gap between the bubble and its expander (transcript.rs USER_TOGGLE_GAP). */
export const USER_TOGGLE_GAP = 8;
/**
 * A locally-sent prompt parks this far below the viewport top
 * (transcript.rs:205) — `TITLEBAR_HEIGHT + 10`: the titlebar overlays the
 * full-height list, so its height is part of the inset.
 */
export const OWN_SEND_TOP_INSET_PX = TITLEBAR_HEIGHT + 10;
/** Legal resting slack under the hold (transcript.rs:213). */
export const OWN_SEND_SCROLL_SLACK_PX = 2;
/** Per-60fps-frame retained fraction of the entry glide's error (:216). */
export const OWN_SEND_GLIDE_RETAIN = 0.85;
/** The entry glide snaps within this error (transcript.rs:218). */
export const OWN_SEND_GLIDE_SNAP_PX = 1;
/** Flavour-word rotation period (transcript.rs FLAVOUR_ROTATE_SECS). */
export const FLAVOUR_ROTATE_SECS = 7;
/** Long-press delay that toggles a prompt's fold (transcript.rs:4460). */
export const USER_HOLD_DELAY_MS = 360;
/** Copy-feedback clear (transcript.rs:5689, :5721). */
export const COPIED_CLEAR_MS = 1200;
/** Tool-group chip stack top pad — ticket 19 consumes (transcript.rs:166). */
export const CHIPS_TOP_PAD = 2;

// ---------------------------------------------------------------------------
// Working-trailer helpers (transcript.rs:1886-1952)
// ---------------------------------------------------------------------------

/** The 21 flavour words, in desktop order (transcript.rs:1886-1915). */
export const FLAVOUR_WORDS: readonly string[] = [
  "Zeroning",
  "Thinking",
  "Pondering",
  "Scheming",
  "Brewing",
  "Weaving",
  "Tinkering",
  "Musing",
  "Composing",
  "Sifting",
  "Untangling",
  "Distilling",
  "Sketching",
  "Plotting",
  "Riffing",
  "Combobulating",
  "Percolating",
  "Marinating",
  "Noodling",
  "Puzzling",
  "Conjuring",
];

/** The flavour word for a seed at an elapsed time (transcript.rs:1919). */
export function flavourWord(seed: number, elapsedSecs: number): string {
  const step = Math.floor(Math.max(elapsedSecs, 0) / FLAVOUR_ROTATE_SECS);
  const count = FLAVOUR_WORDS.length;
  const ix = ((Math.trunc(seed) + step) % count + count) % count;
  return FLAVOUR_WORDS[ix]!;
}

/** A stable per-chat seed (transcript.rs:1925 — fnv1a over the id). */
export function flavourSeed(chatId: string): number {
  return fnv1a(chatId);
}

/** Compact elapsed formatting (transcript.rs `format_elapsed`): at most
 *  two units, scaling seconds → minutes → hours → days. */
export function formatElapsed(secs: number): string {
  const clamped = Math.max(0, Math.floor(secs));
  if (clamped < 60) {
    return `${clamped}s`;
  }
  if (clamped < 3_600) {
    return `${Math.floor(clamped / 60)}m ${clamped % 60}s`;
  }
  if (clamped < 86_400) {
    return `${Math.floor(clamped / 3_600)}h ${Math.floor((clamped % 3_600) / 60)}m`;
  }
  return `${Math.floor(clamped / 86_400)}d ${Math.floor((clamped % 86_400) / 3_600)}h`;
}

/**
 * The working trailer's "Sending…" bridge (transcript.rs:1933): true while an
 * in-flight send is fresher than the session row's turn start — the row still
 * carries the PREVIOUS turn (or none), so a timer would count the send
 * round-trip and restart when the turn actually begins.
 */
export function sendingBridge(sendStarted: number | null, turnStarted: number | null): boolean {
  if (sendStarted === null) {
    return false;
  }
  if (turnStarted === null) {
    return true;
  }
  return turnStarted <= sendStarted;
}

// ---------------------------------------------------------------------------
// Selection drag auto-scroll (transcript.rs:142, §3.7)
// ---------------------------------------------------------------------------

/**
 * The t²-ramped auto-scroll step while a selection drag sits near a viewport
 * edge. Positive moves toward the document bottom; the tick cadence is
 * `SELECTION_SCROLL_TICK_MS` (24ms).
 */
export function selectionScrollStep(
  bounds: { top: number; bottom: number },
  position: { x: number; y: number },
): number {
  const height = bounds.bottom - bounds.top;
  if (height <= 0) {
    return 0;
  }
  const edge = Math.min(SELECTION_SCROLL_EDGE_PX, height / 3);
  if (edge <= 0) {
    return 0;
  }
  const scaled = (penetration: number): number => {
    const t = Math.min(Math.max(penetration / edge, 0), 1);
    return SELECTION_SCROLL_MAX_STEP_PX * t * t;
  };
  if (position.y < bounds.top + edge) {
    return -scaled(bounds.top + edge - position.y);
  }
  if (position.y > bounds.bottom - edge) {
    return scaled(position.y - (bounds.bottom - edge));
  }
  return 0;
}

/**
 * Ticket 78 — the arming rules of the selection drag's edge auto-scroll.
 * The edge step above is fine; the ARming was the live bug: a window-level
 * `pointermove` with `buttons !== 0` armed the tracker from ANY hold with
 * micro-drift (titlebar, composer, safe-area), scrolling the chat for the
 * whole contact. The tracker owns the rules so the component's listeners
 * stay thin:
 *
 * - Only a primary-button press on non-interactive content INSIDE the
 *   scroller arms it (`press`); interactive targets disarm (`pressInteractive`).
 * - Window `pointermove` only ever UPDATES an armed drag (`move`) — never
 *   arms one — and drops it when the buttons release.
 * - `pointercancel` clears exactly like `pointerup` (the composer's copy of
 *   this listener set handles cancel; the transcript's did not, so a browser
 *   takeover left the tracker armed past finger-lift).
 */
export class SelectionDragTracker {
  #position: { x: number; y: number } | null = null;

  /** A primary press landed on non-interactive content inside the scroller. */
  press(x: number, y: number): void {
    this.#position = { x, y };
  }

  /** A press landed on interactive content — any armed drag disarms. */
  pressInteractive(): void {
    this.#position = null;
  }

  /** Window pointermove: tracks an ARMED drag only, never arms one. */
  move(buttons: number, x: number, y: number): void {
    if (this.#position === null) {
      return;
    }
    if (buttons === 0) {
      this.#position = null;
      return;
    }
    this.#position = { x, y };
  }

  clear(): void {
    this.#position = null;
  }

  /** The armed drag's position, or null when disarmed. */
  get position(): { x: number; y: number } | null {
    return this.#position;
  }
}

/**
 * Ticket 78 — the edge auto-scroll is a SELECTION-drag affordance (the
 * desktop's `step_selection_scroll` rides a real selection drag): the tick
 * only steps while the document carries a non-collapsed selection. A
 * stationary hold inside the scroller's own edge band without a selection
 * must not scroll either.
 */
export function selectionDragAutoscrolls(
  selection: { isCollapsed: boolean; rangeCount: number } | null,
): boolean {
  return selection !== null && selection.rangeCount > 0 && !selection.isCollapsed;
}

// ---------------------------------------------------------------------------
// User-fold resize spec (transcript.rs:1178-1192)
// ---------------------------------------------------------------------------

/** Fold tween duration scales with travel, bounded to 850ms (:1178). */
export function userResizeDurationMs(heightDelta: number): number {
  return Math.round(Math.min(220 + Math.max(heightDelta, 0) * 0.32, 850));
}

/**
 * The fold tween's curve (:1185): short folds keep the decisive ease-out;
 * large folds ease-in-out so thousands of pixels do not vanish in the first
 * few frames of a front-loaded curve. The name maps to the motion catalog's
 * `--rb-ease-<curve>` custom property.
 */
export function userResizeCurve(heightDelta: number): "easeInOut" | "easeOut" {
  return heightDelta > 500 ? "easeInOut" : "easeOut";
}

/** Whether a prompt may need a fold affordance (first-frame proxy). */
export function userMessageNeedsCollapse(text: string): boolean {
  return text.split("\n").length > USER_COLLAPSED_LINES || [...text].length > USER_COLLAPSE_CHARS;
}

// ---------------------------------------------------------------------------
// Sent file mentions (crates/ui/src/composer.rs:860-1338 — the projection the
// transcript reuses so sent chips read as chips, not raw Markdown). Ticket 14
// owns the implementation once, in `./mentions.ts`; this module re-exports
// the transcript-facing surface it has always exposed.
// ---------------------------------------------------------------------------

export type { SentMentionSpan } from "./mentions";
export { sentMentionDisplay } from "./mentions";


/**
 * Clipboard payload for an assistant/system entry: authored text parts in
 * document order, preserving Markdown while excluding tool traces.
 */
export function assistantCopyText(entry: SessionMessageEntry): string | null {
  const text = entry.parts
    .filter((part): part is Extract<MessagePart, { kind: "text" }> => part.kind === "text" && part.text.trim().length > 0)
    .map((part) => part.text)
    .join("\n\n");
  return text.length > 0 ? text : null;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Absolute hover-timestamp label, e.g. "Jul 1, 3:45 PM" (local timezone). */
export function formatTimestamp(ms: number): string {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const hours = date.getHours();
  const hour12 = hours % 12 === 0 ? 12 : hours % 12;
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const ampm = hours < 12 ? "AM" : "PM";
  return `${MONTHS[date.getMonth()]} ${date.getDate()}, ${hour12}:${minutes} ${ampm}`;
}

/** FNV-1a over UTF-16 code units, folded to 32 bits — the row-version hash. */
export function fnv1a(text: string): number {
  let hash = 0xcbf29ce484222325n;
  const prime = 0x1000001b3n;
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    hash ^= BigInt(unit & 0xff);
    hash = BigInt.asUintN(64, hash * prime);
    hash ^= BigInt(unit >> 8);
    hash = BigInt.asUintN(64, hash * prime);
  }
  // 32 bits keep `version * 2 + bit` and `version ^ (1 << 30)` exact in JS.
  return Number(BigInt.asIntN(32, hash));
}

// ---------------------------------------------------------------------------
// Row model (transcript.rs rows_for_entry)
// ---------------------------------------------------------------------------

export type TranscriptRowKind =
  | {
      readonly kind: "user";
      /**
       * The visible prompt — the PROJECTED display text when it carries file
       * mentions (chip labels in place of the raw Markdown links); the
       * attachment-ref trailer is already stripped.
       */
      readonly text: string;
      /** File-mention chips over `text`, in display-string offsets. */
      readonly mentions: readonly SentMentionSpan[];
      /**
       * Structured context the prompt folded in as text, lifted back out
       * (`badges::split`) — the pill's data (lib/badges.ts).
       */
      readonly badges: readonly MessageBadge[];
      /** Optimistic echo not yet confirmed by a doc frame. */
      readonly pending: boolean;
      /**
       * A pending echo past `UNDELIVERED_GRACE_MS` with nothing confirming it
       * (`send_undelivered`) — the trailer under the last row carries the
       * "Not delivered — click to retry" affordance. Only ever set on echo
       * rows; a real doc row is delivered by definition.
       */
      readonly undelivered?: boolean;
      /** Attachment refs parsed out of the message's refs trailer
       *  (composer/use-attachments.ts `withAttachments`). The transcript
       *  renders a thumbnail strip above the bubble when non-empty. */
      readonly attachments: readonly UserImageAttachment[];
    }
  /** One top-level markdown block of a completed message. */
  | { readonly kind: "markdown"; readonly tree: BlockTree; readonly blockIx: number }
  /** One top-level block of a STREAMING message (same split as settled rows). */
  | { readonly kind: "liveMarkdown"; readonly tree: BlockTree; readonly blockIx: number }
  | { readonly kind: "toolGroup"; readonly tools: readonly ToolItem[]; readonly autoOpen: boolean }
  | { readonly kind: "inputChip"; readonly header: string; readonly resolved: boolean }
  | { readonly kind: "errorChip"; readonly message: string };

/** A transcript row: stable id + content version (diff key) + block payload. */
export interface TranscriptRow {
  readonly id: string;
  readonly version: number;
  /** First row of its message entry (gets the turn gap). */
  readonly turnStart: boolean;
  readonly rowKind: TranscriptRowKind;
  /** The owning message entry (hover reveals its timestamp strip). */
  readonly entryId: string;
  /** Epoch-ms for the hover-timestamp strip UNDER this row (settled last row). */
  readonly timestamp: number | null;
  /** Text copied by the entry-level hover action (settled last row only). */
  readonly copyText: string | null;
}

export interface RowsOptions {
  /** Optimistic echo flag for user entries. */
  readonly pending?: boolean;
  /** Maps `(partKey, text, live)` to a block tree (a MarkdownCache parse). */
  readonly parse: (key: string, text: string, live: boolean) => BlockTree;
}

// ---------------------------------------------------------------------------
// Virtualizer window
// ---------------------------------------------------------------------------

/** The mounted row range of the virtualizer: `first`..`last`, inclusive. */
export interface RowWindow {
  readonly first: number;
  readonly last: number;
}

/**
 * The visible window over prefix-sum `positions` (row bottoms are monotonic
 * by construction — `positions[ix] + rowHeights[ix]` is `positions[ix + 1]`):
 * `first` is the first row whose BOTTOM crosses `top − overdraw`, `last` the
 * last row whose TOP is at or below `top + height + overdraw` (the desktop's
 * 320px overdraw on both ends, transcript.rs OVERDRAW_PX). One contiguous
 * run: everything strictly above is replaced by the top spacer.
 *
 * When every row sits above the window (a stale `top` the scroller has not
 * clamped yet), `first` pins to the last row so the spacer math stays inside
 * `positions`; the next scroll event corrects the view.
 */
export function visibleRowWindow(
  positions: readonly number[],
  rowHeights: readonly number[],
  top: number,
  height: number,
  overdraw: number,
): RowWindow {
  const count = positions.length;
  const windowStart = top - overdraw;
  const windowEnd = top + height + overdraw;
  let first = count;
  let last = -1;
  for (let ix = 0; ix < count; ix++) {
    const rowTop = positions[ix]!;
    const bottom = rowTop + (rowHeights[ix] ?? 0);
    // The first row to cross the window's start — the mounted prefix ends
    // here. (The old `ix < first` test with `first` starting at 0 was never
    // true, so every render mounted ALL rows from index 0 and top
    // virtualization was dead.)
    if (first === count && bottom >= windowStart) {
      first = ix;
    }
    if (rowTop <= windowEnd) {
      last = ix;
    }
  }
  if (count > 0 && first >= count) {
    first = count - 1;
  }
  return { first, last };
}

/**
 * Build the block rows of one (already continuation-joined) entry — a direct
 * port of `rows_for_entry`: user entries are one bubble row; assistant/system
 * entries split into one row per top-level markdown block with consecutive
 * same-genus tools folded into group rows; input and error parts are chips.
 */
export function rowsForEntry(entry: SessionMessageEntry, options: RowsOptions): TranscriptRow[] {
  const pending = options.pending ?? false;
  const streaming = entry.status === "streaming";
  const rows: TranscriptRow[] = [];

  if (entry.role === "user") {
    const raw = entry.parts
      .filter((part): part is Extract<MessagePart, { kind: "text" }> => part.kind === "text")
      .map((part) => part.text)
      .join("\n\n");
    const parsed = parseUserMessageImages(raw);
    // Badges split BEFORE the mention projection, so a comment body's own
    // Markdown never lands in the bubble (transcript.rs:1226).
    const { text: body, badges } = splitBadges(parsed.text);
    // File mentions render as chips here too, not just in the composer. The
    // projection is pure over the text, so the raw-length row version stays
    // a valid cache/diff key.
    const mention = sentMentionDisplay(body);
    const text = mention?.display ?? body;
    const mentions = mention?.mentions ?? [];
    const copyText = text.trim().length > 0 ? text : null;
    // `raw.length << 1 | pending` on the desktop; BigInt-free equivalent.
    return [
      {
        id: entry.id,
        version: raw.length * 2 + (pending ? 1 : 0),
        turnStart: true,
        rowKind: { kind: "user", text, mentions, badges, pending, attachments: parsed.attachments },
        entryId: entry.id,
        // User rows always carry the strip (the optimistic echo included).
        timestamp: entry.createdAt,
        copyText,
      },
    ];
  }

  // Assistant/system: split parts into block rows, folding consecutive
  // ordinary tools. Agent/spawn chips flush into their own group so they
  // never share a collapse with Reads/Runs.
  const lastPartIx = entry.parts.length - 1;
  let groupIx = 0;
  let pendingGroup: ToolItem[] = [];
  let groupLastPartIx = 0;

  const flushGroup = (): void => {
    if (pendingGroup.length === 0) {
      return;
    }
    const tools = pendingGroup;
    pendingGroup = [];
    const autoOpen = streaming && groupLastPartIx === lastPartIx;
    rows.push({
      id: `${entry.id}#g${groupIx}`,
      version: toolFingerprint(tools, autoOpen),
      turnStart: false,
      rowKind: { kind: "toolGroup", tools, autoOpen },
      entryId: entry.id,
      timestamp: null,
      copyText: null,
    });
    groupIx++;
  };

  entry.parts.forEach((part, partIx) => {
    if (part.kind === "tool") {
      const item: ToolItem = {
        call: part.call,
        isError: part.isError,
        resolved: part.resolved,
        detail: toolDetail(part.output, part.diff ?? null, part.diffStats),
        invocation: callBlock(part.call),
        outputRef: part.outputRef ?? null,
        outputBytes: part.outputBytes ?? null,
        diffRef: part.diffRef ?? null,
        subagentRef: part.subagentRef ?? null,
        subagentStatus: part.subagentStatus ?? null,
        subagentTail: part.subagentTail ?? null,
        isThought: false,
      };
      // Agent chips don't share a fold with ordinary tools: flush whenever
      // the genus flips so each group is uniform.
      const head = pendingGroup[0];
      if (head !== undefined && isAgentTool(head) !== isAgentTool(item)) {
        flushGroup();
      }
      pendingGroup.push(item);
      groupLastPartIx = partIx;
      return;
    }
    if (part.kind === "reasoning") {
      if (part.text.trim().length === 0) {
        return;
      }
      // Live only while it is the tail of a streaming reply.
      const live = streaming && partIx === lastPartIx;
      const detail = thoughtDetail(part.text, live);
      const item: ToolItem = {
        call: { kind: "unknown", name: "Thought process" },
        isError: false,
        resolved: !live,
        detail,
        invocation: null,
        outputRef: null,
        outputBytes: null,
        diffRef: null,
        subagentRef: null,
        subagentStatus: null,
        subagentTail: null,
        isThought: true,
      };
      // Thoughts join ordinary tool groups; agent groups stay pure.
      if (pendingGroup[0] !== undefined && isAgentTool(pendingGroup[0])) {
        flushGroup();
      }
      pendingGroup.push(item);
      groupLastPartIx = partIx;
      return;
    }

    flushGroup();
    if (part.kind === "text") {
      if (part.text.trim().length === 0) {
        return;
      }
      const key = `${entry.id}#${part.id}`;
      const tree = options.parse(key, part.text, streaming);
      // Live and completed parts split identically — one row per top-level
      // block, same ids, so the live→complete handoff never changes identity.
      for (let blockIx = 0; blockIx < tree.blocks.length; blockIx++) {
        const top = tree.blocks[blockIx]!;
        const bytes = part.text.slice(Math.min(top.start, part.text.length), Math.min(top.end, part.text.length));
        rows.push({
          id: `${key}.${blockIx}`,
          version: fnv1a(bytes) * 2 + (streaming ? 1 : 0),
          turnStart: false,
          entryId: entry.id,
          timestamp: null,
          copyText: null,
          rowKind: streaming
            ? { kind: "liveMarkdown", tree, blockIx }
            : { kind: "markdown", tree, blockIx },
        });
      }
      return;
    }
    if (part.kind === "input") {
      const header = singleLine(part.questions[0]?.header ?? "Question");
      rows.push({
        id: `${entry.id}#${part.id}`,
        version: fnv1a(header) * 2 + (part.resolved ? 1 : 0),
        turnStart: false,
        rowKind: { kind: "inputChip", header, resolved: part.resolved },
        entryId: entry.id,
        timestamp: null,
        copyText: null,
      });
      return;
    }
    if (part.kind === "error") {
      rows.push({
        id: `${entry.id}#${part.id}`,
        version: part.message.length,
        turnStart: false,
        // Harness-generated; the chip is one line.
        rowKind: { kind: "errorChip", message: singleLine(part.message) },
        entryId: entry.id,
        timestamp: null,
        copyText: null,
      });
    }
  });
  flushGroup();

  if (rows.length > 0) {
    rows[0] = { ...rows[0]!, turnStart: true };
  }
  // Timestamp strip under the entry's LAST row once the turn has settled
  // ("No timestamp hover mid-stream").
  if (!streaming && rows.length > 0) {
    const last = rows[rows.length - 1]!;
    rows[rows.length - 1] = {
      ...last,
      timestamp: entry.createdAt,
      copyText: assistantCopyText(entry),
      version: last.version ^ 0x40000000,
    };
  }
  return rows;
}

/**
 * Content fingerprint for a tool group row (the diff key, transcript.rs
 * :1051) — per tool: the chip label bytes, the detail string LENGTH, the
 * packed `is_error | resolved<<1` byte, a detail tag byte (0 none, 1 Output,
 * 2 Diff, 3 Stats, 4 Thought) plus kind-specific payload (Output: line
 * count, truncated_by, total byte count; Thought: line count, truncated_by,
 * then EVERY run's bytes and a packed style byte `bold | italic<<1 |
 * code<<2 | strike<<3 | link<<4`, `\n` per line; Diff: path, additions,
 * deletions, hunk count; Stats: each `(path, additions, deletions)`), the
 * invocation's line bytes + truncated_by, a packed
 * `output_ref.is_some() | diff_ref.is_some()<<1` byte, a packed
 * `subagent_ref.is_some() | status<<1` byte, and the subagent tail bytes.
 * Finally the `auto_open` byte.
 */
function toolFingerprint(tools: readonly ToolItem[], autoOpen: boolean): number {
  let acc = "";
  for (const tool of tools) {
    const { label, detail } = toolChipContent(tool.call);
    acc += label;
    acc += String(detail.length);
    acc += String(Number(tool.isError) | (Number(tool.resolved) << 1));
    if (tool.detail === null) {
      acc += "0";
    } else if (tool.detail.kind === "output") {
      acc += `1${tool.detail.lines.length},${tool.detail.truncatedBy},${tool.detail.lines.join("").length}`;
    } else if (tool.detail.kind === "diff") {
      const file = tool.detail.file;
      acc += `2${file.path},${file.additions},${file.deletions},${file.hunks.length}`;
    } else if (tool.detail.kind === "stats") {
      acc += `3${tool.detail.stats.map((stat) => `${stat.path},${stat.additions},${stat.deletions}`).join(";")}`;
    } else {
      acc += `4${tool.detail.lines.length},${tool.detail.truncatedBy},${tool.detail.lines
        .map((line) =>
          line
            .map((run) => {
              const style = run.style;
              const packed =
                Number(style.bold === true) |
                (Number(style.italic === true) << 1) |
                (Number(style.code === true) << 2) |
                (Number(style.strikethrough === true) << 3) |
                (Number(style.link !== null && style.link !== undefined) << 4);
              return `${run.text}\u{1}${packed}`;
            })
            .join(""),
        )
        .join("\n")}`;
    }
    if (tool.invocation !== null && tool.invocation.kind === "output") {
      acc += `i${tool.invocation.lines.join("")},${tool.invocation.truncatedBy}`;
    }
    acc += String(Number(tool.outputRef !== null) | (Number(tool.diffRef !== null) << 1));
    acc += String(
      Number(tool.subagentRef !== null) |
        (tool.subagentStatus === null ? 0 : tool.subagentStatus === "running" ? 1 << 1 : tool.subagentStatus === "done" ? 2 << 1 : 3 << 1),
    );
    acc += tool.subagentTail ?? "";
  }
  acc += String(Number(autoOpen));
  return fnv1a(acc);
}

// ---------------------------------------------------------------------------
// Row spacing and diffs
// ---------------------------------------------------------------------------

/** `Theme::SPACE_SM` (proto/layout.rs) — the small step. */
export const SPACE_SM = layout.space.sm;
/** `Theme::SPACE_MD` (proto/layout.rs) — the medium step. */
export const SPACE_MD = layout.space.md;
/** `Theme::SPACE_LG` (proto/layout.rs) — the turn gap. */
export const SPACE_LG = layout.space.lg;
/** Gap between sibling markdown block rows (render.rs MD_BLOCK_GAP). */
export const MD_BLOCK_GAP = 12;

/** Markdown row ids are `{entry}#{part}.{blockIx}` — the part prefix. */
function partPrefix(id: string): string {
  const dot = id.lastIndexOf(".");
  return dot < 0 ? id : id.slice(0, dot);
}

/**
 * Vertical gap opening `row` given its predecessor (transcript.rs:1630): the
 * turn gap at turn starts; the markdown block gap between sibling rows of the
 * same part (BOTH rows markdown kinds — the guard keeps a chip following a
 * block on the ordinary small step); tool groups open with the medium step
 * and close into it.
 */
export function topGapFor(prev: TranscriptRow | null, row: TranscriptRow): number {
  if (row.turnStart) {
    return SPACE_LG;
  }
  const bothMarkdown =
    prev !== null &&
    (prev.rowKind.kind === "markdown" || prev.rowKind.kind === "liveMarkdown") &&
    (row.rowKind.kind === "markdown" || row.rowKind.kind === "liveMarkdown");
  if (bothMarkdown && partPrefix(prev.id) === partPrefix(row.id)) {
    return MD_BLOCK_GAP;
  }
  if (row.rowKind.kind === "toolGroup" || prev?.rowKind.kind === "toolGroup") {
    return SPACE_MD;
  }
  return SPACE_SM;
}

/**
 * Minimal splice for a row-set change: `[start, deleteCount, insertCount]`,
 * or `null` when the sets are identical by (id, version).
 */
export function diffRows(
  oldRows: readonly TranscriptRow[],
  newRows: readonly TranscriptRow[],
): [number, number, number] | null {
  const eq = (a: TranscriptRow, b: TranscriptRow): boolean => a.id === b.id && a.version === b.version;
  let prefix = 0;
  const maxPrefix = Math.min(oldRows.length, newRows.length);
  while (prefix < maxPrefix && eq(oldRows[prefix]!, newRows[prefix]!)) {
    prefix++;
  }
  if (prefix === oldRows.length && prefix === newRows.length) {
    return null;
  }
  let suffix = 0;
  const maxSuffix = Math.min(oldRows.length - prefix, newRows.length - prefix);
  while (suffix < maxSuffix && eq(oldRows[oldRows.length - 1 - suffix]!, newRows[newRows.length - 1 - suffix]!)) {
    suffix++;
  }
  return [prefix, oldRows.length - suffix - prefix, newRows.length - suffix - prefix];
}

// ---------------------------------------------------------------------------
// Viewport memory + own-turn runway (transcript.rs:2260-2470)
// ---------------------------------------------------------------------------

/** A stable per-chat viewport anchor (transcript.rs:2349). */
export interface ViewportAnchor {
  readonly rowId: string;
  readonly entryId: string;
  readonly fallbackIx: number;
  readonly offsetInRow: number;
}

/** A resolved scroll position: row index + offset inside that row. */
export interface RowOffset {
  readonly itemIx: number;
  readonly offsetInItem: number;
}

/**
 * `ViewportAnchor::capture` (:2357): the first row whose bottom crosses the
 * viewport top, keeping the intra-row offset. `null` when the list is empty.
 */
export function captureViewportAnchor(
  rows: readonly TranscriptRow[],
  scrollTop: number,
  positions: readonly number[],
  heights: readonly number[],
): ViewportAnchor | null {
  if (rows.length === 0) {
    return null;
  }
  for (let ix = 0; ix < rows.length; ix++) {
    const top = positions[ix] ?? 0;
    const bottom = top + (heights[ix] ?? 0);
    if (bottom > scrollTop + 0.5) {
      const row = rows[ix]!;
      return { rowId: row.id, entryId: row.entryId, fallbackIx: ix, offsetInRow: scrollTop - top };
    }
  }
  // Scrolled past the last row: pin to the last row, as the desktop's
  // `item_ix.min(rows.len() - 1)` does for a glued offset.
  const last = rows.length - 1;
  const row = rows[last]!;
  return { rowId: row.id, entryId: row.entryId, fallbackIx: last, offsetInRow: scrollTop - (positions[last] ?? 0) };
}

/** `resolve_exact` (:2368): the index of the row whose id matches, offset kept. */
function resolveViewportAnchorExact(anchor: ViewportAnchor, rows: readonly TranscriptRow[]): RowOffset | null {
  const itemIx = rows.findIndex((row) => row.id === anchor.rowId);
  if (itemIx < 0) {
    return null;
  }
  return { itemIx, offsetInItem: anchor.offsetInRow };
}

/**
 * `resolve` (:2376): exact first; otherwise stay in the same message entry,
 * choosing the surviving row nearest the old location (the intra-row offset
 * is no longer meaningful then); else the clamped index with offset 0.
 * `null` when the list is empty.
 */
export function resolveViewportAnchor(
  anchor: ViewportAnchor,
  rows: readonly TranscriptRow[],
  allowFallback: boolean,
): RowOffset | null {
  const exact = resolveViewportAnchorExact(anchor, rows);
  if (exact !== null) {
    return exact;
  }
  if (!allowFallback) {
    return null;
  }
  let best = -1;
  let bestDistance = Number.POSITIVE_INFINITY;
  rows.forEach((row, ix) => {
    if (row.entryId === anchor.entryId) {
      const distance = Math.abs(ix - anchor.fallbackIx);
      if (distance < bestDistance) {
        best = ix;
        bestDistance = distance;
      }
    }
  });
  if (best >= 0) {
    return { itemIx: best, offsetInItem: 0 };
  }
  if (rows.length === 0) {
    return null;
  }
  return { itemIx: Math.min(anchor.fallbackIx, rows.length - 1), offsetInItem: 0 };
}

/**
 * A locally-sent turn reserves the viewport below its prompt
 * (transcript.rs:2269). `held`: the runway still owns the viewport (glide →
 * hold) — any wheel/touch input releases it while the reservation stays as
 * plain scrollable space. `positioned`: the entry glide has landed; the hold
 * re-asserts the prompt's position after every layout.
 */
export interface OwnTurnAnchor {
  readonly chatId: string;
  readonly messageId: string;
  readonly held: boolean;
  readonly positioned: boolean;
  readonly seenPrompt: boolean;
}

/** `released_for_restore` (:2287): navigation restores the reservation only. */
export function ownTurnReleasedForRestore(anchor: OwnTurnAnchor): OwnTurnAnchor {
  return { ...anchor, held: false, positioned: false, seenPrompt: true };
}

/**
 * `observe_prompt` (:2294): a fresh send may install the anchor one
 * notification before its echo. Once the prompt has appeared, its later
 * disappearance is terminal — the runway must retire.
 */
export function ownTurnObservesPrompt(anchor: OwnTurnAnchor, exists: boolean): boolean {
  return exists || !anchor.seenPrompt;
}

/**
 * `PendingQueuedTurns` (transcript.rs:2307): locally-authored queue rows whose
 * ids have not appeared in the transcript yet. Registration is deliberately
 * inert; once a matching prompt materializes, the newest match becomes the
 * own-turn anchor.
 */
export class PendingQueuedTurns {
  #items: Array<{ chatId: string; messageId: string }> = [];

  /** Test seam. */
  get size(): number {
    return this.#items.length;
  }

  register(chatId: string, messageId: string): void {
    this.#items = this.#items.filter(
      (item) => item.chatId !== chatId || item.messageId !== messageId,
    );
    this.#items.push({ chatId, messageId });
    while (this.#items.length > MAX_PENDING_QUEUED_TURNS) {
      this.#items.shift();
    }
  }

  /**
   * Consume every candidate from this chat that is now present (a
   * `turn_start` row with `entry_id == messageId`) and return the newest.
   * Multiple rows can land in one doc frame; the last send owns the runway.
   */
  takeLatestMaterialized(chatId: string, rows: readonly TranscriptRow[]): string | null {
    let latest: string | null = null;
    this.#items = this.#items.filter((item) => {
      const materialized =
        item.chatId === chatId &&
        rows.some((row) => row.turnStart && row.entryId === item.messageId);
      if (materialized) {
        latest = item.messageId;
      }
      return !materialized;
    });
    return latest;
  }
}

/**
 * Session-local viewport state (transcript.rs:2401): chats that were
 * following their tail keep following it; only user-owned viewports restore a
 * concrete row anchor.
 */
export type SavedViewport =
  | { readonly kind: "followTail" }
  | {
      readonly kind: "anchored";
      readonly anchor: ViewportAnchor;
      readonly distanceFromBottom: number;
      /** The runway that made a short active turn scrollable, released. */
      readonly ownTurn: OwnTurnAnchor | null;
    };

/**
 * `SavedViewport::capture` (:2453): `null` when the rows are empty (a
 * partial replay must never overwrite an older snapshot).
 */
export function captureSavedViewport(
  rows: readonly TranscriptRow[],
  scrollTop: number,
  positions: readonly number[],
  heights: readonly number[],
  pinned: boolean,
  distanceFromBottom: number,
  ownTurn: OwnTurnAnchor | null,
): SavedViewport | null {
  if (rows.length === 0) {
    return null;
  }
  if (pinned) {
    return { kind: "followTail" };
  }
  const anchor = captureViewportAnchor(rows, scrollTop, positions, heights);
  if (anchor === null) {
    return null;
  }
  return { kind: "anchored", anchor, distanceFromBottom, ownTurn };
}

/** Per-chat viewport memory, LRU-bounded by `MAX_SAVED_VIEWPORTS`. */
export class SavedViewportCache {
  #map = new Map<string, SavedViewport>();

  get(chatId: string): SavedViewport | undefined {
    return this.#map.get(chatId);
  }

  save(chatId: string, viewport: SavedViewport): void {
    this.#map.delete(chatId);
    this.#map.set(chatId, viewport);
    while (this.#map.size > MAX_SAVED_VIEWPORTS) {
      const oldest = this.#map.keys().next();
      if (oldest.done) {
        break;
      }
      this.#map.delete(oldest.value);
    }
  }

  /** Test seam. */
  clear(): void {
    this.#map.clear();
  }
}

// ---------------------------------------------------------------------------
// Parse wiring (transcript.rs:1557-1650, `parse_for_row`)
// ---------------------------------------------------------------------------

/** One `parse_for_row` outcome — why the returned tree is what it is. */
export type ParseOutcome =
  | { readonly kind: "incremental"; readonly parsedBytes: number; readonly stablePrefixBlocks: number }
  | { readonly kind: "cached" }
  | { readonly kind: "handoff" }
  | { readonly kind: "full" };

export interface ParseEntry {
  readonly text: string;
  readonly live: boolean;
  readonly tree: BlockTree;
}

/** Leading top-level blocks whose char ranges are unchanged between trees. */
function stablePrefixBlocks(prior: BlockTree | undefined, next: BlockTree): number {
  if (prior === undefined) {
    return 0;
  }
  let stable = 0;
  while (
    stable < prior.blocks.length &&
    stable < next.blocks.length &&
    prior.blocks[stable]!.start === next.blocks[stable]!.start &&
    prior.blocks[stable]!.end === next.blocks[stable]!.end
  ) {
    stable++;
  }
  return stable;
}

/**
 * `parse_for_row`, the transcript's markdown parse wiring extracted for
 * testability — one call per text part per sync. Streaming parses with the
 * mended tail (display-only closers); settling reuses the live tree when the
 * sources match (the flicker-free handoff), serves settled trees from the
 * cache, and otherwise parses from scratch.
 */
export function parseForRow(
  state: Map<string, ParseEntry>,
  key: string,
  text: string,
  streaming: boolean,
): { tree: BlockTree; outcome: ParseOutcome } {
  const prior = state.get(key);
  if (streaming) {
    if (prior !== undefined && prior.live && prior.text === text) {
      return { tree: prior.tree, outcome: { kind: "incremental", parsedBytes: 0, stablePrefixBlocks: stablePrefixBlocks(prior.tree, prior.tree) } };
    }
    const tree = parseMarkdown(text, true);
    const parsedBytes =
      prior !== undefined && text.startsWith(prior.text) ? text.length - prior.text.length : text.length;
    const outcome: ParseOutcome = {
      kind: "incremental",
      parsedBytes,
      stablePrefixBlocks: stablePrefixBlocks(prior?.tree, tree),
    };
    state.set(key, { text, live: true, tree });
    return { tree, outcome };
  }
  if (prior !== undefined && prior.text === text) {
    if (!prior.live) {
      return { tree: prior.tree, outcome: { kind: "cached" } };
    }
    // Live→complete handoff: the live parser's exact tree is adopted — the
    // split rows then share the tree the unsplit row painted.
    state.set(key, { text, live: false, tree: prior.tree });
    return { tree: prior.tree, outcome: { kind: "handoff" } };
  }
  const tree = parseMarkdown(text, false);
  state.set(key, { text, live: false, tree });
  return { tree, outcome: { kind: "full" } };
}

// ---------------------------------------------------------------------------
// Delta application (doc/src/transcript_delta.rs apply_transcript_frame)
// ---------------------------------------------------------------------------

/** A frame that could not be applied cleanly — resubscribe for a reset. */
export class TranscriptDesync extends Error {
  constructor(message: string) {
    super(`transcript delta desync: ${message}`);
    this.name = "TranscriptDesync";
  }
}

/**
 * Apply a frame immutably: the returned array is new only when something
 * changed, and entries the frame doesn't touch keep their object identity so
 * React rows memoize across stream ticks. Throws `TranscriptDesync` on any
 * inconsistency — the consumer's copy has diverged and it must resubscribe.
 */
export function applyTranscriptFrame(
  current: readonly SessionMessageEntry[],
  frame: TranscriptFrame,
): readonly SessionMessageEntry[] {
  if ("reset" in frame) {
    return preserveIdentity(current, frame.reset);
  }
  const { upsert, append, remove, count } = frame;
  let next: SessionMessageEntry[] | null = null;
  const ensure = (): SessionMessageEntry[] => {
    if (next === null) {
      next = [...current];
    }
    return next;
  };

  if (remove.length > 0) {
    const gone = new Set(remove);
    const list = ensure().filter((entry) => !gone.has(entry.id));
    next = list;
  }
  for (const { after, entry } of upsert) {
    const list = ensure();
    const existing = list.findIndex((candidate) => candidate.id === entry.id);
    if (existing >= 0) {
      list.splice(existing, 1);
    }
    let at = 0;
    if (after !== null) {
      const anchor = list.findIndex((candidate) => candidate.id === after);
      if (anchor < 0) {
        throw new TranscriptDesync(`missing anchor ${after}`);
      }
      at = anchor + 1;
    }
    list.splice(at, 0, entry);
  }
  for (const { entry: entryId, part: partId, text, len } of append) {
    const list = ensure();
    const index = list.findIndex((candidate) => candidate.id === entryId);
    if (index < 0) {
      throw new TranscriptDesync(`missing append entry ${entryId}`);
    }
    const target = list[index]!;
    const partIndex = target.parts.findIndex(
      (candidate) => (candidate.kind === "text" || candidate.kind === "reasoning") && candidate.id === partId,
    );
    if (partIndex < 0) {
      throw new TranscriptDesync(`missing append part ${partId}`);
    }
    const oldPart = target.parts[partIndex] as Extract<MessagePart, { kind: "text" | "reasoning" }>;
    const grown = oldPart.text + text;
    if (grown.length !== len) {
      throw new TranscriptDesync(
        `append length mismatch on ${entryId}#${partId}: have ${grown.length}, expected ${len}`,
      );
    }
    const parts = [...target.parts];
    parts[partIndex] = { ...oldPart, text: grown };
    list[index] = { ...target, parts };
    next = list;
  }
  const result = next ?? current;
  if (result.length !== count) {
    throw new TranscriptDesync(`count mismatch: have ${result.length}, expected ${count}`);
  }
  return result;
}

/** Reuse entry identities across a reset when deep-equal (cache-swap case). */
function preserveIdentity(
  current: readonly SessionMessageEntry[],
  incoming: readonly SessionMessageEntry[],
): readonly SessionMessageEntry[] {
  const byId = new Map(current.map((entry) => [entry.id, entry]));
  let changed = incoming.length !== current.length;
  const next = incoming.map((entry) => {
    const existing = byId.get(entry.id);
    if (existing !== undefined && jsonEqual(existing, entry)) {
      return existing;
    }
    changed = true;
    return entry;
  });
  // Order is authoritative too: same identities in a new order must rebuild.
  const reordered = !changed && next.some((entry, ix) => current[ix] !== entry);
  return changed || reordered ? next : current;
}

function jsonEqual(a: unknown, b: unknown): boolean {
  if (a === b) {
    return true;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((value, index) => jsonEqual(value, b[index]));
  }
  if (typeof a === "object" && a !== null && typeof b === "object" && b !== null) {
    const keys = Object.keys(a as Record<string, unknown>);
    const other = b as Record<string, unknown>;
    return (
      keys.length === Object.keys(other).length &&
      keys.every((key) => Object.hasOwn(other, key) && jsonEqual((a as Record<string, unknown>)[key], other[key]))
    );
  }
  return false;
}

/** The text a streaming row veils: the block's flat visible text. */
export function rowFlatText(row: TranscriptRow): string | null {
  if (row.rowKind.kind !== "liveMarkdown" && row.rowKind.kind !== "markdown") {
    return null;
  }
  const top = row.rowKind.tree.blocks[row.rowKind.blockIx];
  return top === undefined ? null : blockFlatText(top.block);
}
