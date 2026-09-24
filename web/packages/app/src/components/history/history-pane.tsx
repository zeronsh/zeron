import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import type { PointerEvent as ReactPointerEvent, ReactNode, RefObject } from "react";
import { motion } from "@zeron/theme";
import { Icon } from "@zeron/icons";
import type { GitHistoryCommit } from "@zeron/proto";
import { useEngineSession } from "../../state/session-provider";
import { useEngineStatus } from "../../state/hooks";
import { useFleetSnapshot } from "../../state/fleet";
import { useUiSettings, uiSettings } from "../../state/ui-settings";
import { rightPaneStore } from "../../state/right-pane";
import { historyStoreFor, type HistorySnapshot, type HistoryStore } from "../../state/history-store";
import { chatPageRow } from "../../lib/view";
import {
  HISTORY_ROW_HEIGHT,
  hoveredGraphPath,
  interpolateGraphGeometry,
  naturalGeometry,
  resolveHistoryScrollAnchor,
  responsiveGraphGeometry,
  reorderedHistoryColumns,
  shouldUseCompactGraph,
  stabilizedGraphGeometry,
  visibleHistoryColumns,
  historyColumnDropIndex,
  historyColumnWidth,
  resizedHistoryColumnWidths,
  DEFAULT_HISTORY_COLUMN_WIDTHS,
  type GraphGeometry,
  type HistoryColumn,
  type HistoryDataColumn,
} from "../../lib/git-history";
import { HistoryGraph, useGraphPalette } from "./history-graph";
import { HistoryRow } from "./history-row";
import { AuthorMenu, ColumnMenu, type HistoryMenuPoint } from "./history-columns-menu";
import { MatrixSpinner } from "../glyph-spinner";

/**
 * The History pane's body (§2.6-§2.13, `GitHistory::render`): the
 * fetch/search error banners, the column header row (drag-reorder,
 * divider-resize, the columns button, the Author header's right-click), and
 * the virtualized commit list with its SVG lane graph and Load-more footer.
 *
 * Geometry: a `ResizeObserver` feeds `responsiveGraphGeometry` (the desktop's
 * `container_query`); the compact/full flip morphs over 180ms COLLAPSE while
 * plain resizes snap to the 2px-stabilized width. Row-list changes restore
 * the scroll anchor through `resolveHistoryScrollAnchor`; view changes and
 * branch folds animate through the store's interim transition list.
 */

const COLLAPSE_MS = motion.specs.find((spec) => spec.name === "collapse")?.durationMs ?? 180;
/** The virtualized list's overscan (rows above/below the viewport). */
const OVERSCAN = 6;
/** The footer row's height (§2.12). */
const FOOTER_HEIGHT = 48;
/** A pointer must travel this far before a header press reads as a drag. */
const DRAG_ARM_PX = 4;

const EMPTY_SNAPSHOT: HistorySnapshot = {
  ready: false,
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

export function HistoryPane({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const watch = useFleetSnapshot();
  const settings = useUiSettings();

  const client = session?.client ?? null;
  const store: HistoryStore | null =
    client !== null ? historyStoreFor(chatId, surfaceId, client, { authorDisplay: settings.gitHistoryAuthorDisplay }) : null;
  const subscribe = store?.subscribe ?? NOOP_SUBSCRIBE;
  const getSnapshot = store?.getSnapshot ?? EMPTY_GET_SNAPSHOT;
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  // The chat's checkout — where the history lives.
  const deviceId = status !== null && status.state === "connected" ? status.info.deviceId : null;
  const chat =
    watch.chats.loaded
      ? chatPageRow(chatId, watch.chats.rows, watch.spaces.rows, watch.statuses.rows, Date.now())?.chat ?? null
      : null;
  const cwd = chat?.cwd ?? null;
  const chatDeviceId = chat?.deviceId ?? null;
  const targetDeviceId =
    chatDeviceId !== null && deviceId !== null && chatDeviceId !== deviceId ? chatDeviceId : null;

  useEffect(() => {
    store?.ensureLoaded({ cwd, targetDeviceId });
  }, [store, cwd, targetDeviceId]);

  useEffect(() => {
    store?.setAuthorDisplay(settings.gitHistoryAuthorDisplay);
  }, [store, settings.gitHistoryAuthorDisplay]);

  // ── The visible column layout (persisted through the settings store) ───
  const columns = useMemo(
    () => visibleHistoryColumns(settings.gitHistoryColumnOrder, settings.gitHistoryColumns),
    [settings.gitHistoryColumnOrder, settings.gitHistoryColumns],
  );
  const widths = settings.gitHistoryColumnWidths;
  const optionalColumnsWidth = useMemo(
    () => columns.reduce((total, column) => total + historyColumnWidth(column, widths), 0),
    [columns, widths],
  );

  // ── The responsive graph geometry (§2.8's motion family) ───────────────
  // A callback ref, not a mount effect: the list only mounts once the
  // loading state gives way, so an effect with [] deps would observe null
  // forever (the desktop's container_query attaches with the element).
  const listRef = useRef<HTMLDivElement | null>(null);
  const listObserverRef = useRef<ResizeObserver | null>(null);
  const [containerWidth, setContainerWidth] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);

  const attachListRef = useCallback((el: HTMLDivElement | null): void => {
    listObserverRef.current?.disconnect();
    listObserverRef.current = null;
    listRef.current = el;
    if (el === null) {
      return;
    }
    const measure = (): void => {
      setContainerWidth((current) => (current === el.clientWidth ? current : el.clientWidth));
      setViewportHeight((current) => (current === el.clientHeight ? current : el.clientHeight));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    listObserverRef.current = observer;
  }, []);

  useEffect(
    () => () => {
      listObserverRef.current?.disconnect();
      listObserverRef.current = null;
    },
    [],
  );

  const [geometry, setGeometry] = useState<GraphGeometry>(() => naturalGeometry(1));
  const [morph, setMorph] = useState<{ from: GraphGeometry; to: GraphGeometry; startedAt: number } | null>(null);
  const geometryRef = useRef(geometry);
  const settledRef = useRef<GraphGeometry>(geometry);
  const laneCapacity = snapshot.graphLaneCapacity;

  useEffect(() => {
    if (containerWidth === 0) {
      return;
    }
    const target = responsiveGraphGeometry(laneCapacity, containerWidth, optionalColumnsWidth);
    const compact = shouldUseCompactGraph(target, settledRef.current, containerWidth, optionalColumnsWidth);
    const stable = stabilizedGraphGeometry(target, settledRef.current, compact);
    const settled = settledRef.current;
    const changed =
      stable.width !== settled.width ||
      stable.laneCount !== settled.laneCount ||
      stable.compact !== settled.compact;
    if (!changed) {
      return;
    }
    if (compact !== settled.compact) {
      // Mode flip: 180ms COLLAPSE morph from the RENDERED geometry — the
      // lanes converge before the rail takes over.
      setMorph({ from: geometryRef.current, to: stable, startedAt: performance.now() });
      settledRef.current = stable;
    } else {
      // Plain width change: snap to the stabilized value.
      geometryRef.current = stable;
      setGeometry(stable);
      settledRef.current = stable;
    }
  }, [containerWidth, laneCapacity, optionalColumnsWidth]);

  useEffect(() => {
    if (morph === null) {
      return;
    }
    const apply = (value: GraphGeometry): void => {
      geometryRef.current = value;
      setGeometry(value);
    };
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      apply(morph.to);
      setMorph(null);
      return;
    }
    let raf = 0;
    const tick = (now: number): void => {
      const progress = (now - morph.startedAt) / COLLAPSE_MS;
      if (progress >= 1) {
        apply(morph.to);
        setMorph(null);
        return;
      }
      apply(interpolateGraphGeometry(morph.from, morph.to, progress));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
    };
  }, [morph]);

  // ── The virtualized row list + scroll anchoring ────────────────────────
  const [scrollTop, setScrollTop] = useState(0);
  const scrollStateRef = useRef({ scrollTop: 0, list: [] as readonly GitHistoryCommit[] });
  const listEpochRef = useRef(0);
  const searchActiveRef = useRef(false);
  const preSearchTopRef = useRef(0);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (el === null) {
      return;
    }
    const { scrollTop: was, list: oldList } = scrollStateRef.current;
    if (snapshot.listEpoch !== listEpochRef.current) {
      const index = Math.floor(was / HISTORY_ROW_HEIGHT);
      const anchor = {
        sha: oldList[index]?.sha ?? null,
        offsetInItem: was - index * HISTORY_ROW_HEIGHT,
      };
      const resolved =
        anchor.sha !== null
          ? resolveHistoryScrollAnchor({ sha: anchor.sha, offsetInItem: anchor.offsetInItem }, oldList, snapshot.visibleCommits)
          : null;
      if (resolved !== null) {
        const targetIndex = snapshot.visibleCommits.findIndex((commit) => commit.sha === resolved.sha);
        if (targetIndex >= 0) {
          el.scrollTop = targetIndex * HISTORY_ROW_HEIGHT + resolved.offsetInItem;
        }
      }
      listEpochRef.current = snapshot.listEpoch;
    }
    scrollStateRef.current = { scrollTop: el.scrollTop, list: snapshot.visibleCommits };
  });

  // A fresh search starts at the top; clearing it restores where you were.
  useLayoutEffect(() => {
    const el = listRef.current;
    if (el === null || searchActiveRef.current === snapshot.searchActive) {
      return;
    }
    if (snapshot.searchActive) {
      preSearchTopRef.current = el.scrollTop;
      el.scrollTop = 0;
    } else {
      el.scrollTop = preSearchTopRef.current;
    }
    scrollStateRef.current.scrollTop = el.scrollTop;
    searchActiveRef.current = snapshot.searchActive;
  }, [snapshot.searchActive]);

  const rows = snapshot.visibleCommits;
  const totalHeight = rows.length * HISTORY_ROW_HEIGHT + (snapshot.hasLoadMore ? FOOTER_HEIGHT : 0);
  const firstRow = Math.max(0, Math.floor(scrollTop / HISTORY_ROW_HEIGHT) - OVERSCAN);
  const lastRow = Math.min(
    rows.length,
    Math.ceil((scrollTop + Math.max(viewportHeight, 120)) / HISTORY_ROW_HEIGHT) + OVERSCAN,
  );
  const windowed = rows.slice(firstRow, lastRow);

  // ── Hover lane focus (the 150ms crossfade lives in the CSS) ────────────
  const [hoveredColorId, setHoveredColorId] = useState<number | null>(null);
  const onGraphPointer = useCallback(
    (row: Parameters<typeof hoveredGraphPath>[0], x: number, y: number) => {
      setHoveredColorId(hoveredGraphPath(row, x, y, geometryRef.current));
    },
    [],
  );

  const palette = useGraphPalette();
  const railMode = snapshot.viewMode === "allCommits";

  // ── The commit-open path: a NEW pinned commit-diff tab per click ────────
  const openCommit = useCallback(
    (commit: GitHistoryCommit) => {
      rightPaneStore.addCommitDiffSurface(chatId, { sha: commit.sha, subject: commit.subject });
    },
    [chatId],
  );
  const onCopySha = useCallback(
    (sha: string) => {
      store?.copySha(sha);
    },
    [store],
  );
  const onToggleFold = useCallback(
    (refKey: string) => {
      store?.toggleBranchRef(refKey);
    },
    [store],
  );

  // ── The column header row: drag-reorder + divider resize ───────────────
  const columnsGroupRef = useRef<HTMLDivElement | null>(null);
  const [columnDrag, setColumnDrag] = useState<{
    from: number;
    over: number;
    label: string;
    pointerX: number;
    pointerY: number;
  } | null>(null);
  const columnDragRef = useRef(columnDrag);
  columnDragRef.current = columnDrag;

  const resizeRef = useRef<{
    left: HistoryDataColumn;
    right: HistoryDataColumn;
    startX: number;
    leftWidth: number;
    rightWidth: number;
  } | null>(null);

  const persistWidths = useCallback((next: { author: number; date: number; sha: number }) => {
    // Drag samples coalesce into one write (SavePolicy::Debounced).
    persistColumnWidths(next);
  }, []);

  const overFor = (clientX: number): { over: number; relativeX: number } => {
    const group = columnsGroupRef.current;
    if (group === null) {
      return { over: 0, relativeX: 0 };
    }
    const rect = group.getBoundingClientRect();
    const relativeX = clientX - rect.left;
    return {
      over: historyColumnDropIndex(relativeX, rect.width, columns, widths),
      relativeX,
    };
  };

  const startColumnDrag = (event: ReactPointerEvent, from: number, label: string) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    const startX = event.clientX;
    const startY = event.clientY;
    let moved = false;
    const onMove = (move: PointerEvent): void => {
      if (!moved) {
        if (Math.abs(move.clientX - startX) <= DRAG_ARM_PX && Math.abs(move.clientY - startY) <= DRAG_ARM_PX) {
          return;
        }
        moved = true;
      }
      const { over } = overFor(move.clientX);
      const current = columnDragRef.current;
      const next =
        current === null
          ? { from, over, label, pointerX: move.clientX, pointerY: move.clientY }
          : current.over === over
            ? { ...current, pointerX: move.clientX, pointerY: move.clientY }
            : { ...current, over, pointerX: move.clientX, pointerY: move.clientY };
      setColumnDrag(next);
    };
    const finish = (up: PointerEvent): void => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      const current = columnDragRef.current;
      setColumnDrag(null);
      if (current === null || !moved) {
        return;
      }
      const { over } = overFor(up.clientX);
      const dragged = columns[current.from];
      const target = columns[over] ?? dragged;
      if (dragged === undefined || target === undefined) {
        return;
      }
      const reordered = reorderedHistoryColumns(settings.gitHistoryColumnOrder, dragged, target);
      if (reordered.join(",") !== settings.gitHistoryColumnOrder.join(",")) {
        uiSettings.update({ gitHistoryColumnOrder: reordered }, "immediate");
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  };

  const startColumnResize = (
    event: ReactPointerEvent,
    left: HistoryDataColumn,
    right: HistoryDataColumn,
  ) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    resizeRef.current = {
      left,
      right,
      startX: event.clientX,
      leftWidth: historyColumnWidth(left, widths),
      rightWidth: historyColumnWidth(right, widths),
    };
    const onMove = (move: PointerEvent): void => {
      const anchor = resizeRef.current;
      if (anchor === null) {
        return;
      }
      const delta = move.clientX - anchor.startX;
      persistWidths(resizedHistoryColumnWidths(widths, anchor, delta));
    };
    const finish = (): void => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      resizeRef.current = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  };

  // ── The menus (opened at the click point) ─────────────────────────────
  const [menu, setMenu] = useState<{ kind: "columns" | "author" } & HistoryMenuPoint | null>(null);

  const toggleColumn = (column: HistoryColumn): void => {
    const next = { ...settings.gitHistoryColumns, [column]: !settings.gitHistoryColumns[column] };
    uiSettings.update({ gitHistoryColumns: next }, "immediate");
  };

  const resetColumns = (): void => {
    uiSettings.update(
      {
        gitHistoryColumns: { author: true, date: true, sha: true },
        gitHistoryColumnWidths: { ...DEFAULT_HISTORY_COLUMN_WIDTHS },
        gitHistoryColumnOrder: ["author", "date", "sha"],
      },
      "immediate",
    );
  };

  const toggleAuthorDisplay = (): void => {
    uiSettings.update(
      { gitHistoryAuthorDisplay: settings.gitHistoryAuthorDisplay === "avatar" ? "name" : "avatar" },
      "immediate",
    );
  };

  // ── The body swap (§2.13) ──────────────────────────────────────────────
  let body: ReactNode;
  if (cwd === null) {
    body = <EmptyState text="No repository selected" warn={false} />;
  } else if (snapshot.loading && rows.length === 0) {
    body = (
      <div className="history-loading" role="status">
        <MatrixSpinner size={15} />
        <span>Loading history…</span>
      </div>
    );
  } else if (rows.length === 0) {
    const message =
      snapshot.searchActive
        ? snapshot.searchError ?? snapshot.error
        : snapshot.error;
    const text =
      message ??
      (snapshot.searchActive
        ? "No matching commits"
        : snapshot.viewMode === "branchTips"
          ? "No branch tips found"
          : "No commits found");
    body = <EmptyState text={text} warn={message !== null && message !== undefined} />;
  } else {
    const transitions = snapshot.rowTransitions;
    body = (
      <>
        <ColumnHeaderRow
          geometry={geometry}
          columns={columns}
          widths={widths}
          columnsGroupRef={columnsGroupRef}
          drag={columnDrag}
          onStartDrag={startColumnDrag}
          onStartResize={startColumnResize}
          onResetWidths={() => persistWidths({ ...DEFAULT_HISTORY_COLUMN_WIDTHS })}
          onOpenColumnMenu={(point) => setMenu({ kind: "columns", ...point })}
          onOpenAuthorMenu={(point) => setMenu({ kind: "author", ...point })}
          menuOpen={menu?.kind === "columns"}
        />
        <div
          ref={attachListRef}
          className="history-list"
          onScroll={(event) => {
            const el = event.currentTarget;
            scrollStateRef.current.scrollTop = el.scrollTop;
            setScrollTop(el.scrollTop);
          }}
        >
          <div className="history-list-content" style={{ height: totalHeight }}>
            <HistoryGraph
              rows={snapshot.graph.rows}
              geometry={geometry}
              hoveredColorId={hoveredColorId}
              railMode={railMode}
              palette={palette}
              contentHeight={totalHeight}
            />
            {windowed.map((commit, index) => {
              const rowIndex = firstRow + index;
              const graphRow = snapshot.graph.rows[rowIndex];
              if (graphRow === undefined) {
                return null;
              }
              return (
                <div
                  key={commit.sha}
                  className="history-row-slot"
                  style={{ top: rowIndex * HISTORY_ROW_HEIGHT }}
                >
                  <HistoryRow
                    commit={commit}
                    graphRow={graphRow}
                    geometry={geometry}
                    palette={palette}
                    hoveredColorId={hoveredColorId}
                    transition={transitions !== null ? transitions[rowIndex] ?? null : null}
                    columns={columns}
                    widths={widths}
                    authorDisplay={settings.gitHistoryAuthorDisplay}
                    avatar={snapshot.avatars.get(commit.authorEmail.trim().toLowerCase()) ?? null}
                    copied={snapshot.copiedSha === commit.sha}
                    collapsedBranches={snapshot.collapsedBranches}
                    collapsedCounts={snapshot.collapsedCounts}
                    showFoldControl={railMode}
                    onOpenCommit={openCommit}
                    onCopySha={onCopySha}
                    onHoverLane={setHoveredColorId}
                    onGraphPointer={onGraphPointer}
                    onToggleFold={onToggleFold}
                  />
                </div>
              );
            })}
            {snapshot.hasLoadMore ? (
              <div
                className="history-row-slot history-footer-slot"
                style={{ top: rows.length * HISTORY_ROW_HEIGHT, height: FOOTER_HEIGHT }}
              >
                <LoadMoreFooter snapshot={snapshot} onLoadOlder={() => store?.loadOlder()} />
              </div>
            ) : null}
          </div>
        </div>
      </>
    );
  }

  return (
    <div className="history-pane">
      {snapshot.fetchError !== null ? (
        <div className="history-banner" role="alert">
          {snapshot.fetchError}
        </div>
      ) : null}
      {rows.length > 0 && (snapshot.error !== null || snapshot.searchError !== null) ? (
        <div className="history-banner" role="alert">
          {snapshot.error ?? snapshot.searchError}
        </div>
      ) : null}
      {body}
      {menu !== null && menu.kind === "columns" ? (
        <ColumnMenu
          anchor={menu}
          columns={settings.gitHistoryColumns}
          widths={widths}
          order={settings.gitHistoryColumnOrder}
          onToggleColumn={toggleColumn}
          onReset={resetColumns}
          onClose={() => setMenu(null)}
        />
      ) : null}
      {menu !== null && menu.kind === "author" ? (
        <AuthorMenu
          anchor={menu}
          display={settings.gitHistoryAuthorDisplay}
          onToggle={toggleAuthorDisplay}
          onClose={() => setMenu(null)}
        />
      ) : null}
      {columnDrag !== null &&
        createPortal(
          <div className="history-column-ghost" style={{ left: columnDrag.pointerX, top: columnDrag.pointerY }}>
            {columnDrag.label}
          </div>,
          document.body,
        )}
    </div>
  );
}

/** The settings-store write the resize drag coalesces into. */
let pendingWidthWrite: { timer: number } | null = null;

/**
 * Width drags persist debounced (`schedule_column_layout_save`'s
 * SavePolicy::Debounced): samples coalesce into one write after the drag
 * goes quiet.
 */
function persistColumnWidths(next: { author: number; date: number; sha: number }): void {
  if (pendingWidthWrite !== null) {
    window.clearTimeout(pendingWidthWrite.timer);
  }
  const timer = window.setTimeout(() => {
    pendingWidthWrite = null;
    uiSettings.update({ gitHistoryColumnWidths: next }, "immediate");
  }, 400);
  pendingWidthWrite = { timer };
}

function EmptyState({ text, warn }: { text: string; warn: boolean }) {
  return (
    <div className={`history-empty ${warn ? "history-empty-warn" : ""}`} role="status">
      {text}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The column header row (§2.7, history.rs:4436-4495)
// ---------------------------------------------------------------------------

interface ColumnHeaderRowProps {
  readonly geometry: GraphGeometry;
  readonly columns: readonly HistoryColumn[];
  readonly widths: { author: number; date: number; sha: number };
  readonly columnsGroupRef: React.RefObject<HTMLDivElement | null>;
  readonly drag: { from: number; over: number; label: string; pointerX: number; pointerY: number } | null;
  readonly onStartDrag: (event: ReactPointerEvent, from: number, label: string) => void;
  readonly onStartResize: (event: ReactPointerEvent, left: HistoryDataColumn, right: HistoryDataColumn) => void;
  readonly onResetWidths: () => void;
  readonly onOpenColumnMenu: (point: HistoryMenuPoint) => void;
  readonly onOpenAuthorMenu: (point: HistoryMenuPoint) => void;
  /** The columns-button stays lit while its menu is open (`group_hover` +). */
  readonly menuOpen: boolean;
}

function ColumnHeaderRow(props: ColumnHeaderRowProps) {
  const {
    geometry,
    columns,
    widths,
    columnsGroupRef,
    drag,
    onStartDrag,
    onStartResize,
    onResetWidths,
    onOpenColumnMenu,
    onOpenAuthorMenu,
    menuOpen,
  } = props;
  return (
    <div className={`history-columns ${menuOpen ? "history-columns-menu-open" : ""}`} role="row">
      <span className="history-columns-spacer" style={{ width: geometry.width }} aria-hidden />
      <span className="history-columns-commit">Commit</span>
      <div className="history-optional-columns" ref={columnsGroupRef}>
        {columns.map((column, index) => {
          const previous: HistoryDataColumn = index === 0 ? "commit" : columns[index - 1]!;
          const label = column === "author" ? "Author" : column === "date" ? "Date" : "SHA";
          const isDragTarget = drag !== null && drag.over === index && drag.from !== index;
          const placeAfter = drag !== null && drag.from < drag.over;
          return (
            <span
              key={column}
              className={`history-column-header ${column === "author" ? "history-column-header-author" : ""}`}
              style={{ width: historyColumnWidth(column, widths) }}
              onPointerDown={(event) => onStartDrag(event, index, label)}
              onContextMenu={(event) => {
                if (column === "author") {
                  event.preventDefault();
                  onOpenAuthorMenu({ x: event.clientX, y: event.clientY });
                }
              }}
            >
              <span className="history-column-header-label">{label}</span>
              {isDragTarget ? (
                <span className={`history-column-drop ${placeAfter ? "history-column-drop-after" : ""}`} aria-hidden />
              ) : null}
              <span
                className="history-column-resize"
                role="separator"
                aria-label={`Resize ${label} column`}
                onPointerDown={(event) => {
                  event.stopPropagation();
                  onStartResize(event, previous, column);
                }}
                onDoubleClick={(event) => {
                  event.stopPropagation();
                  onResetWidths();
                }}
              />
            </span>
          );
        })}
      </div>
      <button
        type="button"
        id="history-columns-button"
        className="history-columns-button"
        aria-label="Show column menu"
        aria-haspopup="menu"
        onClick={(event) => {
          const rect = (event.currentTarget as HTMLButtonElement).getBoundingClientRect();
          onOpenColumnMenu({ x: rect.left, y: rect.bottom });
        }}
      >
        <Icon name="checklist" size={12} />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The Load-more / loading / retry footer (§2.12)
// ---------------------------------------------------------------------------

function LoadMoreFooter({ snapshot, onLoadOlder }: { snapshot: HistorySnapshot; onLoadOlder: () => void }) {
  const pending = snapshot.searchActive ? snapshot.searchLoading : snapshot.loading;
  const hasError = snapshot.searchActive ? snapshot.searchError !== null : snapshot.error !== null;
  const label = pending ? "Loading…" : hasError ? "Retry" : "Load more";
  const icon = hasError ? "refresh" : "altArrowDown";
  return (
    <button
      type="button"
      id="history-load-older"
      className={`history-footer ${pending ? "history-footer-pending" : ""}`}
      disabled={pending}
      onClick={() => {
        if (!pending) {
          onLoadOlder();
        }
      }}
    >
      {!pending ? <Icon name={icon} size={11} className="history-footer-icon" /> : null}
      <span>{label}</span>
    </button>
  );
}
