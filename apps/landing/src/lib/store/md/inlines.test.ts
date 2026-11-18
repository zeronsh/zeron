import { describe, expect, test } from "vitest";
import type { Definition } from "./blocks.ts";
import { inlinesOf, plainText, unescape, type Inline } from "./inlines.ts";

const none = new Map<string, Definition>();
const parse = (s: string, d = none) => inlinesOf(s, d).inlines;

describe("the inline pass", () => {
  test("emphasis, strong, strikethrough, code", () => {
    expect(parse("*a* **b** ~~c~~ ~d~ `e`")).toEqual([
      { kind: "em", children: [{ kind: "text", text: "a" }] },
      { kind: "text", text: " " },
      { kind: "strong", children: [{ kind: "text", text: "b" }] },
      { kind: "text", text: " " },
      { kind: "del", children: [{ kind: "text", text: "c" }] },
      { kind: "text", text: " " },
      { kind: "del", children: [{ kind: "text", text: "d" }] },
      { kind: "text", text: " " },
      { kind: "code", text: "e" },
    ]);
    // (Three tildes or more, or a run of one closing a run of two, are characters.)
    expect(plainText(parse("~~~a~~~ ~b~~"))).toBe("~~~a~~~ ~b~~");
  });

  test("a code span closes with a run of its own length; an escaped backtick opens none", () => {
    expect(parse("``a ` b`` `c")).toEqual([{ kind: "code", text: "a ` b" }, { kind: "text", text: " `c" }]);
    expect(parse("\\``a`")).toEqual([{ kind: "text", text: "`" }, { kind: "code", text: "a" }]);
  });

  test("links and pictures: inline, by reference, with titles", () => {
    const d = new Map<string, Definition>([["ref", { destination: "/r", title: "R" }]]);
    expect(parse('[a](/x "t") ![p](i.png) [b][ref] [ref] [c][]', new Map([...d, ["c", { destination: "/c", title: null }]]))).toEqual([
      { kind: "link", destination: "/x", title: "t", children: [{ kind: "text", text: "a" }] },
      { kind: "text", text: " " },
      { kind: "image", destination: "i.png", title: null, children: [{ kind: "text", text: "p" }] },
      { kind: "text", text: " " },
      { kind: "link", destination: "/r", title: "R", children: [{ kind: "text", text: "b" }] },
      { kind: "text", text: " " },
      { kind: "link", destination: "/r", title: "R", children: [{ kind: "text", text: "ref" }] },
      { kind: "text", text: " " },
      { kind: "link", destination: "/c", title: null, children: [{ kind: "text", text: "c" }] },
    ]);
  });

  test("a badge: a picture inside a link", () => {
    expect(parse("[![build](https://ci.example/badge.svg)](https://ci.example)")).toEqual([
      { kind: "link", destination: "https://ci.example", title: null, children: [{ kind: "image", destination: "https://ci.example/badge.svg", title: null, children: [{ kind: "text", text: "build" }] }] },
    ]);
  });

  test("a link inside a link: the inner one wins, the outer is its characters", () => {
    expect(plainText(parse("[a [b](c) d](e)"))).toBe("[a b d](e)");
    const out = parse("[a [b](c) d](e)");
    expect(out.filter((n) => n.kind === "link")).toHaveLength(1);
  });

  test("destinations: balanced parentheses, <…>, escapes; a parenthesis left open isn't one", () => {
    expect((parse("[a](b(c)d)")[0] as Extract<Inline, { kind: "link" }>).destination).toBe("b(c)d");
    expect((parse("[a](<b c>)")[0] as Extract<Inline, { kind: "link" }>).destination).toBe("b c");
    expect((parse("[a](b\\)c)")[0] as Extract<Inline, { kind: "link" }>).destination).toBe("b)c");
    expect(plainText(parse("[a](b(c)"))).toBe("[a](b(c)");
  });

  test("autolinks: <scheme:…>, <a@b.c>, and bare https:// less its sentence's punctuation", () => {
    expect(parse("<https://a.b/c> <me@x.org>")).toEqual([
      { kind: "link", destination: "https://a.b/c", title: null, children: [{ kind: "text", text: "https://a.b/c" }] },
      { kind: "text", text: " " },
      { kind: "link", destination: "mailto:me@x.org", title: null, children: [{ kind: "text", text: "me@x.org" }] },
    ]);
    expect(parse("See https://a.b/c_(d)). Or http://x.y.")).toEqual([
      { kind: "text", text: "See " },
      { kind: "link", destination: "https://a.b/c_(d)", title: null, children: [{ kind: "text", text: "https://a.b/c_(d)" }] },
      { kind: "text", text: "). Or http://x.y." },
    ]);
  });

  test("raw HTML is its characters; references are what they stand for", () => {
    expect(parse("<b>x</b> &amp; &copy; &#65; &#x1F600; &nope;")).toEqual([{ kind: "text", text: "<b>x</b> & © A 😀 &nope;" }]);
    expect(unescape("a\\*b&lt;")).toBe("a*b<");
  });

  test("breaks: two spaces or a backslash before a line end is hard; else soft", () => {
    expect(parse("a  \nb\\\nc\nd")).toEqual([
      { kind: "text", text: "a" },
      { kind: "hardbreak" },
      { kind: "text", text: "b" },
      { kind: "hardbreak" },
      { kind: "text", text: "c" },
      { kind: "softbreak" },
      { kind: "text", text: "d" },
    ]);
  });

  test("emphasis nested as deep as its text is long builds without recursion", () => {
    const n = 50_000;
    const out = parse(`${"*a ".repeat(n)}${"a* ".repeat(n)}`);
    let depth = 0;
    let node: Inline | undefined = out[0];
    while (node && node.kind === "em") {
      depth++;
      node = node.children.find((c) => c.kind === "em");
    }
    expect(depth).toBe(n);
    expect(plainText(out).length).toBe(n * 4);
  });

  test("what it reads is a constant times the text, whatever the shape", () => {
    const shapes = [
      "[",
      "![",
      "*a [a](b) ",
      "[a](",
      "[a](<",
      "[a](b \"",
      "[a](((",
      "](",
      "[a][",
      "*_",
      "a* *a ",
      "``a`",
      "<a",
      "&#1",
      "https://a.b/)",
      "\\",
    ];
    for (const shape of shapes) {
      const text = shape.repeat(Math.ceil((256 * 1024) / shape.length));
      const { work } = inlinesOf(text, none);
      expect(work / text.length, JSON.stringify(shape)).toBeLessThan(16);
    }
  });
});
