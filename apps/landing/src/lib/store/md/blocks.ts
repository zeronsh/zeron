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
// Linear, by construction (the argument; markdown.test.ts measures it):
// - Each line is read once, left to right. The open blocks are a stack at
//   most NESTING containers deep, and each line is matched against that
//   stack once: at most NESTING + 2 steps a line, before its text.
// - Every test at a line's start (a quote's `>`, a heading's `#`s, a fence,
//   a rule, a list marker, a table's delimiter row, an HTML block's start)
//   reads forward from the cursor without going back, and each runs at most
//   once per container it could open on that line. An HTML block's end is
//   looked for in each of its lines once.
// - A list item whose marker is followed by a tab has that run of spaces
//   rewritten once: at most NESTING rewrites of a line, in the worst case.
// - Link reference definitions are taken from a paragraph's start when it
//   closes, each read once; the first that fails ends them. Of those, one
//   alone can read far (a title opened on the next line and never closed)
//   and it is the last read of that paragraph (takeDefinition).
// - A table's rows are split once each. A row has at most COLUMNS cells:
//   missing ones are filled, extra ones dropped, so a table's cells are at
//   most COLUMNS times its rows (the only place the output can be larger
//   than the input by more than a few bytes in each, and it is bounded).
// - Tightness of a list is read from the lines its children start and end
//   at, once per child.
// - No regular expression runs over the author's text here, except the one
//   that splits it into lines (/\r\n|\r|\n/: no repeat).
//
// `work` counts what was read: each line's characters, the steps matching
// it against the open blocks, each definition's characters, each table
// row's characters. The tests hold it to a constant per byte.

/** Containers (quotes and lists) one inside another; past it, a marker is text. */
export const NESTING = 32;
/** A table's columns, at most; a header with more is a paragraph. */
export const COLUMNS = 64;
/** A link label's characters, at most (CommonMark's own bound). */
export const LABEL = 999;

export type Align = "left" | "center" | "right" | null;

export type Block =
  | { kind: "paragraph"; text: string }
  | { kind: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; text: string }
  | { kind: "code"; info: string; text: string }
  | { kind: "rule" }
  | { kind: "quote"; children: Block[] }
  | { kind: "list"; ordered: boolean; start: number; tight: boolean; items: Item[] }
  | { kind: "table"; align: Align[]; head: string[]; rows: string[][] }
  /** An HTML block: its characters, as typed. */
  | { kind: "html"; text: string };

/** A list's item: a task's box (`[ ]` false, `[x]` true) or null, and its blocks. */
export interface Item {
  task: boolean | null;
  children: Block[];
}

/** A link reference definition, as written (the inline pass undoes escapes). */
export interface Definition {
  destination: string;
  title: string | null;
}

export interface Blocks {
  blocks: Block[];
  /** By normalized label (`labelKey`); the first definition of a label wins. */
  definitions: Map<string, Definition>;
  /** What was read (see above). */
  work: number;
}

type Kind = "document" | "quote" | "list" | "item" | "paragraph" | "heading" | "fence" | "indented" | "rule" | "table" | "html";

interface Open {
  kind: Kind;
  children: Open[];
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
  rows: string[][];
}

const node = (kind: Kind, line: number): Open => ({
  kind,
  children: [],
  start: line,
  end: line,
  lines: [],
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
  align: [],
  head: [],
  rows: [],
});

const isSpace = (c: string | undefined) => c === " " || c === "\t";
const isDigit = (c: string | undefined) => c !== undefined && c >= "0" && c <= "9";

/** Spaces at `i` (tabs before the cursor are already spaces). */
function spaces(line: string, i: number): number {
  let j = i;
  while (line[j] === " ") j++;
  return j - i;
}

function blankFrom(line: string, i: number): boolean {
  for (let j = i; j < line.length; j++) if (!isSpace(line[j])) return false;
  return true;
}

/**
 * The run of spaces and tabs at `from`, its tabs made spaces to the next
 * stop of 4 columns. (Nothing before `from` is a tab, so a column is an index.)
 */
function expandRun(line: string, from: number): string {
  let j = from;
  while (isSpace(line[j])) j++;
  if (!line.slice(from, j).includes("\t")) return line;
  let run = "";
  for (let k = from; k < j; k++) run += line[k] === "\t" ? " ".repeat(4 - ((from + run.length) % 4)) : " ";
  return line.slice(0, from) + run + line.slice(j);
}

/** A line's lead (its spaces, tabs and quotes' `>`), tabs made spaces. */
function expandLead(line: string): string {
  let j = 0;
  while (j < line.length && (isSpace(line[j]) || line[j] === ">")) j++;
  if (!line.slice(0, j).includes("\t")) return line;
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
  if (j - at < 3) return null;
  const info = line.slice(j).trim();
  if (c === "`" && info.includes("`")) return null;
  return { fence: c, length: j - at, info };
}

function fenceCloses(line: string, i: number, open: Open): boolean {
  const indent = spaces(line, i);
  if (indent > 3) return false;
  let j = i + indent;
  const from = j;
  while (line[j] === open.fence) j++;
  return j - from >= open.fenceLength && blankFrom(line, j);
}

/** A thematic break at `at`: three or more of one of `*`, `-`, `_`, with spaces between. */
function rule(line: string, at: number): boolean {
  const c = line[at];
  if (c !== "*" && c !== "-" && c !== "_") return false;
  let count = 0;
  for (let j = at; j < line.length; j++) {
    if (line[j] === c) count++;
    else if (!isSpace(line[j])) return false;
  }
  return count >= 3;
}

/** A setext underline at `at`: its level, or 0. */
function underline(line: string, at: number): number {
  const c = line[at];
  if (c !== "=" && c !== "-") return 0;
  let j = at;
  while (line[j] === c) j++;
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

/** A table row's cells, split at unescaped pipes; `\|` is a pipe in its cell. */
function cells(text: string): string[] {
  const out: string[] = [];
  let i = 0;
  while (isSpace(text[i])) i++;
  let end = text.length;
  while (end > i && isSpace(text[end - 1])) end--;
  if (text[i] === "|") i++;
  if (end > i && text[end - 1] === "|" && text[end - 2] !== "\\") end--;
  let cell = "";
  for (let j = i; j < end; j++) {
    const ch = text[j];
    if (ch === "\\" && j + 1 < end) {
      cell += text[j + 1] === "|" ? "|" : ch + text[j + 1];
      j++;
    } else if (ch === "|") {
      out.push(cell.trim());
      cell = "";
    } else cell += ch;
  }
  out.push(cell.trim());
  return out;
}

/** A table's delimiter row: each cell's alignment, or null. */
function delimiterRow(line: string, at: number): Align[] | null {
  if (!line.includes("|", at)) return null;
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
  while (isLetter(line[j]) || isDigit(line[j])) j++;
  if (BLOCK_TAGS.has(line.slice(name, j).toLowerCase())) {
    const after = line[j];
    if (after === undefined || isSpace(after) || after === ">" || (after === "/" && line[j + 1] === ">")) return 6;
  }
  return !interrupts && wholeTag(line, at) ? 7 : 0;
}

/** Whether a line ends an HTML block of `kind` (kinds 6 and 7 end at a blank line instead). */
function htmlEnds(kind: number, line: string): boolean {
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

const PUNCTUATION = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

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
    if (s[k] === close) {
      let after = k + 1;
      while (isSpace(s[after])) after++;
      if (after >= s.length || s[after] === "\n") {
        return { end: Math.min(s.length, after + 1), label, definition: { destination, title: s.slice(t + 1, k) } };
      }
    }
  }
  return bareEnd < 0 ? null : { end: bareEnd, label, definition: { destination, title: null } };
}

/** The blocks of `markdown`. */
export function blocksOf(markdown: string): Blocks {
  const definitions = new Map<string, Definition>();
  let work = 0;
  const document = node("document", 0);
  const open: Open[] = [document];
  const lines = markdown.split(/\r\n|\r|\n/);
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();

  // Quotes and lists open now (NESTING bounds it).
  let containers = 0;
  const depth = () => containers;

  /** A paragraph's definitions taken; what's left is its text. */
  const paragraphText = (p: Open): string => {
    const s = p.lines.join("\n");
    let at = 0;
    for (;;) {
      const d = takeDefinition(s, at);
      if (!d) break;
      work += d.end - at;
      const key = labelKey(d.label);
      if (!definitions.has(key)) definitions.set(key, d.definition);
      at = d.end;
    }
    let end = s.length;
    while (end > at && isSpace(s[end - 1])) end--;
    return s.slice(at, end);
  };

  const finalize = (block: Open, line: number) => {
    const parent = open[open.length - 1];
    switch (block.kind) {
      case "paragraph": {
        block.text = paragraphText(block);
        if (block.text === "" && parent.children[parent.children.length - 1] === block) parent.children.pop();
        break;
      }
      case "indented":
        while (block.lines.length > 0 && blankFrom(block.lines[block.lines.length - 1], 0)) block.lines.pop();
        break;
      case "quote":
        block.end = Math.max(block.start, line);
        break;
      case "item":
      case "list":
        block.end = block.children.length > 0 ? block.children[block.children.length - 1].end : block.start;
        break;
      default:
        break;
    }
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
    open[open.length - 1].children.push(block);
    open.push(block);
    return block;
  };

  for (let n = 0; n < lines.length; n++) {
    let line = expandLead(lines[n].includes("\0") ? lines[n].replaceAll("\0", "�") : lines[n]);
    work += line.length + 1 + open.length;
    let i = 0;
    let handled = false;

    // 1. The open blocks this line continues.
    let matched = 0;
    for (let k = 1; k < open.length; k++) {
      const block = open[k];
      const indent = spaces(line, i);
      const blank = blankFrom(line, i);
      let next = -1;
      switch (block.kind) {
        case "quote":
          if (indent <= 3 && line[i + indent] === ">") {
            next = i + indent + 1;
            if (line[next] === " ") next++;
          }
          break;
        case "list":
          next = i;
          break;
        case "item":
          if (blank) next = block.children.length === 0 ? -1 : i + indent;
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
      const indent = spaces(line, i);
      const at = i + indent;
      const blank = at >= line.length;
      if (indent >= 4) {
        if (open[open.length - 1].kind !== "paragraph" && !blank) {
          closeUnmatched();
          i += 4;
          container = add("indented", n);
        }
        break;
      }
      const c = line[at];
      if (c === ">" && depth() < NESTING) {
        closeUnmatched();
        i = at + 1;
        if (line[i] === " ") i++;
        container = add("quote", n);
        matched = open.length - 1;
        continue;
      }
      if (c === "#") {
        const h = atx(line, at);
        if (h) {
          closeUnmatched();
          const heading = add("heading", n);
          heading.level = h.level;
          heading.text = h.text;
          close(n);
          done = true;
          break;
        }
      }
      if (c === "`" || c === "~") {
        const f = fenceOpen(line, at);
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
        const kind = htmlStart(line, at, container.kind === "paragraph");
        if (kind) {
          closeUnmatched();
          container = add("html", n);
          container.htmlKind = kind;
          break;
        }
      }
      if (container.kind === "paragraph" && closedUnmatched) {
        const level = underline(line, at);
        if (level) {
          const text = paragraphText(container);
          if (text !== "") {
            container.kind = "heading";
            container.level = level;
            container.text = text;
            container.end = n;
            open.pop();
            done = true;
            break;
          }
          // (Only definitions: they were taken; what's left is no paragraph.)
          container.lines = [];
        }
        const align = container.lines.length > 0 ? delimiterRow(line, at) : null;
        const head = align ? cells(container.lines[container.lines.length - 1]) : null;
        if (align && head && head.length === align.length && align.length <= COLUMNS) {
          work += line.length + container.lines[container.lines.length - 1].length;
          container.lines.pop();
          container.end = n - 2;
          close(n - 2);
          const table = add("table", n - 1);
          table.align = align;
          table.head = head;
          table.end = n;
          container = table;
          done = true;
          break;
        }
      }
      if (rule(line, at)) {
        closeUnmatched();
        add("rule", n);
        close(n);
        done = true;
        break;
      }
      if (depth() < NESTING) {
        const m = listMarker(line, at, container.kind === "paragraph");
        if (m) {
          closeUnmatched();
          line = m.line;
          if (!(container.kind === "list" && container.ordered === m.ordered && container.marker === m.marker)) {
            container = add("list", n);
            container.ordered = m.ordered;
            container.marker = m.marker;
            container.first = m.first;
          }
          container = add("item", n);
          container.offset = indent + m.width;
          i = m.content;
          matched = open.length - 1;
          continue;
        }
      }
      break;
    }
    if (done) continue;

    // 3. Its text: a lazy paragraph's, or the open block's, or a new paragraph's.
    const blank = blankFrom(line, i);
    const tip = open[open.length - 1];
    if (!closedUnmatched && !blank && tip.kind === "paragraph") {
      tip.lines.push(line.slice(i + spaces(line, i)));
      tip.end = n;
      continue;
    }
    closeUnmatched();
    switch (container.kind) {
      case "paragraph":
        container.lines.push(line.slice(i + spaces(line, i)));
        container.end = n;
        break;
      case "fence": {
        const strip = Math.min(spaces(line, i), container.fenceIndent);
        container.lines.push(line.slice(i + strip));
        container.end = n;
        break;
      }
      case "indented":
        container.lines.push(line.slice(i));
        if (!blank) container.end = n;
        break;
      case "html":
        work += line.length - i;
        container.lines.push(line.slice(i));
        container.end = n;
        if (htmlEnds(container.htmlKind, line.slice(i))) close(n);
        break;
      case "table": {
        const row = cells(line.slice(i)).slice(0, container.head.length);
        work += line.length - i;
        while (row.length < container.head.length) row.push("");
        container.rows.push(row);
        container.end = n;
        break;
      }
      default:
        if (!blank) {
          const p = add("paragraph", n);
          p.lines.push(line.slice(i + spaces(line, i)));
        }
        break;
    }
  }
  while (open.length > 1) close(lines.length - 1);

  return { blocks: document.children.map(toBlock).filter((b): b is Block => b !== null), definitions, work };
}

/** Whether a list is loose: a blank line between its items, or between two blocks of one item. */
function loose(list: Open): boolean {
  const items = list.children;
  for (let x = 0; x < items.length; x++) {
    const item = items[x];
    if (x + 1 < items.length && items[x + 1].start > item.end + 1) return true;
    const children = item.children;
    for (let y = 0; y + 1 < children.length; y++) if (children[y + 1].start > children[y].end + 1) return true;
  }
  return false;
}

function toBlock(block: Open): Block | null {
  switch (block.kind) {
    case "paragraph":
      return block.text === "" ? null : { kind: "paragraph", text: block.text };
    case "heading":
      return { kind: "heading", level: block.level as 1 | 2 | 3 | 4 | 5 | 6, text: block.text };
    case "fence":
      return { kind: "code", info: block.info, text: block.lines.length > 0 ? `${block.lines.join("\n")}\n` : "" };
    case "indented":
      return { kind: "code", info: "", text: `${block.lines.join("\n")}\n` };
    case "rule":
      return { kind: "rule" };
    case "html": {
      const lines = block.lines.slice();
      while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
      return { kind: "html", text: lines.join("\n") };
    }
    case "quote":
      return { kind: "quote", children: block.children.map(toBlock).filter((b): b is Block => b !== null) };
    case "table":
      return { kind: "table", align: block.align, head: block.head, rows: block.rows };
    case "list":
      return {
        kind: "list",
        ordered: block.ordered,
        start: block.first,
        tight: !loose(block),
        items: block.children.map((item) => {
          const children = item.children.map(toBlock).filter((b): b is Block => b !== null);
          return { task: taskOf(children), children };
        }),
      };
    default:
      return null;
  }
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
