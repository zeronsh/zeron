/**
 * The diff-side staged card — `comment_ui.rs::render_comment_card`
 * (:48-145): the 3px accent bar at 0.35 over an `ink(0.05)` plate, the 22px
 * header (icon + mono location + the hover-revealed edit pen and remove ×),
 * and the body clipped to the ANALYTIC height (`card_height`, never
 * measured, so it composes with the changes pane's fold math exactly).
 *
 * `CommentEditButton` is shared verbatim with the Files preview's floating
 * editor card (preview.rs:2941) — one component, two mount points.
 */

import type { MouseEvent as ReactMouseEvent } from "react";
import { Icon } from "@zeron/icons";
import { cardHeight, location, type ReviewComment } from "../../lib/review-comments";

export interface CommentCardProps {
  readonly comment: ReviewComment;
  readonly onEdit: (id: string) => void;
  readonly onRemove: (id: string) => void;
}

export function CommentCard({ comment, onEdit, onRemove }: CommentCardProps) {
  return (
    <div className="comment-card" style={{ height: cardHeight(comment.body) }}>
      <span className="comment-accent comment-card-accent" aria-hidden />
      <div className="comment-card-column">
        <div className="comment-card-header">
          <Icon name="chatRoundLine" size={12} className="comment-card-icon" />
          <span className="comment-card-location mono">{location(comment)}</span>
          <CommentEditButton commentId={comment.id} onEdit={onEdit} />
          <button
            type="button"
            className="comment-card-action comment-card-remove"
            aria-label="Remove comment"
            onMouseDown={(event: ReactMouseEvent<HTMLButtonElement>) => event.stopPropagation()}
            onClick={(event: ReactMouseEvent<HTMLButtonElement>) => {
              event.stopPropagation();
              onRemove(comment.id);
            }}
          >
            <Icon name="closeCircle" size={12} />
          </button>
        </div>
        <div className="comment-card-body">{comment.body}</div>
      </div>
    </div>
  );
}

/**
 * `render_comment_edit` (comment_ui.rs:147-179): the hover-revealed pen —
 * 16px, radius 4, 12px pen in `text_muted`, idle opacity 0, revealed by the
 * ENCLOSING card's hover (not its own). Pointer-reachable only, no
 * tab-index, matching desktop.
 */
export function CommentEditButton({ commentId, onEdit }: { commentId: string; onEdit: (id: string) => void }) {
  return (
    <button
      type="button"
      className="comment-card-action comment-card-edit"
      aria-label="Edit comment"
      onMouseDown={(event: ReactMouseEvent<HTMLButtonElement>) => event.stopPropagation()}
      onClick={(event: ReactMouseEvent<HTMLButtonElement>) => {
        event.stopPropagation();
        onEdit(commentId);
      }}
    >
      <Icon name="pen" size={12} />
    </button>
  );
}
