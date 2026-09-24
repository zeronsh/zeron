import { useEffect, useMemo, useRef, useState } from "react";
import { Icon, type IconName } from "@zeron/icons";
import type { Space } from "@zeron/proto";
import { methods, parseScopedId } from "@zeron/engine-client";
import { useEngineSessions } from "../state/session-provider";
import type { EngineSession } from "../state/engine-session";
import { engineStatesOf, useFleetRegistry, useFleetSnapshot } from "../state/fleet";
import { useNow } from "../state/hooks";
import { sidebarStore, useSidebar } from "../state/sidebar";
import { uiSettings } from "../state/ui-settings";
import { healedSpaceFilter, mergePendingSpaces, spaceDeviceTag, spaceDisplayName, spacesSorted } from "../lib/view";
import { filterIndices } from "../lib/picker-search";
import { flyoutOpensLeft } from "../lib/flyout-side";
import { addSpaceStore, usePendingSpaces } from "../state/add-space";
import { sidebarNotice } from "../state/notice";
import { RbSwitch } from "./base/switch";
import {
  RbContextMenu,
  RbContextMenuPopup,
  RbContextMenuPortal,
  RbContextMenuPositioner,
  RbContextMenuTrigger,
} from "./base/menu";
import { openChipClass } from "./ui/Chip";
import { PickerSearchField, useCursorList } from "./ui/CursorList";
import { Dialog, DialogCard, DialogTitle, DialogBody, DialogField, BtnGhost, BtnPrimary, BtnDanger } from "./ui/Dialog";
import { MenuHeading, MenuRowNav, MenuSeparator } from "./ui/MenuRows";
import { NestedMenu } from "./ui/NestedMenu";
import { PickerCard } from "./ui/PickerCard";
import { MenuScrollbar } from "./ui/Scrollbar";
import { SidebarFadedLabel } from "./sidebar-faded-label";
import { TOOLTIP_VIEW_OPTIONS_MS } from "./ui/Tooltip";

/**
 * The sidebar's space header — the desktop's `render_spaces_filter` row:
 * a 29px disclosure trigger reading "All projects" (or the picked space)
 * with a folder mark, an "@ device" tag hugging the name, an offline glyph
 * when the space's host is stale, and a quiet caret (static — no rotation).
 * The view-options button beside it opens `SidebarViewMenu`.
 *
 * The trigger opens `render_spaces_menu` (spaces.rs:1187-1323) on the shared
 * popover primitive: a search field, "All projects" (only while the query is
 * empty), every space ranked by `filterIndices` with its "@ device" tag and
 * offline glyph, and "New project…" always last. Right-clicking a space row
 * opens `SpaceContextMenu` at the pointer (rename/delete).
 *
 * The picked space both filters the chat list and targets the new-chat flow;
 * a dangling pick — space deleted, or an engine switch — heals to "All
 * projects" at read (`shell.rs`), so the header never shows a name the fleet
 * no longer has.
 */

/** `SPACES_MENU_LIST_MAX_HEIGHT` (spaces.rs) — the menu list's cap. */
const SPACES_MENU_LIST_MAX_HEIGHT = 336;

export function SpaceFilter() {
  // The MERGED fleet snapshot: every paired engine's spaces/devices under
  // scoped ids, so the picker spans the whole fleet.
  const snapshot = useFleetSnapshot();
  const sessions = useEngineSessions();
  const registry = useFleetRegistry();
  const sidebar = useSidebar();
  const now = useNow(30_000);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  // The right-click overlay: the context menu first, then whichever dialog
  // its rows open — the dialog state must outlive the menu's unmount.
  const [spaceOverlay, setSpaceOverlay] = useState<SpaceOverlay | null>(null);
  const engineStates = engineStatesOf(registry);

  // The menu's rows: the merged spaces with the add-space palette's
  // optimistic rows folded in, merged by id (a confirmed row replaces its
  // optimistic twin), in display order.
  const pending = usePendingSpaces();
  const spaces = useMemo(() => {
    const rows = snapshot.spaces.rows;
    return spacesSorted(mergePendingSpaces(rows, pending));
  }, [snapshot?.spaces.rows, pending]);
  const devices = snapshot.devices.rows;
  const filter = healedSpaceFilter(sidebar.spaceFilter, snapshot.spaces.rows);
  const picked = filter === null ? null : spaces.find((space) => space.id === filter) ?? null;
  const label = picked === null ? "All projects" : spaceDisplayName(picked);
  // The "@ device" tag rides the trigger only under a picked space, with
  // the disconnected GLYPH — never words — when its host reads offline
  // (live engine state when one is supervised, else the 70s window).
  const deviceTag = picked === null ? null : spaceDeviceTag(picked, devices, now, engineStates);

  const listRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [query, setQuery] = useState("");

  // Row order: "All projects" (empty query only), spaces ranked by
  // `filterIndices` over the display name, "New project…" always last.
  const labels = useMemo(() => spaces.map((space) => spaceDisplayName(space)), [spaces]);
  const matched = useMemo(() => {
    const indices = filterIndices(query, labels);
    return indices.map((ix) => spaces[ix]!);
  }, [query, labels, spaces]);
  const rows: readonly (Space | "all" | "new")[] = useMemo(() => {
    const list: (Space | "all" | "new")[] = [];
    if (query.trim().length === 0) {
      list.push("all");
    }
    list.push(...matched);
    list.push("new");
    return list;
  }, [query, matched]);

  // The cursor keyboard model stays card-level (blueprint §6.5): ↑/↓ walk
  // `menuStep` over the rows, Enter/Cmd+Enter activate, typing resets to
  // 0 in the input's own onChange. Escape is Base UI's — the dismiss
  // pipeline owns it, and this menu has no focus-return contract.
  const { cursor, setCursor, onKeyDown } = useCursorList({
    enabled: open,
    count: rows.length,
    onActivate: (ix) => {
      const row = rows[ix];
      if (row !== undefined) {
        pick(row);
      }
    },
    listRef,
    rowAttribute: "space-index",
  });

  // Opening: mints a fresh search input; anchors the cursor on the row
  // matching the current filter (spaces.rs:1200-1210).
  const opened = open;
  useEffect(() => {
    if (!opened) {
      return;
    }
    setQuery("");
    const target =
      filter === null ? 0 : rows.findIndex((row) => row !== "all" && row !== "new" && row.id === filter);
    setCursor(target < 0 ? 0 : target);
    // The search input takes focus before first paint (spaces.rs:1207);
    // `initialFocus` below races for it too — same element either way.
    inputRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opened]);

  function pick(row: Space | "all" | "new"): void {
    if (row === "all") {
      sidebarStore.setSpaceFilter(null);
      setOpen(false);
      return;
    }
    if (row === "new") {
      // "New project…" closes the menu, THEN opens the add-space palette
      // (spaces.rs:1204-1207 / §2.7 — `close_space_menu` always runs before
      // the overlay opens; ticket 11's `addSpaceStore` owns the surface).
      setOpen(false);
      addSpaceStore.open();
      return;
    }
    sidebarStore.setSpaceFilter(row.id);
    setOpen(false);
  }

  // The card spans the trigger row's content width — `sidebarWidth - 16`
  // (two SPACE_SM gutters) — and opens 6px below it (`anchorBelow`),
  // clamped 8px inside the window. Floating UI tracks the live trigger.

  // The trigger renders unconditionally, empty engine included (shell.rs:4935
  // gates `render_spaces_filter` on nothing): with zero spaces the label falls
  // back to "All projects" and the menu degenerates to ["All projects",
  // "New project…"], exactly the desktop's empty-engine rows.
  if (!snapshot.spaces.loaded) {
    return null;
  }

  return (
    <>
      <PickerCard
        open={open}
        onOpenChange={setOpen}
        placement="anchorBelow"
        cardClassName="popover-card spaces-menu-card"
        role="listbox"
        ariaLabel="Projects"
        width={rowContentWidth(triggerRef.current)}
        onKeyDown={onKeyDown}
        initialFocus={inputRef}
        trigger={
          <button type="button" ref={triggerRef} className={openChipClass("space-filter-trigger", open)}>
            <Icon name="folder" size={16} className="space-filter-icon" />
            <span className="space-filter-label">
              <SidebarFadedLabel className="space-filter-name">{label}</SidebarFadedLabel>
              {deviceTag !== null && (
                <>
                  <SidebarFadedLabel className="space-filter-tag">{deviceTag.tag}</SidebarFadedLabel>
                  {deviceTag.offline && <Icon name="wifiOff" size={12} className="space-filter-offline" />}
                </>
              )}
            </span>
            <Icon name="altArrowDown" size={14} className="space-filter-caret" />
          </button>
        }
      >
        <PickerSearchField
          inputRef={inputRef}
          value={query}
          onQuery={(value) => {
            setQuery(value);
            setCursor(0);
          }}
          placeholder="Search projects…"
          ariaLabel="Search projects"
        />
        <div className="spaces-menu-scroll-host">
          <div className="spaces-menu-list" id="spaces-menu-list" ref={listRef}>
          {rows.map((row, ix) => {
            if (row === "all") {
              return (
                <MenuRowNav
                  key="all"
                  fadeKey="all"
                  data-space-index={ix}
                  highlighted={ix === cursor && filter !== null}
                  selected={filter === null}
                  onClick={() => pick(row)}
                >
                  <Icon name="folder" size={15} className="spaces-menu-row-icon" />
                  <span className="menu-row-label">All projects</span>
                </MenuRowNav>
              );
            }
            if (row === "new") {
              return (
                <MenuRowNav key="new" fadeKey="new" data-space-index={ix} onClick={() => pick(row)}>
                  <Icon name="plus" size={15} className="spaces-menu-row-icon" />
                  <span className="menu-row-label">New project…</span>
                </MenuRowNav>
              );
            }
            const tag = spaceDeviceTag(row, devices, now, engineStates);
            return (
              <SpaceRowContext
                key={row.id}
                row={row}
                ix={ix}
                highlighted={ix === cursor && row.id !== filter}
                selected={row.id === filter}
                tag={tag}
                onPick={() => pick(row)}
                menuOpen={spaceOverlay !== null && spaceOverlay.kind === "menu" && spaceOverlay.space.id === row.id}
                onMenuOpen={() => setSpaceOverlay({ kind: "menu", space: row })}
                onMenuDismiss={(reason, event) => {
                  setSpaceOverlay(null);
                  // An outside press that landed outside the spaces card too
                  // dismisses BOTH menus — the desktop's every-open-popup
                  // mouse-down-out listener, and the old layer's guard, did
                  // exactly that; Base UI's tree nesting shields the parent
                  // while the child is open, so this closes the gap. Presses
                  // inside the card (the search input, a row) stay open.
                  if (
                    reason === "outside-press" &&
                    open &&
                    !(event instanceof MouseEvent && event.target instanceof Node && spacesCardContains(event.target))
                  ) {
                    setOpen(false);
                  }
                }}
                onOpenDialog={(kind) => {
                  // The context menu always closes before its follow-up
                  // dialog opens, and the spaces menu dismisses with it
                  // (`close_space_menu`, spaces.rs:3278-3283).
                  setSpaceOverlay(kind === "rename" ? { kind: "rename", space: row } : { kind: "delete", space: row });
                  setOpen(false);
                }}
              />
            );
          })}
          </div>
          <MenuScrollbar scrollRef={listRef} />
        </div>
      </PickerCard>
      {spaceOverlay !== null && spaceOverlaySession(sessions, spaceOverlay.space) !== null && (
        <>
          {spaceOverlay.kind === "rename" && (
            <RenameSpaceDialog
              space={spaceOverlay.space}
              onCancel={() => setSpaceOverlay(null)}
              onSubmit={(value) => {
                runSpaceMutate(spaceOverlaySession(sessions, spaceOverlay.space), "renameSpace", { spaceId: spaceOverlay.space.id, name: value });
                setSpaceOverlay(null);
              }}
            />
          )}
          {spaceOverlay.kind === "delete" && (
            <DeleteSpaceDialog
              space={spaceOverlay.space}
              deviceName={devices.find((device) => device.id === spaceOverlay.space.deviceId)?.name ?? "its device"}
              chatCount={
                snapshot?.chats.rows.filter((chat) => chat.spaceId === spaceOverlay.space.id).length ?? 0
              }
              onCancel={() => setSpaceOverlay(null)}
              onConfirm={() => {
                runSpaceMutate(spaceOverlaySession(sessions, spaceOverlay.space), "deleteSpace", { spaceId: spaceOverlay.space.id });
                setSpaceOverlay(null);
              }}
            />
          )}
        </>
      )}
    </>
  );
}

/** The right-click overlay states: the context menu, then its dialogs. */
type SpaceOverlay =
  | { readonly kind: "menu"; readonly space: Space }
  | { readonly kind: "rename"; readonly space: Space }
  | { readonly kind: "delete"; readonly space: Space };

/** The Mutate call for a space mutation, with the notice on failure. */
function runSpaceMutate(
  session: EngineSession | null,
  op: "renameSpace" | "deleteSpace",
  params: Record<string, unknown>,
): void {
  if (session === null) {
    sidebarNotice.set("Engine not connected");
    return;
  }
  void session.client
    .call(methods.MUTATE, { op, ...params })
    .catch((error: unknown) => sidebarNotice.set(error instanceof Error ? error.message : String(error)));
}

/**
 * The session owning a (scoped) space id — the space mutations route to
 * the space's engine, not the currently routed one.
 */
function spaceOverlaySession(
  sessions: ReadonlyMap<string, EngineSession>,
  space: Space,
): EngineSession | null {
  try {
    const engine = parseScopedId(space.id).engine;
    return engine === null ? null : sessions.get(engine) ?? null;
  } catch {
    return null;
  }
}

/**
 * The sidebar row's content width — `sidebarWidth - 16`, what the desktop's
 * space menu card spans (two SPACE_SM gutters on either side of the row).
 */
function rowContentWidth(rowChild: HTMLElement | null): number {
  const parent = rowChild?.parentElement;
  if (parent === null || parent === undefined) {
    return 304;
  }
  return Math.max(200, parent.clientWidth - 16);
}

/**
 * Whether a click target sits inside the OPEN spaces-menu card — the
 * context menu's outside-press handler uses this to decide whether the
 * spaces menu should dismiss too (a press inside the card leaves it open).
 */
function spacesCardContains(target: Node): boolean {
  const card = document.querySelector(".rb-popover-popup[data-open].spaces-menu-card");
  return card instanceof Node && card.contains(target);
}

// ---------------------------------------------------------------------------
// SidebarViewMenu (spaces.rs:871-973) — the sort button's Organize/Sort/Show card
// ---------------------------------------------------------------------------

type ViewRow =
  | { readonly kind: "ByDevice" }
  | { readonly kind: "ByProject" }
  | { readonly kind: "InOneList" }
  | { readonly kind: "LastUpdated" }
  | { readonly kind: "Created" }
  | { readonly kind: "ShowBranch" }
  | { readonly kind: "ShowPullRequest" }
  | { readonly kind: "ShowHarness" }
  | { readonly kind: "ShowProjectIcon" }
  | { readonly kind: "ShowProjectLabel" }
  | { readonly kind: "Compact" };

/**
 * `SIDEBAR_VIEW_ROWS` — the submenu rows in group order (spaces.rs, after
 * upstream f9563394's nesting): Compact moved out of the list into its own
 * switch row and 86249cf0's Create Section joins it below the groups.
 */
const SIDEBAR_VIEW_ROWS: readonly { row: ViewRow; label: string; icon: IconName }[] = [
  { row: { kind: "ByDevice" }, label: "By device", icon: "laptop" },
  { row: { kind: "ByProject" }, label: "By project", icon: "folder" },
  { row: { kind: "InOneList" }, label: "In one list", icon: "list" },
  { row: { kind: "LastUpdated" }, label: "Last updated", icon: "clockCircle" },
  { row: { kind: "Created" }, label: "Created", icon: "calendar" },
  { row: { kind: "ShowBranch" }, label: "Branch", icon: "gitBranch" },
  { row: { kind: "ShowPullRequest" }, label: "Pull request", icon: "pullRequest" },
  { row: { kind: "ShowHarness" }, label: "Harness", icon: "bot" },
  { row: { kind: "ShowProjectIcon" }, label: "Project icon", icon: "projectDefault" },
  { row: { kind: "ShowProjectLabel" }, label: "Location", icon: "folder" },
];

/**
 * `SIDEBAR_VIEW_GROUPS` — the nested menu's three triggers (upstream
 * f9563394): Organize/Sort/Show become fly-out submenus with value
 * summaries; Compact and Create Section (86249cf0) ride below as rows.
 */
const SIDEBAR_VIEW_GROUPS: readonly { label: string; rows: readonly number[] }[] = [
  { label: "Organize", rows: [0, 1, 2] },
  { label: "Sort", rows: [3, 4] },
  { label: "Show", rows: [5, 6, 7, 8, 9] },
];

/** The fly-out submenu card's width (`spaces.rs`'s 232px child). */
const VIEW_SUBMENU_WIDTH = 232;

/**
 * The side probe's margin: the nested placement pins the flyout CARD_INSET +
 * ANCHOR_GAP (10, popover.rs's `nested_menu` geometry — see
 * `nestedMenuPlacement`) beyond the row's far edge, so the probe asks
 * whether the row's right edge plus the whole 232px span clears the
 * window before opening to the right (spaces.rs's canvas probe reads
 * 232 + 12 the same way).
 */
const VIEW_SUBMENU_SIDE_OFFSET = 10;

/**
 * The "Sidebar view options" label's show/hide controller — the web port
 * of gpui's `.tooltip(…)` + `.tooltip_show_delay(350ms)` contract
 * (`spaces.rs:1150-1151`: show after the delay while hovered, dismiss
 * when the pointer leaves or the element unmounts), hand-rolled because
 * the label is the inline-span exception (`ui/Tooltip.tsx` — no library
 * to lean on, so the defensive paths are spelled out): one tracked
 * timer, cleared before every re-arm so enter/leave cycles never stack;
 * `leave`, `blur`, `escape`, and `dispose` (unmount) all clear it and
 * hide. Focus NEVER arms it — the trigger's `aria-label` is the
 * assistive-tech path, and the desktop's visible label is hover-only.
 * The 350ms itself stays the shared `TOOLTIP_VIEW_OPTIONS_MS`
 * (`spaces.rs:960-966`), never inlined.
 */
export function createViewOptionsTooltip(
  setVisible: (visible: boolean) => void,
): {
  enter: () => void;
  leave: () => void;
  blur: () => void;
  escape: () => void;
  dispose: () => void;
} {
  let timer: ReturnType<typeof setTimeout> | null = null;
  function clearTimer(): void {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }
  function dismiss(): void {
    clearTimer();
    setVisible(false);
  }
  return {
    enter() {
      clearTimer();
      timer = setTimeout(() => {
        timer = null;
        setVisible(true);
      }, TOOLTIP_VIEW_OPTIONS_MS);
    },
    leave: dismiss,
    blur: dismiss,
    escape: dismiss,
    dispose: dismiss,
  };
}

export function SidebarViewMenu() {
  const sidebar = useSidebar();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  const [tooltip, setTooltip] = useState(false);

  // The nested menu's state (upstream f9563394's `SidebarViewMenu` fields):
  // which group's submenu is open, the keyboard cursors for the top rows
  // and the open child, and the child's side (it flips left when the row's
  // right edge plus the flyout's whole span would pass the window's right
  // edge — the desktop's canvas probe, read at open). Every hover,
  // corridor, press, Escape, and outside transition now routes through
  // ticket 01's `NestedMenu` (`onSubmenuOpenChange` below): its Base UI
  // hover owns open-on-rest and the safe-polygon corridor, and its dismiss
  // pipeline owns Escape and outside presses — the hand-rolled
  // `HoverIntent` corridor this port carried for the clipped inline
  // submenu goes with it.
  const [submenu, setSubmenu] = useState<number | null>(null);
  const [submenuActive, setSubmenuActive] = useState<number | null>(null);
  const [active, setActive] = useState<number | null>(null);
  const [submenuOnLeft, setSubmenuOnLeft] = useState(false);
  const groupRowRefs = useRef<(HTMLDivElement | null)[]>([]);

  // The label's contract rides the controller above; hovering shorter
  // than the 350ms delay shows nothing. Unmounting the sidebar clears
  // any pending arm.
  const tooltipControl = useMemo(() => createViewOptionsTooltip(setTooltip), []);
  useEffect(() => {
    return () => {
      tooltipControl.dispose();
    };
  }, [tooltipControl]);

  function closeSubmenu(): void {
    setSubmenu(null);
    setSubmenuActive(null);
  }

  function isSelected(row: ViewRow): boolean {
    switch (row.kind) {
      case "ByDevice":
        return sidebar.organization === "byDevice";
      case "ByProject":
        return sidebar.organization === "byProject";
      case "InOneList":
        return sidebar.organization === "inOneList";
      case "LastUpdated":
        return sidebar.sort === "lastUpdated";
      case "Created":
        return sidebar.sort === "created";
      case "ShowBranch":
        return sidebar.showBranch;
      case "ShowPullRequest":
        return sidebar.showPullRequest;
      case "ShowHarness":
        return sidebar.showHarness;
      case "ShowProjectIcon":
        return sidebar.showProjectIcon;
      case "ShowProjectLabel":
        return sidebar.showProjectLabel;
      case "Compact":
        return sidebar.compact;
    }
  }

  /** Radio rows (organization + sort) close the submenu; toggles stay open. */
  function closesSubmenu(row: ViewRow): boolean {
    return (
      row.kind === "ByDevice" ||
      row.kind === "ByProject" ||
      row.kind === "InOneList" ||
      row.kind === "LastUpdated" ||
      row.kind === "Created"
    );
  }

  function activate(row: ViewRow): void {
    // A mouse-driven pick clears the cursor so it doesn't linger
    // (spaces.rs:900-908).
    setActive(null);
    switch (row.kind) {
      case "ByDevice":
        uiSettings.updateImmediate({ sidebarOrganization: "byDevice" });
        break;
      case "ByProject":
        uiSettings.updateImmediate({ sidebarOrganization: "byProject" });
        break;
      case "InOneList":
        uiSettings.updateImmediate({ sidebarOrganization: "inOneList" });
        break;
      case "LastUpdated":
        uiSettings.updateImmediate({ sidebarSort: "lastUpdated" });
        break;
      case "Created":
        uiSettings.updateImmediate({ sidebarSort: "created" });
        break;
      case "ShowBranch":
        uiSettings.updateImmediate({ sidebarShowBranch: !sidebar.showBranch });
        break;
      case "ShowPullRequest":
        // Also flips the change-requests-visible flag.
        uiSettings.updateImmediate({ sidebarShowPullRequest: !sidebar.showPullRequest });
        break;
      case "ShowHarness":
        uiSettings.updateImmediate({ sidebarShowHarness: !sidebar.showHarness });
        break;
      case "ShowProjectIcon":
        uiSettings.updateImmediate({ sidebarShowProjectIcon: !sidebar.showProjectIcon });
        break;
      case "ShowProjectLabel":
        uiSettings.updateImmediate({ sidebarShowProjectLabel: !sidebar.showProjectLabel });
        break;
      case "Compact":
        uiSettings.updateImmediate({ sidebarCompact: !sidebar.compact });
        break;
    }
    if (closesSubmenu(row)) {
      closeSubmenu();
    }
  }

  /** `open_section_dialog(None, ..)`: the menu closes, the chat list's dialog opens. */
  function createSection(): void {
    setOpen(false);
    closeSubmenu();
    setActive(null);
    sidebarStore.openSectionDialog();
  }

  /**
   * `open_sidebar_view_submenu` — opens the child; the keyboard path
   * (ArrowRight/Enter) lands its cursor on row 0, hover and press land it
   * nowhere, and the side probe reads the row's live bounds for the flip.
   */
  function openSubmenu(group: number, keyboard: boolean): void {
    setActive(group);
    setSubmenu(group);
    setSubmenuActive(keyboard ? 0 : null);
    const row = groupRowRefs.current[group];
    if (row != null) {
      // The shared side probe (lib/flyout-side.ts) — the reach is the
      // flyout's whole span (the 232px card + its offset, spaces.rs's
      // canvas probe reading 232 + 12 the same way).
      setSubmenuOnLeft(
        flyoutOpensLeft(row.getBoundingClientRect(), VIEW_SUBMENU_WIDTH + VIEW_SUBMENU_SIDE_OFFSET),
      );
    }
  }

  /**
   * The group flyout's change seam (ticket 01's `NestedMenu`): every
   * request the primitive raises — hover intent (`trigger-hover`), the
   * row's press (`trigger-press`), the corridor's exit, outside presses,
   * Escape — lands here and maps onto the shared `submenu` state the way
   * the desktop's own handlers did: hover opens with no cursor
   * (`open_sidebar_view_submenu(group, false)`), the row press dismisses
   * and highlights its row (the group on_click's "Match model settings:
   * hover opens; clicking dismisses"), and the corridor/outside
   * dismissals clear the row highlight with the child (the exit canvas)
   * while an Escape keeps it (`sidebar_view_menu_key`'s escape closes the
   * child alone). The `submenu !== ix` guards keep a stale transition from
   * clobbering a sibling that has already replaced this group.
   */
  function onSubmenuOpenChange(ix: number, next: boolean, details: { reason: string }): void {
    if (next) {
      if (submenu !== ix) {
        openSubmenu(ix, false);
      }
      return;
    }
    if (submenu !== ix) {
      return;
    }
    if (details.reason === "trigger-press") {
      setActive(ix);
      closeSubmenu();
      return;
    }
    if (details.reason === "escape-key") {
      closeSubmenu();
      return;
    }
    closeSubmenu();
    setActive(null);
  }

  /**
   * `sidebar_view_menu_key` — the cursor menu's keyboard half: up/down walk
   * the open child's rows or the five top rows (three groups + Compact +
   * Create Section), right/enter open a child (space does not, matching
   * the desktop's held-key guards), enter/space activate Compact and
   * Create Section, and escape/left close the child only (the card's own
   * escape — Base UI's dismiss — owns the no-child case).
   */
  function onKey(event: React.KeyboardEvent): void {
    if (submenu !== null) {
      const count = SIDEBAR_VIEW_GROUPS[submenu]!.rows.length;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setSubmenuActive((current) => menuStep(current, count, event.key === "ArrowDown" ? 1 : -1));
        return;
      }
      if (event.key === "Enter") {
        if (submenuActive !== null) {
          event.preventDefault();
          const row = SIDEBAR_VIEW_ROWS[SIDEBAR_VIEW_GROUPS[submenu]!.rows[submenuActive] ?? -1];
          if (row !== undefined) {
            activate(row.row);
          }
        }
        return;
      }
      if (event.key === "Escape" || event.key === "ArrowLeft") {
        event.preventDefault();
        event.stopPropagation();
        closeSubmenu();
        return;
      }
      return;
    }
    const TOP_ROW_COUNT = SIDEBAR_VIEW_GROUPS.length + 2;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      setActive((current) => menuStep(current, TOP_ROW_COUNT, event.key === "ArrowDown" ? 1 : -1));
      return;
    }
    if (active === null) {
      return;
    }
    if (event.key === "ArrowRight" || (event.key === "Enter" && active < SIDEBAR_VIEW_GROUPS.length)) {
      if (active < SIDEBAR_VIEW_GROUPS.length) {
        event.preventDefault();
        openSubmenu(active, true);
      }
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (active === SIDEBAR_VIEW_GROUPS.length) {
        activate({ kind: "Compact" });
      } else if (active === SIDEBAR_VIEW_GROUPS.length + 1) {
        createSection();
      }
    }
  }

  // The group triggers' value summaries (`values` in spaces.rs): the
  // current organization and sort labels; the Show toggles carry none.
  const values: readonly string[] = [
    SIDEBAR_VIEW_ROWS[0 + ([isSelected({ kind: "ByDevice" }), isSelected({ kind: "ByProject" }), isSelected({ kind: "InOneList" })].indexOf(true))]!.label,
    SIDEBAR_VIEW_ROWS[3 + (isSelected({ kind: "LastUpdated" }) ? 0 : 1)]!.label,
  ];

  // `anchorBelowEnd` — right-aligned so the full-width card opens leftward
  // without leaving the sidebar.
  return (
    <>
      <PickerCard
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            closeSubmenu();
            setActive(null);
          }
        }}
        placement="anchorBelowEnd"
        cardClassName="popover-card spaces-menu-card"
        role="menu"
        ariaLabel="Sidebar view options"
        width={rowContentWidth(buttonRef.current)}
        onKeyDown={onKey}
        trigger={
          <button
            type="button"
            ref={buttonRef}
            className={openChipClass("space-filter-sort", open)}
            aria-label="Sidebar view options"
            onMouseEnter={tooltipControl.enter}
            onMouseLeave={tooltipControl.leave}
            onBlur={tooltipControl.blur}
            onKeyDown={(event) => {
              // Escape is a LOCAL dismissal of the visible label — the
              // label is not a popover, so it never joins the shell's
              // escape ladder (state/escape.ts). Enter/Space toggle
              // through the trigger's own click semantics; ArrowDown only
              // opens, never closes (spaces.rs:966-973).
              if (event.key === "Escape" && tooltip) {
                tooltipControl.escape();
                return;
              }
              if (event.key === "ArrowDown" && !open) {
                event.preventDefault();
                event.stopPropagation();
                setOpen(true);
              }
            }}
          >
            {/* 86249cf0 swaps the SORT glyph for MORE_HORIZONTAL at the
                trigger, matching the model pickers' overflow mark. */}
            <Icon name="moreHorizontal" size={16} />
            {tooltip && !open && (
              <span className="space-filter-sort-tooltip" role="tooltip">
                Sidebar view options
              </span>
            )}
          </button>
        }
      >
        <div className="view-menu-rows">
          {SIDEBAR_VIEW_GROUPS.map((group, ix) => (
            // Ticket 01's nested-menu unit: the row renders in place as the
            // flyout's trigger (Base UI adopts it — its className/children/
            // ref stay its own, `aria-expanded`/`aria-controls` merge on,
            // and the press/hover toggle through `onSubmenuOpenChange`),
            // while the choices portal to the body beside it on desktop
            // (the escape from `.popover-card`'s clip — bug 5b's fix) and
            // expand in place under the row on phone (ticket 15's drill).
            <NestedMenu
              key={group.label}
              open={submenu === ix}
              onOpenChange={(next, details) => onSubmenuOpenChange(ix, next, details)}
              side={submenuOnLeft ? "left" : "right"}
              label={group.label}
              ariaLabel={group.label}
              nativeButton={false}
              width={VIEW_SUBMENU_WIDTH}
              cardClassName="popover-card view-menu-flyout"
              trigger={
                <div
                  className={`menu-row view-menu-group-row ${active === ix ? "menu-row-highlighted" : ""} ${
                    submenu === ix ? "menu-row-open" : ""
                  }`}
                  role="menuitem"
                  aria-haspopup="menu"
                  ref={(el) => {
                    groupRowRefs.current[ix] = el;
                  }}
                >
                  <span className="menu-row-label">{group.label}</span>
                  {ix < values.length && <span className="view-menu-summary">{values[ix]}</span>}
                  <Icon name="altArrowRight" size={12} className="view-menu-group-chevron" />
                </div>
              }
            >
              <MenuHeading>{group.label}</MenuHeading>
              <div className="view-menu-rows">
                {group.rows.map((rowIx, choice) => {
                  const entry = SIDEBAR_VIEW_ROWS[rowIx]!;
                  return (
                    <MenuRowNav
                      key={entry.row.kind}
                      fadeKey={entry.row.kind}
                      highlighted={submenuActive === choice && !isSelected(entry.row)}
                      selected={isSelected(entry.row)}
                      onClick={() => activate(entry.row)}
                    >
                      <Icon name={entry.icon} size={15} className="spaces-menu-row-icon" />
                      <span className="menu-row-label">{entry.label}</span>
                      {/* The 14px check slot is always reserved so labels never shift. */}
                      <span className="view-menu-check">
                        {isSelected(entry.row) && <Icon name="check" size={14} />}
                      </span>
                    </MenuRowNav>
                  );
                })}
              </div>
            </NestedMenu>
          ))}
          <MenuSeparator />
          <div
            className={`menu-row view-menu-switch-row ${
              active === SIDEBAR_VIEW_GROUPS.length ? "menu-row-highlighted" : ""
            }`}
            role="menuitem"
            onClick={() => activate({ kind: "Compact" })}
          >
            <span className="menu-row-label">Compact</span>
            {/* The switch is presentation: the row's click owns the write. */}
            <RbSwitch checked={sidebar.compact} aria-label="Compact" tabIndex={-1} />
          </div>
          <div
            className={`menu-row view-menu-create-section ${
              active === SIDEBAR_VIEW_GROUPS.length + 1 ? "menu-row-highlighted" : ""
            }`}
            role="menuitem"
            onClick={createSection}
          >
            <Icon name="plus" size={14} className="view-menu-create-icon" />
            <span className="menu-row-label">Create Section</span>
          </div>
        </div>
      </PickerCard>
    </>
  );
}

/** `popover::menu_step` — the cursor menu's wraparound stepper. */
function menuStep(current: number | null, count: number, delta: number): number | null {
  if (current === null) {
    return delta > 0 ? 0 : count - 1;
  }
  return (current + delta + count) % count;
}

// ---------------------------------------------------------------------------
// SpaceContextMenu (spaces.rs:3336-3389) — rename/delete at the pointer
// ---------------------------------------------------------------------------

/**
 * One space row of the spaces menu, wrapped so a right-click opens the
 * space context menu at the pointer (`RbContextMenu` — clamp-only
 * `menu_at` geometry, 170px card). The `RbContextMenuTrigger` adopts the
 * `MenuRowNav` itself via `render`, so the row's DOM is unchanged.
 */
function SpaceRowContext(props: {
  readonly row: Space;
  readonly ix: number;
  readonly highlighted: boolean;
  readonly selected: boolean;
  readonly tag: { tag: string; offline: boolean };
  readonly onPick: () => void;
  readonly menuOpen: boolean;
  readonly onMenuOpen: () => void;
  /** Any dismissal (Escape, outside press, row pick) — with the reason. */
  readonly onMenuDismiss: (reason: string, event: Event | undefined) => void;
  readonly onOpenDialog: (kind: "rename" | "delete") => void;
}) {
  return (
    <RbContextMenu
      open={props.menuOpen}
      onOpenChange={(next, details) => {
        if (next) {
          props.onMenuOpen();
        } else if (props.menuOpen) {
          props.onMenuDismiss(details.reason, details.event);
        }
      }}
    >
      <RbContextMenuTrigger
        render={
          <MenuRowNav
            fadeKey={props.row.id}
            data-space-index={props.ix}
            highlighted={props.highlighted}
            selected={props.selected}
            onClick={props.onPick}
          >
            <Icon name="folder" size={15} className="spaces-menu-row-icon" />
            <span className="menu-row-label">{spaceDisplayName(props.row)}</span>
            <span className="picker-row-tag">{props.tag.tag}</span>
            {props.tag.offline && <Icon name="wifiOff" size={12} className="picker-row-offline" />}
          </MenuRowNav>
        }
      />
      <RbContextMenuPortal>
        <RbContextMenuPositioner>
          <RbContextMenuPopup
            className="rb-popover-popup popover-card"
            role="menu"
            aria-label="Project actions"
            style={{ width: 170 }}
          >
            <MenuRowNav
              fadeKey="space-menu-rename"
              onClick={() => {
                props.onMenuDismiss("item-press", undefined);
                props.onOpenDialog("rename");
              }}
            >
              <Icon name="pen" size={16} className="spaces-menu-row-icon" />
              <span className="menu-row-label">Rename…</span>
            </MenuRowNav>
            <MenuRowNav
              fadeKey="space-menu-delete"
              className="chat-menu-row-danger"
              onClick={() => {
                props.onMenuDismiss("item-press", undefined);
                props.onOpenDialog("delete");
              }}
            >
              <Icon name="trashBinMinimalistic" size={16} className="spaces-menu-row-icon-danger" />
              <span className="menu-row-label">Remove…</span>
            </MenuRowNav>
          </RbContextMenuPopup>
        </RbContextMenuPositioner>
      </RbContextMenuPortal>
    </RbContextMenu>
  );
}

/** The rename dialog (`open_rename_space` / `submit_rename_space`). */
function RenameSpaceDialog({
  space,
  onCancel,
  onSubmit,
}: {
  readonly space: Space;
  readonly onCancel: () => void;
  readonly onSubmit: (name: string) => void;
}) {
  const [value, setValue] = useState(space.name ?? "");
  const inputRef = useRef<HTMLInputElement | null>(null);

  return (
    <Dialog ariaLabel="Rename project" onClose={onCancel} initialFocus={inputRef}>
      <DialogCard>
        <DialogTitle>Rename project</DialogTitle>
        <form
          className="dialog-form-rows"
          onSubmit={(event) => {
            event.preventDefault();
            if (value.trim().length > 0) {
              onSubmit(value.trim());
            }
            onCancel();
          }}
        >
          <DialogField>
            <input
              ref={inputRef}
              type="text"
              value={value}
              onChange={(event) => setValue(event.target.value)}
              placeholder="Project name"
              spellCheck={false}
              aria-label="Project name"
            />
          </DialogField>
          <div className="dialog-actions-row">
            <BtnGhost type="button" onClick={onCancel}>
              Cancel
            </BtnGhost>
            <BtnPrimary type="submit">Rename</BtnPrimary>
          </div>
        </form>
      </DialogCard>
    </Dialog>
  );
}

/**
 * The delete confirm (spaces.rs:3446-3479): "Remove project?" with the
 * singular/plural session count and curly-quote copy, verbatim.
 */
function DeleteSpaceDialog({
  space,
  deviceName,
  chatCount,
  onCancel,
  onConfirm,
}: {
  readonly space: Space;
  readonly deviceName: string;
  readonly chatCount: number;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  const name = spaceDisplayName(space);
  const copy =
    chatCount === 1
      ? `Removing \u201C${name}\u201D permanently deletes its 1 session on ${deviceName}. This can\u2019t be undone.`
      : `Removing \u201C${name}\u201D permanently deletes its ${chatCount} sessions on ${deviceName}. This can\u2019t be undone.`;
  return (
    <Dialog ariaLabel="Remove project?" onClose={onCancel}>
      <DialogCard>
        <DialogTitle>Remove project?</DialogTitle>
        <DialogBody>{copy}</DialogBody>
        <div className="dialog-actions-row">
          <BtnGhost onClick={onCancel}>Cancel</BtnGhost>
          <BtnDanger onClick={onConfirm}>Remove</BtnDanger>
        </div>
      </DialogCard>
    </Dialog>
  );
}
