// The store's markdown, first pass: lines to blocks.
//
// The store draws a stated subset of markdown, the one Zeron's own README
// view draws (the publishing guide says it to authors):
//
//   ATX and setext headings, paragraphs, fenced and indented code, block
//   quotes, ordered, unordered and task lists, GFM pipe tables (at most
//   COLUMNS columns), thematic breaks, and link reference definitions.
//
// Nothing else is a block. An HTML block (CommonMark's seven kinds: a line
// that starts `<div`, `<!--`, `<script` and so on) is recognised as
// CommonMark recognises it, and kept as the characters typed: never parsed,
// never HTML (Zeron's own README view shows it the same way).
//
// Linear, by construction (the argument; the meter counts it, md/meter.ts,
// and md/html.test.ts holds the count to a constant per byte):
// - The text is read once into lines, each read once, left to right. The
//   open blocks are a stack at most NESTING containers deep, and each line
//   is matched against that stack once; a container's test looks at most a
//   few characters past the cursor (its indent is read once a line and
//   kept: `Line`), so a line costs its length plus a constant per open block.
// - Every test at a line's start (a quote's `>`, a heading's `#`s, a fence,
//   a rule, a list marker, a table's delimiter row, an HTML block's start)
//   reads forward from the cursor without going back. One that reads to the
//   line's end (a rule, a delimiter row, a whole tag) runs at most once for
//   each container opened on that line: at most NESTING times the line.
// - A list item whose marker is followed by a tab has that run of spaces
//   rewritten once: at most NESTING rewrites of a line, in the worst case.
// - Link reference definitions are taken from a paragraph's start when it
//   closes, each read once; the first that fails ends them. Of those, one
//   alone can read far (a title opened on the next line and never closed)
//   and it is the last read of that paragraph (takeDefinition). Each is
//   resolved once, here (its escapes undone, its address and title at most
//   ADDRESS and TITLE characters), so a use of it is a lookup.
// - A table's rows are kept as the text written, and split into cells
//   once, as each is written (`cells`, md/html.ts): what a table keeps is
//   its text, and the cells a short row lacks are written, never kept.
// - Tightness of a list is read from the lines its children start and end
//   at, once per child, as each closes.
// - A block becomes its finished form when it closes, and its parent keeps
//   only that: what is kept is the blocks still open (at most NESTING + 2)
//   and the finished ones, never both.
// - What is kept is counted as it is made (NODE for each block, and its
//   text), and the rendering stops past KEEP_MOST.
// - No regular expression runs over the author's text here.

import { keep, meter, NODE, release, type Meter } from "./meter.ts";
import { PUNCTUATION, unescape } from "./text.ts";

/** Containers (quotes and lists) one inside another; past it, a marker is text. */
export const NESTING = 32;
/** A table's columns, at most; a header with more is a paragraph. */
export const COLUMNS = 64;
/** A link label's characters, at most (CommonMark's own bound). */
export const LABEL = 999;
/** A link's address and title, at most, as written: past them, it is no link (its characters are text). */
export const ADDRESS = 2048;
export const TITLE = 1024;

export type Align = "left" | "center" | "right" | null;

export type Block =
  | { kind: "paragraph"; text: string }
  | { kind: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; text: string }
  | { kind: "code"; info: string; text: string }
  | { kind: "rule" }
  | { kind: "quote"; children: Block[] }
  | { kind: "list"; ordered: boolean; start: number; tight: boolean; items: Item[] }
  /** A table: its header's cells, and each row as written (`cells` splits one, to the header's width). */
  | { kind: "table"; align: Align[]; head: string[]; rows: string[] }
  /** An HTML block: its characters, as typed. */
  | { kind: "html"; text: string };

/** A list's item: a task's box (`[ ]` false, `[x]` true) or null, and its blocks. */
export interface Item {
  task: boolean | null;
  children: Block[];
}

/** A link reference definition, resolved: its escapes undone, once. */
export interface Definition {
  destination: string;
  title: string | null;
}

export interface Blocks {
  blocks: Block[];
  /** By normalized label (`labelKey`); the first definition of a label wins. */
  definitions: Map<string, Definition>;
  /** What was read and kept (md/meter.ts). */
  meter: Meter;
}

type Kind = "document" | "quote" | "list" | "item" | "paragraph" | "heading" | "fence" | "indented" | "rule" | "table" | "html";

interface Open {
  kind: Kind;
  /** A container's children, finished (made only for containers). */
  outs: (Block | Item)[];
  /** How many children it has had; where the last that closed ended; a blank line between two of them. */
  count: number;
  lastEnd: number;
  gap: boolean;
  /** The lines (0-based) it starts and ends at: a list's tightness is read from them. */
  start: number;
  end: number;
  lines: string[];
  // list
  ordered: boolean;
  marker: string;
  first: number;
  // item: the column its content starts at, from its container's
  offset: number;
  // heading
  level: number;
  text: string;
  // fence
  fence: string;
  fenceLength: number;
  fenceIndent: number;
  info: string;
  // html: which of CommonMark's seven kinds
  htmlKind: number;
  // table
  align: Align[];
  head: string[];
  rows: string[];
}

const NONE: never[] = [];

const node = (kind: Kind, line: number): Open => {
  if (M) keep(M, NODE);
  const container = kind === "document" || kind === "quote" || kind === "list" || kind === "item";
  const lined = kind === "paragraph" || kind === "fence" || kind === "indented" || kind === "html";
  return {
    kind,
    outs: container ? [] : NONE,
    count: 0,
    lastEnd: -1,
    gap: false,
    start: line,
    end: line,
    lines: lined ? [] : NONE,
    ordered: false,
    marker: "",
    first: 1,
    offset: 0,
    level: 0,
    text: "",
    fence: "",
    fenceLength: 0,
    fenceIndent: 0,
    info: "",
    htmlKind: 0,
    align: NONE,
    head: NONE,
    rows: NONE,
  };
};

/** The meter of the rendering under way (set by blocksOf; the helpers below count what they read). */
let M: Meter | null = null;
const read = (n: number) => {
  if (M) M.read += n;
};

const isSpace = (c: string | undefined) => c === " " || c === "\t";
const isDigit = (c: string | undefined) => c !== undefined && c >= "0" && c <= "9";

/** Spaces at `i` (tabs before the cursor are already spaces). */
function spaces(line: string, i: number): number {
  let j = i;
  while (line[j] === " ") j++;
  read(j - i);
  return j - i;
}

function blankFrom(line: string, i: number): boolean {
  for (let j = i; j < line.length; j++) {
    if (!isSpace(line[j])) {
      read(j - i + 1);
      return false;
    }
  }
  read(line.length - i);
  return true;
}

/**
 * A line as the block pass reads it: where its text ends (its trailing
 * spaces aside) and, for a cursor that only moves forward, where the next
 * character that isn't a space is, each found once and kept.
 */
class Line {
  text: string;
  /** Past the last character that isn't a space or a tab. */
  end = 0;
  private from = -1;
  private to = 0;
  constructor(text: string) {
    this.text = text;
    this.measure();
  }
  /** (Again, after the line's text is rewritten: a tab after a list marker.) */
  measure() {
    let end = this.text.length;
    while (end > 0 && isSpace(this.text[end - 1])) end--;
    read(this.text.length - end);
    this.end = end;
    this.from = -1;
  }
  set(text: string) {
    this.text = text;
    this.measure();
  }
  /** The first index at or after `i` that isn't a space. */
  nonSpace(i: number): number {
    if (this.from >= 0 && i >= this.from && i <= this.to) return this.to;
    let j = i;
    while (this.text[j] === " ") j++;
    read(j - i);
    this.from = i;
    this.to = j;
    return j;
  }
  indent(i: number) {
    return this.nonSpace(i) - i;
  }
  blank(i: number) {
    return i >= this.end;
  }
}

/**
 * The run of spaces and tabs at `from`, its tabs made spaces to the next
 * stop of 4 columns. (Nothing before `from` is a tab, so a column is an index.)
 */
function expandRun(line: string, from: number): string {
  let j = from;
  while (isSpace(line[j])) j++;
  read(j - from);
  if (!line.slice(from, j).includes("\t")) return line;
  let run = "";
  for (let k = from; k < j; k++) run += line[k] === "\t" ? " ".repeat(4 - ((from + run.length) % 4)) : " ";
  read(line.length + run.length);
  return line.slice(0, from) + run + line.slice(j);
}

/** A line's lead (its spaces, tabs and quotes' `>`), tabs made spaces. */
function expandLead(line: string): string {
  let j = 0;
  while (j < line.length && (isSpace(line[j]) || line[j] === ">")) j++;
  read(j);
  if (!line.slice(0, j).includes("\t")) return line;
  read(line.length + j * 4);
  let lead = "";
  for (let k = 0; k < j; k++) lead += line[k] === "\t" ? " ".repeat(4 - (lead.length % 4)) : line[k];
  return lead + line.slice(j);
}

/** An ATX heading at `at`: its level and text, or null. */
function atx(line: string, at: number): { level: number; text: string } | null {
  let j = at;
  while (line[j] === "#" && j - at < 7) j++;
  const level = j - at;
  if (level < 1 || level > 6 || (j < line.length && !isSpace(line[j]))) return null;
  read(line.length - at);
  let end = line.length;
  while (end > j && isSpace(line[end - 1])) end--;
  // A closing run of `#`s, after a space (or the whole of it), is not its text.
  let hashes = end;
  while (hashes > j && line[hashes - 1] === "#") hashes--;
  if (hashes < end && (hashes === j || isSpace(line[hashes - 1]))) end = hashes;
  return { level, text: line.slice(j, end).trim() };
}

/** A fence's opening at `at`: its character, length and info string, or null. */
function fenceOpen(line: string, at: number): { fence: string; length: number; info: string } | null {
  const c = line[at];
  if (c !== "`" && c !== "~") return null;
  let j = at;
  while (line[j] === c) j++;
  read(j - at);
  if (j - at < 3) return null;
  read(line.length - j);
  const info = line.slice(j).trim();
  if (c === "`" && info.includes("`")) return null;
  return { fence: c, length: j - at, info };
}

function fenceCloses(line: Line, i: number, open: Open): boolean {
  const indent = line.indent(i);
  if (indent > 3) return false;
  let j = i + indent;
  const from = j;
  while (line.text[j] === open.fence) j++;
  read(j - from);
  return j - from >= open.fenceLength && line.blank(j);
}

/** A thematic break at `at`: three or more of one of `*`, `-`, `_`, with spaces between. */
function rule(line: string, at: number): boolean {
  const c = line[at];
  if (c !== "*" && c !== "-" && c !== "_") return false;
  let count = 0;
  for (let j = at; j < line.length; j++) {
    if (line[j] === c) count++;
    else if (!isSpace(line[j])) {
      read(j - at + 1);
      return false;
    }
  }
  read(line.length - at);
  return count >= 3;
}

/** A setext underline at `at`: its level, or 0. */
function underline(line: string, at: number): number {
  const c = line[at];
  if (c !== "=" && c !== "-") return 0;
  let j = at;
  while (line[j] === c) j++;
  read(j - at);
  return blankFrom(line, j) ? (c === "=" ? 1 : 2) : 0;
}

interface Marker {
  ordered: boolean;
  marker: string;
  first: number;
  /** The line, its spaces after the marker made spaces. */
  line: string;
  /** Where its content starts, and whether it has any on this line. */
  content: number;
  blank: boolean;
  /** From the marker's own column to its content's. */
  width: number;
}

/** A list item's marker at `at`, or null (`interrupts`: it would interrupt a paragraph). */
function listMarker(line: string, at: number, interrupts: boolean): Marker | null {
  const c = line[at];
  let j = at;
  let ordered = false;
  let first = 1;
  let marker: string;
  if (c === "-" || c === "+" || c === "*") {
    marker = c;
    j++;
  } else if (isDigit(c)) {
    while (isDigit(line[j]) && j - at < 9) j++;
    read(j - at);
    const d = line[j];
    if (d !== "." && d !== ")") return null;
    ordered = true;
    first = Number(line.slice(at, j));
    marker = d;
    j++;
  } else return null;
  if (j < line.length && !isSpace(line[j])) return null;
  const expanded = expandRun(line, j);
  const blank = blankFrom(expanded, j);
  if (interrupts && (blank || (ordered && first !== 1))) return null;
  const after = spaces(expanded, j);
  // Five or more spaces: one belongs to the marker, the rest are an indented code block's.
  const padding = blank || after >= 5 ? 1 : after;
  return { ordered, marker, first, line: expanded, content: j + (blank ? after : padding), blank, width: j - at + padding };
}

/** A table row's cells, split at unescaped pipes; `\|` is a pipe in its cell. At most `most` (past them, not read). */
export function cells(text: string, most = Infinity): string[] {
  const out: string[] = [];
  let i = 0;
  while (isSpace(text[i])) i++;
  let end = text.length;
  while (end > i && isSpace(text[end - 1])) end--;
  if (text[i] === "|") i++;
  if (end > i && text[end - 1] === "|" && text[end - 2] !== "\\") end--;
  let from = i;
  let escaped = false;
  const push = (to: number) => {
    let cell = text.slice(from, to);
    if (escaped) cell = cell.replaceAll("\\|", "|");
    out.push(cell.trim());
    escaped = false;
  };
  let j = i;
  for (; j < end && out.length < most; j++) {
    const ch = text[j];
    if (ch === "\\" && j + 1 < end) {
      if (text[j + 1] === "|") escaped = true;
      j++;
    } else if (ch === "|") {
      push(j);
      from = j + 1;
    }
  }
  if (out.length < most) push(end);
  read(j + (text.length - end));
  return out;
}

/** A table's delimiter row: each cell's alignment, or null. */
function delimiterRow(line: string, at: number): Align[] | null {
  const pipe = line.indexOf("|", at);
  read((pipe < 0 ? line.length : pipe) - at);
  if (pipe < 0) return null;
  const align: Align[] = [];
  for (const cell of cells(line.slice(at))) {
    let j = 0;
    const left = cell[j] === ":";
    if (left) j++;
    const from = j;
    while (cell[j] === "-") j++;
    if (j === from) return null;
    const right = cell[j] === ":";
    if (right) j++;
    if (j !== cell.length) return null;
    align.push(left && right ? "center" : left ? "left" : right ? "right" : null);
  }
  return align;
}

/** Tags whose block runs to their closing tag (CommonMark's kind 1). */
const RAW_TAGS = ["pre", "script", "style", "textarea"];

/** Tags that start a block of kind 6 (CommonMark's list). */
const BLOCK_TAGS = new Set(
  "address article aside base basefont blockquote body caption center col colgroup dd details dialog dir div dl dt fieldset figcaption figure footer form frame frameset h1 h2 h3 h4 h5 h6 head header hr html iframe legend li link main menu menuitem nav noframes ol optgroup option p param search section summary table tbody td tfoot th thead title tr track ul".split(" "),
);

const isLetter = (c: string | undefined) => c !== undefined && ((c >= "a" && c <= "z") || (c >= "A" && c <= "Z"));
const isNameChar = (c: string | undefined) => isLetter(c) || isDigit(c) || c === "-";

/**
 * Whether the rest of a line from `at` is one whole open or closing tag and
 * spaces (CommonMark's kind 7), read once.
 */
function wholeTag(line: string, at: number): boolean {
  let j = at + 1;
  const closing = line[j] === "/";
  if (closing) j++;
  if (!isLetter(line[j])) return false;
  const name = j;
  while (isNameChar(line[j])) j++;
  read(line.length - at);
  if (RAW_TAGS.includes(line.slice(name, j).toLowerCase())) return false;
  if (!closing) {
    for (;;) {
      const space = j;
      while (isSpace(line[j])) j++;
      const c = line[j];
      if (!(isLetter(c) || c === "_" || c === ":") || j === space) break;
      while (isLetter(line[j]) || isDigit(line[j]) || "_.:-".includes(line[j] ?? " ")) j++;
      let k = j;
      while (isSpace(line[k])) k++;
      if (line[k] !== "=") continue;
      k++;
      while (isSpace(line[k])) k++;
      const q = line[k];
      if (q === '"' || q === "'") {
        const end = line.indexOf(q, k + 1);
        if (end < 0) return false;
        j = end + 1;
      } else {
        const from = k;
        while (k < line.length && !isSpace(line[k]) && !"\"'=<>`".includes(line[k])) k++;
        if (k === from) return false;
        j = k;
      }
    }
    if (line[j] === "/") j++;
  } else while (isSpace(line[j])) j++;
  if (line[j] !== ">") return false;
  return blankFrom(line, j + 1);
}

/** Which of CommonMark's HTML blocks starts at `at`, or 0 (`interrupts`: it would interrupt a paragraph). */
function htmlStart(line: string, at: number, interrupts: boolean): number {
  if (line[at] !== "<") return 0;
  read(12);
  const start = line.slice(at, at + 12).toLowerCase();
  for (const tag of RAW_TAGS) {
    if (start.startsWith(`<${tag}`)) {
      const after = line[at + 1 + tag.length];
      if (after === undefined || isSpace(after) || after === ">") return 1;
    }
  }
  if (start.startsWith("<!--")) return 2;
  if (start.startsWith("<?")) return 3;
  if (start.startsWith("<![cdata[")) return 5;
  if (start.startsWith("<!") && isLetter(line[at + 2])) return 4;
  let j = at + 1;
  if (line[j] === "/") j++;
  const name = j;
  while ((isLetter(line[j]) || isDigit(line[j])) && j - name < 16) j++;
  read(j - name);
  if (BLOCK_TAGS.has(line.slice(name, j).toLowerCase())) {
    const after = line[j];
    if (after === undefined || isSpace(after) || after === ">" || (after === "/" && line[j + 1] === ">")) return 6;
  }
  return !interrupts && wholeTag(line, at) ? 7 : 0;
}

/** Whether a line ends an HTML block of `kind` (kinds 6 and 7 end at a blank line instead). */
function htmlEnds(kind: number, line: string): boolean {
  if (kind <= 5) read(kind === 1 ? line.length * 5 : line.length);
  switch (kind) {
    case 1: {
      const lower = line.toLowerCase();
      return RAW_TAGS.some((tag) => lower.includes(`</${tag}>`));
    }
    case 2:
      return line.includes("-->");
    case 3:
      return line.includes("?>");
    case 4:
      return line.includes(">");
    case 5:
      return line.includes("]]>");
    default:
      return false;
  }
}

/** A label as definitions are found by: its spaces made one, case folded. */
export function labelKey(label: string): string {
  read(label.length * 3);
  let out = "";
  let space = false;
  for (const ch of label.trim()) {
    if (ch === " " || ch === "\t" || ch === "\n") space = true;
    else {
      if (space) out += " ";
      space = false;
      out += ch;
    }
  }
  // (Folded as Unicode folds case: "ẞ" and "SS" are one label.)
  return out.toLowerCase().toUpperCase().toLowerCase();
}

/** Spaces and tabs, then at most one line end, then spaces and tabs. */
function skipToNext(s: string, j: number): number {
  while (isSpace(s[j])) j++;
  if (s[j] === "\n") {
    j++;
    while (isSpace(s[j])) j++;
  }
  return j;
}

/**
 * A link reference definition at `p` of a paragraph's text: where it ends
 * (past its line end), its label and what it says; or null.
 *
 * It reads forward once. A title that doesn't close (or has more after it)
 * is read up to where it gives up; the definition may still stand without
 * it when the title began on the next line, but then the next attempt starts
 * at that title, which is not a `[`, and the paragraph's definitions end. So
 * a paragraph has at most one such long read.
 */
function takeDefinition(s: string, p: number): { end: number; label: string; definition: Definition } | null {
  if (s[p] !== "[") return null;
  let j = p + 1;
  while (j < s.length && j - p - 1 <= LABEL) {
    const c = s[j];
    if (c === "\\" && j + 1 < s.length) j += 2;
    else if (c === "[") return null;
    else if (c === "]") break;
    else j++;
  }
  if (s[j] !== "]" || j - p - 1 > LABEL) return null;
  const label = s.slice(p + 1, j);
  if (label.trim() === "" || s[j + 1] !== ":") return null;
  j = skipToNext(s, j + 2);
  let destination: string;
  if (s[j] === "<") {
    let k = j + 1;
    while (k < s.length && s[k] !== ">" && s[k] !== "\n" && s[k] !== "<") k += s[k] === "\\" && k + 1 < s.length && s[k + 1] !== "\n" ? 2 : 1;
    if (s[k] !== ">") return null;
    destination = s.slice(j + 1, k);
    j = k + 1;
  } else {
    let k = j;
    let depth = 0;
    while (k < s.length) {
      const c = s[k];
      if (c === "\\" && k + 1 < s.length && PUNCTUATION.includes(s[k + 1])) {
        k += 2;
        continue;
      }
      if (c <= " ") break;
      if (c === "(") {
        if (++depth > NESTING) return null;
      } else if (c === ")") {
        if (depth === 0) break;
        depth--;
      }
      k++;
    }
    if (k === j || depth !== 0) return null;
    destination = s.slice(j, k);
    j = k;
  }
  if (destination.length > ADDRESS) return null;
  // Where it ends without a title: the end of the destination's line.
  let bare = j;
  while (isSpace(s[bare])) bare++;
  const bareEnd = bare >= s.length ? s.length : s[bare] === "\n" ? bare + 1 : -1;
  const t = skipToNext(s, j);
  const open = s[t];
  if (t > j && (open === '"' || open === "'" || open === "(")) {
    const close = open === "(" ? ")" : open;
    let k = t + 1;
    while (k < s.length && s[k] !== close) {
      if (s[k] === "\\" && k + 1 < s.length) k += 2;
      else if (open === "(" && s[k] === "(") break;
      else k++;
    }
    if (s[k] === close && k - t - 1 <= TITLE) {
      let after = k + 1;
      while (isSpace(s[after])) after++;
      if (after >= s.length || s[after] === "\n") {
        return { end: Math.min(s.length, after + 1), label, definition: { destination, title: s.slice(t + 1, k) } };
      }
    }
  }
  return bareEnd < 0 ? null : { end: bareEnd, label, definition: { destination, title: null } };
}

/** The blocks of `markdown`, counted on `m` (which may stop it: TooLarge). */
export function blocksOf(markdown: string, m: Meter = meter()): Blocks {
  M = m;
  try {
    return parse(markdown, m);
  } finally {
    M = null;
  }
}

function parse(markdown: string, m: Meter): Blocks {
  const definitions = new Map<string, Definition>();
  const document = node("document", 0);
  const open: Open[] = [document];

  // Quotes and lists open now (NESTING bounds it).
  let containers = 0;
  const depth = () => containers;

  /** A paragraph's definitions taken (and resolved, once); what's left is its text. */
  const paragraphText = (p: Open): string => {
    const s = p.lines.length === 1 ? p.lines[0] : p.lines.join("\n");
    let at = 0;
    for (;;) {
      const d = takeDefinition(s, at);
      // (A definition read is its length; the one that fails, the most it could have read.)
      read((d ? d.end : s.length) - at);
      if (!d) break;
      const key = labelKey(d.label);
      if (!definitions.has(key)) {
        const resolved = { destination: unescape(d.definition.destination, m), title: d.definition.title === null ? null : unescape(d.definition.title, m) };
        keep(m, NODE + resolved.destination.length + (resolved.title?.length ?? 0));
        definitions.set(key, resolved);
      }
      at = d.end;
    }
    let end = s.length;
    while (end > at && isSpace(s[end - 1])) end--;
    return s.slice(at, end);
  };

  /** A block closed: its finished form, given to its parent (with what it says of the parent's tightness). */
  const finalize = (block: Open, line: number) => {
    const parent = open[open.length - 1];
    let out: Block | Item | null = null;
    switch (block.kind) {
      case "paragraph": {
        const text = paragraphText(block);
        block.lines = NONE;
        if (text !== "") out = { kind: "paragraph", text };
        break;
      }
      case "heading":
        out = { kind: "heading", level: block.level as 1 | 2 | 3 | 4 | 5 | 6, text: block.text };
        break;
      case "fence":
        out = { kind: "code", info: block.info, text: block.lines.length > 0 ? `${block.lines.join("\n")}\n` : "" };
        break;
      case "indented":
        while (block.lines.length > 0 && blankFrom(block.lines[block.lines.length - 1], 0)) block.lines.pop();
        out = { kind: "code", info: "", text: `${block.lines.join("\n")}\n` };
        break;
      case "html": {
        while (block.lines.length > 0 && block.lines[block.lines.length - 1] === "") block.lines.pop();
        out = { kind: "html", text: block.lines.join("\n") };
        break;
      }
      case "rule":
        out = { kind: "rule" };
        break;
      case "table":
        out = { kind: "table", align: block.align, head: block.head, rows: block.rows };
        break;
      case "quote":
        block.end = Math.max(block.start, line);
        out = { kind: "quote", children: block.outs as Block[] };
        break;
      case "item": {
        block.end = block.lastEnd >= 0 ? block.lastEnd : block.start;
        const children = block.outs as Block[];
        out = { task: taskOf(children), children };
        break;
      }
      case "list":
        block.end = block.lastEnd >= 0 ? block.lastEnd : block.start;
        out = { kind: "list", ordered: block.ordered, start: block.first, tight: !block.gap, items: block.outs as Item[] };
        break;
      default:
        break;
    }
    // (The open block goes; its finished form stays: about as much, with its arrays.)
    release(m, NODE);
    if (!out) return;
    keep(m, NODE);
    parent.outs.push(out);
    // A list is loose when a blank line is between two of its items, or between two blocks of one item.
    if (parent.kind === "item" || parent.kind === "list") {
      if (parent.lastEnd >= 0 && block.start > parent.lastEnd + 1) parent.gap = true;
      if (block.kind === "item" && block.gap) parent.gap = true;
    }
    parent.lastEnd = block.end;
  };

  const close = (line: number) => {
    const block = open.pop()!;
    if (block.kind === "quote" || block.kind === "list") containers--;
    finalize(block, line);
  };

  const canContain = (parent: Kind, child: Kind) =>
    parent === "list" ? child === "item" : (parent === "document" || parent === "quote" || parent === "item") && child !== "item";

  const add = (kind: Kind, line: number): Open => {
    while (!canContain(open[open.length - 1].kind, kind)) close(line - 1);
    const block = node(kind, line);
    if (kind === "quote" || kind === "list") containers++;
    open[open.length - 1].count++;
    open.push(block);
    return block;
  };

  /** A line's text from `from`, kept by the block it joins (counted). */
  const kept = (text: string, from: number) => {
    keep(m, text.length - from + 16);
    return text.slice(from);
  };

  // Lines end at \n, \r\n or \r alone; each is found once.
  let pos = 0;
  let n = -1;
  while (pos < markdown.length) {
    n++;
    let stop = pos;
    while (stop < markdown.length && markdown[stop] !== "\n" && markdown[stop] !== "\r") stop++;
    const raw = markdown.slice(pos, stop);
    pos = stop + (markdown[stop] === "\r" && markdown[stop + 1] === "\n" ? 2 : 1);
    // (Its characters, its end, and one step for each open block it is matched against.)
    read(raw.length + 1 + open.length);
    const line = new Line(expandLead(raw.includes("\0") ? raw.replaceAll("\0", "\uFFFD") : raw));
    let i = 0;
    let handled = false;

    // 1. The open blocks this line continues.
    let matched = 0;
    for (let k = 1; k < open.length; k++) {
      const block = open[k];
      const indent = line.indent(i);
      const blank = line.blank(i);
      let next = -1;
      switch (block.kind) {
        case "quote":
          if (indent <= 3 && line.text[i + indent] === ">") {
            next = i + indent + 1;
            if (line.text[next] === " ") next++;
          }
          break;
        case "list":
          next = i;
          break;
        case "item":
          if (blank) next = block.count === 0 ? -1 : i + indent;
          else if (indent >= block.offset) next = i + block.offset;
          break;
        case "paragraph":
        case "table":
          if (!blank) next = i;
          break;
        case "html":
          if (!blank || block.htmlKind <= 5) next = i;
          break;
        case "fence":
          if (fenceCloses(line, i, block)) {
            block.end = n;
            close(n);
            handled = true;
          } else next = i;
          break;
        case "indented":
          if (indent >= 4) next = i + 4;
          else if (blank) next = i + indent;
          break;
        default:
          break;
      }
      if (handled) break;
      if (next < 0) break;
      i = next;
      matched = k;
    }
    if (handled) continue;

    let container = open[matched];
    let closedUnmatched = matched === open.length - 1;
    const closeUnmatched = () => {
      if (closedUnmatched) return;
      while (open.length - 1 > matched) close(n - 1);
      closedUnmatched = true;
    };

    // 2. The blocks it opens.
    let done = false;
    while (container.kind !== "fence" && container.kind !== "indented" && container.kind !== "html") {
      const indent = line.indent(i);
      const at = i + indent;
      const blank = line.blank(at);
      if (indent >= 4) {
        if (open[open.length - 1].kind !== "paragraph" && !blank) {
          closeUnmatched();
          i += 4;
          container = add("indented", n);
        }
        break;
      }
      const text = line.text;
      const c = text[at];
      if (c === ">" && depth() < NESTING) {
        closeUnmatched();
        i = at + 1;
        if (text[i] === " ") i++;
        container = add("quote", n);
        matched = open.length - 1;
        continue;
      }
      if (c === "#") {
        const h = atx(text, at);
        if (h) {
          closeUnmatched();
          const heading = add("heading", n);
          heading.level = h.level;
          heading.text = h.text;
          keep(m, h.text.length);
          close(n);
          done = true;
          break;
        }
      }
      if (c === "`" || c === "~") {
        const f = fenceOpen(text, at);
        if (f) {
          closeUnmatched();
          container = add("fence", n);
          container.fence = f.fence;
          container.fenceLength = f.length;
          container.fenceIndent = indent;
          container.info = f.info;
          done = true;
          break;
        }
      }
      if (c === "<") {
        const kind = htmlStart(text, at, container.kind === "paragraph");
        if (kind) {
          closeUnmatched();
          container = add("html", n);
          container.htmlKind = kind;
          break;
        }
      }
      if (container.kind === "paragraph" && closedUnmatched) {
        const level = underline(text, at);
        if (level) {
          const words = paragraphText(container);
          if (words !== "") {
            container.kind = "heading";
            container.level = level;
            container.text = words;
            container.lines = NONE;
            container.end = n;
            close(n);
            done = true;
            break;
          }
          // (Only definitions: they were taken; what's left is no paragraph.)
          container.lines = [];
        }
        const align = container.lines.length > 0 ? delimiterRow(text, at) : null;
        const head = align && align.length <= COLUMNS ? cells(container.lines[container.lines.length - 1], COLUMNS + 1) : null;
        if (align && head && head.length === align.length) {
          container.lines.pop();
          container.end = n - 2;
          close(n - 2);
          const table = add("table", n - 1);
          table.align = align;
          table.head = head;
          table.rows = [];
          table.end = n;
          container = table;
          done = true;
          break;
        }
      }
      if (rule(text, at)) {
        closeUnmatched();
        add("rule", n);
        close(n);
        done = true;
        break;
      }
      if (depth() < NESTING) {
        const marker = listMarker(text, at, container.kind === "paragraph");
        if (marker) {
          closeUnmatched();
          if (marker.line !== text) line.set(marker.line);
          if (!(container.kind === "list" && container.ordered === marker.ordered && container.marker === marker.marker)) {
            container = add("list", n);
            container.ordered = marker.ordered;
            container.marker = marker.marker;
            container.first = marker.first;
          }
          container = add("item", n);
          container.offset = indent + marker.width;
          i = marker.content;
          matched = open.length - 1;
          continue;
        }
      }
      break;
    }
    if (done) continue;

    // 3. Its text: a lazy paragraph's, or the open block's, or a new paragraph's.
    const blank = line.blank(i);
    const tip = open[open.length - 1];
    if (!closedUnmatched && !blank && tip.kind === "paragraph") {
      tip.lines.push(kept(line.text, line.nonSpace(i)));
      tip.end = n;
      continue;
    }
    closeUnmatched();
    switch (container.kind) {
      case "paragraph":
        container.lines.push(kept(line.text, line.nonSpace(i)));
        container.end = n;
        break;
      case "fence": {
        const strip = Math.min(line.indent(i), container.fenceIndent);
        container.lines.push(kept(line.text, i + strip));
        container.end = n;
        break;
      }
      case "indented":
        container.lines.push(kept(line.text, i));
        if (!blank) container.end = n;
        break;
      case "html": {
        const rest = kept(line.text, i);
        container.lines.push(rest);
        container.end = n;
        if (htmlEnds(container.htmlKind, rest)) close(n);
        break;
      }
      case "table": {
        // (As written: split when it is written.)
        const row = kept(line.text, i);
        container.rows.push(row);
        container.end = n;
        break;
      }
      default:
        if (!blank) {
          const p = add("paragraph", n);
          p.lines.push(kept(line.text, line.nonSpace(i)));
        }
        break;
    }
  }
  while (open.length > 1) close(n);

  return { blocks: document.outs as Block[], definitions, meter: m };
}

/** A task item: its first paragraph starts `[ ]` or `[x]` and a space; the box is taken from its text. */
function taskOf(children: Block[]): boolean | null {
  const first = children[0];
  if (first?.kind !== "paragraph") return null;
  const t = first.text;
  if (t[0] !== "[" || t[2] !== "]" || !(t[3] === " " || t[3] === "\t" || t[3] === "\n")) return null;
  const box = t[1];
  if (box !== " " && box !== "x" && box !== "X") return null;
  let j = 4;
  while (isSpace(t[j])) j++;
  first.text = t.slice(j);
  return box !== " ";
}
