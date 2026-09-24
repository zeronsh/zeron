/**
 * The diff line's comment adder — `comment_ui.rs::render_comment_adder`
 * (:18-46): a 16px solid square with an 11px plus. Rendered only while its
 * owning line is hovered (mount/unmount, per ticket 22's hover-hook
 * contract), positioned by the row's `.diff-adder-slot` at
 * `comment_adder_left` (lib/diff.ts).
 */

import type { MouseEvent as ReactMouseEvent } from "react";
import { Icon } from "@zeron/icons";

export interface CommentAdderProps {
  /** `open_draft` (changes.rs:2745) — the click target's `(path, side, line)`. */
  readonly onOpen: () => void;
}

export function CommentAdder({ onOpen }: CommentAdderProps) {
  return (
    <button
      type="button"
      className="comment-adder"
      aria-label="Add comment"
      onMouseDown={(event: ReactMouseEvent<HTMLButtonElement>) => event.stopPropagation()}
      onClick={(event: ReactMouseEvent<HTMLButtonElement>) => {
        // Must not steal the row's own hover/click handling
        // (`cx.stop_propagation`, comment_ui.rs:35-39).
        event.stopPropagation();
        onOpen();
      }}
    >
      <Icon name="plus" size={11} />
    </button>
  );
}
