// @vitest-environment jsdom

/**
 * Ticket 15 (web-bugs-2026-09) — the popover→drawer arm's phone contracts,
 * at the shared-component level:
 *
 * - `PickerCard` at ≤768px renders the bottom sheet (`RbDrawerSheet`) with
 *   the caller's card classes ON the popup — the full-width surface the
 *   sheet frame's phone rules size; `placement`/`width` stay ignored (the
 *   sheet spans the viewport, so no inline width may leak onto it).
 * - The ticket's nested-menu-in-drawer pattern: a `NestedMenu` group row
 *   INSIDE the sheet drills its choices down IN PLACE — the
 *   `.rb-submenu-drill` block expands under the row inside
 *   `.rb-drawer-card`, never a portaled `.rb-popover-popup` side flyout —
 *   and the back header closes it.
 * - The desktop arm regression after the width change: the same Host at
 *   ≥769px renders the portaled floating card and the nested flyout, the
 *   pre-15 tree.
 *
 * The mounted idiom follows project-actions-control.test.ts (controllable
 * matchMedia cell for `useIsPhone`, Base UI running for real, no JSX).
 */

import { act, createElement, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { PHONE_QUERY } from "../src/state/media";
import { PickerCard } from "../src/components/ui/PickerCard";
import { NestedMenu } from "../src/components/ui/NestedMenu";

// ── The mocked viewport (controllable cell) ─────────────────────────────────

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

interface MountedPicker {
  readonly container: HTMLDivElement;
  trigger(): HTMLElement | null;
  groupRow(): HTMLElement | null;
  sheet(): HTMLElement | null;
  drill(): HTMLElement | null;
  /** The nested flyout — portaled to the document (desktop arm only). */
  nestedPopup(): HTMLElement | null;
  unmount(): void;
}

const mounted: MountedPicker[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  h.phone = false;
  document.body.replaceChildren();
});

/**
 * A picker card whose body carries one nested group row — the view-options
 * shape (`PickerCard` + `NestedMenu`, both arms resolved by the components).
 */
function mountPicker(options?: { open?: boolean; nestedOpen?: boolean }): MountedPicker {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const startOpen = options?.open ?? false;
  const startNested = options?.nestedOpen ?? false;

  function Host() {
    const [open, setOpen] = useState(startOpen);
    const [nested, setNested] = useState(startNested);
    return createElement(PickerCard, {
      open,
      onOpenChange: (next: boolean) => {
        setOpen(next);
      },
      placement: "anchorBelow",
      cardClassName: "popover-card spaces-menu-card",
      role: "menu",
      ariaLabel: "Test picker",
      width: 232,
      trigger: createElement(
        "button",
        { type: "button", className: "picker-trigger" },
        "Options",
      ),
      children: [
        createElement(NestedMenu, {
          open: nested,
          onOpenChange: (next: boolean) => {
            setNested(next);
          },
          label: "Show",
          nativeButton: false,
          trigger: createElement(
            "div",
            { className: "menu-row group-row", role: "menuitem" },
            "Show",
          ),
          children: [createElement("button", { type: "button", key: "a" }, "Branches")],
        }),
        createElement("button", { type: "button", key: "plain" }, "Compact"),
      ],
    });
  }

  act(() => {
    root.render(createElement(Host));
  });
  const handle: MountedPicker = {
    container,
    trigger: () => container.querySelector<HTMLElement>(".picker-trigger"),
    // The card body renders inside the surface (the sheet's portal at
    // phone, the popover's at desktop) — query the document, not the
    // mount container.
    groupRow: () => document.querySelector<HTMLElement>(".group-row"),
    sheet: () => document.querySelector<HTMLElement>(".rb-drawer-card"),
    drill: () => document.querySelector<HTMLElement>(".rb-submenu-drill"),
    nestedPopup: () => document.querySelector<HTMLElement>(".rb-popover-popup"),
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

// ── The phone arm ───────────────────────────────────────────────────────────

describe("PickerCard phone arm (ticket 15 regression)", () => {
  it("renders the bottom sheet — the caller's card classes on the popup, no inline width", () => {
    h.phone = true;
    const handle = mountPicker({ open: true });
    const sheet = handle.sheet();
    expect(sheet).not.toBeNull();
    // The card classes land ON the sheet popup (the sheet IS the card at
    // phone): `.popover-card`'s safe-area term composes on this element.
    expect(sheet!.classList.contains("popover-card")).toBe(true);
    expect(sheet!.classList.contains("spaces-menu-card")).toBe(true);
    // The floating form's geometry is inert: `placement`/`width` never put
    // an inline width on the full-width sheet.
    expect(sheet!.getAttribute("role")).toBe("menu");
    expect(sheet!.getAttribute("aria-label")).toBe("Test picker");
    expect(sheet!.style.width).toBe("");
    // The sheet frame carries the entrance key while open.
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    // And the floating card never mounts.
    expect(handle.nestedPopup()).toBeNull();
    expect(document.querySelector(".rb-popover-positioner")).toBeNull();
  });

  it("the trigger's press opens the sheet through the Drawer.Trigger adoption", () => {
    h.phone = true;
    const handle = mountPicker();
    expect(handle.sheet()).toBeNull();
    press(handle.trigger()!);
    const sheet = handle.sheet();
    expect(sheet).not.toBeNull();
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    // The adopted trigger carries the expanded state.
    expect(handle.trigger()!.getAttribute("aria-expanded")).toBe("true");
  });
});

describe("NestedMenu inside the sheet — the drill-down (ticket 15's pattern)", () => {
  it("the group row's press expands its choices in place, never a side flyout", () => {
    h.phone = true;
    const handle = mountPicker({ open: true });
    expect(handle.drill()).toBeNull();
    press(handle.groupRow()!);
    const drill = handle.drill();
    expect(drill).not.toBeNull();
    // In place: the drill is a plain DOM block inside the card's body —
    // which lives inside the sheet — never a portaled popup.
    expect(handle.sheet()!.contains(drill!)).toBe(true);
    expect(document.querySelector(".rb-popover-popup")).toBeNull();
    // The row carries the expanded state.
    expect(handle.groupRow()!.getAttribute("aria-expanded")).toBe("true");
  });

  it("the back header closes the drill (close-press), leaving the sheet open", () => {
    h.phone = true;
    const handle = mountPicker({ open: true });
    press(handle.groupRow()!);
    const header = handle.drill()!.querySelector<HTMLElement>(".rb-submenu-drill-header");
    expect(header).not.toBeNull();
    expect(header!.textContent).toContain("Show");
    press(header!);
    expect(handle.drill()).toBeNull();
    expect(handle.groupRow()!.getAttribute("aria-expanded")).toBe("false");
    // The sheet itself stays up — the drill is a LOCAL dismissal.
    expect(handle.sheet()).not.toBeNull();
    expect(handle.sheet()!.hasAttribute("data-open")).toBe(true);
  });
});

// ── The desktop arm regression ──────────────────────────────────────────────

describe("PickerCard desktop arm (post-width-change regression)", () => {
  it("renders the portaled floating card with the caller's width — no sheet", () => {
    h.phone = false;
    const handle = mountPicker({ open: true });
    expect(handle.sheet()).toBeNull();
    const popup = handle.nestedPopup();
    // The card's own popup: the PickerCard body portals to the document as
    // the `.rb-popover-popup` positioner child.
    expect(popup).not.toBeNull();
    expect(popup!.classList.contains("popover-card")).toBe(true);
    expect(popup!.classList.contains("spaces-menu-card")).toBe(true);
  });

  it("the nested group's flyout portals beside the row (ticket 01's desktop arm)", () => {
    h.phone = false;
    const handle = mountPicker({ open: true, nestedOpen: true });
    // Two popovers mount: the card and the nested flyout — both portaled,
    // the nested one linked after the parent's in DOM order.
    const popups = document.querySelectorAll(".rb-popover-popup");
    expect(popups.length).toBeGreaterThanOrEqual(2);
    expect(handle.drill()).toBeNull();
  });
});
