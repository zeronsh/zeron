// @vitest-environment jsdom

/**
 * Ticket 07 (bug 5b) — the sidebar view-options menu's group submenu must
 * render through ticket 01's `NestedMenu`: a PORTALED flyout on desktop
 * (the old inline-absolute `.view-menu-submenu` lived inside
 * `.popover-card`'s `overflow: hidden` clip box and never painted), the
 * in-place drill-down in the bottom sheet on phone (ticket 15's mobile
 * surface rule), with the card's cursor keyboard model unchanged
 * (`sidebar_view_menu_key` parity: right/enter open the child with the
 * cursor landed, escape/left close the child alone, the card's own escape
 * is the second press). The mounted idiom follows nested-menu.test.ts /
 * project-actions-control.test.ts: no JSX, per-file jsdom pragma, Base UI
 * running for real behind the matchMedia / ResizeObserver / scrollIntoView
 * / rAF stubs.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarViewMenu } from "../src/components/space-filter";
import { PHONE_QUERY } from "../src/state/media";
import { uiSettings } from "../src/state/ui-settings";

// ── jsdom gaps the mounted cards hit (the shared stub set) ─────────────────

/** The useIsPhone answer for every mount in this file (PHONE_QUERY match). */
let phoneMode = false;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    // `(max-width: 768px)` matches in phone mode — the exact query
    // `useIsPhone` derives from the one breakpoint.
    matches: query === PHONE_QUERY && phoneMode,
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

// ── The mounted view-menu harness ───────────────────────────────────────────

interface MountedView {
  /** The host div (the sidebar stand-in; everything floating portals out). */
  readonly container: HTMLDivElement;
  /** The view-options trigger button (adopted by the card / sheet). */
  trigger(): HTMLElement;
  unmount(): void;
}

const mounted: Array<() => void> = [];

beforeEach(() => {
  phoneMode = false;
  // A deterministic selection baseline for the whole file: nothing in the
  // Organize group is selected ("in one list") and the Show toggles rest
  // off, so the cursor's `menu-row-highlighted` wash reads unambiguously
  // off the choice rows and the toggle pick has a known from-state.
  uiSettings.updateImmediate({ sidebarOrganization: "inOneList", sidebarShowBranch: false });
});

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
  document.body.replaceChildren();
});

function mountViewMenu(): MountedView {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(createElement(SidebarViewMenu));
  });
  const unmount = (): void => {
    act(() => {
      root.unmount();
    });
    container.remove();
  };
  mounted.push(unmount);
  return {
    container,
    trigger: () => {
      const button = container.querySelector<HTMLElement>(".space-filter-sort");
      if (button === null) {
        throw new Error("the view-options trigger did not render");
      }
      return button;
    },
    unmount,
  };
}

/** The view-options card while still open (an exiting card keeps painting `data-closed` until its window drains). */
function viewCard(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".rb-popover-popup.spaces-menu-card:not([data-closed])");
}

/** The three group trigger rows, in order (Organize, Sort, Show). */
function groupRows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".view-menu-group-row"));
}

/** The group flyout while still open, by its accessible name. */
function flyout(label: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    `.rb-popover-popup.view-menu-flyout:not([data-closed])[aria-label="${label}"]`,
  );
}

/** A real press pair on `target`: pointerdown (marks the press) then click. */
function press(target: HTMLElement): void {
  act(() => {
    target.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

/** A key on `target` — the card-level cursor model's input path. */
function pressKey(key: string, target: HTMLElement): void {
  act(() => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  });
}

/** Escape through Base UI's document pipeline (the dismiss ladder). */
function pressEscape(): void {
  act(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  });
}

// ── The desktop arm: the portaled flyout ────────────────────────────────────

describe("SidebarViewMenu — the nested group submenu (bug 5b)", () => {
  it("renders the group submenu as a body-portal flyout, never a clipped inline child", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    const card = viewCard();
    expect(card).not.toBeNull();

    press(groupRows()[0]!);
    const child = flyout("Organize");
    expect(child).not.toBeNull();
    // Bug 5b inverted: the old `.view-menu-submenu` painted inside the
    // card's `overflow: hidden` clip box (left: calc(100% + 4px) — entirely
    // outside it, hence invisible); the flyout is not the card's DOM
    // descendant at all — both ride the body portal.
    expect(card!.contains(child!)).toBe(false);
    expect(document.body.contains(child!)).toBe(true);
    // The desktop child's shape: a menu-typed glass card with the group's
    // heading and its rows, riding the shared menu tier.
    expect(child!.getAttribute("role")).toBe("menu");
    expect(child!.classList.contains("popover-card")).toBe(true);
    expect(child!.closest(".rb-popover-positioner")).not.toBeNull();
    expect(child!.querySelector(".menu-heading")!.textContent).toBe("Organize");
    expect(
      Array.from(child!.querySelectorAll(".menu-row-label")).map((row) => row.textContent),
    ).toEqual(["By device", "By project", "In one list"]);
    // The adopted trigger row carries the expanded state; the card stays
    // open — the flyout is part of its popup tree, not a dismissal target.
    expect(groupRows()[0]!.getAttribute("aria-expanded")).toBe("true");
    expect(viewCard()).not.toBeNull();
  });

  it("hover opens the flyout after the intent delay, and the corridor owns the close", () => {
    // The desktop's nested menus open on hover intent (the row's
    // `hover_sidebar_view_group`); ticket 01's component encodes the same
    // contract on Base UI's own hover layer — only setTimeout is faked,
    // the positioner's rAF stays real.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const handle = mountViewMenu();
      press(handle.trigger());
      const row = groupRows()[0]!;
      act(() => {
        row.dispatchEvent(new MouseEvent("mouseenter"));
        row.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
      });
      // Still inside the 100ms rest window — nothing opened yet.
      expect(flyout("Organize")).toBeNull();
      act(() => {
        vi.advanceTimersByTime(120);
      });
      expect(flyout("Organize")).not.toBeNull();
      // Leaving the row and the flyout behind (far outside the corridor)
      // dismisses after the close grace; the card never wavers.
      act(() => {
        row.dispatchEvent(new MouseEvent("mouseleave", { clientX: 999, clientY: 999 }));
      });
      act(() => {
        vi.advanceTimersByTime(120);
      });
      expect(flyout("Organize")).toBeNull();
      expect(viewCard()).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keyboard: right opens the child with the cursor landed, down walks it, enter picks", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    const card = viewCard()!;

    // ArrowDown walks the top rows onto Organize; ArrowRight opens the
    // child (`sidebar_view_menu_key`) with its cursor landed on row 0.
    pressKey("ArrowDown", card);
    pressKey("ArrowRight", card);
    const child = flyout("Organize");
    expect(child).not.toBeNull();
    const choices = Array.from(child!.querySelectorAll(".menu-row"));
    expect(choices[0]!.classList.contains("menu-row-highlighted")).toBe(true);
    expect(choices[1]!.classList.contains("menu-row-highlighted")).toBe(false);

    // Down moves the cursor to the second row.
    pressKey("ArrowDown", card);
    expect(choices[1]!.classList.contains("menu-row-highlighted")).toBe(true);
    expect(choices[0]!.classList.contains("menu-row-highlighted")).toBe(false);

    // Enter activates the cursor row — a radio row, so the child closes
    // and the card stays open for the next adjustment.
    pressKey("Enter", card);
    expect(flyout("Organize")).toBeNull();
    expect(viewCard()).not.toBeNull();
    expect(groupRows()[0]!.querySelector(".view-menu-summary")!.textContent).toBe("By project");
  });

  it("escape through Base UI's pipeline closes the flyout alone, then the card on the second press", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    press(groupRows()[0]!);
    expect(flyout("Organize")).not.toBeNull();

    pressEscape();
    expect(flyout("Organize")).toBeNull();
    expect(viewCard()).not.toBeNull();

    pressEscape();
    expect(viewCard()).toBeNull();
  });

  it("the card's own escape branch closes the child and keeps the row cursor (sidebar_view_menu_key)", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    const card = viewCard()!;
    pressKey("ArrowDown", card);
    pressKey("ArrowRight", card);
    expect(flyout("Organize")).not.toBeNull();

    pressKey("Escape", card);
    expect(flyout("Organize")).toBeNull();
    expect(viewCard()).not.toBeNull();
    // The desktop's key branch keeps the row cursor: the group row stays
    // highlighted through an escape close (spaces.rs's `escape` arm clears
    // the submenu only, never `active`).
    expect(groupRows()[0]!.classList.contains("menu-row-highlighted")).toBe(true);

    // The second escape belongs to the card itself (Base UI's dismiss).
    pressKey("Escape", card);
    expect(viewCard()).toBeNull();
  });

  it("a press on a sibling group swaps the open flyout for that sibling's", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    press(groupRows()[0]!);
    expect(flyout("Organize")).not.toBeNull();

    // The press lands outside the open flyout (it dismisses) and on the
    // sibling's own trigger (it opens) — one `submenu` state, replaced.
    press(groupRows()[1]!);
    expect(flyout("Organize")).toBeNull();
    expect(flyout("Sort")).not.toBeNull();
    expect(
      Array.from(flyout("Sort")!.querySelectorAll(".menu-row-label")).map((row) => row.textContent),
    ).toEqual(["Last updated", "Created"]);
    expect(viewCard()).not.toBeNull();
  });

  it("a toggle pick inside the flyout applies and keeps the child open (multi-adjust)", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    press(groupRows()[2]!);
    const child = flyout("Show");
    expect(child).not.toBeNull();
    const branch = Array.from(child!.querySelectorAll<HTMLElement>(".menu-row")).find(
      (row) => row.querySelector(".menu-row-label")!.textContent === "Branch",
    )!;
    press(branch);
    // The toggle applied…
    expect(branch.getAttribute("aria-selected")).toBe("true");
    // …and the child stays open for the next toggle — `closesSubmenu` is
    // the radio rows' rule only (`activate_sidebar_view_row`).
    expect(flyout("Show")).not.toBeNull();
    expect(viewCard()).not.toBeNull();
  });

  it("closing the card resets the nested state — reopening starts clean", () => {
    const handle = mountViewMenu();
    press(handle.trigger());
    press(groupRows()[0]!);
    expect(flyout("Organize")).not.toBeNull();

    press(handle.trigger());
    expect(viewCard()).toBeNull();
    press(handle.trigger());
    expect(viewCard()).not.toBeNull();
    expect(flyout("Organize")).toBeNull();
    expect(groupRows()[0]!.getAttribute("aria-expanded")).toBe("false");
  });

  // ── The phone arm: the drill-down (ticket 15's mobile surface rule) ──────

  it("phone: the choices expand in place under the row (the drill-down), never a flyout portal", () => {
    phoneMode = true;
    const handle = mountViewMenu();
    press(handle.trigger());
    const sheet = document.querySelector<HTMLElement>(".rb-drawer-card");
    expect(sheet).not.toBeNull();

    press(groupRows()[0]!);
    const drill = sheet!.querySelector<HTMLElement>(".rb-submenu-drill");
    expect(drill).not.toBeNull();
    // No Base UI flyout ever mounts at phone width — the choices render
    // inside the sheet, under the row.
    expect(document.querySelector(".rb-popover-popup.view-menu-flyout")).toBeNull();
    expect(drill!.querySelector(".rb-submenu-drill-header")!.textContent).toContain("Organize");
    expect(
      Array.from(drill!.querySelectorAll(".rb-submenu-drill-body .menu-row-label")).map(
        (row) => row.textContent,
      ),
    ).toEqual(["By device", "By project", "In one list"]);

    // A radio pick inside the drill applies and closes it.
    press(drill!.querySelector<HTMLElement>(".rb-submenu-drill-body .menu-row")!);
    expect(sheet!.querySelector(".rb-submenu-drill")).toBeNull();
    expect(groupRows()[0]!.querySelector(".view-menu-summary")!.textContent).toBe("By device");
    expect(document.querySelector(".rb-drawer-card")).not.toBeNull();
  });

  it("phone: the drill's back header closes it while the sheet stays up", () => {
    phoneMode = true;
    const handle = mountViewMenu();
    press(handle.trigger());
    press(groupRows()[1]!);
    const drill = document.querySelector<HTMLElement>(".rb-submenu-drill");
    expect(drill).not.toBeNull();

    press(drill!.querySelector<HTMLElement>(".rb-submenu-drill-header")!);
    expect(document.querySelector(".rb-submenu-drill")).toBeNull();
    expect(document.querySelector(".rb-drawer-card")).not.toBeNull();
  });
});
