// A plugin's README (and a version's notes) as HTML, on the server. What is
// shown must be what was reviewed, and nothing in it may run:
//
//   markdown ─ remark-parse + remark-gfm (CommonMark + GFM)
//            ─ remark-rehype (raw HTML is dropped, never parsed)
//            ─ point: pictures in the package go through the store's file
//              route; any other picture becomes a link to it; links keep
//              only http(s) and mailto; in-page anchors are made to match
//            ─ rehype-sanitize, with a schema that admits only what the
//              steps above make
//            ─ finish: heading ids and levels, rel on links, tables in a
//              scroller, code highlighted (classes only, no inline style)
//            ─ HTML
//
// Every step after the sanitizer only adds what it builds itself.

import type { Element, ElementContent, Root, RootContent } from "hast";
import { toText } from "hast-util-to-text";
import GithubSlugger from "github-slugger";
import { common, createLowlight } from "lowlight";
import rehypeSanitize, { type Options as SanitizeSchema } from "rehype-sanitize";
import rehypeStringify from "rehype-stringify";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { SKIP, visit } from "unist-util-visit";
import { highlightable, nestsTooDeep } from "./highlight.ts";
import { resolvePackagePath } from "./names.ts";

export interface RenderOptions {
  /**
   * The address a file of the package is served at, given its normalized
   * path (names.ts); every picture shown comes from here.
   */
  file: (path: string) => string;
  /** The page's own heading is an h1: the document's start a level under it. */
  demote?: boolean;
  /**
   * The plugin's name, said by the page already: a leading top-level
   * heading that only says it again is not drawn (the README never
   * outweighs the page; the app's plugin page does the same).
   */
  title?: string;
}

/** A heading's words, for comparing with a name: spaces made one, case set aside. */
const headingWords = (text: string) => text.replace(/\s+/g, " ").trim().toLocaleLowerCase("en");

/** Pictures a README may show from its package, by extension (what the store serves inline). */
const PICTURES = /\.(png|jpe?g|svg)$/i;

/** The prefix every id the document makes carries, so none can clobber the page's. */
const PREFIX = "user-content-";

/** What one document's code blocks may highlight, together, in bytes (highlight.ts bounds each). */
const HIGHLIGHT_BUDGET = 16 * 1024;

/** Links a person may follow out of a README: https, and mail. */
const LINK_PROTOCOLS = new Set(["https:", "mailto:"]);

const REL = ["nofollow", "noopener", "ugc"];

const lowlight = createLowlight(common);
// The names people write after ``` that common's grammars know by another.
lowlight.registerAlias({ javascript: ["mjs", "cjs"], lua: ["luau"], bash: ["zsh"], json: ["jsonc", "json5"] });

/** What the sanitizer admits: exactly what remark-rehype and `point` make. */
const SCHEMA: SanitizeSchema = {
  strip: ["script", "style"],
  clobber: [],
  clobberPrefix: "",
  allowComments: false,
  allowDoctypes: false,
  tagNames: [
    "p", "br", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "ul", "ol", "li", "pre", "code",
    "em", "strong", "del", "a", "img", "table", "thead", "tbody", "tr", "th", "td", "input", "section", "sup",
  ],
  ancestors: {
    tbody: ["table"],
    thead: ["table"],
    tr: ["table"],
    th: ["table"],
    td: ["table"],
    input: ["li"],
  },
  attributes: {
    a: [
      "href",
      "dataFootnoteRef",
      "dataFootnoteBackref",
      ["className", "data-footnote-backref"],
      ["id", /^user-content-fnref-[\w-]+$/],
      ["ariaDescribedBy", "footnote-label"],
      ["ariaLabel", /^Back to reference [\w-]+$/],
    ],
    img: ["src", "alt", "title"],
    code: [["className", /^language-[\w+#.-]{1,32}$/]],
    input: [["type", "checkbox"], ["disabled", true], "checked"],
    li: [["className", "task-list-item"], ["id", /^user-content-fn-[\w-]+$/]],
    ul: [["className", "contains-task-list"]],
    ol: ["start", ["className", "contains-task-list"]],
    section: ["dataFootnotes", ["className", "footnotes"]],
    h2: [["className", "sr-only"], ["id", "footnote-label"]],
    th: [["align", "left", "center", "right"]],
    td: [["align", "left", "center", "right"]],
  },
  required: { input: { type: "checkbox", disabled: true } },
  protocols: {
    href: ["http", "https", "mailto"],
    // Only the store's own file route, which `point` makes relative.
    src: [],
  },
};

const processor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  // Footnotes' ids are made by remark-rehype with PREFIX; nothing else in
  // the tree has an id until `finish` gives headings theirs.
  .use(remarkRehype, { allowDangerousHtml: false, clobberPrefix: PREFIX, footnoteLabelTagName: "h2" })
  .freeze();

const sanitize = unified().use(rehypeSanitize, SCHEMA).freeze();
const stringify = unified().use(rehypeStringify, { allowDangerousHtml: false }).freeze();

/** Bump when the HTML this makes changes, so kept renderings are made again. */
export const RENDERER = 6;

/**
 * What one rendering may cost. The parser's work grows faster than its
 * input for some shapes (setext headings, list items, nested brackets or
 * quotes, emphasis delimiters: a 64 KB README could take tens of seconds),
 * and a Worker's clock doesn't move while code runs, so the budget is
 * counted in work, not time:
 *
 * - each block (lines between blank lines, outside fenced code) is bounded
 *   in lines, bytes, nesting and emphasis delimiters, the shapes whose cost
 *   is superlinear: one past a bound and the document is its text;
 * - blocks are parsed in pieces of a bounded size, each on its own (the
 *   link and footnote definitions it refers to carried into it), then joined into one
 *   document, so no single parse can run long;
 * - what a rendering will cost is reckoned before it parses (COST: each
 *   byte, line, emphasis delimiter and bracket at what it was measured to
 *   take, code's bytes at what highlighting them takes, and the definitions
 *   carried into pieces, in milliseconds), and a rendering may cost at most `work`; a page
 *   shares one such budget among everything it renders (`Budget`). Past
 *   either, the document is shown as its text.
 *
 * Measured (markdown.test.ts holds it): every hostile shape at the store's
 * caps (a 512 KB README, 64 KB notes) is drawn or shown as text within a
 * second; an ordinary README of about 64 KB is drawn.
 */
export const BOUNDS = {
  blockLines: 400,
  blockBytes: 8 * 1024,
  pieceLines: 400,
  pieceBytes: 8 * 1024,
  /** Brackets open at once in a block (an image in a link is two). */
  nesting: 32,
  /** Quote markers at a line's start. */
  quotes: 16,
  /** Emphasis delimiters (`*`, `_`, `~`) in a block, outside code spans. */
  delimiters: 1500,
  /** Brackets, `[` and `]` together, in a block (a run of either costs the square of its length). */
  brackets: 2000,
  /** A table's columns (each row is padded to the header's width: a wide header makes every row wide). */
  columns: 64,
  /** The HTML a rendering may come to, in characters: past it, the text. */
  html: 2 * 1024 * 1024,
  /** What one rendering may cost, reckoned by COST (about milliseconds). */
  work: 600,
} as const;

/**
 * What each part of a document was measured to cost to draw, in
 * milliseconds: a byte outside fenced code, a line, an emphasis delimiter,
 * a bracket (`[` or `]`), a byte and a line of fenced code, a table cell
 * (each row is as wide as its header) and a cell filled (each `|`). Each
 * from a shape that isolates it, divided by its count.
 *
 * Measured on AMD Ryzen 9 5900X (24 threads, shared, loaded), Node 22.23,
 * the shapes in markdown.shapes.ts, each drawn in a fresh process (cold,
 * as a first rendering in a new isolate is) and as the best of three (warm):
 *
 *   shape                                            estimate  cold ms  ratio  warm ms  ratio
 *   an ordinary README (53 KB)                          248       367   1.48     237   0.96
 *   prose and tables (77 KB, the reference)             327       204   0.62     133   0.41
 *   setext headings, a block each (24 KB)               540       415   0.77     246   0.46
 *   list items, a block each (24 KB)                    290       411   1.42     272   0.94
 *   nested pictures, many blocks (58 KB)                455       373   0.82     281   0.62
 *   nested links, many blocks (60 KB)                   526       133   0.25      82   0.16
 *   autolinks (264 KB)                                  545       490   0.90     379   0.69
 *   raw HTML, dropped (79 KB)                           306        73   0.24      26   0.09
 *   character references (234 KB)                       483       182   0.38     114   0.24
 *   long lines (234 KB)                                 482       147   0.31     100   0.21
 *   fenced code, 465 KB (highlighted within bounds)     522       132   0.25      86   0.16
 *   fenced code, huge blocks (309 KB)                   346        69   0.20      28   0.08
 *   footnotes, 1,000 (33 KB)                            367       358   0.97     261   0.71
 *   every footnote referenced from every piece          446       459   1.03     369   0.83
 *   a fence closed with an info string (16 KB)           66        77   1.18      38   0.58
 *   tables, sparse, 64 columns                          244       256   1.05     180   0.74
 *   tables, full, 32 columns (39 KB)                    507       549   1.08     387   0.76
 *   closing brackets, under the bound (22 KB)           411       217   0.53     195   0.47
 *   code of short lines (98 KB)                         400       256   0.64     181   0.45
 *   every link definition referenced from every piece   378       275   0.73     160   0.42
 *
 * The worst real ÷ estimated seen: 1.48 cold here (an ordinary README), 1.83
 * for it in an earlier, more loaded run; 0.96 warm. markdown.test.ts fails
 * if any shape's (warm, judged against the reference's) passes
 * RATIO_CEILING. What beat the estimate before (BEATERS, in the shapes,
 * each held to its outcome): every definition carried into every piece
 * (5.47×), code uncounted (465 KB at 0), and the review's second attack:
 * fences CommonMark doesn't open (346× to 700×), a table 1,300 columns
 * wide (57×, 83 MB of HTML), runs of `]` (35×), lines ended by CR (28×),
 * highlighting grammars past their bounds (to 1,150×; highlight.ts).
 */
export const COST = { byte: 0.002, line: 0.05, delimiter: 0.03, bracket: 0.016, code: 0.001, codeLine: 0.006, cell: 0.004, pipe: 0.016 } as const;

/** No measured shape may cost more than this many times its estimate (markdown.test.ts). */
export const RATIO_CEILING = 2.5;

/**
 * What a page may spend on all it renders together, reckoned as BOUNDS.work
 * is. At the worst ratio seen (1.83), 900 is about 1.6 s of real work on a
 * first visit. Each rendering is kept by its text's hash, so that is spent
 * once per version a data center draws, never per visit: the page's budget
 * bounds that one first visit, and a document left undrawn by it (never
 * kept) is drawn by a later visit, with what was kept costing nothing.
 */
export const PAGE_WORK = 900;

/** What a page may spend rendering, shared by every document on it: work left (COST). */
export class Budget {
  work: number;
  constructor(work = PAGE_WORK) {
    this.work = work;
  }
}

/** Why a document was shown as its text: a shape past a bound, its own budget, or the page's. */
export type Cut = "shape" | "budget" | "page";

export interface Rendered {
  html: string;
  /** Null: drawn. Otherwise why it is its text. */
  cut: Cut | null;
}

/**
 * What a kept rendering was made by: the pipeline's version (RENDERER,
 * bumped by hand when a step changes), a fingerprint of the sanitizer's
 * schema and of the bounds (each changes by itself when they do).
 * rendered.ts keys every kept rendering by it, so a change makes them all again.
 */
export const PIPELINE = `${RENDERER}.${fingerprint(JSON.stringify(SCHEMA, (_, v) => (v instanceof RegExp ? `/${v.source}/${v.flags}` : v)))}.${fingerprint(JSON.stringify(BOUNDS))}`;

function fingerprint(text: string): string {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619) >>> 0;
  return h.toString(36);
}

/** How many emphasis delimiters a document has outside fenced code and code spans. */
export function delimiters(markdown: string): number {
  let count = 0;
  let fence: string | null = null;
  for (const line of markdown.split(/\r\n|\r|\n/)) {
    const opens = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (fence) {
      if (opens && opens[0] === fence[0] && opens.length >= fence.length) fence = null;
      continue;
    }
    if (opens) {
      fence = opens;
      continue;
    }
    count += lineDelimiters(line);
  }
  return count;
}

const lineDelimiters = (line: string) => {
  let count = 0;
  for (const ch of line.replace(/(`+)[^`]*?\1/g, "")) if (ch === "*" || ch === "_" || ch === "~") count++;
  return count;
};

/** A block: its lines, and how many of them (and bytes) are outside fenced code. */
interface Block {
  lines: string[];
  textLines: number;
  textBytes: number;
}

/**
 * The document as blocks (split at blank lines outside fenced code), with
 * its definitions (links' and footnotes'), or null when a block is past a
 * bound. A fence opens at any indent (one inside a list item too), so a
 * blank line inside code never splits it.
 */
function blocksOf(markdown: string): { blocks: Block[]; definitions: Map<string, string>; work: number } | null {
  const blocks: Block[] = [];
  const definitions = new Map<string, string>();
  let work = 0;
  let current: Block = { lines: [], textLines: 0, textBytes: 0 };
  /** The open fence (CommonMark's rules), and the open table's width, in the current block. */
  let fence: { char: string; length: number } | null = null;
  let columns = 0;
  let depth = 0;
  let marks = 0;
  let brackets = 0;
  const close = () => {
    if (current.lines.length) blocks.push(current);
    current = { lines: [], textLines: 0, textBytes: 0 };
    columns = 0;
    depth = 0;
    marks = 0;
    brackets = 0;
  };
  // (A line ends at \n, \r\n or \r, as CommonMark has it.)
  for (const line of markdown.split(/\r\n|\r|\n/)) {
    if (fence) {
      current.lines.push(line);
      // (Code isn't parsed as markdown, but may be highlighted: COST.code a byte, codeLine a line.)
      work += COST.code * (line.length + 1) + COST.codeLine;
      const closer = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line)?.[1];
      if (closer && closer[0] === fence.char && closer.length >= fence.length) fence = null;
      continue;
    }
    // A fence opens as CommonMark opens one: indented three spaces at most,
    // never by a tab; a backtick fence's info string has no backtick in it.
    const opener = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (opener && !(opener[1]![0] === "`" && opener[2]!.includes("`"))) {
      fence = { char: opener[1]![0]!, length: opener[1]!.length };
      current.lines.push(line);
      work += COST.code * (line.length + 1) + COST.codeLine;
      continue;
    }
    if (line.trim() === "") {
      close();
      continue;
    }
    current.lines.push(line);
    current.textLines++;
    current.textBytes += line.length + 1;
    // (A link's definition; a footnote's, which runs on, is taken whole below.)
    const defined = /^ {0,3}\[(?!\^)([^\]\n]+)\]:\s*\S/.exec(line);
    if (defined && !definitions.has(label(defined[1]!))) definitions.set(label(defined[1]!), line);
    // A table's delimiter row, under its header: from here every row is as wide as it.
    if (!columns && current.textLines > 1 && line.includes("|") && /^ {0,3}\|?(\s*:?-+:?\s*\|)*\s*:?-+:?\s*\|?\s*$/.test(line)) {
      columns = line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").length;
      if (columns > BOUNDS.columns) return null;
      work += 2 * COST.cell * columns;
    } else if (columns) work += COST.cell * columns + COST.pipe * (line.split("|").length - 1);
    // Quote markers at the line's start, brackets open at once and in all, delimiters.
    if ((/^[\s>]*/.exec(line)?.[0].match(/>/g)?.length ?? 0) > BOUNDS.quotes) return null;
    const bare = line.replace(/\\[[\]]/g, "").replace(/(`+)[^`]*?\1/g, "");
    let lineBrackets = 0;
    for (const ch of bare) {
      if (ch === "[") {
        depth++;
        lineBrackets++;
      } else if (ch === "]") {
        depth = Math.max(0, depth - 1);
        lineBrackets++;
      }
      if (depth > BOUNDS.nesting) return null;
    }
    brackets += lineBrackets;
    const delimited = lineDelimiters(line);
    marks += delimited;
    work += COST.byte * (line.length + 1) + COST.line + COST.delimiter * delimited + COST.bracket * lineBrackets;
    if (current.textLines > BOUNDS.blockLines || current.textBytes > BOUNDS.blockBytes || marks > BOUNDS.delimiters || brackets > BOUNDS.brackets) return null;
  }
  close();
  // A footnote's definition is a block starting `[^label]:` (its lines with it).
  for (const block of blocks) {
    const footnote = /^ {0,3}\[(\^[^\]\n]+)\]:/.exec(block.lines[0]!);
    if (footnote && !definitions.has(label(footnote[1]!))) definitions.set(label(footnote[1]!), block.lines.join("\n"));
  }
  return { blocks, definitions, work };
}

/** A label as references match it: case and runs of space set aside (`^name` for a footnote's). */
const label = (text: string) => text.trim().replace(/\s+/g, " ").toLowerCase();

/**
 * The definitions a piece refers to (each `[…]` in it that names one), to
 * be carried into it, and what they add to the work: only what each piece
 * uses, so carrying them never costs the square of their number.
 */
function carriedInto(piece: string, definitions: Map<string, string>): { text: string; work: number } {
  if (!definitions.size) return { text: "", work: 0 };
  const used = new Set<string>();
  for (const match of piece.matchAll(/\[([^\]\n]{1,999})\]/g)) {
    const key = label(match[1]!);
    if (definitions.has(key)) used.add(key);
  }
  const texts = [...used].map((key) => definitions.get(key)!);
  const text = texts.length ? `\n\n${texts.join("\n\n")}` : "";
  return { text, work: COST.byte * text.length + COST.line * (text.split("\n").length - 1) };
}

/**
 * Blocks packed into pieces of at most BOUNDS.pieceLines and pieceBytes,
 * never breaking before an indented block (it continues the one before).
 */
function piecesOf(blocks: Block[]): string[] {
  const pieces: string[] = [];
  let lines: string[] = [];
  let count = 0;
  let bytes = 0;
  for (const block of blocks) {
    const continues = /^\s/.test(block.lines[0]!);
    if (lines.length && !continues && (count + block.textLines > BOUNDS.pieceLines || bytes + block.textBytes > BOUNDS.pieceBytes)) {
      pieces.push(lines.join("\n"));
      lines = [];
      count = 0;
      bytes = 0;
    }
    if (lines.length) lines.push("");
    // (One at a time: a fence can hold more lines than a call can take as arguments.)
    for (const line of block.lines) lines.push(line);
    count += block.textLines;
    bytes += block.textBytes;
  }
  if (lines.length) pieces.push(lines.join("\n"));
  return pieces;
}

const plain = (markdown: string) =>
  stringify.stringify({
    type: "root",
    children: [
      { type: "element", tagName: "p", properties: { className: ["as-text"] }, children: [{ type: "text", value: "Shown as plain text: it's too long or too deeply nested to lay out." }] },
      { type: "element", tagName: "pre", properties: { className: ["plain"], tabIndex: 0 }, children: [{ type: "text", value: markdown }] },
    ],
  });

/**
 * Markdown in, safe HTML out, within BOUNDS (and the page's `budget`, when
 * given, which it spends): past them, or when the parser can't finish (a
 * nesting it can't hold), the document is shown as its plain text, and
 * `cut` says why.
 */
export function render(markdown: string, options: RenderOptions, budget?: Budget): Rendered {
  const cut = (why: Cut): Rendered => ({ html: plain(markdown), cut: why });
  const plan = planOf(markdown);
  if (!plan) return cut("shape");
  const { pieces, carried, work } = plan;
  if (work > BOUNDS.work) return cut("budget");
  if (budget) {
    if (work > budget.work) return cut("page");
    budget.work -= work;
  }
  try {
    const children: RootContent[] = [];
    for (const [i, piece] of pieces.entries()) {
      for (const node of (processor.parse(piece + (carried[i] ?? "")) as unknown as { children: RootContent[] }).children) children.push(node);
    }
    const tree = processor.runSync({ type: "root", children } as never) as Root;
    point(tree, options);
    const clean = sanitize.runSync(tree) as Root;
    finish(clean, options);
    footnotesOwn(clean, fingerprint(markdown));
    const html = stringify.stringify(clean);
    // (Nothing comes out larger than BOUNDS.html, whatever got past the estimate.)
    return html.length > BOUNDS.html ? cut("budget") : { html, cut: null };
  } catch {
    return cut("shape");
  }
}

/**
 * What drawing a document would cost, reckoned without parsing it (COST,
 * about milliseconds), or null when a block is past a bound (BOUNDS).
 */
export function workOf(markdown: string): number | null {
  return planOf(markdown)?.work ?? null;
}

/**
 * Whether a document would be drawn, reckoned without parsing it: why not
 * (a shape past a bound, or past its budget), or null. (A document can still
 * be its text if the parser fails on it; render says so when it does.)
 */
export function drawable(markdown: string): Cut | null {
  const plan = planOf(markdown);
  return !plan ? "shape" : plan.work > BOUNDS.work ? "budget" : null;
}

/** The pieces a document is parsed in, what each carries, and what it all costs; null past a bound. */
function planOf(markdown: string): { pieces: string[]; carried: string[]; work: number } | null {
  const shaped = blocksOf(markdown);
  if (!shaped) return null;
  const pieces = piecesOf(shaped.blocks);
  // (Each piece knows the definitions it refers to, so a reference finds its target wherever it is.)
  const carried = pieces.length > 1 ? pieces.map((piece) => carriedInto(piece, shaped.definitions)) : [];
  return { pieces, carried: carried.map((c) => c.text), work: shaped.work + carried.reduce((n, c) => n + c.work, 0) };
}

/** Markdown in, safe HTML out (render's HTML, within its own budget). */
export function renderMarkdown(markdown: string, options: RenderOptions): string {
  return render(markdown, options).html;
}

// ── point ──────────────────────────────────────────────────────────────

/** What a reference in the document is. */
export type Target =
  | { kind: "file"; path: string }
  | { kind: "anchor"; id: string }
  | { kind: "web"; url: string }
  | { kind: "none" };

/** Classifies a link's or a picture's reference, as written. */
export function target(reference: string): Target {
  const value = reference.trim();
  if (value === "") return { kind: "none" };
  if (value.startsWith("#")) {
    let id: string;
    try {
      id = decodeURIComponent(value.slice(1));
    } catch {
      return { kind: "none" };
    }
    return id ? { kind: "anchor", id } : { kind: "none" };
  }
  // A scheme (`https:`, `javascript:`, `data:`), or the protocol-relative `//host`.
  // eslint-disable-next-line no-control-regex
  const stripped = value.replace(/[\u0000- \u007f]/g, "");
  if (/^[a-z][a-z0-9+.-]*:/i.test(stripped) || stripped.startsWith("//") || /^[\\/]{2}/.test(stripped)) {
    let url: URL;
    try {
      url = new URL(stripped.startsWith("//") ? `https:${stripped}` : stripped);
    } catch {
      return { kind: "none" };
    }
    // (Parsed, and given back as the parsed address: never the author's text. No credentials.)
    return LINK_PROTOCOLS.has(url.protocol) && !url.username && !url.password ? { kind: "web", url: url.href } : { kind: "none" };
  }
  const path = resolvePackagePath(value);
  return path ? { kind: "file", path } : { kind: "none" };
}

function point(tree: Root, options: RenderOptions): void {
  visit(tree, "element", (node, index, parent) => {
    if (node.tagName === "img") {
      if (!parent || index === undefined) return SKIP;
      const replacement = picture(node, options);
      parent.children.splice(index, 1, ...replacement);
      return [SKIP, index + replacement.length];
    }
    if (node.tagName === "a") {
      const href = typeof node.properties.href === "string" ? node.properties.href : "";
      // Footnotes' own links (made by remark-rehype, already prefixed).
      if (href.startsWith(`#${PREFIX}`) && (node.properties.dataFootnoteRef !== undefined || node.properties.dataFootnoteBackref !== undefined)) return;
      const to = target(href);
      if (to.kind === "web") node.properties.href = to.url;
      else if (to.kind === "anchor") node.properties.href = `#${PREFIX}${slug(to.id)}`;
      else if (parent && index !== undefined) {
        // A link to another file of the package, or one that can't be
        // followed: its words stay, as words.
        parent.children.splice(index, 1, ...node.children);
        return [SKIP, index];
      }
    }
    return undefined;
  });
}

/** A picture: the package's own, through the file route; any other, a link to it or its words. */
function picture(node: Element, options: RenderOptions): ElementContent[] {
  const src = typeof node.properties.src === "string" ? node.properties.src : "";
  const alt = typeof node.properties.alt === "string" ? node.properties.alt : "";
  const to = target(src);
  if (to.kind === "file" && PICTURES.test(to.path)) {
    const out: Element = { type: "element", tagName: "img", properties: { src: options.file(to.path), alt }, children: [] };
    if (typeof node.properties.title === "string") out.properties.title = node.properties.title;
    return [out];
  }
  if (to.kind === "web" && to.url.startsWith("https:")) {
    // Not what was reviewed, so not shown: a link to it, named by its words.
    return [{ type: "element", tagName: "a", properties: { href: to.url }, children: [{ type: "text", value: alt || to.url }] }];
  }
  return alt ? [{ type: "text", value: alt }] : [];
}

// ── finish ─────────────────────────────────────────────────────────────

function finish(tree: Root, options: RenderOptions): void {
  const left = { bytes: HIGHLIGHT_BUDGET };
  // A first heading that is the plugin's name, said again: not drawn.
  if (options.title) {
    const first = tree.children.findIndex((node) => !(node.type === "text" && node.value.trim() === ""));
    const node = tree.children[first];
    if (node?.type === "element" && node.tagName === "h1" && headingWords(toText(node)) === headingWords(options.title)) tree.children.splice(first, 1);
  }
  const slugger = new GithubSlugger();
  visit(tree, "element", (node, index, parent) => {
    const level = /^h([1-6])$/.exec(node.tagName)?.[1];
    if (level && node.properties.id === undefined) {
      node.properties.id = `${PREFIX}${slugger.slug(toText(node))}`;
      if (options.demote) node.tagName = `h${Math.min(6, Number(level) + 1)}`;
      return;
    }
    if (node.tagName === "a") {
      const href = String(node.properties.href ?? "");
      if (!href.startsWith("#")) node.properties.rel = REL;
      return;
    }
    if (node.tagName === "img") {
      node.properties.loading = "lazy";
      node.properties.decoding = "async";
      return;
    }
    if (node.tagName === "table" && parent && index !== undefined) {
      // A wide table scrolls in its own box, never the page (reachable by
      // keyboard, so it can be scrolled without a pointer).
      const scroller: Element = { type: "element", tagName: "div", properties: { className: ["table-scroll"], tabIndex: 0 }, children: [node] };
      parent.children[index] = scroller as RootContent & ElementContent;
      return SKIP;
    }
    if (node.tagName === "pre") {
      // (A long line scrolls in its box: reachable by keyboard.)
      node.properties.tabIndex = 0;
      return undefined;
    }
    if (node.tagName === "code" && parent && parent.type === "element" && parent.tagName === "pre") {
      highlight(node, left);
      return SKIP;
    }
    return undefined;
  });
}

/**
 * A document's footnotes, its own: their ids (and the links to them, and
 * the footnotes' heading) carry the document's tag, so a README and a
 * version's notes on one page never share an id.
 */
function footnotesOwn(tree: Root, tag: string): void {
  const own = (id: string) => `${PREFIX}${tag}-${id.startsWith(PREFIX) ? id.slice(PREFIX.length) : id}`;
  visit(tree, "element", (node) => {
    const id = node.properties.id;
    if (typeof id === "string" && (id === "footnote-label" || id.startsWith(`${PREFIX}fn`))) node.properties.id = own(id);
    const href = node.properties.href;
    if (typeof href === "string" && href.startsWith(`#${PREFIX}fn`)) node.properties.href = `#${own(href.slice(1))}`;
    const described: unknown = node.properties.ariaDescribedBy;
    if (described === "footnote-label" || (Array.isArray(described) && described.includes("footnote-label"))) node.properties.ariaDescribedBy = [own("footnote-label")];
  });
}

function highlight(code: Element, left: { bytes: number }): void {
  const classes = Array.isArray(code.properties.className) ? code.properties.className : [];
  const language = classes.map(String).find((c) => c.startsWith("language-"))?.slice("language-".length).toLowerCase();
  const text = toText(code, { whitespace: "pre" });
  // (Within highlight.ts's bounds, and the document's budget; else plain code.)
  if (!language || !lowlight.registered(language) || !highlightable(text, language) || text.length > left.bytes) return;
  left.bytes -= text.length;
  let tree: Root;
  try {
    tree = lowlight.highlight(language, text) as Root;
  } catch {
    return;
  }
  // (A block that nests the highlighter's tree past its bound stays plain code.)
  if (nestsTooDeep(tree)) return;
  code.children = tree.children as ElementContent[];
  code.properties.className = ["hljs", `language-${language}`];
}

/** An anchor's id, as GitHub slugs a heading (without the de-duplication). */
function slug(value: string): string {
  return GithubSlugger.prototype.slug.call(new GithubSlugger(), value);
}

/**
 * A version's notes in one line, for its row (the first line that says
 * something, without its markdown): "- Fixed **the** timer" → "Fixed the timer".
 */
export function firstLine(markdown: string | null | undefined): string {
  for (const raw of (markdown ?? "").split("\n")) {
    const line = raw
      .replace(/^\s*(#{1,6}\s+|[-*+]\s+(\[[ xX]\]\s+)?|\d+[.)]\s+|>\s*)/, "")
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/(\*\*|__|\*|_|~~|`)/g, "")
      .trim();
    if (line) return line;
  }
  return "";
}
