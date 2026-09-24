import { describe, expect, it } from "vitest";
import { cursorListKeyAction, cursorStepLanding, type CursorKeyEvent } from "../src/components/ui/CursorList";

/**
 * The cursor model's composed dispatch — the decisions every card used to
 * hand-roll around `classifyKey`/`menuStep` (whose own tables are covered
 * by `picker-search.test.ts`): which keys step, which activate, and where
 * an empty walk lands per mode.
 */

const key = (k: string, cmd = false, ctrl = false): CursorKeyEvent => ({
  key: k,
  metaKey: cmd,
  ctrlKey: ctrl,
  preventDefault: () => {},
});

describe("cursorListKeyAction", () => {
  it("arrow keys step in their direction", () => {
    expect(cursorListKeyAction(key("ArrowDown"), { count: 3, cursor: 1 })).toEqual({ kind: "step", delta: 1 });
    expect(cursorListKeyAction(key("ArrowUp"), { count: 3, cursor: 1 })).toEqual({ kind: "step", delta: -1 });
  });

  it("ctrl n/p mirror down/up (readline motion)", () => {
    expect(cursorListKeyAction(key("n", false, true), { count: 3, cursor: 0 })).toEqual({ kind: "step", delta: 1 });
    expect(cursorListKeyAction(key("p", false, true), { count: 3, cursor: 2 })).toEqual({ kind: "step", delta: -1 });
  });

  it("enter and mod-enter activate a real row", () => {
    expect(cursorListKeyAction(key("Enter"), { count: 3, cursor: 2 })).toEqual({ kind: "activate", index: 2 });
    expect(cursorListKeyAction(key("enter", true, false), { count: 3, cursor: 0 })).toEqual({
      kind: "activate",
      index: 0,
    });
    expect(cursorListKeyAction(key("enter", false, true), { count: 3, cursor: 0 })).toEqual({
      kind: "activate",
      index: 0,
    });
  });

  it("a null cursor (Option<usize> menus) never activates", () => {
    expect(cursorListKeyAction(key("Enter"), { count: 3, cursor: null })).toBe(null);
  });

  it("a cursor past the end is a no-op — the old row !== undefined guard", () => {
    expect(cursorListKeyAction(key("Enter"), { count: 3, cursor: 3 })).toBe(null);
    // The trailing-row card's sentinel index is the LAST valid one.
    expect(cursorListKeyAction(key("Enter"), { count: 3, cursor: 2 })).toEqual({ kind: "activate", index: 2 });
  });

  it("an empty list steps but never activates", () => {
    expect(cursorListKeyAction(key("Enter"), { count: 0, cursor: 0 })).toBe(null);
    expect(cursorListKeyAction(key("ArrowDown"), { count: 0, cursor: 0 })).toEqual({ kind: "step", delta: 1 });
  });

  it("other keys are not the model's", () => {
    expect(cursorListKeyAction(key("a"), { count: 3, cursor: 1 })).toBe(null);
    expect(cursorListKeyAction(key("Escape"), { count: 3, cursor: 1 })).toBe(null);
    expect(cursorListKeyAction(key("Backspace"), { count: 3, cursor: 1 })).toBe(null);
    // Cmd+arrow is not mod-enter and not a step modifier.
    expect(cursorListKeyAction(key("ArrowDown", true, false), { count: 3, cursor: 1 })).toEqual({
      kind: "step",
      delta: 1,
    });
  });
});

describe("cursorStepLanding", () => {
  it("anchored floors an empty walk at 0 — the cards' ?? 0", () => {
    expect(cursorStepLanding(null, "anchored")).toBe(0);
    expect(cursorStepLanding(2, "anchored")).toBe(2);
  });

  it("nullable keeps menuStep's null — Option<usize> semantics", () => {
    expect(cursorStepLanding(null, "nullable")).toBe(null);
    expect(cursorStepLanding(2, "nullable")).toBe(2);
  });
});
