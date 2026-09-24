// @vitest-environment jsdom

/**
 * Ticket 15 (web-bugs-2026-09, research W4) — the phone drawer family's
 * surface contracts, two halves that only together mean "a good sheet":
 *
 * 1. MOUNTED (the composer-reasoning idiom: per-file jsdom pragma, Base UI
 *    running for real, no JSX — createElement): the dialog arm renders the
 *    bottom sheet at phone and the exact `RbDialog` tree at desktop; the
 *    sheet popup carries `[data-open]` while open — the attribute the
 *    `rb-dialog-in` entrance keys on, so the animation contract is live
 *    DOM, not just CSS; and a SECOND sheet stacked over a first (the
 *    add-action shape after ticket 09: the picker sheet beneath, the
 *    editor dialog sheet above) still mounts `[data-open]`, so its
 *    entrance applies over another sheet rather than skipping.
 *
 * 2. CSS contracts (the phone-drawer-titlebar-clearance idiom): the phone
 *    block's full-width `.dialog-card` rule (width 100% + the shared
 *    `--rb-safe-sheet-pad` home-indicator term), every other sheet arm's
 *    composition with that ONE term, the `[data-open]` entrance rule,
 *    and the two guards that must NOT move — the backstop
 *    (`.dialog-card:not(.rb-drawer-card *)` keeps excluding in-sheet
 *    cards) and the desktop card's own 360px.
 */

import { act, createElement, useState } from "react";
import { createRoot } from "react-dom/client";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { PHONE_QUERY } from "../src/state/media";
import { RbDrawerSheet, RbResponsiveDialog } from "../src/components/base/responsive-surface";
import { DialogCard } from "../src/components/ui/Dialog";

// ── The mocked viewport (project-actions-control's controllable cell) ───────

const h = vi.hoisted(() => ({ phone: false }));

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: query === PHONE_QUERY && h.phone,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  if (typeof Element.prototype.scrollIntoView !== "function") {
    Element.prototype.scrollIntoView = () => {};
  }
  if (typeof globalThis.requestAnimationFrame !== "function") {
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    }) as typeof requestAnimationFrame;
  }
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

// ── The mounted harness ─────────────────────────────────────────────────────

interface Mounted {
  readonly container: HTMLDivElement;
  unmount(): void;
}

const mounted: Mounted[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  h.phone = false;
  document.body.replaceChildren();
});

/** The dialog surface, controlled: `Dialog`'s mount-while-open shape. */
function mountDialog(): Mounted {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  function Host() {
    const [open, setOpenState] = useState(true);
    return createElement(RbResponsiveDialog, {
      open,
      onOpenChange: (next: boolean) => {
        setOpenState(next);
      },
      ariaLabel: "Test dialog",
      children: createElement(DialogCard, null, createElement("h2", null, "Title")),
    });
  }
  act(() => {
    root.render(createElement(Host));
  });
  const handle: Mounted = {
    container,
    unmount() {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  mounted.push(handle);
  return handle;
}

/** The sheet stack: a picker-style sheet and, above it, a dialog-style sheet. */
function mountSheetStack(): { sheetOne(): HTMLElement | null; sheetTwo(): HTMLElement | null } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  function Host() {
    const [one, setOne] = useState(true);
    const [two, setTwo] = useState(true);
    return createElement(
      "div",
      null,
      createElement(RbDrawerSheet, {
        open: one,
        onOpenChange: (next: boolean) => {
          setOne(next);
        },
        cardClassName: "popover-card",
        role: "menu",
        ariaLabel: "Actions",
        children: createElement("button", { type: "button" }, "Row"),
      }),
      createElement(RbDrawerSheet, {
        open: two,
        onOpenChange: (next: boolean) => {
          setTwo(next);
        },
        disablePointerDismissal: true,
        ariaLabel: "Add action",
        children: createElement(DialogCard, null, createElement("h2", null, "Title")),
      }),
    );
  }
  act(() => {
    root.render(createElement(Host));
  });
  const unmount = (): void => {
    act(() => {
      root.unmount();
    });
    container.remove();
  };
  mounted.push({ container, unmount });
  return {
    // The stack's DOM order: the picker sheet renders first, the dialog
    // sheet's portal after it (the sheet-over-sheet paint order).
    sheetOne: () => document.querySelectorAll<HTMLElement>(".rb-drawer-card")[0] ?? null,
    sheetTwo: () => document.querySelectorAll<HTMLElement>(".rb-drawer-card")[1] ?? null,
  };
}

// ── The mounted contracts ───────────────────────────────────────────────────

describe("RbResponsiveDialog arms", () => {
  it("desktop: the exact RbDialog tree — the centered card, never a sheet", () => {
    h.phone = false;
    mountDialog();
    const card = document.querySelector<HTMLElement>(".rb-dialog-card");
    expect(card).not.toBeNull();
    expect(card!.getAttribute("aria-label")).toBe("Test dialog");
    expect(document.querySelector(".rb-drawer-card")).toBeNull();
  });

  it("phone: the bottom sheet — the popup carries [data-open], the entrance's key", () => {
    h.phone = true;
    mountDialog();
    const sheet = document.querySelector<HTMLElement>(".rb-drawer-card[aria-label='Test dialog']");
    expect(sheet).not.toBeNull();
    // `rb-dialog-in` keys on exactly this attribute (`.rb-drawer-card[data-open]`,
    // app.css) — the animation contract is live DOM, and the dialog card the
    // full-width rule targets renders inside the sheet frame.
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    expect(sheet!.querySelector(".dialog-card")).not.toBeNull();
    // The centered dialog tree never mounts at phone.
    expect(document.querySelector(".rb-dialog-card")).toBeNull();
  });

  it("phone: sheet-over-sheet — the sheet above keeps its entrance while another is up", () => {
    // The add-action shape after ticket 09: the picker sheet beneath, the
    // editor dialog sheet above. Both sheets ride the modal tier through
    // their own portals; the upper one mounts `[data-open]` over the lower,
    // so its `rb-dialog-in` entrance applies rather than skipping.
    h.phone = true;
    const stack = mountSheetStack();
    const one = stack.sheetOne();
    const two = stack.sheetTwo();
    expect(one).not.toBeNull();
    expect(two).not.toBeNull();
    expect(one!.hasAttribute("data-open")).toBe(true);
    expect(two!.hasAttribute("data-open")).toBe(true);
    expect(two!.querySelector(".dialog-card")).not.toBeNull();
    // The stacked pair both live in the document's portals, the dialog sheet
    // after the picker sheet — the paint order that puts the second on top.
    const sheets = document.querySelectorAll(".rb-drawer-card");
    expect(sheets).toHaveLength(2);
    expect(sheets[1]!.contains(one!)).toBe(false);
  });
});

// ── The CSS contracts (phone-drawer-titlebar-clearance idiom) ───────────────

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

/** The scoped `selector { … }` rule bodies inside the phone blocks. */
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

describe("full-width drawer cards (ticket 15, research W4)", () => {
  it("the dialog arm's inner card stretches to the sheet's width", () => {
    const rules = phoneRules("\\.rb-drawer-card \\.dialog-card");
    expect(rules).toHaveLength(1);
    expect(rules[0]).toMatch(/width:\s*100%;/);
  });

  it("the in-sheet card folds to the frame's geometry and clears the home indicator", () => {
    const rules = phoneRules("\\.rb-drawer-card \\.dialog-card");
    expect(rules).toHaveLength(1);
    // Square bottom corners matching the frame's cap — spelled through the
    // `--rb-radius-bubble` token (16px, the frame's own top corners) …
    expect(rules[0]).toMatch(
      /border-radius:\s*var\(--rb-radius-bubble\) var\(--rb-radius-bubble\) 0 0;/,
    );
    // … no bloom inside the frame's clip …
    expect(rules[0]).toMatch(/box-shadow:\s*none;/);
    // … and the card's own 20px bottom padding composes with the shared
    // safe-area term.
    expect(rules[0]).toMatch(/padding-bottom:\s*calc\(20px \+ var\(--rb-safe-sheet-pad\)\);/);
  });

  it("every sheet arm's content clears the home indicator — the picker sheet and the `+` menu", () => {
    const picker = phoneRules("\\.rb-drawer-card\\.popover-card");
    expect(picker).toHaveLength(1);
    // The 4px rides the token — the wave's one spelling (the review's
    // unification: `var(--rb-space-xs)` everywhere the sheet family
    // composes its inset).
    expect(picker[0]).toMatch(
      /padding-bottom:\s*calc\(var\(--rb-space-xs\) \+ var\(--rb-safe-sheet-pad\)\);/,
    );
    const plus = phoneRules("\\.rb-drawer-card\\.right-plus-menu-sheet");
    expect(plus).toHaveLength(1);
    expect(plus[0]).toMatch(
      /padding:\s*var\(--rb-space-xs\) var\(--rb-space-xs\)\s*calc\(var\(--rb-space-xs\) \+ var\(--rb-safe-sheet-pad\)\);/,
    );
    const select = phoneRules("\\.rb-drawer-card \\.settings-select-menu");
    expect(select).toHaveLength(1);
    expect(select[0]).toMatch(
      /padding-bottom:\s*calc\(var\(--rb-space-xs\) \+ var\(--rb-safe-sheet-pad\)\);/,
    );
  });

  it("the safe-area term is spelled ONCE — the frame defines it, the arms compose with it", () => {
    // The review's dedup: `env(safe-area-inset-bottom)` appears exactly
    // once across the sheet family's phone rules (the frame's definition);
    // every arm composes `var(--rb-safe-sheet-pad)` instead of re-spelling
    // the env() term.
    const family = [
      ...phoneRules("\\.rb-drawer-card"),
      ...phoneRules("\\.rb-drawer-card \\.dialog-card"),
      ...phoneRules("\\.rb-drawer-card\\.popover-card"),
      ...phoneRules("\\.rb-drawer-card\\.right-plus-menu-sheet"),
      ...phoneRules("\\.rb-drawer-card \\.settings-select-menu"),
    ].join("\n");
    expect(family.match(/env\(safe-area-inset-bottom\)/g)).toHaveLength(1);
  });

  it("the sheet frame itself stays full-bleed: left/right/bottom 0, top-only radius", () => {
    const rules = phoneRules("\\.rb-drawer-card");
    // One frame rule across the phone blocks (the scoped child rules above
    // never match `.rb-drawer-card` alone).
    const frame = rules.find((body) => body.includes("position: fixed"));
    expect(frame).toBeDefined();
    expect(frame).toMatch(/left:\s*0;/);
    expect(frame).toMatch(/right:\s*0;/);
    expect(frame).toMatch(/bottom:\s*0;/);
    expect(frame).toMatch(/border-radius:\s*16px 16px 0 0;/);
    // The frame owns the family's ONE shared safe-area term — every arm
    // composes with `var(--rb-safe-sheet-pad)` (the dedup contract).
    expect(frame).toMatch(/--rb-safe-sheet-pad:\s*env\(safe-area-inset-bottom\);/);
  });

  it("the entrance is present and keyed to [data-open] — the rb-dialog-in reuse", () => {
    const rules = phoneRules("\\.rb-drawer-card\\[data-open\\]");
    expect(rules).toHaveLength(1);
    expect(rules[0]).toMatch(/animation:\s*rb-dialog-in var\(--rb-motion-dialog-in\) var\(--rb-ease-ease\);/);
  });

  it("the phone backstop still excludes in-sheet cards — one width rule per site", () => {
    // Ticket 54's backstop (`.dialog-card:not(.rb-drawer-card *)`) must keep
    // its exclusion: a card inside the sheet gets the full-width rule above,
    // never both.
    const backstop = phoneRules("\\.dialog-card:not\\(\\.rb-drawer-card \\*\\)");
    expect(backstop).toHaveLength(1);
    expect(backstop[0]).toMatch(/width:\s*min\(360px, calc\(100vw - 2 \* var\(--rb-space-lg\)\)\);/);
  });

  it("desktop geometry is untouched — the centered card keeps its own 360px", () => {
    const rule = topLevelRule("\\.dialog-card");
    expect(rule).toMatch(/width:\s*360px;/);
    expect(rule).toMatch(/padding:\s*20px;/);
  });
});
