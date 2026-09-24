/**
 * The Files preview's floating comment card — `preview.rs::
 * render_editor_comment_card` (:2896-2985): the same header/body/edit-pen/
 * remove-× family as the diff-side `CommentCard`, but a FLOATING overlay —
 * the `.popover-card` chrome (radius 12, hairline 0.10, glass overlay over
 * the 44px frost) positioned absolutely at the anchor math's
 * left/top/width, height `card_height(body)`, `px(16) py(10)`.
 */

import type { MouseEvent as ReactMouseEvent } from "react";
import { Icon } from "@zeron/icons";
import { PopoverCard } from "../ui/PopoverCard";
import { cardHeight, location, type ReviewComment } from "../../lib/review-comments";
import { CommentEditButton } from "./comment-card";

export interface EditorCommentCardProps {
  readonly comment: ReviewComment;
  /** The overlay geometry (lib/review-comments' anchor math). */
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly onEdit: (id: string) => void;
  readonly onRemove: (id: string) => void;
}

export function EditorCommentCard({ comment, left, top, width, onEdit, onRemove }: EditorCommentCardProps) {
  return (
    <PopoverCard
      className="editor-comment-card"
      style={{
        position: "absolute",
        left: `${left}px`,
        top: `${top}px`,
        width: `${width}px`,
        height: `${cardHeight(comment.body)}px`,
      }}
    >
      <div className="editor-comment-header">
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
    </PopoverCard>
  );
}
