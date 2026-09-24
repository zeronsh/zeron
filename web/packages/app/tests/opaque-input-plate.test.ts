import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

/**
 * Ticket 79's stylesheet contract: the input-glass family — the composer
 * pill, the question wizard, generic form inputs, and the files-preview
 * comment draft — paints the opaque `--rb-input-plate` (the web port of the
 * desktop opaque-mode `input_glass_bg()` flatten), never the alpha
 * interpolation or the raw (dark-alpha) input role. The web is forced
 * opaque; every other surface was audited opaque already.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

/** The declarations of a top-level (column-0) rule for a selector. */
function topLevelRule(selector: string): string {
  const match = css.match(new RegExp(`^${selector}\\s*\\{([^}]*)\\}`, "m"));
  const rule = match?.[1];
  if (rule === undefined) {
    throw new Error(`top-level ${selector} rule not found in app.css`);
  }
  return rule;
}

describe("opaque input plate (ticket 79)", () => {
  it("the composer pill and wizard paint the plate", () => {
    expect(topLevelRule("\\.composer-pill")).toMatch(/background:\s*var\(--rb-input-plate\);/);
    expect(topLevelRule("\\.wizard-panel")).toMatch(/background:\s*var\(--rb-input-plate\);/);
  });

  it("form inputs and the comment draft paint the plate, not the raw input role", () => {
    expect(topLevelRule("\\.input")).toMatch(/background:\s*var\(--rb-input-plate\);/);
    expect(topLevelRule("\\.input")).not.toMatch(/background:\s*var\(--rb-input\);/);
    expect(topLevelRule("\\.editor-comment-input")).toMatch(
      /background:\s*var\(--rb-input-plate\);/,
    );
  });

  it("the alpha-interpolating input glass is gone", () => {
    expect(css).not.toContain("color-mix(in srgb, var(--rb-input) 82%");
    // No raw input-role background remains anywhere (the role keeps its
    // authored alpha in dark variants — only the flattened plate may paint).
    expect(css).not.toMatch(/^(\s*)background:\s*var\(--rb-input\);/m);
  });
});
