import { useEffect, useMemo, useRef } from "react";
import type { ReactNode } from "react";
import { useNavigate } from "@tanstack/react-router";
import { Icon, harnessBrandIcon } from "@zeron/icons";
import { useEngineSession } from "../state/session-provider";
import { useNow, useWatchSnapshot } from "../state/hooks";
import { useSidebar } from "../state/sidebar";
import { useResolvedAppearance } from "../state/appearance";
import { appearanceStore } from "../state/appearance";
import { useChatChangeRequests } from "../state/change-requests-store";
import { ESCAPE_PRIORITY, registerEscapeSurface } from "../state/escape";
import { commandPaletteStore, useCommandPaletteSnapshot } from "../state/command-palette";
import {
  actionsFor,
  paletteChats,
  type CommandAction,
  type PaletteChatRow,
} from "../lib/command-palette";
import { highlightRanges } from "../lib/add-space";
import { statusWord } from "../lib/view";
import { classifyKey } from "../lib/picker-search";
import { badgeCombo, isMacPlatform } from "../state/shortcuts";
import { RbDialogGlass } from "./base/dialog";
import { KbdHint } from "./ui/KeyHint";
import { MenuRowNav } from "./ui/MenuRows";
import { ChangeRequestBadge } from "./change-request-badge";

/**
 * The command palette — the web port of the desktop's
 * `render_command_palette` (`crates/ui/src/shell/command_palette.rs`,
 * upstream 1400f7ab): a 600px glass card (the shared `modal_glass` surface,
 * 14px radius) with an Actions section over the global chat history.
 * Search matches highlight inside labels (the same `highlightRanges` the
 * add-space palette uses), the keyboard highlight wraps and scrolls, and
 * Enter runs the entry: New chat routes to the canvas, New project opens
 * the add-space step ladder, Open settings routes to settings, and a chat
 * row routes to that chat. Archived chats stay searchable; every chat's
 * metadata (project, device, branch, PR) is a search target.
 *
 * The mount lifecycle rides `RbDialogGlass` (see `state/command-palette.ts`);
 * the escape ladder closes it at the `commandPalette` priority — first on
 * the ladder, above the blocking overlays, matching the desktop's
 * capture-phase check order. Headless while closed: the change-request
 * watches sit empty until the card is up.
 */

/** One palette entry in render order: an action, or a chat-history row. */
type PaletteEntry =
  | { readonly kind: "action"; readonly action: CommandAction }
  | { readonly kind: "chat"; readonly row: PaletteChatRow };

export function CommandPalette() {
  const session = useEngineSession();
  const snapshot = useWatchSnapshot(session);
  const now = useNow(10_000);
  const sidebar = useSidebar();
  const state = useCommandPaletteSnapshot();
  const resolvedAppearance = useResolvedAppearance();
  const navigate = useNavigate();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const fadeRef = useRef<HTMLDivElement | null>(null);

  // The navigation hops ride refs so the session binding only re-runs when
  // the session does (the add-space palette's pattern).
  const goToCanvasRef = useRef(() => {
    void navigate({ to: "/" });
  });
  goToCanvasRef.current = () => {
    void navigate({ to: "/" });
  };
  const openChatRef = useRef((chatId: string) => {
    void navigate({ to: "/chat/$chatId", params: { chatId } });
  });
  openChatRef.current = (chatId: string) => {
    void navigate({ to: "/chat/$chatId", params: { chatId } });
  };
  const openSettingsRef = useRef(() => {
    void navigate({ to: "/settings" });
  });
  openSettingsRef.current = () => {
    void navigate({ to: "/settings" });
  };
  useEffect(() => {
    commandPaletteStore.attach({
      session,
      goToCanvas: () => {
        goToCanvasRef.current();
      },
      openChat: (chatId) => {
        openChatRef.current(chatId);
      },
      openSettings: () => {
        openSettingsRef.current();
      },
    });
  }, [session]);
  // Only a true host unmount force-closes — nothing is left to paint, so
  // no exit window either.
  useEffect(() => () => commandPaletteStore.forceClose(), []);

  // The shell's Escape ladder owns Escape at the command palette's
  // priority — above every other surface, matching the desktop's
  // capture-phase check order.
  useEffect(() => {
    if (state.status === "closed") {
      return;
    }
    return registerEscapeSurface(ESCAPE_PRIORITY.commandPalette, () => {
      commandPaletteStore.close();
      // Consumed either way — closing still counts; a second Escape in the
      // exit window must not fall through to the chat interrupt.
      return true;
    });
  }, [state.status]);

  // `focus_pending`: the search input takes focus on open.
  useEffect(() => {
    if (state.status === "open") {
      inputRef.current?.focus({ preventScroll: true });
    }
  }, [state.status]);

  const live = state.status !== "closed";
  const chats = live ? (snapshot?.chats.rows ?? []) : [];
  const changeRequests = useChatChangeRequests(
    session?.client ?? null,
    chats,
    session?.client.engineInfo?.deviceId ?? null,
  );
  const entries = useMemo<PaletteEntry[]>(() => {
    if (!live) {
      return [];
    }
    const actions = actionsFor(state.query, resolvedAppearance === "dark").map(
      (action) => ({ kind: "action", action }) as const,
    );
    const history = paletteChats({
      chats,
      spaces: snapshot?.spaces.rows ?? [],
      statuses: snapshot?.statuses.rows ?? [],
      devices: snapshot?.devices.rows ?? [],
      changeRequests,
      now,
      query: state.query,
      sort: sidebar.sort,
    }).map((row) => ({ kind: "chat", row }) as const);
    return [...actions, ...history];
  }, [live, state.query, chats, snapshot, changeRequests, now, sidebar.sort, resolvedAppearance]);

  // Every search edit scrolls back to the top (the desktop resets the
  // scroll offset on `Edited`).
  useEffect(() => {
    listRef.current?.scrollTo({ top: 0 });
  }, [state.query]);
  // Keyboard reveal keeps the highlighted row in view (`scroll_to_item`).
  useEffect(() => {
    const row = listRef.current?.children.item(state.active);
    row?.scrollIntoView({ block: "nearest" });
  }, [state.active]);
  // The results viewport's edge fade (edge_faded, RESULTS_FADE_BAND 18):
  // gated per edge from the live scroll offset, 1px dead-zone — the same
  // contract as the sidebar's scroll fade.
  useEffect(() => {
    const wrap = fadeRef.current;
    const scroller = listRef.current;
    if (wrap === null || scroller === null) {
      return;
    }
    let raf = 0;
    const apply = (): void => {
      raf = 0;
      const top = scroller.scrollTop > 1.0;
      const bottom = scroller.scrollTop < scroller.scrollHeight - scroller.clientHeight - 1.0;
      wrap.style.setProperty("--rb-command-fade-top", top ? "1" : "0");
      wrap.style.setProperty("--rb-command-fade-bottom", bottom ? "1" : "0");
    };
    const schedule = (): void => {
      if (raf === 0) {
        raf = requestAnimationFrame(apply);
      }
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(scroller);
    scroller.addEventListener("scroll", schedule, { passive: true });
    apply();
    return () => {
      observer.disconnect();
      scroller.removeEventListener("scroll", schedule);
      if (raf !== 0) {
        cancelAnimationFrame(raf);
      }
    };
  }, [state.status]);

  if (state.status === "closed") {
    return null;
  }

  function activate(index: number): void {
    const entry = entries[index];
    if (entry === undefined) {
      return;
    }
    if (entry.kind === "action") {
      if (entry.action.id === "theme") {
        // The quick theme action targets the opposite of the resolved
        // appearance and keeps the palette open so it updates to its next
        // state (upstream b4dd24d7).
        const mode = entry.action.theme;
        if (mode !== undefined) {
          commandPaletteStore.activateEntry({
            kind: "theme",
            setTheme: () => {
              appearanceStore.setMode(mode);
            },
          });
        }
        return;
      }
      commandPaletteStore.activateEntry({ kind: entry.action.id });
    } else {
      commandPaletteStore.activateEntry({ kind: "chat", chatId: entry.row.chat.id });
    }
  }

  function keyDown(event: React.KeyboardEvent<HTMLInputElement>): boolean {
    const key = classifyKey(
      event.nativeEvent.key,
      event.nativeEvent.metaKey,
      event.nativeEvent.ctrlKey,
    );
    switch (key) {
      case "escape":
        commandPaletteStore.close();
        return true;
      case "up":
        commandPaletteStore.move(-1, entries.length);
        return true;
      case "down":
        commandPaletteStore.move(1, entries.length);
        return true;
      case "enter":
        // The desktop latches Enter against X11's unflagged repeats
        // (EnterPress); the DOM reports repeats directly (`event.repeat`),
        // so a held Enter activates once until release.
        if (event.nativeEvent.repeat) {
          return true;
        }
        activate(state.active);
        return true;
      default:
        return false;
    }
  }

  const actionCount = entries.reduce(
    (count, entry) => (entry.kind === "action" ? count + 1 : count),
    0,
  );
  // End spacing belongs to the content, so it scrolls out of the fade
  // instead of leaving a permanent gutter beside the chrome (b4dd24d7).
  const rows: ReactNode[] = [];
  entries.forEach((entry, ix) => {
    if (ix === actionCount && actionCount > 0) {
      rows.push(
        <div key="command-separator" className="command-palette-divider" role="separator" />,
      );
    }
    rows.push(
      <PaletteRow
        key={`command-row-${ix}`}
        entry={entry}
        ix={ix}
        first={ix === 0}
        last={ix + 1 === entries.length}
        active={ix === state.active}
        query={state.query}
        sidebar={sidebar}
        onClick={() => {
          activate(ix);
        }}
      />,
    );
  });

  return (
    <RbDialogGlass
      open={state.status === "open"}
      onOpenChange={(next) => {
        if (!next) {
          commandPaletteStore.close();
        }
      }}
      onOpenChangeComplete={(next) => {
        if (!next) {
          commandPaletteStore.unmounted();
        }
      }}
      ariaLabel="Command palette"
      overlaySource="command-palette"
      overlayOpen
      cardClassName="command-palette-frost"
    >
      <div className="command-palette-card">
        <div className="add-space-header command-palette-header">
          <span className="command-palette-search-icon" aria-hidden>
            <Icon name="paletteSearch" size={16} />
          </span>
          <div className="add-space-search">
            <input
              ref={inputRef}
              type="text"
              value={state.query}
              placeholder="Search commands and chats…"
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              onChange={(event) => {
                commandPaletteStore.setQuery(event.target.value);
              }}
              onKeyDown={(event) => {
                // The desktop's "PaletteSearch" context leaves the
                // navigation keys unbound so they bubble to the card.
                if (keyDown(event)) {
                  event.preventDefault();
                }
              }}
            />
          </div>
          <KbdHint>{badgeCombo("mod-k", isMacPlatform())}</KbdHint>
        </div>
        <div className="command-palette-results" ref={fadeRef}>
          <div className="add-space-list command-palette-list" ref={listRef}>
            {entries.length === 0 && (
              <div className="command-palette-empty">
                <span>No results</span>
                <span className="command-palette-empty-hint">
                  Try a command, chat title, project, or device.
                </span>
              </div>
            )}
            {rows}
          </div>
        </div>
        <div className="command-palette-footer">
          <CommandKeyHint keys="↑ ↓" label="Navigate" />
          <CommandKeyHint keys="↵" label="Select" />
          <CommandKeyHint keys="Esc" label="Close" />
        </div>
      </div>
    </RbDialogGlass>
  );
}

/** `command_key_hint`: a kbd chip plus its tiny verb (b4dd24d7). */
function CommandKeyHint(props: { readonly keys: string; readonly label: string }) {
  return (
    <span className="command-key-hint">
      <KbdHint>{props.keys}</KbdHint>
      <span className="command-key-hint-label">{props.label}</span>
    </span>
  );
}

/** One palette row: an action row, or a chat-history row. */
function PaletteRow(props: {
  readonly entry: PaletteEntry;
  readonly ix: number;
  readonly first: boolean;
  readonly last: boolean;
  readonly active: boolean;
  readonly query: string;
  readonly sidebar: ReturnType<typeof useSidebar>;
  readonly onClick: () => void;
}) {
  const { entry, ix, first, last, active, query, sidebar, onClick } = props;
  const spacing = `${first ? "command-row-first" : ""} ${last ? "command-row-last" : ""}`;
  if (entry.kind === "action") {
    return (
      <MenuRowNav
        fadeKey={`command-action-${ix}`}
        highlighted={active}
        onClick={onClick}
        className={`command-palette-action ${spacing}`}
      >
        <Icon name={entry.action.icon} size={16} className="command-palette-action-icon" />
        <Highlighted text={entry.action.label} query={query} />
      </MenuRowNav>
    );
  }
  return (
    <ChatRow
      row={entry.row}
      ix={ix}
      active={active}
      query={query}
      spacing={spacing}
      showBranch={sidebar.showBranch}
      showPullRequest={sidebar.showPullRequest}
      showHarness={sidebar.showHarness}
      onClick={onClick}
    />
  );
}

/**
 * The chat-history row — the palette's copy of the sidebar session row:
 * "project @ device" + status on line one, harness + title on line two,
 * branch + PR badge below (the desktop reuses `render_chat_row` with
 * palette-namespaced ids; the web's palette row is its own element, so it
 * never animates the sidebar copy).
 */
function ChatRow(props: {
  readonly row: PaletteChatRow;
  readonly ix: number;
  readonly active: boolean;
  readonly query: string;
  readonly spacing: string;
  readonly showBranch: boolean;
  readonly showPullRequest: boolean;
  readonly showHarness: boolean;
  readonly onClick: () => void;
}) {
  const { row, ix, active, query, spacing, showBranch, showPullRequest, showHarness, onClick } = props;
  const brand = showHarness && row.harness !== null ? harnessBrandIcon(row.harness) : null;
  const branch = showBranch ? row.branch : null;
  const changeRequest = showPullRequest ? row.changeRequest : null;
  const word = statusWord(row.status);
  return (
    <button
      type="button"
      data-rb-row-key={`command-chat-${ix}`}
      className={`command-chat-row ${spacing} ${active ? "command-chat-row-active" : ""}`}
      onClick={onClick}
    >
      <span className="command-chat-line command-chat-line-1">
        <Highlighted text={row.folder} query={query} />
        <span className="command-chat-status">{word !== null ? word : row.timeAgo}</span>
      </span>
      <span className="command-chat-line command-chat-line-2">
        {brand !== null && (
          <Icon
            name={brand.name}
            size={13}
            className="command-chat-harness"
            style={brand.tint !== null ? { color: brand.tint } : undefined}
          />
        )}
        <Highlighted text={row.title} query={query} />
      </span>
      {(branch !== null || changeRequest !== null) && (
        <span className="command-chat-line command-chat-line-3">
          {branch !== null && (
            <>
              <Icon name="gitBranch" size={11} className="command-chat-branch-icon" />
              <Highlighted text={branch} query={query} />
            </>
          )}
          <span className="command-chat-rest" />
          {changeRequest !== null && <ChangeRequestBadge summary={changeRequest} />}
        </span>
      )}
    </button>
  );
}

/** Match-highlighted label (popover.rs `search_highlight`). */
function Highlighted(props: { readonly text: string; readonly query: string }) {
  const ranges = highlightRanges(props.text, props.query);
  if (ranges.length === 0) {
    return <span className="command-chat-label">{props.text}</span>;
  }
  const parts: ReactNode[] = [];
  let at = 0;
  ranges.forEach((range, ix) => {
    if (range.start > at) {
      parts.push(props.text.slice(at, range.start));
    }
    parts.push(
      <span key={ix} className="add-space-hl">
        {props.text.slice(range.start, range.end)}
      </span>,
    );
    at = range.end;
  });
  if (at < props.text.length) {
    parts.push(props.text.slice(at));
  }
  return <span className="command-chat-label">{parts}</span>;
}
