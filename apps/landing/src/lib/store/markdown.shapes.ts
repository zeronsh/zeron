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
  "nested links, many blocks": () => `${"[".repeat(30)}a${"](b)".repeat(30)}\n\n`.repeat(400),
  "autolinks": () => `${"https://example.com/a www.example.org a@b.co ".repeat(60)}\n\n`.repeat(100),
  "raw HTML (dropped)": () => "<div><span>x</span></div>\n\n".repeat(3000),
  "character references": () => `${"&amp; &#123; &copy; ".repeat(200)}\n\n`.repeat(60),
  "long lines": () => `${"word ".repeat(1600)}\n\n`.repeat(30),
  "fenced code, highlighted, 465 KB": () => code("js", JS.repeat(60), 120),
  "fenced code, highlighted, huge blocks": () => code("js", JS.repeat(600), 8),
  "footnotes, 1,000": () => Array.from({ length: 1000 }, (_, i) => `Text.[^n${i}]\n\n[^n${i}]: Note ${i}.\n`).join("\n"),
  "every footnote referenced from every piece": () =>
    `${Array.from({ length: 12 }, () => Array.from({ length: 300 }, (_, i) => `[^n${i}]`).join(" ")).join("\n\n")}\n\n${Array.from({ length: 300 }, (_, i) => `[^n${i}]: Note ${i} with words.`).join("\n\n")}`,
  "a fence closed with an info string (the rest is code)": () => "```\n```js\n" + "a\n=\n".repeat(4096),
  "tables, sparse, 64 columns": () => Array(2).fill("|" + "a|".repeat(64) + "\n|" + "-|".repeat(64) + "\n" + "a\n".repeat(390).trimEnd()).join("\n\n"),
  "tables, full, 32 columns": () => Array(6).fill("|" + "a|".repeat(32) + "\n|" + "-|".repeat(32) + "\n" + ("|" + "b|".repeat(32) + "\n").repeat(100).trimEnd()).join("\n\n"),
  "closing brackets, under the bound": () => Array(12).fill("]".repeat(1900)).join("\n\n"),
  "code of short lines, 100 KB": () => "```\n" + "a\n".repeat(50_000) + "```",
  "every link definition referenced from every piece": () =>
    `${Array.from({ length: 6 }, () => Array.from({ length: 300 }, (_, i) => `[x][d${i}]`).join(" ")).join("\n\n")}\n\n${Array.from({ length: 300 }, (_, i) => `[d${i}]: https://example.com/${i}`).join("\n\n")}`,
};

/**
 * Shapes that beat the estimate (the first five before definitions were
 * carried only where used and code counted; the rest the review's second
 * attack, with its sources): each must now be drawn within the budget, or
 * be its text, its outcome held in markdown.test.ts.
 */
export const BEATERS: Record<string, () => string> = {
  "a fake fence (a backtick in its info string), 32 KB": () => "```a`\n" + "a\n=\n".repeat(8192),
  "a fake fence, 64 KB": () => "```a`\n" + "a\n=\n".repeat(16384),
  "a fence indented four spaces": () => "a\n    ```\n" + "a\n=\n".repeat(4096),
  "a fence indented by a tab": () => "a\n\t```\n" + "a\n=\n".repeat(4096),
  "a sparse table 1,300 columns wide": () => Array(16).fill("|" + "a|".repeat(1300) + "\n|" + "-|".repeat(1300) + "\n" + "a\n".repeat(398).trimEnd()).join("\n\n"),
  "runs of closing brackets": () => Array(36).fill("]".repeat(7900)).join("\n\n"),
  "lines ended by CR alone": () => Array(36).fill("a\r=\r".repeat(2000)).join("\n\n"),
  "a markdown code block of link openers": () => "```markdown\n" + "[a](".repeat(4000) + "\n```",
  "a rust code block of raw-string openers": () => "```rust\n" + 'r#"'.repeat(13000) + "\n```",
  "2,000 footnote definitions at the end of 100 KB": () => `${filler(100)}\n\n${Array.from({ length: 2000 }, (_, i) => `[^n${i}]: Note ${i}.`).join("\n\n")}`,
  "link definitions spread out with blank lines": () => `${filler(60)}\n\n${Array.from({ length: 2000 }, (_, i) => `[d${i}]: https://example.com/${i}`).join("\n\n")}`,
  "every footnote referenced from every piece": SHAPES["every footnote referenced from every piece"]!,
  "465 KB of fenced code": SHAPES["fenced code, highlighted, 465 KB"]!,
  "2,000 footnotes": () => Array.from({ length: 2000 }, (_, i) => `Text.[^n${i}]\n\n[^n${i}]: Note ${i}.\n`).join("\n"),
};
