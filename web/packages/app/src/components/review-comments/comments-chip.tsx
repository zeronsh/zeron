/**
 * The composer's staged-comment chip — `composer.rs::render_comments_chip`
 * (:4608-4633): the badge pill (`badges::render`) above the attachment
 * strip, wrapped in the strip's padding (`px(16) pt(12)`). Read-only and
 * deliberately WITHOUT a hover card — the staged set is already visible in
 * the Changes pane, so a card would only repeat it (`details: []`).
 *
 * The pill chrome itself is ticket 20's `BadgePill`.
 */

import { BadgePill } from "../badges";
import { chipLabel } from "../../lib/review-comments";

export interface CommentsChipProps {
  readonly count: number;
}

export function CommentsChip({ count }: CommentsChipProps) {
  if (count === 0) {
    return null;
  }
  return (
    <div className="comments-chip">
      <BadgePill
        badge={{
          icon: "chatRoundLine",
          label: chipLabel(count),
          details: [],
        }}
      />
    </div>
  );
}
