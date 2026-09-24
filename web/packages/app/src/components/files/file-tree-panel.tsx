import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type DragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { Icon } from "@zeron/icons";
import type { Appearance } from "@zeron/theme";
import type { WorkspaceFileSearchMatch } from "@zeron/proto";
import { buildSearchTree, isSearchNodeExpanded, toggleSearchNode, type SearchTree, type SearchTreeRow } from "../../lib/file-search-tree";
import { resolveDirectoryIcon, resolveFileIcon } from "../../lib/file-icons";
import type { FileTreeModel, FileTreeSnapshot, TreeRow } from "../../lib/file-tree";
import type { WorkspaceFilesClient } from "../../lib/files-client";
import { describeFilesError } from "../../lib/files-client";
import { useResolvedAppearance } from "../../state/appearance";
import { uiSettings, useUiSettings } from "../../state/ui-settings";
import { Tooltip, TOOLTIP_VIEW_OPTIONS_MS } from "../ui/Tooltip";
import { FileIcon } from "./file-icon";

/** `search.rs:317-318` — the 200ms debounce after the last keystroke. */
const SEARCH_DEBOUNCE_MS = 200;
/** `search.rs:26` — the result cap the "showing first N" banner reports. */
const SEARCH_RESULT_LIMIT = 200;

/** Desktop `tree.rs:18` — the indent step shared by tree and search rows. */
const TREE_INDENT = 14;

/**
 * The tree pane — the desktop's browser-mode `tree_pane` (mod.rs):
 * `render_header` (the `surface_chrome` toolbar with the search field and
 * the show-all-files toggle), the watch-error banner with its "Refresh
 * now", and below it the search results (query non-empty) or the lazy
 * directory tree — 27px rows, file-type icons, keyboard navigation,
 * drag-out, paged "Load more" rows, error rows with retry.
 */

/**
 * The search keyboard surface — what the header's input talks to while
 * results are mounted (the desktop's `ComposerInput` mention events fanned
 * into `FileSearchState`). Arrows move the active row; Enter activates it.
 */
export interface SearchKeyboard {
  onArrow(delta: number): void;
  onEnter(): void;
}

export function FileTreePanel({
  model,
  client,
  onOpenFile,
  gitStatus,
}: {
  model: FileTreeModel;
  client: WorkspaceFilesClient;
  onOpenFile: (path: string) => void;
  /**
   * The shared remote-safe Git status stream (b25dd404 parity): subscribe
   * through the files client and feed frames to the model. The host owns
   * the engine session wiring; omitting it leaves the tree uncolored.
   */
  gitStatus?: (handlers: {
    onItem: (frame: { status: { files: { path: string; index: string; worktree: string }[] } | null }) => void;
    onEnd?: (error?: unknown) => void;
  }) => { cancel(): void };
}) {
  const subscribe = useCallback((listener: () => void) => model.subscribe(listener), [model]);
  const getSnapshot = useCallback(() => model.getSnapshot(), [model]);
  const snapshot = useSyncExternalStore(subscribe, getSnapshot);
  const appearance = useResolvedAppearance();
  const settings = useUiSettings();
  const [query, setQuery] = useState("");
  const searchKeyboard = useRef<SearchKeyboard | null>(null);

  // The show-all-files preference is stored, not local (the desktop's
  // `ShowAllFilesChanged` → settings → `set_show_all_files` fan-out): the
  // toggle writes the store, and every mounted Files surface re-applies it
  // here. Applying also clears the search box (mod.rs apply_show_all_files).
  useEffect(() => {
    if (model.includeIgnored() !== settings.filesShowAll) {
      model.setIncludeIgnored(settings.filesShowAll);
      setQuery("");
    }
  }, [model, settings.filesShowAll]);

  // The Git status stream (b25dd404): frames decorate the tree rows; an
  // unavailable status clears the decorations (never reads as clean).
  useEffect(() => {
    if (gitStatus === undefined) {
      return;
    }
    const handle = gitStatus({
      onItem: (frame) => model.applyGitStatus(frame.status === null ? null : frame.status.files),
    });
    return () => {
      handle.cancel();
      model.applyGitStatus(null);
    };
  }, [gitStatus, model]);

  const includeIgnored = snapshot.includeIgnored;
  const trimmed = query.trim();

  /** The input's search keys (the ComposerInput's mention/submit events). */
  const onSearchKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setQuery("");
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      searchKeyboard.current?.onArrow(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      searchKeyboard.current?.onEnter();
    }
  };

  return (
    <div className="files-tree-panel">
      <div className="surface-toolbar files-header" role="toolbar" aria-label="Files">
        <div className="surface-input files-search">
          <Icon name="magnifer" size={12} className="files-search-icon" />
          <input
            type="text"
            placeholder="Search files"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onSearchKeyDown}
            autoComplete="off"
            spellCheck={false}
            aria-label="Search files"
          />
        </div>
        <Tooltip
          label={includeIgnored ? "Hide hidden and ignored files" : "Show all files (even hidden)"}
          delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <button
              type="button"
              className={`files-toggle-ignored${includeIgnored ? " files-toggle-ignored-on" : ""}`}
              aria-pressed={includeIgnored}
              aria-label={includeIgnored ? "Hide hidden and ignored files" : "Show all files (even hidden)"}
              onClick={() => uiSettings.updateImmediate({ filesShowAll: !includeIgnored })}
            >
              <Icon name={includeIgnored ? "eye" : "eyeClosed"} size={14} />
            </button>
          }
        />
      </div>
      {snapshot.watchError !== null && (
        <div className="files-watch-note" role="status">
          <Icon name="refresh" size={11} className="files-watch-icon" />
          <span className="files-watch-message">{snapshot.watchError}</span>
          <button type="button" className="files-watch-refresh" onClick={() => model.refresh()}>
            Refresh now
          </button>
        </div>
      )}
      {trimmed.length > 0 ? (
        <SearchResults
          model={model}
          client={client}
          query={trimmed}
          includeIgnored={includeIgnored}
          appearance={appearance}
          onOpenFile={onOpenFile}
          onDismiss={() => setQuery("")}
          keyboardRef={searchKeyboard}
        />
      ) : snapshot.rootError !== null && !snapshot.rootLoaded ? (
        <div className="files-root-error">
          <p className="files-root-error-message">{snapshot.rootError}</p>
          <button type="button" className="files-root-retry" onClick={() => model.retryRoot()}>
            Retry
          </button>
        </div>
      ) : !snapshot.rootLoaded ? (
        <div className="files-placeholder" />
      ) : (
        <TreeList model={model} snapshot={snapshot} appearance={appearance} onOpenFile={onOpenFile} />
      )}
    </div>
  );
}

// ── The tree ───────────────────────────────────────────────────────────────

function TreeList({
  model,
  snapshot,
  appearance,
  onOpenFile,
}: {
  model: FileTreeModel;
  snapshot: FileTreeSnapshot;
  appearance: Appearance;
  onOpenFile: (path: string) => void;
}) {
  const listRef = useRef<HTMLUListElement | null>(null);

  // `reveal_tree_selection` — keep the selected row in view, both for the
  // keyboard walk and for the search reveal that lands on it.
  const selected = snapshot.selected;
  const rows = snapshot.rows;
  useEffect(() => {
    if (selected === null) {
      return;
    }
    const index = rows.findIndex((row) => row.path === selected);
    if (index >= 0) {
      listRef.current
        ?.querySelector<HTMLElement>(`[data-row-index="${index}"]`)
        ?.scrollIntoView({ block: "nearest" });
    }
  }, [selected, rows]);

  /** `on_tree_key_down` (tree.rs:300-357). */
  const onKeyDown = (event: ReactKeyboardEvent<HTMLUListElement>): void => {
    let handled = false;
    switch (event.key) {
      case "ArrowUp":
        model.selectPrevious();
        handled = true;
        break;
      case "ArrowDown":
        model.selectNext();
        handled = true;
        break;
      case "ArrowLeft": {
        const path = model.selected();
        if (path !== null) {
          if (model.isExpanded(path)) {
            model.toggleExpanded(path);
          } else {
            model.selectParent();
          }
        }
        handled = true;
        break;
      }
      case "ArrowRight": {
        const path = model.selected();
        const entry = path !== null ? model.entry(path) : undefined;
        if (path !== null && entry !== undefined && entry.kind === "directory") {
          if (model.isExpanded(path)) {
            model.selectFirstChild();
          } else {
            model.toggleExpanded(path);
          }
        }
        handled = true;
        break;
      }
      case "Enter":
      case " ": {
        const path = model.selected();
        if (path !== null) {
          activateTreePath(model, rows, path, onOpenFile);
        }
        handled = true;
        break;
      }
      default:
        break;
    }
    if (handled) {
      event.preventDefault();
      event.stopPropagation();
    }
  };

  return (
    <ul ref={listRef} className="files-tree" role="tree" aria-label="Files" tabIndex={0} onKeyDown={onKeyDown}>
      {rows.map((row, index) => (
        <TreeRowView
          key={row.path}
          row={row}
          index={index}
          model={model}
          snapshot={snapshot}
          appearance={appearance}
          onOpenFile={onOpenFile}
        />
      ))}
    </ul>
  );
}

/**
 * `activate_tree_path` (tree.rs:268): select, then toggle a directory or
 * open a file; the LoadMore row's activation is its own load.
 */
function activateTreePath(
  model: FileTreeModel,
  rows: readonly TreeRow[],
  path: string,
  onOpenFile: (path: string) => void,
): void {
  const row = rows.find((candidate) => candidate.path === path);
  if (row === undefined) {
    return;
  }
  model.select(path);
  if (row.kind === "loadMore") {
    model.loadMore(row.directory);
  } else if (row.kind === "entry") {
    if (row.entry.kind === "directory") {
      model.toggleExpanded(path);
    } else {
      onOpenFile(path);
    }
  }
}

function TreeRowView({
  row,
  index,
  model,
  snapshot,
  appearance,
  onOpenFile,
}: {
  row: TreeRow;
  index: number;
  model: FileTreeModel;
  snapshot: FileTreeSnapshot;
  appearance: Appearance;
  onOpenFile: (path: string) => void;
}) {
  const selected = snapshot.selected === row.path;
  switch (row.kind) {
    case "entry": {
      const entry = row.entry;
      const isDirectory = entry.kind === "directory";
      const git = snapshot.gitStatus.get(row.path);
      const classes = [
        "files-row",
        selected ? "files-row-active" : "",
        entry.ignored ? "files-row-ignored" : "",
      ]
        .filter((name) => name.length > 0)
        .join(" ");
      return (
        <li role="treeitem" aria-expanded={isDirectory ? row.expanded : undefined} aria-selected={selected}>
          <button
            type="button"
            className={classes}
            style={{ paddingLeft: `${8 + row.depth * TREE_INDENT}px` }}
            data-row-index={index}
            data-git={git ?? undefined}
            draggable
            onDragStart={(event) => beginRowDrag(event, row.path, isDirectory, appearance)}
            onClick={() => activateTreePath(model, snapshot.rows, row.path, onOpenFile)}
          >
            {/* Theme-aware indentation guides (c4d63fa8, tree.rs). */}
            {row.depth > 0 && (
              <span className="files-row-guides" aria-hidden>
                {Array.from({ length: row.depth }, (_, level) => (
                  <span key={level} className="files-row-guide" />
                ))}
              </span>
            )}
            <span className="files-chevron" aria-hidden>
              {isDirectory ? <Icon name={row.expanded ? "altArrowDown" : "altArrowRight"} size={11} /> : null}
            </span>
            <FileIcon className="files-row-icon" kind={entry.kind} name={entry.name} expanded={row.expanded} appearance={appearance} />
            <span className="files-row-name">{entry.name}</span>
          </button>
        </li>
      );
    }
    case "loading":
      return <StatusRow index={index} depth={row.depth} label="Loading…" noteClass="files-row-note" />;
    case "empty":
      return <StatusRow index={index} depth={row.depth} label="Empty folder" noteClass="files-row-note files-row-empty" />;
    case "loadMore":
      return (
        <li>
          <button
            type="button"
            className="files-row files-row-note files-row-action"
            style={{ paddingLeft: `${8 + (row.depth + 1) * TREE_INDENT}px` }}
            data-row-index={index}
            onClick={() => model.loadMore(row.directory)}
          >
            <span className="files-row-name">Load more…</span>
          </button>
        </li>
      );
    case "error":
      return (
        <li>
          <button
            type="button"
            className="files-row files-row-error"
            style={{ paddingLeft: `${8 + (row.depth + 1) * TREE_INDENT}px` }}
            data-row-index={index}
            onClick={() => model.retryDirectory(row.directory)}
          >
            <span className="files-row-name">{row.message} — Retry</span>
          </button>
        </li>
      );
  }
}

/** `status_row` (tree.rs:374-395): 10.5px faint status text at depth+1. */
function StatusRow({
  index,
  depth,
  label,
  noteClass,
}: {
  index: number;
  depth: number;
  label: string;
  noteClass: string;
}) {
  return (
    <li
      className={`files-row files-row-status ${noteClass}`}
      style={{ paddingLeft: `${8 + (depth + 1) * TREE_INDENT}px` }}
      data-row-index={index}
    >
      <span className="files-row-name">{label}</span>
    </li>
  );
}

// ── Drag-out (WorkspacePathDrag + its ghost) ───────────────────────────────

/**
 * `WorkspacePathDrag::new` + `workspace_path_drag_ghost` (mod.rs:67-137):
 * the payload is the workspace-relative path and an isDirectory flag; the
 * ghost is the compact pill (24px, ≤220px, raised surface, strong border,
 * 11.5px, opacity 0.85) the composer's drop target turns into a file
 * mention. `setDragImage` snapshots the detached node, which is removed
 * after the drag starts.
 */
function beginRowDrag(
  event: DragEvent<HTMLElement>,
  path: string,
  isDirectory: boolean,
  appearance: Appearance,
): void {
  event.dataTransfer.setData("application/x-zeron-workspace-path", JSON.stringify({ path, isDirectory }));
  event.dataTransfer.setData("text/plain", path);
  event.dataTransfer.effectAllowed = "copyLink";

  const title = path.trimEnd().split("/").pop() ?? path;
  const ghost = document.createElement("div");
  ghost.className = "files-drag-ghost";
  const icon = document.createElement("img");
  icon.src = isDirectory ? resolveDirectoryIcon(title, appearance) : resolveFileIcon(title, appearance);
  icon.width = 14;
  icon.height = 14;
  icon.alt = "";
  icon.draggable = false;
  const label = document.createElement("span");
  label.textContent = title;
  ghost.append(icon, label);
  document.body.append(ghost);
  event.dataTransfer.setDragImage(ghost, 10, 12);
  window.setTimeout(() => ghost.remove(), 0);
}

// ── Search ─────────────────────────────────────────────────────────────────

type SearchState =
  | { readonly kind: "searching" }
  | { readonly kind: "loaded"; readonly matches: readonly WorkspaceFileSearchMatch[] }
  | { readonly kind: "error"; readonly message: string };

function SearchResults({
  model,
  client,
  query,
  includeIgnored,
  appearance,
  onOpenFile,
  onDismiss,
  keyboardRef,
}: {
  model: FileTreeModel;
  client: WorkspaceFilesClient;
  query: string;
  includeIgnored: boolean;
  appearance: Appearance;
  onOpenFile: (path: string) => void;
  onDismiss: () => void;
  keyboardRef: RefObject<SearchKeyboard | null>;
}) {
  const [state, setState] = useState<SearchState>({ kind: "searching" });
  const [tree, setTree] = useState<SearchTree | null>(null);
  const [active, setActive] = useState(0);
  const requestRef = useRef(0);
  const listRef = useRef<HTMLUListElement | null>(null);

  // 200ms debounce → `SearchWorkspaceFiles` (on_search_edited, search.rs:270).
  useEffect(() => {
    const request = ++requestRef.current;
    setState({ kind: "searching" });
    setTree(null);
    setActive(0);
    const timer = setTimeout(() => {
      void client
        .search(query, includeIgnored, SEARCH_RESULT_LIMIT)
        .then((matches) => {
          if (requestRef.current === request) {
            setState({ kind: "loaded", matches });
            setTree(buildSearchTree(matches));
            setActive(0);
          }
        })
        .catch((error: unknown) => {
          if (requestRef.current === request) {
            setState({ kind: "error", message: describeFilesError(error) });
          }
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [client, query, includeIgnored]);

  const rows = tree?.rows ?? [];

  /** `activate_search_result` (search.rs:357-388). */
  const activateRow = useCallback(
    (row: SearchTreeRow): void => {
      if (tree === null) {
        return;
      }
      if (row.kind === "directory" && row.hasChildren) {
        const next = toggleSearchNode(tree, row.path);
        if (next !== null) {
          setTree(next);
          const index = next.rows.findIndex((candidate) => candidate.path === row.path);
          setActive(index >= 0 ? index : 0);
        }
        return;
      }
      void model.revealInTree(row.path).then((error) => {
        if (error !== null) {
          setState({ kind: "error", message: error });
          return;
        }
        onDismiss();
        if (row.kind !== "directory") {
          onOpenFile(row.path);
        }
      });
    },
    [tree, model, onDismiss, onOpenFile],
  );

  // The header input's arrow/enter reach the results through the keyboard
  // ref — the web shape of the ComposerInput's mention events.
  useEffect(() => {
    keyboardRef.current = {
      onArrow: (delta) => {
        if (rows.length === 0) {
          return;
        }
        setActive((current) => Math.max(0, Math.min(rows.length - 1, current + delta)));
      },
      onEnter: () => {
        const row = rows[active];
        if (row !== undefined) {
          activateRow(row);
        }
      },
    };
    return () => {
      keyboardRef.current = null;
    };
  }, [keyboardRef, rows, active, activateRow]);

  // `search_list.scroll_to_reveal_item` — the active row stays in view.
  useEffect(() => {
    if (rows.length === 0) {
      return;
    }
    listRef.current
      ?.querySelector<HTMLElement>(`[data-row-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active, rows]);

  const banner =
    state.kind === "loaded" && state.matches.length >= SEARCH_RESULT_LIMIT ? (
      <div className="files-search-banner">Showing the first {SEARCH_RESULT_LIMIT} matches</div>
    ) : null;

  let body: ReactNode;
  if (state.kind === "error") {
    body = <div className="files-search-message files-search-message-error">{state.message}</div>;
  } else if (rows.length === 0) {
    body = <div className="files-search-message">{state.kind === "searching" ? "Searching…" : "No files found."}</div>;
  } else {
    body = (
      <ul ref={listRef} className="files-tree files-search-results" role="tree" aria-label="Search results">
        {rows.map((row, index) => (
          <SearchRowView
            key={row.path}
            row={row}
            index={index}
            active={index === active}
            appearance={appearance}
            expanded={tree !== null && isSearchNodeExpanded(tree, row.path)}
            onSelect={() => setActive(index)}
            onActivate={() => activateRow(row)}
          />
        ))}
      </ul>
    );
  }

  return (
    <div className="files-search-results-panel">
      {banner}
      {body}
    </div>
  );
}

/** `render_search_row` (search.rs:516-605). */
function SearchRowView({
  row,
  index,
  active,
  appearance,
  expanded,
  onSelect,
  onActivate,
}: {
  row: SearchTreeRow;
  index: number;
  active: boolean;
  appearance: Appearance;
  expanded: boolean;
  onSelect: () => void;
  onActivate: () => void;
}) {
  const isDirectory = row.kind === "directory";
  return (
    <li role="treeitem" aria-expanded={row.hasChildren ? expanded : undefined} aria-selected={active}>
      <button
        type="button"
        className={`files-row files-row-search${active ? " files-row-active" : ""}`}
        style={{ paddingLeft: `${8 + row.depth * TREE_INDENT}px` }}
        data-row-index={index}
        draggable
        onDragStart={(event) => beginRowDrag(event, row.path, isDirectory, appearance)}
        onClick={() => {
          onSelect();
          onActivate();
        }}
      >
        <span className="files-chevron" aria-hidden>
          {row.hasChildren ? <Icon name={expanded ? "altArrowDown" : "altArrowRight"} size={11} /> : null}
        </span>
        <FileIcon className="files-row-icon" kind={row.kind} name={row.name} expanded={expanded} appearance={appearance} />
        <span className={`files-row-name${isDirectory ? " files-row-name-directory" : ""}`}>{row.name}</span>
      </button>
    </li>
  );
}
