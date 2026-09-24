// @vitest-environment jsdom

/**
 * Ticket 14 (web-bugs-2026-09, research W1) — the phone drawers must clear
 * the titlebar band. The bar is a transparent, fully hit-testable,
 * full-width overlay at z 40 that grows to `calc(38px +
 * env(safe-area-inset-top))` at phone, while both drawers pin at `top: 0`
 * under it at z 30 (the ladder's documented decision — the bar's cluster
 * must stay clickable to close what it opened). Anything a drawer paints
 * inside the band renders through the bar but every click lands on the
 * bar: the right-pane drawer's 38px strip header sat exactly in it (its
 * tabs visible yet dead), and on notched devices the grown bar covered the
 * sidebar's space-filter row.
 *
 * The fix is the drawer-side pad the research recommends — never a z lift —
 * so coverage is two halves that only together mean "clickable":
 *
 * 1. Geometry contracts on the sheet (the `phone-titlebar-gestures` idiom):
 *    the drawers pad their content below the bar's full phone height, the
 *    pad is exactly the bar's height (the strip's top edge IS the bar's
 *    bottom edge, notch included), the pad lives on the drawer alone, the
 *    desktop base pads are untouched, and z 30 stays under 40.
 * 2. A mounted phone viewport (the `composer-reasoning` idiom: per-file
 *    jsdom pragma, matchMedia stubbed so `(max-width: 768px)` matches, no
 *    JSX — createElement) driving the REAL `RightPane` against the REAL
 *    right-pane store: the phone arm's strip header mounts as the drawer's
 *    first content row and its chips click through the store's real pick
 *    path. jsdom runs no layout and no z hit-testing, so the mounted half
 *    proves the phone wiring is live while the sheet contracts above prove
 *    the geometry that makes those clicks land in a real browser.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { RightPane, usePaneGlide } from "../src/components/right-pane";
import { rightPaneStore, surfaceKey, useRightPane } from "../src/state/right-pane";

// The surface bodies (Changes/Files/Terminal/History mounts) are not under
// test here and drag xterm plus the engine-session providers with them; the
// pane HOST and its strip need only the registry's chrome half. The real
// `RightTabStrip` keeps everything else — chips, drag bookkeeping, the `+`
// sheet arm, the store wiring.
vi.mock("../src/components/surface-registry", () => ({
  surfaceEntry: () => ({
    kind: "diff",
    title: () => "Diff",
    detail: () => null,
    icon: () => "list",
    render: () => null,
  }),
  renderRightSurface: () => null,
}));

// ── The mocked phone viewport ────────────────────────────────────────────────

/** The layer `matchMedia` reports: `phone` is the `(max-width: 768px)` arm. */
const media = { phone: true };
type ChangeListener = () => void;
const mediaListeners = new Set<ChangeListener>();

/** Resize the mocked viewport across the 768px breakpoint. */
function setViewportPhone(phone: boolean): void {
  media.phone = phone;
  for (const listener of [...mediaListeners]) {
    listener();
  }
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => {
    const matches = (): boolean =>
      query === "(max-width: 768px)" ? media.phone : query === "(min-width: 769px)" ? !media.phone : false;
    return {
      get matches() {
        return matches();
      },
      media: query,
      onchange: null,
      addEventListener: (type: string, listener: ChangeListener) => {
        if (type === "change") {
          mediaListeners.add(listener);
        }
      },
      removeEventListener: (type: string, listener: ChangeListener) => {
        if (type === "change") {
          mediaListeners.delete(listener);
        }
      },
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  }) as unknown as typeof window.matchMedia;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
  delete (globalThis as { matchMedia?: unknown }).matchMedia;
});

// ── The mounted pane harness ─────────────────────────────────────────────────

interface MountedPane {
  readonly container: HTMLElement;
  unmount(): void;
}

const mounted: MountedPane[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  document.body.replaceChildren();
  // The suite's default layer is the phone arm; the flip test restores it.
  media.phone = true;
  mediaListeners.clear();
});

/** The real `RightPane` under the shell's own two hooks, at the mocked layer. */
async function mountPane(chatId: string): Promise<MountedPane> {
  function Host() {
    const pane = useRightPane(chatId);
    const glide = usePaneGlide(pane.open, pane.expanded, 480);
    return createElement(RightPane, { chatId, pane, openWidth: 480, glide });
  }
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(createElement(Host));
  });
  // Settle the fleet/ui-settings microtasks the strip's `+` arm subscribes to.
  await act(async () => {});
  let unmounted = false;
  const handle: MountedPane = {
    container,
    unmount() {
      if (unmounted) {
        return;
      }
      unmounted = true;
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  mounted.push(handle);
  return handle;
}

describe("phone drawer tab strip (mounted, mocked 375px-class viewport)", () => {
  it("the strip header is the drawer's first content row and its chips click through to the store", async () => {
    // Two diff tabs: the store mints ids, opens the pane, and lands the
    // active pick on the second add.
    const chatId = "chat-strip";
    rightPaneStore.addDiffSurface(chatId, "diff");
    rightPaneStore.addDiffSurface(chatId, "history");
    const tabs = rightPaneStore.stateFor(chatId).tabs;
    expect(tabs).toHaveLength(2);
    expect(rightPaneStore.stateFor(chatId).open).toBe(true);

    const handle = await mountPane(chatId);

    // The open drawer, with the phone arm's content order: the strip
    // header is the aside's FIRST child — the row the drawer's titlebar
    // pad pushes below the bar's band (strip header, then the inner).
    const aside = handle.container.querySelector<HTMLElement>(".right-pane");
    expect(aside).not.toBeNull();
    expect(aside!.getAttribute("aria-hidden")).toBe("false");
    const strip = handle.container.querySelector<HTMLElement>(".right-pane-strip-header");
    expect(strip).not.toBeNull();
    expect(aside!.firstElementChild).toBe(strip);

    // Both chips render inside the strip, the history tab selected.
    const chips = strip!.querySelectorAll<HTMLElement>(".right-tab");
    expect(chips).toHaveLength(2);
    expect(chips[1]!.getAttribute("aria-selected")).toBe("true");

    // Clickability: the chip's real click path — the strip's `onPick` →
    // `rightPaneStore.setActive` — flips the stored pick to the first tab.
    act(() => {
      chips[0]!.click();
    });
    expect(surfaceKey(rightPaneStore.stateFor(chatId).active)).toBe(surfaceKey(tabs[0]!));
    // And the strip re-renders the selection from the store's verdict.
    const after = strip!.querySelectorAll<HTMLElement>(".right-tab");
    expect(after[0]!.getAttribute("aria-selected")).toBe("true");
    expect(after[1]!.getAttribute("aria-selected")).toBe("false");
  });

  it("the strip header is the live phone arm: desktop widths drop it, a flip back restores it", async () => {
    const chatId = "chat-flip";
    rightPaneStore.addDiffSurface(chatId, "diff");
    const handle = await mountPane(chatId);
    expect(handle.container.querySelector(".right-pane-strip-header")).not.toBeNull();

    // Grow past 768px: the pane is a desktop column again — its tabs ride
    // the titlebar band, never an in-drawer strip header.
    act(() => {
      setViewportPhone(false);
    });
    expect(handle.container.querySelector(".right-pane-strip-header")).toBeNull();

    // And back to phone: the drawer re-arms its header through the same
    // live media hook the sheet's phone blocks key.
    act(() => {
      setViewportPhone(true);
    });
    const strip = handle.container.querySelector<HTMLElement>(".right-pane-strip-header");
    expect(strip).not.toBeNull();
    // Still live after the round trip — the chips keep their click path.
    const chip = strip!.querySelector<HTMLElement>(".right-tab");
    expect(chip).not.toBeNull();
    act(() => {
      chip!.click();
    });
    const tab = rightPaneStore.stateFor(chatId).tabs[0]!;
    expect(surfaceKey(rightPaneStore.stateFor(chatId).active)).toBe(surfaceKey(tab));
  });
});

// ── The sheet's geometry contracts (the phone-titlebar-gestures idiom) ───────

const css = readFileSync(join(process.cwd(), "src/styles/app.css"), "utf8");

/** The contents of the balanced block opening at `openIndex`. */
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

/** Every `@media (max-width: 768px)` block in the sheet. */
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

/**
 * The scoped `selector { … }` rule bodies inside the phone blocks — every
 * match across all of them, so a fork into a second block or a stray copy
 * surfaces as a length failure.
 */
function phoneRules(selector: string): string[] {
  const pattern = new RegExp(`^\\s+${selector}\\s*\\{([^}]*)\\}`, "m");
  const rules: string[] = [];
  for (const block of phoneMediaBlocks()) {
    const body = block.match(pattern);
    if (body !== null) {
      rules.push(body[1]!);
    }
  }
  return rules;
}

/** The declarations of a top-level (column-0) rule. */
function topLevelRule(selector: string): string {
  const match = css.match(new RegExp(`^${selector}\\s*\\{([^}]*)\\}`, "m"));
  const rule = match?.[1];
  if (rule === undefined) {
    throw new Error(`top-level ${selector} rule not found in app.css`);
  }
  return rule;
}

describe("phone drawer titlebar clearance (ticket 14, research W1)", () => {
  it("the pane drawer pads its content below the bar's full phone height, without lifting z", () => {
    const rules = phoneRules("\\.right-pane");
    // Exactly one phone rule carries the drawer — the pad must not fork.
    expect(rules).toHaveLength(1);
    expect(rules[0]).toMatch(/padding-top:\s*calc\(var\(--rb-titlebar-height\)\s*\+\s*env\(safe-area-inset-top\)\);/);
    // The documented ladder decision stands: the drawer stays UNDER the
    // bar (40) so the bar's cluster keeps closing what the drawer opened.
    // A z lift above the bar is the fix this ticket must not trade to.
    expect(rules[0]).toMatch(/z-index:\s*30;/);
  });

  it("the pad is exactly the phone bar's height — the strip's top edge is the bar's bottom edge", () => {
    const bar = phoneRules("\\.titlebar");
    expect(bar).toHaveLength(1);
    const barHeight = bar[0]!.match(/height:\s*(calc\([^;]+\));/)?.[1];
    const drawer = phoneRules("\\.right-pane");
    expect(drawer).toHaveLength(1);
    const pad = drawer[0]!.match(/padding-top:\s*(calc\([^;]+\));/)?.[1];
    // Equal expressions, not merely compatible ones: 38 + the same inset
    // term, so no notch can grow the bar without growing the pad.
    expect(barHeight).toBeDefined();
    expect(pad).toBeDefined();
    expect(pad).toBe(barHeight);
  });

  it("the pad lives on the drawer alone — the body's desktop titlebar pad stays zeroed at phone", () => {
    const body = phoneRules("\\.right-pane-body");
    expect(body).toHaveLength(1);
    expect(body[0]).toMatch(/padding-top:\s*0;/);
    expect(body[0]).not.toMatch(/safe-area/);
  });

  it("the strip header stays a plain 38px row — the notch inset is the drawer pad's term, never the strip's", () => {
    const strip = phoneRules("\\.right-pane-strip-header");
    expect(strip).toHaveLength(1);
    expect(strip[0]).toMatch(/height:\s*var\(--rb-titlebar-height\);/);
    expect(strip[0]).not.toMatch(/safe-area/);
  });

  it("the notched sidebar drawer pads below the same grown band", () => {
    const inner = phoneRules("\\.sidebar-inner");
    expect(inner).toHaveLength(1);
    expect(inner[0]).toMatch(/padding-top:\s*calc\(var\(--rb-titlebar-height\)\s*\+\s*env\(safe-area-inset-top\)\);/);
  });

  it("desktop geometry is untouched — the base pads stay the plain 38px token", () => {
    const sidebar = topLevelRule("\\.sidebar-inner");
    expect(sidebar).toMatch(/padding-top:\s*var\(--rb-titlebar-height\);/);
    expect(sidebar).not.toMatch(/safe-area/);
    const body = topLevelRule("\\.right-pane-body");
    expect(body).toMatch(/padding-top:\s*var\(--rb-titlebar-height\);/);
    expect(body).not.toMatch(/safe-area/);
  });
});
