// A file the highlighter can't take must still be shown: as its text, and said so.
import { describe, expect, test } from "vitest";
import { common, createLowlight } from "lowlight";
import { highlighted, HighlightBudget } from "./highlight.ts";

const lowlightLanguages = () => createLowlight(common).listLanguages();

describe("the highlighter, given a file built to break it", () => {
  test("a template nest within the bounds (short lines, 1.2 KB) is shown plain, escaped, and said to be, never thrown", () => {
    const text = "`${\n".repeat(300);
    const out = highlighted(text, "javascript");
    expect(out.plain).toBe(true);
    expect(out.html).toBe(text);
  });

  test("past the bounds, plain and quiet: a line past 400, a text past its language's cap, a budget spent", () => {
    expect(highlighted("_".repeat(32_768), "typescript")).toEqual({ html: "_".repeat(32_768), plain: false });
    expect(highlighted("let a = 1;\n".repeat(600), "javascript").html).not.toContain("hljs");
    expect(highlighted("[a](\n".repeat(300), "markdown").html).not.toContain("hljs");
    const budget = new HighlightBudget(20);
    expect(highlighted("const a = 1;", "javascript", budget).html).toContain("hljs");
    expect(highlighted("const b = 2;", "javascript", budget).html).not.toContain("hljs");
  });

  test("every shipped grammar's worst opener, within the bounds, is quick", () => {
    // (The survey behind HIGHLIGHT: each grammar against the openers that ran some of them long.)
    const openers = ["[a](", 'r#"', "`${", "/*", '"', "'", "<", "{{", "#{", "(*", "<!--", "${", "\\", "@", "$(", "[[", "_", "-", "#", "*", "a("];
    for (const language of lowlightLanguages()) {
      for (const p of openers) {
        const line = p.repeat(Math.floor(400 / p.length));
        const text = Array.from({ length: Math.floor(4096 / (line.length + 1)) }, () => line).join("\n");
        const started = performance.now();
        highlighted(text, language);
        expect(performance.now() - started, `${language} ${p}`).toBeLessThan(500);
      }
    }
  }, 120_000);

  test("ordinary code is highlighted, and not said to be plain", () => {
    const out = highlighted("const a = `x ${b}`;", "javascript");
    expect(out.plain).toBe(false);
    expect(out.html).toContain('<span class="hljs-keyword">const</span>');
  });

  test("text that could be markup comes out as text", () => {
    expect(highlighted("<script>alert(1)</script>", null).html).toBe("&lt;script&gt;alert(1)&lt;/script&gt;");
  });
});

describe("htmlLines", () => {
  test("a file's highlighted lines, each whole: a span the break cuts closes and opens again; a last break ends it", async () => {
    const { htmlLines, highlight } = await import("./highlight.ts");
    expect(htmlLines('a<span class="x">b\nc</span>d\n')).toEqual(['a<span class="x">b</span>', '<span class="x">c</span>d']);
    expect(htmlLines("one\n\nthree")).toEqual(["one", "", "three"]);
    expect(htmlLines("")).toEqual([""]);
    // (A real file: a comment over three lines, each line its own, every span balanced.)
    const lines = htmlLines(highlight("/* one\n   two\n   three */\nconst x = `a\nb`;\n", "javascript"));
    expect(lines).toHaveLength(5);
    for (const l of lines) expect((l.match(/<span/g) ?? []).length).toBe((l.match(/<\/span>/g) ?? []).length);
    expect(lines.map((l) => l.replace(/<[^>]+>/g, ""))).toEqual(["/* one", "   two", "   three */", "const x = `a", "b`;"]);
  });
});
