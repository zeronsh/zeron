// @vitest-environment jsdom

/**
 * Ticket 15 (web-bugs-2026-09) — the sidebar view-options surface at phone:
 * the group rows (Organize/Sort/Show) must DRILL DOWN — the choices expand
 * IN PLACE inside the drawer sheet under the back-affordance header, never
 * the absolute `.view-menu-submenu` side-flyout card the desktop arm paints
 * (and which, before this ticket, a phone tap could never even reach: the
 * tap's synthetic hover armed the deferred open and the row's own
 * click-dismiss cancelled it in the same press).
 *
 * The surface rides the real components end to end: `SidebarViewMenu` →
 * `PickerCard`'s phone arm (`RbDrawerSheet`) → `NestedMenu`'s phone arm
 * (`.rb-submenu-drill`), with the real sidebar/ui-settings stores fielding
 * the picks. The mounted idiom follows project-actions-control.test.ts
 * (controllable matchMedia cell, Base UI for real, no JSX).
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { PHONE_QUERY } from "../src/state/media";
import { SidebarViewMenu } from "../src/components/space-filter";
import { sidebarStore } from "../src/state/sidebar";

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

interface MountedMenu {
  readonly container: HTMLDivElement;
  trigger(): HTMLElement | null;
  /** The view-options sheet while mounted (portal-rendered). */
  sheet(): HTMLElement | null;
  groupRow(label: string): HTMLElement | null;
  drill(): HTMLElement | null;
  unmount(): void;
}

const mounted: MountedMenu[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  h.phone = false;
  document.body.replaceChildren();
});

function mountViewMenu(): MountedMenu {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(createElement(SidebarViewMenu));
  });
  const handle: MountedMenu = {
    container,
    trigger: () => container.querySelector<HTMLElement>(".space-filter-sort"),
    sheet: () => document.querySelector<HTMLElement>(".rb-drawer-card"),
    // The group rows render inside the sheet's portal — query the document.
    groupRow: (label: string) =>
      Array.from(document.querySelectorAll<HTMLElement>(".view-menu-group-row")).find((row) =>
        row.textContent!.startsWith(label),
      ) ?? null,
    drill: () => document.querySelector<HTMLElement>(".rb-submenu-drill"),
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

// ── The phone arm: the drill-down ───────────────────────────────────────────

describe("SidebarViewMenu phone arm (ticket 15)", () => {
  it("the sheet opens from the trigger, and a group row drills its choices down in place", () => {
    h.phone = true;
    const handle = mountViewMenu();
    expect(handle.sheet()).toBeNull();
    press(handle.trigger()!);
    const sheet = handle.sheet();
    expect(sheet).not.toBeNull();
    expect(sheet!.hasAttribute("data-open")).toBe(true);

    // Three group rows render inside the sheet, summaries and chevrons and
    // all — the same row recipe the desktop arm paints.
    for (const label of ["Organize", "Sort", "Show"]) {
      expect(handle.groupRow(label)).not.toBeNull();
    }

    // The drill-down: the Show row's press expands its choices IN PLACE…
    expect(handle.drill()).toBeNull();
    press(handle.groupRow("Show")!);
    const drill = handle.drill();
    expect(drill).not.toBeNull();
    // …inside the sheet, never a side flyout: the drill is plain DOM inside
    // the drawer card, and neither the desktop arm's absolute submenu card
    // nor any portaled popover mounts.
    expect(sheet!.contains(drill!)).toBe(true);
    expect(document.querySelector(".view-menu-submenu")).toBeNull();
    expect(document.querySelector(".rb-popover-popup")).toBeNull();
    // The row carries the expanded state; the back header reads the group.
    expect(handle.groupRow("Show")!.getAttribute("aria-expanded")).toBe("true");
    expect(drill!.querySelector(".rb-submenu-drill-header")!.textContent).toContain("Show");
    expect(drill!.querySelectorAll(".rb-submenu-drill-body .menu-row").length).toBe(5);
  });

  it("the back header closes the drill; the sheet stays up (a local dismissal)", () => {
    h.phone = true;
    const handle = mountViewMenu();
    press(handle.trigger()!);
    press(handle.groupRow("Show")!);
    expect(handle.drill()).not.toBeNull();
    press(handle.drill()!.querySelector<HTMLElement>(".rb-submenu-drill-header")!);
    expect(handle.drill()).toBeNull();
    expect(handle.groupRow("Show")!.getAttribute("aria-expanded")).toBe("false");
    expect(handle.sheet()).not.toBeNull();
    expect(handle.sheet()!.hasAttribute("data-open")).toBe(true);
  });

  it("a toggle pick applies through the real stores and keeps the drill open for multi-adjust", () => {
    h.phone = true;
    const handle = mountViewMenu();
    press(handle.trigger()!);
    press(handle.groupRow("Show")!);
    const before = sidebarStore.getSnapshot().showBranch;
    // The Show group's first row is Branch (ShowBranch's toggle).
    const branch = handle.drill()!.querySelectorAll<HTMLElement>(".rb-submenu-drill-body .menu-row")[0]!;
    expect(branch.textContent).toContain("Branch");
    press(branch);
    expect(sidebarStore.getSnapshot().showBranch).toBe(!before);
    // Toggle rows keep the submenu open (multi-adjust) — the drill stays.
    expect(handle.drill()).not.toBeNull();
  });

  it("a radio pick applies and closes the drill (the desktop's closesSubmenu rule)", () => {
    h.phone = true;
    const handle = mountViewMenu();
    press(handle.trigger()!);
    press(handle.groupRow("Organize")!);
    const byProject = handle.drill()!.querySelectorAll<HTMLElement>(".rb-submenu-drill-body .menu-row")[1]!;
    expect(byProject.textContent).toContain("By project");
    press(byProject);
    expect(sidebarStore.getSnapshot().organization).toBe("byProject");
    // Organization rows close the submenu — the drill collapses, the sheet
    // stays for the next adjustment.
    expect(handle.drill()).toBeNull();
    expect(handle.sheet()!.hasAttribute("data-open")).toBe(true);
  });
});

// ── The desktop arm regression ──────────────────────────────────────────────

describe("SidebarViewMenu desktop arm (post-change regression)", () => {
  it("renders the portaled floating card — no sheet, no drill", () => {
    h.phone = false;
    const handle = mountViewMenu();
    press(handle.trigger()!);
    // The card is the portaled popover, never the drawer sheet…
    expect(handle.sheet()).toBeNull();
    expect(document.querySelector(".rb-popover-popup")).not.toBeNull();
    // …and the phone drill never mounts: the group row's press rides the
    // desktop arm's own semantics (hover opens, click dismisses).
    expect(handle.drill()).toBeNull();
    press(handle.groupRow("Show")!);
    expect(handle.drill()).toBeNull();
  });
});
