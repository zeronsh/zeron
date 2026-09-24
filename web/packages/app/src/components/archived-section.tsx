import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { Icon, harnessBrandIcon } from "@zeron/icons";
import { parseScopedId } from "@zeron/engine-client";
import { useEngineSessions } from "../state/session-provider";
import type { EngineSession } from "../state/engine-session";
import { useFleetChatChangeRequests } from "../state/change-requests-store";
import { useFleetSnapshot } from "../state/fleet";
import { useNow } from "../state/hooks";
import { sidebarStore, useSidebar } from "../state/sidebar";
import { sidebarNotice } from "../state/notice";
import { describeMutateError, setChatArchived } from "../lib/chat-actions";
import { healedSpaceFilter, archivedChatRows, sidebarRowHeight, type ChatRow } from "../lib/view";
import { useChatMenu } from "./chat-menu";
import { ProjectIconMark } from "./project-monogram";
import { SidebarFadedLabel } from "./sidebar-faded-label";
import { SidebarDisclosureBody, SidebarDisclosureHeader, useSidebarDisclosure } from "./sidebar-disclosure";

const INITIAL = 10;
const PAGE = 25;
/** `shell.rs::SIDEBAR_LIST_GAP`. */
const SIDEBAR_LIST_GAP = 2;
/** `spaces.rs::SIDEBAR_DISCLOSURE_BODY_INSET`. */
const SIDEBAR_DISCLOSURE_BODY_INSET = 4;
/** The "Show N more" tail row (the desktop keeps its one-line 36px shape). */
const ARCHIVED_MORE_ROW_HEIGHT = 36;

/**
 * The sidebar's archived shelf (`spaces.rs::render_archived_section` after
 * upstream dfd2fc0c): a collapsible "Archived (N)" disclosure — the count
 * shows only while collapsed — of rows in the user's sidebar sort sharing
 * the ACTIVE list's `ChatRow` data and layout (compact mode, project
 * monograms, branch/PR metadata — `archivedChatRows` is the web peer of
 * `sidebar_chat_data`). Archived history recedes: monogram and title dim
 * at rest and restore on hover/selection; the time label yields to the
 * Unarchive pill on row hover; the tail pages behind "Show N more".
 * Nothing renders when nothing is archived.
 */
export function ArchivedSection() {
  // The MERGED fleet snapshot: archived rows from every paired engine.
  const snapshot = useFleetSnapshot();
  const sessions = useEngineSessions();
  const sidebar = useSidebar();
  const now = useNow(10_000);
  const [shown, setShown] = useState(INITIAL);

  // Hooks must run unconditionally across the empty/loading returns below:
  // the shelf mounts on a page whose first render has no snapshot at all.
  const chats = snapshot.chats;
  const filter = healedSpaceFilter(sidebar.spaceFilter, snapshot.spaces.rows);
  // The change-request watches the active list resolved (shared metadata).
  const visibleChats =
    chats.error === null && chats.loaded
      ? filter === null
        ? chats.rows
        : chats.rows.filter((chat) => chat.spaceId !== undefined && chat.spaceId === filter)
      : [];
  const changeRequests = useFleetChatChangeRequests(sessions, visibleChats);
  const compact = sidebar.compact;
  const showLabel = sidebar.showProjectLabel;
  const rows =
    chats.error === null && chats.loaded
      ? archivedChatRows(
          chats.rows,
          snapshot.spaces.rows,
          filter,
          snapshot.statuses.rows,
          now,
          snapshot.devices.rows,
          {
            sort: sidebar.sort,
            showHarness: sidebar.showHarness,
            showBranch: sidebar.showBranch,
            showPullRequest: sidebar.showPullRequest,
            changeRequests,
          },
        )
      : [];
  const open = sidebar.archivedOpen;
  const visible = rows.slice(0, Math.max(INITIAL, shown));
  const remaining = rows.length - visible.length;
  const rowHeight = (row: ChatRow): number =>
    sidebarRowHeight(compact, showLabel, row.branch !== null, row.changeRequest !== null);
  // The body-height estimate the disclosure tween and collapsed clipping
  // share: inset + rows + gaps + the "Show N more" tail when it pages.
  const bodyHeight =
    SIDEBAR_DISCLOSURE_BODY_INSET +
    visible.reduce((total, row) => total + rowHeight(row), 0) +
    Math.max(visible.length - 1, 0) * SIDEBAR_LIST_GAP +
    (remaining > 0 ? ARCHIVED_MORE_ROW_HEIGHT + SIDEBAR_LIST_GAP : 0);
  const { bodyRef, chevronRef, toggle } = useSidebarDisclosure("archived", open, bodyHeight);

  if (chats.error !== null || !chats.loaded) {
    return null;
  }
  if (rows.length === 0) {
    return null;
  }

  function onToggle(): void {
    // The motion begins on the CURRENT height before the flip; reopening
    // never remembers a previous "show more" expansion.
    toggle();
    sidebarStore.setArchivedOpen(!open);
    setShown(INITIAL);
  }

  return (
    <section className="archived" aria-label="Archived chats">
      <SidebarDisclosureHeader
        id="archived-toggle"
        label={open ? "Archived" : `Archived (${rows.length})`}
        open={open}
        withRule={false}
        chevronRef={chevronRef}
        onToggle={onToggle}
      />
      <SidebarDisclosureBody bodyRef={bodyRef}>
        <ul className="archived-list">
          {visible.map((row) => (
            <ArchivedRow
              key={row.chat.id}
              row={row}
              compact={compact}
              showLabel={showLabel}
              showProjectIcon={sidebar.showProjectIcon}
            />
          ))}
          {remaining > 0 && (
            <li>
              <button
                type="button"
                className="archived-more"
                onClick={() => setShown((current) => Math.max(current, INITIAL) + PAGE)}
              >
                <Icon name="plus" size={14} />
                Show {Math.min(remaining, PAGE)} more
              </button>
            </li>
          )}
        </ul>
      </SidebarDisclosureBody>
    </section>
  );
}

/**
 * The archived row's right-slot choice (`spaces.rs:1669-1712`): exactly
 * ONE child, picked at render — the time-ago at rest, the Unarchive pill
 * while the row is hovered. Never both, and never pinned by focus or
 * touch: no CSS decides this, the row does.
 */
export function archivedRightSlot(hovered: boolean): "time" | "pill" {
  return hovered ? "pill" : "time";
}

function ArchivedRow({
  row,
  compact,
  showLabel,
  showProjectIcon,
}: {
  row: ChatRow;
  compact: boolean;
  showLabel: boolean;
  showProjectIcon: boolean;
}) {
  // Unarchive routes to the row's owning engine off its scoped id.
  const sessions = useEngineSessions();
  const session = archivedSession(sessions, row.chat.id);
  const harness = row.harness;
  const brand = harness === null ? null : harnessBrandIcon(harness);
  const { menu, element } = useChatMenu(row.chat);
  // Per-row hover state — the web equivalent of the desktop's row-hover
  // listener, never a sidebar-store concern.
  const [hovered, setHovered] = useState(false);
  const device = row.deviceName ?? "Unknown device";
  const projectName = row.projectPath === null ? "Home" : row.project;
  const projectSeed = row.projectPath ?? "home";

  function unarchive(event: React.MouseEvent): void {
    // The row's own click opens the chat; only the pill restores.
    event.preventDefault();
    event.stopPropagation();
    if (session === null) {
      sidebarNotice.set("Engine not connected");
      return;
    }
    setChatArchived(session.client, row.chat.id, false).catch((error: unknown) => {
      sidebarNotice.set(describeMutateError(error));
    });
  }

  // Right slot: time at rest; the Unarchive affordance takes its place on
  // row hover — ONE child, chosen at render the way the desktop does it
  // and the way the active rows' corner already does, so no CSS pin can
  // hold the pill on touch or after a click. The pill sits inside the
  // row's Link, so hovering it keeps the row hovered — no flicker. `menu`
  // wraps the Link so a right-click opens the SAME chat context menu the
  // active rows use, at the pointer.
  return (
    <li className="arch-row-item">
      {menu(
        <Link
          to="/chat/$chatId"
          params={{ chatId: row.chat.id}}
          className="arch-row"
          data-compact={compact ? "1" : undefined}
          activeProps={{ className: "arch-row arch-row-active" }}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
        >
          {showProjectIcon && (
            <span className="arch-row-project">
              <ProjectIconMark
                name={projectName}
                seed={projectSeed}
                device={device}
                spaceId={row.chat.spaceId ?? null}
              />
            </span>
          )}
          {brand !== null && (
            <Icon
              name={brand.name}
              size={13}
              className="arch-row-brand"
              style={brand.tint === null ? undefined : { color: brand.tint }}
            />
          )}
          <SidebarFadedLabel className="arch-row-title" fill>
            {row.chat.title === null || row.chat.title.trim().length === 0
              ? "New session"
              : row.chat.title}
          </SidebarFadedLabel>
          {!compact && row.branch !== null && (
            <SidebarFadedLabel className="arch-row-branch">{row.branch}</SidebarFadedLabel>
          )}
          {archivedRightSlot(hovered) === "pill" ? (
            <button
              type="button"
              className="arch-row-unarchive"
              aria-label="Unarchive chat"
              onClick={unarchive}
            >
              <Icon name="archiveUpMinimalistic" size={11} />
              Unarchive
            </button>
          ) : (
            <span className="arch-row-time">{row.timeAgo}</span>
          )}
        </Link>,
      )}
      {element}
    </li>
  );
}

/** The session owning a scoped chat id — the unarchive router. */
function archivedSession(
  sessions: ReadonlyMap<string, EngineSession>,
  chatId: string,
): EngineSession | null {
  try {
    const engine = parseScopedId(chatId).engine;
    return engine === null ? null : sessions.get(engine) ?? null;
  } catch {
    return null;
  }
}
