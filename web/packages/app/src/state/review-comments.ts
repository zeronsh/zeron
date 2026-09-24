import { useSyncExternalStore } from "react";
import {
  diffAnchor,
  newDiffComment,
  newFileComment,
  type CommentSide,
  type ReviewComment,
} from "../lib/review-comments";

/**
 * The staged review comments + the two draft surfaces — the web peer of
 * `AppState.review_comments` (state.rs:580, 692-756) plus the desktop's two
 * draft entities: the diff-side draft (`changes.rs::CommentDraft`) and the
 * editor-side draft (`preview.rs::EditorCommentDraft`).
 *
 * Keying matches the composer's `stagedByChat` convention (state.rs:688-690:
 * `composer_key` = the selected chat id, `""` on the new-chat canvas), so a
 * chat switch never leaks one chat's drafts into another's chip. Comments
 * stage onto the key the DRAFT was opened against (`draft.key`), not the
 * live selection.
 *
 * One module-level store (not React state) because three surfaces read it:
 * the composer (chip + send), the Changes pane (adder/draft/card), and the
 * Files preview (gutter icon + floating card/draft).
 */

/** The diff-side draft (`changes.rs::CommentDraft`). */
export interface DiffCommentDraft {
  readonly path: string;
  readonly side: CommentSide;
  readonly line: number;
  /** The parsed diff's pre-rename path — an Old-side comment cites it. */
  readonly oldPath: string | null;
  /** The staged comment being edited, `null` while composing a new one. */
  readonly editingId: string | null;
  readonly body: string;
}

/** The editor-side draft (`preview.rs::EditorCommentDraft`). */
export interface EditorCommentDraft {
  readonly path: string;
  readonly line: number;
  readonly editingId: string | null;
  readonly body: string;
}

export interface ReviewCommentSnapshot {
  /** The staged set for this chat (diff- and file-sourced, in staged order). */
  readonly comments: readonly ReviewComment[];
  readonly diffDraft: DiffCommentDraft | null;
  readonly editorDraft: EditorCommentDraft | null;
  /** The open editor-side card's comment id (`preview.active_comment`). */
  readonly activeEditorComment: string | null;
}

const EMPTY_COMMENTS: readonly ReviewComment[] = [];
const EMPTY_SNAPSHOT: ReviewCommentSnapshot = {
  comments: EMPTY_COMMENTS,
  diffDraft: null,
  editorDraft: null,
  activeEditorComment: null,
};

class ReviewCommentStore {
  readonly #staged = new Map<string, ReviewComment[]>();
  readonly #diffDrafts = new Map<string, DiffCommentDraft>();
  readonly #editorDrafts = new Map<string, EditorCommentDraft>();
  readonly #activeEditor = new Map<string, string | null>();
  readonly #snapshots = new Map<string, ReviewCommentSnapshot>();
  readonly #listeners = new Set<() => void>();
  #version = 0;

  getVersion = (): number => this.#version;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  snapshotFor(chatKey: string): ReviewCommentSnapshot {
    return this.#snapshots.get(chatKey) ?? EMPTY_SNAPSHOT;
  }

  stagedFor(chatKey: string): readonly ReviewComment[] {
    return this.#staged.get(chatKey) ?? EMPTY_COMMENTS;
  }

  /** `add_review_comment` (state.rs:699-703). */
  addComment(chatKey: string, comment: ReviewComment): void {
    this.#staged.set(chatKey, [...(this.#staged.get(chatKey) ?? []), comment]);
    this.#emitFor(chatKey);
  }

  /**
   * `update_review_comment_body` (state.rs:718-728): updates only a staged
   * comment's body — a stale editor must never recreate a comment that has
   * already been removed or sent.
   */
  updateCommentBody(chatKey: string, id: string, body: string): void {
    const list = this.#staged.get(chatKey);
    const at = list?.findIndex((comment) => comment.id === id) ?? -1;
    if (list === undefined || at < 0) {
      return;
    }
    const next = list.slice();
    next[at] = { ...next[at]!, body };
    this.#staged.set(chatKey, next);
    this.#emitFor(chatKey);
  }

  /** `remove_review_comment` (state.rs:706-712) — the empty slot is dropped. */
  removeComment(chatKey: string, id: string): void {
    const list = this.#staged.get(chatKey);
    if (list === undefined) {
      return;
    }
    const next = list.filter((comment) => comment.id !== id);
    if (next.length === 0) {
      this.#staged.delete(chatKey);
    } else {
      this.#staged.set(chatKey, next);
    }
    this.#emitFor(chatKey);
  }

  /** `take_review_comments` (state.rs:749-752) — the send-time snapshot-and-clear. */
  takeComments(chatKey: string): ReviewComment[] {
    const taken = this.#staged.get(chatKey) ?? [];
    if (taken.length > 0) {
      this.#staged.delete(chatKey);
      this.#emitFor(chatKey);
    }
    return taken;
  }

  /** The send-failure hand-back (composer.rs:6665) — re-stage the taken set. */
  restoreComments(chatKey: string, comments: readonly ReviewComment[]): void {
    if (comments.length === 0) {
      return;
    }
    this.#staged.set(chatKey, [...(this.#staged.get(chatKey) ?? []), ...comments]);
    this.#emitFor(chatKey);
  }

  /** `purge_review_comments` (state.rs:754-757) — a deleted chat's stage can never be sent again. */
  purgeChat(chatKey: string): void {
    if (
      !this.#staged.has(chatKey) &&
      !this.#diffDrafts.has(chatKey) &&
      !this.#editorDrafts.has(chatKey) &&
      !this.#activeEditor.has(chatKey)
    ) {
      return;
    }
    this.#staged.delete(chatKey);
    this.#diffDrafts.delete(chatKey);
    this.#editorDrafts.delete(chatKey);
    this.#activeEditor.delete(chatKey);
    this.#emitFor(chatKey);
  }

  // ── The diff-side draft (changes.rs:2745-2815) ──────────────────────────

  /** `open_draft` (changes.rs:2745): a fresh draft at a diff-line anchor. */
  openDiffDraft(chatKey: string, anchor: { path: string; side: CommentSide; line: number; oldPath: string | null }): void {
    this.#diffDrafts.set(chatKey, { ...anchor, editingId: null, body: "" });
    this.#emitFor(chatKey);
  }

  /**
   * `edit_comment` (changes.rs:2748-2772): re-open a staged diff comment
   * into the draft, pre-filled, anchored at its own `(path, side, line)`.
   */
  editDiffComment(chatKey: string, id: string): void {
    const comment = this.stagedFor(chatKey).find(
      (candidate) => candidate.id === id && candidate.source.kind === "diff",
    );
    const anchor = comment === undefined ? null : diffAnchor(comment);
    if (comment === undefined || anchor === null) {
      return;
    }
    this.#diffDrafts.set(chatKey, {
      path: comment.path,
      side: anchor.side,
      line: anchor.line,
      oldPath: comment.source.kind === "diff" ? comment.source.oldPath : null,
      editingId: comment.id,
      body: comment.body,
    });
    this.#emitFor(chatKey);
  }

  setDiffDraftBody(chatKey: string, body: string): void {
    const draft = this.#diffDrafts.get(chatKey);
    if (draft === undefined) {
      return;
    }
    this.#diffDrafts.set(chatKey, { ...draft, body });
    this.#emitFor(chatKey);
  }

  /** `cancel_draft` (changes.rs:2769). */
  cancelDiffDraft(chatKey: string): void {
    if (!this.#diffDrafts.delete(chatKey)) {
      return;
    }
    this.#emitFor(chatKey);
  }

  /**
   * `commit_draft` (changes.rs:2774-2794): an empty body discards the draft
   * silently; a non-empty one updates the edited comment's body or stages a
   * new one (carrying the rename's old path onto its source).
   */
  commitDiffDraft(chatKey: string): void {
    const draft = this.#diffDrafts.get(chatKey);
    if (draft === undefined) {
      return;
    }
    const body = draft.body.trim();
    this.#diffDrafts.delete(chatKey);
    if (body.length === 0) {
      this.#emitFor(chatKey);
      return;
    }
    if (draft.editingId !== null) {
      this.updateCommentBody(chatKey, draft.editingId, body);
      return;
    }
    this.addComment(chatKey, newDiffComment(draft.path, draft.side, draft.line, body, draft.oldPath));
  }

  // ── The editor-side draft + card (preview.rs:904-1090) ──────────────────

  /** `open_editor_comment_draft` (preview.rs:904-934). */
  openEditorDraft(chatKey: string, path: string, line: number): void {
    this.#editorDrafts.set(chatKey, { path, line, editingId: null, body: "" });
    this.#activeEditor.set(chatKey, null);
    this.#emitFor(chatKey);
  }

  /** `edit_editor_comment` (preview.rs:944-967): the card re-opened for editing. */
  editEditorComment(chatKey: string, id: string): void {
    const comment = this.stagedFor(chatKey).find(
      (candidate) => candidate.id === id && candidate.source.kind === "file",
    );
    if (comment === undefined) {
      return;
    }
    this.#editorDrafts.set(chatKey, { path: comment.path, line: comment.line, editingId: comment.id, body: comment.body });
    this.#activeEditor.set(chatKey, null);
    this.#emitFor(chatKey);
  }

  setEditorDraftBody(chatKey: string, body: string): void {
    const draft = this.#editorDrafts.get(chatKey);
    if (draft === undefined) {
      return;
    }
    this.#editorDrafts.set(chatKey, { ...draft, body });
    this.#emitFor(chatKey);
  }

  /** `cancel_editor_comment` (preview.rs:969-974): the card view returns. */
  cancelEditorDraft(chatKey: string): void {
    const draft = this.#editorDrafts.get(chatKey);
    if (draft === undefined) {
      return;
    }
    this.#editorDrafts.delete(chatKey);
    this.#activeEditor.set(chatKey, draft.editingId);
    this.#emitFor(chatKey);
  }

  /**
   * `commit_editor_comment` (preview.rs:977-1022): an empty body is a NO-OP
   * that re-opens the card view (the comment is neither created nor
   * deleted); a non-empty one updates or stages the File-sourced comment.
   */
  commitEditorDraft(chatKey: string): void {
    const draft = this.#editorDrafts.get(chatKey);
    if (draft === undefined) {
      return;
    }
    const body = draft.body.trim();
    if (body.length === 0) {
      this.#editorDrafts.delete(chatKey);
      this.#activeEditor.set(chatKey, draft.editingId);
      this.#emitFor(chatKey);
      return;
    }
    this.#editorDrafts.delete(chatKey);
    if (draft.editingId !== null) {
      this.updateCommentBody(chatKey, draft.editingId, body);
      this.#activeEditor.set(chatKey, draft.editingId);
      this.#emitFor(chatKey);
      return;
    }
    this.addComment(chatKey, newFileComment(draft.path, draft.line, body));
    this.#activeEditor.set(chatKey, null);
    this.#emitFor(chatKey);
  }

  /** `toggle_editor_comment` (preview.rs:1083-1091). */
  toggleEditorComment(chatKey: string, id: string): void {
    const current = this.#activeEditor.get(chatKey) ?? null;
    this.#activeEditor.set(chatKey, current === id ? null : id);
    if (current !== id) {
      this.#editorDrafts.delete(chatKey);
    }
    this.#emitFor(chatKey);
  }

  #emitFor(chatKey: string): void {
    const comments = this.#staged.get(chatKey) ?? EMPTY_COMMENTS;
    this.#snapshots.set(chatKey, {
      comments,
      diffDraft: this.#diffDrafts.get(chatKey) ?? null,
      editorDraft: this.#editorDrafts.get(chatKey) ?? null,
      activeEditorComment: this.#activeEditor.get(chatKey) ?? null,
    });
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /** Test-only: every live key (the reset loop). */
  snapshotKeysForTests(): readonly string[] {
    return [
      ...this.#staged.keys(),
      ...this.#diffDrafts.keys(),
      ...this.#editorDrafts.keys(),
      ...this.#activeEditor.keys(),
    ];
  }

  /** Test-only: drop the snapshot cache (the maps are purged by key first). */
  clearSnapshotsForTests(): void {
    this.#snapshots.clear();
    this.#version += 1;
  }
}

export const reviewCommentStore = new ReviewCommentStore();

/**
 * Bind one chat's staged set + drafts. Re-subscribes when the chat key
 * moves (navigation); the snapshot identity is stable per mutation.
 */
export function useReviewComments(chatKey: string): ReviewCommentSnapshot {
  const subscribe = reviewCommentStore.subscribe;
  const getVersion = reviewCommentStore.getVersion;
  const version = useSyncExternalStore(subscribe, getVersion, getVersion);
  void version;
  return reviewCommentStore.snapshotFor(chatKey);
}

/** Test-only escape hatch so each test starts from a clean slate. */
export function __resetReviewCommentsForTests(): void {
  for (const key of reviewCommentStore.snapshotKeysForTests()) {
    reviewCommentStore.purgeChat(key);
  }
  reviewCommentStore.clearSnapshotsForTests();
}
