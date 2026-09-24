import { afterEach, describe, expect, it, vi } from "vitest";
import { flyoutOpensLeft } from "../src/lib/flyout-side";

/**
 * The side probe's contract (lib/flyout-side.ts): both callers — the
 * sidebar view submenu (space-filter) and the composer settings flyout
 * (composer-pickers) — pin their own reach constant (the card span +
 * offset each actually pins), and the helper owns only the comparison
 * shape. The node environment has no `window`, so the suite stubs one;
 * the probe reads `innerWidth` only at call time.
 */

afterEach(() => {
  vi.unstubAllGlobals();
});

/** A minimal DOMRect stand-in — the probe reads `.right` only. */
function rectAt(right: number): DOMRect {
  return { right } as DOMRect;
}

describe("flyoutOpensLeft", () => {
  it("opens left when the reach past the anchor's right edge crosses the window", () => {
    vi.stubGlobal("window", { innerWidth: 1024 });
    // The settings flyout's 244 reach past a row near the right edge.
    expect(flyoutOpensLeft(rectAt(900), 244)).toBe(true);
    // The same reach with plenty of room to the right.
    expect(flyoutOpensLeft(rectAt(700), 244)).toBe(false);
    // The view submenu's whole span — the 232px card + its 10px offset.
    expect(flyoutOpensLeft(rectAt(900), 232 + 10)).toBe(true);
    expect(flyoutOpensLeft(rectAt(500), 232 + 10)).toBe(false);
  });

  it("the boundary is strict — a reach that exactly meets the edge stays right", () => {
    vi.stubGlobal("window", { innerWidth: 1024 });
    // 780 + 244 === 1024: the card lands flush, not past, so it opens right.
    expect(flyoutOpensLeft(rectAt(780), 244)).toBe(false);
    // One more pixel and there is no room — left.
    expect(flyoutOpensLeft(rectAt(781), 244)).toBe(true);
  });
});
