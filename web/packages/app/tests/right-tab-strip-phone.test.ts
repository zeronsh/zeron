// @vitest-environment jsdom

/**
 * Ticket 15 (web-bugs-2026-09) — the right pane's `+` menu phone arm, the
 * popover→drawer adoption the ticket keeps: pressing the strip's `+` opens
 * the shared bottom sheet (`.rb-drawer-card.right-plus-menu-sheet`, the
 * full-width form whose safe-area term the responsive-surface CSS
 * contracts pin), the sheet carries `[data-open]` (the `rb-dialog-in`
 * entrance's key), a row pick routes through the real right-pane store and
 * closes the sheet, and the pane-closing effect (the sheet leaves with the
 * drawer) still holds after the width change.
 *
 * The mounted harness is phone-drawer-titlebar-clearance.test.ts's: the
 * REAL `RightPane` (its strip, its `+` arm, the real right-pane store)
 * under a controllable matchMedia, with the surface bodies stubbed — the
 * pane HOST is the unit under test, not any surface's body.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { RightPane, usePaneGlide } from "../src/components/right-pane";
import { rightPaneStore, surfaceKey, useRightPane } from "../src/state/right-pane";

// The surface bodies (Changes/Files/Terminal/History mounts) drag xterm and
// the engine-session providers with them; the pane HOST and its strip need
// only the registry's chrome half. `useGitDetected` is forced on so the
// git-gated rows (Diffs/History) render — their store route is the
// assertable one (the terminal dock is not wired under jsdom, so the
// Terminal row's pick is a no-op there).
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

vi.mock("../src/components/surface-picker", async (importOriginal) => {
  const actual = await importOriginal<
    typeof import("../src/components/surface-picker")
  >();
  return { ...actual, useGitDetected: () => true };
});

// ── The mocked phone viewport ────────────────────────────────────────────────

/** The layer `matchMedia` reports: `phone` is the `(max-width: 768px)` arm. */
const media = { phone: true };

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
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  }) as unknown as typeof window.matchMedia;
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
  delete (globalThis as { matchMedia?: unknown }).matchMedia;
});

// ── The mounted pane harness ─────────────────────────────────────────────────

interface MountedPane {
  readonly container: HTMLDivElement;
  addButton(): HTMLElement | null;
  /** The `+` sheet while open (portal-rendered — query the document). */
  sheet(): HTMLElement | null;
  /** `true` while the sheet is mounted AND open (jsdom drains the exit
   *  instantly — a closed sheet unmounts, so absence reads closed). */
  sheetOpen(): boolean;
  unmount(): void;
}

const mounted: MountedPane[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  document.body.replaceChildren();
  media.phone = true;
});

/** The real `RightPane` at the mocked layer, with one tab already present. */
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
  // Settle the fleet/ui-settings microtasks the strip's `+` arm reads.
  await act(async () => {});
  const handle: MountedPane = {
    container,
    addButton: () => document.querySelector<HTMLElement>("#right-surface-add"),
    sheet: () => document.querySelector<HTMLElement>(".rb-drawer-card.right-plus-menu-sheet"),
    sheetOpen: () =>
      document
        .querySelector<HTMLElement>(".rb-drawer-card.right-plus-menu-sheet:not([data-closed])") !== null,
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

describe("right pane `+` menu phone arm (ticket 15 regression)", () => {
  it("pressing the strip's `+` opens the shared bottom sheet with its entrance key", async () => {
    const chatId = "chat-plus-open";
    rightPaneStore.addDiffSurface(chatId, "diff");
    const handle = await mountPane(chatId);

    // The sheet is closed: no portal content.
    expect(handle.sheet()).toBeNull();

    press(handle.addButton()!);
    const sheet = handle.sheet();
    expect(sheet).not.toBeNull();
    // The full-width form the CSS owns: the sheet frame with the `+` menu's
    // own class, carrying `[data-open]` — the attribute `rb-dialog-in`
    // keys on (`.rb-drawer-card[data-open]`).
    expect(sheet!.classList.contains("rb-drawer-card")).toBe(true);
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    expect(sheet!.getAttribute("role")).toBe("menu");
    expect(sheet!.getAttribute("aria-label")).toBe("Add panel surface");

    // The press-was-open toggle: a second press closes it again.
    press(handle.addButton()!);
    expect(handle.sheetOpen()).toBe(false);
  });

  it("a row pick routes through the real store and closes the sheet", async () => {
    const chatId = "chat-plus-pick";
    rightPaneStore.addDiffSurface(chatId, "diff");
    const handle = await mountPane(chatId);
    press(handle.addButton()!);

    // The fleet snapshot's git probe is forced on, so the git-gated rows
    // render — Diffs routes to `addDiffSurface`, the store-backed pick.
    const rows = handle.sheet()!.querySelectorAll<HTMLElement>(".right-plus-menu-row");
    expect(rows.length).toBeGreaterThanOrEqual(2);
    const diffs = Array.from(rows).find((row) => row.textContent!.includes("Diffs"));
    expect(diffs).toBeDefined();

    const before = rightPaneStore.stateFor(chatId).tabs.length;
    press(diffs!);

    // The pick opened a diff surface through the store…
    expect(rightPaneStore.stateFor(chatId).tabs.length).toBe(before + 1);
    expect(
      rightPaneStore.surfaceRows(chatId).filter((entry) => entry.surface.kind === "diff").length,
    ).toBeGreaterThanOrEqual(2);
    // …and the sheet the row dismisses stays dismissed.
    expect(handle.sheetOpen()).toBe(false);
    void surfaceKey;
  });

  it("the sheet leaves with the pane: closing the drawer closes an open sheet", async () => {
    const chatId = "chat-plus-close";
    rightPaneStore.addDiffSurface(chatId, "diff");
    const handle = await mountPane(chatId);
    press(handle.addButton()!);
    expect(handle.sheetOpen()).toBe(true);

    // The pane-closing effect: the menu belongs to the strip, so it must
    // not linger over the closed drawer.
    act(() => {
      rightPaneStore.close(chatId);
    });
    await act(async () => {});
    expect(handle.sheetOpen()).toBe(false);
  });
});
