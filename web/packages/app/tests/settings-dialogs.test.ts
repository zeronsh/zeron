// @vitest-environment jsdom

/**
 * Ticket 18 — the settings routes' two dialogs on the shared `ui/Dialog`
 * family: LoginDialog (settings-accounts.tsx, over accounts.rs:1114's
 * `popover::modal`) and RenameDeviceDialog (settings-devices.tsx, over
 * devices.rs render_rename_dialog). The swaps retire the hand-rolled
 * `.login-dialog-*`/`.rename-dialog-*` scrim/card chrome; the mounted
 * contracts here pin what the family must carry in its place:
 *
 * - The shared tree at desktop: `.rb-dialog-card` + `.modal-backdrop` +
 *   the `dialog-card`/`dialog-card-title` chrome, with the card's own
 *   accessible name.
 * - The phone arm (the point of the swap at ≤768px): `RbDrawerSheet`'s
 *   bottom sheet — `.rb-drawer-card[data-open]` carrying the same
 *   `.dialog-card` body — where both old cards stayed fixed-centered and
 *   squashed at phone widths.
 * - The interaction contract: the paste-code arm's submit is disabled
 *   until a code is typed and routes the typed value; the rename arm
 *   pre-fills its field and routes the name; Cancel routes; Escape
 *   routes (the one gained path — the old hand-rolls had no Escape, and
 *   the rename scrim swallowed presses by design, which
 *   `RbDialog`'s `disablePointerDismissal` preserves).
 *
 * The mounted idiom follows nested-menu.test.ts: no JSX, per-file jsdom
 * pragma, Base UI dialogs in jsdom behind the matchMedia /
 * ResizeObserver / scrollIntoView / rAF stubs.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import type { AgentLoginStart } from "@zeron/proto";
import { LoginDialog } from "../src/routes/settings-accounts";
import { RenameDeviceDialog } from "../src/routes/settings-devices";

// ── jsdom gaps the mounted dialogs hit (nested-menu.test.ts's set) ──────────

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

// ── The mounted dialog harness ──────────────────────────────────────────────

interface MountedDialog {
  /** The host container the dialog component renders into. */
  readonly container: HTMLDivElement;
  unmount(): void;
}

const mounted: MountedDialog[] = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  phoneMode = false;
  document.body.replaceChildren();
});

/** The login flow's paste-code start (accounts.rs's `AgentLoginStart`). */
const start: AgentLoginStart = {
  loginId: "login-1",
  url: "https://example.com/authorize",
  mode: "paste-code",
  cliOpensBrowser: false,
};

/** Mount the paste-code LoginDialog; record every routed callback. */
function mountLogin(): { handle: MountedDialog; calls: string[] } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const calls: string[] = [];
  act(() => {
    root.render(
      createElement(LoginDialog, {
        flow: { kind: "paste-code", harness: "claude-code", start, submitting: false, error: null },
        onCancel: () => calls.push("cancel"),
        onSubmitCode: (code: string) => calls.push(`code:${code}`),
      }),
    );
  });
  const handle: MountedDialog = {
    container,
    unmount() {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  mounted.push(handle);
  return { handle, calls };
}

/** Mount the rename dialog; record every routed callback. */
function mountRename(name: string): { handle: MountedDialog; calls: string[] } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const calls: string[] = [];
  act(() => {
    root.render(
      createElement(RenameDeviceDialog, {
        dialog: { deviceId: "device-1", name },
        onCancel: () => calls.push("cancel"),
        onSubmit: (next: string) => calls.push(`submit:${next}`),
      }),
    );
  });
  const handle: MountedDialog = {
    container,
    unmount() {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  mounted.push(handle);
  return { handle, calls };
}

/** The open card at the CURRENT viewport: the dialog at ≥769px, the sheet
 *  at ≤768px — the responsive family's two arms of the one surface. */
function loginSurface(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.rb-dialog-card[aria-label="Add Claude account"], .rb-drawer-card[aria-label="Add Claude account"]');
}

function renameSurface(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.rb-dialog-card[aria-label="Rename device"], .rb-drawer-card[aria-label="Rename device"]');
}

/** Type into a React controlled input — the native-setter route React's
 *  value tracker leaves open (a bare `.value` write never re-renders). */
function typeInto(input: HTMLInputElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

// ── LoginDialog ─────────────────────────────────────────────────────────────

describe("LoginDialog on the shared ui/Dialog family (ticket 18)", () => {
  it("desktop: the shared modal tree — backdrop, centered card, the dialog chrome, the paste-code contract", () => {
    const { handle } = mountLogin();
    // The shared tree replaced the `.login-dialog-backdrop`/`-card`
    // hand-roll: Base UI's portal renders the scrim + the rb-dialog card.
    const card = document.querySelector<HTMLElement>('.rb-dialog-card[aria-label="Add Claude account"]');
    expect(card).not.toBeNull();
    expect(document.querySelector(".modal-backdrop")).not.toBeNull();
    // The card is portaled to the body, not a child of the route host.
    expect(handle.container.contains(card!)).toBe(false);
    // The family's chrome carries the title and the field; the old
    // `.login-dialog-title`/`-form` classes are gone with the hand-roll.
    expect(card!.querySelector(".dialog-card .dialog-card-title")?.textContent).toBe("Add Claude account");
    const input = card!.querySelector<HTMLInputElement>(".dialog-field input");
    expect(input).not.toBeNull();
    expect(card!.querySelector(".login-dialog-title")).toBeNull();
    expect(card!.querySelector(".login-dialog-form")).toBeNull();
    // The action row is the family's: a ghost Cancel and a solid primary.
    expect(card!.querySelector(".dialog-btn-ghost")?.textContent).toBe("Cancel");
    const primary = card!.querySelector<HTMLButtonElement>(".dialog-btn-primary");
    expect(primary?.textContent).toBe("Add account");
    // The paste-code arm's disabled-while-empty contract rides the family
    // button unchanged.
    expect(primary!.disabled).toBe(true);
  });

  it("desktop: typing a code enables the primary and submits the typed value", () => {
    const { calls } = mountLogin();
    const input = document.querySelector<HTMLInputElement>('.rb-dialog-card .dialog-field input');
    expect(input).not.toBeNull();
    act(() => {
      typeInto(input!, "auth-code-123");
    });
    const primary = document.querySelector<HTMLButtonElement>(".rb-dialog-card .dialog-btn-primary");
    expect(primary?.disabled).toBe(false);
    act(() => {
      primary!.click();
    });
    expect(calls).toEqual(["code:auth-code-123"]);
  });

  it("desktop: Escape cancels — the path the old hand-roll never had", () => {
    const { calls } = mountLogin();
    expect(loginSurface()).not.toBeNull();
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(calls).toEqual(["cancel"]);
  });

  it("phone: the arm the old fixed-centered card never had — the shared bottom sheet", () => {
    phoneMode = true;
    const { handle } = mountLogin();
    // The sheet, not the centered card: `.rb-drawer-card[data-open]`
    // carrying the same `.dialog-card` body — where the old hand-roll
    // stayed fixed-centered and squashed at ≤768px.
    const sheet = document.querySelector<HTMLElement>('.rb-drawer-card[aria-label="Add Claude account"]');
    expect(sheet).not.toBeNull();
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    expect(sheet!.querySelector(".dialog-card .dialog-card-title")?.textContent).toBe("Add Claude account");
    expect(document.querySelector(".rb-dialog-card")).toBeNull();
    expect(handle.container.contains(sheet!)).toBe(false);
  });
});

// ── RenameDeviceDialog ──────────────────────────────────────────────────────

describe("RenameDeviceDialog on the shared ui/Dialog family (ticket 18)", () => {
  it("desktop: the shared modal tree with the pre-filled field, Cancel and Rename", () => {
    const { handle } = mountRename("Studio");
    const card = document.querySelector<HTMLElement>('.rb-dialog-card[aria-label="Rename device"]');
    expect(card).not.toBeNull();
    expect(handle.container.contains(card!)).toBe(false);
    expect(card!.querySelector(".dialog-card .dialog-card-title")?.textContent).toBe("Rename device");
    // The old `.rename-dialog-*` chrome classes are gone with the hand-roll.
    expect(card!.querySelector(".rename-dialog-title")).toBeNull();
    expect(card!.querySelector(".rename-dialog-field")).toBeNull();
    expect(card!.querySelector(".rename-dialog-actions")).toBeNull();
    const input = card!.querySelector<HTMLInputElement>(".dialog-field input");
    expect(input?.value).toBe("Studio");
    expect(card!.querySelector(".dialog-btn-ghost")?.textContent).toBe("Cancel");
    expect(card!.querySelector(".dialog-btn-primary")?.textContent).toBe("Rename");
  });

  it("desktop: Rename routes the (edited) name through the form submit", () => {
    const { calls } = mountRename("Studio");
    const input = document.querySelector<HTMLInputElement>(".rb-dialog-card .dialog-field input");
    act(() => {
      typeInto(input!, "Desk");
    });
    act(() => {
      document.querySelector<HTMLButtonElement>(".rb-dialog-card .dialog-btn-primary")!.click();
    });
    expect(calls).toEqual(["submit:Desk"]);
  });

  it("desktop: Escape cancels — the gained path over the old no-Escape quirk", () => {
    const { calls } = mountRename("Studio");
    expect(renameSurface()).not.toBeNull();
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(calls).toEqual(["cancel"]);
  });

  it("phone: the shared bottom sheet arm with the same pre-filled body", () => {
    phoneMode = true;
    const { handle } = mountRename("Studio");
    const sheet = document.querySelector<HTMLElement>('.rb-drawer-card[aria-label="Rename device"]');
    expect(sheet).not.toBeNull();
    expect(sheet!.hasAttribute("data-open")).toBe(true);
    expect(sheet!.querySelector<HTMLInputElement>(".dialog-card .dialog-field input")?.value).toBe("Studio");
    expect(document.querySelector(".rb-dialog-card")).toBeNull();
    expect(handle.container.contains(sheet!)).toBe(false);
  });
});
