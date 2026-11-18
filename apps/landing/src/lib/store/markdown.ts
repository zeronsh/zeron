// A plugin's README (and a version's notes) as HTML, on the server. What is
// shown must be what was reviewed, and nothing in it may run.
//
// The store's markdown is a stated subset (the publishing guide says it to
// authors; it is what Zeron's own README view draws), read by a renderer of
// the store's own (md/): linear in the text by construction, and writing
// HTML only from a fixed list of tags and attributes, so there is nothing
// to sanitize and no estimate of cost to trust. Its only bound is on what
// it writes (BOUNDS.html); past it, the document is shown as its text.
//
//   markdown ─ md/blocks.ts   lines to blocks
//            ─ md/inlines.ts  a block's text to inlines
//            ─ md/html.ts     HTML: pictures in the package through the file
//                             route, any other a link to it; links https and
//                             mail only; ids prefixed; code highlighted
//                             (classes only) within highlight.ts's bounds

import { blocksOf } from "./md/blocks.ts";
import { asText, TAGS, toHtml, TooLarge, type RenderOptions } from "./md/html.ts";
import { inlinesOf, plainText } from "./md/inlines.ts";

export { target, type RenderOptions, type Target } from "./md/html.ts";

/** Bump when the HTML this makes changes, so kept renderings are made again. */
export const RENDERER = 8;

/** What a rendering may write: past it, the document is shown as its text. */
export const BOUNDS = { html: 2 * 1024 * 1024 } as const;

/** Why a document was shown as its text: what it would write is past BOUNDS.html, or the renderer failed. */
export type Cut = "size" | "error";

export interface Rendered {
  html: string;
  cut: Cut | null;
}

function fingerprint(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193) >>> 0;
  return h.toString(36);
}

/**
 * What a kept rendering was made by: the renderer's version (RENDERER,
 * bumped by hand), the tags it may write and its bounds (fingerprinted, so
 * a change to either is a new pipeline without anyone remembering to bump).
 */
export const PIPELINE = `${RENDERER}.${fingerprint(JSON.stringify(TAGS))}.${fingerprint(JSON.stringify(BOUNDS))}`;

/** Markdown in, safe HTML out; past BOUNDS.html (or if the renderer fails), its text, and `cut` says why. */
export function render(markdown: string, options: RenderOptions): Rendered {
  try {
    return { html: toHtml(markdown, options, BOUNDS.html).html, cut: null };
  } catch (error) {
    return { html: asText(markdown), cut: error instanceof TooLarge ? "size" : "error" };
  }
}

/** Markdown in, safe HTML out (render's HTML). */
export function renderMarkdown(markdown: string, options: RenderOptions): string {
  return render(markdown, options).html;
}

/**
 * A version's notes in one line, for its row (the first block that says
 * something, without its markdown): "- Fixed **the** timer" → "Fixed the timer".
 */
export function firstLine(markdown: string | null | undefined): string {
  try {
    return firstLineOf(markdown ?? "");
  } catch (error) {
    if (!(error instanceof TooLarge)) throw error;
    // (Notes too large to read stand as their first line, as written.)
    return (markdown ?? "").split("\n", 64).find((l) => l.trim() !== "")?.trim() ?? "";
  }
}

function firstLineOf(markdown: string): string {
  const { blocks, definitions } = blocksOf(markdown);
  // (Depth first: a list's first item, a quote's first paragraph.)
  const stack = [...blocks].reverse();
  while (stack.length > 0) {
    const block = stack.pop()!;
    let text: string | null = null;
    if (block.kind === "paragraph" || block.kind === "heading") text = plainText(inlinesOf(block.text, definitions).inlines);
    else if (block.kind === "html" || block.kind === "code") text = block.text;
    else if (block.kind === "quote") for (let i = block.children.length - 1; i >= 0; i--) stack.push(block.children[i]);
    else if (block.kind === "list") for (let i = block.items.length - 1; i >= 0; i--) for (let j = block.items[i].children.length - 1; j >= 0; j--) stack.push(block.items[i].children[j]);
    else if (block.kind === "table") text = block.head.join(" ");
    const line = text?.split("\n").find((l) => l.trim() !== "")?.trim();
    if (line) return line;
  }
  return "";
}
