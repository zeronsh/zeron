// @vitest-environment jsdom

/**
 * Ticket 02 (web-bugs-2026-09, research Bug 2): the sidebar account
 * control's menu must actually paint. The card used to mount inline
 * (absolute, `left: calc(100% + 6px)`) inside `<aside class="sidebar">`,
 * whose `overflow: hidden` (the collapse clip) cut it off entirely — the
 * press looked dead while the state and navigation worked. The fix: the
 * card rides `PickerCard`'s `anchorRight` variant (the desktop's
 * `anchored_menu_right`, shell.rs:6430), which portals it to
 * `document.body` through Base UI's Popover — unclipped by any column.
 *
 * The REAL AccountRow mounts inside a real `<aside class="sidebar">`
 * stand-in (the mounted-suite idiom — Base UI runs for real in jsdom, its
 * portals into document.body and all). The fleet/session/snapshot layers
 * are doubled narrowly (exactly the rows the identity resolution reads),
 * and the router's navigate is a recording double. No JSX (createElement),
 * per-file jsdom pragma only.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AccountRow } from "../src/components/account-row";
import { PHONE_QUERY } from "../src/state/media";

// ── Controllable doubles ──────────────────────────────────────────────────

const h = vi.hoisted(() => {
  /** The active engine's registry row — the label the identity falls back to. */
  const engines = [{ baseUrl: "local", label: "Local Engine" }];

  /** The identity the row carries: the engine's own device, "Vu's Studio". */
  const session = {
    client: { engineInfo: { deviceId: "engine:v1:dev-a" } },
  };

  /** The watch snapshot the device row resolves from. */
  const snapshot = {
    devices: { rows: [{ id: "engine:v1:dev-a", name: "Vu's Studio" }] },
  };

  /** `useNavigate` double — records every route the menu lands on. */
  const navigateCalls: Array<{ to: string }> = [];

  const cells = {
    /** The viewport arm: `true` arms `(max-width: 768px)` in the matchMedia stub. */
    phone: false,
  };

  const browserSession: {
    authenticated: boolean;
    profile?: { firstName?: string; lastName?: string; email?: string; avatarUrl?: string };
  } = { authenticated: true };

  const signOutCalls: number[] = [];
  const signOut = (): Promise<void> => {
    signOutCalls.push(1);
    return Promise.resolve();
  };

  return { engines, session, snapshot, navigateCalls, cells, browserSession, signOut, signOutCalls };
});

vi.mock("../src/state/fleet", () => ({
  // Exactly what AccountRow reads: the active engine (the routing key) and
  // the registry rows the label fallback resolves from.
  useFleet: () => ({ active: "local", engines: h.engines, session: h.browserSession, configurationError: null }),
  signOut: h.signOut,
}));

vi.mock("../src/state/session-provider", () => ({
  // The active engine's session — only `client.engineInfo.deviceId` is read
  // on this path; the real provider's notification machinery is unrelated
  // to the clipping contract under test.
  useEngineSessions: () => new Map([["local", h.session]]),
}));

vi.mock("../src/state/hooks", () => ({
  // The device row the identity resolves from — a real snapshot's shape.
  useWatchSnapshot: () => h.snapshot,
}));

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => (options: { to: string }): Promise<void> => {
    h.navigateCalls.push({ to: options.to });
    return Promise.resolve();
  },
}));

// ── jsdom gaps the mounted cards hit (composer-reasoning.test.ts's set) ──────

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: query === PHONE_QUERY && h.cells.phone,
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

// ── The mounted AccountRow harness ─────────────────────────────────────────

interface MountedAccountRow {
  /** The clipping column — `<aside class="sidebar">`'s exact shape (app.css:1355-1361). */
  readonly sidebar: HTMLElement;
  /** The avatar trigger, rendered inside the clipping column. */
  trigger(): HTMLButtonElement;
  /** The user menu card, wherever it portals to. */
  card(): HTMLElement | null;
  unmount(): void;
}

const mounted: Array<() => void> = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
  document.body.replaceChildren();
  h.cells.phone = false;
  h.navigateCalls.length = 0;
});

function mountAccountRow(): MountedAccountRow {
  const sidebar = document.createElement("aside");
  sidebar.className = "sidebar";
  document.body.appendChild(sidebar);
  const root = createRoot(sidebar);
  act(() => {
    root.render(createElement(AccountRow));
  });
  const unmount = (): void => {
    act(() => {
      root.unmount();
    });
    sidebar.remove();
  };
  mounted.push(unmount);
  return {
    sidebar,
    trigger: () => {
      const button = sidebar.querySelector<HTMLButtonElement>(".user-menu-trigger");
      if (button === null) {
        throw new Error("the account trigger did not render");
      }
      return button;
    },
    // Portal-rendered: queried on the document, never the sidebar. The
    // exiting `[data-closed]` card does not count as open.
    card: () =>
      document.querySelector<HTMLElement>(".user-menu-body:not([data-closed])"),
    unmount,
  };
}

/** A real press pair on `target`: pointerdown (marks the press) then click. */
function press(target: HTMLElement): void {
  act(() => {
    target.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

function pressEscape(): void {
  act(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  });
}

// ── The desktop arm: the portaled user menu ─────────────────────────────────

describe("AccountRow — the user menu escapes the clipping sidebar (bug 2)", () => {
  it("portals the card to the body, anchored right — never inside the aside", () => {
    const handle = mountAccountRow();
    // Headless while closed: no card anywhere, and the trigger carries the
    // row identity (the device's name in the label and the aria label, its
    // initial in the avatar).
    expect(handle.card()).toBeNull();
    const trigger = handle.trigger();
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
    expect(trigger.getAttribute("aria-label")).toBe("Account menu: Vu's Studio");
    expect(trigger.querySelector(".avatar")!.textContent).toBe("V");
    expect(trigger.querySelector(".user-menu-label")!.textContent).toBe("Vu's Studio");

    press(trigger);

    // The portal is the fix (bug 2's root cause inverted): the card paints
    // in the body, not inside the `overflow: hidden` sidebar — where the
    // old inline `.user-menu-card` (absolute, left: calc(100% + 6px)) was
    // fully clipped and the press read as dead.
    const card = handle.card();
    expect(card).not.toBeNull();
    expect(handle.sidebar.contains(card!)).toBe(false);
    expect(document.body.contains(card!)).toBe(true);
    // The frame is the shared popover-card glass on the menu tier; the
    // placement rides `anchorRight` (`anchored_menu_right`, shell.rs:6430)
    // through the positioner.
    expect(card!.classList.contains("rb-popover-popup")).toBe(true);
    expect(card!.classList.contains("popover-card")).toBe(true);
    expect(card!.closest(".rb-popover-positioner")).not.toBeNull();
    expect(card!.getAttribute("role")).toBe("menu");
    expect(card!.getAttribute("aria-label")).toBe("Account menu");
    // The trigger carries the expanded state through Base UI's adoption.
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    // Desktop parity, unchanged content: the muted identity line, then the
    // single Settings row.
    expect(card!.querySelector(".user-menu-identity")!.textContent).toBe("Stored on this device");
    expect(card!.querySelector(".menu-item")!.textContent).toContain("Settings");
  });

  it("carries the signed-in WorkOS account's profile — avatar, name, email", () => {
    h.browserSession = {
      authenticated: true,
      profile: {
        firstName: "Vu",
        lastName: "Khanh",
        email: "vu@example.com",
        avatarUrl: "https://example.com/vu.png",
      },
    };
    try {
      const handle = mountAccountRow();
      const trigger = handle.trigger();
      // The account leads the identity: the profile's name in the row's
      // label and the aria label, its picture in the avatar circle.
      expect(trigger.getAttribute("aria-label")).toBe("Account menu: Vu Khanh");
      const image = trigger.querySelector<HTMLImageElement>(".avatar .avatar-image");
      expect(image).not.toBeNull();
      expect(image!.getAttribute("src")).toBe("https://example.com/vu.png");
      expect(trigger.querySelector(".user-menu-label")!.textContent).toBe("Vu Khanh");

      press(trigger);

      const card = handle.card()!;
      // The card's identity block is the signed-in account, not the muted
      // device line.
      expect(card.querySelector(".user-menu-account-name")!.textContent).toBe("Vu Khanh");
      expect(card.querySelector(".user-menu-account-email")!.textContent).toBe("vu@example.com");
      expect(card.querySelector(".user-menu-identity")).toBeNull();
    } finally {
      h.browserSession = { authenticated: true };
    }
  });

  it("carries a Sign out row that signs out of the browser session", async () => {
    const handle = mountAccountRow();
    press(handle.trigger());
    const rows = Array.from(handle.card()!.querySelectorAll<HTMLButtonElement>(".menu-item"));
    const row = rows.find((button) => button.textContent?.includes("Sign out"));
    expect(row).toBeDefined();

    await act(async () => {
      row!.click();
    });

    // The row closes the card first, then revokes the session through the
    // fleet's signOut.
    expect(h.signOutCalls.length).toBe(1);
    expect(handle.card()).toBeNull();
  });

  it("closes on Escape and on an outside press", () => {
    // The outside target must exist BEFORE the popover opens: Base UI marks
    // the pre-existing outside elements `data-base-ui-inert` when the popup
    // opens, and a target injected after open reads as a third-party
    // element the dismissal pass deliberately ignores. Real app DOM exists
    // before any popover opens, so this is the faithful shape.
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const handle = mountAccountRow();

    press(handle.trigger());
    expect(handle.card()).not.toBeNull();
    pressEscape();
    expect(handle.card()).toBeNull();
    expect(handle.trigger().getAttribute("aria-expanded")).toBe("false");

    press(handle.trigger());
    expect(handle.card()).not.toBeNull();
    press(outside);
    expect(handle.card()).toBeNull();
    outside.remove();
  });

  it("the Settings row closes the menu and navigates to the Devices section", async () => {
    const handle = mountAccountRow();
    press(handle.trigger());
    const row = handle.card()!.querySelector<HTMLButtonElement>(".menu-item");
    expect(row).not.toBeNull();

    await act(async () => {
      row!.click();
    });

    // `open_settings(SettingsSection::Devices)` (shell.rs:6417-6427): the
    // row's press closes the card first, then lands on the devices route.
    expect(h.navigateCalls).toEqual([{ to: "/settings/devices" }]);
    expect(handle.card()).toBeNull();
  });
});

// ── The phone arm: the drawer sheet (no clipping regressions at ≤768px) ─────

describe("AccountRow — phone arm", () => {
  it("opens as the bottom sheet at the body, never an inline card inside the aside", () => {
    h.cells.phone = true;
    const handle = mountAccountRow();
    expect(handle.card()).toBeNull();

    press(handle.trigger());

    // PickerCard's phone arm: the card body rides the shared drawer sheet
    // (Base UI Drawer portal, the modal tier) — still NOT inside the
    // clipping sidebar, and no desktop flyout mounts.
    expect(document.querySelector(".rb-popover-popup")).toBeNull();
    const sheet = document.querySelector<HTMLElement>(".rb-drawer-card.user-menu-body");
    expect(sheet).not.toBeNull();
    expect(handle.sidebar.contains(sheet!)).toBe(false);
    expect(document.body.contains(sheet!)).toBe(true);
    expect(sheet!.querySelector(".user-menu-identity")!.textContent).toBe("Stored on this device");
    expect(sheet!.querySelector(".menu-item")).not.toBeNull();
    expect(handle.trigger().getAttribute("aria-expanded")).toBe("true");
  });

  it("the sheet's Settings row navigates to the devices route too", async () => {
    h.cells.phone = true;
    const handle = mountAccountRow();
    press(handle.trigger());
    const row = document.querySelector<HTMLElement>(".rb-drawer-card .menu-item");
    expect(row).not.toBeNull();

    await act(async () => {
      row!.click();
    });

    expect(h.navigateCalls).toEqual([{ to: "/settings/devices" }]);
  });
});
