// The one writer of HTML the store's pages put an author's words through
// (a README, a version's notes, a file's code): a fixed set of tags with a
// fixed set of attributes (TAGS), every text and attribute value escaped.
// Anything else is refused, so a mistake fails loudly instead of being
// written. No imports: md/html.ts and highlight.ts both write with it.

import type { Root, RootContent } from "hast";

/** Every tag written, and the attributes each may carry. */
export const TAGS: Readonly<Record<string, readonly string[]>> = {
  // (`class` on p and pre only for asText's own notice and block, never from a document.)
  p: ["class"],
  br: [],
  hr: [],
  h1: ["id"],
  h2: ["id"],
  h3: ["id"],
  h4: ["id"],
  h5: ["id"],
  h6: ["id"],
  blockquote: [],
  ul: ["class"],
  ol: ["start", "class"],
  li: ["class"],
  input: ["type", "disabled", "checked"],
  pre: ["tabindex", "class"],
  code: ["class"],
  span: ["class"],
  em: [],
  strong: [],
  del: [],
  a: ["href", "rel"],
  img: ["src", "alt", "title", "loading", "decoding"],
  div: ["class", "tabindex"],
  table: [],
  thead: [],
  tbody: [],
  tr: [],
  th: ["align"],
  td: ["align"],
};

const VOID = new Set(["br", "hr", "img", "input"]);

/** A rendering past this many characters is not written (the text is shown instead). */
export class TooLarge extends Error {}

/** A text as HTML: `&`, `<`, `>` and `"` escaped. */
export const escapeText = (s: string) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");

/** Writes HTML from the allowed tags alone, and stops at `most` characters. */
export class Html {
  private readonly parts: string[] = [];
  private size = 0;
  private readonly most: number;
  constructor(most: number) {
    this.most = most;
  }

  private add(s: string) {
    this.size += s.length;
    if (this.size > this.most) throw new TooLarge();
    this.parts.push(s);
  }

  open(tag: string, attributes: [string, string | true][] = []) {
    const allowed = TAGS[tag];
    if (!allowed) throw new Error(`not a tag this writes: ${tag}`);
    let out = `<${tag}`;
    for (const [name, value] of attributes) {
      if (!allowed.includes(name)) throw new Error(`not an attribute this writes: ${tag} ${name}`);
      out += value === true ? ` ${name}` : ` ${name}="${escapeText(value)}"`;
    }
    this.add(`${out}>`);
  }

  close(tag: string) {
    if (!TAGS[tag] || VOID.has(tag)) throw new Error(`not a tag this closes: ${tag}`);
    this.add(`</${tag}>`);
  }

  text(s: string) {
    if (s) this.add(escapeText(s));
  }

  /** A line end between blocks (as CommonMark's own HTML has). */
  line() {
    this.add("\n");
  }

  toString() {
    return this.parts.join("");
  }
}


/** The highlighter's tree: its spans' classes and its text, nothing else (walked without recursion). */
export function writeHighlighted(out: Html, tree: Root): void {
  const stack: { nodes: RootContent[]; i: number; close: boolean }[] = [{ nodes: tree.children, i: 0, close: false }];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame.i >= frame.nodes.length) {
      stack.pop();
      if (frame.close) out.close("span");
      continue;
    }
    const node = frame.nodes[frame.i++];
    if (node.type === "text") out.text(node.value);
    else if (node.type === "element") {
      const classes = (Array.isArray(node.properties.className) ? node.properties.className : []).map(String).filter((c) => /^[\w-]{1,64}$/.test(c));
      out.open("span", classes.length > 0 ? [["class", classes.join(" ")]] : []);
      stack.push({ nodes: node.children as RootContent[], i: 0, close: true });
    }
  }
}


/** A highlighter's tree as HTML (its spans' classes and its text). */
export function highlightedHtml(tree: Root): string {
  const out = new Html(Infinity);
  writeHighlighted(out, tree);
  return out.toString();
}
