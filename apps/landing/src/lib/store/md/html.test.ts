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
    const n = 100_000;
    const { html } = toHtml(`${"*a ".repeat(n)}${"a* ".repeat(n)}`, options, 64 * MOST);
    expect(html.match(/<em>/g)?.length).toBeLessThanOrEqual(64);
  });
});

/**
 * Shapes built to be slow for a markdown parser (the ones that have made
 * others quadratic), at the store's cap for a README.
 */
export const HOSTILE: Record<string, (n: number) => string> = {
  "link openers": (n) => "[".repeat(n),
  "picture openers": (n) => "![".repeat(n),
  "emphasis openers, then links": (n) => "*a ".repeat(n / 6) + "[a](b) ".repeat(n / 14),
  "links inside unclosed brackets": (n) => "[".repeat(n / 2) + "[a](b)".repeat(n / 12),
  "a ladder of backtick runs": (n) => {
    let s = "";
    for (let k = 1; s.length < n; k++) s += `${"`".repeat(k)}a`;
    return s;
  },
  "angle brackets": (n) => "<a".repeat(n / 2),
  "character references, unfinished": (n) => "&#1".repeat(n / 3),
  "link tails, unclosed": (n) => "[a](".repeat(n / 4),
  "link tails, <unclosed": (n) => "[a](<".repeat(n / 5),
  "link tails, unclosed title": (n) => '[a](b "'.repeat(n / 7),
  "link tails, open parentheses": (n) => `[a](${"(".repeat(n)}`,
  "reference labels, unclosed": (n) => "[a][".repeat(n / 4),
  "empty brackets": (n) => "[]".repeat(n / 2),
  "* and _ alternating": (n) => "*_".repeat(n / 2),
  "closers then openers": (n) => "a* *a ".repeat(n / 6),
  "emphasis nested deep": (n) => "*a ".repeat(n / 6) + "a* ".repeat(n / 6),
  "quotes nested past the bound": (n) => `${"> ".repeat(40)}a\n`.repeat(n / 82),
  "ordered lists nested past the bound": (n) => `${"1. ".repeat(40)}a\n`.repeat(n / 122),
  "lazy quote lines": (n) => "> a\nb\n".repeat(n / 6),
  "definitions with a title on the next line": (n) => '[a]: b\n"\n'.repeat(n / 9),
  "references, many": (n) => `${"[l1] ".repeat(n / 5)}\n\n[l1]: /u`,
  "a bare link closed many times": (n) => `https://a.b/${")".repeat(n)}`,
  "an HTML comment never closed, then blank lines": (n) => `<!--\n${"\n".repeat(n)}`,
  "<pre> never closed": (n) => `<pre>\n${"a\n".repeat(n / 2)}`,
  "block tags, a line each": (n) => "<div>\n\n".repeat(n / 7),
  "a tag line's attributes, a quote never closed": (n) => `<a ${"b='x".repeat(n / 5)}\n`,
  "random markup": (n) => hostile(7, n),
};

describe("linear in what it is given", () => {
  const README = 512 * 1024;
  // Work counted (the passes' own count of what they read and wrote) is the
  // bound; time under Node is a second witness, judged by how it grows
  // (four times the text, at most about four times the time: a square
  // would be sixteen).
  test.each(Object.keys(HOSTILE))("%s", (name) => {
    const make = HOSTILE[name];
    const timed = (bytes: number) => {
      const md = make(bytes);
      let best = Infinity;
      let work = 0;
      for (let k = 0; k < 2; k++) {
        const started = performance.now();
        try {
          work = toHtml(md, options, MOST).work;
        } catch (error) {
          if (!(error instanceof TooLarge)) throw error;
          work = 0;
        }
        best = Math.min(best, performance.now() - started);
      }
      return { ms: best, perByte: work / md.length };
    };
    const small = timed(README / 4);
    const large = timed(README);
    expect(large.perByte).toBeLessThan(16);
    expect(small.perByte).toBeLessThan(16);
    expect(large.ms).toBeLessThan(Math.max(8 * small.ms, 50));
    expect(large.ms).toBeLessThan(1_500);
  });
});
