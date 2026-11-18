// A file the highlighter can't take must still be shown: as its text, and said so.
import { describe, expect, test } from "vitest";
import { highlighted } from "./highlight.ts";

describe("the highlighter, given a file built to break it", () => {
  test("a 3 KB template nest is shown plain, escaped, and said to be, never thrown", () => {
    const text = "`${".repeat(1000);
    const out = highlighted(text, "javascript");
    expect(out.plain).toBe(true);
    expect(out.html).toBe(text);
  });

  test("ordinary code is highlighted, and not said to be plain", () => {
    const out = highlighted("const a = `x ${b}`;", "javascript");
    expect(out.plain).toBe(false);
    expect(out.html).toContain('<span class="hljs-keyword">const</span>');
  });

  test("text that could be markup comes out as text", () => {
    expect(highlighted("<script>alert(1)</script>", null).html).toBe("&#x3C;script>alert(1)&#x3C;/script>");
  });
});
