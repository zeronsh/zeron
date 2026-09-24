import { describe, expect, it } from "vitest";
import {
  DESKTOP_QUERY,
  PHONE_QUERY,
  useIsDesktop,
  useIsPhone,
  useMediaQuery,
} from "../src/state/media";
import { PHONE_MAX_WIDTH } from "../src/state/layout";

/**
 * The breakpoint primitive (ticket 49, research M8 §(b)1): `state/media.ts`
 * is the app's ONE media hook, and its two query constants must be locked
 * to the layout constant the stylesheet's phone blocks key — JS and CSS
 * agree by construction, so no resize can ever flip one without the other.
 * Pure parts only — the node environment renders nothing (the
 * `base-popover.test.ts` header's own rule): the hook bodies live behind
 * `matchMedia` + `useSyncExternalStore` and are covered by the acceptance
 * captures, not a render test; importing them here just pins the export
 * set the call sites (`app-shell`, `chat-page`, the responsive surface)
 * consume.
 */

/** `"(max-width: 768px)"` → { feature: "max-width", boundary: 768, unit: "px" }. */
function parseMediaQuery(query: string): { feature: string; boundary: number; unit: string } {
  const match = /^\((min-width|max-width):\s*(\d+)(px)\)$/.exec(query);
  if (match === null) {
    throw new Error(`unparseable media query: ${query}`);
  }
  return { feature: match[1]!, boundary: Number(match[2]), unit: match[3]! };
}

describe("PHONE_QUERY", () => {
  it("derives from PHONE_MAX_WIDTH, so the JS breakpoint can never drift from the layout constant", () => {
    expect(PHONE_QUERY).toBe(`(max-width: ${PHONE_MAX_WIDTH}px)`);
  });

  it("is the stylesheet's phone boundary: (max-width: 768px)", () => {
    expect(PHONE_QUERY).toBe("(max-width: 768px)");
    expect(PHONE_MAX_WIDTH).toBe(768);
  });
});

describe("DESKTOP_QUERY", () => {
  it("is (min-width: 769px)", () => {
    expect(DESKTOP_QUERY).toBe("(min-width: 769px)");
  });

  it("is the exact complement of PHONE_QUERY — one matches whenever the other does not", () => {
    const phone = parseMediaQuery(PHONE_QUERY);
    const desktop = parseMediaQuery(DESKTOP_QUERY);
    // Both media features are width in the same unit, and the desktop
    // boundary is the first integer the phone query excludes: at every
    // viewport width exactly one of the two queries matches.
    expect(phone.feature).toBe("max-width");
    expect(desktop.feature).toBe("min-width");
    expect(phone.unit).toBe(desktop.unit);
    expect(desktop.boundary).toBe(phone.boundary + 1);
  });
});

describe("state/media exports", () => {
  it("ships the one hook set the call sites consume", () => {
    expect(typeof useMediaQuery).toBe("function");
    expect(typeof useIsPhone).toBe("function");
    expect(typeof useIsDesktop).toBe("function");
  });
});
