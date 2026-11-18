// Code as HTML with the highlighter's classes (no inline style: the CSP
// allows none), every character escaped by the serializer. A file can be
// made to nest the highlighter's tree deeper than the serializer can walk
// (a 3 KB `"\`${"` run): past NESTING, or if anything throws, the text is
// shown plain, and the caller is told, so a reviewer can say so.
//
// What highlighting costs can't be told from outside the grammars: one
// long line runs some of them into the square of its length or worse (a
// 32 KB line of "_" takes TypeScript's 3 s, of "[a](" markdown's 160 s),
// and some grow with the whole text whatever its lines (C's and shell's
// past 2 s at 32 KB, Rust's raw strings 0.6 s). Measured on every grammar
// shipped, each against its worst of 43 openers, so: a text is highlighted
// only when no line passes HIGHLIGHT.line and the whole is within its
// language's cap, and a page or document highlights within a budget of
// bytes; everything else is shown as plain text (with nothing said: being
// long isn't suspicious). Within these, the worst measured is about 60 ms
// a call.
import type { Root, RootContent } from "hast";
import { toHtml } from "hast-util-to-html";
import { common, createLowlight } from "lowlight";

const lowlight = createLowlight(common);

/** Deeper than any real file's highlighting nests (a few levels). */
export const NESTING = 64;

/** How long a line, and how much text, a highlighting may take on. */
export const HIGHLIGHT = { line: 400, bytes: 4 * 1024 } as const;

/** Grammars whose worst grows faster still (markdown's is about cubic): a smaller cap. */
const SMALLER: Record<string, number> = { markdown: 1024, md: 1024, mkdown: 1024, mkd: 1024 };

/** What one page (a submission's changes) or one document may highlight, together, in bytes. */
export class HighlightBudget {
  bytes: number;
  constructor(bytes: number) {
    this.bytes = bytes;
  }
}

/** Whether a text may be highlighted in a language at all: within its cap, and no line past HIGHLIGHT.line. */
export function highlightable(text: string, language: string): boolean {
  if (text.length > (SMALLER[language] ?? HIGHLIGHT.bytes)) return false;
  let start = 0;
  for (;;) {
    const end = text.indexOf("\n", start);
    if ((end === -1 ? text.length : end) - start > HIGHLIGHT.line) return false;
    if (end === -1) return true;
    start = end + 1;
  }
}

export interface Highlighted {
  html: string;
  /** It couldn't be highlighted (nested past NESTING, or the highlighter failed): shown plain. */
  plain: boolean;
}

const asText = (text: string) => toHtml({ type: "text", value: text });

/** Whether a highlighter's tree nests past NESTING (the serializer would fail on it). */
export const nestsTooDeep = (tree: Root): boolean => depth(tree) > NESTING;

/** How deep a tree nests, walked without recursion. */
function depth(tree: Root): number {
  let most = 0;
  const stack: [Root | RootContent, number][] = [[tree, 0]];
  while (stack.length) {
    const [node, level] = stack.pop()!;
    if (level > most) most = level;
    if (most > NESTING) return most;
    if ("children" in node) for (const child of node.children) stack.push([child, level + 1]);
  }
  return most;
}

/**
 * One file's text (or a line of it), highlighted when its language is known
 * and it is within the bounds (and the `budget`, when given, which it
 * spends); plain otherwise. Plain, and said so, when the highlighter fails on it.
 */
export function highlighted(text: string, language: string | null, budget?: HighlightBudget): Highlighted {
  if (!language || !lowlight.registered(language) || !highlightable(text, language)) return { html: asText(text), plain: false };
  if (budget) {
    if (text.length > budget.bytes) return { html: asText(text), plain: false };
    budget.bytes -= text.length;
  }
  try {
    const tree = lowlight.highlight(language, text);
    if (depth(tree) > NESTING) return { html: asText(text), plain: true };
    return { html: toHtml(tree), plain: false };
  } catch {
    return { html: asText(text), plain: true };
  }
}

/** The HTML alone. */
export const highlight = (text: string, language: string | null): string => highlighted(text, language).html;
