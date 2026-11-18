import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { PREFIX, TAGS, toHtml, TooLarge, type RenderOptions } from "./html.ts";
import { referenceHtml } from "./reference.ts";

const options: RenderOptions = { file: (p) => `/plugins/a/files/1.0.0/${p}`, demote: true };
const MOST = 2 * 1024 * 1024;

interface Example {
  suite: string;
  n: number;
  markdown: string;
  html: string;
}

const { examples } = JSON.parse(readFileSync(new URL("./commonmark.examples.json", import.meta.url), "utf8")) as { examples: Example[] };

/**
 * The examples the store's subset answers otherwise, on purpose:
 * - a tag inside a paragraph is its characters (raw inline HTML, 613–631
 *   and 642–643, and those where a tag would have taken precedence over a
 *   code span, emphasis or a link). HTML blocks are recognised as CommonMark
 *   recognises them (the reference writer writes them through, so 149–191
 *   hold their structure); the store writes their characters;
 * - a bare `https://` is a link (GFM's extended autolinks, as GitHub and
 *   Zeron's own README view draw it; CommonMark alone leaves it text).
 */
const RAW_HTML = new Set([148, 168, 187, 201, 344, 475, 476, 477, 491, 494, 524, 536, 613, 614, 615, 616, 617, 623, 625, 626, 627, 628, 629, 630, 631, 642, 643]);
const BARE_LINKS = new Set([608, 611]);

/** As CommonMark's own spec tests compare: whitespace between tags, self-closing slashes and `&quot;` aside. */
const normal = (html: string) => html.replaceAll("&quot;", '"').replace(/\s*\/>/g, ">").replace(/>\s+</g, "><").replace(/\s+/g, " ").trim();

describe("the subset, against CommonMark's and GFM's examples", () => {
  const kept = examples.filter((e) => !(e.suite === "spec" && (RAW_HTML.has(e.n) || BARE_LINKS.has(e.n))));
  test("the examples the subset answers otherwise are only those it means to", () => {
    expect(examples).toHaveLength(666);
    expect(examples.length - kept.length).toBe(RAW_HTML.size + BARE_LINKS.size);
  });
  test.each(kept.map((e) => [`${e.suite} ${e.n}`, e] as const))("%s", (_, e) => {
    expect(normal(referenceHtml(e.markdown))).toBe(normal(e.html));
  });
});

/** Documents made of the characters markdown and HTML give meaning to, at random (seeded). */
function hostile(seed: number, length: number): string {
  const pieces = [
    "*", "_", "~", "[", "]", "(", ")", "`", "<", ">", "!", "\\", "&", "#", "\n", "\n\n", " ", "  ", "a", "|", "-", "\"", "'", ":",
    "<script>alert(1)</script>", "<img src=x onerror=alert(1)>", "javascript:alert(1)", "data:text/html,x", "https://x.y/", "//evil.example",
    "](", "](javascript:", "![", "[x]: ", "    ", "> ", "- ", "1. ", "```", "---", "&lt;", "&#0;", "\u0000", "\r", "\t", "on", "=",
  ];
  let x = seed;
  let out = "";
  while (out.length < length) {
    x = (Math.imul(x, 1103515245) + 12345) & 0x7fffffff;
    out += pieces[x % pieces.length];
  }
  return out;
}

/** Every tag in a rendering, and its attributes; anything else that looks like markup. */
function markup(html: string): { tags: { name: string; attributes: [string, string | null][] }[]; stray: number } {
  const tags: { name: string; attributes: [string, string | null][] }[] = [];
  const pattern = /<\/?([a-z0-9]+)((?: [a-z-]+(?:="[^"<>]*")?)*)>/g;
  let matched = 0;
  for (const m of html.matchAll(pattern)) {
    matched++;
    const attributes: [string, string | null][] = [...m[2].matchAll(/ ([a-z-]+)(?:="([^"]*)")?/g)].map((a) => [a[1], a[2] ?? null]);
    tags.push({ name: m[1], attributes });
  }
  return { tags, stray: (html.match(/</g)?.length ?? 0) - matched };
}

describe("what it writes", () => {
  test("only the allowed tags and attributes, every address its own, nothing else that is markup (500 hostile documents)", () => {
    for (let seed = 1; seed <= 500; seed++) {
      const md = hostile(seed, 2_000 + (seed % 7) * 1_000);
      const { html } = toHtml(md, options, MOST);
      const { tags, stray } = markup(html);
      expect(stray, md).toBe(0);
      for (const { name, attributes } of tags) {
        expect(TAGS[name], name).toBeDefined();
        for (const [attribute, value] of attributes) {
          expect(TAGS[name], `${name} ${attribute}`).toContain(attribute);
          if (attribute === "href") expect(value!, md).toMatch(/^(https:\/\/|mailto:|#user-content-)/);
          if (attribute === "src") expect(value!).toMatch(/^\/plugins\/a\/files\/1\.0\.0\//);
          if (attribute === "id") expect(value!.startsWith(PREFIX)).toBe(true);
        }
      }
      // (The words `<script>` and `onerror=` may be there, as text, escaped: the tags and attributes above are all there is.)
    }
  });

  test("the writer refuses a tag or attribute outside its list", async () => {
    // (Reached only by a mistake in this file: shown by asking for one.)
    const { TAGS: tags } = await import("./html.ts");
    expect(Object.keys(tags)).not.toContain("script");
    expect(tags.a).toEqual(["href", "rel"]);
  });

  test("a rendering larger than its bound stops, said so", () => {
    expect(() => toHtml(`|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n${"a\n".repeat(20_000)}`, options, MOST)).toThrow(TooLarge);
  });

  test("inlines nested deeper than NESTING are written as their words, without recursion", () => {
    // (Ten thousand deep: far past any stack, within what a text may keep.)
    const n = 10_000;
    const { html } = toHtml(`${"*a ".repeat(n)}${"a* ".repeat(n)}`, options, 64 * MOST);
    expect(html.match(/<em>/g)?.length).toBeLessThanOrEqual(64);
  });
});
