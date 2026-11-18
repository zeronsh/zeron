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
import { resolvePackagePath } from "./names.ts";

export interface RenderOptions {
  /**
   * The address a file of the package is served at, given its normalized
   * path (names.ts); every picture shown comes from here.
   */
  file: (path: string) => string;
  /** The page's own heading is an h1: the document's start a level under it. */
  demote?: boolean;
}

/** Pictures a README may show from its package, by extension (what the store serves inline). */
const PICTURES = /\.(png|jpe?g|svg)$/i;

/** The prefix every id the document makes carries, so none can clobber the page's. */
const PREFIX = "user-content-";

/** Code past this many characters is shown plain: highlighting is a nicety. */
const HIGHLIGHT_LIMIT = 40_000;

/** Links a person may follow out of a README. */
const LINK_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);

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
export const RENDERER = 3;

/**
 * Emphasis delimiters (`*`, `_`, `~`) outside code a document may hold and
 * still be parsed: resolving them costs the square of their number (40,000
 * took 18 s), so past this the document is shown as its text. No README
 * needs a tenth of it; code, where underscores live, doesn't count.
 */
export const MOST_DELIMITERS = 3000;

/** How many emphasis delimiters a document has outside fenced code and code spans. */
export function delimiters(markdown: string): number {
  let count = 0;
  let fence: string | null = null;
  for (const line of markdown.split("\n")) {
    const opens = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (fence) {
      if (opens && opens[0] === fence[0] && opens.length >= fence.length) fence = null;
      continue;
    }
    if (opens) {
      fence = opens;
      continue;
    }
    for (const ch of line.replace(/(`+)[^`]*?\1/g, "")) if (ch === "*" || ch === "_" || ch === "~") count++;
  }
  return count;
}

const plain = (markdown: string) =>
  stringify.stringify({ type: "root", children: [{ type: "element", tagName: "pre", properties: { className: ["plain"], tabIndex: 0 }, children: [{ type: "text", value: markdown }] }] });

/**
 * Markdown in, safe HTML out. A document that would cost too much to parse
 * (MOST_DELIMITERS), or that the parser can't finish (nesting deep enough
 * to exhaust the stack), is shown as its plain text instead.
 */
export function renderMarkdown(markdown: string, options: RenderOptions): string {
  if (delimiters(markdown) > MOST_DELIMITERS) return plain(markdown);
  try {
    const tree = processor.runSync(processor.parse(markdown)) as Root;
    point(tree, options);
    const clean = sanitize.runSync(tree) as Root;
    finish(clean, options);
    return stringify.stringify(clean);
  } catch {
    return plain(markdown);
  }
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
    return LINK_PROTOCOLS.has(url.protocol) ? { kind: "web", url: url.href } : { kind: "none" };
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
  if (to.kind === "web" && to.url.startsWith("http")) {
    // Not what was reviewed, so not shown: a link to it, named by its words.
    return [{ type: "element", tagName: "a", properties: { href: to.url }, children: [{ type: "text", value: alt || to.url }] }];
  }
  return alt ? [{ type: "text", value: alt }] : [];
}

// ── finish ─────────────────────────────────────────────────────────────

function finish(tree: Root, options: RenderOptions): void {
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
      highlight(node);
      return SKIP;
    }
    return undefined;
  });
}

function highlight(code: Element): void {
  const classes = Array.isArray(code.properties.className) ? code.properties.className : [];
  const language = classes.map(String).find((c) => c.startsWith("language-"))?.slice("language-".length).toLowerCase();
  const text = toText(code, { whitespace: "pre" });
  if (!language || !lowlight.registered(language) || text.length > HIGHLIGHT_LIMIT) return;
  const tree = lowlight.highlight(language, text);
  code.children = tree.children as ElementContent[];
  code.properties.className = ["hljs", `language-${language}`];
}

/** An anchor's id, as GitHub slugs a heading (without the de-duplication). */
function slug(value: string): string {
  return GithubSlugger.prototype.slug.call(new GithubSlugger(), value);
}
