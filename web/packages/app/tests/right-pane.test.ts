import { describe, expect, it } from "vitest";
import {
  RightPaneStore,
  panelKey,
  pushUniqueRightSurface,
  resolvedActive,
  surfaceKey,
  workspaceFileTitle,
  type PaneTerminalSource,
  type RightSurface,
} from "../src/state/right-pane";
import { dropIndex, slideOffset } from "../src/components/right-tab-strip";
import { uiSettings } from "../src/state/ui-settings";

/**
 * The right pane's surface model, against the desktop's (`shell.rs:457-532`,
 * `:1901-1922`, `:2284-3004`). Tests mirror the Rust ones by name; each gets
 * a fresh store so the per-chat maps never leak between cases.
 */

/**
 * The pane's embedded terminal host, faked: xterm must not load in the node
 * environment, and a fake keeps the terminal tab entities chat-local and
 * inspectable. Titles follow the desktop's `tab_summaries` (the OSC/shell
 * label, "Terminal N" fallback — here "term" prefixed by the key).
 */
class FakePaneTerminal implements PaneTerminalSource {
  readonly openTabs: string[] = [];
  closed: string[] = [];

  openTabFor(chatId: string, key: string): boolean {
    void chatId;
    this.openTabs.push(key);
    return true;
  }

  closeTab(chatId: string, key: string): void {
    void chatId;
    this.closed.push(key);
    this.openTabs.splice(this.openTabs.indexOf(key), 1);
  }

  tabTitle(chatId: string, key: string): string | null {
    void chatId;
    return this.openTabs.includes(key) ? `term-${key}` : null;
  }
}

function fresh(): { store: RightPaneStore; terminals: FakePaneTerminal } {
  const terminals = new FakePaneTerminal();
  return { store: new RightPaneStore(terminals), terminals };
}

describe("panel keys", () => {
  it("keys the new-chat canvas per space", () => {
    // `panel_key()` — one shared key made a canvas toggle read as global
    // state across unrelated spaces (user report, `shell.rs:1901-1913`).
    expect(panelKey("chat-1", "sp-a")).toBe("chat-1");
    expect(panelKey(null, "sp-a")).toBe("space-canvas:sp-a");
    expect(panelKey(null, "sp-b")).toBe("space-canvas:sp-b");
  });
});

describe("session_panels_default_closed_per_chat", () => {
  it("starts every chat closed, unexpanded, on the picker, with no tabs", () => {
    const { store } = fresh();
    for (const chatId of ["chat-1", "chat-2"]) {
      const pane = store.stateFor(chatId);
      expect(pane.open).toBe(false);
      expect(pane.expanded).toBe(false);
      expect(pane.tabs).toEqual([]);
      expect(pane.active).toEqual({ kind: "picker" });
      expect(resolvedActive(pane)).toEqual({ kind: "picker" });
    }
  });
});

describe("session_panels_flags_are_chat_scoped", () => {
  it("opening one chat's pane leaves every other chat closed", () => {
    const { store } = fresh();
    store.toggle("chat-1");
    expect(store.stateFor("chat-1").open).toBe(true);
    expect(store.stateFor("chat-2").open).toBe(false);
  });
});

describe("session_panels_both_flags_coexist_per_chat", () => {
  it("open and expanded ride together on one chat without crossing chats", () => {
    const { store } = fresh();
    store.toggle("chat-1");
    store.toggleExpanded("chat-1");
    const pane = store.stateFor("chat-1");
    expect(pane.open).toBe(true);
    expect(pane.expanded).toBe(true);
    // Closing always leaves takeover mode (`toggle_right_pane`, P19) —
    // reopening after a takeover close lands in normal mode.
    store.toggle("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, expanded: false });
    store.toggle("chat-1");
    expect(store.stateFor("chat-1").expanded).toBe(false);
    // The other chat never saw any of it.
    expect(store.stateFor("chat-2")).toMatchObject({ open: false, expanded: false });
  });
});

describe("close_resets_logical_flags_immediately (ticket 72)", () => {
  it("close and toggle clear open+expanded in the same commit — no presentation state in the store", () => {
    const { store } = fresh();
    store.setSurfacesOpen("chat-1", true);
    store.toggleExpanded("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, expanded: true });

    // `close()` (Escape / backdrop): the flags reset synchronously — the
    // phone close's width hold is component presentation, never a delayed
    // or deferred flag here.
    store.close("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, expanded: false });

    // `toggle()` out of takeover resets the same way, and the reopen after
    // either close lands in normal mode.
    store.toggle("chat-1");
    store.toggleExpanded("chat-1");
    store.toggle("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, expanded: false });
    store.toggle("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, expanded: false });

    // The state shape is exactly the logical model — the transient close
    // presentation lives in `RightPane`, not in the pane store.
    expect(Object.keys(store.stateFor("chat-1")).sort()).toEqual([
      "active",
      "expanded",
      "filesOpen",
      "open",
      "tabs",
      "width",
    ]);
  });
});

describe("session_panels_update_tracks_right_surfaces", () => {
  it("resolvedActive follows the live tab list and falls back to the picker", () => {
    const { store } = fresh();
    store.addFileSurface("chat-1", "src/main.rs");
    store.addTerminalSurface("chat-1");
    // The surface id IS the embedded terminal tab's key.
    expect(resolvedActive(store.stateFor("chat-1"))).toEqual({ kind: "terminal", id: "t1" });

    // The stored pick goes stale when its tab closes — never render a dead
    // surface; the first remaining tab wins.
    store.closeSurface("chat-1", { kind: "terminal", id: "t1" });
    expect(resolvedActive(store.stateFor("chat-1"))).toEqual({ kind: "file", id: "f1" });

    // Emptied, the surface host collapses (fe45a1cd collapse_surfaces_if_empty)
    // unless the docked explorer keeps the pane alive.
    store.closeSurface("chat-1", { kind: "file", id: "f1" });
    let pane = store.stateFor("chat-1");
    expect(pane.open).toBe(false);
    expect(resolvedActive(pane)).toEqual({ kind: "picker" });
    store.openFilesPanel("chat-1");
    store.addTerminalSurface("chat-1");
    store.closeSurface("chat-1", { kind: "terminal", id: "t2" });
    pane = store.stateFor("chat-1");
    expect(pane.open).toBe(true);
    expect(pane.filesOpen).toBe(true);
  });
});

describe("terminal_surfaces_are_per_instance (add_terminal_surface, shell.rs:2634-2650)", () => {
  it("every click opens a FRESH embedded terminal tab addressing its own PTY", () => {
    const { store, terminals } = fresh();
    store.addTerminalSurface("chat-1");
    store.addTerminalSurface("chat-1");
    const pane = store.stateFor("chat-1");
    expect(pane.tabs).toEqual([{ kind: "terminal", id: "t1" }, { kind: "terminal", id: "t2" }]);
    expect(terminals.openTabs).toEqual(["t1", "t2"]);
    expect(resolvedActive(pane)).toEqual({ kind: "terminal", id: "t2" });

    // The chip title is the terminal tab's own live label, and closing the
    // surface closes THAT tab (close_tab_by_key) — the sibling stays.
    expect(store.describe({ kind: "terminal", id: "t1" }, "chat-1")?.title).toBe("term-t1");
    store.closeSurface("chat-1", { kind: "terminal", id: "t1" });
    expect(terminals.closed).toEqual(["t1"]);
    expect(store.describe({ kind: "terminal", id: "t1" }, "chat-1")).toBeNull();
    expect(resolvedActive(store.stateFor("chat-1"))).toEqual({ kind: "terminal", id: "t2" });

    // A tab that vanished under the pane (its entity gone) disappears from
    // the rows entirely — right_surface_rows' skip signal.
    expect(store.surfaceRows("chat-1")).toHaveLength(1);
  });

  it("without a terminal host wired, the mint is a no-op", () => {
    const store = new RightPaneStore(null);
    store.addTerminalSurface("chat-1");
    expect(store.stateFor("chat-1").tabs).toEqual([]);
  });
});

describe("explorer_is_a_docked_portion_of_one_right_pane (tickets 22/23)", () => {
  it("the files toggle opens the pane alone; the pane toggle drives only the surface host", () => {
    const { store } = fresh();
    // The explorer toggle opens the pane with only its portion.
    store.toggleFilesPanel("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, filesOpen: true });
    // The pane toggle drives only the host: the explorer stays docked.
    store.toggle("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, filesOpen: true });
    store.toggle("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, filesOpen: true });
    store.toggleFilesPanel("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, filesOpen: false });
    // The Files chord routes to the docked portion, never a tab.
    store.revealSurface("chat-1", "files");
    expect(store.stateFor("chat-1").tabs).toHaveLength(0);
    expect(store.stateFor("chat-1").filesOpen).toBe(true);
  });

  it("programmatic file opens never close an open pane (set_surfaces_open)", () => {
    const { store } = fresh();
    store.openFilesPanel("chat-1");
    store.addFileSurface("chat-1", "src/main.rs");
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, filesOpen: true });
    store.setSurfacesOpen("chat-1", true);
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, filesOpen: true });
  });

  it("reveal requests dock the explorer and queue one reveal per path", () => {
    const { store } = fresh();
    store.revealInFilesPanel("chat-1", "src/lib.rs");
    expect(store.stateFor("chat-1").filesOpen).toBe(true);
    expect(store.pendingFilesReveal()).toMatchObject({ chatId: "chat-1", path: "src/lib.rs" });
    store.clearFilesReveal();
    expect(store.pendingFilesReveal()).toBeNull();
  });

  it("the files panel width clamps into the drag bounds", () => {
    const { store } = fresh();
    store.setFilesPanelWidth(40);
    expect(uiSettings.getSnapshot().filesPanelWidth).toBe(220);
    store.setFilesPanelWidth(9000);
    expect(uiSettings.getSnapshot().filesPanelWidth).toBe(440);
  });
});

describe("commit_diff_surfaces_are_independent_pinned_tabs", () => {
  it("each click mints a fresh tab titled with the commit's subject", () => {
    const store = fresh().store;
    store.addDiffSurface("chat-1", "history");
    store.addCommitDiffSurface("chat-1", { sha: "896e31f0abcd", subject: "Merge branch 'feature'" });
    store.addCommitDiffSurface("chat-1", { sha: "216321b0cdef", subject: "  " });
    const pane = store.stateFor("chat-1");
    expect(pane.tabs).toHaveLength(3);

    const [history, first, second] = pane.tabs as [
      { kind: "diff"; id: string },
      { kind: "diff"; id: string },
      { kind: "diff"; id: string },
    ];
    // The titles: the trimmed subject, else the first 7 sha chars
    // (`tab_title`, changes.rs:1725).
    expect(store.describe(first)?.title).toBe("Merge branch 'feature'");
    expect(store.describe(second)?.title).toBe("216321b");
    expect(store.describe(history)?.title).toBe("History");
    expect(store.describe(history)?.isHistory).toBe(true);
    expect(store.describe(first)?.isHistory).toBe(false);

    // The pins the surfaces mount with (`Changes::for_commit`).
    expect(store.diffMetaOf(first.id)).toMatchObject({ flavor: "commit", commitSha: "896e31f0abcd" });
    expect(store.diffMetaOf(second.id)).toMatchObject({ flavor: "commit", commitSha: "216321b0cdef" });

    // Closing a pinned tab drops only its own meta — the others stay; the
    // stored pick resets and the first remaining tab (History) resolves.
    store.closeSurface("chat-1", second);
    expect(store.diffMetaOf(second.id)).toBeNull();
    expect(store.diffMetaOf(first.id)?.commitSha).toBe("896e31f0abcd");
    expect(resolvedActive(store.stateFor("chat-1"))).toEqual(history);
  });
});

describe("file_editors_are_distinct_surface_tabs_with_stable_titles", () => {
  it("one tab per path, basename titles, ids stable across reorder and reopen", () => {
    const { store } = fresh();
    store.addFileSurface("chat-1", "src/lib/shell.rs");
    store.addFileSurface("chat-1", "web/packages/app/src/main.tsx");
    let pane = store.stateFor("chat-1");
    expect(pane.tabs).toHaveLength(2);

    const [first, second] = pane.tabs as [{ kind: "file"; id: string }, { kind: "file"; id: string }];
    expect(first.id).not.toBe(second.id);
    expect(store.describe(first)?.title).toBe("shell.rs");
    expect(store.describe(second)?.title).toBe("main.tsx");
    // The tooltip/aria detail is the full workspace path.
    expect(store.describe(first)?.detail).toBe("src/lib/shell.rs");

    // Reopening the same path activates the existing tab — no duplicate.
    store.addFileSurface("chat-1", "src/lib/shell.rs");
    pane = store.stateFor("chat-1");
    expect(pane.tabs).toHaveLength(2);
    expect(resolvedActive(pane)).toEqual(first);

    // Reorder: the tab keeps its id and position only moves.
    store.moveTab("chat-1", 0, 1);
    pane = store.stateFor("chat-1");
    expect(pane.tabs[1]).toEqual(first);
    expect(store.describe(first)?.title).toBe("shell.rs");

    // A rename keeps the id and the position; only the title changes.
    store.renameFileSurface(first.id, "src/lib/shell.rs", "src/lib/shell2.rs");
    expect(store.describe(first)?.title).toBe("shell2.rs");
    expect(store.stateFor("chat-1").tabs[1]).toEqual(first);
    // And the old path is gone from the one-tab-per-path index: a fresh
    // open of the old path mints a NEW tab.
    store.addFileSurface("chat-1", "src/lib/shell.rs");
    expect(store.stateFor("chat-1").tabs).toHaveLength(3);
  });
});

describe("surface keys and value equality", () => {
  it("compares surfaces by kind + id", () => {
    expect(surfaceKey({ kind: "picker" })).toBe("picker");
    expect(surfaceKey({ kind: "file", id: "f1" })).toBe("file:f1");
    expect(pushUniqueRightSurface([{ kind: "file", id: "f1" }], { kind: "file", id: "f1" })).toBe(false);
    const tabs: RightSurface[] = [{ kind: "file", id: "f1" }];
    expect(pushUniqueRightSurface(tabs, { kind: "terminal", id: "t1" })).toBe(true);
    expect(tabs).toEqual([{ kind: "file", id: "f1" }, { kind: "terminal", id: "t1" }]);
  });

  it("derives the basename title from either separator shape", () => {
    expect(workspaceFileTitle("src/lib/shell.rs")).toBe("shell.rs");
    expect(workspaceFileTitle("src\\lib\\shell.rs")).toBe("shell.rs");
    expect(workspaceFileTitle("shell.rs")).toBe("shell.rs");
  });
});

describe("tab drag geometry", () => {
  it("drop index picks the slot under the pointer", () => {
    // CHIP_SLOT is 116: x 0…115 is slot 0, 116…231 slot 1, and past the end
    // clamps to the last slot (`terminal::panel::drop_index`).
    expect(dropIndex(0, 3)).toBe(0);
    expect(dropIndex(115, 3)).toBe(0);
    expect(dropIndex(116, 3)).toBe(1);
    expect(dropIndex(300, 3)).toBe(2);
    expect(dropIndex(10_000, 3)).toBe(2);
    expect(dropIndex(-20, 3)).toBe(0);
    expect(dropIndex(500, 0)).toBe(0);
  });

  it("siblings slide one slot toward the vacated index", () => {
    const drag = { from: 0, over: 2 };
    expect(slideOffset(drag, 0)).toBe(0);
    // 1 and 2 shift left one slot to open the gap at 2.
    expect(slideOffset(drag, 1)).toBe(-116);
    expect(slideOffset(drag, 2)).toBe(-116);
    expect(slideOffset(drag, 3)).toBe(0);

    const back = { from: 2, over: 0 };
    expect(slideOffset(back, 0)).toBe(116);
    expect(slideOffset(back, 1)).toBe(116);
    expect(slideOffset(back, 2)).toBe(0);

    expect(slideOffset(null, 1)).toBe(0);
    expect(slideOffset({ from: 1, over: 1 }, 1)).toBe(0);
  });
});
