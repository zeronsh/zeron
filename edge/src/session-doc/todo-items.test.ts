import { describe, expect, it } from "vitest";
import { sanitizeToolCall } from "./render-parts";

describe("todo checklist items", () => {
  it("keeps an in-progress status through the render-only policy", () => {
    const call = {
      _tag: "Todo" as const,
      items: [
        { text: "read", done: true },
        { text: "fix", done: false, status: "inProgress" as const },
      ],
    };
    expect(sanitizeToolCall(call)).toEqual(call);
  });
  it("leaves legacy items (no status) byte-for-byte unchanged", () => {
    const call = { _tag: "Todo" as const, items: [{ text: "a", done: false }] };
    const out = sanitizeToolCall(call);
    expect(out).toEqual(call);
    expect(Object.keys((out as typeof call).items[0]!)).toEqual(["text", "done"]);
  });
});
