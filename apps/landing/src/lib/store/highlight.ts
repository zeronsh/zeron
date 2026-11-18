// Code as HTML with the highlighter's classes (no inline style: the CSP
// allows none), every character escaped by the serializer. A file can be
// made to nest the highlighter's tree deeper than the serializer can walk
// (a 3 KB `"\`${"` run): past NESTING, or if anything throws, the text is
// shown plain, and the caller is told, so a reviewer can say so.
import type { Root, RootContent } from "hast";
import { toHtml } from "hast-util-to-html";
import { common, createLowlight } from "lowlight";

const lowlight = createLowlight(common);

/** Deeper than any real file's highlighting nests (a few levels). */
export const NESTING = 64;

/** Text past this many characters is shown plain: highlighting is a nicety. */
const LIMIT = 200_000;

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

/** One file's text (or a line of it), highlighted when its language is known; plain, and said so, when it can't be. */
export function highlighted(text: string, language: string | null): Highlighted {
  if (!language || !lowlight.registered(language) || text.length > LIMIT) return { html: asText(text), plain: false };
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
