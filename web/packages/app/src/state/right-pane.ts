import { useSyncExternalStore } from "react";
import { rightPaneMaxWidth, rightPaneTakeoverWidth } from "./layout";
import { changesSurfaceStore } from "./changes-surface";
import { disposeHistoryStore } from "./history-store";
import { fileDocuments } from "./file-documents";
import {
  FILES_PANEL_MAX,
  FILES_PANEL_MIN,
  RIGHT_PANE_DEFAULT,
  RIGHT_PANE_MIN,
  uiSettings,
} from "./ui-settings";

/**
 * The pane's embedded terminal host — `terminal/store.tsx`'s
 * `paneTerminalStore`, injected at boot by the surface registry rather than
 * imported (that module pulls xterm.js, which the node-environment tests
 * must not load). `open_tab_for_selected` / `close_tab_by_key` /
 * `tab_summaries` are the desktop peers (panel.rs:482-510).
 */
export interface PaneTerminalSource {
  /** Create a tab with the caller's key (the surface id); false = no host. */
  openTabFor(chatId: string, key: string): boolean;
  /** Close the tab (kills the PTY) — the surface ✕ / middle-click. */
  closeTab(chatId: string, key: string): void;
  /** The tab's display title, or null when the tab is gone. */
  tabTitle(chatId: string, key: string): string | null;
}

/**
 * The right pane's surface model and per-chat panel flags — the desktop's
 * `RightSurface` / `SessionPanels` (`shell.rs:457-532`, `:1901-1922`).
 *
 * Tabs are **created on demand** from the surface picker or the `+` menu; the
 * list starts empty and `resolvedActive` falls back to the picker when it
 * empties. N file tabs, N diff tabs, subagent tabs, one Terminal.
 *
 * The file EXPLORER is no longer a surface tab (tickets 22/23 parity): it is
 * a docked portion of the one right pane, owned by the per-chat `filesOpen`
 * flag — the pane toggle drives only the surface host, the explorer toggle
 * opens the pane alone when it was closed, and closing the last surface tab
 * collapses the pane unless the explorer is docked.
 *
 * Open/expanded/active/tabs are **per chat, in memory, all-defaults-closed**
 * (`ChatPanels` is never persisted on the desktop either). The *widths* are
 * the pieces that persist — globally, through ticket 03's settings store
 * (`settings.right_pane_width`, `settings.files_panel_width`), not per chat.
 *
 * `Browser(u64)` is deliberately absent: a web client cannot host arbitrary
 * cross-origin pages in a pane (research §6), and the desktop's preview list
 * is the empty-tab body of that surface — there is no Preview surface.
 */

/** `shell.rs::RightSurface` minus the desktop-only `Browser(u64)` variant. */
export type RightSurface =
  | { kind: "picker" }
  | { kind: "file"; id: string }
  | { kind: "diff"; id: string }
  | { kind: "terminal"; id: string }
  | { kind: "subagent"; id: string };

/** Surfaces compare by value (kind + id); the key makes that one string. */
export function surfaceKey(surface: RightSurface): string {
  return "id" in surface ? `${surface.kind}:${surface.id}` : surface.kind;
}

/**
 * `collapse_surfaces_if_empty` (fe45a1cd): with the last surface tab gone,
 * the surface host collapses — unless the docked explorer keeps the pane
 * alive. Pure, so the node-environment tests drive it directly.
 */
export function collapseSurfacesIfEmpty(pane: ChatPaneState): ChatPaneState {
  if (!pane.open || pane.tabs.length > 0 || pane.filesOpen) {
    return pane;
  }
  return { ...pane, open: false, expanded: false, active: { kind: "picker" } };
}

/** Value equality — the tab list's `retain`/`contains` predicate. */
export function surfaceEqual(a: RightSurface, b: RightSurface): boolean {
  return surfaceKey(a) === surfaceKey(b);
}

/**
 * `shell.rs::push_unique_right_surface`: append unless already present.
 * Returns false (and pushes nothing) when the surface already has a tab.
 * Mutating, like the Rust — callers pass their own working list.
 */
export function pushUniqueRightSurface(
  tabs: RightSurface[],
  surface: RightSurface,
): boolean {
  if (tabs.some((tab) => surfaceEqual(tab, surface))) {
    return false;
  }
  tabs.push(surface);
  return true;
}

/**
 * `shell.rs::workspace_file_title`: the tab title is the path's basename.
 * Both separators, because workspace paths may arrive POSIX- or Win32-shaped.
 */
export function workspaceFileTitle(path: string): string {
  const base = path.split(/[/\\]/).pop() ?? path;
  return base.length > 0 ? base : path;
}

/**
 * `shell.rs::panel_key`: per-chat flags key. The new-chat canvas keys per
 * space (`space-canvas:{space}`) so a canvas toggle can never read as global
 * state across unrelated spaces.
 */
export function panelKey(chatId: string | null, space: string): string {
  return chatId === null ? `space-canvas:${space}` : chatId;
}

/** What the tab strip and pane need to know about a live surface. */
export interface SurfaceFacts {
  readonly title: string;
  /** The tooltip / accessible detail — a file's full workspace path. */
  readonly detail: string | null;
  /** `Changes::is_history()` — switches the diff chip's icon to git-branch. */
  readonly isHistory: boolean;
  readonly isDirty: boolean;
}

/**
 * `shell.rs::right_surface_rows`: walk the stored order, describing each tab
 * from its backing entity; entries whose entity is gone are skipped. `null`
 * from `describe` is the "gone" signal. The chat id rides along because a
 * terminal's live tab title is per-chat panel state.
 */
export function rightSurfaceRows(
  tabs: readonly RightSurface[],
  describe: (surface: RightSurface, chatId: string) => SurfaceFacts | null,
  chatId: string,
): { surface: RightSurface; facts: SurfaceFacts }[] {
  const rows: { surface: RightSurface; facts: SurfaceFacts }[] = [];
  for (const surface of tabs) {
    const facts = describe(surface, chatId);
    if (facts !== null) {
      rows.push({ surface, facts });
    }
  }
  return rows;
}

export interface ChatPaneState {
  readonly open: boolean;
  /** Takeover: the pane's width derives from the viewport, not the drag. */
  readonly expanded: boolean;
  /** The docked explorer portion — independent of the surface host. */
  readonly filesOpen: boolean;
  /** The stored pick — may be stale; read through `resolvedActive`. */
  readonly active: RightSurface;
  /** Tab order, drag-reorderable. Starts EMPTY (`shell.rs::493-532`). */
  readonly tabs: readonly RightSurface[];
  readonly width: number;
}

/**
 * `settings.rs` RIGHT_PANE_DEFAULT / _MIN, re-exported from the settings store
 * that owns them.
 */
export { RIGHT_PANE_DEFAULT, RIGHT_PANE_MIN };

function initial(): ChatPaneState {
  return {
    open: false,
    expanded: false,
    filesOpen: false,
    active: { kind: "picker" },
    tabs: [],
    // The persisted global width — what was last dragged, healed to its floor.
    width: uiSettings.getSnapshot().rightPaneWidth,
  };
}

/**
 * `shell.rs::resolved_right_active`: the stored pick if it still exists in
 * the live tab list, else the first remaining tab, else `Picker`. Terminal
 * keys go stale when their tab closes — never render a dead surface.
 */
export function resolvedActive(pane: ChatPaneState): RightSurface {
  if (pane.tabs.some((tab) => surfaceEqual(tab, pane.active))) {
    return pane.active;
  }
  return pane.tabs[0] ?? { kind: "picker" };
}

/** A diff's flavour — plain / History / a pinned commit (`changes.rs`). */
export type DiffFlavor = "diff" | "history" | "commit";

/** `diffs`' backing row: flavour, tab label, and the commit pin if any. */
export interface DiffMeta {
  readonly flavor: DiffFlavor;
  readonly label: string | null;
  /** The pinned sha (`Changes::for_commit`); commit flavour only. */
  readonly commitSha?: string;
  /** The pinned commit's raw subject; commit flavour only. */
  readonly subject?: string;
}

export class RightPaneStore {
  #byChat = new Map<string, ChatPaneState>();
  #version = 0;
  readonly #listeners = new Set<() => void>();
  /**
   * `pending_file_closes` (shell.rs:2882): surfaces whose close waits on
   * unsaved edits — the surface stays until the saves land (pending) or the
   * user picks Retry / Keep Open / Discard (blocked).
   */
  readonly #closeRequests = new Set<string>();

  /**
   * The pane's embedded terminal host (`Shell::right_terminal`). Injected by
   * the surface registry at boot; tests pass a fake so xterm never loads in
   * the node environment. Null until wired — terminal surfaces mint nothing
   * without a host (the desktop's `if let Some(tab)` guard).
   */
  #terminals: PaneTerminalSource | null = null;

  // Backing entities, keyed by surface id — the desktop's `file_surfaces`,
  // `diffs` and `subagent_tabs` maps. Ids are monotonic and never reused, so
  // a tab's title is stable for its whole life.
  #fileSeq = 0;
  #diffSeq = 0;
  #terminalSeq = 0;
  #subagentSeq = 0;
  /** `file_surfaces` — id → workspace path plus the panel that opened it. */
  readonly #files = new Map<string, { path: string; panel: string }>();
  /** `file_surface_keys` — `${panel}\u{0}${path}` → id (one tab per path). */
  readonly #fileKeys = new Map<string, string>();
  /** The RevealFile request the docked explorer column consumes. */
  #pendingReveal: { chatId: string; path: string; seq: number } | null = null;
  /** `diffs` — id → flavour + label (scope label / pinned commit subject). */
  readonly #diffMeta = new Map<string, DiffMeta>();
  /** `subagent_tabs` — id → { chatId, docId, title, frozen }. One tab per doc. */
  readonly #subagentMeta = new Map<string, { chatId: string; docId: string; title: string; frozen: boolean }>();

  constructor(terminals: PaneTerminalSource | null = null) {
    this.#terminals = terminals;
  }

  /** Boot wiring: hand the pane its embedded terminal host. */
  setTerminalSource(terminals: PaneTerminalSource): void {
    this.#terminals = terminals;
  }

  getVersion = (): number => this.#version;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  stateFor(chatId: string): ChatPaneState {
    return this.#byChat.get(chatId) ?? initial();
  }

  #update(chatId: string, next: (current: ChatPaneState) => ChatPaneState): void {
    this.#byChat.set(chatId, next(this.stateFor(chatId)));
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /**
   * The titlebar's one trailing control (`toggle-changes`). Closing always
   * leaves takeover mode (`toggle_right_pane`, `shell.rs:1970-1975`) —
   * reopening after a takeover close lands in normal mode. The pane toggle
   * drives ONLY the surface host (fe45a1cd): the docked explorer portion is
   * independent and stays as it was.
   */
  toggle(chatId: string): void {
    this.#update(chatId, (current) =>
      current.open
        ? { ...current, open: false, expanded: false }
        : { ...current, open: true },
    );
  }

  /**
   * Open on a given surface — what a shortcut or a link into a surface does.
   * Re-picking the active surface while open closes the pane, matching the
   * desktop's toggle semantics for its panel shortcuts.
   */
  show(chatId: string, surface: RightSurface): void {
    this.#update(chatId, (current) =>
      current.open && surfaceEqual(current.active, surface)
        ? { ...current, open: false, expanded: false }
        : { ...current, open: true, active: surface },
    );
  }

  /**
   * A shortcut's way in (`Mod+R`): the first tab of this kind, minting one
   * if none exists, then `show` — so the chord toggles the surface it opened.
   * The minting runs through the add paths so the tab list and the stored
   * pick move together (a bare `#mint` would open the pane onto a surface
   * `resolvedActive` cannot see). The Files chord opens the docked explorer
   * portion, not a tab.
   */
  revealSurface(chatId: string, kind: "files" | "terminal" | "diff"): void {
    if (kind === "files") {
      this.openFilesPanel(chatId);
      return;
    }
    const existing = this.stateFor(chatId).tabs.find((tab) => tab.kind === kind);
    if (existing !== undefined) {
      this.show(chatId, existing);
      return;
    }
    if (kind === "terminal") {
      this.addTerminalSurface(chatId);
    } else {
      this.addDiffSurface(chatId, "diff");
    }
  }

  /** `set_right_active` — sets the stored pick and opens the pane. */
  setActive(chatId: string, surface: RightSurface): void {
    this.#update(chatId, (current) => ({ ...current, open: true, active: surface }));
  }

  /** `toggle_right_pane_expand` — session-local view state, never persisted. */
  toggleExpanded(chatId: string): void {
    this.#update(chatId, (current) => ({ ...current, expanded: !current.expanded }));
  }

  close(chatId: string): void {
    this.#update(chatId, (current) => ({ ...current, open: false, expanded: false }));
  }

  #mint(kind: "terminal" | "diff", flavor: DiffFlavor | null): RightSurface {
    if (kind === "terminal") {
      this.#terminalSeq += 1;
      return { kind: "terminal", id: `t${this.#terminalSeq}` };
    }
    this.#diffSeq += 1;
    this.#diffMeta.set(`d${this.#diffSeq}`, { flavor: flavor ?? "diff", label: null });
    return { kind: "diff", id: `d${this.#diffSeq}` };
  }

  /**
   * The explorer toggle (`toggle_files_panel`, 1de3b4ba/fe45a1cd): opens the
   * pane with only the explorer portion when the pane was closed, and closes
   * just that portion when open. `set_surfaces_open(false)` keeps the host's
   * state untouched — programmatic opens never close a pane the user has open.
   */
  toggleFilesPanel(chatId: string): void {
    const current = this.stateFor(chatId);
    if (!current.filesOpen) {
      this.openFilesPanel(chatId);
      return;
    }
    this.#update(chatId, (pane) => ({ ...pane, filesOpen: false }));
  }

  /** `add_files_surface` — dock the explorer portion, opening nothing else. */
  openFilesPanel(chatId: string): void {
    this.#update(chatId, (pane) => ({ ...pane, filesOpen: true }));
  }

  /** `close_files_panel` — programmatic close of the explorer portion only. */
  closeFilesPanel(chatId: string): void {
    this.#update(chatId, (pane) => ({ ...pane, filesOpen: false }));
  }

  /**
   * `set_surfaces_open` (1de3b4ba): programmatic surface opens (file links,
   * subagent chips) open the HOST without ever closing it, and without
   * touching the explorer portion.
   */
  setSurfacesOpen(chatId: string, open: boolean): void {
    this.#update(chatId, (pane) => (pane.open === open ? pane : { ...pane, open }));
  }

  /**
   * `FilesEvent::RevealFile` → `add_files_surface` + `reveal_file_explicit`:
   * dock the explorer portion and ask its tree to reveal the path. The docked
   * column consumes the pending reveal on its model.
   */
  revealInFilesPanel(chatId: string, path: string): void {
    this.#pendingReveal = { chatId, path, seq: (this.#pendingReveal?.seq ?? 0) + 1 };
    this.openFilesPanel(chatId);
  }

  /** The docked column's reveal request (consumed on render). */
  pendingFilesReveal(): { chatId: string; path: string; seq: number } | null {
    return this.#pendingReveal;
  }

  /** The docked column clears its consumed reveal. */
  clearFilesReveal(): void {
    this.#pendingReveal = null;
  }

  /**
   * `add_file_surface`: one tab per `(panel, path)`. An already-open path
   * activates the existing tab; a fresh open mints a monotonic id so the
   * basename title is stable for the tab's whole life.
   */
  addFileSurface(chatId: string, path: string, panelFor: string = chatId): void {
    const key = `${panelFor}\u{0}${path}`;
    const existingId = this.#fileKeys.get(key);
    if (existingId !== undefined) {
      this.setActive(chatId, { kind: "file", id: existingId });
      return;
    }
    this.#fileSeq += 1;
    const id = `f${this.#fileSeq}`;
    this.#files.set(id, { path, panel: panelFor });
    this.#fileKeys.set(key, id);
    this.#update(chatId, (pane) => ({ ...pane, tabs: [...pane.tabs, { kind: "file", id }] }));
    this.setActive(chatId, { kind: "file", id });
  }

  /**
   * `add_diff_surface` / `add_history_surface` / `add_commit_diff_surface`:
   * every click opens a FRESH diff tab with its own scope selection (no
   * dedupe — N clicks make N tabs). A commit-pinned tab carries the sha +
   * subject its surface pins (`Changes::for_commit`, never re-scooped).
   */
  addDiffSurface(chatId: string, flavor: DiffFlavor, label: string | null = null): void {
    const surface = this.#mint("diff", flavor);
    if (surface.kind === "diff" && label !== null) {
      const meta = this.#diffMeta.get(surface.id);
      if (meta !== undefined) {
        this.#diffMeta.set(surface.id, { ...meta, label });
      }
    }
    this.#update(chatId, (pane) => ({ ...pane, tabs: [...pane.tabs, surface] }));
    this.setActive(chatId, surface);
  }

  /**
   * `add_commit_diff_surface` (shell.rs:2613) — a History row click: a NEW
   * pinned commit-diff tab per click, titled with the commit's trimmed
   * subject or its first 7 sha chars (`tab_title`, changes.rs:1725).
   */
  addCommitDiffSurface(
    chatId: string,
    commit: { sha: string; subject: string },
  ): void {
    const surface = this.#mint("diff", "commit");
    if (surface.kind !== "diff") {
      return;
    }
    const subject = commit.subject.trim();
    this.#diffMeta.set(surface.id, {
      flavor: "commit",
      label: subject.length > 0 ? subject : commit.sha.slice(0, 7),
      commitSha: commit.sha,
      subject: commit.subject,
    });
    this.#update(chatId, (pane) => ({ ...pane, tabs: [...pane.tabs, surface] }));
    this.setActive(chatId, surface);
  }

  /**
   * A diff surface's backing meta — flavor, scope label, and (for the
   * commit-pinned flavour) the sha + subject the pane mounts with.
   */
  diffMetaOf(surfaceId: string): DiffMeta | null {
    return this.#diffMeta.get(surfaceId) ?? null;
  }

  /**
   * The picker's Terminal card / `+` row: every click opens a FRESH embedded
   * terminal tab (`add_terminal_surface`, shell.rs:2634-2650) — the surface
   * id IS the terminal tab's key, so the pane chip addresses its own PTY
   * (`Terminal(tab)` surfaces, one per instance).
   */
  addTerminalSurface(chatId: string): void {
    if (this.#terminals === null) {
      return;
    }
    const surface = this.#mint("terminal", null);
    if (surface.kind === "terminal" && !this.#terminals.openTabFor(chatId, surface.id)) {
      return;
    }
    this.#update(chatId, (pane) => ({ ...pane, tabs: [...pane.tabs, surface] }));
    this.setActive(chatId, surface);
  }

  /**
   * `add_subagent_surface` (shell.rs:2682): one tab per doc. Added
   * programmatically from a transcript spawn chip, never from the picker.
   * `frozen` (subagent done/failed) tries the uploaded transcript blob
   * first and falls back to the live doc watch; running subagents watch
   * the doc directly.
   */
  addSubagentSurface(
    chatId: string,
    spawn: { chatId: string; docId: string; title: string; frozen: boolean },
  ): void {
    for (const [id, meta] of this.#subagentMeta) {
      if (meta.docId === spawn.docId) {
        this.setActive(chatId, { kind: "subagent", id });
        return;
      }
    }
    this.#subagentSeq += 1;
    const id = `s${this.#subagentSeq}`;
    this.#subagentMeta.set(id, { chatId: spawn.chatId, docId: spawn.docId, title: spawn.title, frozen: spawn.frozen });
    this.#update(chatId, (pane) => ({ ...pane, tabs: [...pane.tabs, { kind: "subagent", id }] }));
    this.setActive(chatId, { kind: "subagent", id });
  }

  /** The subagent tab's instance (`{chatId, docId, title, frozen}`). */
  subagentSurfaceOf(surfaceId: string): { chatId: string; docId: string; title: string; frozen: boolean } | null {
    const meta = this.#subagentMeta.get(surfaceId);
    if (meta === undefined) {
      return null;
    }
    return { ...meta };
  }

  /**
   * `close_right_surface` (shell.rs:2788): the ✕ — removes THAT tab, not
   * the pane. A File surface with a live document consults `prepare_close`
   * first: "allow" (or no document) completes immediately; "pending" /
   * "blocked" keep the tab, reveal it, and mark the close request — the
   * surface's banner carries the rest (ticket 25's contract with the
   * desktop's `on_file_close_ready` / `complete_file_close`).
   */
  closeSurface(chatId: string, surface: RightSurface): void {
    if (surface.kind === "picker") {
      return;
    }
    if (surface.kind === "file") {
      const disposition = fileDocuments.prepareClose(surface.id);
      if (disposition === null || disposition === "allow") {
        this.#completeFileClose(chatId, surface);
        return;
      }
      // `set_right_active`: reveal the surface the close is waiting on.
      this.#update(chatId, (current) =>
        surfaceEqual(current.active, surface) ? current : { ...current, open: true, active: surface },
      );
      this.#closeRequests.add(surfaceKey(surface));
      this.#notify();
      return;
    }
    this.#update(chatId, (current) => {
      const tabs = current.tabs.filter((tab) => !surfaceEqual(tab, surface));
      const nextActive: RightSurface = surfaceEqual(current.active, surface)
        ? { kind: "picker" }
        : current.active;
      return collapseSurfacesIfEmpty({ ...current, tabs, active: nextActive });
    });
    // Per-kind teardown: drop the backing entity so a stale id never
    // resolves again (`diffs.remove`, `subagent_tabs.remove`, …). A diff
    // tab also drops its per-surface Changes state (scope/folds), which
    // outlives the tab's component tree by design — and a History tab its
    // GitHistory store (commits/search state).
    if (surface.kind === "diff") {
      this.#diffMeta.delete(surface.id);
      changesSurfaceStore.dispose(chatId, surface.id);
      disposeHistoryStore(chatId, surface.id);
    } else if (surface.kind === "subagent") {
      this.#subagentMeta.delete(surface.id);
    } else if (surface.kind === "terminal") {
      // `close_right_surface`'s Terminal branch: the panel closes THAT tab
      // (killing the PTY, `close_tab_by_key`, shell.rs:2832-2835).
      this.#terminals?.closeTab(chatId, surface.id);
    }
  }

  /** `complete_file_close` (shell.rs:2992): the close finally happens. */
  completeFileClose(chatId: string, surface: RightSurface): void {
    if (surface.kind !== "file") {
      this.closeSurface(chatId, surface);
      return;
    }
    this.#completeFileClose(chatId, surface);
  }

  #completeFileClose(chatId: string, surface: RightSurface): void {
    this.#closeRequests.delete(surfaceKey(surface));
    this.#update(chatId, (current) => {
      const tabs = current.tabs.filter((tab) => !surfaceEqual(tab, surface));
      const nextActive: RightSurface = surfaceEqual(current.active, surface)
        ? { kind: "picker" }
        : current.active;
      return collapseSurfacesIfEmpty({ ...current, tabs, active: nextActive });
    });
    if (surface.kind === "file") {
      this.#dropFileEntity(surface.id);
      // The document's real teardown (the `file_surfaces` entity drop) —
      // its buffer survived every tab switch until now.
      fileDocuments.disposeSurface(surface.id);
    }
  }

  /** `cancel_file_close` (shell.rs:2907): Keep Open. */
  cancelFileClose(chatId: string, surface: RightSurface): void {
    if (this.#closeRequests.delete(surfaceKey(surface))) {
      this.#notify();
    }
  }

  /** `pending_file_closes.contains` — the surface shows the close banner. */
  isCloseRequested(surface: RightSurface): boolean {
    return this.#closeRequests.has(surfaceKey(surface));
  }

  /** The surface's backing path (`file_surface_paths`, shell.rs). */
  filePathOf(surfaceId: string): string | null {
    return this.#files.get(surfaceId)?.path ?? null;
  }

  #notify(): void {
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /**
   * Public notify for external-but-owned state that the rows render: the
   * file documents' dirty flags flow through here (the desktop's
   * TitleChanged event fan-out's peer).
   */
  notify(): void {
    this.#notify();
  }

  #dropFileEntity(id: string): void {
    const entry = this.#files.get(id);
    this.#files.delete(id);
    if (entry !== undefined) {
      this.#fileKeys.delete(`${entry.panel}\u{0}${entry.path}`);
    }
  }

  /**
   * `rename_file_surface`: the tab keeps its id and its position; only its
   * title changes. An existing entry for the new path wins (`or_insert`).
   */
  renameFileSurface(id: string, oldPath: string, newPath: string): void {
    const entry = this.#files.get(id);
    if (entry === undefined || entry.path !== oldPath) {
      return;
    }
    this.#files.set(id, { ...entry, path: newPath });
    this.#fileKeys.delete(`${entry.panel}\u{0}${oldPath}`);
    const key = `${entry.panel}\u{0}${newPath}`;
    if (!this.#fileKeys.has(key)) {
      this.#fileKeys.set(key, id);
    }
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /**
   * The live backing row for a surface, or `null` when the entity is gone —
   * `right_surface_rows`' skip signal. Titles are contextual (gap R18):
   * a file's basename, a diff's scope label or pinned commit subject, a
   * terminal's own tab title, a subagent's name.
   */
  describe(surface: RightSurface, chatId: string | null = null): SurfaceFacts | null {
    switch (surface.kind) {
      case "picker":
        return { title: "Picker", detail: null, isHistory: false, isDirty: false };
      case "file": {
        const entry = this.#files.get(surface.id);
        if (entry === undefined) {
          return null;
        }
        return {
          title: workspaceFileTitle(entry.path),
          detail: entry.path,
          isHistory: false,
          // `right_surface_rows`' dirty dot — the live document's
          // unflushed edits (the file surface registers itself).
          isDirty: fileDocuments.isDirtyFor(surface.id),
        };
      }
      case "diff": {
        const meta = this.#diffMeta.get(surface.id);
        if (meta === undefined) {
          return null;
        }
        return {
          title: meta.label ?? (meta.flavor === "history" ? "History" : "Diffs"),
          detail: null,
          isHistory: meta.flavor === "history",
          isDirty: false,
        };
      }
      case "terminal": {
        // `right_surface_rows`: the terminal tab's own title — the live
        // OSC/shell-basename label from the embedded panel's
        // `tab_summaries`; a gone tab means the row disappears.
        const title = chatId === null ? null : (this.#terminals?.tabTitle(chatId, surface.id) ?? null);
        if (title === null) {
          return null;
        }
        return { title, detail: null, isHistory: false, isDirty: false };
      }
      case "subagent": {
        const meta = this.#subagentMeta.get(surface.id);
        if (meta === undefined) {
          return null;
        }
        return { title: meta.title, detail: null, isHistory: false, isDirty: false };
      }
    }
  }

  /** `right_surface_rows` over a chat's stored order. */
  surfaceRows(chatId: string): { surface: RightSurface; facts: SurfaceFacts }[] {
    return rightSurfaceRows(this.stateFor(chatId).tabs, (surface, chat) => this.describe(surface, chat), chatId);
  }

  /**
   * A drag sample. `max` is the room left by the sidebar and the
   * conversation's floor (`rightPaneMaxWidth`) — the desktop's
   * `on_right_pane_drag`: clamp into [MIN, max] when both fit, and when they
   * cannot, hand the scarce space to the conversation and let the pane sit
   * below its own minimum.
   */
  setWidth(chatId: string, width: number, max: number): void {
    const clamped =
      max >= RIGHT_PANE_MIN ? Math.min(max, Math.max(RIGHT_PANE_MIN, width)) : max;
    this.#update(chatId, (current) => ({ ...current, width: clamped }));
    // The drag also moves the global default, coalesced into one write.
    uiSettings.update({ rightPaneWidth: clamped }, "debounced");
  }

  /** Double-clicking the seam restores the default (`shell.rs:7953`). */
  resetWidth(chatId: string): void {
    this.#update(chatId, (current) => ({ ...current, width: RIGHT_PANE_DEFAULT }));
    uiSettings.update({ rightPaneWidth: RIGHT_PANE_DEFAULT }, "immediate");
  }

  /**
   * The docked explorer column's drag (`on_files_panel_drag`): clamped into
   * [FILES_PANEL_MIN, FILES_PANEL_MAX] and coalesced into the settings store.
   */
  setFilesPanelWidth(width: number): void {
    const clamped = Math.min(FILES_PANEL_MAX, Math.max(FILES_PANEL_MIN, width));
    uiSettings.update({ filesPanelWidth: clamped }, "debounced");
    this.#notify();
  }

  /** Drag-reorder in the tab strip. */
  moveTab(chatId: string, from: number, to: number): void {
    this.#update(chatId, (current) => {
      if (from === to || from < 0 || from >= current.tabs.length) {
        return current;
      }
      const tabs = [...current.tabs];
      const [moved] = tabs.splice(from, 1);
      if (moved === undefined) {
        return current;
      }
      tabs.splice(Math.max(0, Math.min(tabs.length, to)), 0, moved);
      return { ...current, tabs };
    });
  }
}

export const rightPaneStore = new RightPaneStore();

/*
 * The tab strip's dirty dots re-render when a document's dirty state
 * flips: the registry notifies, the store bumps its version. (The desktop
 * re-renders its tab rows off the same TitleChanged event fan-out.)
 */
fileDocuments.setDirtyListener(() => {
  rightPaneStore.notify();
});

/**
 * The pane's laid-out width — the desktop's `shell.rs::right_target`. The
 * stored width is what the user dragged; what it resolves to depends on the
 * window and the sidebar, so a narrowing window shrinks the pane without
 * destroying the width the user chose.
 */
export function resolvePaneWidth(
  pane: ChatPaneState,
  viewport: number,
  sidebar: number,
): number {
  if (!pane.open) {
    return 0;
  }
  if (pane.expanded) {
    return rightPaneTakeoverWidth(viewport, sidebar);
  }
  return Math.min(pane.width, rightPaneMaxWidth(viewport, sidebar));
}

export function useRightPane(chatId: string): ChatPaneState {
  useSyncExternalStore(rightPaneStore.subscribe, rightPaneStore.getVersion);
  return rightPaneStore.stateFor(chatId);
}
