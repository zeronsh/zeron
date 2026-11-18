// The README pipeline against what an author could write to escape it.
// Whatever goes in, what comes out runs nothing, loads nothing from
// anywhere but the package, and can't pass for the page's own parts.
import { describe, expect, test } from "vitest";
import { renderMarkdown, target } from "./markdown.ts";

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
    expect(html).toMatch(/<pre><code class="hljs language-html">/);
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
    expect(html).toContain('<div class="table-scroll"><table>');
    expect(html).toContain('<th align="left">a</th>');
    expect(html).toContain('<input type="checkbox" checked disabled>');
    expect(html).toContain("<del>old</del>");
    expect(html).toContain('<a href="https://example.com/" rel="nofollow noopener ugc">https://example.com</a>');
    expect(html).toContain('href="#user-content-fn-1"');
    expect(html).toContain('id="user-content-fn-1"');
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
    expect(html.startsWith('<pre class="plain">')).toBe(true);
    expect(html).toContain("&#x3C;script>");
    expect(html).not.toContain("<script");
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
