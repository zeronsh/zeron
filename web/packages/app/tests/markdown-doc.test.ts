import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  buildHeadingAnchors,
  clipMarkdownBytes,
  markdownLinkTarget,
  parseMarkdown,
  relativeTarget,
} from "../src/lib/markdown-doc";

describe("parseMarkdown blocks", () => {
  it("parses ATX headings and paragraphs", () => {
    const blocks = parseMarkdown("# Title\n\nSome text here.\n\n## Sub");
    expect(blocks).toEqual([
      { kind: "heading", level: 1, inlines: [{ kind: "text", text: "Title" }] },
      { kind: "paragraph", inlines: [{ kind: "text", text: "Some text here." }] },
      { kind: "heading", level: 2, inlines: [{ kind: "text", text: "Sub" }] },
    ]);
  });

  it("parses setext headings", () => {
    const blocks = parseMarkdown("Title\n=====\n\nSub\n---");
    expect(blocks).toEqual([
      { kind: "heading", level: 1, inlines: [{ kind: "text", text: "Title" }] },
      { kind: "heading", level: 2, inlines: [{ kind: "text", text: "Sub" }] },
    ]);
  });

  it("parses fenced code blocks with a language", () => {
    const blocks = parseMarkdown("```rust\nfn main() {}\n```\n\nafter");
    expect(blocks).toEqual([
      { kind: "code", language: "rust", text: "fn main() {}" },
      { kind: "paragraph", inlines: [{ kind: "text", text: "after" }] },
    ]);
  });

  it("keeps an unclosed fence as code until end of input", () => {
    const blocks = parseMarkdown("```\nlet x = 1;");
    expect(blocks).toEqual([{ kind: "code", language: null, text: "let x = 1;" }]);
  });

  it("parses horizontal rules", () => {
    expect(parseMarkdown("---")).toEqual([{ kind: "rule" }]);
    expect(parseMarkdown("***")).toEqual([{ kind: "rule" }]);
  });

  it("parses unordered and ordered lists", () => {
    const blocks = parseMarkdown("- one\n- two\n\n1. first\n2. second");
    expect(blocks).toEqual([
      {
        kind: "list",
        ordered: false,
        items: [
          { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "one" }] }], task: null },
          { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "two" }] }], task: null },
        ],
      },
      {
        kind: "list",
        ordered: true,
        items: [
          { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "first" }] }], task: null },
          { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "second" }] }], task: null },
        ],
      },
    ]);
  });

  it("parses task markers with their source offset", () => {
    const source = "- [ ] open\n- [x] done";
    const [block] = parseMarkdown(source);
    expect(block).toEqual({
      kind: "list",
      ordered: false,
      items: [
        { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "open" }] }], task: { offset: 2, checked: false } },
        { blocks: [{ kind: "paragraph", inlines: [{ kind: "text", text: "done" }] }], task: { offset: 13, checked: true } },
      ],
    });
  });

  it("nests indented lists", () => {
    const blocks = parseMarkdown("- outer\n  - inner\n- back");
    expect(blocks).toHaveLength(1);
    const list = blocks[0]!;
    expect(list.kind).toBe("list");
    if (list.kind !== "list") {
      return;
    }
    expect(list.items).toHaveLength(2);
    expect(list.items[0]!.blocks.some((block) => block.kind === "list")).toBe(true);
  });

  it("parses block quotes recursively", () => {
    const blocks = parseMarkdown("> # Quoted\n> text");
    expect(blocks).toEqual([
      {
        kind: "quote",
        blocks: [
          { kind: "heading", level: 1, inlines: [{ kind: "text", text: "Quoted" }] },
          { kind: "paragraph", inlines: [{ kind: "text", text: "text" }] },
        ],
      },
    ]);
  });

  it("parses pipe tables", () => {
    const blocks = parseMarkdown("| a | b |\n|---|---|\n| 1 | 2 |");
    expect(blocks).toEqual([
      {
        kind: "table",
        header: [[{ kind: "text", text: "a" }], [{ kind: "text", text: "b" }]],
        rows: [[[{ kind: "text", text: "1" }], [{ kind: "text", text: "2" }]]],
      },
    ]);
  });

  it("a paragraph followed by a table stays a paragraph-plus-table", () => {
    const blocks = parseMarkdown("intro\n\n| h |\n|---|\n| c |");
    expect(blocks.map((block) => block.kind)).toEqual(["paragraph", "table"]);
  });
});

describe("parseMarkdown inlines", () => {
  it("parses code spans, emphasis, and strikethrough", () => {
    const [block] = parseMarkdown("a `code` **bold** *em* ~~gone~~ b");
    expect(block).toEqual({
      kind: "paragraph",
      inlines: [
        { kind: "text", text: "a " },
        { kind: "code", text: "code" },
        { kind: "text", text: " " },
        { kind: "bold", children: [{ kind: "text", text: "bold" }] },
        { kind: "text", text: " " },
        { kind: "italic", children: [{ kind: "text", text: "em" }] },
        { kind: "text", text: " " },
        { kind: "strike", children: [{ kind: "text", text: "gone" }] },
        { kind: "text", text: " b" },
      ],
    });
  });

  it("parses links and images", () => {
    const [block] = parseMarkdown("[docs](guide.md) ![shot](https://x.test/a.png)");
    expect(block).toEqual({
      kind: "paragraph",
      inlines: [
        { kind: "link", href: "guide.md", children: [{ kind: "text", text: "docs" }] },
        { kind: "text", text: " " },
        { kind: "image", src: "https://x.test/a.png", alt: "shot" },
      ],
    });
  });

  it("does not treat emphasis markers mid-word as italic", () => {
    const [block] = parseMarkdown("snake_case_name stays text");
    expect(block).toEqual({ kind: "paragraph", inlines: [{ kind: "text", text: "snake_case_name stays text" }] });
  });
});

describe("markdown link policy", () => {
  it("routes http(s) and mailto externally", () => {
    expect(markdownLinkTarget("https://example.com")).toEqual({ kind: "external", href: "https://example.com" });
    expect(markdownLinkTarget("mailto:a@b.c")).toEqual({ kind: "external", href: "mailto:a@b.c" });
  });

  it("drops unknown schemes", () => {
    expect(markdownLinkTarget("javascript:alert(1)")).toEqual({ kind: "text" });
  });

  it("treats scheme-less targets as workspace paths", () => {
    expect(markdownLinkTarget("src/lib.rs")).toEqual({ kind: "workspace", path: "src/lib.rs" });
    expect(markdownLinkTarget("./guide.md#install")).toEqual({ kind: "workspace", path: "./guide.md" });
  });

  it("resolves workspace links against the open file's directory", () => {
    expect(relativeTarget("docs/guide.md", "install.md")).toEqual({ path: "docs/install.md", anchor: null });
    expect(relativeTarget("docs/guide.md", "../README.md")).toEqual({ path: "README.md", anchor: null });
    expect(relativeTarget("README.md", "docs/guide.md")).toEqual({ path: "docs/guide.md", anchor: null });
  });

  it("relative_target ports the desktop's rejection rules", () => {
    // The ticket's §2.3 port cases (markdown_preview.rs:66-112).
    expect(relativeTarget("docs/readme.md", "../a%20b.md#hello")).toEqual({ path: "a b.md", anchor: "hello" });
    expect(relativeTarget("readme.md", "../secret")).toBeNull();
    expect(relativeTarget("readme.md", "%2Fetc/passwd")).toBeNull();
    expect(relativeTarget("readme.md", "https://example.com")).toBeNull();
    expect(relativeTarget("readme.md", "#hello")).toEqual({ path: "readme.md", anchor: "hello" });
    // The zeron-file: prefix strips recursively and resolves from the ROOT.
    expect(relativeTarget("docs/readme.md", "zeron-file:guide.md")).toEqual({ path: "guide.md", anchor: null });
    // A malformed escape rejects the path (decode failure).
    expect(relativeTarget("docs/readme.md", "a%2.md")).toBeNull();
  });

  it("clips the live buffer at 2 MiB on a UTF-8 boundary", () => {
    expect(clipMarkdownBytes("hello")).toEqual({ text: "hello", truncated: false });
    const ascii = "a".repeat(2 * 1024 * 1024 + 10);
    const clipped = clipMarkdownBytes(ascii);
    expect(clipped.truncated).toBe(true);
    expect(clipped.text.length).toBe(2 * 1024 * 1024);
    // A surrogate pair never splits: the clip lands before the pair.
    const prefix = "b".repeat(2 * 1024 * 1024 - 1);
    const withPair = prefix + "😀" + "c".repeat(20);
    const clippedPair = clipMarkdownBytes(withPair);
    expect(clippedPair.truncated).toBe(true);
    expect(clippedPair.text.endsWith("b")).toBe(true);
  });

  it("builds heading anchors with slugs and collision suffixes", () => {
    const blocks = parseMarkdown("# Same Title\n\n## Same Title\n\n## Other");
    const anchors = buildHeadingAnchors(blocks);
    expect([...anchors.values()]).toEqual(["same-title", "same-title-1", "other"]);
  });
});

// ---------------------------------------------------------------------------
// Markdown preview scroll padding (3ebe2d8a, markdown_preview.rs, ported)
// ---------------------------------------------------------------------------

describe("markdown preview scroll padding", () => {
  it("the 16px breathing room rides the scrollport, not an outer container", () => {
    // The desktop moved py(16) from the preview container onto the gpui list
    // so the padding scrolls with the document and content clips at the
    // viewport edge. The web's scrollport owns the padding directly (CSS
    // scrollport padding scrolls with the content); pin that contract.
    const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");
    const rule = /\.files-markdown-scroll\s*\{[^}]*\}/.exec(css)?.[0] ?? "";
    expect(rule).toContain("overflow-y: auto");
    expect(rule).toContain("padding: 16px 0");
  });
});
