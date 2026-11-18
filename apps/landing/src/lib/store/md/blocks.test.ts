import { describe, expect, test } from "vitest";
import { blocksOf, COLUMNS, labelKey, NESTING, type Block } from "./blocks.ts";

const kinds = (blocks: Block[]) => blocks.map((b) => b.kind);

describe("the block pass", () => {
  test("headings, paragraphs, code, rules, quotes", () => {
    const { blocks } = blocksOf("# One\n\nTwo\n===\n\npara\ngraph\n\n```js\nconst a = 1;\n```\n\n    indented\n\n***\n\n> quoted\n");
    expect(blocks).toEqual([
      { kind: "heading", level: 1, text: "One" },
      { kind: "heading", level: 1, text: "Two" },
      { kind: "paragraph", text: "para\ngraph" },
      { kind: "code", info: "js", text: "const a = 1;\n" },
      { kind: "code", info: "", text: "indented\n" },
      { kind: "rule" },
      { kind: "quote", children: [{ kind: "paragraph", text: "quoted" }] },
    ]);
  });

  test("lines end at \\n, \\r\\n or \\r alone", () => {
    for (const end of ["\n", "\r\n", "\r"]) {
      expect(kinds(blocksOf(["# a", "", "b", "-", "", "```", "c", "```"].join(end)).blocks)).toEqual(["heading", "heading", "code"]);
    }
  });

  test("a fence closes only with its own character, at least as long; a backtick fence's info has no backtick", () => {
    expect(blocksOf("````\na\n```\n~~~~\n````\nb\n").blocks).toEqual([{ kind: "code", info: "", text: "a\n```\n~~~~\n" }, { kind: "paragraph", text: "b" }]);
    expect(blocksOf("``` a`b\nc\n").blocks).toEqual([{ kind: "paragraph", text: "``` a`b\nc" }]);
    // (A fence never closed runs to the end.)
    expect(blocksOf("~~~\na\n").blocks).toEqual([{ kind: "code", info: "", text: "a\n" }]);
  });

  test("lists: ordered and not, their start, tight or loose, nested, tasks", () => {
    const { blocks } = blocksOf("3. a\n4. b\n\n- [ ] todo\n- [x] done\n  - inner\n\n* one\n\n* two\n");
    expect(blocks).toEqual([
      { kind: "list", ordered: true, start: 3, tight: true, items: [{ task: null, children: [{ kind: "paragraph", text: "a" }] }, { task: null, children: [{ kind: "paragraph", text: "b" }] }] },
      {
        kind: "list",
        ordered: false,
        start: 1,
        tight: true,
        items: [
          { task: false, children: [{ kind: "paragraph", text: "todo" }] },
          { task: true, children: [{ kind: "paragraph", text: "done" }, { kind: "list", ordered: false, start: 1, tight: true, items: [{ task: null, children: [{ kind: "paragraph", text: "inner" }] }] }] },
        ],
      },
      { kind: "list", ordered: false, start: 1, tight: false, items: [{ task: null, children: [{ kind: "paragraph", text: "one" }] }, { task: null, children: [{ kind: "paragraph", text: "two" }] }] },
    ]);
  });

  test("tables: alignment, escaped pipes, rows filled or cut to the header's width", () => {
    const { blocks } = blocksOf("before\n| a | b \\| c | d |\n|:--|:-:|--:|\n| 1 |\n| 1 | 2 | 3 | 4 |\nlazy row\n\nafter\n");
    expect(blocks).toEqual([
      { kind: "paragraph", text: "before" },
      { kind: "table", align: ["left", "center", "right"], head: ["a", "b | c", "d"], rows: [["1", "", ""], ["1", "2", "3"], ["lazy row", "", ""]] },
      { kind: "paragraph", text: "after" },
    ]);
  });

  test(`a table of more than ${COLUMNS} columns is a paragraph`, () => {
    const wide = `|${"a|".repeat(COLUMNS + 1)}\n|${"-|".repeat(COLUMNS + 1)}\n`;
    expect(kinds(blocksOf(wide).blocks)).toEqual(["paragraph"]);
    const fits = `|${"a|".repeat(COLUMNS)}\n|${"-|".repeat(COLUMNS)}\n`;
    expect(kinds(blocksOf(fits).blocks)).toEqual(["table"]);
  });

  test("link reference definitions are taken from paragraphs, the first of a label kept", () => {
    const { blocks, definitions } = blocksOf('[Foo  Bar]: /one "One"\n[foo bar]: /two\n[b]:\n  <c d>\n  \'title\'\ntext\n');
    expect(blocks).toEqual([{ kind: "paragraph", text: "text" }]);
    expect(definitions.get("foo bar")).toEqual({ destination: "/one", title: "One" });
    expect(definitions.get("b")).toEqual({ destination: "c d", title: "title" });
    expect(labelKey(" ẞ ")).toBe(labelKey("SS"));
  });

  test("an HTML block is recognised as CommonMark does, and kept as typed", () => {
    expect(blocksOf("<div>\n*a* &amp;\n</div>\n\nafter\n").blocks).toEqual([
      { kind: "html", text: "<div>\n*a* &amp;\n</div>" },
      { kind: "paragraph", text: "after" },
    ]);
    // (A comment runs to its end, blank lines and all; a tag inside a paragraph is the paragraph's.)
    expect(blocksOf("<!-- a\n\nb -->\nc\n").blocks).toEqual([{ kind: "html", text: "<!-- a\n\nb -->" }, { kind: "paragraph", text: "c" }]);
    expect(blocksOf("a\n<span>b</span>\n").blocks).toEqual([{ kind: "paragraph", text: "a\n<span>b</span>" }]);
  });

  test(`containers nest at most ${NESTING} deep; past it, a marker is text`, () => {
    let depth = 0;
    let node: Block | undefined = blocksOf(`${"> ".repeat(NESTING + 10)}deep\n`).blocks[0];
    while (node?.kind === "quote") {
      depth++;
      node = node.children[0];
    }
    expect(depth).toBe(NESTING);
    expect(node).toEqual({ kind: "paragraph", text: `${"> ".repeat(10)}deep`.trimEnd() });
    // (Lists count too: a list and its item are one level.)
    let lists = 0;
    let list: Block | undefined = blocksOf(`${"- ".repeat(NESTING + 10)}deep\n`).blocks[0];
    while (list?.kind === "list") {
      lists++;
      list = list.items[0]?.children[0];
    }
    expect(lists).toBe(NESTING);
  });

  test("what it reads is a constant times the text, whatever the shape", () => {
    const shapes = [
      "> ".repeat(40) + "a\n",
      "1. ".repeat(40) + "a\n",
      "- a\n  - b\n    - c\n",
      "> a\nb\n",
      `|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n`,
      "[a]: b\n\"\n",
      "a\n=\n",
      "```\n",
      "\t- \t- \t- a\n",
    ];
    for (const shape of shapes) {
      const text = shape.repeat(Math.ceil((256 * 1024) / shape.length));
      const { work } = blocksOf(text);
      expect(work / text.length, JSON.stringify(shape)).toBeLessThan(4);
    }
  });
});
