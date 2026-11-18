// A form's post: read no further than any form needs, its token compared whole.
import { describe, expect, test } from "vitest";
import { formOf, MOST_FORM, sameText } from "./forms.ts";

describe("a form's body, read no further than any form needs", () => {
  const post = (body: BodyInit, headers: Record<string, string> = {}) =>
    new Request("https://zeron.sh/x", { method: "POST", body, headers: { "content-type": "application/x-www-form-urlencoded", ...headers }, duplex: "half" } as RequestInit);

  test("a form within the bound is read", async () => {
    const form = await formOf(post("note=hello&csrf=t"));
    expect(form?.get("note")).toBe("hello");
  });

  test("past it, by what it says it is, or by what it turns out to be, it is never parsed", async () => {
    expect(await formOf(post("a=b", { "content-length": String(MOST_FORM + 1) }))).toBeNull();
    // (A body that doesn't say how long it is: read only to the bound, then let go.)
    let pulled = 0;
    const endless = new ReadableStream<Uint8Array>({
      pull(controller) {
        pulled++;
        controller.enqueue(new TextEncoder().encode("x".repeat(16 * 1024)));
      },
    });
    expect(await formOf(post(endless))).toBeNull();
    expect(pulled).toBeLessThan(10);
  });

  test("a token is compared whole, whatever its first difference", () => {
    expect(sameText("abc", "abc")).toBe(true);
    expect(sameText("abc", "abd")).toBe(false);
    expect(sameText("abc", "ab")).toBe(false);
    expect(sameText("", "")).toBe(true);
  });
});
