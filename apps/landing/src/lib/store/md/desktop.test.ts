// The two stores read a README alike: the site's parse of the desktop's
// own fixture READMEs (apps/zgx/ui/store_stub.luau's, as files here, and
// one README of the whole subset) has the same outline as the desktop's
// parser (crates/markdown) makes of them: headings, paragraphs' words,
// lists and their tasks, quotes, code and its language, tables, rules,
// links and pictures.
//
// `fixtures/desktop.outline` is the desktop's, written by
// `fixtures/outline.rs` (a few lines over crates/markdown's parse_full; see
// its head for how to run it). Remake it when crates/markdown changes.

import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { blocksOf, type Block, type Definition } from "./blocks.ts";
import { inlinesOf, type Inline } from "./inlines.ts";

const FIXTURES = ["focus-timer.md", "screenshot-tray.md", "short.md", "notes.md", "extremes.md", "subset.md"];
const read = (name: string) => readFileSync(new URL(`./fixtures/${name}`, import.meta.url), "utf8");

const squash = (s: string) => s.split(/\s+/).filter(Boolean).join(" ");
/** As Rust's `{:?}` writes a string (enough for these fixtures). */
const quoted = (s: string) => JSON.stringify(s);
/** As Rust's `{:?}` writes a list of strings. */
const listed = (list: string[]) => `[${list.map(quoted).join(", ")}]`;

/** A text's words (pictures aside), with a line for each link and picture in it. */
function inline(list: Inline[], out: string[], pad: string, link: string | null = null): string {
  let text = "";
  for (const n of list) {
    switch (n.kind) {
      case "text":
      case "code":
        text += n.text;
        break;
      case "softbreak":
      case "hardbreak":
        text += " ";
        break;
      case "em":
      case "strong":
      case "del":
        text += inline(n.children, out, pad, link);
        break;
      case "image": {
        let alt = "";
        const sink: string[] = [];
        alt = inline(n.children, sink, pad);
        out.push(`${pad}  picture ${quoted(squash(alt))} <- ${n.destination}${link ? ` in ${link}` : ""}`);
        break;
      }
      case "link": {
        const destination = n.destination.replace(/^mailto:/, "");
        const words = inline(n.children, out, pad, destination);
        text += words;
        // (A badge, a link of a picture alone, is the picture with its link: the desktop holds it so.)
        if (squash(words) !== "" || !n.children.some((c) => c.kind === "image")) out.push(`${pad}  link ${quoted(squash(words))} -> ${destination}`);
        break;
      }
    }
  }
  return squash(text);
}

function blocks(list: Block[], d: Map<string, Definition>, out: string[], depth: number): void {
  const pad = "  ".repeat(depth);
  const words = (text: string, sub: string[]) => inline(inlinesOf(text, d).inlines, sub, pad);
  for (const b of list) {
    switch (b.kind) {
      case "paragraph":
      case "heading": {
        const sub: string[] = [];
        const t = words(b.text, sub);
        out.push(`${pad}${b.kind === "paragraph" ? "p" : `h${b.level}`} ${quoted(t)}`, ...sub);
        break;
      }
      case "html":
        out.push(`${pad}p ${quoted(squash(b.text))}`);
        break;
      case "code":
        out.push(`${pad}code[${b.info.split(/[ \t]/, 1)[0]}] ${quoted(b.text.replace(/\n$/, ""))}`);
        break;
      case "quote":
        out.push(`${pad}quote`);
        blocks(b.children, d, out, depth + 1);
        break;
      case "list":
        out.push(`${pad}list ${b.ordered ? `ordered from ${b.start}` : "bulleted"}`);
        for (const item of b.items) {
          out.push(`${pad}  item task=${item.task === null ? "none" : item.task ? "done" : "open"}`);
          blocks(item.children, d, out, depth + 2);
        }
        break;
      case "table": {
        const sub: string[] = [];
        out.push(`${pad}table ${listed(b.head.map((c) => words(c, sub)))} align ${listed(b.align.map((a) => a ?? "left"))}`);
        for (const row of b.rows) out.push(`${pad}  row ${listed(row.map((c) => words(c, sub)))}`);
        out.push(...sub);
        break;
      }
      case "rule":
        out.push(`${pad}rule`);
        break;
    }
  }
}

function outline(name: string): string[] {
  const parsed = blocksOf(read(name));
  const out = [`=== ${name}`];
  blocks(parsed.blocks, parsed.definitions, out, 0);
  return out;
}

describe("the site reads a README as the desktop does", () => {
  const desktop = read("desktop.outline").trimEnd().split("\n");
  test.each(FIXTURES)("%s", (name) => {
    const from = desktop.indexOf(`=== ${name}`);
    const to = desktop.findIndex((line, i) => i > from && line.startsWith("=== "));
    expect(from).toBeGreaterThanOrEqual(0);
    expect(outline(name)).toEqual(desktop.slice(from, to < 0 ? undefined : to));
  });
});
