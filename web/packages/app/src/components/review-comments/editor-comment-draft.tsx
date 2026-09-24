/**
 * The Files preview's floating comment draft — `preview.rs::
 * render_editor_comment_draft` (:2987-3060): the popover-card shell at a
 * FIXED 92px, a 48px input row (radius 7, hairline border, input glass
 * plate), and the Cancel / Comment (Save while editing) action row.
 * Escape cancels; Enter commits; an empty commit is a no-op that re-opens
 * the card view (the store's `commitEditorDraft` owns that rule).
 *
 * Placeholder: "Add a comment…" for code files, "Request a change…" for
 * Markdown files (preview.rs:911-921).
 */

import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { EDITOR_COMMENT_DRAFT_HEIGHT } from "../../lib/review-comments";
import { PopoverCard } from "../ui/PopoverCard";

export interface EditorCommentDraftProps {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly body: string;
  readonly editing: boolean;
  /** The placeholder pair — the host picks by file type (preview.rs:911). */
  readonly placeholder: string;
  readonly onBody: (body: string) => void;
  readonly onCancel: () => void;
  readonly onCommit: () => void;
}

export function EditorCommentDraft({
  left,
  top,
  width,
  body,
  editing,
  placeholder,
  onBody,
  onCancel,
  onCommit,
}: EditorCommentDraftProps) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => {
    inputRef.current?.focus();
  }, []);
  return (
    // The shared `PopoverCard` frame (ticket 18) — the raw div carried the
    // `.popover-card` class by hand; the positioning stays selection-anchored
    // (absolute, the caller's coordinates — the sanctioned exception).
    <PopoverCard
      className="editor-comment-draft"
      style={{ position: "absolute", left: `${left}px`, top: `${top}px`, width: `${width}px`, height: `${EDITOR_COMMENT_DRAFT_HEIGHT}px` }}
    >
      <textarea
        ref={inputRef}
        className="editor-comment-input"
        value={body}
        placeholder={placeholder}
        rows={1}
        spellCheck={false}
        onChange={(event) => onBody(event.target.value)}
        onKeyDown={(event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
          if (event.nativeEvent.isComposing) {
            return;
          }
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
            return;
          }
          if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            event.stopPropagation();
            onCommit();
          }
        }}
      />
      <div className="comment-actions">
        <button type="button" className="comment-action" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="comment-action comment-action-primary" onClick={onCommit}>
          {editing ? "Save" : "Comment"}
        </button>
      </div>
    </PopoverCard>
  );
}
