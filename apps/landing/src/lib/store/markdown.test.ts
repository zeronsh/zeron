// The README pipeline against what an author could write to escape it.
// Whatever goes in, what comes out runs nothing, loads nothing from
// anywhere but the package, and can't pass for the page's own parts.
import { describe, expect, test } from "vitest";
import { BOUNDS, firstLine, render as renderWith, renderMarkdown, target } from "./markdown.ts";

const FILES = "/plugins/demo/files/1.0.0/";
const render = (md: string) => renderMarkdown(md, { file: (p) => FILES + p.split("/").map(encodeURIComponent).join("/"), demote: true }).trim();

/** Every attribute value of a kind in some HTML, e.g. all hrefs. */
const attributes = (html: string, name: string) => [...html.matchAll(new RegExp(`\\s${name}="([^"]*)"`, "g"))].map((m) => m[1]!);

/** What no rendered README may hold, whatever its source. (Raw HTML is its characters, escaped: only real tags are looked at.) */
function assertInert(html: string) {
  const tags = (html.match(/<[^>]*>/g) ?? []).join("\n");
  expect(tags).not.toMatch(/<script/i);
  expect(tags).not.toMatch(/<(iframe|object|embed|form|style|link|meta|base|svg|math|template|details|textarea|button|select|section|sup)\b/i);
  expect(tags).not.toMatch(/\son[a-z]+=/i);
  expect(tags).not.toMatch(/\sstyle=/i);
  expect(tags).not.toMatch(/\s(srcset|formaction|action|xlink:href|background|poster)=/i);
  for (const href of attributes(html, "href")) expect(href).toMatch(/^(https:\/\/|mailto:|#user-content-)/);
  for (const src of attributes(html, "src")) expect(src.startsWith(FILES)).toBe(true);
  for (const id of attributes(html, "id")) expect(id).toMatch(/^user-content-/);
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
  ["a badge whose link is javascript", "[![b](https://x.example/b.svg)](javascript:alert(1))"],
  ["a badge whose picture is the package's, in a link out", "[![b](shots/b.png)](https://x.example/)"],
];

describe("a README's hostile input renders inert", () => {
  for (const [name, md] of HOSTILE) {
    test(name, () => assertInert(render(md)));
  }

  test("raw HTML is the characters typed, escaped", () => {
    expect(render("<script>alert(1)</script>")).toBe("<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
    expect(render("<details>\n<summary>x</summary> &amp;\n</details>")).toBe("<p>&lt;details&gt;<br>\n&lt;summary&gt;x&lt;/summary&gt; &amp;amp;<br>\n&lt;/details&gt;</p>");
    expect(render("a <b>bold</b> word")).toBe("<p>a &lt;b&gt;bold&lt;/b&gt; word</p>");
  });

  test("text that looks like HTML stays as text", () => {
    const html = render("&lt;script&gt;alert(1)&lt;/script&gt; and `<b>code</b>`");
    expect(html).toContain("&lt;script&gt;alert(1)");
    expect(html).toContain("<code>&lt;b&gt;code&lt;/b&gt;</code>");
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
    expect(render("![Build status](https://ci.example/badge.svg)")).toBe('<p><a href="https://ci.example/badge.svg" rel="nofollow noopener ugc">Build status</a></p>');
  });

  test("a badge: a remote picture inside a link is the link, named by the picture's words", () => {
    expect(render("[![build](https://ci.example/badge.svg)](https://ci.example/runs)")).toBe('<p><a href="https://ci.example/runs" rel="nofollow noopener ugc">build</a></p>');
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

  test("tables scroll in their own box; task lists, strikethrough, bare links; footnotes and math are their characters", () => {
    const html = render("| a | b |\n|:-|-:|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n\n~~old~~ https://example.com\n\nNote[^1] and $x^2$.\n\n[^1]: The note.");
    expect(html).toContain('<div class="table-scroll" tabindex="0"><table>');
    expect(html).toContain('<th align="left">a</th>');
    expect(html).toContain('<ul class="contains-task-list">');
    expect(html).toContain('<li class="task-list-item"><input type="checkbox" disabled checked> done</li>');
    expect(html).toContain("<del>old</del>");
    expect(html).toContain('<a href="https://example.com/" rel="nofollow noopener ugc">https://example.com</a>');
    expect(html).toContain("Note[^1] and $x^2$.");
    expect(html).toContain("[^1]: The note.");
    assertInert(html);
  });

  test("code is highlighted with classes, never inline style", () => {
    const html = render("```ts\nconst x: number = 1;\n```\n\n```nope\nplain\n```");
    expect(html).toContain('<span class="hljs-keyword">const</span>');
    expect(html).toContain('<code class="language-nope">plain\n</code>');
    expect(html).not.toContain("style=");
  });

  test("nested past 32, a quote or a list is its text; the page is drawn, never shown as text for it", () => {
    const md = `${"> ".repeat(5_000)}<script>alert(1)</script>`;
    const html = render(md);
    expect(html.match(/<blockquote>/g)).toHaveLength(32);
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain('class="as-text"');
    assertInert(html);
  });

  test("emphasis and brackets by the ten thousand are drawn, at once", () => {
    for (const md of ["*a_ ".repeat(12_000), "**_ ".repeat(16_000), "~~a~~ ".repeat(8_000), `${"[".repeat(20_000)}\n\n${"*a".repeat(4_000)}`]) {
      const started = performance.now();
      const html = render(md);
      expect(performance.now() - started).toBeLessThan(250);
      expect(html).not.toContain('class="as-text"');
      assertInert(html);
    }
  });

  test(`a document that would write more than ${BOUNDS.html / 1024 / 1024} MB is shown as its text, said so`, () => {
    const wide = `|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n${"a\n".repeat(20_000)}`;
    const out = renderWith(wide, { file: (p) => FILES + p });
    expect(out.cut).toBe("size");
    expect(out.html).toBe(`<p class="as-text">Shown as plain text: it's too long to lay out.</p><pre class="plain" tabindex="0">${wide}</pre>`);
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
  test("the first line that says something, without its markdown", () => {
    expect(firstLine("## 1.2.0\n\n- Fixed **the** timer, see [the issue](https://x)")).toBe("1.2.0");
    expect(firstLine("\n\n- Fixed **the** `timer`, see [the issue](https://x)")).toBe("Fixed the timer, see the issue");
    expect(firstLine("1. First\n2. Second")).toBe("First");
    expect(firstLine("> - quoted, listed")).toBe("quoted, listed");
    expect(firstLine(null)).toBe("");
  });

  test("…at once, whatever it holds (a line of 64 KB of brackets once took the square of its length)", () => {
    const started = performance.now();
    expect(firstLine("[".repeat(64 * 1024))).toBe("[".repeat(64 * 1024));
    expect(performance.now() - started).toBeLessThan(250);
  });
});
