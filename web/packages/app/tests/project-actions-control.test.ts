// @vitest-environment jsdom

/**
 * Ticket 09 (web-bugs-2026-09, research Bug 7): the empty-state "Add
 * action" segment must open ONLY the editor — its click is consumed
 * (`stopPropagation`, the preferred-run segment's own rule) before it can
 * bubble into the whole-button Base UI trigger (PickerCard's
 * Popover.Trigger at ≥769px, its phone arm's Drawer.Trigger at ≤768px) and
 * re-open the menu `openEditor` just closed. Desktop parity: the add
 * segment's on_click goes straight to `open_project_action_editor`
 * (actions_ui.rs:614), which closes the menu first — GPUI's sibling
 * segments never bubble into a trigger, so the dual-open cannot happen
 * there; the click consumption is what buys the web the same guarantee.
 *
 * The REAL ProjectActionsControl mounts against the real PickerCard /
 * menu / dialog surfaces with a scripted engine client (the
 * composer-reasoning mounted-suite idiom — Base UI runs for real in
 * jsdom, its portals into document.body and all). The fleet and session
 * layers are doubled narrowly (exactly the rows `projectActionContext`
 * reads), the terminal drawer is a recording double, and the phone arm
 * runs under a controllable `matchMedia` whose `matches` answers
 * PHONE_QUERY. No JSX (createElement), per-file jsdom pragma only.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Chat, ProjectActionsSnapshot, Space } from "@zeron/proto";
import type { EngineSession } from "../src/state/engine-session";
import { ProjectActionsControl } from "../src/components/project-actions-control";
import { projectActionsStore } from "../src/lib/project-actions";
import { PHONE_QUERY } from "../src/state/media";

// ── Controllable doubles ──────────────────────────────────────────────────

const h = vi.hoisted(() => {
  const DEVICE = "engine:v1:dev-a";

  /** The chat the control is mounted for — `projectActionContext`'s row. */
  const chat: Chat = {
    id: "chat-1",
    deviceId: DEVICE,
    title: null,
    archived: false,
    cwd: "/repo",
    branch: null,
    checkoutId: null,
    sourceContext: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: null,
    createdAt: "2026-01-01T00:00:00Z",
    spaceId: "space-1",
  };

  /** The chat's space row — same device, so the context resolves. */
  const space: Space = {
    id: "space-1",
    deviceId: DEVICE,
    path: "/repo",
    name: null,
    gitDetected: true,
    checkoutId: null,
    createdAt: "2026-01-01T00:00:00Z",
  };

  /** The merged fleet snapshot the control selects its rows from. */
  const fleetSnapshot = {
    generation: 1,
    capabilities: [],
    chats: { rows: [chat], loaded: true, error: null },
    spaces: { rows: [space], loaded: true, error: null },
    devices: { rows: [], loaded: true, error: null },
    statuses: { rows: [], loaded: true, error: null },
    connectivity: { value: null, loaded: false, error: null },
  };

  /** The registry snapshot `fleetLocalDeviceId` reads (the local engine). */
  const registry = {
    engines: [{ key: "local", state: "connected", info: { deviceId: DEVICE, capabilities: [] } }],
    configurationError: null,
  };

  /** Scripted engine client: `ListProjectActions` resolves the seeded snapshot. */
  class FakeClient {
    snapshot: ProjectActionsSnapshot = {
      spaceId: "space-1",
      actions: [],
      importableActions: [],
      projectFileIssue: null,
    };
    readonly calls: { method: string; params: Record<string, unknown> }[] = [];

    call<T>(method: string, params: unknown = {}): Promise<T> {
      this.calls.push({ method, params: params as Record<string, unknown> });
      return Promise.resolve(this.snapshot as T);
    }
  }

  /** The terminal drawer double — `run`'s reserve/attach surface, recorded. */
  const terminals = {
    reserved: [] as { chatId: string; title: string }[],
    reserveTabForChat(chatId: string, title: string): string | null {
      this.reserved.push({ chatId, title });
      return `tab-${this.reserved.length}`;
    },
    attachReservedSession(): boolean {
      return true;
    },
    failReservedTab(): void {},
  };

  const cells = {
    /** The viewport arm: `true` arms `(max-width: 768px)` in the matchMedia stub. */
    phone: false,
    client: null as FakeClient | null,
    session: null as EngineSession | null,
    reset(): void {
      this.client = new FakeClient();
      // The chat-owning engine's session: `engine.baseUrl` is the routing
      // key the context hashes, `client` is the store's RPC target.
      this.session = { engine: { baseUrl: "local" }, client: this.client } as unknown as EngineSession;
    },
  };

  return { fleetSnapshot, registry, FakeClient, terminals, cells };
});

vi.mock("../src/state/fleet", () => ({
  // Exactly what ProjectActionsControl reads: the active engine, the
  // registry row `fleetLocalDeviceId` resolves, and the merged rows the
  // context resolver selects the chat/space from.
  useFleet: () => ({ active: "local" }),
  useFleetRegistry: () => h.registry,
  useFleetSnapshot: () => h.fleetSnapshot,
  fleetLocalDeviceId: (
    registry: { engines: { key: string; info: { deviceId: string } | null }[] },
    active: string | null,
  ): string | null => registry.engines.find((engine) => engine.key === active)?.info?.deviceId ?? null,
}));

vi.mock("../src/state/session-provider", () => ({
  // The routed engine's session — only `engine.baseUrl` and `client` are
  // read on this path; the real provider's notification machinery is
  // unrelated to the click-propagation contract under test.
  useEngineSession: () => h.cells.session,
  useEngineSessions: () => new Map(),
  useEngineRetry: () => {},
  EngineSessionProvider: () => null,
}));

vi.mock("../src/terminal/store", () => ({
  // Only `drawerTerminalStore` is imported (run()'s attach surface); the
  // real module drags xterm in for a flow these tests never enter.
  drawerTerminalStore: h.terminals,
}));

// ── jsdom gaps the mounted control hits ───────────────────────────────────
// matchMedia (useIsPhone in PickerCard / RbResponsiveDialog — controllable
// so the phone arm can be armed per test), ResizeObserver (the popover
// positioner's tracking), scrollIntoView, and rAF (Base UI transitions).

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

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
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
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

// ── Mounted-control harness ────────────────────────────────────────────────

interface Mounted {
  readonly container: HTMLElement;
  unmount(): void;
}

const mounted: Mounted[] = [];

beforeEach(() => {
  h.cells.phone = false;
  h.cells.reset();
  projectActionsStore.activate(null);
});

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!.unmount();
  }
  document.body.replaceChildren();
});

function mountControl(): Mounted {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    // 800px of titlebar room: the "Add action" label renders with the icon.
    root.render(createElement(ProjectActionsControl, { chatId: "chat-1", availableTitlebarWidth: 800 }));
  });
  let unmounted = false;
  const handle: Mounted = {
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

/** Drain the store's load promises and the re-renders they bump. */
async function flush(rounds = 6): Promise<void> {
  await act(async () => {
    for (let round = 0; round < rounds; round += 1) {
      await Promise.resolve();
    }
  });
}

/** The control's trigger — rendered once the first load settles. */
function triggerButton(handle: Mounted): HTMLElement {
  const button = handle.container.querySelector<HTMLElement>(".project-action-control");
  if (button === null) {
    throw new Error("the project-actions trigger did not render");
  }
  return button;
}

/** The editor's open card (dialog at ≥769px, bottom sheet at ≤768px). */
function editorSurface(): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    '.rb-dialog-card[aria-label="Add action"], .rb-drawer-card[aria-label="Add action"]',
  );
}

/**
 * The actions menu while still open. An exiting card keeps rendering with
 * `data-closed` until its (absent, in jsdom) exit animation drains — that
 * one is dismissed, not open, so it must not count as a second surface.
 */
function openMenu(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".project-actions-menu:not([data-closed])");
}

/** The menu's trailing add row (portal-rendered, so query the document). */
function menuAddRow(): HTMLElement {
  const row = document.querySelector<HTMLElement>('[data-rb-row-key="project-actions-add-row"]');
  if (row === null) {
    throw new Error("the menu's Add action row did not render");
  }
  return row;
}

describe("ProjectActionsControl — Add action opens one surface", () => {
  it("desktop: the empty-state segment's click opens the editor dialog only; the menu never re-opens", async () => {
    const handle = mountControl();
    await flush();
    const segment = triggerButton(handle).querySelector<HTMLElement>(".project-action-main");
    expect(segment).not.toBeNull();

    await act(async () => {
      segment!.click();
    });

    // The editor dialog is up and owns the modal tier…
    expect(editorSurface()).not.toBeNull();
    expect(projectActionsStore.editor).not.toBeNull();
    // …and the popover the whole button triggers never opened: without the
    // click consumption this same press bubbles into the trigger and flips
    // `menuOpen` back on under the dialog (the reported dual surface).
    expect(openMenu()).toBeNull();
    expect(projectActionsStore.menuOpen).toBe(false);
  });

  it("desktop: the menu's Add action row opens the editor and closes the menu", async () => {
    const handle = mountControl();
    await flush();
    await act(async () => {
      triggerButton(handle).click();
    });
    expect(openMenu()).not.toBeNull();
    expect(projectActionsStore.menuOpen).toBe(true);

    await act(async () => {
      menuAddRow().click();
    });

    expect(editorSurface()).not.toBeNull();
    expect(openMenu()).toBeNull();
    expect(projectActionsStore.menuOpen).toBe(false);
    expect(projectActionsStore.editor).not.toBeNull();
  });

  it("phone: tapping the empty-state segment opens the editor sheet only; the actions sheet never opens", async () => {
    h.cells.phone = true;
    const handle = mountControl();
    await flush();
    const segment = triggerButton(handle).querySelector<HTMLElement>(".project-action-main");
    expect(segment).not.toBeNull();

    await act(async () => {
      segment!.click();
    });

    // The editor sheet is the one surface up (the dialog→drawer arm)…
    expect(editorSurface()).not.toBeNull();
    expect(projectActionsStore.editor).not.toBeNull();
    // …and PickerCard's phone arm — the drawer whose trigger is the same
    // whole button — never opened beside it: unstopped, the tap bubbles
    // into Drawer.Trigger and stacks the actions sheet under/over the
    // editor sheet (the 2026-09-24 mobile report).
    expect(openMenu()).toBeNull();
    expect(projectActionsStore.menuOpen).toBe(false);
  });

  it("phone: tapping the menu's Add action row in the actions sheet swaps in the editor sheet only", async () => {
    h.cells.phone = true;
    const handle = mountControl();
    await flush();
    await act(async () => {
      triggerButton(handle).click();
    });
    expect(openMenu()).not.toBeNull();
    expect(projectActionsStore.menuOpen).toBe(true);

    await act(async () => {
      menuAddRow().click();
    });

    // The row lives in the actions sheet's portal, so its click cannot
    // reach the trigger; the sheet the row dismisses stays dismissed and
    // the editor sheet is the only surface left.
    expect(editorSurface()).not.toBeNull();
    expect(openMenu()).toBeNull();
    expect(projectActionsStore.menuOpen).toBe(false);
    expect(projectActionsStore.editor).not.toBeNull();
    // Ticket 15: the editor sheet's entrance applies even as it opens over
    // the actions sheet's exit — the popup carries `[data-open]`, the key
    // `rb-dialog-in` rides (`.rb-drawer-card[data-open]`), and it animates
    // over (not under) the dying sheet: its portal mounts after the menu's.
    const editorSheet = document.querySelector<HTMLElement>('.rb-drawer-card[aria-label="Add action"]');
    expect(editorSheet).not.toBeNull();
    expect(editorSheet!.hasAttribute("data-open")).toBe(true);
    expect(editorSheet!.querySelector(".dialog-card")).not.toBeNull();
  });
});
