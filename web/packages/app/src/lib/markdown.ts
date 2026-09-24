/**
 * The web client's markdown stack — a compact port of the desktop's
 * `crates/ui/src/markdown/` (pulldown-cmark block parse + the display-only
 * streaming mend) to dependency-free TypeScript. The block model mirrors the
 * desktop's `Block`/`BlockTree` shapes one for one so the transcript's row
 * model (`./transcript.ts`) splits streamed and settled text identically on
 * both surfaces.
 *
 * Scope (matching what harness output actually contains): paragraphs,
 * headings, fenced code, block quotes, lists (bullet/ordered, task markers,
 * nesting), GFM tables, thematic rules, and inline bold/italic/code/
 * strikethrough/links/images. Accepted gaps vs CommonMark: no setext
 * headings, no indented code blocks, no raw-HTML blocks (rendered literally,
 * like the desktop), no reference-style links.
 */

/** Sentinel destination for a link whose URL is still streaming (mend). */
export const PENDING_LINK_URL = "zeron:pending-link";

export interface InlineStyle {
  readonly bold?: boolean;
  readonly italic?: boolean;
  readonly code?: boolean;
  readonly strikethrough?: boolean;
  /** Destination URL when inside a link (or an image's source). */
  readonly link?: string | null;
  /** Alt text marks an image run; hosts decide whether to render media. */
  readonly image?: boolean;
}

/** One run of identically-styled inline text. */
export interface InlineRun {
  readonly text: string;
  readonly style: InlineStyle;
}

/** GFM column alignment. */
export type TableAlign = "left" | "center" | "right";

export interface ListItem {
  /** Task marker state; null for ordinary items. */
  readonly checked: boolean | null;
  readonly blocks: Block[];
}

/** A markdown block. Containers nest. */
export type Block =
  | { readonly kind: "paragraph"; readonly runs: readonly InlineRun[] }
  | { readonly kind: "heading"; readonly level: number; readonly runs: readonly InlineRun[] }
  | { readonly kind: "codeBlock"; readonly language: string | null; readonly code: string }
  | { readonly kind: "blockQuote"; readonly children: readonly Block[] }
  | { readonly kind: "list"; readonly orderedStart: number | null; readonly items: readonly ListItem[] }
  | {
      readonly kind: "table";
      readonly header: readonly (readonly InlineRun[])[];
      readonly align: readonly TableAlign[];
      readonly rows: readonly (readonly (readonly InlineRun[])[])[];
    }
  | { readonly kind: "rule" };

/** A top-level block plus its char range in the source. */
export interface TopBlock {
  readonly block: Block;
  readonly start: number;
  readonly end: number;
}

export interface BlockTree {
  readonly blocks: readonly TopBlock[];
}

export const EMPTY_TREE: BlockTree = { blocks: [] };

/** The block's visible text (what the streaming veil diffs against). */
export function blockFlatText(block: Block): string {
  switch (block.kind) {
    case "paragraph":
    case "heading":
      return block.runs.map((run) => run.text).join("");
    case "codeBlock":
      return block.code;
    case "blockQuote":
      return block.children.map(blockFlatText).join("\n");
    case "list":
      return block.items
        .map((item) => item.blocks.map(blockFlatText).join("\n"))
        .join("\n");
    case "table":
      return [block.header, ...block.rows]
        .map((row) => row.map((cell) => cell.map((run) => run.text).join("")).join(" · "))
        .join("\n");
    case "rule":
      return "———";
  }
}

// ---------------------------------------------------------------------------
// Block structure
// ---------------------------------------------------------------------------

interface Line {
  readonly start: number;
  readonly text: string;
}

function splitLines(source: string): Line[] {
  const lines: Line[] = [];
  let start = 0;
  for (let i = 0; i < source.length; i++) {
    if (source[i] === "\n") {
      lines.push({ start, text: source.slice(start, i) });
      start = i + 1;
    }
  }
  if (start < source.length) {
    lines.push({ start, text: source.slice(start) });
  }
  return lines;
}

const FENCE_RE = /^ {0,3}(`{3,}|~{3,})\s*([^`]*)$/;
const HEADING_RE = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/;
const RULE_RE = /^ {0,3}(?:([*]\s*){3,}|(-\s*){3,}|(_\s*){3,})$/;
const QUOTE_RE = /^ {0,3}>[ \t]?(.*)$/;
const BULLET_RE = /^( *)([-*+])[ \t]+(.*)$/;
const ORDERED_RE = /^( *)(\d{1,9})[.)][ \t]+(.*)$/;
const TASK_RE = /^\[([ xX])\][ \t]+(.*)$/;
const TABLE_DELIM_RE = /^\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

interface Fence {
  readonly marker: string;
  readonly length: number;
}

function fenceOf(text: string): (Fence & { info: string }) | null {
  const match = FENCE_RE.exec(text);
  if (match === null) {
    return null;
  }
  const marker = match[1]!;
  return { marker: marker[0]!, length: marker.length, info: (match[2] ?? "").trim() };
}

function isFenceClose(text: string, fence: Fence): boolean {
  const match = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(text);
  return match !== null && match[1]![0] === fence.marker && match[1]!.length >= fence.length;
}

/** One block-start test shared by the paragraph accumulator and the main loop. */
function startsBlock(lines: readonly Line[], ix: number): boolean {
  const text = lines[ix]!.text;
  return (
    fenceOf(text) !== null ||
    HEADING_RE.test(text) ||
    RULE_RE.test(text) ||
    QUOTE_RE.test(text) ||
    BULLET_RE.test(text) ||
    ORDERED_RE.test(text) ||
    startsTable(lines, ix)
  );
}

function startsTable(lines: readonly Line[], ix: number): boolean {
  const text = lines[ix]!.text;
  if (!text.includes("|") || ix + 1 >= lines.length) {
    return false;
  }
  const delim = lines[ix + 1]!.text;
  return delim.includes("-") && TABLE_DELIM_RE.test(delim);
}

function tableAligns(delim: string): TableAlign[] {
  return splitTableRow(delim).map((cell) => {
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    return left && right ? "center" : right ? "right" : "left";
  });
}

/** Split a table row on pipes, tolerating the optional edge pipes. */
function splitTableRow(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) {
    text = text.slice(1);
  }
  if (text.endsWith("|")) {
    text = text.slice(0, -1);
  }
  const cells: string[] = [];
  let current = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (c === "\\" && text[i + 1] === "|") {
      current += "|";
      i++;
    } else if (c === "|") {
      cells.push(current.trim());
      current = "";
    } else {
      current += c;
    }
  }
  cells.push(current.trim());
  return cells;
}

interface BlockDraft {
  readonly block: Block;
  /** Char span in the parsed source (accurate at top level). */
  readonly start: number;
  readonly end: number;
  /** Raw inline source for paragraph/heading blocks — mended lazily. */
  readonly inline?: string;
}

/**
 * Split the source into top-level blocks. Inline content stays raw here;
 * [`materialize`] parses it (mending the tail block for streaming).
 */
function draftBlocks(source: string): BlockDraft[] {
  const lines = splitLines(source);
  const drafts: BlockDraft[] = [];
  let ix = 0;

  while (ix < lines.length) {
    const line = lines[ix]!;
    if (line.text.trim().length === 0) {
      ix++;
      continue;
    }
    const start = line.start;

    const fence = fenceOf(line.text);
    if (fence !== null) {
      const codeLines: string[] = [];
      let end = line.start + line.text.length;
      ix++;
      // An unterminated fence (the streaming case) closes at end of input,
      // exactly like CommonMark — the desktop's mend relies on the same rule.
      while (ix < lines.length && !isFenceClose(lines[ix]!.text, fence)) {
        codeLines.push(lines[ix]!.text);
        end = lines[ix]!.start + lines[ix]!.text.length;
        ix++;
      }
      if (ix < lines.length) {
        end = lines[ix]!.start + lines[ix]!.text.length;
        ix++;
      }
      // The language label is the fence info's first token, VERBATIM — the
      // header renders the string as typed (render.rs:1952-1954, parser.rs
      // `info.split_whitespace().next()`); the highlighter resolves the
      // alias case-insensitively on its own.
      const language = fence.info.split(/\s+/)[0] ?? "";
      drafts.push({
        block: { kind: "codeBlock", language: language.length > 0 ? language : null, code: codeLines.join("\n") },
        start,
        end,
      });
      continue;
    }

    const heading = HEADING_RE.exec(line.text);
    if (heading !== null) {
      drafts.push({
        block: { kind: "heading", level: heading[1]!.length, runs: [] },
        inline: heading[2] ?? "",
        start,
        end: line.start + line.text.length,
      });
      ix++;
      continue;
    }

    if (RULE_RE.test(line.text)) {
      drafts.push({ block: { kind: "rule" }, start, end: line.start + line.text.length });
      ix++;
      continue;
    }

    if (QUOTE_RE.test(line.text)) {
      const quoted: string[] = [];
      let end = line.start + line.text.length;
      while (ix < lines.length) {
        const text = lines[ix]!.text;
        const quotedLine = QUOTE_RE.exec(text);
        if (quotedLine !== null) {
          quoted.push(quotedLine[1]!);
          end = lines[ix]!.start + text.length;
          ix++;
        } else if (text.trim().length === 0 && ix + 1 < lines.length && QUOTE_RE.test(lines[ix + 1]!.text)) {
          ix++;
        } else {
          break;
        }
      }
      drafts.push({
        block: { kind: "blockQuote", children: parseNested(quoted.join("\n")) },
        start,
        end,
      });
      continue;
    }

    if (startsTable(lines, ix)) {
      const header = splitTableRow(line.text);
      const align = tableAligns(lines[ix + 1]!.text);
      const rows: string[][] = [];
      let end = lines[ix + 1]!.start + lines[ix + 1]!.text.length;
      ix += 2;
      while (ix < lines.length && lines[ix]!.text.includes("|") && lines[ix]!.text.trim().length > 0) {
        rows.push(splitTableRow(lines[ix]!.text));
        end = lines[ix]!.start + lines[ix]!.text.length;
        ix++;
      }
      drafts.push({
        block: {
          kind: "table",
          header: header.map((cell) => autolinkRuns(parseInline(cell))),
          align,
          rows: rows.map((row) => row.map((cell) => autolinkRuns(parseInline(cell)))),
        },
        start,
        end,
      });
      continue;
    }

    const bullet = BULLET_RE.exec(line.text);
    const ordered = bullet === null ? ORDERED_RE.exec(line.text) : null;
    if (bullet !== null || ordered !== null) {
      const { block, next, end } = parseList(lines, ix);
      drafts.push({ block, start, end });
      ix = next;
      continue;
    }

    // Paragraph: accumulate until a blank line or another block start.
    const parts: string[] = [line.text];
    let end = line.start + line.text.length;
    ix++;
    while (ix < lines.length && lines[ix]!.text.trim().length > 0 && !startsBlock(lines, ix)) {
      parts.push(lines[ix]!.text);
      end = lines[ix]!.start + lines[ix]!.text.length;
      ix++;
    }
    const inline = parts.join("\n");
    drafts.push({ block: { kind: "paragraph", runs: [] }, inline, start, end });
  }

  return drafts;
}

/** Parse nested block content (quote interiors, list items). */
function parseNested(source: string): Block[] {
  return materialize(draftBlocks(source), false).map((draft) => draft.block);
}

function parseList(lines: readonly Line[], startIx: number): { block: Block; next: number; end: number } {
  const first = lines[startIx]!;
  const ordered = ORDERED_RE.exec(first.text);
  const firstMarker = ordered ?? BULLET_RE.exec(first.text);
  const baseIndent = firstMarker![1]!.length;
  const orderedStart = ordered !== null ? Number.parseInt(ordered[2]!, 10) : null;
  const items: ListItem[] = [];
  let ix = startIx;
  let end = first.start + first.text.length;

  while (ix < lines.length) {
    const line = lines[ix]!;
    const marker = ORDERED_RE.exec(line.text) ?? BULLET_RE.exec(line.text);
    const sameList =
      marker !== null &&
      marker[1]!.length === baseIndent &&
      (orderedStart === null) === (ORDERED_RE.exec(line.text) === null);
    if (!sameList) {
      break;
    }
    const contentIndent = baseIndent + marker[0]!.length - marker[3]!.length;
    const itemLines: string[] = [marker[3]!];
    end = line.start + line.text.length;
    ix++;
    // Continuation lines: blank lines stay out (a blank ends the list for
    // our purposes); anything indented past the marker belongs to the item.
    while (ix < lines.length) {
      const next = lines[ix]!;
      if (next.text.trim().length === 0) {
        break;
      }
      const indent = next.text.length - next.text.trimStart().length;
      const nested = ORDERED_RE.exec(next.text) ?? BULLET_RE.exec(next.text);
      if (nested !== null && nested[1]!.length <= baseIndent) {
        break;
      }
      if (indent < contentIndent && nested === null) {
        break;
      }
      itemLines.push(next.text.slice(Math.min(contentIndent, indent)));
      end = next.start + next.text.length;
      ix++;
    }
    const task = TASK_RE.exec(itemLines[0]!);
    const blocks = parseNested(task !== null ? [task[2]!, ...itemLines.slice(1)].join("\n") : itemLines.join("\n"));
    items.push({ checked: task === null ? null : task[1]!.toLowerCase() === "x", blocks });
  }

  return { block: { kind: "list", orderedStart, items }, next: ix, end };
}

// ---------------------------------------------------------------------------
// Inline parsing
// ---------------------------------------------------------------------------

const PLAIN: InlineStyle = {};

function pushRun(runs: InlineRun[], text: string, style: InlineStyle): void {
  if (text.length === 0) {
    return;
  }
  const last = runs[runs.length - 1];
  if (last !== undefined && styleEquals(last.style, style)) {
    runs[runs.length - 1] = { text: last.text + text, style: last.style };
    return;
  }
  runs.push({ text, style });
}

function styleEquals(a: InlineStyle, b: InlineStyle): boolean {
  return (
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.code === b.code &&
    a.strikethrough === b.strikethrough &&
    a.link === b.link &&
    a.image === b.image
  );
}

function isAlnum(c: string | undefined): boolean {
  return c !== undefined && /[\p{L}\p{N}]/u.test(c);
}

/** Find the index just past `marker`'s closing run, or -1 when unclosed. */
function findClosing(src: string, from: number, marker: string): number {
  let ix = from;
  while (ix < src.length) {
    const found = src.indexOf(marker, ix);
    if (found < 0) {
      return -1;
    }
    // Escaped markers are literal.
    if (found > 0 && src[found - 1] === "\\") {
      ix = found + marker.length;
      continue;
    }
    // Intraword `_` (and single `*`) never delimits (CommonMark).
    const before = found > 0 ? src[found - 1] : undefined;
    const after = src[found + marker.length];
    if ((marker === "_" || marker === "*") && isAlnum(before) && isAlnum(after)) {
      ix = found + marker.length;
      continue;
    }
    return found;
  }
  return -1;
}

/**
 * Parse inline markup into styled runs. Emphasis nesting is resolved by
 * simple recursion — chat markdown is shallow, and unclosed markers render
 * literally (the mend repairs them for streaming display).
 */
export function parseInline(src: string, style: InlineStyle = PLAIN): InlineRun[] {
  const runs: InlineRun[] = [];
  let ix = 0;
  let plain = "";

  const flush = (): void => {
    if (plain.length > 0) {
      pushRun(runs, plain, style);
      plain = "";
    }
  };

  while (ix < src.length) {
    const c = src[ix]!;

    if (c === "\\" && ix + 1 < src.length && /[^A-Za-z0-9\s]/.test(src[ix + 1]!)) {
      plain += src[ix + 1];
      ix += 2;
      continue;
    }

    if (c === "`") {
      let run = 1;
      while (src[ix + run] === "`") {
        run++;
      }
      const marker = "`".repeat(run);
      const close = findClosing(src, ix + run, marker);
      if (close < 0) {
        plain += marker;
        ix += run;
        continue;
      }
      flush();
      const code = src.slice(ix + run, close).replace(/\n/g, " ");
      pushRun(runs, code, { ...style, code: true });
      ix = close + run;
      continue;
    }

    if (c === "!" && src[ix + 1] === "[" && !style.code) {
      const parsed = parseLink(src, ix + 1);
      if (parsed !== null) {
        flush();
        pushRun(runs, parsed.text.length > 0 ? parsed.text : parsed.dest, {
          ...style,
          link: parsed.dest,
          image: true,
        });
        ix = parsed.end;
        continue;
      }
    }

    if (c === "[" && !style.code) {
      const parsed = parseLink(src, ix);
      if (parsed !== null) {
        flush();
        for (const run of parseInline(parsed.text, { ...style, link: parsed.dest })) {
          pushRun(runs, run.text, run.style);
        }
        ix = parsed.end;
        continue;
      }
    }

    if ((c === "*" || c === "_" || c === "~") && !style.code) {
      const isTilde = c === "~";
      const marker = isTilde ? "~~" : c + c;
      const single = isTilde ? null : c;
      const before = ix > 0 ? src[ix - 1] : undefined;
      // Try strong (double) first when a double marker is present.
      if (src.startsWith(marker, ix)) {
        const after = src[ix + 2];
        const intraword = (c === "_" || c === "*") && isAlnum(before) && isAlnum(after);
        const close = intraword ? -1 : findClosing(src, ix + 2, marker);
        if (close >= 0) {
          flush();
          const inner = src.slice(ix + 2, close);
          const next = isTilde ? { ...style, strikethrough: true } : { ...style, bold: true };
          for (const run of parseInline(inner, next)) {
            pushRun(runs, run.text, run.style);
          }
          ix = close + 2;
          continue;
        }
        // Strong marker didn't close — render the whole `**` literally so
        // the streaming mend can still close it on the tail block, and the
        // settled parse keeps it visible (matches CommonMark/GFM).
        plain += marker;
        ix += marker.length;
        continue;
      }
      if (single !== null && !src.startsWith(marker, ix + 1)) {
        // Single `*`/`_`: italic, never intraword.
        const after = src[ix + 1];
        const intraword = isAlnum(before) && isAlnum(after);
        const close = intraword ? -1 : findClosing(src, ix + 1, single);
        if (close >= 0) {
          flush();
          const inner = src.slice(ix + 1, close);
          for (const run of parseInline(inner, { ...style, italic: true })) {
            pushRun(runs, run.text, run.style);
          }
          ix = close + 1;
          continue;
        }
      }
      plain += c;
      ix++;
      continue;
    }

    plain += c;
    ix++;
  }
  flush();
  return runs;
}

/** Parse `[text](dest)` or the `[` of one starting at `open`. */
function parseLink(src: string, open: number): { text: string; dest: string; end: number } | null {
  let depth = 0;
  let close = -1;
  for (let i = open; i < src.length; i++) {
    const c = src[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (c === "[") {
      depth++;
    } else if (c === "]") {
      depth--;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }
  if (close < 0 || src[close + 1] !== "(") {
    return null;
  }
  let parens = 0;
  for (let i = close + 2; i < src.length; i++) {
    const c = src[i];
    if (c === "(") {
      parens++;
    } else if (c === ")") {
      if (parens === 0) {
        const raw = src.slice(close + 2, i).trim();
        // An optional "title" rides after whitespace; the transcript never
        // shows it, so only the destination survives.
        const dest = raw.split(/\s+/)[0] ?? "";
        return { text: src.slice(open + 1, close), dest, end: i + 1 };
      }
      parens--;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Bare-URL autolinking (port of parser.rs::autolink_runs, :489-576)
// ---------------------------------------------------------------------------

/**
 * Promote bare `http(s)://` URLs in plain runs into link runs — GFM's
 * autolink extension. Runs already inside a link or code span pass through
 * untouched; idempotent, so re-applying on merged output is harmless.
 */
export function autolinkRuns(runs: readonly InlineRun[]): InlineRun[] {
  const out: InlineRun[] = [];
  for (const run of runs) {
    if ((run.style.link !== null && run.style.link !== undefined) || run.style.code) {
      out.push(run);
    } else {
      pushTextAutolinked(out, run.text, run.style);
    }
  }
  return out;
}

function pushTextAutolinked(runs: InlineRun[], text: string, style: InlineStyle): void {
  let rest = text;
  for (;;) {
    const at = findUrlStart(rest);
    if (at === null) {
      break;
    }
    const schemeLen = rest.startsWith("https://", at) ? "https://".length : "http://".length;
    const from = rest.slice(at);
    const len = bareUrlLen(from);
    if (len <= schemeLen) {
      // A scheme with nothing after it stays text (don't re-find it).
      pushRun(runs, rest.slice(0, at + schemeLen), style);
      rest = from.slice(schemeLen);
      continue;
    }
    pushRun(runs, rest.slice(0, at), style);
    const url = from.slice(0, len);
    pushRun(runs, url, { ...style, link: url });
    rest = from.slice(len);
  }
  pushRun(runs, rest, style);
}

/** The full code point ending at UTF-16 index `at - 1`, for boundary tests. */
function codePointBefore(text: string, at: number): string | undefined {
  if (at <= 0) {
    return undefined;
  }
  const prev = text.charCodeAt(at - 1);
  if (prev >= 0xd800 && prev <= 0xdbff) {
    return text.slice(at - 2, at);
  }
  return text[at - 1]!;
}

/**
 * First viable `http(s)://` occurrence: not glued to a preceding alphanumeric
 * (`foohttps://x` stays text, per GFM's boundary rule).
 */
export function findUrlStart(text: string): number | null {
  let from = 0;
  for (;;) {
    const at = text.indexOf("http", from);
    if (at < 0) {
      return null;
    }
    const after = text.slice(at);
    const isScheme = after.startsWith("http://") || after.startsWith("https://");
    const before = codePointBefore(text, at);
    const boundary = before === undefined || !isAlnum(before);
    if (isScheme && boundary) {
      return at;
    }
    from = at + "http".length;
  }
}

/**
 * Length of the bare URL at the start of `text`: run to whitespace (or a
 * delimiter that never appears in pasted URLs), then trim the trailing
 * punctuation GFM excludes — a closing paren only stays when an opener
 * inside the URL balances it.
 */
export function bareUrlLen(text: string): number {
  let end = text.length;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (/\s/.test(c) || c === "<" || c === ">" || c === '"' || c === "'" || c === "`") {
      end = i;
      break;
    }
  }
  let urlEnd = end;
  for (;;) {
    if (urlEnd === 0) {
      break;
    }
    const code = text.codePointAt(urlEnd - 1)!;
    const size = code > 0xffff ? 2 : 1;
    const ch = String.fromCodePoint(code);
    let trim = false;
    if (".,;:!?*_~".includes(ch)) {
      trim = true;
    } else if (ch === ")") {
      const url = text.slice(0, urlEnd);
      const opens = countOccurrences(url, "(");
      const closes = countOccurrences(url, ")");
      trim = opens < closes;
    }
    if (!trim) {
      break;
    }
    urlEnd -= size;
  }
  return urlEnd;
}

function countOccurrences(text: string, needle: string): number {
  let count = 0;
  let at = text.indexOf(needle);
  while (at >= 0) {
    count++;
    at = text.indexOf(needle, at + needle.length);
  }
  return count;
}

// ---------------------------------------------------------------------------
// Table column geometry (port of render.rs::table_columns, :727-742)
// ---------------------------------------------------------------------------

/** Uniform cell padding (render.rs:50, `TABLE_CELL_PADDING`). */
export const TABLE_CELL_PADDING = 12;
/** Floor for a column's max-content share (`TABLE_MIN_COLUMN_CONTENT`). */
export const TABLE_MIN_COLUMN_CONTENT = 48;
/** Minimum rendered column width in px, padding included. */
export const TABLE_MIN_COLUMN_WIDTH = 96;

export interface TableColumns {
  /** Content-proportional natural widths, padding included. */
  readonly naturals: readonly number[];
  /** The width below which a column stops shrinking. */
  readonly minimums: readonly number[];
  /** Sum of minimums — the width the table needs before it scrolls. */
  readonly minTableWidth: number;
}

/**
 * Resolve column geometry from measured per-column max-content widths
 * (content only — padding is added here, as the source adds
 * `2 * TABLE_CELL_PADDING`).
 */
export function tableColumns(contentWidths: readonly number[]): TableColumns {
  const naturals = contentWidths.map((w) => Math.max(w, TABLE_MIN_COLUMN_CONTENT) + 2 * TABLE_CELL_PADDING);
  const minimums = naturals.map((n) => Math.min(n, TABLE_MIN_COLUMN_WIDTH));
  return { naturals, minimums, minTableWidth: minimums.reduce((a, b) => a + b, 0) };
}

// ---------------------------------------------------------------------------
// Streaming mend (port of crates/ui/src/markdown/mend.rs)
// ---------------------------------------------------------------------------

interface OpenDelim {
  ch: string;
  len: number;
  /** Char index just past the run: nesting order + content-must-follow. */
  pos: number;
}

/**
 * Repair hanging inline markers in a streaming block's source, returning
 * `null` when nothing hangs. Display-only: the settled text is parsed as-is.
 */
export function closeHanging(text: string): string | null {
  const cs = [...text];
  const n = cs.length;
  const at = (i: number): string | undefined => cs[i];

  const delims: OpenDelim[] = [];
  const brackets: number[] = [];
  let code: { open: number; cpos: number } | null = null;
  let lastContent: number | null = null;
  let pendingUrl: number | null = null;

  const runLen = (i: number): number => {
    let len = 0;
    while (i + len < n && cs[i + len] === cs[i]) {
      len++;
    }
    return len;
  };

  const delim = (c: string, run: number, i: number): void => {
    const end = i + run;
    if (c === "~" && run > 2) {
      lastContent = end - 1;
      return;
    }
    const prev = i > 0 ? cs[i - 1] : undefined;
    const next = cs[end];
    // Intraword `_` never delimits; intraword single `*` treated the same.
    if (isAlnum(prev) && isAlnum(next) && (c === "_" || (c === "*" && run === 1))) {
      lastContent = end - 1;
      return;
    }
    const canClose = prev !== undefined && !/\s/.test(prev);
    const canOpen = next !== undefined && !/\s/.test(next);
    let rest = run;
    if (canClose) {
      for (let k = delims.length - 1; k >= 0; k--) {
        if (delims[k]!.ch === c) {
          const take = Math.min(rest, delims[k]!.len);
          delims[k]!.len -= take;
          rest -= take;
          delims.length = delims[k]!.len === 0 ? k : k + 1;
          break;
        }
      }
    }
    if (rest > 0) {
      if (canOpen && (c !== "~" || rest === 2)) {
        delims.push({ ch: c, len: rest, pos: end });
      } else {
        lastContent = end - 1;
      }
    }
  };

  let i = 0;
  scan: while (i < n) {
    const c = cs[i]!;
    if (code === null && c === "\\") {
      if (i + 1 < n) {
        lastContent = i + 1;
      }
      i += 2;
      continue;
    }
    if (c === "`") {
      const run = runLen(i);
      if (code !== null && run === code.open) {
        code = null;
      } else if (code !== null) {
        lastContent = i + run - 1;
      } else {
        code = { open: run, cpos: i + run };
      }
      i += run;
      continue;
    }
    if (code !== null) {
      lastContent = i;
      i++;
      continue;
    }
    switch (c) {
      case "*":
      case "_":
      case "~": {
        const run = runLen(i);
        delim(c, run, i);
        i += run;
        break;
      }
      case "[":
        brackets.push(i);
        i++;
        break;
      case "]": {
        const open = brackets.pop();
        if (open !== undefined) {
          // Emphasis opened inside a completed `[…]` stays literal.
          for (let k = delims.length - 1; k >= 0; k--) {
            if (delims[k]!.pos >= open) {
              delims.splice(k, 1);
            }
          }
          if (at(i + 1) === "(") {
            let j = i + 2;
            let depth = 0;
            for (;;) {
              const cj = at(j);
              if (cj === "(") {
                depth++;
              } else if (cj === ")" && depth === 0) {
                break;
              } else if (cj === ")") {
                depth--;
              } else if (cj === undefined) {
                pendingUrl = i;
                break scan;
              }
              j++;
            }
            lastContent = j;
            i = j + 1;
            break;
          }
        }
        lastContent = i;
        i++;
        break;
      }
      default:
        if (/\s/.test(c)) {
          i++;
        } else {
          lastContent = i;
          i++;
        }
    }
  }

  // Text ends inside a link/image URL: drop the partial URL, keep the text.
  if (pendingUrl !== null) {
    return `${cs.slice(0, pendingUrl).join("")}](${PENDING_LINK_URL})`;
  }

  // Collect closers innermost-first (descending open position).
  const pending: Array<{ pos: number; closer: string }> = [];
  if (code !== null && lastContent !== null && lastContent >= code.cpos) {
    pending.push({ pos: code.cpos, closer: "`".repeat(code.open) });
  }
  for (const d of delims) {
    if (lastContent !== null && lastContent >= d.pos) {
      pending.push({ pos: d.pos, closer: d.ch.repeat(d.len) });
    }
  }
  const openBracket = brackets[brackets.length - 1];
  if (openBracket !== undefined && lastContent !== null && lastContent > openBracket) {
    pending.push({ pos: openBracket, closer: `](${PENDING_LINK_URL})` });
  }
  pending.sort((a, b) => b.pos - a.pos);
  const closers = pending.map((p) => p.closer).join("");

  // A trailing line of only `-`/`--`/`=`/`==` under text is a setext
  // underline to a CommonMark parser but almost always a streaming list
  // item; a zero-width space breaks the reading invisibly.
  const setext = setextPartial(text);

  if (closers.length === 0 && !setext) {
    return null;
  }
  if (setext) {
    const nl = text.lastIndexOf("\n");
    if (nl >= 0 && closers.length > 0) {
      return `${text.slice(0, nl)}${closers}${text.slice(nl)}​`;
    }
    return `${text}​`;
  }
  // Insert before trailing whitespace: a closer after a trailing space is
  // not right-flanking and would not close.
  const end = text.trimEnd().length;
  return `${text.slice(0, end)}${closers}${text.slice(end)}`;
}

/** Last line is only 1–2 `-` or `=` under a non-empty line. */
function setextPartial(text: string): boolean {
  const nl = text.lastIndexOf("\n");
  if (nl < 0) {
    return false;
  }
  const trimmed = text.slice(nl + 1).trimStart();
  const underline = (c: string): boolean =>
    trimmed.length > 0 && trimmed.length <= 2 && [...trimmed].every((x) => x === c);
  if (!underline("-") && !underline("=")) {
    return false;
  }
  const above = text.slice(0, nl).split("\n");
  const last = above[above.length - 1]!;
  return last.trim().length > 0;
}

// ---------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------

function materialize(drafts: readonly BlockDraft[], mendTail: boolean): TopBlock[] {
  return drafts.map((draft, ix) => {
    if (draft.inline === undefined) {
      return { block: draft.block, start: draft.start, end: draft.end };
    }
    const tail = mendTail && ix === drafts.length - 1;
    const source = tail ? (closeHanging(draft.inline) ?? draft.inline) : draft.inline;
    // Autolink AFTER the mend (parser.rs:407-410): a half-streamed bare URL
    // becomes clickable mid-stream, and mended pending links (link set) are
    // skipped by the boundary scan.
    const block: Block =
      draft.block.kind === "heading"
        ? { kind: "heading", level: draft.block.level, runs: autolinkRuns(parseInline(source)) }
        : { kind: "paragraph", runs: autolinkRuns(parseInline(source)) };
    return { block, start: draft.start, end: draft.end };
  });
}

/**
 * Parse a whole source into a [`BlockTree`]. `mendTail` mends hanging inline
 * markers of the last block for display while a message streams.
 */
export function parseMarkdown(source: string, mendTail = false): BlockTree {
  if (source.trim().length === 0) {
    return EMPTY_TREE;
  }
  return { blocks: materialize(draftBlocks(source), mendTail) };
}

/**
 * Parse cache keyed by part key, mirroring the desktop's settled-tree cache:
 * a re-delivered identical text returns the same tree identity, so settled
 * rows never re-render during streaming.
 */
export class MarkdownCache {
  readonly #cache = new Map<string, { text: string; live: boolean; tree: BlockTree }>();
  readonly #cap: number;

  constructor(cap = 512) {
    this.#cap = cap;
  }

  parse(key: string, text: string, live: boolean): BlockTree {
    const cached = this.#cache.get(key);
    if (cached !== undefined && cached.text === text && cached.live === live) {
      return cached.tree;
    }
    const tree = parseMarkdown(text, live);
    if (this.#cache.size >= this.#cap) {
      const oldest = this.#cache.keys().next();
      if (!oldest.done) {
        this.#cache.delete(oldest.value);
      }
    }
    this.#cache.set(key, { text, live, tree });
    return tree;
  }

  clear(): void {
    this.#cache.clear();
  }
}
