import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useSyncExternalStore } from "react";
import { Icon } from "@zeron/icons";
import { useEngineSession } from "../../state/session-provider";
import { useFleetSnapshot } from "../../state/fleet";
import { chatPageRow } from "../../lib/view";
import { Tooltip, TOOLTIP_VIEW_OPTIONS_MS } from "../ui/Tooltip";
import { GlyphSpinner } from "../glyph-spinner";
import {
  historyStoreFor,
  type HistorySnapshot,
  type HistoryViewMode,
} from "../../state/history-store";
import { HISTORY_COMPARISON_MIN_WIDTH } from "../../lib/git-history";

/**
 * The History tab's toolbar row — the History mode of
 * `Changes::render_header_controls` (history.rs §3.1): the branch-name title
 * (mono, dim, truncated — the tab already says History), the commit count
 * with its ahead/behind pill, then the trailing control group (search
 * control, fetch-all, the All-commits/Branch-tips view toggle, refresh).
 *
 * The store is the shared per-(chat, History tab) instance the pane body
 * drives (`state/history-store.ts`); this row reads and mutates it through
 * the same registry, so the two sibling trees stay in lockstep.
 */

const EMPTY_SNAPSHOT: HistorySnapshot = {  ready: false,
  cwd: null,
  visibleCommits: [],
  viewMode: "allCommits",
  collapsedBranches: new Set(),
  collapsedCounts: new Map(),
  headSha: null,
  searchQuery: "",
  searchActive: false,
  searchLoading: false,
  searchError: null,
  searchTotalCount: null,
  headCommitCount: null,
  totalCount: null,
  comparison: null,
  loading: false,
  error: null,
  fetchingAll: false,
  fetchError: null,
  rowTransitions: null,
  graph: { rows: [], maxLaneCount: 0 },
  graphLaneCapacity: 0,
  hasLoadMore: false,
  avatars: new Map(),
  copiedSha: null,
  listEpoch: 0,
};

/** The no-store stand-ins — identity-stable for `useSyncExternalStore`. */
const EMPTY_GET_SNAPSHOT = (): HistorySnapshot => EMPTY_SNAPSHOT;
const NOOP_SUBSCRIBE = (): (() => void) => () => {};

export function HistoryToolbar({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const session = useEngineSession();
  const store = session !== null ? historyStoreFor(chatId, surfaceId, session.client) : null;
  const subscribe = store?.subscribe ?? NOOP_SUBSCRIBE;
  const getSnapshot = store?.getSnapshot ?? EMPTY_GET_SNAPSHOT;
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  // The branch title (`chat.branch` or "HEAD") — the chat row, read-only.
  const watch = useFleetSnapshot();
  const branch =
    watch.chats.loaded
      ? chatPageRow(chatId, watch.chats.rows, watch.spaces.rows, watch.statuses.rows, Date.now())?.chat.branch ?? null
      : null;

  return (
    <div className="surface-toolbar history-toolbar" role="toolbar" aria-label="History options">
      <span className="history-title mono" title={branch ?? undefined}>
        {branch ?? "HEAD"}
      </span>
      <HistoryCount snapshot={snapshot} />
      <div className="history-tools">
        <HistorySearchControl chatId={chatId} surfaceId={surfaceId} snapshot={snapshot} />
        <HistoryFetchButton
          fetching={snapshot.fetchingAll}
          onFetchAll={() => store?.fetchAll()}
        />
        <HistoryViewButton
          viewMode={snapshot.viewMode}
          onSetViewMode={(mode: HistoryViewMode) => store?.setViewMode(mode)}
        />
        <Tooltip label="Refresh history" delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <button
              type="button"
              id="history-refresh"
              className="history-tool"
              aria-label="Refresh history"
              onClick={() => store?.refresh()}
            >
              <Icon name="refresh" size={14} />
            </button>
          }
        />
      </div>
    </div>
  );
}

/** `GitHistoryCount` (§2.2) — "{n} commits" plus the ahead/behind pill. */
function HistoryCount({ snapshot }: { snapshot: HistorySnapshot }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [wide, setWide] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el === null) {
      return;
    }
    const measure = (): void => {
      setWide((current) => {
        const next = el.clientWidth >= HISTORY_COMPARISON_MIN_WIDTH;
        return current === next ? current : next;
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => {
      observer.disconnect();
    };
  }, []);

  const count = snapshot.searchActive
    ? snapshot.searchTotalCount ?? snapshot.visibleCommits.length
    : snapshot.headCommitCount;
  if (count === null || count === undefined) {
    return <div ref={ref} className="history-count" />;
  }
  const comparison = snapshot.comparison;
  const ahead = comparison?.ahead ?? 0;
  const behind = comparison?.behind ?? 0;
  const showPill = wide && (ahead > 0 || behind > 0);
  const label = count === 1 ? "1 commit" : `${count} commits`;
  return (
    <div ref={ref} className="history-count">
      <span className="history-count-label">{label}</span>
      {showPill && comparison !== null ? (
        <Tooltip
          label={`Compared with ${comparison.base}: ${comparison.ahead} ahead, ${comparison.behind} behind`}
          delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <span className="history-count-pill">
              <span className="history-count-ahead">{`${comparison.ahead} ahead`}</span>
              {comparison.ahead > 0 && comparison.behind > 0 ? (
                <span className="history-count-dot" aria-hidden>
                  ·
                </span>
              ) : null}
              <span className="history-count-behind">{`${comparison.behind} behind`}</span>
            </span>
          }
        />
      ) : null}
    </div>
  );
}

type SearchMode = "collapsed" | "expanded" | "collapsing";

/**
 * `GitHistorySearchControl` (§2.3) — the collapsed magnifier ⇄ the expanded
 * field with a 200ms width/opacity morph (24 ⇄ 196, 0.45 ⇄ 1.0). The query
 * filters the loaded page synchronously; the debounced full search rides
 * behind it in the store. Empty-and-idle for 1.5s auto-collapses; blurring
 * while empty collapses; × clears and re-arms the idle dismiss.
 */
function HistorySearchControl({
  chatId,
  surfaceId,
  snapshot,
}: {
  chatId: string;
  surfaceId: string;
  snapshot: HistorySnapshot;
}) {
  const session = useEngineSession();
  const store = session !== null ? historyStoreFor(chatId, surfaceId, session.client) : null;
  const [mode, setMode] = useState<SearchMode>("collapsed");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const idleTimer = useRef<number | null>(null);
  const collapseTimer = useRef<number | null>(null);
  // The timers outlive renders; read the live state through refs.
  const modeRef = useRef<SearchMode>("collapsed");
  const queryRef = useRef("");
  modeRef.current = mode;
  queryRef.current = snapshot.searchQuery;

  useEffect(
    () => () => {
      if (idleTimer.current !== null) {
        window.clearTimeout(idleTimer.current);
      }
      if (collapseTimer.current !== null) {
        window.clearTimeout(collapseTimer.current);
      }
    },
    [],
  );

  /** `schedule_idle_dismiss` (history.rs:1443) — 1.5s empty-and-idle. */
  const scheduleIdleDismiss = (): void => {
    if (idleTimer.current !== null) {
      window.clearTimeout(idleTimer.current);
    }
    idleTimer.current = window.setTimeout(() => {
      idleTimer.current = null;
      if (modeRef.current === "expanded" && queryRef.current.length === 0) {
        beginCollapse();
      }
    }, 1500);
  };

  /** `begin_collapse` (history.rs:1465) — the tween back, then unmount. */
  const beginCollapse = (): void => {
    if (modeRef.current !== "expanded" || queryRef.current.length > 0) {
      return;
    }
    if (idleTimer.current !== null) {
      window.clearTimeout(idleTimer.current);
      idleTimer.current = null;
    }
    if (collapseTimer.current !== null) {
      window.clearTimeout(collapseTimer.current);
    }
    setMode("collapsing");
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = null;
      setMode((current) => (current === "collapsing" ? "collapsed" : current));
    }, 200);
  };

  const expand = (): void => {
    if (idleTimer.current !== null) {
      window.clearTimeout(idleTimer.current);
      idleTimer.current = null;
    }
    if (collapseTimer.current !== null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    setMode("expanded");
    scheduleIdleDismiss();
  };

  // The expanded input mounts in this state's commit — focus it as soon as
  // it exists (the desktop defers a frame for GPUI; the DOM needs no such
  // workaround).
  useLayoutEffect(() => {
    if (mode === "expanded") {
      inputRef.current?.focus();
    }
  }, [mode]);

  const expanded = mode !== "collapsed";
  return (
    <div className={`history-search ${expanded ? "history-search-open" : ""}`}>
      <button
        type="button"
        id="history-search-trigger"
        className="history-search-trigger"
        aria-label="Search commits"
        aria-expanded={expanded}
        onClick={() => {
          if (mode === "collapsed") {
            expand();
          } else if (mode === "expanded") {
            inputRef.current?.focus();
          }
        }}
      >
        <Icon name="magnifer" size={14} />
      </button>
      {expanded ? (
        <>
          <span className="history-search-status" aria-hidden>
            {snapshot.searchLoading ? (
              <GlyphSpinner size={6} />
            ) : (
              <Icon name="magnifer" size={11} className="history-search-status-idle" />
            )}
          </span>
          <input
            ref={inputRef}
            className="history-search-input"
            type="text"
            placeholder="Search"
            value={snapshot.searchQuery}
            spellCheck={false}
            aria-label="Search commits"
            onChange={(event) => {
              const text = event.target.value;
              if (text.trim().length > 0) {
                if (idleTimer.current !== null) {
                  window.clearTimeout(idleTimer.current);
                  idleTimer.current = null;
                }
              } else {
                scheduleIdleDismiss();
              }
              store?.setSearchQuery(text);
            }}
            onBlur={() => {
              if (snapshot.searchQuery.length === 0) {
                beginCollapse();
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                inputRef.current?.blur();
              }
            }}
          />
          <button
            type="button"
            className="history-search-close"
            aria-label="Clear search"
            onClick={() => {
              if (snapshot.searchQuery.length > 0) {
                store?.setSearchQuery("");
              }
              scheduleIdleDismiss();
            }}
          >
            <Icon name="close" size={9} />
          </button>
        </>
      ) : null}
    </div>
  );
}

/** `GitHistoryFetchButton` (§2.4) — "Fetch all" ⇄ "Fetching…". */
function HistoryFetchButton({ fetching, onFetchAll }: { fetching: boolean; onFetchAll: () => void }) {
  return (
    <button
      type="button"
      id="history-fetch-all"
      className={`history-fetch ${fetching ? "history-fetch-busy" : ""}`}
      disabled={fetching}
      onClick={() => {
        if (!fetching) {
          onFetchAll();
        }
      }}
    >
      {fetching ? (
        <GlyphSpinner size={7} />
      ) : (
        <Icon name="cloud" size={14} className="history-fetch-icon" />
      )}
      <span className="history-fetch-label">{fetching ? "Fetching…" : "Fetch all"}</span>
    </button>
  );
}

/** `GitHistoryViewButton` (§2.5) — All-commits ⇄ Branch-tips. */
function HistoryViewButton({
  viewMode,
  onSetViewMode,
}: {
  viewMode: HistoryViewMode;
  onSetViewMode: (mode: HistoryViewMode) => void;
}) {
  const active = viewMode === "branchTips";
  return (
    <Tooltip
      label={active ? "Show all commits" : "Show branch tips"}
      delay={TOOLTIP_VIEW_OPTIONS_MS}
      trigger={
        <button
          type="button"
          id="history-view-toggle"
          className={`history-tool ${active ? "history-tool-active" : ""}`}
          aria-pressed={active}
          aria-label={active ? "Show all commits" : "Show branch tips"}
          onClick={() => onSetViewMode(active ? "allCommits" : "branchTips")}
        >
          <Icon name="foldVertical" size={14} />
        </button>
      }
    />
  );
}
