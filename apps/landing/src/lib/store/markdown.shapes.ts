// Documents the cost estimate (markdown.ts COST) was measured on: each one
// a shape a README or a version's notes can take, ordinary or built to be
// slow. markdown.test.ts holds the estimate to them (real ÷ estimated past
// RATIO_CEILING fails), and the table beside COST is their measurement.

const code = (lang: string, body: string, n: number) => Array.from({ length: n }, () => `\`\`\`${lang}\n${body}\n\`\`\`\n`).join("\n");
const JS = "const value = compute(alpha, beta) + `template ${x}` * 2; // note\n";
const filler = (kb: number) => "Plain words in a paragraph, nothing more to it than that.\n\n".repeat(Math.round((kb * 1024) / 60));

/** The shape real ÷ estimated is judged against (a machine's speed set aside): plain prose and tables. */
export const REFERENCE = "prose and tables";

export const SHAPES: Record<string, () => string> = {
  "an ordinary README": () => "## A heading\n\nSome *words* and a [link](https://example.com), `code`.\n\n- one\n- two\n\n```js\nconst a = 1;\n```\n\n".repeat(500),
  [REFERENCE]: () => "## Usage\n\nThis plugin shows the cost of each chat in the sidebar, from your agents' own usage.\n\n| Setting | What it does |\n|---|---|\n| `limit` | The monthly limit. |\n| `warn` | When to warn you. |\n\n".repeat(400),
  "setext headings, a block each": () => "a\n=\n\n".repeat(5000),
  "list items, a block each": () => "- a\n\n".repeat(5000),
  "nested pictures, many blocks": () => `${"![".repeat(16)}a${"](b)".repeat(16)}\n\n`.repeat(600),
  "nested links, many blocks": () => `${"[".repeat(30)}a${"](b)".repeat(30)}\n\n`.repeat(600),
  "autolinks": () => `${"https://example.com/a www.example.org a@b.co ".repeat(60)}\n\n`.repeat(100),
  "raw HTML (dropped)": () => "<div><span>x</span></div>\n\n".repeat(3000),
  "character references": () => `${"&amp; &#123; &copy; ".repeat(200)}\n\n`.repeat(60),
  "long lines": () => `${"word ".repeat(1600)}\n\n`.repeat(30),
  "fenced code, highlighted, 465 KB": () => code("js", JS.repeat(60), 120),
  "fenced code, highlighted, huge blocks": () => code("js", JS.repeat(600), 8),
  "footnotes, 1,000": () => Array.from({ length: 1000 }, (_, i) => `Text.[^n${i}]\n\n[^n${i}]: Note ${i}.\n`).join("\n"),
  "every footnote referenced from every piece": () =>
    `${Array.from({ length: 12 }, () => Array.from({ length: 300 }, (_, i) => `[^n${i}]`).join(" ")).join("\n\n")}\n\n${Array.from({ length: 300 }, (_, i) => `[^n${i}]: Note ${i} with words.`).join("\n\n")}`,
  "every link definition referenced from every piece": () =>
    `${Array.from({ length: 12 }, () => Array.from({ length: 300 }, (_, i) => `[x][d${i}]`).join(" ")).join("\n\n")}\n\n${Array.from({ length: 300 }, (_, i) => `[d${i}]: https://example.com/${i}`).join("\n\n")}`,
};

/** Shapes that beat the estimate before definitions were carried only where used, and code counted: each must now be drawn within the budget, or be its text. */
export const BEATERS: Record<string, () => string> = {
  "2,000 footnote definitions at the end of 100 KB": () => `${filler(100)}\n\n${Array.from({ length: 2000 }, (_, i) => `[^n${i}]: Note ${i}.`).join("\n\n")}`,
  "link definitions spread out with blank lines": () => `${filler(60)}\n\n${Array.from({ length: 2000 }, (_, i) => `[d${i}]: https://example.com/${i}`).join("\n\n")}`,
  "every footnote referenced from every piece": SHAPES["every footnote referenced from every piece"]!,
  "465 KB of fenced code": SHAPES["fenced code, highlighted, 465 KB"]!,
  "2,000 footnotes": () => Array.from({ length: 2000 }, (_, i) => `Text.[^n${i}]\n\n[^n${i}]: Note ${i}.\n`).join("\n"),
};
