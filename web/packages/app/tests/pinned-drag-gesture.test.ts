// @vitest-environment jsdom

/**
 * Ticket 13 — the pinned-session drag reorder, mounted for real.
 *
 * The reported bug ("the web version doesn't have the sorting"): every
 * sidebar row is an `<a>` (the chat row's Link), and anchors are
 * draggable by default (HTML DnD). A left press that moves a few pixels
 * therefore starts the BROWSER's drag, not ours — the user agent fires
 * `pointercancel` "immediately before drag operation starts" (Pointer
 * Events §4.2.7, the pointer that caused the drag), our window-level
 * `cancel` handler obeys it, and the gesture tears itself down before any
 * reorder preview or commit. The same spec names the remedy: "If the start
 * of the drag operation is prevented through any means (e.g. through
 * calling preventDefault on the dragstart event) there will be no
 * pointercancel event."
 *
 * The contract under test, in the browser's own event order:
 *   1. every drag-armed row wrapper prevents its bubbled `dragstart`
 *      (the executable reproduction — before the fix nothing prevented
 *      it, so the browser always won the press);
 *   2. drag-start → drag-over pinned rows → drop commits the reorder
 *      through the real store chain and the synced surface (ticket 11);
 *   3. a drop below the section transfers out; a regular row dropped
 *      inside the pinned section pins at the drop slot (desktop parity);
 *   4. a `pointercancel` — what the browser fires when a native drag IS
 *      allowed to start — still tears the drag down with no commit, and
 *      Escape cancels; a no-op drop writes nothing.
 *
 * The REAL ChatList mounts with the real PinnedSection, the real store
 * chain (SidebarStore → UiSettingsStore → jsdom localStorage), and, for
 * the write-through, a real `SidebarStateSync` over a fake engine. The
 * fleet/session/router layers are doubled narrowly — the mounted-suite
 * idiom (session-provider / account-row). No JSX (createElement),
 * per-file jsdom pragma only.
 */

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeScopedId, methods } from "@zeron/engine-client";
import type { Chat, SidebarStateSnapshot } from "@zeron/proto";
import { ChatList } from "../src/components/chat-list";
import { SIDEBAR_SESSION_SLOT } from "../src/lib/sidebar-pins";
import { SidebarStateSync, type SidebarStateClient } from "../src/lib/sidebar-state-sync";
import { sidebarStore } from "../src/state/sidebar";
import { uiSettings, UI_SETTINGS_STORAGE_KEY, type UiSettings } from "../src/state/ui-settings";

// ── Controllable doubles (the fleet/session/router layers) ─────────────────

const h = vi.hoisted(() => {
  // The new-thread artwork prewarm (state/appearance.ts, pulled in by the
  // row monogram) rides `Image#decode`; jsdom has none, and its promise
  // chain settles before any beforeAll — patch at hoist time, ahead of
  // the import graph evaluating the store.
  if (
    typeof HTMLImageElement !== "undefined" &&
    typeof HTMLImageElement.prototype.decode !== "function"
  ) {
    (HTMLImageElement.prototype as unknown as { decode: () => Promise<void> }).decode =
      () => Promise.resolve();
  }
  const engines = [{ key: "eng-1", label: "Local Engine", baseUrl: "local" }];
  const sessions = new Map<string, unknown>();
  const navigateCalls: Array<{ to: string }> = [];
  /** The one paired engine — local scope, chats loaded, no spaces. */
  const engineEntry = {
    key: "eng-1",
    info: { deviceId: "dev-1", workspaceScope: "local" as const },
    state: "connected" as const,
    lastError: null,
    generation: 1,
    chats: { rows: [] as Chat[], loaded: true, error: null },
    spaces: { rows: [], loaded: true, error: null },
    devices: { rows: [], loaded: true, error: null },
    sessions: { rows: [], loaded: true, error: null },
  };
  const registry = { engines: [engineEntry], configurationError: null };
  const snapshot = {
    generation: 1,
    capabilities: [],
    chats: engineEntry.chats,
    spaces: engineEntry.spaces,
    devices: engineEntry.devices,
    statuses: engineEntry.sessions,
  };
  return { engines, sessions, navigateCalls, engineEntry, registry, snapshot };
});

vi.mock("../src/state/fleet", () => ({
  // Exactly what ChatList and the chat menu read: the merged snapshot's
  // chats/spaces/statuses/devices row sets, the registry's one local
  // engine (identity + chats loaded), and the fleet's active key.
  useFleet: () => ({ active: "eng-1", engines: h.engines, configurationError: null }),
  useFleetRegistry: () => h.registry,
  useFleetSnapshot: () => h.snapshot,
  engineStatesOf: () => new Map(),
  fleetLocalDeviceId: () => null,
}));

vi.mock("../src/state/session-provider", () => ({
  useEngineSessions: () => h.sessions,
  useEngineSession: () => null,
}));

vi.mock("@tanstack/react-router", async () => {
  const { createElement } = await import("react");
  return {
    useNavigate: () => (options: { to: string }): Promise<void> => {
      h.navigateCalls.push(options);
      return Promise.resolve();
    },
    useParams: () => ({}),
    useRouterState: <T,>(opts: { select: (state: unknown) => T }): T =>
      opts.select({ location: { pathname: "/" } }),
    // The row's real anchor: an `<a>` with the chat's scoped id in the
    // href — the natively-draggable element the whole ticket turns on.
    Link: (props: { params?: { chatId?: string }; className?: string; children?: React.ReactNode }) =>
      createElement(
        "a",
        {
          href: props.params?.chatId === undefined ? "#" : `#/chat/${props.params.chatId}`,
          className: props.className,
        },
        props.children,
      ),
  };
});

vi.mock("../src/state/session-provider", () => ({
  useEngineSessions: () => h.sessions,
  useEngineSession: () => null,
}));

// ── jsdom gaps the mounted list hits (the mounted-suite set) ───────────────

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: false,
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
  // The FLIP resort glide rides Web Animations (`Element.animate`) — jsdom
  // has none; the stub only needs `cancel` (the effect's cleanup).
  if (typeof Element.prototype.animate !== "function") {
    (Element.prototype as unknown as { animate: () => { cancel(): void } }).animate = () => ({
      cancel(): void {},
    });
  }
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

// ── Geometry: jsdom lays nothing out, so the drag reads fake rects ─────────

interface Rect {
  readonly top: number;
  readonly bottom: number;
  readonly left: number;
  readonly right: number;
}

/** The pinned rows group: top 200, two 63px slots, the sidebar column. */
const GROUP_TOP = 200;
const GROUP_LEFT = 0;
const GROUP_RIGHT = 260;
const PIN_COUNT = 2;
const GROUP_BOTTOM = GROUP_TOP + PIN_COUNT * SIDEBAR_SESSION_SLOT - 2;
/** The section root: the header above the group, room below it. */
const SECTION_TOP = 150;
const SECTION_BOTTOM = 400;

const rectOverrides = new WeakMap<Element, Rect>();
const realGetBoundingClientRect = Element.prototype.getBoundingClientRect;

function domRect(rect: Rect): DOMRect {
  return {
    x: rect.left,
    y: rect.top,
    width: rect.right - rect.left,
    height: rect.bottom - rect.top,
    top: rect.top,
    right: rect.right,
    bottom: rect.bottom,
    left: rect.left,
    toJSON(): Record<string, number> {
      return { x: rect.left, y: rect.top, width: rect.right - rect.left, height: rect.bottom - rect.top, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left };
    },
  } as DOMRect;
}

beforeAll(() => {
  Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
    const override = rectOverrides.get(this);
    return domRect(override ?? { top: 0, bottom: 0, left: 0, right: 0 });
  };
});

afterAll(() => {
  Element.prototype.getBoundingClientRect = realGetBoundingClientRect;
});

// ── The fake engine (ticket 11's suite idiom, leaned down) ─────────────────

interface WatchHandlers {
  onItem: (item: SidebarStateSnapshot, context: { generation: number }) => void;
}

interface Call {
  readonly method: string;
  readonly params: unknown;
}

/** The engine's sidebar-state store, in miniature. */
class FakeEngineStore {
  pinsByProfile: Record<string, string[]> = {};
  readonly watchers: Array<{ handlers: WatchHandlers; cancelled: boolean }> = [];

  setPins(profileKey: string, sessionIds: readonly string[]): SidebarStateSnapshot {
    if (sessionIds.length === 0) {
      delete this.pinsByProfile[profileKey];
    } else {
      this.pinsByProfile[profileKey] = [...sessionIds];
    }
    return this.publish();
  }

  private publish(): SidebarStateSnapshot {
    const snapshot: SidebarStateSnapshot = {
      pinsByProfile: { ...this.pinsByProfile },
      sectionsByProfile: {},
    };
    for (const watch of this.watchers) {
      if (!watch.cancelled) {
        watch.handlers.onItem(snapshot, { generation: 1 });
      }
    }
    return snapshot;
  }
}

class FakeClient {
  readonly calls: Call[] = [];
  readonly engine: FakeEngineStore;

  constructor(engine: FakeEngineStore) {
    this.engine = engine;
  }

  async call<T>(method: string, params?: unknown): Promise<T> {
    // A real EngineClient never runs a request in the task that wrote the
    // store — defer one tick so synchronous setup applies first.
    await new Promise((resolve) => setTimeout(resolve, 0));
    this.calls.push({ method, params });
    if (method === methods.SET_SIDEBAR_PINS) {
      const { profileKey, sessionIds } = params as { profileKey: string; sessionIds: string[] };
      return this.engine.setPins(profileKey, sessionIds) as T;
    }
    if (method === methods.SET_SIDEBAR_SECTIONS) {
      return { pinsByProfile: { ...this.engine.pinsByProfile }, sectionsByProfile: {} } as T;
    }
    throw new Error(`unknown method: ${method}`);
  }

  watch(_method: string, _params: unknown, handlers: WatchHandlers): { cancel: () => void } {
    const watch = { handlers, cancelled: false };
    this.engine.watchers.push(watch);
    // The stream's first item is the current value — engine parity.
    handlers.onItem({ pinsByProfile: { ...this.engine.pinsByProfile }, sectionsByProfile: {} }, {
      generation: 1,
    });
    return { cancel: () => { watch.cancelled = true; } };
  }
}

/** Let the bridge's self-driving write loop settle (one round per tick). */
async function settle(rounds = 12): Promise<void> {
  for (let i = 0; i < rounds; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// ── Fixtures ───────────────────────────────────────────────────────────────

/** A scoped chat id — the sidebar's namespace for engine "eng-1" rows. */
function sc(rawId: string): string {
  return encodeScopedId("eng-1", rawId);
}

function chat(rawId: string, at: string): Chat {
  return {
    id: sc(rawId),
    deviceId: "dev-1",
    title: `Chat ${rawId}`,
    archived: false,
    cwd: null,
    branch: null,
    checkoutId: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: at,
    createdAt: at,
  };
}

const PIN_1 = "p1";
const PIN_2 = "p2";
const REGULAR_1 = "r1";
const REGULAR_2 = "r2";

/** Recency: p1 newest … r1 oldest — the pins lead regardless. */
function seedFleet(): void {
  h.engineEntry.chats.rows = [
    chat(PIN_1, "2026-09-16T12:04:00Z"),
    chat(PIN_2, "2026-09-16T12:03:00Z"),
    chat(REGULAR_2, "2026-09-16T12:02:00Z"),
    chat(REGULAR_1, "2026-09-16T12:01:00Z"),
  ];
}

/** The saved pin bucket, exactly the drag math reads it. */
function pins(): readonly string[] {
  return sidebarStore.getSnapshot().pinnedByProfile["local"] ?? [];
}

// ── The mounted ChatList harness ───────────────────────────────────────────

interface MountedChatList {
  readonly container: HTMLElement;
  unmount(): void;
  /** The pinned row wrappers, in display order. */
  pinnedRows(): HTMLElement[];
  /** The regular row wrappers, in display order. */
  regularRows(): HTMLElement[];
  /** The hrefs inside the pinned rows — the display order, as the DOM has it. */
  pinnedHrefs(): string[];
}

const mounted: Array<() => void> = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
  document.body.replaceChildren();
  h.navigateCalls.length = 0;
});

function mountChatList(): MountedChatList {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(createElement(ChatList));
  });
  const list = container.querySelector<HTMLElement>(".chat-list");
  if (list === null) {
    throw new Error("the chat list did not render");
  }
  // The rects the gesture math reads: the pinned group (two slots), the
  // section root around it, and the sidebar column that contains the drag.
  const section = container.querySelector<HTMLElement>('[data-testid="sidebar-pinned-section"]');
  if (section !== null) {
    rectOverrides.set(section, { top: SECTION_TOP, bottom: SECTION_BOTTOM, left: GROUP_LEFT, right: GROUP_RIGHT });
  }
  const group = container.querySelector<HTMLElement>('[data-testid="sidebar-pinned-sessions"]');
  if (group !== null) {
    rectOverrides.set(group, { top: GROUP_TOP, bottom: GROUP_BOTTOM, left: GROUP_LEFT, right: GROUP_RIGHT });
  }
  rectOverrides.set(list, { top: 0, bottom: 800, left: GROUP_LEFT, right: GROUP_RIGHT });
  const unmount = (): void => {
    act(() => {
      root.unmount();
    });
    container.remove();
  };
  mounted.push(unmount);
  const pinnedRows = (): HTMLElement[] =>
    Array.from(container.querySelectorAll<HTMLElement>(".pinned-row"));
  return {
    container,
    unmount,
    pinnedRows,
    regularRows: () => Array.from(container.querySelectorAll<HTMLElement>(".regular-row")),
    pinnedHrefs: () =>
      Array.from(container.querySelectorAll<HTMLElement>(".pinned-row a")).map(
        (anchor) => anchor.getAttribute("href") ?? "",
      ),
  };
}

// ── Event drivers (real DOM events, the browser's order) ───────────────────

function firePointer(target: Element | Window, type: string, at: { clientX: number; clientY: number }): void {
  act(() => {
    (target as Element | Window).dispatchEvent(
      new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, ...at }),
    );
  });
}

/**
 * The dragstart the browser fires on the row's anchor when it starts the
 * native drag (cancelable, bubbling). Returns the event so the caller can
 * read `defaultPrevented` — the whole fix rides on that flag.
 */
function fireDragStart(target: Element): Event {
  const event = new Event("dragstart", { bubbles: true, cancelable: true });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

/**
 * The ticket's gesture: press the row, move past the 4px arm threshold,
 * settle over slot `to`, release. `over` reads the group-relative slot
 * math (`SIDEBAR_SESSION_SLOT` quantization, exactly the component's).
 */
function dragPinnedRow(handle: MountedChatList, from: number, to: number): void {
  const rows = handle.pinnedRows();
  const row = rows[from];
  if (row === undefined) {
    throw new Error(`no pinned row ${from}`);
  }
  const anchorX = (GROUP_LEFT + GROUP_RIGHT) / 2;
  firePointer(row, "pointerdown", { clientX: anchorX, clientY: GROUP_TOP + 30 + from * SIDEBAR_SESSION_SLOT });
  // 6px down — past the arm threshold, still inside the source slot.
  firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 36 + from * SIDEBAR_SESSION_SLOT });
  // The destination slot.
  firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 30 + to * SIDEBAR_SESSION_SLOT + 1 });
  firePointer(window, "pointerup", { clientX: anchorX, clientY: GROUP_TOP + 30 + to * SIDEBAR_SESSION_SLOT + 1 });
}

/** A regular row's transfer-in gesture, released at `release`. */
function dragRegularRow(handle: MountedChatList, from: number, release: { clientX: number; clientY: number }): void {
  const rows = handle.regularRows();
  const row = rows[from];
  if (row === undefined) {
    throw new Error(`no regular row ${from}`);
  }
  const startX = (GROUP_LEFT + GROUP_RIGHT) / 2;
  firePointer(row, "pointerdown", { clientX: startX, clientY: 500 });
  firePointer(window, "pointermove", { clientX: startX, clientY: 506 });
  firePointer(window, "pointerup", release);
}

// ── The suites ─────────────────────────────────────────────────────────────

beforeEach(() => {
  seedFleet();
  // The saved bucket the drag math reads: p1 above p2, the desktop's
  // `commit_pinned_session_drag` input shape.
  sidebarStore.replacePinsByProfile({ local: [sc(PIN_1), sc(PIN_2)] });
  window.localStorage.clear();
});

describe("the drag gesture owns the press (ticket 13's root cause)", () => {
  it("prevents the native dragstart on every drag-armed row — no pointercancel can fire mid-gesture", () => {
    const handle = mountChatList();
    // The browser starts a native drag when a press on the row's anchor
    // moves (links are draggable by default); Pointer Events §4.2.7 then
    // fires pointercancel "immediately before drag operation starts".
    // Preventing the dragstart is the spec's own remedy — "there will be
    // no pointercancel event" — so the window-level gesture keeps its
    // pointer stream. Before the fix NOTHING prevented this event: the
    // browser won every press, and the reorder never happened.
    const pinnedAnchor = handle.pinnedRows()[0]!.querySelector("a")!;
    expect(fireDragStart(pinnedAnchor).defaultPrevented).toBe(true);
    const regularAnchor = handle.regularRows()[0]!.querySelector("a")!;
    expect(fireDragStart(regularAnchor).defaultPrevented).toBe(true);
  });

  it("drag-start → drag-over pinned rows → drop reorders and commits", () => {
    const handle = mountChatList();
    const rows = handle.pinnedRows();
    const anchorX = (GROUP_LEFT + GROUP_RIGHT) / 2;
    // Press row 0, move past the arm threshold — the section is dragging.
    firePointer(rows[0]!, "pointerdown", { clientX: anchorX, clientY: GROUP_TOP + 30 });
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 36 });
    expect(handle.pinnedRows().every((row) => row.dataset.dragging === "1")).toBe(true);
    // Drag over row 1's slot: the dragged row rides its slot and the
    // sibling slides toward the vacated space (the reorder preview).
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 94 });
    const dragging = handle.pinnedRows();
    expect(dragging[0]!.style.transform).toBe(`translateY(${SIDEBAR_SESSION_SLOT}px)`);
    expect(dragging[1]!.style.transform).toBe(`translateY(-${SIDEBAR_SESSION_SLOT}px)`);
    // Release: the commit lands — the saved bucket reorders…
    firePointer(window, "pointerup", { clientX: anchorX, clientY: GROUP_TOP + 94 });
    expect(pins()).toEqual([sc(PIN_2), sc(PIN_1)]);
    // …and the rows render in the new order, no glide (already in place).
    expect(handle.pinnedHrefs()).toEqual([`#/chat/${sc(PIN_2)}`, `#/chat/${sc(PIN_1)}`]);
  });

  it("the pointercancel a native drag WOULD fire tears the gesture down with no commit", () => {
    const handle = mountChatList();
    const rows = handle.pinnedRows();
    const anchorX = (GROUP_LEFT + GROUP_RIGHT) / 2;
    firePointer(rows[0]!, "pointerdown", { clientX: anchorX, clientY: GROUP_TOP + 30 });
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 36 });
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 94 });
    // What the browser does when the native drag is NOT prevented: the
    // pointer stream is suppressed (pointercancel) — our cancel path
    // obeys, the drag dies, nothing commits.
    firePointer(rows[0]!, "pointercancel", { clientX: anchorX, clientY: GROUP_TOP + 94 });
    expect(pins()).toEqual([sc(PIN_1), sc(PIN_2)]);
    expect(handle.pinnedRows().every((row) => row.dataset.dragging === undefined)).toBe(true);
  });

  it("a drop below the section transfers out — the pin membership clears", () => {
    const handle = mountChatList();
    const rows = handle.pinnedRows();
    const anchorX = (GROUP_LEFT + GROUP_RIGHT) / 2;
    firePointer(rows[0]!, "pointerdown", { clientX: anchorX, clientY: GROUP_TOP + 30 });
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 36 });
    // Below the group the drag previews the transfer out (the row lifts,
    // the siblings settle home)…
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_BOTTOM + 26 });
    expect(handle.pinnedRows()[0]!.dataset.transfer).toBe("1");
    // …and the release unpins: p1 lands among the regular rows.
    firePointer(window, "pointerup", { clientX: anchorX, clientY: GROUP_BOTTOM + 26 });
    expect(pins()).toEqual([sc(PIN_2)]);
    const regularHrefs = handle
      .regularRows()
      .map((row) => row.querySelector("a")?.getAttribute("href") ?? "");
    expect(regularHrefs).toContain(`#/chat/${sc(PIN_1)}`);
  });

  it("a regular row dropped inside the pinned section pins at the drop slot", () => {
    const handle = mountChatList();
    // The newest regular chat (recency head) dragged into the pinned
    // group's first slot — `SidebarSessionDrop::Pinned(0)`.
    dragRegularRow(handle, 0, { clientX: (GROUP_LEFT + GROUP_RIGHT) / 2, clientY: GROUP_TOP + 10 });
    expect(pins()).toEqual([sc(REGULAR_2), sc(PIN_1), sc(PIN_2)]);
    expect(handle.pinnedHrefs()).toEqual([
      `#/chat/${sc(REGULAR_2)}`,
      `#/chat/${sc(PIN_1)}`,
      `#/chat/${sc(PIN_2)}`,
    ]);
  });

  it("escape cancels; a no-op drop writes nothing", () => {
    const handle = mountChatList();
    const rows = handle.pinnedRows();
    const anchorX = (GROUP_LEFT + GROUP_RIGHT) / 2;
    // Escape mid-drag.
    firePointer(rows[0]!, "pointerdown", { clientX: anchorX, clientY: GROUP_TOP + 30 });
    firePointer(window, "pointermove", { clientX: anchorX, clientY: GROUP_TOP + 36 });
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    });
    expect(pins()).toEqual([sc(PIN_1), sc(PIN_2)]);
    expect(handle.pinnedRows().every((row) => row.dataset.dragging === undefined)).toBe(true);
    // A release in the source slot is a no-op (from === over).
    dragPinnedRow(handle, 0, 0);
    expect(pins()).toEqual([sc(PIN_1), sc(PIN_2)]);
  });
});

describe("the reorder commit persists through ticket 11's synced surface", () => {
  it("writes the reordered bucket to the engine over SetSidebarPins, caching locally", async () => {
    // Engine-side pre-seeded (raw ids): the bridge adopts, never fights.
    const engine = new FakeEngineStore();
    engine.setPins("local", [PIN_1, PIN_2]);
    const client = new FakeClient(engine);
    const sync = new SidebarStateSync(uiSettings);
    try {
      sync.attach("eng-1", client as unknown as SidebarStateClient, "local");
      const handle = mountChatList();
      expect(pins()).toEqual([sc(PIN_1), sc(PIN_2)]);

      // The real gesture's real commit — through SidebarStore →
      // UiSettingsStore → the bridge's subscription.
      dragPinnedRow(handle, 0, 1);
      expect(pins()).toEqual([sc(PIN_2), sc(PIN_1)]);
      await settle();

      // The RPC write: the profile bucket, raw engine ids, in the new
      // order — exactly what the engine persists engine-side.
      const writes = client.calls.filter((call) => call.method === methods.SET_SIDEBAR_PINS);
      expect(writes.length).toBeGreaterThan(0);
      expect(writes[writes.length - 1]!.params).toEqual({
        profileKey: "local",
        sessionIds: [PIN_2, PIN_1],
      });
      expect(engine.pinsByProfile["local"]).toEqual([PIN_2, PIN_1]);
      // The offline cache (localStorage) holds the optimistic bucket.
      const persisted = JSON.parse(
        window.localStorage.getItem(UI_SETTINGS_STORAGE_KEY)!,
      ) as UiSettings;
      expect(persisted.sidebarPinnedSessionIdsByProfile["local"]).toEqual([sc(PIN_2), sc(PIN_1)]);
    } finally {
      sync.dispose();
    }
  });
});
