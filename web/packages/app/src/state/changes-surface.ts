import { useSyncExternalStore } from "react";
import {
  bodyHeightWith,
  FOLD_TWEEN_WINDOW_MS,
  type DiffDraftAnchor,
  type DiffMode,
  type DiffScope,
  type FileDiff,
  type FileFold,
} from "../lib/diff";
import type { ReviewComment } from "../lib/review-comments";
import { diffLineHeight } from "../lib/typography";
import { uiSettings } from "./ui-settings";

/**
 * The Changes surface's pane-level state — scope, base ref, layout, wrap, and
 * the per-file fold model — one instance per (chat, diff tab), the web peer
 * of the desktop's per-`Changes`-entity state. Ticket 07's right pane mounts
 * a surface's toolbar and body as SEPARATE element trees, and every diff tab
 * keeps its own scope selection for its whole life (`add_diff_surface`:
 * N clicks make N tabs), so this state cannot live in either component's
 * React tree. It lives here, keyed by the surface's stable minted id, and
 * both halves read it through `useSyncExternalStore`.
 *
 * `layout` and `wrap` persist globally through ticket 03's settings store
 * (`diffSplit`/`diffWrap`, written immediately on toggle — the desktop's
 * `settings::update` with `SavePolicy::Immediate`); the fold map and scope
 * are tab-local, in memory only.
 *
 * The fold model is the desktop's `FileFold` + tween arming: toggling a
 * header swaps the file's body rows for one height-animated stand-in whose
 * `from`/`to` are analytic (`bodyHeightWith`), and a settle sweep after the
 * 400ms window swaps in the steady rows. Under `prefers-reduced-motion` (and
 * whenever wrap is on, whose rows have no analytic height) the toggle writes
 * steady state directly — no tween, matching `cx.reduce_motion()` handling.
 */

export interface ChangesSurfaceSnapshot {
  readonly scope: DiffScope;
  readonly baseRef: string | null;
  readonly layout: DiffMode;
  readonly wrap: boolean;
  readonly folds: ReadonlyMap<string, FileFold>;
  /**
   * Bumped whenever the horizontal-scroll-extent inputs change (scope,
   * layout, wrap) — the viewer resets every file's code-plane offset.
   */
  readonly scrollEpoch: number;
  /**
   * The chat's conversation branch and its checkout's branch list, mirrored
   * by the body (which owns the wire) so the toolbar's `{branch} →` label
   * and base picker can render without their own fetch.
   */
  readonly branch: string | null;
  readonly branches: readonly string[];
  /**
   * The pinned commit sha (commit flavour only, `Changes::for_commit`) —
   * the surface's scope never moves off it.
   */
  readonly commitSha: string | null;
}

function reducedMotion(): boolean {
  return (globalThis as { matchMedia?: (query: string) => { matches: boolean } })
    .matchMedia?.("(prefers-reduced-motion: reduce)")
    .matches === true;
}

interface SurfaceState {
  scope: DiffScope;
  baseRef: string | null;
  layout: DiffMode;
  wrap: boolean;
  folds: ReadonlyMap<string, FileFold>;
  scrollEpoch: number;
  branch: string | null;
  branches: readonly string[];
  commitSha: string | null;
}

function freshState(): SurfaceState {
  const settings = uiSettings.getSnapshot();
  return {
    scope: "workingTree",
    baseRef: null,
    layout: settings.diffSplit ? "split" : "unified",
    wrap: settings.diffWrap,
    folds: new Map(),
    scrollEpoch: 0,
    branch: null,
    branches: EMPTY_BRANCHES,
    commitSha: null,
  };
}

const EMPTY_BRANCHES: readonly string[] = [];
const EMPTY_COMMENTS: readonly ReviewComment[] = [];

export class ChangesSurfaceStore {
  readonly #bySurface = new Map<string, SurfaceState>();
  #version = 0;
  readonly #listeners = new Set<() => void>();
  #settleTimer: ReturnType<typeof setTimeout> | null = null;
  /** The parsed files of the ACTIVE surface registration (fold heights). */
  #files: readonly FileDiff[] = [];
  /**
   * The active surface's staged diff comments + draft anchor (ticket 23):
   * `bodyHeightWith` reads the live set at toggle time, so a file whose
   * body carries comment cards folds at the height the rows actually sum
   * to (changes.rs:2324-2329).
   */
  #comments: readonly ReviewComment[] = EMPTY_COMMENTS;
  #draft: DiffDraftAnchor | null = null;

  getVersion = (): number => this.#version;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The state for one (chat, diff tab) — created on first read. */
  snapshotFor(chatId: string, surfaceId: string): ChangesSurfaceSnapshot {
    const key = surfaceKey(chatId, surfaceId);
    let state = this.#bySurface.get(key);
    if (state === undefined) {
      state = freshState();
      this.#bySurface.set(key, state);
    }
    return state;
  }

  /** The viewer's current parse, registered so fold actions can measure. */
  setFiles(files: readonly FileDiff[]): void {
    this.#files = files;
  }

  /**
   * The viewer's current staged comment set + draft anchor (ticket 23) —
   * the fold heights' second analytic input. No notification: the rows
   * themselves re-render through the comment store's own subscription.
   */
  setComments(comments: readonly ReviewComment[], draft: DiffDraftAnchor | null): void {
    this.#comments = comments;
    this.#draft = draft;
  }

  /**
   * The body's mirror of the chat's branch context — the toolbar's
   * ref-selector inputs. No notification when nothing changed.
   */
  setChatContext(chatId: string, surfaceId: string, context: { branch: string | null; branches: readonly string[] }): void {
    this.#update(chatId, surfaceId, (state) => {
      if (state.branch === context.branch && state.branches === context.branches) {
        return null;
      }
      return { ...state, branch: context.branch, branches: context.branches };
    });
  }

  setScope(chatId: string, surfaceId: string, scope: DiffScope): void {
    this.#update(chatId, surfaceId, (state) => {
      if (state.commitSha !== null) {
        // A commit-pinned pane never offers its scope back (for_commit).
        return null;
      }
      if (state.scope === scope) {
        return null;
      }
      return {
        ...state,
        scope,
        // Leaving the branch scope drops its base; entering branch keeps any
        // previously picked base for the return trip (desktop parity: the
        // store's base_ref survives scope switches, only the fetch changes).
        baseRef: scope === "branch" ? state.baseRef : null,
        scrollEpoch: state.scrollEpoch + 1,
      };
    });
  }

  /**
   * Pin a commit-diff tab to its sha (`Changes::for_commit`): the scope
   * becomes `commit` for the surface's whole life — there is no scope row
   * to move it back.
   */
  pinCommit(chatId: string, surfaceId: string, sha: string): void {
    this.#update(chatId, surfaceId, (state) => {
      if (state.commitSha === sha) {
        return null;
      }
      return {
        ...state,
        scope: "commit",
        baseRef: null,
        commitSha: sha,
        scrollEpoch: state.scrollEpoch + 1,
      };
    });
  }

  setBaseRef(chatId: string, surfaceId: string, base: string): void {
    this.#update(chatId, surfaceId, (state) => {
      if (state.scope !== "branch" || state.baseRef === base) {
        return null;
      }
      return { ...state, baseRef: base, scrollEpoch: state.scrollEpoch + 1 };
    });
  }

  /**
   * Unified ⇄ split (`toggle_mode`): persisted immediately, every file's
   * horizontal scroll reset, the row model re-flattened by the re-render.
   */
  toggleLayout(chatId: string, surfaceId: string): void {
    this.#update(chatId, surfaceId, (state) => {
      const layout: DiffMode = state.layout === "split" ? "unified" : "split";
      uiSettings.update({ diffSplit: layout === "split" }, "immediate");
      return { ...state, layout, scrollEpoch: state.scrollEpoch + 1 };
    });
  }

  /**
   * Wrap ⇄ nowrap (`toggle_wrap`). Wrapped rows have no analytic height, so
   * any folding stand-in settles to steady rows first — the desktop does the
   * same before `remeasure`.
   */
  toggleWrap(chatId: string, surfaceId: string): void {
    this.#update(chatId, surfaceId, (state) => {
      const wrap = !state.wrap;
      uiSettings.update({ diffWrap: wrap }, "immediate");
      const folds = wrap ? settleFolds(state.folds) : state.folds;
      return { ...state, wrap, folds, scrollEpoch: state.scrollEpoch + 1 };
    });
  }

  /**
   * Fold/unfold one file (`toggle_fold`). With wrap on or motion reduced the
   * write is steady-state; otherwise the body becomes a stand-in row tweened
   * from `from` to `to` over the 180ms COLLAPSE curve, settled by the sweep.
   */
  toggleFold(chatId: string, surfaceId: string, path: string): void {
    const file = this.#files.find((candidate) => candidate.path === path);
    if (file === undefined) {
      return;
    }
    const fileComments = this.#comments.filter(
      (comment) => comment.source.kind === "diff" && comment.path === path,
    );
    const fileDraft = this.#draft !== null && this.#draft.path === path ? this.#draft : null;
    let armed = false;
    this.#update(chatId, surfaceId, (state) => {
      const current = state.folds.get(path);
      const collapsed = !(current?.collapsed ?? false);
      const steady = state.wrap || reducedMotion();
      // `bodyHeightWith` reads the live code size at toggle time (the same
      // value the renderer's FileBodyUpto paints against).
      const bodyLineHeight = diffLineHeight(uiSettings.getSnapshot().codeFontSize);
      const fold: FileFold = steady
        ? { collapsed, epoch: (current?.epoch ?? 0) + 1, from: 0, to: 0, toggledAt: null, folding: false }
        : {
          collapsed,
          epoch: (current?.epoch ?? 0) + 1,
          from: current?.collapsed === true ? 0 : bodyHeightWith(file, state.layout, fileComments, fileDraft, bodyLineHeight),
          to: current?.collapsed === true ? bodyHeightWith(file, state.layout, fileComments, fileDraft, bodyLineHeight) : 0,
          toggledAt: Date.now(),
          folding: true,
        };
      const folds = new Map(state.folds);
      folds.set(path, fold);
      const next = { ...state, folds };
      armed = !steady;
      return next;
    });
    if (armed) {
      this.#armSettle();
    }
  }

  /**
   * Collapse every file, or expand them all when everything is already shut
   * (`toggle_collapse_all`) — a steady-state write, no per-row tween.
   */
  toggleCollapseAll(chatId: string, surfaceId: string): void {
    if (this.#files.length === 0) {
      return;
    }
    const collapse = !this.#files.every(
      (file) => this.snapshotFor(chatId, surfaceId).folds.get(file.path)?.collapsed === true,
    );
    this.#update(chatId, surfaceId, (state) => {
      const folds = new Map<string, FileFold>();
      for (const file of this.#files) {
        folds.set(file.path, { collapsed: collapse, epoch: 0, from: 0, to: 0, toggledAt: null, folding: false });
      }
      return { ...state, folds };
    });
  }

  /** Drop a closed tab's state (`diffs.remove` teardown). */
  dispose(chatId: string, surfaceId: string): void {
    const key = surfaceKey(chatId, surfaceId);
    const state = this.#bySurface.get(key);
    if (state === undefined) {
      return;
    }
    this.#bySurface.delete(key);
    this.#version += 1;
    this.#emit();
  }

  #update(chatId: string, surfaceId: string, next: (state: SurfaceState) => SurfaceState | null): void {
    const key = surfaceKey(chatId, surfaceId);
    let state = this.#bySurface.get(key);
    if (state === undefined) {
      state = freshState();
      this.#bySurface.set(key, state);
    }
    const updated = next(state);
    if (updated === null) {
      return;
    }
    this.#bySurface.set(key, updated);
    this.#version += 1;
    this.#emit();
  }

  /**
   * The settle sweep: while any stand-in rows remain, tick after the tween
   * window and convert the elapsed ones to steady rows (`ensure_fold_settle`).
   */
  #armSettle(): void {
    if (this.#settleTimer !== null) {
      return;
    }
    this.#settleTimer = setTimeout(() => {
      this.#settleTimer = null;
      let pending = false;
      for (const [key, state] of this.#bySurface) {
        if (!someFolding(state.folds)) {
          continue;
        }
        const folds = settleFolds(state.folds);
        pending = someFolding(folds) || pending;
        this.#bySurface.set(key, { ...state, folds });
      }
      if (pending) {
        this.#armSettle();
      }
      this.#version += 1;
      this.#emit();
    }, FOLD_TWEEN_WINDOW_MS);
  }

  #emit(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** True while any fold still owns a stand-in row. */
function someFolding(folds: ReadonlyMap<string, FileFold>): boolean {
  for (const fold of folds.values()) {
    if (fold.folding) {
      return true;
    }
  }
  return false;
}

/** Convert elapsed tweens to steady rows; returns the input when none ran. */
function settleFolds(folds: ReadonlyMap<string, FileFold>): ReadonlyMap<string, FileFold> {
  let next: Map<string, FileFold> | null = null;
  for (const [path, fold] of folds) {
    if (!fold.folding) {
      continue;
    }
    const elapsed = fold.toggledAt === null || Date.now() - fold.toggledAt >= FOLD_TWEEN_WINDOW_MS;
    if (!elapsed) {
      continue;
    }
    if (next === null) {
      next = new Map(folds);
    }
    next.set(path, { ...fold, folding: false, toggledAt: null });
  }
  return next ?? folds;
}

function surfaceKey(chatId: string, surfaceId: string): string {
  return `${chatId}\u0000${surfaceId}`;
}

export const changesSurfaceStore = new ChangesSurfaceStore();

/**
 * Bind one (chat, diff tab) surface's state. Both the toolbar and the body
 * call this; the store instance is shared, the snapshot read per render.
 */
export function useChangesSurface(chatId: string, surfaceId: string): ChangesSurfaceSnapshot {
  const subscribe = changesSurfaceStore.subscribe;
  const version = useSyncExternalStore(subscribe, changesSurfaceStore.getVersion, changesSurfaceStore.getVersion);
  void version;
  return changesSurfaceStore.snapshotFor(chatId, surfaceId);
}
