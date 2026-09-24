/**
 * The diff-side inline draft — `comment_ui.rs::render_comment_draft`
 * (:186-287): fixed 116px so an open draft never fights the fold tween,
 * `ink(0.08)` plate with a 3px accent bar at 0.7, the location header, a
 * 46px embedded input ("Request a change…"), and the Cancel / Comment
 * (Save while editing) action row. Escape cancels; Enter commits.
 */

import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { Icon } from "@zeron/icons";
import { DRAFT_CARD_HEIGHT } from "../../lib/review-comments";

export interface CommentDraftProps {
  /** The header cites `draft_cite_path` — the pre-rename path on the Old side (changes.rs:3291-3294). */
  readonly path: string;
  readonly line: number;
  readonly body: string;
  /** `draft.editing_id.is_some()` — the primary button reads "Save". */
  readonly editing: boolean;
  readonly onBody: (body: string) => void;
  readonly onCancel: () => void;
  readonly onCommit: () => void;
}

export function CommentDraft({ path, line, body, editing, onBody, onCancel, onCommit }: CommentDraftProps) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  // The desktop focuses the embedded input the moment the draft opens
  // (`window.focus(&handle)`, changes.rs:2763).
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div
      className={`comment-draft${editing ? " comment-draft-editing" : ""}`}
      style={{ height: DRAFT_CARD_HEIGHT }}
      onMouseDown={(event: ReactMouseEvent<HTMLDivElement>) => event.stopPropagation()}
    >
      <span className="comment-accent comment-draft-accent" aria-hidden />
      <div className="comment-draft-column">
        <div className="comment-draft-header">
          <Icon name="chatRoundLine" size={12} className="comment-draft-icon" />
          <span className="comment-draft-location mono">{`${path}:${line}`}</span>
        </div>
        <textarea
          ref={inputRef}
          className="comment-draft-input"
          value={body}
          placeholder="Request a change…"
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
              // `ComposerInputEvent::Submitted` → `commit_draft`.
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
          <button
            type="button"
            className="comment-action comment-action-primary"
            onClick={onCommit}
          >
            {editing ? "Save" : "Comment"}
          </button>
        </div>
      </div>
    </div>
  );
}
