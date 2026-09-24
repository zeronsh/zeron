// @vitest-environment jsdom

/**
 * Ticket 18 — `sidebar-sections.tsx`'s SectionMenu on the shared menu
 * family: the old hand-roll was window mousedown/keydown listeners plus
 * an inline absolute `.section-context-menu` painted inside
 * `<aside class="sidebar">` — clipped by its `overflow: hidden`, the same
 * bug class as the user menu (ticket 02) — with raw `<button role=menuitem>`
 * rows. The swap rides `PickerCard` (`anchorBelowEnd`, the card under the
 * header's kebab) with `MenuRow` rows; the mounted contracts here pin:
 *
 * - The card paints in the BODY portal — outside any clipping host —
 *   under the shared `.popover-card` frame (the `section-menu-body`
 *   content specifics), with `menu-row` rows, not raw buttons.
 * - The dismissal pipeline is Base UI's now: an outside press and Escape
 *   route `onOpenChange(false)` with no window listeners of the
 *   component's own; a row pick routes its action and closes.
 * - The phone arm: the same rows in the shared bottom sheet
 *   (`RbDrawerSheet`, the PickerCard phone arm) instead of the clipped
 *   inline card.
 *
 * The mounted idiom follows nested-menu.test.ts: no JSX, per-file jsdom
 * pragma, Base UI popovers in jsdom behind the matchMedia /
 * ResizeObserver / scrollIntoView / rAF stubs.
 */

import { act, createElement, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { SectionMenu } from "../src/components/sidebar-sections";

// ── jsdom gaps the mounted cards hit (nested-menu.test.ts's set) ──────────

/** The useIsPhone answer for every mount in this file (PHONE_QUERY match). */
let phoneMode = false;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    // `(max-width: 768px)` matches in phone mode; `(min-width: 769px)` in
    // desktop mode — the exact pair `state/media.ts` derives from the one
    // breakpoint, so the two queries can never disagree.
    matches: query.startsWith("(max-width") === phoneMode,
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

// ── The mounted menu harness ──────────────────────────────────────────────

interface MountedMenu {
  /** The clipping host the section header renders inside (the `.sidebar`
   *  stand-in whose `overflow: hidden` used to eat the inline card). */
  readonly container: HTMLDivElement;
  readonly calls: string[];
  setOpen(next: boolean): void;
  unmount(): void;
}

const mounted: MountedMenu[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  phoneMode = false;
  document.body.replaceChildren();
});

/** Mount the SectionMenu controlled, inside a clipping host; record every
 *  routed callback (`open:<bool>` / the three actions). */
function mountSectionMenu(): MountedMenu {
  const container = document.createElement("div");
  // The stand-in for `<aside class="sidebar">`: the old inline absolute
  // card painted INSIDE this box and was clipped by its overflow.
  container.style.overflow = "hidden";
  container.style.height = "40px";
  document.body.appendChild(container);
  const root = createRoot(container);
  const calls: string[] = [];
  function Host() {
    const [open, setOpenState] = useState(true);
    (globalThis as { __sectionSetOpen?: (next: boolean) => void }).__sectionSetOpen = setOpenState;
    return createElement(SectionMenu, {
      sectionName: "Work",
      open,
      onOpenChange: (next: boolean) => {
        calls.push(`open:${next}`);
        setOpenState(next);
      },
      onEdit: () => calls.push("edit"),
      onArchiveAll: () => calls.push("archive-all"),
      onDelete: () => calls.push("delete"),
    });
  }
  act(() => {
    root.render(createElement(Host));
  });
  const handle: MountedMenu = {
    container,
    calls,
    setOpen(next: boolean) {
      act(() => {
        (globalThis as { __sectionSetOpen?: (next: boolean) => void }).__sectionSetOpen!(next);
      });
    },
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

/** A real press pair on `target`: pointerdown (marks the press) then click. */
function press(target: HTMLElement): void {
  act(() => {
    target.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

/** The open card at the CURRENT viewport: the portaled picker card at
 *  ≥769px, the bottom sheet at ≤768px. */
function sectionCard(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.rb-popover-popup[role="menu"]');
}

// ── The contracts ─────────────────────────────────────────────────────────

describe("SectionMenu on the shared menu family (ticket 18)", () => {
  it("desktop: the card paints in the body portal — outside the clipping host — with MenuRow rows", () => {
    const handle = mountSectionMenu();
    // The kebab trigger renders where the header shows it.
    const kebab = handle.container.querySelector<HTMLElement>(".sidebar-section-menu-button");
    expect(kebab).not.toBeNull();
    // The card: the shared popover frame + the section-menu-body content
    // specifics — and it lives OUTSIDE the clipping host, the fix for the
    // sidebar-overflow clipping the inline card suffered.
    const card = sectionCard();
    expect(card).not.toBeNull();
    expect(card!.classList.contains("popover-card")).toBe(true);
    expect(card!.classList.contains("section-menu-body")).toBe(true);
    expect(handle.container.contains(card!)).toBe(false);
    expect(card!.closest("body")).toBe(document.body);
    // The rows are the shared MenuRow recipe (button.menu-row with the
    // fade key), not the raw menuitem buttons the hand-roll carried.
    const keys = Array.from(card!.querySelectorAll<HTMLElement>(".menu-row")).map(
      (row) => row.dataset.rbRowKey,
    );
    expect(keys).toEqual(["section-edit", "section-archive-all", "section-delete"]);
    // The old hand-rolled class is gone.
    expect(document.querySelector(".section-context-menu")).toBeNull();
  });

  it("a row pick routes its action and closes the menu", () => {
    const handle = mountSectionMenu();
    const editRow = document.querySelector<HTMLElement>('[data-rb-row-key="section-edit"]');
    expect(editRow).not.toBeNull();
    press(editRow!);
    expect(handle.calls).toEqual(["open:false", "edit"]);
  });

  it("an outside press dismisses through Base UI's pipeline — no window listeners of the component's own", () => {
    const handle = mountSectionMenu();
    // A press on the bare body (nowhere near the card or the kebab) reads
    // as outside and dismisses.
    press(document.body);
    expect(handle.calls).toEqual(["open:false"]);
  });

  it("Escape dismisses — the window keydown listener is Base UI's now", () => {
    const handle = mountSectionMenu();
    expect(sectionCard()).not.toBeNull();
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(handle.calls).toEqual(["open:false"]);
  });

  it("the controlled seam still owns the mount: closing unmounts the card", () => {
    const handle = mountSectionMenu();
    expect(sectionCard()).not.toBeNull();
    handle.setOpen(false);
    expect(sectionCard()).toBeNull();
  });

  it("phone: the same rows in the shared bottom sheet — the arm the clipped inline card never had", () => {
    phoneMode = true;
    const handle = mountSectionMenu();
    const sheet = document.querySelector<HTMLElement>('.rb-drawer-card[role="menu"]');
    expect(sheet).not.toBeNull();
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    expect(sheet!.classList.contains("section-menu-body")).toBe(true);
    // The sheet is portaled — and full-width (the sheet card recipe), not
    // the 180px anchored card the desktop arm renders.
    expect(handle.container.contains(sheet!)).toBe(false);
    const keys = Array.from(sheet!.querySelectorAll<HTMLElement>(".menu-row")).map(
      (row) => row.dataset.rbRowKey,
    );
    expect(keys).toEqual(["section-edit", "section-archive-all", "section-delete"]);
  });
});
