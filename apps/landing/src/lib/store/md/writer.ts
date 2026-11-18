// The one writer of HTML the store's pages put an author's words through
// (a README, a version's notes, a file's code): a fixed set of tags with a
// fixed set of attributes (TAGS), every text and attribute value escaped.
// Anything else is refused, so a mistake fails loudly instead of being
// written. No imports: md/html.ts and highlight.ts both write with it.

import type { Root, RootContent } from "hast";
import { TOO_LARGE, TooLarge, type Meter } from "./meter.ts";

export { TooLarge };

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


/** A text as HTML: `&`, `<`, `>` and `"` escaped. */
export const escapeText = (s: string) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");

/** A chunk of what is written, in UTF-16 code units: one buffer, not a string for every piece. */
const CHUNK = 16 * 1024;
const decoder = new TextDecoder("utf-16le");

/**
 * Writes HTML from the allowed tags alone, and stops at `most` characters.
 * What it writes is copied, escaped as it goes, into a buffer of code units
 * turned into a string a chunk at a time: what it keeps is what it wrote,
 * not an object for every tag and text.
 */
export class Html {
  private readonly chunks: string[] = [];
  private buffer = new Uint16Array(CHUNK);
  private at = 0;
  private size = 0;
  private readonly most: number;
  private readonly meter: Meter | null;
  /** Writes at most `most` characters (each counted on `meter`, when given), and stops past them. */
  constructor(most: number, meter: Meter | null = null) {
    this.most = most;
    this.meter = meter;
  }

  private flush() {
    this.chunks.push(decoder.decode(this.buffer.subarray(0, this.at)));
    this.at = 0;
  }

  private unit(code: number) {
    // (Checked a character at a time: a long text stops at the bound, not a text past it.)
    if (this.size === this.most) {
      if (this.meter) this.meter.written = this.size;
      throw TOO_LARGE;
    }
    if (this.at === CHUNK) this.flush();
    this.buffer[this.at++] = code;
    this.size++;
  }

  /** `s` as it is (`escape`: `&`, `<`, `>`, `"` as references). */
  private add(s: string, escape = false) {
    for (let i = 0; i < s.length; i++) {
      const code = s.charCodeAt(i);
      if (escape && (code === 38 || code === 60 || code === 62 || code === 34)) {
        const ref = code === 38 ? "&amp;" : code === 60 ? "&lt;" : code === 62 ? "&gt;" : "&quot;";
        for (let k = 0; k < ref.length; k++) this.unit(ref.charCodeAt(k));
      } else this.unit(code);
    }
    if (this.meter) this.meter.written = this.size;
  }

  open(tag: string, attributes: [string, string | true][] = []) {
    const allowed = TAGS[tag];
    if (!allowed) throw new Error(`not a tag this writes: ${tag}`);
    this.add(`<${tag}`);
    for (const [name, value] of attributes) {
      if (!allowed.includes(name)) throw new Error(`not an attribute this writes: ${tag} ${name}`);
      if (value === true) this.add(` ${name}`);
      else {
        this.add(` ${name}="`);
        this.add(value, true);
        this.add(`"`);
      }
    }
    this.add(">");
  }

  close(tag: string) {
    if (!TAGS[tag] || VOID.has(tag)) throw new Error(`not a tag this closes: ${tag}`);
    this.add(`</${tag}>`);
  }

  text(s: string) {
    if (s) this.add(s, true);
  }

  /** A line end between blocks (as CommonMark's own HTML has). */
  line() {
    this.add("\n");
  }

  toString() {
    if (this.at > 0) this.flush();
    return this.chunks.join("");
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
