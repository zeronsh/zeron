/**
 * A small block-level markdown parser for the files preview — the web peer
 * of the desktop's pulldown-cmark markdown stack, scoped to the blocks that
 * carry workspace documents: headings (ATX + setext), fenced code, lists
 * (with task markers carrying their source offset), quotes, pipe tables,
 * rules, and paragraphs, with inline code, emphasis, strikethrough, links,
 * and images. Not CommonMark-complete; the desktop's rendered preview
 * remains the reference.
 */

/** `MAX_MARKDOWN_BYTES` (markdown_preview.rs) — the client-side parse clip. */
export const MAX_MARKDOWN_BYTES = 2 * 1024 * 1024;

export interface TaskMarker {
  /** Offset of the `[` in the parsed source (a single edit toggles here). */
  readonly offset: number;
  readonly checked: boolean;
}

export interface MdListItem {
  readonly blocks: readonly MdBlock[];
  /** GFM task state with the source offset the toggle writes at. */
  readonly task: TaskMarker | null;
}

export type MdBlock =
  | { readonly kind: "heading"; readonly level: number; readonly inlines: readonly MdInline[] }
  | { readonly kind: "paragraph"; readonly inlines: readonly MdInline[] }
  | { readonly kind: "code"; readonly language: string | null; readonly text: string }
  | { readonly kind: "quote"; readonly blocks: readonly MdBlock[] }
  | { readonly kind: "list"; readonly ordered: boolean; readonly items: readonly MdListItem[] }
  | { readonly kind: "table"; readonly header: readonly (readonly MdInline[])[], readonly rows: readonly (readonly (readonly MdInline[])[])[] }
  | { readonly kind: "rule" };

export type MdInline =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "code"; readonly text: string }
  | { readonly kind: "bold"; readonly children: readonly MdInline[] }
  | { readonly kind: "italic"; readonly children: readonly MdInline[] }
  | { readonly kind: "strike"; readonly children: readonly MdInline[] }
  | { readonly kind: "link"; readonly href: string; readonly children: readonly MdInline[] }
  | { readonly kind: "image"; readonly src: string; readonly alt: string };

export function parseMarkdown(source: string): MdBlock[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const starts: number[] = [];
  let at = 0;
  for (const line of lines) {
    starts.push(at);
    at += line.length + 1;
  }
  return parseBlocks(lines, starts);
}

/**
 * The 2 MiB client-side clip (`markdown_preview.rs` set_source): a live
 * buffer larger than `MAX_MARKDOWN_BYTES` parses only its first
 * `MAX_MARKDOWN_BYTES` bytes, cut at the nearest UTF-8 boundary (a code
 * point, never inside a surrogate pair).
 */
export function clipMarkdownBytes(source: string): { readonly text: string; readonly truncated: boolean } {
  let bytes = 0;
  for (let index = 0; index < source.length; ) {
    const codePoint = source.codePointAt(index)!;
    const width = codePoint > 0xffff ? 2 : 1;
    const encoded = codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes + encoded > MAX_MARKDOWN_BYTES) {
      return { text: source.slice(0, index), truncated: true };
    }
    bytes += encoded;
    index += width;
  }
  return { text: source, truncated: false };
}

function parseBlocks(lines: readonly string[], starts: readonly number[]): MdBlock[] {
  const blocks: MdBlock[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index]!;
    if (line.trim().length === 0) {
      index += 1;
      continue;
    }

    const fence = line.match(/^\s*```(\S*)\s*$/) ?? line.match(/^\s*~~~(\S*)\s*$/);
    if (fence !== null) {
      const marker = line.trimStart().slice(0, 3);
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !lines[index]!.trimStart().startsWith(marker)) {
        body.push(lines[index]!);
        index += 1;
      }
      index += 1; // the closing fence (or end of input)
      blocks.push({ kind: "code", language: fence[1] !== undefined && fence[1].length > 0 ? fence[1] : null, text: body.join("\n") });
      continue;
    }

    const heading = line.match(/^\s{0,3}(#{1,6})\s+(.*)$/);
    if (heading !== null) {
      blocks.push({ kind: "heading", level: heading[1]!.length, inlines: parseInlines(heading[2]!.replace(/\s+#+\s*$/, "")) });
      index += 1;
      continue;
    }

    if (/^\s{0,3}(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      blocks.push({ kind: "rule" });
      index += 1;
      continue;
    }

    if (/^\s{0,3}>/.test(line)) {
      const quote: string[] = [];
      while (index < lines.length && /^\s{0,3}>/.test(lines[index]!)) {
        quote.push(lines[index]!.replace(/^\s{0,3}>\s?/, ""));
        index += 1;
      }
      blocks.push({ kind: "quote", blocks: parseBlocks(quote, quoteStarts(starts, index - quote.length, quote)) });
      continue;
    }

    if (isListItem(line)) {
      const { blocks: list, next } = parseList(lines, starts, index);
      blocks.push(list);
      index = next;
      continue;
    }

    if (isTableStart(lines, index)) {
      const header = splitTableRow(lines[index]!);
      index += 2; // header + delimiter
      const rows: string[][] = [];
      while (index < lines.length && lines[index]!.includes("|") && lines[index]!.trim().length > 0) {
        rows.push(splitTableRow(lines[index]!));
        index += 1;
      }
      blocks.push({
        kind: "table",
        header: header.map((cell) => parseInlines(cell)),
        rows: rows.map((row) => row.map((cell) => parseInlines(cell))),
      });
      continue;
    }

    // Paragraph: consume until a blank line or another block opener.
    const body: string[] = [];
    while (index < lines.length) {
      const current = lines[index]!;
      if (current.trim().length === 0) {
        break;
      }
      if (body.length > 0 && /^\s{0,3}(#{1,6}\s|>|```|~~~)/.test(current)) {
        break;
      }
      if (body.length > 0 && isListItem(current)) {
        break;
      }
      // A setext underline ends the paragraph (checked after the loop).
      if (body.length > 0 && /^\s{0,3}(=+|-+|\*{3,}|_{3,})\s*$/.test(current)) {
        break;
      }
      body.push(current);
      index += 1;
    }
    // Setext headings: a paragraph of one line underlined with === or ---.
    if (index < lines.length && body.length > 0) {
      const underline = lines[index]!;
      if (/^\s{0,3}=+\s*$/.test(underline)) {
        blocks.push({ kind: "heading", level: 1, inlines: parseInlines(body.join(" ")) });
        index += 1;
        continue;
      }
      if (/^\s{0,3}-+\s*$/.test(underline)) {
        blocks.push({ kind: "heading", level: 2, inlines: parseInlines(body.join(" ")) });
        index += 1;
        continue;
      }
    }
    if (body.length > 0) {
      blocks.push({ kind: "paragraph", inlines: parseInlines(body.join("\n")) });
    }
  }
  return blocks;
}

/** The source offsets of a re-sliced child range (quotes re-number lines). */
function quoteStarts(starts: readonly number[], first: number, body: readonly string[]): number[] {
  const out: number[] = [];
  let cursor = starts[first] ?? 0;
  for (const line of body) {
    out.push(cursor);
    cursor += line.length + 1;
  }
  return out;
}

function isListItem(line: string): boolean {
  return /^(\s*)([-*+]|\d{1,9}[.)])\s+/.test(line);
}

function listMarker(line: string): { indent: number; ordered: boolean; rest: string } | null {
  const match = line.match(/^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/);
  if (match === null) {
    return null;
  }
  const indent = match[1]!.replace(/\t/g, "    ").length;
  return { indent, ordered: /\d/.test(match[2]!), rest: match[3]! };
}

/** GFM task marker at the head of a list item's first line, if any. */
function taskMarker(rest: string, lineStart: number, line: string): TaskMarker | null {
    const match = rest.match(/^\[([ xX])\](\s+|$)/);
    if (match === null) {
      return null;
    }
    // The `[` sits at `line.length - rest.length` into the line.
    const offset = lineStart + (line.length - rest.length);
    return { offset, checked: match[1] !== " " };
  }

/** The item content with the task marker stripped (GFM renders it as the checkbox). */
function stripTaskMarker(rest: string): string {
    return rest.replace(/^\[([ xX])\]\s+/, "");
  }

  function parseList(lines: readonly string[], starts: readonly number[], start: number): { blocks: MdBlock; next: number } {
    const first = listMarker(lines[start]!)!;
    const ordered = first.ordered;
    const items: MdListItem[] = [];
    let index = start;
    let current: string[] = [];
    let currentTask: TaskMarker | null = null;
    let currentStarts: number[] = [];
    let currentIndent = first.indent;

    const flush = (): void => {
      if (current.length > 0) {
        items.push({ blocks: parseBlocks(current, currentStarts), task: currentTask });
        current = [];
        currentStarts = [];
        currentTask = null;
      }
    };

    while (index < lines.length) {
      const line = lines[index]!;
      if (line.trim().length === 0) {
        break;
      }
      const marker = listMarker(line);
      if (marker !== null && marker.indent <= first.indent) {
        if (marker.ordered !== ordered || marker.indent < first.indent) {
          break;
        }
        flush();
        const task = taskMarker(marker.rest, starts[index]!, line);
        const content = task === null ? marker.rest : stripTaskMarker(marker.rest);
        current = [content];
        currentStarts = [starts[index]! + (line.length - content.length)];
        currentTask = task;
        currentIndent = marker.indent;
        index += 1;
        continue;
      }
    // Continuation: an indented line belonging to the current item.
    const indent = line.match(/^(\s*)/)![1]!.replace(/\t/g, "    ").length;
    if (indent > currentIndent) {
      const slice = line.slice(Math.min(line.length, currentIndent + 2));
      current.push(slice);
      currentStarts.push(starts[index]! + (line.length - slice.length));
      index += 1;
      continue;
    }
    break;
  }
  flush();
  return { blocks: { kind: "list", ordered, items }, next: index };
}

function isTableStart(lines: readonly string[], index: number): boolean {
  if (index + 1 >= lines.length) {
    return false;
  }
  const header = lines[index]!;
  const delimiter = lines[index + 1]!;
  if (!header.includes("|") || header.trim().length === 0) {
    return false;
  }
  return /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(delimiter) && delimiter.includes("-");
}

function splitTableRow(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) {
    text = text.slice(1);
  }
  if (text.endsWith("|")) {
    text = text.slice(0, -1);
  }
  return text.split("|").map((cell) => cell.trim());
}

/**
 * Inline parsing: code spans bind first, then links/images, then emphasis.
 * Input is one logical block's text (may contain soft line breaks).
 */
export function parseInlines(text: string): MdInline[] {
  return parseInlineRange(text);
}

function parseInlineRange(text: string): MdInline[] {
  const out: MdInline[] = [];
  let index = 0;
  const pushText = (value: string): void => {
    if (value.length === 0) {
      return;
    }
    const last = out[out.length - 1];
    if (last !== undefined && last.kind === "text") {
      out[out.length - 1] = { kind: "text", text: last.text + value };
    } else {
      out.push({ kind: "text", text: value });
    }
  };

  while (index < text.length) {
    const rest = text.slice(index);
    // Inline code span.
    const code = rest.match(/^`([^`]+)`/);
    if (code !== null) {
      out.push({ kind: "code", text: code[1]! });
      index += code[0].length;
      continue;
    }
    // Image.
    const image = rest.match(/^!\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/);
    if (image !== null) {
      out.push({ kind: "image", src: image[2]!, alt: image[1]! });
      index += image[0].length;
      continue;
    }
    // Link.
    const link = rest.match(/^\[([^\]]+)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/);
    if (link !== null) {
      out.push({ kind: "link", href: link[2]!, children: parseInlineRange(link[1]!) });
      index += link[0].length;
      continue;
    }
    // Underscore emphasis never opens inside a word (CommonMark flanking).
    const intraword = index > 0 && /[A-Za-z0-9]/.test(text[index - 1]!);
    // Strong / strike / em, in binding order.
    const strong = rest.match(/^\*\*([^*]+)\*\*/) ?? (!intraword ? rest.match(/^__([^_]+)__/) : null);
    if (strong !== null && strong !== undefined) {
      out.push({ kind: "bold", children: parseInlineRange(strong[1]!) });
      index += strong[0].length;
      continue;
    }
    const strike = rest.match(/^~~([^~]+)~~/);
    if (strike !== null) {
      out.push({ kind: "strike", children: parseInlineRange(strike[1]!) });
      index += strike[0].length;
      continue;
    }
    const em = rest.match(/^\*([^*\s][^*]*)\*/) ?? (!intraword ? rest.match(/^_([^_\s][^_]*)_/) : null);
    if (em !== null && em !== undefined) {
      out.push({ kind: "italic", children: parseInlineRange(em[1]!) });
      index += em[0].length;
      continue;
    }
    // Plain text up to the next possible opener.
    const next = rest.slice(1).search(/[`*!_~[]/);
    if (next === -1) {
      pushText(rest);
      break;
    }
    pushText(rest.slice(0, next + 1));
    index += next + 1;
  }
  return out;
}

/**
 * Link policy for the preview (desktop `preview_link_outcome`, simplified):
 * absolute http(s) and mailto links are external; anything else with a
 * scheme is dropped; scheme-less targets are workspace-relative paths the
 * viewer can open.
 */
export type MarkdownLinkTarget = { readonly kind: "external"; readonly href: string } | { readonly kind: "workspace"; readonly path: string } | { readonly kind: "text" };

export function markdownLinkTarget(href: string): MarkdownLinkTarget {
  if (/^https?:\/\//i.test(href) || /^mailto:/i.test(href)) {
    return { kind: "external", href };
  }
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
    return { kind: "text" };
  }
  const path = href.split("#")[0] ?? "";
  if (path.length === 0) {
    // A bare `#anchor` still resolves — against the current document.
    return href.startsWith("#") ? { kind: "workspace", path: href } : { kind: "text" };
  }
  return { kind: "workspace", path };
}

/** A resolved workspace-relative markdown target: path plus decoded anchor. */
export interface ResolvedMarkdownTarget {
  readonly path: string;
  readonly anchor: string | null;
}

/**
 * `relative_target` (markdown_preview.rs:66-112), ported: strips a leading
 * `zeron-file:` prefix (recursively), rejects absolute (`/`), scheme-
 * qualified (`:`), or backslash targets, percent-decodes the path and the
 * `#anchor`, rejects decoded paths containing `\\`, `:`, NUL, or a leading
 * `/`, resolves `.`/`..` against the DOCUMENT'S DIRECTORY, and returns null
 * when resolution walks above the root or empties out. An empty target
 * (bare `#anchor`) resolves to the current document.
 */
export function relativeTarget(
  document: string,
  target: string,
): ResolvedMarkdownTarget | null {
  if (target.startsWith("zeron-file:")) {
    return relativeTarget("", target.slice("zeron-file:".length));
  }
  if (target.startsWith("/") || target.includes(":") || target.includes("\\")) {
    return null;
  }
  const hash = target.indexOf("#");
  const rawPath = hash < 0 ? target : target.slice(0, hash);
  const rawAnchor = hash < 0 ? null : target.slice(hash + 1);
  const path = percentDecode(rawPath);
  if (path === null) {
    return null;
  }
  // Desktop: a failed ANCHOR decode yields no anchor, not a rejected target.
  const anchor = rawAnchor === null ? null : percentDecode(rawAnchor);
  if (path.includes("\\") || path.includes(":") || path.includes("\0") || path.startsWith("/")) {
    return null;
  }
  if (path.length === 0) {
    return { path: document, anchor };
  }
  const parts = document.split("/");
  parts.pop();
  for (const part of path.split("/")) {
    if (part === "" || part === ".") {
      continue;
    }
    if (part === "..") {
      if (parts.pop() === undefined) {
        return null;
      }
      continue;
    }
    parts.push(part);
  }
  if (parts.length === 0) {
    return null;
  }
  return { path: parts.join("/"), anchor };
}

function percentDecode(value: string): string | null {
  try {
    // decodeURIComponent rejects stray `%` and malformed escapes — the
    // desktop's hand-rolled decoder returns None for the same shapes.
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

/**
 * `heading_anchor`/`slug` (markdown_preview.rs): lower-case, spaces → `-`;
 * duplicates get a numeric suffix on collision. The map is computed once
 * per parse so anchors stay stable across re-renders.
 */
export function buildHeadingAnchors(blocks: readonly MdBlock[]): ReadonlyMap<MdBlock, string> {
  const anchors = new Map<MdBlock, string>();
  const seen = new Map<string, number>();
  const walk = (list: readonly MdBlock[]): void => {
    for (const block of list) {
      if (block.kind === "heading") {
        const base = slugify(inlinesText(block.inlines));
        const count = seen.get(base) ?? 0;
        seen.set(base, count + 1);
        anchors.set(block, count === 0 ? base : `${base}-${count}`);
      } else if (block.kind === "quote") {
        walk(block.blocks);
      } else if (block.kind === "list") {
        for (const item of block.items) {
          walk(item.blocks);
        }
      }
    }
  };
  walk(blocks);
  return anchors;
}

/** The slug rule: lower-case, spaces → `-`. */
export function slugify(text: string): string {
  return text.trim().toLowerCase().replace(/\s+/g, "-");
}

function inlinesText(inlines: readonly MdInline[]): string {
  return inlines
    .map((inline) => {
      switch (inline.kind) {
        case "text":
        case "code":
          return inline.text;
        case "image":
          return inline.alt;
        default:
          return inlinesText(inline.children);
      }
    })
    .join("");
}
