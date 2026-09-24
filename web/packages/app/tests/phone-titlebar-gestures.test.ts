import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

/**
 * Ticket 76's stylesheet contract: the phone-scoped app titlebar disallows
 * direct-manipulation panning (`touch-action: none` inside the
 * `@media (max-width: 768px)` block) while the desktop titlebar and the
 * transcript keep their existing touch policy. W3C Pointer Events 3 §8.2
 * intersects a touched element's touch-action with its ancestors', so the
 * titlebar's `none` also covers gestures that land on its descendant
 * controls without per-button overrides. This guards the artifact's
 * scoping only — whether a real hold/drag still moves the document, the
 * transcript or the visual viewport is device evidence, recorded in the
 * ticket's Comments rather than asserted here.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

/** The contents of the balanced `{ … }` block opening at `openIndex`. */
function balancedBlock(openIndex: number): string {
  let depth = 0;
  for (let i = openIndex; i < css.length; i++) {
    if (css[i] === "{") depth++;
    else if (css[i] === "}") {
      depth--;
      if (depth === 0) return css.slice(openIndex + 1, i);
    }
  }
  throw new Error("unbalanced braces in app.css");
}

/** Every `@media (max-width: 768px) { … }` block in the sheet. */
function phoneMediaBlocks(): string[] {
  const blocks: string[] = [];
  let from = 0;
  while (true) {
    const at = css.indexOf("@media (max-width: 768px)", from);
    if (at === -1) return blocks;
    blocks.push(balancedBlock(css.indexOf("{", at)));
    from = at + 1;
  }
}

/** The declarations of a top-level (column-0) rule — media-nested rules are
    indented, so `^` anchoring separates the base rule from scoped ones. */
function topLevelRule(selector: string): string {
  const match = css.match(new RegExp(`^${selector}\\s*\\{([^}]*)\\}`, "m"));
  const rule = match?.[1];
  if (rule === undefined) {
    throw new Error(`top-level ${selector} rule not found in app.css`);
  }
  return rule;
}

describe("phone titlebar gesture containment (ticket 76)", () => {
  it("the phone-scoped .titlebar disallows panning, on its existing safe-area rule", () => {
    const guarded = phoneMediaBlocks()
      .map((block) => block.match(/^\s+\.titlebar\s*\{([^}]*)\}/m)?.[1])
      .filter((rule): rule is string => rule !== undefined);
    // Exactly one phone block carries the titlebar — the guard must not
    // fork into a second rule or multiply across blocks.
    expect(guarded).toHaveLength(1);
    expect(guarded[0]).toMatch(/touch-action:\s*none;/);
    // The guard rides ticket 50's safe-area rule — no geometry fork.
    expect(guarded[0]).toMatch(/env\(safe-area-inset-top\)/);
  });

  it("the desktop titlebar keeps manipulation — no desktop-width gesture change", () => {
    const base = topLevelRule("\\.titlebar");
    expect(base).toMatch(/touch-action:\s*manipulation;/);
    expect(base).not.toMatch(/touch-action:\s*none;/);
  });

  it("the transcript's touch policy is untouched (default auto, its own scroller)", () => {
    const transcript = topLevelRule("\\.transcript");
    expect(transcript).not.toMatch(/touch-action/);
    expect(transcript).toMatch(/overflow-y:\s*auto;/);
  });

  it("no body-wide gesture suppression sneaks in with the titlebar guard", () => {
    expect(topLevelRule("body")).not.toMatch(/touch-action/);
  });
});
