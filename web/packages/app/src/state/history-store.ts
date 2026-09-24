import type { GitHistoryCommit, GitHistoryComparison, GitHistoryPage } from "@zeron/proto";
import { motion } from "@zeron/theme";
import { methods } from "@zeron/engine-client";
import {
  collapseBranchRuns,
  compactCommitsToVisible,
  gitHistoryMatches,
  historyTransitionRows,
  layoutGraph,
  decodeHistoryAvatar,
  HISTORY_PAGE_SIZE,
  HISTORY_SEARCH_DEBOUNCE_MS,
  type GraphLayout,
  type HistoryRowTransition,
} from "../lib/git-history";

/**
 * The History pane's store — the web peer of the desktop's `GitHistory`
 * entity (`crates/ui/src/history.rs`), one instance per (chat, History tab),
 * created and disposed by the surface component like `ChangesStore`.
 *
 * The four RPCs are one-shot `call()`s: `ListGitHistory` pages forward from
 * a cursor, `SearchGitHistory` rides a 70ms debounce behind a synchronous
 * local re-filter of the loaded page (both paths run — instant feedback
 * plus the full-repository result), `ResolveGitAvatars` batches per page
 * boundary when the author display is Avatar, and `FetchAll` (`repoPath`)
 * reloads page 0 on success. Paging is uniform at
 * `HISTORY_PAGE_SIZE` (100), cursor-based, forward-only.
 *
 * View changes (the All-commits/Branch-tips toggle, a branch fold) animate
 * through `history_transition_rows`' interim list and settle after the 180ms
 * COLLAPSE window; row-list identity flows through `historyListSplice` so a
 * no-op change never notifies.
 */

/** `motion::COLLAPSE` — the view-transition settle window. */
const COLLAPSE_MS = motion.specs.find((spec) => spec.name === "collapse")?.durationMs ?? 180;
/** How long the SHA pill shows "Copied". */
const COPIED_MS = 1200;

export type HistoryViewMode = "allCommits" | "branchTips";
export type HistoryAuthorDisplay = "avatar" | "name";

export interface HistorySnapshot {
  readonly ready: boolean;
  readonly cwd: string | null;
  readonly visibleCommits: readonly GitHistoryCommit[];
  readonly viewMode: HistoryViewMode;
  readonly collapsedBranches: ReadonlySet<string>;
  readonly collapsedCounts: ReadonlyMap<string, number>;
  readonly headSha: string | null;
  readonly searchQuery: string;
  readonly searchActive: boolean;
  readonly searchLoading: boolean;
  readonly searchError: string | null;
  readonly searchTotalCount: number | null;
  readonly headCommitCount: number | null;
  readonly totalCount: number | null;
  readonly comparison: GitHistoryComparison | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly fetchingAll: boolean;
  readonly fetchError: string | null;
  readonly rowTransitions: readonly HistoryRowTransition[] | null;
  readonly graph: GraphLayout;
  readonly graphLaneCapacity: number;
  readonly hasLoadMore: boolean;
  readonly avatars: ReadonlyMap<string, string>;
  readonly copiedSha: string | null;
  /** Bumped on every visible-row-list change — the anchor-restore hook. */
  readonly listEpoch: number;
}

/** The RPC surface the store needs — `EngineClient` satisfies it. */
export interface HistoryClient {
  call<T>(method: string, params?: unknown): Promise<T>;
}

/** Where the history lives: the chat's checkout, on its host device. */
export interface HistoryTarget {
  readonly cwd: string | null;
  readonly targetDeviceId: string | null;
}

const EMPTY_GRAPH: GraphLayout = { rows: [], maxLaneCount: 0 };

function reducedMotion(): boolean {
  return (
    (globalThis as { matchMedia?: (query: string) => { matches: boolean } })
      .matchMedia?.("(prefers-reduced-motion: reduce)")
      .matches === true
  );
}

function historyPageOf(value: unknown): GitHistoryPage | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const page = value as Partial<GitHistoryPage>;
  if (!Array.isArray(page.commits)) {
    return null;
  }
  return {
    commits: page.commits as GitHistoryCommit[],
    branchTips: page.branchTips ?? [],
    headSha: page.headSha ?? null,
    nextCursor: page.nextCursor ?? null,
    totalCount: page.totalCount ?? null,
    headCommitCount: page.headCommitCount ?? null,
    comparison: page.comparison ?? null,
  };
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class HistoryStore {
  readonly #client: HistoryClient;
  #targetKey: string | null = null;
  #cwd: string | null = null;
  #targetDeviceId: string | null = null;
  #commits: GitHistoryCommit[] = [];
  #visibleCommits: GitHistoryCommit[] = [];
  #branchTips: GitHistoryCommit[] = [];
  #viewMode: HistoryViewMode = "allCommits";
  #viewEpoch = 0;
  #viewTransition: {
    rows: HistoryRowTransition[];
    finalCommits: GitHistoryCommit[];
    finalCollapsedCounts: Map<string, number>;
    epoch: number;
  } | null = null;
  #viewTransitionTimer: ReturnType<typeof setTimeout> | null = null;
  #collapsedBranches = new Set<string>();
  #collapsedCounts = new Map<string, number>();
  #headSha: string | null = null;
  #nextCursor: number | null = null;
  #totalCount: number | null = null;
  #headCommitCount: number | null = null;
  #comparison: GitHistoryComparison | null = null;
  #searchQuery = "";
  #searchResults: GitHistoryCommit[] | null = null;
  #searchNextCursor: number | null = null;
  #searchTotalCount: number | null = null;
  #searchLoading = false;
  #searchError: string | null = null;
  #searchGeneration = 0;
  #requestGeneration = 0;
  #searchTimer: ReturnType<typeof setTimeout> | null = null;
  #loading = false;
  #error: string | null = null;
  #fetchingAll = false;
  #fetchFor: string | null = null;
  #fetchError: string | null = null;
  #avatars = new Map<string, string>();
  #authorDisplay: HistoryAuthorDisplay = "avatar";
  #copiedSha: string | null = null;
  #copiedTimer: ReturnType<typeof setTimeout> | null = null;
  #graph: GraphLayout = EMPTY_GRAPH;
  #graphLaneCapacity = 0;
  #listEpoch = 0;
  #disposed = false;
  #snapshot: HistorySnapshot;
  readonly #listeners = new Set<() => void>();

  constructor(
    client: HistoryClient,
    options: { authorDisplay?: HistoryAuthorDisplay } = {},
  ) {
    this.#client = client;
    this.#authorDisplay = options.authorDisplay ?? "avatar";
    this.#snapshot = this.#takeSnapshot();
  }

  /** The RPC client this instance talks to (the registry's retarget check). */
  get client(): HistoryClient {
    return this.#client;
  }

  getSnapshot = (): HistorySnapshot => this.#snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /**
   * `ensure_loaded` (history.rs:2024): no target clears everything; a new
   * target resets wholesale; the same target only fetches when empty and
   * never errored (a refresh is explicit).
   */
  ensureLoaded(target: HistoryTarget): void {
    if (this.#disposed) {
      return;
    }
    if (target.cwd === null) {
      this.#reset(new Map());
      this.#cwd = null;
      this.#targetDeviceId = null;
      this.#targetKey = null;
      this.#commit();
      return;
    }
    const key = `${target.targetDeviceId ?? "local"}|${target.cwd}`;
    if (this.#targetKey === key) {
      if (!this.#loading && this.#commits.length === 0 && this.#error === null) {
        this.#fetchPage(key, target, 0, true);
      }
      return;
    }
    this.#searchTimer = null;
    clearTimeout(this.#viewTransitionTimer ?? undefined);
    this.#viewTransitionTimer = null;
    this.#fetchingAll = false;
    this.#fetchFor = null;
    this.#fetchError = null;
    this.#loading = false;
    this.#targetKey = key;
    this.#cwd = target.cwd;
    this.#targetDeviceId = target.targetDeviceId;
    this.#reset(new Map());
    this.#fetchPage(key, target, 0, true);
  }

  /** `refresh` — the toolbar's refresh button. */
  refresh(): void {
    if (this.#cwd === null || this.#targetKey === null) {
      return;
    }
    this.#fetchPage(this.#targetKey, { cwd: this.#cwd, targetDeviceId: this.#targetDeviceId }, 0, true);
  }

  /** `fetch_all` (history.rs:2108) — then reload page 0 on success. */
  fetchAll(): void {
    if (this.#fetchingAll || this.#cwd === null || this.#targetKey === null) {
      return;
    }
    const key = this.#targetKey;
    const cwd = this.#cwd;
    const target = { cwd, targetDeviceId: this.#targetDeviceId };
    this.#fetchingAll = true;
    this.#fetchFor = key;
    this.#fetchError = null;
    this.#commit();
    const params: Record<string, unknown> = { repoPath: cwd };
    if (this.#targetDeviceId !== null) {
      params.targetDeviceId = this.#targetDeviceId;
    }
    this.#client
      .call(methods.FETCH_ALL, params)
      .then(() => {
        if (this.#disposed || this.#fetchFor !== key) {
          return;
        }
        this.#fetchingAll = false;
        this.#fetchFor = null;
        this.#fetchError = null;
        // Invalidate an in-flight page request (the desktop drops the task)
        // so the reload observes the new remote refs; reload from cursor 0.
        this.#requestGeneration += 1;
        this.#loading = false;
        this.#searchGeneration += 1;
        this.#fetchPage(key, target, 0, true);
      })
      .catch((error: unknown) => {
        if (this.#disposed || this.#fetchFor !== key) {
          return;
        }
        this.#fetchingAll = false;
        this.#fetchFor = null;
        this.#fetchError = `Fetch failed: ${describeError(error)}`;
        this.#commit();
      });
  }

  /** `set_view_mode` — switching back to All-commits clears the folds. */
  setViewMode(mode: HistoryViewMode): void {
    const clearedIndividual = mode === "allCommits" && this.#collapsedBranches.size > 0;
    if (clearedIndividual) {
      this.#collapsedBranches.clear();
    }
    if (this.#viewMode === mode && !clearedIndividual) {
      return;
    }
    this.#viewMode = mode;
    this.#viewEpoch += 1;
    this.#applyViewChange(true);
  }

  /** `toggle_branch_ref` (history.rs:2397). */
  toggleBranchRef(refKey: string): void {
    this.#viewMode = "allCommits";
    this.#viewEpoch += 1;
    if (!this.#collapsedBranches.delete(refKey)) {
      this.#collapsedBranches.add(refKey);
    }
    this.#applyViewChange(true);
  }

  /**
   * `set_search_query` (history.rs:2430): the query is trimmed of leading
   * whitespace, filters the loaded page synchronously, and arms the 70ms
   * debounced full-repository search. Emptying the query rebuilds the plain
   * view.
   */
  setSearchQuery(query: string): void {
    const trimmed = query.replace(/^\s+/, "");
    if (this.#searchQuery === trimmed) {
      return;
    }
    this.#searchGeneration += 1;
    if (this.#searchTimer !== null) {
      clearTimeout(this.#searchTimer);
      this.#searchTimer = null;
    }
    this.#searchQuery = trimmed;
    this.#searchResults = null;
    this.#searchNextCursor = null;
    this.#searchTotalCount = null;
    this.#searchLoading = false;
    this.#searchError = null;
    if (trimmed.trim().length === 0) {
      this.#rebuildView();
      this.#commit();
      return;
    }
    this.#rebuildView();
    this.#commit();
    this.#requestSearchPage(trimmed, 0, true, HISTORY_SEARCH_DEBOUNCE_MS);
  }

  /** `load_older` — the footer row's click. */
  loadOlder(): void {
    if (this.#searchActive()) {
      if (this.#searchLoading) {
        return;
      }
      const cursor = this.#searchNextCursor;
      if (cursor === null) {
        return;
      }
      this.#requestSearchPage(this.#searchQuery, cursor, false, 0);
      return;
    }
    const cursor = this.#nextCursor;
    if (cursor === null || this.#cwd === null || this.#targetKey === null) {
      return;
    }
    this.#fetchPage(
      this.#targetKey,
      { cwd: this.#cwd, targetDeviceId: this.#targetDeviceId },
      cursor,
      false,
    );
  }

  /** `copy_sha` — the clipboard write is the caller's (the DOM's) job. */
  copySha(sha: string): void {
    this.#copiedSha = sha;
    if (this.#copiedTimer !== null) {
      clearTimeout(this.#copiedTimer);
    }
    this.#copiedTimer = setTimeout(() => {
      this.#copiedTimer = null;
      this.#copiedSha = null;
      this.#commit();
    }, COPIED_MS);
    this.#commit();
  }

  /** The author-display preference; Avatar backfills loaded avatars. */
  setAuthorDisplay(display: HistoryAuthorDisplay): void {
    if (this.#authorDisplay === display) {
      return;
    }
    this.#authorDisplay = display;
    if (display === "avatar") {
      this.#resolveLoadedAvatars();
    }
  }

  /** The commit-count label's data (history.rs:2071). */
  commitCount(): number | null {
    if (this.#searchActive()) {
      return this.#searchTotalCount ?? this.#visibleCommits.length;
    }
    return this.#headCommitCount;
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    if (this.#searchTimer !== null) {
      clearTimeout(this.#searchTimer);
    }
    if (this.#viewTransitionTimer !== null) {
      clearTimeout(this.#viewTransitionTimer);
    }
    if (this.#copiedTimer !== null) {
      clearTimeout(this.#copiedTimer);
    }
    this.#listeners.clear();
  }

  // ── Internals ──────────────────────────────────────────────────────────

  #searchActive(): boolean {
    return this.#searchQuery.trim().length > 0;
  }

  #hasLoadMore(): boolean {
    if (this.#searchActive()) {
      return this.#searchNextCursor !== null;
    }
    return this.#viewMode === "allCommits" && this.#nextCursor !== null;
  }

  #reset(avatars: Map<string, string>): void {
    this.#commits = [];
    this.#visibleCommits = [];
    this.#branchTips = [];
    this.#collapsedBranches.clear();
    this.#collapsedCounts = new Map();
    this.#headSha = null;
    this.#nextCursor = null;
    this.#totalCount = null;
    this.#headCommitCount = null;
    this.#comparison = null;
    this.#searchResults = null;
    this.#searchNextCursor = null;
    this.#searchTotalCount = null;
    this.#searchLoading = false;
    this.#searchError = null;
    this.#error = null;
    this.#graph = EMPTY_GRAPH;
    this.#graphLaneCapacity = 0;
    this.#avatars = avatars;
    this.#viewTransition = null;
    if (this.#viewTransitionTimer !== null) {
      clearTimeout(this.#viewTransitionTimer);
      this.#viewTransitionTimer = null;
    }
    this.#listEpoch += 1;
  }

  /** `recompute_view` (history.rs:2088). */
  #recomputeView(): void {
    if (this.#searchActive()) {
      const source = this.#searchResults ?? this.#commits;
      const visible = new Set(
        source.filter((commit) => gitHistoryMatches(this.#searchQuery, commit)).map((commit) => commit.sha),
      );
      this.#visibleCommits = compactCommitsToVisible(source, visible);
      this.#collapsedCounts = new Map();
      this.#updateGraphLayout();
      return;
    }
    if (this.#viewMode === "allCommits") {
      const { visible, hiddenCounts } = collapseBranchRuns(
        this.#commits,
        this.#collapsedBranches,
        this.#headSha,
      );
      this.#visibleCommits = visible;
      this.#collapsedCounts = hiddenCounts;
    } else {
      // Branch tips are independent overview entries — clear the parents so
      // no false adjacency is painted between them (history.rs:2098).
      this.#visibleCommits = this.#branchTips.map((commit) => ({ ...commit, parentShas: [] }));
      this.#collapsedCounts = new Map();
    }
    this.#updateGraphLayout();
  }

  #updateGraphLayout(): void {
    this.#graph = layoutGraph(this.#visibleCommits, this.#headSha);
    // Keep the lane capacity at the widest count ever loaded so a fold
    // cannot shift every subject when its compact graph settles.
    const loaded = layoutGraph(this.#commits, this.#headSha).maxLaneCount;
    this.#graphLaneCapacity = Math.max(this.#graphLaneCapacity, this.#graph.maxLaneCount, loaded);
  }

  #rebuildView(): void {
    this.#viewTransition = null;
    if (this.#viewTransitionTimer !== null) {
      clearTimeout(this.#viewTransitionTimer);
      this.#viewTransitionTimer = null;
    }
    this.#recomputeView();
    this.#listEpoch += 1;
  }

  /**
   * `apply_view_change` (history.rs:2313) — the interim transition list when
   * the change animates (and motion is allowed), else the final list direct.
   */
  #applyViewChange(animateRows: boolean): void {
    if (this.#viewTransition !== null) {
      this.#settleViewTransition();
    }
    const oldCommits = this.#visibleCommits;
    this.#recomputeView();
    const finalCommits = this.#visibleCommits;
    const finalCollapsedCounts = this.#collapsedCounts;

    const unchanged =
      oldCommits.length === finalCommits.length &&
      oldCommits.every((entry, index) => entry.sha === finalCommits[index]?.sha);
    if (unchanged || !animateRows || reducedMotion()) {
      this.#visibleCommits = finalCommits;
      this.#collapsedCounts = finalCollapsedCounts;
      this.#updateGraphLayout();
      this.#listEpoch += 1;
      this.#commit();
      return;
    }

    const { commits, transitions } = historyTransitionRows(oldCommits, finalCommits);
    this.#visibleCommits = commits;
    this.#collapsedCounts = finalCollapsedCounts;
    this.#updateGraphLayout();
    this.#listEpoch += 1;
    const epoch = this.#viewEpoch;
    this.#viewTransition = { rows: transitions, finalCommits, finalCollapsedCounts, epoch };
    if (this.#viewTransitionTimer !== null) {
      clearTimeout(this.#viewTransitionTimer);
    }
    this.#viewTransitionTimer = setTimeout(() => {
      this.#viewTransitionTimer = null;
      if (this.#viewTransition?.epoch === epoch) {
        this.#settleViewTransition();
      }
    }, COLLAPSE_MS);
    this.#commit();
  }

  /** `settle_view_transition` (history.rs:2269) — swap in the final list. */
  #settleViewTransition(): void {
    const transition = this.#viewTransition;
    if (transition === null) {
      return;
    }
    this.#viewTransition = null;
    if (this.#viewTransitionTimer !== null) {
      clearTimeout(this.#viewTransitionTimer);
      this.#viewTransitionTimer = null;
    }
    this.#visibleCommits = transition.finalCommits;
    this.#collapsedCounts = transition.finalCollapsedCounts;
    this.#updateGraphLayout();
    this.#listEpoch += 1;
    this.#commit();
  }

  /** `request_search_page` (history.rs:2463) — debounce + generation guards. */
  #requestSearchPage(query: string, cursor: number, reset: boolean, delayMs: number): void {
    if (this.#cwd === null || this.#targetKey === null) {
      return;
    }
    const key = this.#targetKey;
    const cwd = this.#cwd;
    const target = { cwd, targetDeviceId: this.#targetDeviceId };
    const generation = this.#searchGeneration;
    this.#searchLoading = true;
    this.#searchError = null;
    this.#commit();
    const fire = (): void => {
      if (this.#disposed || this.#searchGeneration !== generation) {
        return;
      }
      const params: Record<string, unknown> = {
        cwd,
        query,
        cursor,
        limit: HISTORY_PAGE_SIZE,
      };
      if (this.#targetDeviceId !== null) {
        params.targetDeviceId = this.#targetDeviceId;
      }
      this.#client
        .call<unknown>(methods.SEARCH_GIT_HISTORY, params)
        .then((value) => {
          if (
            this.#disposed ||
            this.#targetKey !== key ||
            this.#searchGeneration !== generation ||
            this.#searchQuery !== query
          ) {
            return;
          }
          this.#searchLoading = false;
          const page = historyPageOf(value);
          if (page === null) {
            this.#searchError = "Malformed search reply";
            this.#commit();
            return;
          }
          if (reset) {
            this.#searchResults = page.commits;
          } else {
            const seen = new Set(this.#searchResults?.map((entry) => entry.sha));
            const merged = [...(this.#searchResults ?? [])];
            for (const entry of page.commits) {
              if (!seen.has(entry.sha)) {
                seen.add(entry.sha);
                merged.push(entry);
              }
            }
            this.#searchResults = merged;
          }
          this.#searchNextCursor = page.nextCursor;
          this.#searchTotalCount = page.totalCount;
          this.#rebuildView();
          this.#commit();
          this.#resolveAvatars(key, target, cursor);
        })
        .catch((error: unknown) => {
          if (
            this.#disposed ||
            this.#targetKey !== key ||
            this.#searchGeneration !== generation ||
            this.#searchQuery !== query
          ) {
            return;
          }
          this.#searchLoading = false;
          // Keep the instantaneous local matches useful while the full
          // search is unavailable.
          this.#searchError = describeError(error);
          this.#commit();
        });
    };
    if (delayMs > 0) {
      this.#searchTimer = setTimeout(() => {
        this.#searchTimer = null;
        fire();
      }, delayMs);
    } else {
      fire();
    }
  }

  /** `fetch_page` (history.rs:2642). */
  #fetchPage(key: string, target: HistoryTarget, cursor: number, reset: boolean): void {
    if (this.#loading) {
      return;
    }
    this.#loading = true;
    this.#error = null;
    this.#commit();
    const generation = this.#requestGeneration;
    const params: Record<string, unknown> = {
      cwd: target.cwd ?? "",
      cursor,
      limit: HISTORY_PAGE_SIZE,
    };
    if (target.targetDeviceId !== null) {
      params.targetDeviceId = target.targetDeviceId;
    }
    this.#client
      .call<unknown>(methods.LIST_GIT_HISTORY, params)
      .then((value) => {
        if (this.#disposed || this.#targetKey !== key || this.#requestGeneration !== generation) {
          return;
        }
        this.#loading = false;
        const page = historyPageOf(value);
        if (page === null) {
          this.#error = "Malformed history reply";
          this.#commit();
          return;
        }
        const restartSearch = reset && this.#searchActive() ? this.#searchQuery : null;
        if (restartSearch !== null) {
          this.#searchGeneration += 1;
          if (this.#searchTimer !== null) {
            clearTimeout(this.#searchTimer);
            this.#searchTimer = null;
          }
          this.#searchResults = null;
          this.#searchNextCursor = null;
          this.#searchTotalCount = null;
          this.#searchLoading = false;
          this.#searchError = null;
        }
        if (reset) {
          this.#commits = page.commits;
          this.#branchTips = page.branchTips;
          this.#totalCount = page.totalCount;
          this.#headCommitCount = page.headCommitCount;
          this.#comparison = page.comparison ?? null;
        } else {
          const seen = new Set(this.#commits.map((entry) => entry.sha));
          for (const entry of page.commits) {
            if (!seen.has(entry.sha)) {
              seen.add(entry.sha);
              this.#commits.push(entry);
            }
          }
          if (page.totalCount !== null) {
            this.#totalCount = page.totalCount;
          }
          if (page.headCommitCount !== null) {
            this.#headCommitCount = page.headCommitCount;
          }
        }
        this.#headSha = page.headSha;
        this.#nextCursor = page.nextCursor;
        this.#rebuildView();
        this.#commit();
        this.#resolveAvatars(key, target, cursor);
        if (restartSearch !== null) {
          this.#requestSearchPage(restartSearch, 0, true, 0);
        }
      })
      .catch((error: unknown) => {
        if (this.#disposed || this.#targetKey !== key || this.#requestGeneration !== generation) {
          return;
        }
        this.#loading = false;
        this.#error = describeError(error);
        this.#commit();
      });
  }

  /** `resolve_avatars` (history.rs:2558) — one page-boundary batch. */
  #resolveAvatars(key: string, target: HistoryTarget, cursor: number): void {
    if (this.#authorDisplay !== "avatar") {
      return;
    }
    const unique = new Map<string, { sha: string; email: string }>();
    for (const commit of [...this.#commits, ...(this.#searchResults ?? [])]) {
      const email = commit.authorEmail.trim().toLowerCase();
      if (email.length > 0) {
        unique.set(email, { sha: commit.sha, email: commit.authorEmail });
      }
    }
    if (unique.size === 0) {
      return;
    }
    const authors = [...unique.values()].slice(0, 200);
    const params: Record<string, unknown> = {
      cwd: target.cwd ?? "",
      authors,
      cursor,
      limit: HISTORY_PAGE_SIZE,
    };
    if (target.targetDeviceId !== null) {
      params.targetDeviceId = target.targetDeviceId;
    }
    this.#client
      .call<Record<string, string>>(methods.RESOLVE_GIT_AVATARS, params)
      .then((avatars) => {
        if (this.#disposed || this.#targetKey !== key || typeof avatars !== "object" || avatars === null) {
          return;
        }
        let changed = false;
        for (const [email, encoded] of Object.entries(avatars)) {
          const dataUrl = decodeHistoryAvatar(encoded);
          if (dataUrl !== null) {
            this.#avatars.set(email, dataUrl);
            changed = true;
          }
        }
        if (changed) {
          this.#commit();
        }
      })
      .catch(() => {
        // Avatar resolution is best-effort; the initials carry the surface.
      });
  }

  /** `resolve_loaded_avatars` — replay the pages already loaded. */
  #resolveLoadedAvatars(): void {
    if (this.#commits.length === 0 || this.#cwd === null || this.#targetKey === null) {
      return;
    }
    const key = this.#targetKey;
    const target = { cwd: this.#cwd, targetDeviceId: this.#targetDeviceId };
    for (let cursor = 0; cursor < this.#commits.length; cursor += HISTORY_PAGE_SIZE) {
      this.#resolveAvatars(key, target, cursor);
    }
  }

  #takeSnapshot(): HistorySnapshot {
    return {
      ready: this.#targetKey !== null,
      cwd: this.#cwd,
      visibleCommits: this.#visibleCommits,
      viewMode: this.#viewMode,
      collapsedBranches: this.#collapsedBranches,
      collapsedCounts: this.#collapsedCounts,
      headSha: this.#headSha,
      searchQuery: this.#searchQuery,
      searchActive: this.#searchActive(),
      searchLoading: this.#searchLoading,
      searchError: this.#searchError,
      searchTotalCount: this.#searchTotalCount,
      headCommitCount: this.#headCommitCount,
      totalCount: this.#totalCount,
      comparison: this.#comparison,
      loading: this.#loading,
      error: this.#error,
      fetchingAll: this.#fetchingAll,
      fetchError: this.#fetchError,
      rowTransitions: this.#viewTransition?.rows ?? null,
      graph: this.#graph,
      graphLaneCapacity: this.#graphLaneCapacity,
      hasLoadMore: this.#hasLoadMore(),
      avatars: this.#avatars,
      copiedSha: this.#copiedSha,
      listEpoch: this.#listEpoch,
    };
  }

  #commit(): void {
    this.#snapshot = this.#takeSnapshot();
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        // A listener must never take the pane down.
        void describeError(error);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// The per-(chat, History tab) registry
// ---------------------------------------------------------------------------

/**
 * One `HistoryStore` per (chat, History tab), shared by the pane's toolbar
 * and body trees the way `changesSurfaceStore` backs the Changes pair — the
 * host mounts the toolbar and body as separate element trees, so the store
 * cannot live in either component's React tree. Instances survive tab
 * switches (the graph state is the tab's) and drop on surface close; a
 * swapped client (engine reconnect) retargets to a fresh instance so stale
 * in-flight replies hit a disposed store.
 */
const instances = new Map<string, HistoryStore>();

function instanceKey(chatId: string, surfaceId: string): string {
  return `${chatId}\u0000${surfaceId}`;
}

export function historyStoreFor(
  chatId: string,
  surfaceId: string,
  client: HistoryClient,
  options: { authorDisplay?: HistoryAuthorDisplay } = {},
): HistoryStore {
  const key = instanceKey(chatId, surfaceId);
  const existing = instances.get(key);
  if (existing !== undefined && existing.client === client) {
    return existing;
  }
  if (existing !== undefined) {
    existing.dispose();
  }
  const created = new HistoryStore(client, options);
  instances.set(key, created);
  return created;
}

/** Drop a closed tab's store (`diffs.remove` teardown). */
export function disposeHistoryStore(chatId: string, surfaceId: string): void {
  const key = instanceKey(chatId, surfaceId);
  const existing = instances.get(key);
  if (existing !== undefined) {
    existing.dispose();
    instances.delete(key);
  }
}
