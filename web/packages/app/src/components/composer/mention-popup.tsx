import { useEffect, useRef, type ReactNode } from "react";
import type { FileSearchMatch } from "@zeron/proto";
import { FileIcon } from "../files/file-icon";
import { useResolvedAppearance } from "../../state/appearance";
import { MenuRow } from "../ui/MenuRows";
import { PopoverCard } from "../ui/PopoverCard";
import { MenuScrollbar } from "../ui/Scrollbar";
import { SkeletonRows } from "../ui/Skeleton";
import type { CompletionToken } from "../../lib/mentions";

/**
 * The `@` file-mention completion popup — `render_file_mention_popup`
 * (composer.rs:5192-5334) and its frame `popover::full_width_menu_above`
 * (popover.rs:564-585): absolutely positioned above the pill, spanning its
 * full width, with the `MENU_IN` entrance, a 6px gap, and the 44px frosted
 * backdrop. The card reuses ticket 09's `.popover-card` shell and rows ride
 * `MenuRow`; only the frame is this ticket's.
 *
 * The card's mouse-down keeps the composer's focus (completion choices
 * belong to the input, composer.rs:5202-5209); a press anywhere outside the
 * card dismisses it (`on_mouse_down_out`). An instant unmount on dismiss —
 * the desktop passes `closing: None` here.
 */

export interface CompletionPopupProps {
  /** Dismissed by an outside press (`on_mouse_down_out`, popover.rs). */
  readonly onDismiss: () => void;
  /** The card's mouse-down — focuses the composer's input. */
  readonly onCardMouseDown: () => void;
  /** The popup's accessible name (which completion surface this is). */
  readonly ariaLabel: string;
  readonly children: ReactNode;
}

/**
 * `full_width_menu_above` as one frame — the shared surface of both
 * composer completions (they are mutually exclusive by token shape, so at
 * most one is ever mounted). Mounts as an absolute child of the composer
 * surface, bottom-aligned to the pill's top edge.
 */
export function CompletionPopup(props: CompletionPopupProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const dismissRef = useRef(props.onDismiss);
  dismissRef.current = props.onDismiss;

  // `on_mouse_down_out`: any press outside this popup dismisses it — the
  // composer, the transcript, other chrome. Presses inside (rows, the
  // scrollbar rail) are the card's own.
  useEffect(() => {
    const onPointerDown = (event: PointerEvent): void => {
      const root = rootRef.current;
      if (root !== null && event.target instanceof Node && !root.contains(event.target)) {
        dismissRef.current();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, []);

  return (
    <div
      className="composer-completion"
      ref={rootRef}
      onMouseDown={(event) => {
        // Keep the composer's textarea focused through the press
        // (preventDefault stops the focus steal before it happens — the
        // card's own handler, composer.rs:5202-5209).
        event.preventDefault();
        props.onCardMouseDown();
      }}
    >
      <PopoverCard className="composer-completion-card" style={{ maxHeight: 320 }} ariaLabel={props.ariaLabel}>
        {props.children}
      </PopoverCard>
    </div>
  );
}

export interface MentionPopupProps {
  readonly token: CompletionToken;
  readonly results: readonly FileSearchMatch[];
  readonly active: number | null;
  readonly loading: boolean;
  readonly error: string | null;
  /** Clicking (or Enter/Tab on) row `ix` accepts it. */
  readonly onAccept: (ix: number) => void;
  readonly onDismiss: () => void;
  readonly onCardMouseDown: () => void;
}

/** The mention popup: skeleton / error / empty / rows (composer.rs:5214-5328). */
export function MentionPopup(props: MentionPopupProps) {
  const appearance = useResolvedAppearance();
  const listRef = useRef<HTMLDivElement | null>(null);
  // New result set: the row stack restarts at the top (composer.rs:5131).
  useEffect(() => {
    if (listRef.current !== null) {
      listRef.current.scrollTop = 0;
    }
  }, [props.results]);

  let body: ReactNode;
  if (props.loading && props.results.length === 0) {
    // `popover::skeleton_rows(3)`: 28px slabs pulsing on the shared clock.
    body = <SkeletonRows count={3} />;
  } else if (props.error !== null) {
    // A failure must never read as "No matching files" (composer.rs:3965).
    body = <div className="composer-completion-error" role="alert">{props.error}</div>;
  } else if (props.results.length === 0) {
    body = (
      <div className="composer-completion-empty">
        {props.token.query.length === 0 ? "No files available" : "No matching files"}
      </div>
    );
  } else {
    body = (
      <div className="composer-completion-host">
        <div className="composer-completion-list" ref={listRef}>
          {props.results.map((result, ix) => (
            <MentionRow
              key={`${result.path}:${ix}`}
              path={result.path}
              isDir={result.isDir}
              selected={props.active === ix}
              appearance={appearance}
              onAccept={() => props.onAccept(ix)}
            />
          ))}
        </div>
        <MenuScrollbar scrollRef={listRef} />
      </div>
    );
  }

  return (
    <CompletionPopup
      onDismiss={props.onDismiss}
      onCardMouseDown={props.onCardMouseDown}
      ariaLabel="File suggestions"
    >
      {body}
    </CompletionPopup>
  );
}

/** One result row (composer.rs:5246-5302): the file-type icon (14px), the
 * basename (13px), and the directory (12.5px, truncated, only when set). */
function MentionRow({
  path,
  isDir,
  selected,
  appearance,
  onAccept,
}: {
  readonly path: string;
  readonly isDir: boolean;
  readonly selected: boolean;
  readonly appearance: ReturnType<typeof useResolvedAppearance>;
  readonly onAccept: () => void;
}) {
  // Path split at the LAST `/` (composer.rs:5248-5251).
  const slash = path.lastIndexOf("/");
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  const directory = slash >= 0 ? path.slice(0, slash) : "";
  return (
    <MenuRow
      fadeKey={`file-mention-result-${path}`}
      selected={selected}
      onClick={onAccept}
      className="composer-completion-row"
    >
      <span className="composer-completion-row-icon">
        <FileIcon kind={isDir ? "directory" : "file"} name={path} appearance={appearance} size={14} />
      </span>
      <span className="composer-completion-row-name">{name}</span>
      {directory.length > 0 && <span className="composer-completion-row-directory">{directory}</span>}
    </MenuRow>
  );
}
