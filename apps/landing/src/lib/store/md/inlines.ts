// The store's markdown, second pass: a block's text to inlines.
//
// The subset: emphasis, strong and strikethrough (`~` or `~~`), code spans,
// backslash escapes, character references, hard and soft breaks, inline and
// reference links and pictures, autolinks (`<https://…>`, `<a@b.c>`, and a
// bare `https://…`). Raw HTML is text: its `<` is the character typed.
//
// Linear, by construction (the argument; markdown.test.ts measures it).
// The text is read once, left to right; at each character one of these:
// - Plain text up to the next character that can mean something, read once.
// - A code span: backtick runs are indexed once (`Runs`), by length, so the
//   run that closes one is found without reading ahead; a run with none is
//   its characters.
// - Emphasis: CommonMark's delimiter stack, processed with `openers_bottom`
//   per (character, whether the closer can open, its length mod 3), so a
//   search for an opener never passes the same delimiter twice under one
//   key: removed when matched past, or below that key's bottom after.
//   A run's characters are a count, never a string cut a character at a
//   time.
// - Brackets: a stack. A `]` pops one. A link made turns every link opener
//   below it inactive by a watermark (the stack's height then), not by
//   walking the stack. Its text is a range of nodes moved in O(1).
// - A link's tail after `](`: the end of a bare destination is a lookup in
//   tables made once for the text (`Tails`): the next space, and the next
//   point where unescaped parentheses close below where they started (each
//   position's next smaller prefix depth, by one monotonic stack). A title's
//   end is the next unescaped quote of its kind, also from a table. An
//   `<…>` destination stops at the next `<`, so two never read the same
//   characters.
// - A reference's label is at most LABEL characters with no bracket in it
//   (the next bracket is from `Tails` too); the labels read for different
//   `]`s don't overlap, so each character is normalized at most twice.
// - `<…>` autolinks stop at the next `<`, space or `>`; character
//   references read at most 32 characters.
// - Bare `https://` links are found afterwards in text that isn't in a link,
//   each character read once.
// No regular expression runs over the author's text with a repeat inside a
// repeat; the few here test one character, or a span already bounded.
//
// Counted (md/meter.ts): each of the steps above adds what it read, and
// every node, delimiter, bracket and table it makes is counted as kept, so
// a text that would make more than KEEP_MOST of them stops (its document
// is shown as its text). A reference is resolved once, where it is
// defined (md/blocks.ts): its use here is a lookup and a pointer, however
// often it is used. An address is at most ADDRESS characters and a title
// TITLE; past either, a link is its characters.

import { ADDRESS, labelKey, LABEL, NESTING, TITLE, type Definition } from "./blocks.ts";
import { keep, meter, NODE, type Meter } from "./meter.ts";
import { isAsciiPunctuation, reference, unescape } from "./text.ts";

export { unescape };

export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "softbreak" }
  | { kind: "hardbreak" }
  | { kind: "em" | "strong" | "del"; children: Inline[] }
  | { kind: "link"; destination: string; title: string | null; children: Inline[] }
  | { kind: "image"; destination: string; title: string | null; children: Inline[] };

export interface Inlines {
  inlines: Inline[];
  /** What this text kept on the meter (its nodes, its tables): the caller lets it go once the text is written. */
  kept: number;
}

// ── nodes ─────────────────────────────────────────────────────────────────

type NodeKind = Inline["kind"] | "run" | "bracket";

/** A node while parsing: a doubly linked list, so a range moves in O(1). */
interface N {
  kind: NodeKind;
  text: string;
  /** A delimiter run's character and how many are left. */
  char: string;
  count: number;
  destination: string;
  title: string | null;
  first: N | null;
  last: N | null;
  prev: N | null;
  next: N | null;
}

const blank = (kind: NodeKind, text: string): N => ({ kind, text, char: "", count: 0, destination: "", title: null, first: null, last: null, prev: null, next: null });

interface Delimiter {
  node: N;
  char: string;
  /** Its length as written (for the rule of 3). */
  length: number;
  canOpen: boolean;
  canClose: boolean;
  prev: Delimiter | null;
  next: Delimiter | null;
}

interface Bracket {
  node: N;
  image: boolean;
  /** Where its text starts. */
  index: number;
  /** The delimiter stack's top when it opened. */
  delimiters: Delimiter | null;
  /** Another `[` opened right after this one's `]`. */
  bracketAfter: boolean;
}

// ── characters ────────────────────────────────────────────────────────────

/** Unicode punctuation and symbols (CommonMark 0.31), one character tested. */
const isPunctuation = (c: string) => (c.charCodeAt(0) < 128 ? isAsciiPunctuation(c) : /^[\p{P}\p{S}]$/u.test(c));
const isWhitespace = (c: string) => {
  if (c === "") return true;
  // (ASCII without a regular expression; past it, Unicode's spaces.)
  return c.charCodeAt(0) < 128 ? c === " " || c === "\t" || c === "\n" || c === "\f" || c === "\r" : /^[\s\p{Zs}]$/u.test(c);
};

/** The characters that can start something: everything else is plain text. */
const SPECIAL = new Uint8Array(128);
for (const c of "\n\\`*_~[]!<&") SPECIAL[c.charCodeAt(0)] = 1;

/** The character before `i` and at `i` (a whole code point, for the flanking rules). */
function before(s: string, i: number): string {
  if (i <= 0) return "";
  const code = s.charCodeAt(i - 1);
  return code >= 0xdc00 && code <= 0xdfff && i >= 2 ? s.slice(i - 2, i) : s[i - 1];
}
function at(s: string, i: number): string {
  if (i >= s.length) return "";
  return String.fromCodePoint(s.codePointAt(i)!);
}

// ── tables made once for a text ───────────────────────────────────────────

/**
 * Backtick runs, by length: the run that closes a code span opened at a
 * position is the first run of the same length after it. Asked in the order
 * the text is read, so each length's cursor only moves forward.
 */
class Runs {
  private readonly byLength = new Map<number, number[]>();
  private readonly cursor = new Map<number, number>();

  constructor(s: string) {
    for (let i = 0; i < s.length; ) {
      if (s[i] !== "`") {
        i++;
        continue;
      }
      let j = i;
      while (s[j] === "`") j++;
      const list = this.byLength.get(j - i);
      if (list) list.push(i);
      else this.byLength.set(j - i, [i]);
      i = j;
    }
  }

  /** The first run of `length` backticks starting after `after`, or -1. */
  closer(length: number, after: number): number {
    const list = this.byLength.get(length);
    if (!list) return -1;
    let c = this.cursor.get(length) ?? 0;
    while (c < list.length && list[c] <= after) c++;
    this.cursor.set(length, c);
    return c < list.length ? list[c] : -1;
  }
}

/**
 * What a link's tail needs, for every position of the text, made in one pass
 * each (and only when a text has a `](` to read).
 */
class Tails {
  /** The next character at or after i that is a space, a control or a line end. */
  readonly space: Int32Array;
  /** The next unescaped `"`, `'`, `)`, `(` at or after i. */
  readonly quote: Int32Array;
  readonly apostrophe: Int32Array;
  readonly close: Int32Array;
  readonly open: Int32Array;
  /** The next unescaped `[` or `]` at or after i. */
  readonly bracket: Int32Array;
  /**
   * Where a bare destination starting at i ends on parentheses: the first
   * unescaped `)` at or after i that closes below the depth at i.
   */
  readonly unbalanced: Int32Array;
  /** Unescaped `(` less `)` before each position. */
  readonly depth: Int32Array;

  constructor(s: string) {
    const n = s.length;
    const escaped = new Uint8Array(n + 1);
    for (let i = 0; i < n; i++) if (s[i] === "\\" && i + 1 < n && isAsciiPunctuation(s[i + 1])) escaped[++i] = 1;
    const next = (test: (c: string, i: number) => boolean) => {
      const out = new Int32Array(n + 1);
      out[n] = n;
      for (let i = n - 1; i >= 0; i--) out[i] = test(s[i], i) ? i : out[i + 1];
      return out;
    };
    this.space = next((c) => c <= " ");
    this.quote = next((c, i) => c === '"' && !escaped[i]);
    this.apostrophe = next((c, i) => c === "'" && !escaped[i]);
    this.close = next((c, i) => c === ")" && !escaped[i]);
    this.open = next((c, i) => c === "(" && !escaped[i]);
    this.bracket = next((c, i) => (c === "[" || c === "]") && !escaped[i]);
    // depth[i]: unescaped `(` less `)` before i. A destination from i ends
    // on parentheses at the first k >= i whose `)` takes the depth below
    // depth[i]: the next m > i with depth[m] < depth[i] (then k = m - 1),
    // each position's next smaller element, by one monotonic stack.
    this.depth = new Int32Array(n + 1);
    for (let i = 0; i < n; i++) this.depth[i + 1] = this.depth[i] + (escaped[i] ? 0 : s[i] === "(" ? 1 : s[i] === ")" ? -1 : 0);
    this.unbalanced = new Int32Array(n + 1).fill(n);
    const waiting: number[] = [];
    for (let m = 0; m <= n; m++) {
      while (waiting.length > 0 && this.depth[m] < this.depth[waiting[waiting.length - 1]]) this.unbalanced[waiting.pop()!] = m - 1;
      waiting.push(m);
    }
  }
}

// ── the parser ────────────────────────────────────────────────────────────

const LOCAL = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.!#$%&'*+/=?^_`{|}~-";
const isAlnum = (c: string) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9");

/** An address `<a@b.c>` may link to: CommonMark's email autolink, read once. */
function isEmail(s: string): boolean {
  const at = s.indexOf("@");
  if (at < 1 || at !== s.lastIndexOf("@")) return false;
  for (let i = 0; i < at; i++) if (!LOCAL.includes(s[i])) return false;
  for (const label of s.slice(at + 1).split(".")) {
    if (label.length < 1 || label.length > 63 || label[0] === "-" || label[label.length - 1] === "-") return false;
    for (const c of label) if (!isAlnum(c) && c !== "-") return false;
  }
  return true;
}

/** The inlines of a block's text. */
export function inlinesOf(s: string, definitions: Map<string, Definition>, m: Meter = meter()): Inlines {
  const keptBefore = m.kept;
  const read = (n: number) => {
    m.read += n;
  };
  read(s.length);
  const make = (kind: NodeKind, text = ""): N => {
    keep(m, NODE);
    return blank(kind, text);
  };
  const root = make("link");
  const append = (n: N) => {
    n.prev = root.last;
    n.next = null;
    if (root.last) root.last.next = n;
    else root.first = n;
    root.last = n;
  };
  const unlink = (n: N) => {
    if (n.prev) n.prev.next = n.next;
    else root.first = n.next;
    if (n.next) n.next.prev = n.prev;
    else root.last = n.prev;
    n.prev = n.next = null;
  };
  const text = (t: string) => {
    if (t === "") return;
    keep(m, t.length);
    if (root.last?.kind === "text") root.last.text += t;
    else append(make("text", t));
  };

  let runs: Runs | null = null;
  let tails: Tails | null = null;
  // (Eight passes over the text, and seven tables of four bytes a character.)
  const tailsOf = () => (tails ??= (read(s.length * 8), keep(m, s.length * 28), new Tails(s)));

  let delimiters: Delimiter | null = null;
  const brackets: Bracket[] = [];
  /** Link openers below this height are inactive: a link was made above them. */
  let linksBelow = 0;

  const removeDelimiter = (d: Delimiter) => {
    if (d.prev) d.prev.next = d.next;
    if (d.next) d.next.prev = d.prev;
    else delimiters = d.prev;
  };

  /** CommonMark's "process emphasis", above `bottom`. */
  const processEmphasis = (bottom: Delimiter | null) => {
    if (delimiters === bottom) return;
    // (Per character, whether the closer can open, and its length mod 3: 3 × 2 × 3 keys.)
    const openersBottom: (Delimiter | null | undefined)[] = new Array(18);
    // (Every delimiter walked past here is above the bottom, and removed at the end.)
    let closer = delimiters;
    while (closer !== null && closer.prev !== bottom) {
      closer = closer.prev;
      read(1);
    }
    while (closer !== null) {
      read(1);
      if (!closer.canClose) {
        closer = closer.next;
        continue;
      }
      const key = (closer.char === "*" ? 0 : closer.char === "_" ? 6 : 12) + (closer.canOpen ? 3 : 0) + (closer.length % 3);
      const floor = openersBottom[key] === undefined ? bottom : openersBottom[key];
      let opener = closer.prev;
      let found = false;
      while (opener !== null && opener !== bottom && opener !== floor) {
        read(1);
        if (opener.char === closer.char && opener.canOpen) {
          if (closer.char === "~") {
            if (opener.length === closer.length) {
              found = true;
              break;
            }
          } else {
            const odd = (closer.canOpen || opener.canClose) && closer.length % 3 !== 0 && (opener.length + closer.length) % 3 === 0;
            if (!odd) {
              found = true;
              break;
            }
          }
        }
        opener = opener.prev;
      }
      const old = closer;
      if (found && opener) {
        const used = closer.char === "~" ? closer.node.count : closer.node.count >= 2 && opener.node.count >= 2 ? 2 : 1;
        const o = opener.node;
        const c = closer.node;
        o.count -= used;
        c.count -= used;
        const wrap = make(closer.char === "~" ? "del" : used === 2 ? "strong" : "em");
        // The nodes between the two runs become its children (a range, moved in O(1)).
        if (o.next !== c) {
          wrap.first = o.next;
          wrap.last = c.prev;
          wrap.first!.prev = null;
          wrap.last!.next = null;
        }
        o.next = wrap;
        wrap.prev = o;
        wrap.next = c;
        c.prev = wrap;
        // The delimiters between them are spent.
        opener.next = closer;
        closer.prev = opener;
        if (o.count === 0) {
          unlink(o);
          removeDelimiter(opener);
        }
        if (c.count === 0) {
          unlink(c);
          const next = closer.next;
          removeDelimiter(closer);
          closer = next;
        }
      } else {
        closer = closer.next;
        openersBottom[key] = old.prev;
        if (!old.canOpen) removeDelimiter(old);
      }
    }
    while (delimiters !== null && delimiters !== bottom) removeDelimiter(delimiters);
  };

  /** A link's tail at `p` (just past `](`'s `(`): its destination, title and end, or null. */
  const inlineTail = (p: number): { destination: string; title: string | null; end: number } | null => {
    const t = tailsOf();
    const skip = (j: number) => {
      while (s[j] === " " || s[j] === "\t") j++;
      if (s[j] === "\n") {
        j++;
        while (s[j] === " " || s[j] === "\t") j++;
      }
      return j;
    };
    let j = skip(p);
    let destination = "";
    if (s[j] === "<") {
      let k = j + 1;
      while (k < s.length && s[k] !== ">" && s[k] !== "<" && s[k] !== "\n") k += s[k] === "\\" && k + 1 < s.length && s[k + 1] !== "\n" ? 2 : 1;
      read(k - j);
      if (k - j - 1 > ADDRESS) return null;
      if (s[k] !== ">") return null;
      destination = s.slice(j + 1, k);
      j = k + 1;
    } else if (s[j] !== ")") {
      const end = Math.min(t.space[j], t.unbalanced[j]);
      // (Empty, or parentheses still open at its end: not a destination.)
      if (end === j || t.depth[end] !== t.depth[j]) return null;
      if (end - j > ADDRESS) return null;
      destination = s.slice(j, end);
      read(end - j);
      j = end;
    }
    const k = skip(j);
    let title: string | null = null;
    if (k > j && (s[k] === '"' || s[k] === "'" || s[k] === "(")) {
      const close = s[k] === '"' ? t.quote[k + 1] : s[k] === "'" ? t.apostrophe[k + 1] : t.close[k + 1];
      if (close >= s.length) return null;
      if (s[k] === "(" && t.open[k + 1] < close) return null;
      if (close - k - 1 > TITLE) return null;
      title = s.slice(k + 1, close);
      read(close - k);
      j = skip(close + 1);
    } else j = k;
    return s[j] === ")" ? { destination, title, end: j + 1 } : null;
  };

  const closeBracket = (pos: number): number => {
    const opener = brackets[brackets.length - 1];
    if (!opener) {
      text("]");
      return pos + 1;
    }
    const height = brackets.length - 1;
    if (!opener.image && height < linksBelow) {
      brackets.pop();
      text("]");
      return pos + 1;
    }
    let after = pos + 1;
    // (An inline link's address and title are undone here, once; a reference's were, where it was defined.)
    let found: { destination: string; title: string | null } | null = null;
    if (s[after] === "(") {
      const tail = inlineTail(after + 1);
      if (tail) {
        found = { destination: unescape(tail.destination, m), title: tail.title === null ? null : unescape(tail.title, m) };
        after = tail.end;
      }
    }
    if (!found) {
      // A reference: `[text][label]`, `[text][]` or `[text]`.
      let label: string | null = null;
      let end = pos + 1;
      if (s[pos + 1] === "[") {
        const t = tailsOf();
        const close = t.bracket[pos + 2];
        if (close < s.length && s[close] === "]" && close - (pos + 2) <= LABEL) {
          if (close > pos + 2) label = s.slice(pos + 2, close);
          end = close + 1;
        }
      }
      if (label === null && !opener.bracketAfter) {
        const t = tailsOf();
        if (t.bracket[opener.index] >= pos && pos - opener.index <= LABEL) label = s.slice(opener.index, pos);
        else label = null;
      }
      if (label !== null && label.trim() !== "") {
        read(label.length);
        const d = definitions.get(labelKey(label));
        if (d) {
          found = d;
          after = end;
        }
      }
    }
    if (!found) {
      brackets.pop();
      text("]");
      return pos + 1;
    }
    // Emphasis inside its text first, then the text becomes the link's.
    processEmphasis(opener.delimiters);
    const link = make(opener.image ? "image" : "link");
    link.destination = found.destination;
    link.title = found.title;
    if (opener.node.next) {
      link.first = opener.node.next;
      link.last = root.last;
      link.first.prev = null;
      opener.node.next = null;
      root.last = opener.node;
    }
    unlink(opener.node);
    append(link);
    brackets.pop();
    if (!opener.image) linksBelow = brackets.length;
    return after;
  };

  let pos = 0;
  while (pos < s.length) {
    const c = s[pos];
    const code = s.charCodeAt(pos);
    if (code >= 128 || SPECIAL[code] === 0) {
      let j = pos + 1;
      while (j < s.length) {
        const k = s.charCodeAt(j);
        if (k < 128 && SPECIAL[k] === 1) break;
        j++;
      }
      read(j - pos);
      text(s.slice(pos, j));
      pos = j;
      continue;
    }
    switch (c) {
      case "\n": {
        // Two spaces or more before it: a hard break. Its spaces are not text.
        const last = root.last;
        let hard = false;
        if (last?.kind === "text") {
          read(last.text.length);
          let e = last.text.length;
          while (e > 0 && last.text[e - 1] === " ") e--;
          hard = last.text.length - e >= 2;
          last.text = last.text.slice(0, e);
          if (last.text === "") unlink(last);
        }
        append(make(hard ? "hardbreak" : "softbreak"));
        pos++;
        while (s[pos] === " " || s[pos] === "\t") pos++;
        break;
      }
      case "\\": {
        const next = s[pos + 1];
        if (next === "\n") {
          append(make("hardbreak"));
          pos += 2;
          while (s[pos] === " " || s[pos] === "\t") pos++;
        } else if (isAsciiPunctuation(next)) {
          text(next);
          pos += 2;
        } else {
          text("\\");
          pos++;
        }
        break;
      }
      case "`": {
        runs ??= new Runs(s);
        let j = pos;
        while (s[j] === "`") j++;
        const length = j - pos;
        read(length);
        const end = runs.closer(length, j - 1);
        if (end < 0) {
          text("`".repeat(length));
          pos = j;
          break;
        }
        read((end - pos - length) * 3);
        let content = s.slice(pos + length, end).replaceAll("\n", " ");
        if (content.length >= 2 && content[0] === " " && content[content.length - 1] === " " && content.trim() !== "") content = content.slice(1, -1);
        append(make("code", content));
        // (The runs between are inside it: the next run read is the closer's next.)
        pos = end + length;
        break;
      }
      case "*":
      case "_":
      case "~": {
        let j = pos;
        while (s[j] === c) j++;
        const length = j - pos;
        read(length + 4);
        const node = make("run");
        node.char = c;
        node.count = length;
        append(node);
        const prev = before(s, pos);
        const next = at(s, j);
        const leftFlanking = !isWhitespace(next) && (!isPunctuation(next) || isWhitespace(prev) || isPunctuation(prev));
        const rightFlanking = !isWhitespace(prev) && (!isPunctuation(prev) || isWhitespace(next) || isPunctuation(next));
        let canOpen = leftFlanking;
        let canClose = rightFlanking;
        if (c === "_") {
          canOpen = leftFlanking && (!rightFlanking || isPunctuation(prev));
          canClose = rightFlanking && (!leftFlanking || isPunctuation(next));
        }
        if (c === "~" && length > 2) canOpen = canClose = false;
        if (canOpen || canClose) {
          keep(m, NODE);
          const d: Delimiter = { node, char: c, length, canOpen, canClose, prev: delimiters, next: null };
          if (delimiters) delimiters.next = d;
          delimiters = d;
        }
        pos = j;
        break;
      }
      case "!": {
        if (s[pos + 1] === "[") {
          const node = make("bracket", "![");
          append(node);
          if (brackets.length > 0) brackets[brackets.length - 1].bracketAfter = true;
          keep(m, NODE);
          brackets.push({ node, image: true, index: pos + 2, delimiters, bracketAfter: false });
          pos += 2;
        } else {
          text("!");
          pos++;
        }
        break;
      }
      case "[": {
        const node = make("bracket", "[");
        append(node);
        if (brackets.length > 0) brackets[brackets.length - 1].bracketAfter = true;
        keep(m, NODE);
        brackets.push({ node, image: false, index: pos + 1, delimiters, bracketAfter: false });
        pos++;
        break;
      }
      case "]":
        pos = closeBracket(pos);
        if (brackets.length < linksBelow) linksBelow = brackets.length;
        break;
      case "<": {
        // An autolink: `<scheme:…>` or `<a@b.c>`, read to the next `>`, `<` or space.
        let j = pos + 1;
        while (j < s.length && s[j] !== ">" && s[j] !== "<" && s[j] > " ") j++;
        read(j - pos);
        const inner = s.slice(pos + 1, j);
        if (s[j] === ">" && /^[A-Za-z][A-Za-z0-9+.-]{1,31}:/.test(inner)) {
          const link = make("link");
          link.destination = inner;
          link.first = link.last = make("text", inner);
          append(link);
          pos = j + 1;
        } else if (s[j] === ">" && isEmail(inner)) {
          const link = make("link");
          link.destination = `mailto:${inner}`;
          link.first = link.last = make("text", inner);
          append(link);
          pos = j + 1;
        } else {
          text("<");
          pos++;
        }
        break;
      }
      case "&": {
        read(34);
        const ref = reference(s, pos);
        if (ref) {
          text(ref.text);
          pos = ref.end;
        } else {
          text("&");
          pos++;
        }
        break;
      }
      default:
        text(c);
        pos++;
    }
  }
  processEmphasis(null);
  const inlines = finish(root, m);
  return { inlines, kept: m.kept - keptBefore };
}

/**
 * The parsed list as Inlines, iteratively (a tree can nest as deep as its
 * text is long): runs left over become their characters, texts side by side
 * become one, and bare `https://` links are found in text outside links.
 */
function finish(root: N, m: Meter): Inline[] {
  const top: Inline[] = [];
  const stack: { from: N | null; into: Inline[] }[] = [{ from: root.first, into: top }];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    const n = frame.from;
    if (!n) {
      stack.pop();
      continue;
    }
    frame.from = n.next;
    m.read += 1;
    keep(m, NODE / 2);
    const into = frame.into;
    const push = (t: string) => {
      if (t === "") return;
      const last = into[into.length - 1];
      if (last?.kind === "text") last.text += t;
      else into.push({ kind: "text", text: t });
    };
    switch (n.kind) {
      case "text":
        push(n.text);
        break;
      case "run":
        push(n.char.repeat(n.count));
        break;
      case "bracket":
        push(n.text);
        break;
      case "code":
        into.push({ kind: "code", text: n.text });
        break;
      case "softbreak":
      case "hardbreak":
        into.push({ kind: n.kind });
        break;
      case "em":
      case "strong":
      case "del": {
        const children: Inline[] = [];
        into.push({ kind: n.kind, children });
        stack.push({ from: n.first, into: children });
        break;
      }
      case "link":
      case "image": {
        const children: Inline[] = [];
        into.push({ kind: n.kind, destination: n.destination, title: n.title, children });
        stack.push({ from: n.first, into: children });
        break;
      }
    }
  }
  // Bare links, in whole texts (a run that was no delimiter has joined its neighbours), outside links.
  const lists: Inline[][] = [top];
  while (lists.length > 0) {
    const list = lists.pop()!;
    if (!list.some((n) => n.kind === "text" && n.text.includes("https://"))) {
      for (const n of list) if (n.kind === "em" || n.kind === "strong" || n.kind === "del") lists.push(n.children);
      continue;
    }
    const rebuilt: Inline[] = [];
    for (const n of list) {
      if (n.kind === "text") for (const piece of autolinked(n.text, m)) rebuilt.push(typeof piece === "string" ? { kind: "text", text: piece } : piece);
      else {
        rebuilt.push(n);
        if (n.kind === "em" || n.kind === "strong" || n.kind === "del") lists.push(n.children);
      }
    }
    // (One at a time: a list can hold more than a call can take as arguments.)
    list.length = 0;
    for (const n of rebuilt) list.push(n);
  }
  return top;
}

/**
 * Bare `https://` links in a text (GFM's extended autolinks, https only):
 * after the start, a space, or one of `*_~(`; a domain with a dot; up to a
 * space or `<`; less trailing punctuation and an unmatched `)`.
 */
function autolinked(t: string, m: Meter): (string | Inline)[] {
  m.read += t.length * 2;
  if (!t.includes("https://")) return [t];
  const out: (string | Inline)[] = [];
  let from = 0;
  let search = 0;
  for (;;) {
    const i = t.indexOf("https://", search);
    if (i < 0) break;
    const prev = i === 0 ? "" : t[i - 1];
    if (!(prev === "" || prev === " " || prev === "\t" || prev === "\n" || "*_~(".includes(prev))) {
      search = i + 8;
      continue;
    }
    let j = i + 8;
    const host = j;
    while (j < t.length && /^[A-Za-z0-9_.-]$/.test(t[j])) j++;
    const domain = t.slice(host, j);
    if (!domain.includes(".") || domain.startsWith(".") || domain.endsWith(".")) {
      search = j;
      continue;
    }
    while (j < t.length && t[j] > " " && t[j] !== "<") j++;
    let end = j;
    let open = 0;
    let shut = 0;
    for (let k = i; k < end; k++) {
      if (t[k] === "(") open++;
      else if (t[k] === ")") shut++;
    }
    // Trailing punctuation is the sentence's; a `)` is the link's only when it closes one of its own.
    for (;;) {
      const last = t[end - 1];
      if ("?!.,:*_~'\"".includes(last)) end--;
      else if (last === ")") {
        if (shut > open) {
          end--;
          shut--;
        } else break;
      } else if (last === ";") {
        let k = end - 2;
        while (k > host && /^[A-Za-z0-9]$/.test(t[k])) k--;
        if (t[k] === "&") end = k;
        else break;
      } else break;
    }
    if (end - host < 1) {
      search = j;
      continue;
    }
    if (i > from) out.push(t.slice(from, i));
    const url = t.slice(i, end);
    out.push({ kind: "link", destination: url, title: null, children: [{ kind: "text", text: url }] });
    from = end;
    search = end;
  }
  if (from < t.length) out.push(t.slice(from));
  return out;
}

/** A tree's words, without its markup (a picture's alt, a heading's id), iteratively. */
export function plainText(inlines: Inline[], m?: Meter): string {
  let out = "";
  const stack: Inline[][] = [inlines];
  const index: number[] = [0];
  while (stack.length > 0) {
    const list = stack[stack.length - 1];
    const i = index[index.length - 1];
    if (i >= list.length) {
      stack.pop();
      index.pop();
      continue;
    }
    index[index.length - 1] = i + 1;
    const n = list[i];
    if (m) m.read += 1 + (n.kind === "text" || n.kind === "code" ? n.text.length : 0);
    if (n.kind === "text" || n.kind === "code") out += n.text;
    else if (n.kind === "softbreak" || n.kind === "hardbreak") out += "\n";
    else {
      stack.push(n.children);
      index.push(0);
    }
  }
  return out;
}

export { NESTING };
