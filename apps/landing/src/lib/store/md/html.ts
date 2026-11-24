// The store's markdown, last pass: blocks and inlines to HTML.
//
// Safe by construction: the author's markdown never becomes HTML by itself.
// It is written by the store's one writer (md/writer.ts): a fixed set of
// tags with a fixed set of attributes (TAGS), every text and attribute
// value escaped, every address through `target`
// (a link: https or mail, parsed and written back as parsed; a picture: a
// file of the package, through the store's file route). `Html` refuses
// anything else, so a mistake here fails loudly instead of writing it.
//
// Bounded: each address is classified once a document, however often it is
// used; a reference used many times writes its address each time (at most
// ADDRESS + TITLE characters a use), and a short row its missing cells (at
// most COLUMNS); what it writes is counted as it is written, and it
// stops at `most` characters (the caller shows the text instead). A tree of
// inlines nests as deep as its text is long, so it is walked without
// recursion; past NESTING, what's inside is written as its words.

import GithubSlugger from "github-slugger";
import type { Root } from "hast";
import { highlightable, lowlight, nestsTooDeep } from "../highlight.ts";
import { resolvePackagePath } from "../names.ts";
import { blocksOf, cells, NESTING, type Align, type Block, type Definition } from "./blocks.ts";
import { inlinesOf, plainText, unescape, type Inline } from "./inlines.ts";
import { meter, release, type Meter } from "./meter.ts";
import { Html, TAGS, TooLarge, writeHighlighted } from "./writer.ts";

export { TAGS, TooLarge };

export interface RenderOptions {
  /** The address a file of the package is served at, given its normalized path (names.ts). */
  file: (path: string) => string;
  /** The page's own heading is an h1: the document's start a level under it. */
  demote?: boolean;
  /** The plugin's name, said by the page already: a first top-level heading that only says it again is not drawn. */
  title?: string;
}

/** The prefix every id the document makes carries, so none can clobber the page's. */
export const PREFIX = "user-content-";

/** Pictures a README may show from its package, by extension (what the store serves inline). */
const PICTURES = /\.(png|jpe?g|svg)$/i;

/** Links a person may follow out of a README: https, and mail. */
const LINK_PROTOCOLS = new Set(["https:", "mailto:"]);

const REL = "nofollow noopener ugc";

/** What one document's code blocks may highlight, together, in bytes (highlight.ts bounds each). */
export const HIGHLIGHT_BUDGET = 16 * 1024;

// ── addresses ─────────────────────────────────────────────────────────────

/** What a reference in the document is. */
export type Target = { kind: "file"; path: string } | { kind: "anchor"; id: string } | { kind: "web"; url: string } | { kind: "none" };

/** Classifies a link's or a picture's reference, as written (escapes undone). */
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

/** An anchor's id, as GitHub slugs a heading (without the de-duplication). */
const slug = (value: string) => GithubSlugger.prototype.slug.call(new GithubSlugger(), value);

/** A heading's words, for comparing with a name: spaces made one, case set aside. */
const headingWords = (text: string) => text.replace(/\s+/g, " ").trim().toLocaleLowerCase("en");

// ── writing ───────────────────────────────────────────────────────────────

interface Writing {
  out: Html;
  definitions: Map<string, Definition>;
  options: RenderOptions;
  slugger: GithubSlugger;
  highlight: { bytes: number };
  m: Meter;
  /** Each address classified once, however often it is used (a reference used a thousand times is one). */
  targets: Map<string, Target>;
}

/** An address, classified once a document; an anchor's id slugged with it. */
function targetOf(w: Writing, destination: string): Target {
  let to = w.targets.get(destination);
  if (!to) {
    w.m.read += destination.length * 4;
    to = target(destination);
    if (to.kind === "anchor") to = { kind: "anchor", id: slug(to.id) };
    w.targets.set(destination, to);
  }
  return to;
}

/** A text's inlines, written; what parsing them kept, let go once they are. */
function writeText(w: Writing, text: string): void {
  const parsed = inlinesOf(text, w.definitions, w.m);
  writeInlines(w, parsed.inlines);
  release(w.m, parsed.kept);
}

/**
 * Inlines, walked without recursion. A link inside a link (a picture's,
 * once it becomes one) is its words; past NESTING, a node is its words.
 */
function writeInlines(w: Writing, list: Inline[]): void {
  const { out, options } = w;
  type Frame = { list: Inline[]; i: number; close: string | null; inLink: boolean };
  const stack: Frame[] = [{ list, i: 0, close: null, inLink: false }];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame.i >= frame.list.length) {
      stack.pop();
      if (frame.close) out.close(frame.close);
      continue;
    }
    const n = frame.list[frame.i++];
    w.m.read++;
    switch (n.kind) {
      case "text":
        out.text(n.text);
        break;
      case "code":
        out.open("code");
        out.text(n.text);
        out.close("code");
        break;
      case "softbreak":
        out.text("\n");
        break;
      case "hardbreak":
        out.open("br");
        out.text("\n");
        break;
      case "em":
      case "strong":
      case "del": {
        if (stack.length > NESTING) {
          out.text(plainText(n.children, w.m));
          break;
        }
        out.open(n.kind);
        stack.push({ list: n.children, i: 0, close: n.kind, inLink: frame.inLink });
        break;
      }
      case "link": {
        const to = targetOf(w, n.destination);
        if (frame.inLink || stack.length > NESTING) {
          out.text(plainText(n.children, w.m));
        } else if (to.kind === "web") {
          out.open("a", [["href", to.url], ["rel", REL]]);
          stack.push({ list: n.children, i: 0, close: "a", inLink: true });
        } else if (to.kind === "anchor") {
          out.open("a", [["href", `#${PREFIX}${to.id}`]]);
          stack.push({ list: n.children, i: 0, close: "a", inLink: true });
        } else {
          // A link to another file of the package, or one that can't be followed: its words stay.
          stack.push({ list: n.children, i: 0, close: null, inLink: frame.inLink });
        }
        break;
      }
      case "image": {
        const alt = plainText(n.children, w.m);
        const to = targetOf(w, n.destination);
        if (to.kind === "file" && PICTURES.test(to.path)) {
          const attributes: [string, string][] = [["src", options.file(to.path)], ["alt", alt]];
          if (n.title !== null) attributes.push(["title", n.title]);
          attributes.push(["loading", "lazy"], ["decoding", "async"]);
          out.open("img", attributes);
        } else if (to.kind === "web" && to.url.startsWith("https:") && !frame.inLink) {
          // Not what was reviewed, so not shown: a link to it, named by its words.
          out.open("a", [["href", to.url], ["rel", REL]]);
          out.text(alt || to.url);
          out.close("a");
        } else out.text(alt);
        break;
      }
    }
  }
}

/** A code block: highlighted when its language is known and it's within the bounds and the document's budget. */
function writeCode(w: Writing, info: string, text: string): void {
  const { out } = w;
  const word = unescape(info, w.m).split(/[ \t]/, 1)[0] ?? "";
  const language = /^[\w+#.-]{1,32}$/.test(word) ? word.toLowerCase() : "";
  out.open("pre", [["tabindex", "0"]]);
  let tree: Root | null = null;
  if (language && lowlight.registered(language) && highlightable(text, language) && text.length <= w.highlight.bytes) {
    w.highlight.bytes -= text.length;
    w.m.read += text.length;
    try {
      tree = lowlight.highlight(language, text) as Root;
      if (nestsTooDeep(tree)) tree = null;
    } catch {
      tree = null;
    }
  }
  if (!tree) {
    out.open("code", language ? [["class", `language-${language}`]] : []);
    out.text(text);
  } else {
    out.open("code", [["class", `hljs language-${language}`]]);
    writeHighlighted(out, tree);
  }
  out.close("code");
  out.close("pre");
}

function writeBlocks(w: Writing, blocks: Block[], tight: boolean): void {
  const { out, options } = w;
  for (const block of blocks) {
    w.m.read++;
    switch (block.kind) {
      case "paragraph":
        if (!tight) out.open("p");
        writeText(w, block.text);
        if (!tight) out.close("p");
        // (A tight item's last paragraph ends with the item.)
        if (!tight || block !== blocks[blocks.length - 1]) out.line();
        break;
      case "heading": {
        const parsed = inlinesOf(block.text, w.definitions, w.m);
        const level = options.demote ? Math.min(6, block.level + 1) : block.level;
        const tag = `h${level}`;
        out.open(tag, [["id", `${PREFIX}${w.slugger.slug(plainText(parsed.inlines, w.m))}`]]);
        writeInlines(w, parsed.inlines);
        release(w.m, parsed.kept);
        out.close(tag);
        out.line();
        break;
      }
      case "code":
        writeCode(w, block.info, block.text);
        out.line();
        break;
      case "rule":
        out.open("hr");
        out.line();
        break;
      case "html": {
        // Raw HTML: the characters typed, line for line. Never HTML.
        out.open("p");
        block.text.split("\n").forEach((line, i) => {
          if (i > 0) {
            out.open("br");
            out.text("\n");
          }
          out.text(line);
        });
        out.close("p");
        out.line();
        break;
      }
      case "quote":
        out.open("blockquote");
        out.line();
        writeBlocks(w, block.children, false);
        out.close("blockquote");
        out.line();
        break;
      case "list": {
        const tag = block.ordered ? "ol" : "ul";
        const tasks = block.items.some((item) => item.task !== null);
        const attributes: [string, string][] = [];
        if (block.ordered && block.start !== 1) attributes.push(["start", String(block.start)]);
        if (tasks) attributes.push(["class", "contains-task-list"]);
        out.open(tag, attributes);
        out.line();
        for (const item of block.items) {
          out.open("li", item.task !== null ? [["class", "task-list-item"]] : []);
          if (item.task !== null) {
            out.open("input", item.task ? [["type", "checkbox"], ["disabled", true], ["checked", true], ["aria-label", "Done"]] : [["type", "checkbox"], ["disabled", true], ["aria-label", "To do"]]);
            out.text(" ");
          }
          // (A tight item's paragraphs are its text; anything else starts on its own line.)
          if (item.children.length > 0 && !(block.tight && item.children[0].kind === "paragraph")) out.line();
          writeBlocks(w, item.children, block.tight);
          out.close("li");
          out.line();
        }
        out.close(tag);
        out.line();
        break;
      }
      case "table": {
        out.open("div", [["class", "table-scroll"], ["tabindex", "0"]]);
        out.open("table");
        out.line();
        out.open("thead");
        out.line();
        writeRow(w, "th", block.head, block.align);
        out.close("thead");
        out.line();
        if (block.rows.length > 0) {
          out.open("tbody");
          out.line();
          for (const row of block.rows) {
            w.m.read += row.length * 2;
            writeRow(w, "td", cells(row, block.align.length), block.align);
          }
          out.close("tbody");
          out.line();
        }
        out.close("table");
        out.close("div");
        out.line();
        break;
      }
    }
  }
}

/** A row: its cells as written, then empty ones to the header's width (written, never kept). */
function writeRow(w: Writing, tag: "th" | "td", cells: string[], align: Align[]): void {
  w.out.open("tr");
  w.out.line();
  for (let i = 0; i < align.length; i++) {
    w.out.open(tag, align[i] ? [["align", align[i]!]] : []);
    if (i < cells.length) writeText(w, cells[i]);
    w.out.close(tag);
    w.out.line();
  }
  w.out.close("tr");
  w.out.line();
}

/** What a document shown as its text says first: too long, or it couldn't be drawn (it failed, or a try at it never finished). */
export const AS_TEXT = { long: "Shown as plain text: it's too long to lay out.", failed: "Shown as plain text: it couldn't be laid out." } as const;

/** A document as its text, said so (every character escaped). */
export function asText(markdown: string, why: keyof typeof AS_TEXT = "long"): string {
  const out = new Html(Infinity);
  out.open("p", [["class", "as-text"]]);
  out.text(AS_TEXT[why]);
  out.close("p");
  out.open("pre", [["class", "plain"], ["tabindex", "0"]]);
  out.text(markdown);
  out.close("pre");
  return out.toString();
}

export interface Written {
  html: string;
  /** What was read, kept and written (md/meter.ts): the tests hold it to constants per byte. */
  meter: Meter;
}

/**
 * Markdown in, HTML out: the subset drawn, everything else its characters.
 * Throws TooLarge past `most` characters of HTML.
 */
export function toHtml(markdown: string, options: RenderOptions, most: number, m: Meter = meter()): Written {
  const parsed = blocksOf(markdown, m);
  let blocks = parsed.blocks;
  const w: Writing = { out: new Html(most, m), definitions: parsed.definitions, options, slugger: new GithubSlugger(), highlight: { bytes: HIGHLIGHT_BUDGET }, m, targets: new Map() };
  // A first heading that is the plugin's name, said again: not drawn.
  const first = blocks[0];
  if (options.title && first?.kind === "heading" && first.level === 1) {
    const heading = inlinesOf(first.text, parsed.definitions, m);
    if (headingWords(plainText(heading.inlines, m)) === headingWords(options.title)) blocks = blocks.slice(1);
    release(m, heading.kept);
  }
  writeBlocks(w, blocks, false);
  return { html: w.out.toString(), meter: m };
}
