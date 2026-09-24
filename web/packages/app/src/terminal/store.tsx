import { useEffect, type ReactNode } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { EngineSession } from "../state/engine-session";
import type { TerminalSession } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { uiSettings } from "../state/ui-settings";
import { effectiveTerminalFontFamily, fontFamilyStack } from "../lib/appearance-store";
import { terminalLineHeight } from "../lib/typography";
import { TerminalSessionController } from "./session";
import { currentTerminalTheme } from "./theme";
import { pasteBytes } from "./tabs";
import {
  TERMINAL_DEFAULT_HEIGHT,
  activeAfterClose,
  activeAfterReorder,
  clampTerminalHeight,
  reorderTabs,
  shellTitle,
} from "./tabs";

/**
 * The terminal panel store, the web peer of the desktop's `TerminalPanel`
 * entity (`crates/ui/src/terminal/panel.rs`): tabs are per selected chat and
 * restored on return — the provider that owns this store lives above the
 * route outlet, so chat navigation keeps PTYs, emulators, and panel flags
 * alive (detach is not close).
 *
 * TWO hosts exist, exactly like the desktop's `Shell::terminal` (drawer) and
 * `Shell::right_terminal` (embedded) — two panel entities that never share
 * PTYs or tab sets:
 *
 * - `drawer` mode: the bottom dock under the conversation column. Toggling
 *   open lazily creates the first tab (`ensure_tab`); closing the LAST tab
 *   collapses the dock (an empty dock is dead space, `close_tab`'s toggle
 *   dispatch). Owns the persisted height (`settings.terminal_height`).
 * - `embedded` mode: the right pane's terminal panel. Tabs are explicit —
 *   the pane's surface chips mint them (`open_tab_for_selected`) and address
 *   them by key (`Terminal(tab)` surfaces). No auto-collapse: the SHELL owns
 *   emptiness (it falls back to the surface picker).
 *
 * Lifetime: one store per host, bound to the current engine session. A
 * session swap (engine switch, re-pair) closes every tab — the PTYs belong
 * to the old connection's watches; the desktop never faces this because its
 * panel's targets float across devices.
 *
 * React reads the store through a version counter (useSyncExternalStore);
 * tab records hold the mutable xterm instances and are never recreated.
 */
export interface TerminalTabRecord {
  readonly key: string;
  /** Fallback label: "Terminal N", then the shell basename once open answers. */
  title: string;
  /** The live OSC 0/2 title when the running program set one — it wins. */
  oscTitle: string | null;
  exited: boolean;
  readonly term: XTerm;
  readonly fitter: FitAddon;
  readonly controller: TerminalSessionController;
  /** `OpenTerminal` was requested (guards double-open on re-attach). */
  openRequested: boolean;
  /**
   * A placeholder waiting for an engine-side run to answer (project Actions'
   * `reserve_tab_for_chat`): the tab host must NOT fire `OpenTerminal` while
   * the owning RPC is in flight — the attach or the failure lands instead.
   */
  reserved: boolean;
}

export interface ChatTerminals {
  open: boolean;
  height: number;
  tabs: TerminalTabRecord[];
  active: number;
  nextKey: number;
}

export type TerminalStoreListener = () => void;

/** `isMac` per the web's convention (Cmd = the mod key there). */
function isMacPlatform(): boolean {
  const navigator = (globalThis as { navigator?: { platform?: string; userAgent?: string } }).navigator;
  if (navigator === undefined) {
    return false;
  }
  return /Mac|iPhone|iPad|Pod/i.test(`${navigator.platform ?? ""} ${navigator.userAgent ?? ""}`);
}

/**
 * The copy/paste keydown policy (`panel.rs::on_key_down`, view.rs §"Copy/paste"):
 * paste on Cmd+V (mac) / Ctrl+Shift+V (elsewhere) through the clipboard with
 * bracketed-paste wrapping; copy on Cmd+C / Ctrl+Shift+C only when a
 * selection exists (and swallows the keystroke only then — plain Ctrl+C is
 * xterm's own \x03 and always reaches the shell). Platform-primary chords
 * (Cmd on macOS, the Super key elsewhere — GPUI's `Modifiers::platform`,
 * which on the web is `metaKey` on both) are refused so app shortcuts like
 * Mod+J reach the shell keymap instead of the PTY.
 */
function attachClipboardPolicy(term: XTerm, controller: TerminalSessionController): void {
  const isMac = isMacPlatform();
  term.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") {
      return true;
    }
    if (event.defaultPrevented) {
      return false;
    }
    const chord = (code: string): boolean =>
      event.code === code &&
      (isMac
        ? event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
        : event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey);
    if (chord("KeyV")) {
      event.preventDefault();
      navigator.clipboard
        ?.readText()
        .then((text) => {
          controller.input(pasteBytes(text, term.modes.bracketedPasteMode));
        })
        .catch(() => {});
      return false;
    }
    if (chord("KeyC")) {
      const selection = term.getSelection();
      if (selection !== undefined && selection.length > 0) {
        void navigator.clipboard?.writeText(selection).catch(() => {});
        event.preventDefault();
      }
      return false;
    }
    if (event.metaKey) {
      return false;
    }
    return true;
  });
}

/** The live terminal font slot, resolved to an XTerm family stack + size. */
function terminalFonts(): { family: string; size: number } {
  const settings = uiSettings.getSnapshot();
  return {
    family: fontFamilyStack(effectiveTerminalFontFamily(settings.terminalFontFamily)),
    size: settings.terminalFontSize,
  };
}

export class TerminalStore {
  readonly #mode: "drawer" | "embedded";
  #session: EngineSession | null = null;
  readonly #chats = new Map<string, ChatTerminals>();
  readonly #listeners = new Set<TerminalStoreListener>();
  #version = 0;
  #fonts: { family: string; size: number };
  readonly #unsubscribeFonts: () => void;

  constructor(mode: "drawer" | "embedded") {
    this.#mode = mode;
    // The terminal font slot (typography.rs `terminal_*`): read live at tab
    // creation, then re-applied to every live tab on change — the desktop's
    // row-height-from-live-value fix (cursor/selection drift) maps to a
    // refit here, since XTerm's cell grid is measured off the options.
    this.#fonts = terminalFonts();
    this.#unsubscribeFonts = uiSettings.subscribe(() => {
      const next = terminalFonts();
      if (next.family === this.#fonts.family && next.size === this.#fonts.size) {
        return;
      }
      this.#fonts = next;
      for (const chat of this.#chats.values()) {
        for (const tab of chat.tabs) {
          tab.term.options.fontFamily = next.family;
          tab.term.options.fontSize = next.size;
          tab.term.options.lineHeight = terminalLineHeight(next.size);
          tab.fitter.fit();
        }
      }
    });
  }

  subscribe = (listener: TerminalStoreListener): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  getVersion = (): number => this.#version;

  /** Bind to a new engine session, closing everything the old one owned. */
  bindSession(session: EngineSession | null): void {
    if (session === this.#session) {
      return;
    }
    this.#session = session;
    for (const chat of this.#chats.values()) {
      for (const tab of chat.tabs) {
        tab.controller.close();
        tab.term.dispose();
      }
    }
    this.#chats.clear();
    this.#bump();
  }

  dispose(): void {
    this.#unsubscribeFonts();
    this.bindSession(null);
    this.#listeners.clear();
  }

  stateFor(chatId: string): ChatTerminals | undefined {
    return this.#chats.get(chatId);
  }

  /** mod-j / header button: toggle the dock; opening an empty chat spawns
   *  its first tab (desktop `ensure_tab`). Drawer mode only. */
  toggle(chatId: string): void {
    const chat = this.#chat(chatId);
    chat.open = !chat.open;
    if (chat.open && chat.tabs.length === 0) {
      this.#addTab(chatId, chat);
    }
    this.#bump();
  }

  setHeight(chatId: string, height: number, viewportH: number): void {
    const chat = this.#chat(chatId);
    const clamped = clampTerminalHeight(height, viewportH);
    if (clamped !== chat.height) {
      chat.height = clamped;
      // The visual height tracks live; only the persistence write debounces
      // (desktop `schedule_save`, settings.rs SAVE_DEBOUNCE_MS = 400).
      uiSettings.updateDebounced({ terminalHeight: clamped });
      this.#bump();
    }
  }

  /**
   * Double-click on the resize handle: back to the 280px default
   * (`TERMINAL_DEFAULT_HEIGHT`). The desktop saves this through the same
   * debounced `schedule_save` path as a drag (shell.rs:6325-6330).
   */
  resetHeight(chatId: string, viewportH: number): void {
    this.setHeight(chatId, TERMINAL_DEFAULT_HEIGHT, viewportH);
  }

  /** The "+" button. The PTY opens when the dock mounts the tab's host. */
  addTab(chatId: string): void {
    const chat = this.#chat(chatId);
    this.#addTab(chatId, chat);
    this.#bump();
  }

  /**
   * Create a named placeholder tab without opening a PTY and reveal the dock
   * (`reserve_tab_for_chat` + the drawer reveal, panel.rs:508 + the run flow
   * in actions_ui.rs): project Actions reserve before their host-side run
   * RPC completes, so the tab exists while the terminal does not. Returns
   * null when no engine session is bound (the caller drops the run).
   */
  reserveTabForChat(chatId: string, title: string): string | null {
    if (this.#session === null) {
      return null;
    }
    const chat = this.#chat(chatId);
    this.#addTab(chatId, chat, undefined, title, true);
    chat.open = true;
    this.#bump();
    return chat.tabs.at(-1)?.key ?? null;
  }

  /**
   * Attach and stream a PTY that was already opened by the owning engine
   * (`attach_reserved_session`, panel.rs:536): the run RPC's terminal takes
   * the placeholder's place — the reserved title (the action's name) stays.
   * False when the tab was closed mid-flight.
   */
  attachReservedSession(chatId: string, key: string, session: TerminalSession): boolean {
    const tab = this.#chats.get(chatId)?.tabs.find((candidate) => candidate.key === key);
    if (tab === undefined) {
      return false;
    }
    tab.reserved = false;
    tab.openRequested = true;
    tab.controller.attach(session);
    this.#bump();
    return true;
  }

  /**
   * Turn a placeholder into a visible failed tab without opening a PTY
   * (`fail_reserved_tab`, panel.rs:516 — the run RPC's error path).
   */
  failReservedTab(chatId: string, key: string, message: string): void {
    const chat = this.#chats.get(chatId);
    const tab = chat?.tabs.find((candidate) => candidate.key === key);
    if (chat === undefined || tab === undefined) {
      return;
    }
    tab.reserved = false;
    tab.term.write(`\x1b[31mfailed to run action: ${message}\x1b[0m\r\n`);
    tab.exited = true;
    this.#bump();
  }

  /**
   * The embedded mint: create a tab with a caller-chosen key — the pane
   * surface's id (desktop `add_terminal_surface`: every click opens a FRESH
   * embedded terminal tab and pushes `Terminal(tab)` addressing it). False
   * when no engine session is bound (the caller drops its surface, like the
   * desktop's `if let Some(tab)`).
   */
  openTabFor(chatId: string, key: string): boolean {
    const chat = this.#chat(chatId);
    if (this.#session === null || chat.tabs.some((tab) => tab.key === key)) {
      return false;
    }
    chat.open = true; // desktop panel.set_open(true)
    this.#addTab(chatId, chat, key);
    this.#bump();
    return true;
  }

  /** Make `key` the rendered tab (`select_tab_by_key`, panel.rs:490). */
  selectTabByKey(chatId: string, key: string): void {
    const chat = this.#chats.get(chatId);
    const ix = chat?.tabs.findIndex((tab) => tab.key === key) ?? -1;
    if (chat !== undefined && ix !== -1) {
      this.selectTab(chatId, ix);
    }
  }

  /** The surface tab's display label, or null when it is gone. */
  tabTitle(chatId: string, key: string): string | null {
    const tab = this.#chats.get(chatId)?.tabs.find((candidate) => candidate.key === key);
    return tab === undefined ? null : displayTitle(tab);
  }

  selectTab(chatId: string, index: number): void {
    const chat = this.#chats.get(chatId);
    if (chat !== undefined && index < chat.tabs.length && chat.active !== index) {
      chat.active = index;
      this.#bump();
    }
  }

  /** The tab's ×, or middle-click. Closing the last tab collapses the DOCK
   *  (drawer mode only) — an empty dock is dead space; embedded panels never
   *  auto-collapse (the shell falls back to the surface picker). */
  closeTab(chatId: string, key: string): void {
    const chat = this.#chats.get(chatId);
    if (chat === undefined) {
      return;
    }
    const ix = chat.tabs.findIndex((tab) => tab.key === key);
    if (ix === -1) {
      return;
    }
    const [tab] = chat.tabs.splice(ix, 1);
    tab!.controller.close();
    tab!.term.dispose();
    chat.active = activeAfterClose(chat.active, ix, chat.tabs.length);
    if (chat.tabs.length === 0 && chat.open && this.#mode === "drawer") {
      chat.open = false;
    }
    this.#bump();
  }

  /** Drag-reorder commit (desktop commit_reorder). */
  reorderTab(chatId: string, from: number, to: number): void {
    const chat = this.#chats.get(chatId);
    if (chat === undefined || from === to) {
      return;
    }
    reorderTabs(chat.tabs, from, to);
    chat.active = activeAfterReorder(chat.active, from, to);
    this.#bump();
  }

  /**
   * Mount (or re-mount) a tab's host element: the xterm element moves into
   * it, the fitter measures, and the first mount fires `OpenTerminal` with
   * the real grid size. Re-mounts (chat navigation, panel reopen) only
   * re-attach and re-fit — the PTY and emulator state persist.
   */
  attachTab(chatId: string, key: string, host: HTMLElement): void {
    const chat = this.#chats.get(chatId);
    const tab = chat?.tabs.find((candidate) => candidate.key === key);
    if (chat === undefined || tab === undefined) {
      return;
    }
    const element = tab.term.element;
    if (element === undefined) {
      tab.term.open(host);
    } else if (element.parentElement !== host) {
      host.appendChild(element);
    }
    this.fitActive(chatId);
    // A reserved placeholder waits for its engine-side run to answer —
    // `OpenTerminal` here would race the action's own PTY open.
    if (!tab.openRequested && !tab.reserved && this.#session !== null) {
      tab.openRequested = true;
      void tab.controller.open(tab.term.cols, tab.term.rows).then(() => {
        const shell = tab.controller.shell;
        if (shell !== null) {
          tab.title = shellTitle(shell);
          this.#bump();
        }
      });
    }
  }

  /** Fit the active tab's emulator to its host and debounce the resize RPC
   *  (the emulator itself resizes immediately, desktop on_grid_metrics). */
  fitActive(chatId: string): void {
    const chat = this.#chats.get(chatId);
    const tab = chat?.tabs[chat.active];
    if (chat === undefined || tab === undefined || tab.term.element === undefined) {
      return;
    }
    tab.fitter.fit();
    tab.controller.resize(tab.term.cols, tab.term.rows);
  }

  /** Focus the active tab's input (tab select, panel open, "+" click). */
  focusActive(chatId: string): void {
    const chat = this.#chats.get(chatId);
    chat?.tabs[chat.active]?.term.focus();
  }

  /** Re-apply the installed variant's terminal roles to every emulator. */
  retheme(): void {
    const theme = currentTerminalTheme();
    if (theme === undefined) {
      return;
    }
    for (const chat of this.#chats.values()) {
      for (const tab of chat.tabs) {
        tab.term.options.theme = theme;
      }
    }
  }

  #chat(chatId: string): ChatTerminals {
    let chat = this.#chats.get(chatId);
    if (chat === undefined) {
      chat = {
        open: false,
        // The drawer's height is the persisted global default (healed to
        // [160, 2000] on load; the 55%-viewport clamp is runtime-only).
        height: this.#mode === "drawer" ? uiSettings.getSnapshot().terminalHeight : TERMINAL_DEFAULT_HEIGHT,
        tabs: [],
        active: 0,
        nextKey: 1,
      };
      this.#chats.set(chatId, chat);
    }
    return chat;
  }

  #addTab(chatId: string, chat: ChatTerminals, key?: string, title?: string, reserved = false): void {
    if (this.#session === null) {
      return;
    }
    const client = this.#session.client;
    const tabKey = key ?? `${chat.nextKey++}`;
    const theme = currentTerminalTheme();
    const fonts = this.#fonts;
    const term = new XTerm({
      fontFamily: fonts.family,
      fontSize: fonts.size,
      lineHeight: terminalLineHeight(fonts.size),
      cursorStyle: "block",
      // Desktop view.rs: the unfocused cursor is an outline of the same
      // translucent `theme.cursor` color.
      cursorInactiveStyle: "outline",
      // SCROLLBACK_LINES — the desktop emulator keeps 10,000 lines.
      scrollback: 10_000,
      theme,
    });
    const fitter = new FitAddon();
    term.loadAddon(fitter);
    const tab: TerminalTabRecord = {
      key: tabKey,
      title: title ?? `Terminal ${chat.tabs.length + 1}`,
      oscTitle: null,
      exited: false,
      term,
      fitter,
      controller: new TerminalSessionController({
        client,
        chatId,
        sink: {
          write: (bytes) => term.write(bytes),
          exited: () => {
            tab.exited = true;
            this.#bump();
          },
        },
      }),
      openRequested: false,
      reserved,
    };
    attachClipboardPolicy(term, tab.controller);
    term.onData((data) => tab.controller.input(data));
    term.onTitleChange((title) => {
      tab.oscTitle = title;
      this.#bump();
    });
    chat.tabs.push(tab);
    chat.active = chat.tabs.length - 1;
  }

  #bump(): void {
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** A tab's display label: the OSC title when set, else the fallback. */
export function displayTitle(tab: TerminalTabRecord): string {
  const osc = tab.oscTitle?.trim();
  return osc !== undefined && osc.length > 0 ? osc : tab.title;
}

/**
 * The drawer host under the conversation column — the desktop's
 * `Shell::terminal` panel entity. Mod+J toggles it.
 */
export const drawerTerminalStore = new TerminalStore("drawer");

/**
 * The right pane's embedded host — the desktop's `Shell::right_terminal`
 * panel entity. Its tabs are the pane's per-instance Terminal surfaces; it
 * never shares PTYs with the drawer.
 */
export const paneTerminalStore = new TerminalStore("embedded");

/**
 * Binds both terminal hosts to the engine session above the route outlet
 * (mirroring the desktop's shell-level panel entities): chat navigation
 * keeps tabs alive, a session swap closes every tab, and a theme-variant
 * change on `<html>` re-themes every emulator.
 */
export function TerminalProvider({ children }: { children: ReactNode }) {
  const session = useEngineSession();

  useEffect(() => {
    drawerTerminalStore.bindSession(session);
    paneTerminalStore.bindSession(session);
  }, [session]);

  useEffect(() => {
    drawerTerminalStore.retheme();
    paneTerminalStore.retheme();
    const observer = new MutationObserver(() => {
      drawerTerminalStore.retheme();
      paneTerminalStore.retheme();
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
    return () => observer.disconnect();
  }, []);

  return <>{children}</>;
}
