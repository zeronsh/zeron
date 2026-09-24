import type { WorkspaceDirectoryPage, WorkspaceEntry, WorkspaceFileChanges } from "@zeron/proto";
import type { WatchHandlers } from "@zeron/engine-client";
import type { WorkspaceFilesClient } from "./files-client";
import { describeFilesError } from "./files-client";
import { compareEntries, isDirectChild, parentPath } from "./files";

/**
 * The directory tree behind the files page — a port of the desktop's
 * `FileTreeModel` (crates/ui/src/files/model.rs) plus the load-driving and
 * watch application the desktop keeps in its surface (mod.rs / watch.rs).
 * The model is observable: every mutation swaps an immutable snapshot and
 * notifies listeners once.
 */

export type DirectoryLoad =
  | { readonly kind: "unloaded" }
  | { readonly kind: "loading"; readonly cursor: string | null }
  | { readonly kind: "loaded"; readonly nextCursor: string | null }
  | { readonly kind: "error"; readonly message: string; readonly cursor: string | null };

interface TreeNode {
  entry: WorkspaceEntry;
  children: string[];
  load: DirectoryLoad;
  stale: boolean;
  hasLoaded: boolean;
}

export type TreeRow =
  | { readonly kind: "entry"; readonly path: string; readonly depth: number; readonly entry: WorkspaceEntry; readonly expanded: boolean }
  | { readonly kind: "loading"; readonly path: string; readonly depth: number; readonly directory: string }
  | { readonly kind: "empty"; readonly path: string; readonly depth: number; readonly directory: string }
  | { readonly kind: "error"; readonly path: string; readonly depth: number; readonly directory: string; readonly message: string }
  | { readonly kind: "loadMore"; readonly path: string; readonly depth: number; readonly directory: string; readonly cursor: string };

export interface FileTreeSnapshot {
  readonly rows: readonly TreeRow[];
  readonly includeIgnored: boolean;
  /** Set while the change stream itself is degraded (desktop: watch_error). */
  readonly watchError: string | null;
  /** The keyboard/mouse selection (desktop model.rs `selected`). */
  readonly selected: string | null;
  /** Desktop `tree_has_content` — the root has produced a page at least once. */
  readonly rootLoaded: boolean;
  /**
   * The surface-level root error (desktop `FilesSurface::error`): set when a
   * root load fails, shown INSTEAD of the tree while the root has never
   * loaded (mod.rs:213-247).
   */
  readonly rootError: string | null;
  /**
   * Git status decorations joined onto the rows (files/git_status.rs):
   * workspace path → classified color kind. Empty while no status has
   * arrived; `null`-free — a partial status simply contributes no rows.
   */
  readonly gitStatus: ReadonlyMap<string, GitDecorationKind>;
}

/**
 * The row-level Git color kinds (`DecorationKind`): staged/unstaged edits
 * collapse to `modified`, adds to `added`, deletions and unmerged entries to
 * `deleted`, untracked to `untracked`, renames to `renamed`.
 */
export type GitDecorationKind = "modified" | "added" | "deleted" | "renamed" | "untracked";

/**
 * `Decorations::from_snapshot` (b25dd404, files/git_status.rs): join a
 * `CheckoutGitStatus` onto workspace paths — direct matches classify by
 * their combined columns, and every ANCESTOR directory of a touched path
 * aggregates its descendants (a directory row shows the union, preferring
 * the stronger signal). Partial/incomplete statuses still classify the
 * paths they carry: incomplete is never reported as clean, it simply
 * decorates less.
 */
export function decorationsFromStatus(
  files: readonly { readonly path: string; readonly index: string; readonly worktree: string }[],
): Map<string, GitDecorationKind> {
  const rank: Record<GitDecorationKind, number> = {
    untracked: 1,
    added: 2,
    renamed: 3,
    modified: 4,
    deleted: 5,
  };
  const out = new Map<string, GitDecorationKind>();
  const stronger = (a: GitDecorationKind, b: GitDecorationKind): GitDecorationKind =>
    rank[a] >= rank[b] ? a : b;
  for (const file of files) {
    const index = stateKind(file.index);
    const worktree = stateKind(file.worktree);
    if (index === null && worktree === null) {
      continue;
    }
    // Untracked dominates; otherwise the worktree column wins the row.
    const kind =
      file.worktree === "untracked" || file.index === "untracked"
        ? "untracked"
        : stronger(worktree ?? index ?? "modified", index ?? worktree ?? "modified");
    out.set(file.path, kind);
    // Ancestor aggregation: every directory on the path inherits the row.
    let slash = file.path.lastIndexOf("/");
    while (slash > 0) {
      const dir = file.path.slice(0, slash);
      out.set(dir, stronger(out.get(dir) ?? kind, kind));
      slash = dir.lastIndexOf("/");
    }
  }
  return out;
}

function stateKind(state: string): GitDecorationKind | null {
  switch (state) {
    case "added":
    case "copied":
      return "added";
    case "modified":
    case "typeChanged":
      return "modified";
    case "deleted":
    case "unmerged":
      return "deleted";
    case "renamed":
      return "renamed";
    case "untracked":
      return "untracked";
    default:
      return null;
  }
}

/** Watch outcomes the open document cares about (watch.rs parity). */
export type FileWatchEvent =
  | { readonly kind: "created" | "modified" | "removed"; readonly path: string }
  | { readonly kind: "renamed"; readonly path: string; readonly oldPath: string }
  | { readonly kind: "resync" };

export interface FileTreeModelOptions {
  readonly client: WorkspaceFilesClient;
  /** Subscribe the workspace change stream; omitted in tests (no watch). */
  readonly watch?: (handlers: WatchHandlers<WorkspaceFileChanges>) => { cancel(): void };
  readonly onFileEvent?: (event: FileWatchEvent) => void;
  readonly includeIgnored?: boolean;
}

const ROOT = "";

export class FileTreeModel {
  readonly #client: WorkspaceFilesClient;
  readonly #watch: FileTreeModelOptions["watch"];
  readonly #onFileEvent: FileTreeModelOptions["onFileEvent"];
  readonly #nodes = new Map<string, TreeNode>();
  readonly #expanded = new Set<string>([ROOT]);
  readonly #listingChildren = new Map<string, Set<string>>();
  readonly #listeners = new Set<() => void>();
  #includeIgnored: boolean;
  #selected: string | null = null;
  #rootError: string | null = null;
  #generation = 0;
  #watchSequence: number | null = null;
  #watchError: string | null = null;
  #watchHandle: { cancel(): void } | null = null;
  #snapshot: FileTreeSnapshot;
  #gitStatus: ReadonlyMap<string, GitDecorationKind> = new Map();
  #disposed = false;

  constructor(options: FileTreeModelOptions) {
    this.#client = options.client;
    this.#watch = options.watch;
    this.#onFileEvent = options.onFileEvent;
    this.#includeIgnored = options.includeIgnored ?? false;
    this.#nodes.set(ROOT, newNode(rootEntry()));
    this.#snapshot = {
      rows: [],
      includeIgnored: this.#includeIgnored,
      watchError: null,
      selected: null,
      rootLoaded: false,
      rootError: null,
      gitStatus: this.#gitStatus,
    };
  }

  /**
   * `apply_git_status` (files/git_status.rs ensure/release): join the frame's
   * status onto the snapshot. An unavailable (null) status clears — never
   * reports clean, the rows simply stop carrying color.
   */
  applyGitStatus(
    status: { readonly path: string; readonly index: string; readonly worktree: string }[] | null,
  ): void {
    this.#gitStatus = status === null ? new Map() : decorationsFromStatus(status);
    this.#commit();
  }

  getSnapshot(): FileTreeSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  includeIgnored(): boolean {
    return this.#includeIgnored;
  }

  isExpanded(path: string): boolean {
    return this.#expanded.has(path);
  }

  entry(path: string): WorkspaceEntry | undefined {
    return this.#nodes.get(path)?.entry;
  }

  /** Begin the root listing and the change stream. */
  start(): void {
    if (this.#disposed) {
      return;
    }
    this.#requestDirectory(ROOT, null);
    if (this.#watch !== undefined && this.#watchHandle === null) {
      this.#watchHandle = this.#watch({
        onItem: (frame) => this.#applyChanges(frame),
        onEnd: (error) => {
          // A ended stream re-subscribes on the next connection; the desktop
          // surfaces the interruption ("File updates interrupted — retrying").
          this.#watchError = error !== undefined ? describeFilesError(error) : null;
          this.#commit();
        },
      });
    }
  }

  /** Stop the watch; in-flight loads settle harmlessly against the guard. */
  dispose(): void {
    this.#disposed = true;
    this.#watchHandle?.cancel();
    this.#watchHandle = null;
    this.#listeners.clear();
  }

  /** Desktop `toggle_expanded`; expanding an unloaded directory loads it. */
  toggleExpanded(path: string): void {
    const node = this.#nodes.get(path);
    if (node === undefined || node.entry.kind !== "directory") {
      return;
    }
    if (this.#expanded.has(path)) {
      this.#expanded.delete(path);
      // A collapsed directory pulls the selection out of its subtree
      // (model.rs:307-313): onto the directory itself, or none at the root.
      if (this.#selected !== null && isDescendant(this.#selected, path)) {
        this.#selected = path === ROOT ? null : path;
      }
    } else {
      this.#expanded.add(path);
      if (node.load.kind === "unloaded" || node.load.kind === "error" || node.stale) {
        this.#requestDirectory(path, null);
      }
    }
    this.#commit();
  }

  /** Desktop "Load more" row. */
  loadMore(directory: string): void {
    const node = this.#nodes.get(directory);
    if (node === undefined || node.load.kind !== "loaded" || node.load.nextCursor === null) {
      return;
    }
    this.#requestDirectory(directory, node.load.nextCursor);
  }

  /** Retry after a failed page (cursor retained on error, like the desktop). */
  retryDirectory(directory: string): void {
    const node = this.#nodes.get(directory);
    if (node === undefined || node.load.kind !== "error") {
      return;
    }
    this.#requestDirectory(directory, node.load.cursor);
  }

  /** Desktop `select` — a path that is not in the model is rejected. */
  select(path: string): boolean {
    if (!this.#nodes.has(path)) {
      return false;
    }
    this.#selected = path;
    this.#commit();
    return true;
  }

  /** The current selection, or null. */
  selected(): string | null {
    return this.#selected;
  }

  /** Desktop `select_next` / `select_previous` (via `move_selection`). */
  selectNext(): string | null {
    return this.#moveSelection(1);
  }

  selectPrevious(): string | null {
    return this.#moveSelection(-1);
  }

  /** Desktop `select_parent`: the parent path, or stay put at the root. */
  selectParent(): string | null {
    if (this.#selected === null) {
      return null;
    }
    const parent = parentPath(this.#selected);
    if (parent === null || parent === ROOT) {
      return this.#selected;
    }
    this.#selected = parent;
    this.#commit();
    return parent;
  }

  /** Desktop `select_first_child`. */
  selectFirstChild(): string | null {
    if (this.#selected === null) {
      return null;
    }
    const first = this.#nodes.get(this.#selected)?.children[0];
    if (first === undefined) {
      return null;
    }
    this.#selected = first;
    this.#commit();
    return first;
  }

  /** Desktop `expand` — force-expand without loading (the reveal path). */
  expand(path: string): boolean {
    const node = this.#nodes.get(path);
    if (node === undefined || node.entry.kind !== "directory") {
      return false;
    }
    const changed = !this.#expanded.has(path);
    if (changed) {
      this.#expanded.add(path);
    }
    return changed;
  }

  /** Desktop `expanded_directories` — for the watch-error "Refresh now". */
  expandedDirectories(): readonly string[] {
    return [...this.#expanded];
  }

  /**
   * Desktop `refresh` (mod.rs:748): invalidate everything and reload the
   * root plus every expanded, loaded directory — the "Refresh now" button's
   * forced resync.
   */
  refresh(): void {
    this.#rootError = null;
    this.#markAllDirectoriesStale();
    this.#reloadExpanded();
    this.#commit();
  }

  /**
   * Desktop `retry_root` — clear the surface error and reload the root,
   * which is what the root-error block's Retry button does.
   */
  retryRoot(): void {
    this.#rootError = null;
    this.#requestDirectory(ROOT, null);
    this.#commit();
  }

  /**
   * Desktop `reveal_search_result` (search.rs:390): expand the match's
   * ancestors — one `ListWorkspaceDirectory` per ancestor, applied page by
   * page so the tree fills in as the reveal walks down — then select the
   * row. Returns the engine's error message on failure, or null on success.
   */
  async revealInTree(path: string): Promise<string | null> {
    const generation = this.#generation;
    const directories: string[] = [ROOT];
    let current = parentPath(path);
    const ancestors: string[] = [];
    while (current !== null && current !== ROOT) {
      ancestors.push(current);
      current = parentPath(current);
    }
    ancestors.reverse();
    directories.push(...ancestors);

    const pages: { directory: string; page: WorkspaceDirectoryPage }[] = [];
    for (const directory of directories) {
      try {
        const page = await this.#client.listDirectory(directory, this.#includeIgnored);
        if (this.#disposed || generation !== this.#generation) {
          return null;
        }
        pages.push({ directory, page });
      } catch (error: unknown) {
        if (this.#disposed || generation !== this.#generation) {
          return null;
        }
        return describeFilesError(error);
      }
    }
    if (this.#disposed || generation !== this.#generation) {
      return null;
    }
    for (const [index, { directory, page }] of pages.entries()) {
      this.#applyPage(directory, page.entries, page.nextCursor ?? null, generation);
      const ancestor = ancestors[index - 1];
      if (ancestor !== undefined) {
        this.#expanded.add(ancestor);
      }
    }
    this.select(path);
    return null;
  }

  /**
   * Desktop `move_selection` (model.rs:419-442): arrow-key navigation only
   * considers selectable rows (`entry`/`loadMore`); wraps to the last item
   * on Up with no prior selection, the first item on Down.
   */
  #moveSelection(delta: number): string | null {
    const selectable = this.#snapshot.rows.filter((row) => isSelectableRow(row)).map((row) => row.path);
    if (selectable.length === 0) {
      this.#selected = null;
      this.#commit();
      return null;
    }
    const current = this.#selected !== null ? selectable.indexOf(this.#selected) : -1;
    let next: number;
    if (current >= 0 && delta < 0) {
      next = Math.max(0, current - 1);
    } else if (current >= 0) {
      next = Math.min(selectable.length - 1, current + delta);
    } else {
      next = delta < 0 ? selectable.length - 1 : 0;
    }
    this.#selected = selectable[next] ?? null;
    this.#commit();
    return this.#selected;
  }

  /** Desktop `set_include_ignored`: flips the flag and re-lists everything. */
  setIncludeIgnored(includeIgnored: boolean): void {
    if (this.#includeIgnored === includeIgnored) {
      return;
    }
    this.#includeIgnored = includeIgnored;
    this.#reset();
    this.#rootError = null;
    this.#requestDirectory(ROOT, null);
    this.#commit();
  }

  // ── Load driving ──────────────────────────────────────────────────────

  #requestDirectory(directory: string, cursor: string | null): void {
    const generation = this.#generation;
    if (!this.#beginLoad(directory, cursor)) {
      return;
    }
    this.#commit();
    void this.#client
      .listDirectory(directory, this.#includeIgnored, cursor ?? undefined)
      .then((page) => {
        this.#applyPage(directory, page.entries, page.nextCursor ?? null, generation);
      })
      .catch((error: unknown) => {
        this.#failLoad(directory, cursor, describeFilesError(error), generation);
      });
  }

  #beginLoad(directory: string, cursor: string | null): boolean {
    const node = this.#nodes.get(directory);
    if (node === undefined || node.load.kind === "loading") {
      return false;
    }
    node.load = { kind: "loading", cursor };
    node.stale = false;
    return true;
  }

  #failLoad(directory: string, cursor: string | null, message: string, generation: number): void {
    if (this.#disposed || generation !== this.#generation) {
      return;
    }
    const node = this.#nodes.get(directory);
    if (node === undefined) {
      return;
    }
    node.load = { kind: "error", message, cursor };
    // mod.rs:871-873 — a root failure also sets the surface-level error the
    // root-error block renders.
    if (directory === ROOT) {
      this.#rootError = message;
    }
    this.#commit();
  }

  /** Desktop `apply_page`: append-or-replace pages, deletion only on the last. */
  #applyPage(directory: string, entries: readonly WorkspaceEntry[], nextCursor: string | null, generation: number): void {
    if (this.#disposed || generation !== this.#generation) {
      return;
    }
    const parent = this.#nodes.get(directory);
    if (parent === undefined || parent.entry.kind !== "directory") {
      return;
    }
    const parentIgnored = parent.entry.ignored;
    const append = parent.load.kind === "loading" && parent.load.cursor !== null;
    if (!append) {
      this.#listingChildren.set(directory, new Set());
    }
    const seen = this.#listingChildren.get(directory)!;
    for (const entry of entries) {
      if (isDirectChild(entry.path, directory)) {
        seen.add(entry.path);
      }
    }
    if (nextCursor === null) {
      const incoming = this.#listingChildren.get(directory) ?? new Set<string>();
      this.#listingChildren.delete(directory);
      for (const child of [...parent.children]) {
        // A partial listing cannot establish that an unseen child was deleted.
        if (!incoming.has(child)) {
          this.#removeSubtree(child);
        }
      }
      parent.children = parent.children.filter((path) => incoming.has(path));
    }

    for (const pageEntry of entries) {
      if (!isDirectChild(pageEntry.path, directory)) {
        continue;
      }
      const entry = { ...pageEntry, ignored: pageEntry.ignored || parentIgnored };
      const existing = this.#nodes.get(entry.path);
      if (existing !== undefined && existing.entry.kind !== entry.kind) {
        this.#removeSubtree(entry.path);
      }
      const node = this.#nodes.get(entry.path);
      if (node !== undefined) {
        node.entry = entry;
      } else {
        this.#nodes.set(entry.path, newNode(entry));
      }
      if (!parent.children.includes(entry.path)) {
        parent.children.push(entry.path);
      }
    }

    parent.children.sort((left, right) => compareEntries(this.#nodes.get(left)!.entry, this.#nodes.get(right)!.entry));
    parent.load = { kind: "loaded", nextCursor };
    parent.stale = false;
    parent.hasLoaded = true;
    if (directory === ROOT) {
      // mod.rs:866 — any successful root page clears the surface error.
      this.#rootError = null;
    }
    this.#commit();
  }

  // ── Watch application (watch.rs parity) ───────────────────────────────

  #applyChanges(frame: WorkspaceFileChanges): void {
    if (this.#disposed || typeof frame?.sequence !== "number" || !Array.isArray(frame.changes)) {
      return;
    }
    const gap = sequenceNeedsResync(this.#watchSequence, frame.sequence);
    this.#watchSequence = frame.sequence;
    this.#watchError = null;
    if (frame.resyncRequired || gap) {
      this.#markAllDirectoriesStale();
      this.#reloadExpanded();
      this.#onFileEvent?.({ kind: "resync" });
      this.#commit();
      return;
    }
    const parents = new Set<string>();
    for (const change of frame.changes) {
      if (typeof change?.path !== "string") {
        continue;
      }
      switch (change.kind) {
        case "created": {
          this.#onFileEvent?.({ kind: "created", path: change.path });
          const parent = parentPath(change.path);
          if (parent !== null) {
            parents.add(parent);
          }
          break;
        }
        case "modified":
          this.#onFileEvent?.({ kind: "modified", path: change.path });
          break;
        case "removed": {
          this.#remove(change.path);
          this.#onFileEvent?.({ kind: "removed", path: change.path });
          const parent = parentPath(change.path);
          if (parent !== null) {
            parents.add(parent);
          }
          break;
        }
        case "renamed": {
          const oldPath = typeof change.oldPath === "string" ? change.oldPath : null;
          if (oldPath !== null) {
            this.#remove(oldPath);
            const oldParent = parentPath(oldPath);
            if (oldParent !== null) {
              parents.add(oldParent);
            }
          }
          this.#onFileEvent?.({ kind: "renamed", path: change.path, oldPath: oldPath ?? change.path });
          const parent = parentPath(change.path);
          if (parent !== null) {
            parents.add(parent);
          }
          break;
        }
      }
    }
    for (const parent of parents) {
      const node = this.#nodes.get(parent);
      if (node !== undefined && node.entry.kind === "directory") {
        node.stale = true;
      }
    }
    for (const parent of parents) {
      // Desktop: only expanded directories reload in the background.
      if (this.#expanded.has(parent) && this.#nodes.get(parent)?.hasLoaded) {
        this.#requestDirectory(parent, null);
      }
    }
    this.#commit();
  }

  /** Desktop `remove`: drop the path and its subtree from the model. */
  #remove(path: string): void {
    if (path === ROOT || !this.#nodes.has(path)) {
      return;
    }
    const parent = parentPath(path);
    if (parent !== null) {
      const node = this.#nodes.get(parent);
      if (node !== undefined) {
        node.children = node.children.filter((child) => child !== path);
      }
    }
    // model.rs:394-400 — a removed row hands its selection to the parent
    // (none at the root), so keyboard nav survives deletions.
    if (this.#selected !== null && (this.#selected === path || isDescendant(this.#selected, path))) {
      this.#selected = parent !== null && parent !== ROOT ? parent : null;
    }
    this.#removeSubtree(path);
  }

  #removeSubtree(path: string): void {
    const node = this.#nodes.get(path);
    if (node !== undefined) {
      for (const child of [...node.children]) {
        this.#removeSubtree(child);
      }
    }
    this.#nodes.delete(path);
    this.#listingChildren.delete(path);
    this.#expanded.delete(path);
  }

  #markAllDirectoriesStale(): void {
    for (const node of this.#nodes.values()) {
      if (node.entry.kind === "directory") {
        node.stale = true;
      }
    }
  }

  #reloadExpanded(): void {
    const targets = [...this.#expanded].filter((path) => this.#nodes.get(path)?.hasLoaded === true);
    targets.sort((a, b) => a.split("/").length - b.split("/").length);
    for (const path of targets) {
      this.#requestDirectory(path, null);
    }
    if (!this.#nodes.get(ROOT)?.hasLoaded) {
      this.#requestDirectory(ROOT, null);
    }
  }

  // ── Snapshot ──────────────────────────────────────────────────────────

  #reset(): void {
    this.#generation += 1;
    this.#nodes.clear();
    this.#listingChildren.clear();
    this.#nodes.set(ROOT, newNode(rootEntry()));
    this.#expanded.clear();
    this.#expanded.add(ROOT);
    this.#watchSequence = null;
    this.#selected = null;
  }

  #commit(): void {
    if (this.#disposed) {
      return;
    }
    const rows = this.#buildRows();
    // model.rs:463-469 — a selection whose row vanished is dropped.
    if (this.#selected !== null && !rows.some((row) => row.path === this.#selected)) {
      this.#selected = null;
    }
    this.#snapshot = {
      rows,
      includeIgnored: this.#includeIgnored,
      watchError: this.#watchError,
      selected: this.#selected,
      rootLoaded: this.#nodes.get(ROOT)?.hasLoaded === true,
      rootError: this.#rootError,
      gitStatus: this.#gitStatus,
    };
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /** Desktop `rebuild_visible_rows` / `append_directory_rows`. */
  #buildRows(): TreeRow[] {
    const rows: TreeRow[] = [];
    const visited = new Set<string>([ROOT]);
    const walk = (directory: string, depth: number): void => {
      const node = this.#nodes.get(directory);
      if (node === undefined) {
        return;
      }
      for (const childPath of node.children) {
        if (visited.has(childPath)) {
          continue;
        }
        visited.add(childPath);
        const child = this.#nodes.get(childPath);
        if (child === undefined) {
          continue;
        }
        rows.push({
          kind: "entry",
          path: childPath,
          depth,
          entry: child.entry,
          expanded: this.#expanded.has(childPath),
        });
        if (child.entry.kind === "directory" && this.#expanded.has(childPath)) {
          walk(childPath, depth + 1);
        }
      }
      if (!this.#expanded.has(directory)) {
        return;
      }
      const load = node.load;
      if (load.kind === "loading" && load.cursor === null && node.hasLoaded) {
        if (node.children.length === 0) {
          rows.push({ kind: "empty", path: syntheticPath(directory, "empty"), depth, directory });
        }
      } else if (load.kind === "loading") {
        rows.push({ kind: "loading", path: syntheticPath(directory, "loading"), depth, directory });
      } else if (load.kind === "loaded") {
        if (node.children.length === 0) {
          rows.push({ kind: "empty", path: syntheticPath(directory, "empty"), depth, directory });
        }
        if (load.nextCursor !== null) {
          rows.push({ kind: "loadMore", path: syntheticPath(directory, "more"), depth, directory, cursor: load.nextCursor });
        }
      } else if (load.kind === "error") {
        rows.push({ kind: "error", path: syntheticPath(directory, "error"), depth, directory, message: load.message });
      }
    };
    walk(ROOT, 0);
    return rows;
  }
}

/** Desktop watch.rs `sequence_needs_resync`. */
export function sequenceNeedsResync(previous: number | null, next: number): boolean {
  return previous !== null && next !== previous + 1;
}

/** Desktop `VisibleTreeRow::selectable`: Entry and LoadMore rows. */
function isSelectableRow(row: TreeRow): boolean {
  return row.kind === "entry" || row.kind === "loadMore";
}

/** Desktop model.rs `is_descendant` (586-603). */
function isDescendant(candidate: string, ancestor: string): boolean {
  if (ancestor === ROOT) {
    return candidate !== ROOT;
  }
  return candidate.startsWith(ancestor) && candidate.charAt(ancestor.length) === "/";
}

function rootEntry(): WorkspaceEntry {
  return { path: "", name: "", kind: "directory", ignored: false, readOnly: false };
}

function newNode(entry: WorkspaceEntry): TreeNode {
  return {
    entry,
    children: [],
    load: entry.kind === "directory" ? { kind: "unloaded" } : { kind: "loaded", nextCursor: null },
    stale: false,
    hasLoaded: false,
  };
}

function syntheticPath(directory: string, kind: string): string {
  return `${directory} ${kind}`;
}
