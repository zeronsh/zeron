// The README pipeline against what an author could write to escape it.
// Whatever goes in, what comes out runs nothing, loads nothing from
// anywhere but the package, and can't pass for the page's own parts.
import { describe, expect, test } from "vitest";
import { BOUNDS, Budget, delimiters, drawable, RATIO_CEILING, render as renderWithin, renderMarkdown, target, workOf } from "./markdown.legacy.ts";
import { BEATERS, REFERENCE, SHAPES } from "./markdown.shapes.ts";

const FILES = "/plugins/demo/files/1.0.0/";
const render = (md: string) => renderMarkdown(md, { file: (p) => FILES + p.split("/").map(encodeURIComponent).join("/"), demote: true });

/** Every attribute value of a kind in some HTML, e.g. all hrefs. */
const attributes = (html: string, name: string) => [...html.matchAll(new RegExp(`\\s${name}="([^"]*)"`, "g"))].map((m) => m[1]!);

/** What no rendered README may hold, whatever its source. */
function assertInert(html: string) {
  expect(html).not.toMatch(/<script/i);
  expect(html).not.toMatch(/<(iframe|object|embed|form|style|link|meta|base|svg|math|template|details|textarea|button|select)\b/i);
  expect(html).not.toMatch(/\son[a-z]+=/i);
  expect(html).not.toMatch(/\sstyle=/i);
  expect(html).not.toMatch(/\s(srcset|formaction|action|xlink:href|background|poster)=/i);
  for (const href of attributes(html, "href")) expect(href).toMatch(/^(https?:\/\/|mailto:|#user-content-)/);
  for (const src of attributes(html, "src")) expect(src.startsWith(FILES)).toBe(true);
  for (const id of attributes(html, "id")) expect(id).toMatch(/^(user-content-|footnote-label$)/);
}

const HOSTILE: [string, string][] = [
  ["a script block", "<script>alert(1)</script>"],
  ["a script inline", "hi <script>alert(1)</script> there"],
  ["an image with onerror", '<img src="x" onerror="alert(1)">'],
  ["an svg with onload", "<svg onload=alert(1)>"],
  ["an iframe", '<iframe src="https://evil.example"></iframe>'],
  ["a style block", "<style>body{display:none}</style>"],
  ["a form", '<form action="https://evil.example"><input name="x"><button>go</button></form>'],
  ["details/summary", "<details open ontoggle=alert(1)><summary>x</summary>y</details>"],
  ["a javascript link", "[x](javascript:alert(1))"],
  ["a JaVaScRiPt link", "[x](JaVaScRiPt:alert(1))"],
  ["a javascript link with a tab", "[x](java\tscript:alert(1))"],
  ["a javascript link with an entity tab", "[x](java&#x09;script:alert(1))"],
  ["a javascript link with leading spaces", "[x]( javascript:alert(1))"],
  ["a javascript link with a NUL", "[x](\u0000javascript:alert(1))"],
  ["a javascript link with an entity colon", "[x](javascript&colon;alert(1))"],
  ["a javascript autolink", "<javascript:alert(1)>"],
  ["a data link", "[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)"],
  ["a vbscript link", "[x](vbscript:msgbox(1))"],
  ["a file link", "[x](file:///etc/passwd)"],
  ["a reference-style javascript link", "[x][1]\n\n[1]: javascript:alert(1)"],
  ["a data image", "![x](data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+)"],
  ["a remote image", "![tracker](https://evil.example/pixel.png)"],
  ["a protocol-relative image", "![x](//evil.example/x.png)"],
  ["a backslash-relative image", "![x](\\\\evil.example\\x.png)"],
  ["a climbing image", "![x](../../secret.png)"],
  ["an absolute-path image", "![x](/etc/passwd.png)"],
  ["an html entity script", "&lt;script&gt;alert(1)&lt;/script&gt;"],
  ["a raw anchor with javascript", '<a href="javascript:alert(1)">x</a>'],
  ["an id that clobbers the page's", '<a id="install">x</a> <span name="csrf">y</span>'],
  ["a footnote whose label is HTML", 'Note[^x"><script>]\n\n[^x"><script>]: <img src=x onerror=alert(1)>'],
  ["a table cell with a link", "| a |\n| - |\n| [x](javascript:alert(1)) |"],
  ["a fenced block that says HTML", "```html\n<script>alert(1)</script>\n```"],
  ["a math-ish block", "$$\\href{javascript:alert(1)}{x}$$"],
  ["comments", "<!-- <script>alert(1)</script> -->"],
  ["a base tag", '<base href="https://evil.example/">'],
  ["a meta refresh", '<meta http-equiv="refresh" content="0;url=https://evil.example">'],
  ["an object", '<object data="https://evil.example/x.swf"></object>'],
];

describe("a README's hostile input renders inert", () => {
  for (const [name, md] of HOSTILE) {
    test(name, () => assertInert(render(md)));
  }

  test("text that looks like HTML stays as text", () => {
    const html = render("&lt;script&gt;alert(1)&lt;/script&gt; and `<b>code</b>`");
    expect(html).toContain("&#x3C;script>");
    expect(html).toContain("<code>&#x3C;b>code&#x3C;/b></code>");
  });

  test("a fenced block that says HTML is shown as code", () => {
    const html = render("```html\n<script>alert(1)</script>\n```");
    expect(html).toMatch(/<pre tabindex="0"><code class="hljs language-html">/);
    expect(html).toContain("alert(1)");
  });
});

describe("what a README may hold", () => {
  test("pictures in the package go through the file route", () => {
    const html = render('![A screen](shots/one.png "Its title") ![Same](./shots/one.png) ![Up and back](docs/../shots/one.png)');
    expect(attributes(html, "src")).toEqual([`${FILES}shots/one.png`, `${FILES}shots/one.png`, `${FILES}shots/one.png`]);
    expect(html).toContain('alt="A screen"');
    expect(html).toContain('title="Its title"');
    expect(html).toContain('loading="lazy"');
  });

  test("a picture's path is encoded, never interpreted", () => {
    const html = render("![x](shots/a%20b.png) ![y](shots/%22onerror%3D.png)");
    expect(attributes(html, "src")).toEqual([`${FILES}shots/a%20b.png`, `${FILES}shots/%22onerror%3D.png`]);
    assertInert(html);
  });

  test("a file in the package that isn't a picture is shown as its words", () => {
    expect(render("![the code](init.luau)")).toBe("<p>the code</p>");
  });

  test("a remote picture becomes a link to it, named by its words", () => {
    const html = render("![Build status](https://ci.example/badge.svg)");
    expect(html).toBe('<p><a href="https://ci.example/badge.svg" rel="nofollow noopener ugc">Build status</a></p>');
  });

  test("links out carry nofollow noopener ugc; in-page links don't", () => {
    const html = render("[site](https://example.com/a?b=1) [mail](mailto:ada@example.com) [usage](#usage)\n\n## Usage");
    expect(html).toContain('<a href="https://example.com/a?b=1" rel="nofollow noopener ugc">site</a>');
    expect(html).toContain('<a href="mailto:ada@example.com" rel="nofollow noopener ugc">mail</a>');
    expect(html).toContain('<a href="#user-content-usage">usage</a>');
    expect(html).toContain('<h3 id="user-content-usage">Usage</h3>');
  });

  test("a link to another file of the package keeps its words, not the link", () => {
    expect(render("See [the guide](docs/guide.md).")).toBe("<p>See the guide.</p>");
  });

  test("only https (and mail) is a link: http, and a link with a password in it, are words", () => {
    expect(render("[plain](http://example.com/) [creds](https://user:pw@example.com/)")).toBe("<p>plain creds</p>");
    expect(render("![tracker](http://example.com/p.png)")).toBe("<p>tracker</p>");
  });

  test("protocol-relative links are https", () => {
    expect(render("[x](//example.com/p)")).toContain('href="https://example.com/p"');
  });

  test("headings sit a level under the page's, with ids that can't be the page's", () => {
    const html = render("# Title\n\n## Part\n\n###### Deep\n\n## Part");
    expect(html).toContain('<h2 id="user-content-title">Title</h2>');
    expect(html).toContain('<h3 id="user-content-part">Part</h3>');
    expect(html).toContain('<h6 id="user-content-deep">Deep</h6>');
    expect(html).toContain('<h3 id="user-content-part-1">Part</h3>');
  });

  test("GFM: tables scroll in their own box, task lists, strikethrough, autolinks, footnotes", () => {
    const html = render("| a | b |\n|:-|-:|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n\n~~old~~ https://example.com\n\nNote[^1].\n\n[^1]: The note.");
    expect(html).toContain('<div class="table-scroll" tabindex="0"><table>');
    expect(html).toContain('<th align="left">a</th>');
    expect(html).toContain('<input type="checkbox" checked disabled>');
    expect(html).toContain("<del>old</del>");
    expect(html).toContain('<a href="https://example.com/" rel="nofollow noopener ugc">https://example.com</a>');
    // (A footnote's ids carry its document's tag: two documents on a page never share one.)
    const tag = /id="user-content-([a-z0-9]+)-fn-1"/.exec(html)?.[1];
    expect(tag).toBeTruthy();
    expect(html).toContain(`href="#user-content-${tag}-fn-1"`);
    expect(html).toContain(`id="user-content-${tag}-footnote-label"`);
    assertInert(html);
  });

  test("code is highlighted with classes, never inline style", () => {
    const html = render("```ts\nconst x: number = 1;\n```\n\n```nope\nplain\n```");
    expect(html).toContain('<span class="hljs-keyword">const</span>');
    expect(html).toContain('<code class="language-nope">plain\n</code>');
    expect(html).not.toContain("style=");
  });

  test("a document nested past what the parser can hold is shown as its text", () => {
    const md = `${"> ".repeat(5_000)}<script>alert(1)</script>`;
    const html = render(md);
    expect(html).toContain(`<p class="as-text">Shown as plain text: it's too long or too deeply nested to lay out.</p><pre class="plain" tabindex="0">`);
    expect(html).toContain("&#x3C;script>");
    expect(html).not.toContain("<script");
  });

  test("emphasis delimiters past the bound are shown as text, at once (the parser's cost is their square)", () => {
    for (const md of ["*a_ ".repeat(12_000), "**_ ".repeat(16_000), "~~a~~ ".repeat(8_000)]) {
      const started = performance.now();
      const html = render(md);
      expect(performance.now() - started).toBeLessThan(250);
      expect(html).toContain('<pre class="plain"');
    }
  });

  test("underscores in code don't count against the bound", () => {
    const code = `\`\`\`py\n${"snake_case_name = other_name_here\n".repeat(2_000)}\`\`\`\n\nSome *real* emphasis and \`inline_code_with_underscores\`.`;
    expect(delimiters(code)).toBe(2);
    expect(render(code)).toContain("<em>real</em>");
  });

  test("brackets and emphasis by the thousand render in good time", () => {
    const md = `${"[".repeat(20_000)}\n\n${"*a".repeat(4_000)}`;
    const started = performance.now();
    assertInert(render(md));
    expect(performance.now() - started).toBeLessThan(2_000);
  });
});

describe("references", () => {
  test.each([
    ["shots/a.png", { kind: "file", path: "shots/a.png" }],
    ["./a.png", { kind: "file", path: "a.png" }],
    ["a/../b.png", { kind: "file", path: "b.png" }],
    ["../a.png", { kind: "none" }],
    ["/a.png", { kind: "none" }],
    ["#Usage", { kind: "anchor", id: "Usage" }],
    ["https://x.example/", { kind: "web", url: "https://x.example/" }],
    ["http://x.example/", { kind: "none" }],
    ["//x.example/p", { kind: "web", url: "https://x.example/p" }],
    ["HTTPS://X.EXAMPLE", { kind: "web", url: "https://x.example/" }],
    ["javascript:alert(1)", { kind: "none" }],
    [" \tjavascript:alert(1)", { kind: "none" }],
    ["data:text/html,x", { kind: "none" }],
    ["ftp://x.example/", { kind: "none" }],
    ["", { kind: "none" }],
  ])("%s", (reference, expected) => {
    expect(target(reference)).toEqual(expected);
  });
});

describe("the README never outweighs the page", () => {
  const file = (p: string) => `/f/${p}`;
  test("a first heading that is the plugin's name, said again, is not drawn; any other is, a step down", () => {
    expect(renderMarkdown("# Focus timer\n\nIt keeps time.", { file, demote: true, title: "Focus timer" }).trim()).toBe("<p>It keeps time.</p>");
    expect(renderMarkdown("#  focus   TIMER \n\nIt keeps time.", { file, demote: true, title: "Focus timer" }).trim()).toBe("<p>It keeps time.</p>");
    expect(renderMarkdown("# Getting started\n\nIt keeps time.", { file, demote: true, title: "Focus timer" })).toContain('<h2 id="user-content-getting-started">Getting started</h2>');
    // (Only the first thing, only a top-level heading.)
    expect(renderMarkdown("Intro.\n\n# Focus timer", { file, demote: true, title: "Focus timer" })).toContain("<h2");
    expect(renderMarkdown("## Focus timer\n\nx", { file, demote: true, title: "Focus timer" })).toContain("<h3");
  });
});

describe("a version's notes in one line", () => {
  test("the first line that says something, without its markdown", async () => {
    const { firstLine } = await import("./markdown.legacy.ts");
    expect(firstLine("## 1.2.0\n\n- Fixed **the** timer, see [the issue](https://x)")).toBe("1.2.0");
    expect(firstLine("\n\n- Fixed **the** `timer`, see [the issue](https://x)")).toBe("Fixed the timer, see the issue");
    expect(firstLine("1. First\n2. Second")).toBe("First");
    expect(firstLine(null)).toBe("");
  });
});

describe("a budget of work, not a clock (a Worker's doesn't move while code runs)", () => {
  const options = { file: (p: string) => `/f/${p}`, demote: true };
  const timed = (md: string, budget?: Budget) => {
    const started = performance.now();
    const out = renderWithin(md, options, budget);
    return { ...out, ms: performance.now() - started };
  };
  /** The store's caps: a README's 512 KB, a version's notes 64 KB. */
  const README = 512 * 1024;
  const NOTES = 64 * 1024;
  // The three shapes the review found, then the same spread over many blocks (each block small, the whole not).
  const SHAPES: [string, (bytes: number) => string][] = [
    ["setext headings", (b) => "a\n=\n".repeat(b / 4)],
    ["quote markers", (b) => ">".repeat(b)],
    ["nested brackets", (b) => "[".repeat(b / 5) + "a" + "](b)".repeat(b / 5)],
    ["setext headings, a block each", (b) => "a\n=\n\n".repeat(b / 5)],
    ["list items, a block each", (b) => "- a\n\n".repeat(b / 5)],
    ["nested pictures, many blocks", (b) => ("![".repeat(16) + "a" + "](b)".repeat(16) + "\n\n").repeat(b / 99)],
    ["emphasis, many blocks", (b) => ("*a_ ".repeat(300) + "\n\n").repeat(b / 1202)],
  ];

  for (const [name, make] of SHAPES) {
    for (const [cap, bytes] of [["notes", NOTES], ["README", README]] as const) {
      test(`${name} at the ${cap}'s cap: answered within the bound, never parsed past it`, () => {
        const { html, cut, ms } = timed(make(bytes));
        expect(ms).toBeLessThan(1_500);
        // (Drawn when it fits the budget; otherwise its text, said so.)
        if (cut) expect(html).toContain(`<p class="as-text">Shown as plain text: it's too long or too deeply nested to lay out.</p><pre class="plain" tabindex="0">`);
      });
    }
  }

  test("a shape past a bound is known before parsing: drawable says so, at once", () => {
    for (const md of ["a\n=\n".repeat(131_072), ">".repeat(65_536), "[".repeat(13_107) + "a" + "](b)".repeat(13_107)]) {
      const started = performance.now();
      expect(drawable(md)).not.toBeNull();
      expect(performance.now() - started).toBeLessThan(250);
    }
  });

  test("an ordinary README of about 64 KB is drawn, whole, its references and footnotes across its pieces", () => {
    const section = (i: number) => `## Part ${i}\n\nSee [the guide][guide] and the note.[^n]\n\n- one\n- two\n\n\`\`\`js\nconst a = ${i};\n\`\`\`\n\n`;
    const md = Array.from({ length: 700 }, (_, i) => section(i)).join("") + "[guide]: https://example.com/guide\n\n[^n]: The note.\n";
    expect(md.length).toBeGreaterThan(60_000);
    const { html, cut } = timed(md);
    expect(cut).toBeNull();
    expect(html.match(/<h3 id="user-content-part-\d+">/g)).toHaveLength(700);
    // (A reference in the first piece finds its definition in the last.)
    expect(html).toContain('<a href="https://example.com/guide" rel="nofollow noopener ugc">the guide</a>');
    expect(html).toContain('data-footnotes');
    expect(html.match(/id="user-content-part-0"/g)).toHaveLength(1);
  });

  test("a page's budget is shared: what is left undrawn is its text, said so, and the budget is spent only on what is drawn", () => {
    const budget = new Budget(700);
    const big = "- a\n\n".repeat(10_000);
    const first = timed(big, budget);
    expect(first.cut).toBeNull();
    const second = timed(big, budget);
    expect(second.cut).toBe("page");
    expect(second.ms).toBeLessThan(250);
    expect(timed("Small *notes*.", budget).cut).toBeNull();
  });
});

test("two documents' footnotes on one page share no id", () => {
  const options = { file: (p: string) => p };
  const ids = (html: string) => [...html.matchAll(/id="([^"]+)"/g)].map((m) => m[1]);
  const a = renderMarkdown("A note.[^1]\n\n[^1]: The note.", options);
  const b = renderMarkdown("Another.[^1]\n\n[^1]: Another note.", options);
  expect(ids(a)).toHaveLength(3);
  expect(ids(a).filter((id) => ids(b).includes(id))).toEqual([]);
  expect(a).not.toContain('id="footnote-label"');
});

describe("the estimate holds: what beat it before is caught, and no shape costs far more than reckoned", () => {
  const options = { file: (p: string) => `/f/${p}`, demote: true };
  /** What each shape must come to: drawn within the budget, or its text because it's past it. */
  const OUTCOMES: Record<string, "drawn" | "budget" | "shape"> = {
    "a fake fence (a backtick in its info string), 32 KB": "shape",
    "a fake fence, 64 KB": "shape",
    "a fence indented four spaces": "shape",
    "a fence indented by a tab": "shape",
    "a sparse table 1,300 columns wide": "shape",
    "runs of closing brackets": "shape",
    "lines ended by CR alone": "shape",
    // (Code past what may be highlighted is drawn as plain code.)
    "a markdown code block of link openers": "drawn",
    "a rust code block of raw-string openers": "drawn",
    "2,000 footnote definitions at the end of 100 KB": "budget",
    "link definitions spread out with blank lines": "budget",
    "every footnote referenced from every piece": "drawn",
    "465 KB of fenced code": "drawn",
    "2,000 footnotes": "budget",
  };

  for (const [name, make] of Object.entries(BEATERS)) {
    const outcome = OUTCOMES[name]!;
    test(`${name}: ${outcome === "drawn" ? "drawn, within the budget of work" : outcome === "shape" ? "its text, said so: a block past a bound, known before parsing" : "its text, said so: past the budget of work"}`, () => {
      const md = make();
      const work = workOf(md);
      const { html, cut } = renderWithin(md, options);
      if (outcome === "drawn") {
        expect(cut).toBeNull();
        expect(work).not.toBeNull();
        expect(work!).toBeLessThanOrEqual(BOUNDS.work);
        // (Within the output's bound too.)
        expect(html.length).toBeLessThanOrEqual(BOUNDS.html);
      } else {
        expect(cut).toBe(outcome);
        if (outcome === "shape") expect(work).toBeNull();
        else expect(work!).toBeGreaterThan(BOUNDS.work);
        expect(html).toContain(`<p class="as-text">Shown as plain text: it's too long or too deeply nested to lay out.</p>`);
      }
    });
  }

  test(`no measured shape costs more than ${RATIO_CEILING}× its estimate (each judged against the reference shape, so a machine's speed is set aside)`, () => {
    // (Here the clock moves: this is Node, not a Worker. Best of three, warm.)
    const ratio = (md: string) => {
      let best = Infinity;
      for (let i = 0; i < 3; i++) {
        const started = performance.now();
        renderWithin(md, options);
        best = Math.min(best, performance.now() - started);
      }
      return best / workOf(md)!;
    };
    // The reference's ratio on the measuring box (markdown.ts, warm), and here.
    const MEASURED_REFERENCE = 0.41;
    const scale = MEASURED_REFERENCE / ratio(SHAPES[REFERENCE]!());
    const over = Object.entries(SHAPES)
      .map(([name, make]) => [name, ratio(make()) * scale] as const)
      .filter(([, r]) => r > RATIO_CEILING);
    expect(over).toEqual([]);
  }, 120_000);
});

