import { useEffect, useRef, useState } from "react";
import { chatRowHeight, type ChatRow } from "../lib/view";
import { slideOffset } from "../lib/queue-row-logic";
import {
  pinnedDragScrollDelta,
  pinnedDragScrollStep,
  pinnedSectionBodyHeight,
  pinnedSessionClampedIndex,
  pinnedSessionDropIndex,
  SIDEBAR_DRAG_SCROLL_FRAME_MS,
  SIDEBAR_PINNED_DIVIDER_FRAME_HEIGHT,
  SIDEBAR_PINNED_DIVIDER_KEY,
  SIDEBAR_SESSION_SLOT,
} from "../lib/sidebar-pins";
import { preventNativeSidebarRowDrag } from "../lib/sidebar-drag-events";
import {
  SidebarDisclosureBody,
  SidebarDisclosureHeader,
  useSidebarDisclosure,
  useSidebarDisclosureDivider,
} from "./sidebar-disclosure";

/**
 * The pinned section of the sidebar's session list — the desktop's
 * `render_pinned_section` (upstream zeron fd42e2ab…6851fc34, ported
 * local-only: NO registry sync). A collapsible disclosure ("Pinned" open,
 * "Pinned (N)" collapsed) whose body and divider ride the shared disclosure
 * tween; the locally ordered pinned rows sit above the divider, reorderable
 * by dragging a row between slots: siblings slide one slot toward the
 * vacated space on the tab-slide tween and the dragged row rides the
 * pointer's slot. Dragging a row BELOW the section previews a transfer out
 * (6851fc34): the siblings settle home and releasing unpins — the FLIP
 * resort glide carries the row to its live activity position. A drop inside
 * commits the reordered pins to device-local settings; a drag that leaves
 * the sidebar's column (or Escape) cancels, and a commit lands without a
 * resort glide — the rows are already visually in place.
 *
 * The parent renders and keys the rows (so the list-wide FLIP diff still
 * reaches them); this component wraps each in the drag-offset box and owns
 * the disclosure, the gesture, the edge autoscroll, and the commit.
 */

/** A pointer must travel this far before the press reads as a drag. */
const DRAG_ARM_PX = 4;

interface PinDrag {
  readonly chatId: string;
  readonly from: number;
  readonly over: number;
  /**
   * The pointer left the section downward — a transfer out, not a reorder:
   * siblings sit at their natural slots and the release unpins
   * (`SidebarSessionDrop::Regular`).
   */
  readonly overRegular: boolean;
  /** The visible pins at arm time — the drag dies with any of them. */
  readonly snapshotIds: readonly string[];
}

export function PinnedSection({
  rows,
  items,
  open,
  hasDivider,
  onToggle,
  onCommit,
  onTransferOut,
  sectionRef,
}: {
  /** The visible pinned rows, in display order. */
  readonly rows: readonly ChatRow[];
  /** The parent's keyed element per row, aligned with `rows`. */
  readonly items: readonly React.ReactNode[];
  /** `Shell::pinned_open` — the disclosure's live state (the parent's store). */
  readonly open: boolean;
  /** The hairline divider renders only when regular rows follow. */
  readonly hasDivider: boolean;
  /** The header's flip: the parent owns the in-memory open flag. */
  readonly onToggle: () => void;
  /**
   * A drop's commit: the from/to slots in the CURRENT visible order — the
   * parent owns the saved-order math (`commit_pinned_session_drag`), since
   * pins are bucketed per workspace profile.
   */
  readonly onCommit: (from: number, to: number) => void;
  /**
   * A release below the section: the parent resolves the target
   * (`finish_sidebar_session_transfer`) — a custom section under the
   * pointer claims it (Section arm: unpin + assign); anywhere else is the
   * Regular arm (unpin, membership clears).
   */
  readonly onTransferOut: (chatId: string, pointer: { clientX: number; clientY: number }) => void;
  /** The parent's handle on the section root (its own transfer gesture reads bounds). */
  readonly sectionRef: React.RefObject<HTMLElement | null>;
}) {
  const groupRef = useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = useState<PinDrag | null>(null);
  // The window-level handlers outlive the render; they read the live state
  // through refs (the tab strip's pattern — never a state-updater read).
  const dragRef = useRef<PinDrag | null>(null);
  const setDragState = (next: PinDrag | null): void => {
    dragRef.current = next;
    setDrag(next);
  };
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const onCommitRef = useRef(onCommit);
  onCommitRef.current = onCommit;
  const onTransferOutRef = useRef(onTransferOut);
  onTransferOutRef.current = onTransferOut;
  const pointerYRef = useRef<number | null>(null);
  const teardownRef = useRef<(() => void) | null>(null);
  // A completed drag suppresses the click its pointerup would fire on the row.
  const suppressClickRef = useRef(false);

  const count = rows.length;
  const bodyHeight = pinnedSectionBodyHeight(
    rows.map((row) => chatRowHeight(row.branch !== null, row.changeRequest !== null)),
  );
  const { bodyRef, chevronRef, toggle } = useSidebarDisclosure("pinned", open, bodyHeight);
  const { dividerRef } = useSidebarDisclosureDivider(
    "pinned",
    open,
    SIDEBAR_PINNED_DIVIDER_FRAME_HEIGHT,
    bodyHeight,
  );
  // A collapsed section hides its rows (the body clips at height 0), so no
  // press inside it can arm a drag (`render_active_rows`'s pinned_open gate).
  // A SINGLE pin is draggable now: the press can leave the section and
  // transfer out (6851fc34 removed the ≥2 gate).
  const draggable = open && count > 0;

  // A teardown outliving its gesture (unmount mid-drag — the space filter
  // flipped) is a cancel, exactly like `set_space_filter`'s guard.
  useEffect(
    () => () => {
      teardownRef.current?.();
    },
    [],
  );

  // While a drag is armed, the pointer's edge proximity scrolls the sidebar
  // (`start_pinned_session_autoscroll`: a frame-timed loop, bound to the drag).
  useEffect(() => {
    if (drag === null) {
      return;
    }
    const scroller = groupRef.current?.closest(".sidebar-list");
    if (scroller == null) {
      return;
    }
    const timer = window.setInterval(() => {
      const live = dragRef.current;
      const pointerY = pointerYRef.current;
      if (live === null || pointerY === null) {
        return;
      }
      const rect = scroller.getBoundingClientRect();
      const delta = pinnedDragScrollDelta(pointerY, rect.top, rect.bottom);
      const max = Math.max(scroller.scrollHeight - scroller.clientHeight, 0);
      const next = pinnedDragScrollStep(true, 0, 0, scroller.scrollTop, max, delta);
      if (next === null) {
        return;
      }
      scroller.scrollTop = next;
      const group = groupRef.current?.getBoundingClientRect();
      if (group !== undefined) {
        const over = pinnedSessionDropIndex(pointerY - group.top, rowsRef.current.length);
        if (over !== null && over !== live.over) {
          setDragState({ ...live, over });
        }
      }
    }, SIDEBAR_DRAG_SCROLL_FRAME_MS);
    return () => window.clearInterval(timer);
  }, [drag]);

  function armDrag(event: React.PointerEvent, chatId: string, from: number): void {
    if (!draggable || event.button !== 0) {
      return;
    }
    // Interactive corners (the Archive pill) own their press.
    if ((event.target as HTMLElement).closest("button") !== null) {
      return;
    }
    const startX = event.clientX;
    const startY = event.clientY;
    const snapshotIds = rowsRef.current.map((row) => row.chat.id);
    let moved = false;
    const onMove = (move: PointerEvent): void => {
      const group = groupRef.current;
      if (group === null) {
        return;
      }
      // `contain_pinned_session_drag`: leaving the sidebar's column cancels.
      const bounds = group.getBoundingClientRect();
      if (move.clientX < bounds.left || move.clientX > bounds.right) {
        cancel();
        return;
      }
      if (!moved) {
        if (Math.abs(move.clientX - startX) <= DRAG_ARM_PX && Math.abs(move.clientY - startY) <= DRAG_ARM_PX) {
          return;
        }
        moved = true;
        // Arm in its own commit so the slide transition is already live when
        // the first retarget lands; commit/cancel drops class and transform
        // together, snapping instantly like the desktop's state teardown.
        setDragState({ chatId, from, over: from, overRegular: false, snapshotIds });
        return;
      }
      // Below the section the drag previews a TRANSFER OUT (6851fc34): the
      // siblings sit at their natural slots and the release unpins — the
      // FLIP resort glide carries the row to its live activity position.
      // Only an in-section pointer retargets and feeds the edge autoscroll.
      const relY = move.clientY - bounds.top;
      if (move.clientY > bounds.bottom) {
        const current = dragRef.current;
        if (current !== null && !current.overRegular) {
          setDragState({ ...current, overRegular: true, over: current.from });
        }
        pointerYRef.current = null;
        return;
      }
      const over = pinnedSessionClampedIndex(relY, rowsRef.current.length);
      if (over === null) {
        return;
      }
      const current = dragRef.current;
      if (current !== null && (current.over !== over || current.overRegular)) {
        setDragState({ ...current, over, overRegular: false });
      }
      pointerYRef.current = pinnedSessionDropIndex(relY, rowsRef.current.length) === null ? null : move.clientY;
    };
    const teardown = (): void => {
      teardownRef.current = null;
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", onKey, true);
      pointerYRef.current = null;
    };
    teardownRef.current = teardown;
    const finish = (up: PointerEvent): void => {
      teardown();
      const current = dragRef.current;
      setDragState(null);
      // `finish_sidebar_session_transfer`: below the section the release is
      // a transfer out — the parent hit-tests the release point for a
      // custom section (Section arm) and falls back to the regular unpin.
      // Inside, a no-op move writes nothing; a drag whose snapshot pins did
      // not all survive cancels.
      if (current !== null) {
        if (current.overRegular) {
          const visible = rowsRef.current.map((row) => row.chat.id);
          const stillValid =
            current.snapshotIds.includes(current.chatId) &&
            current.snapshotIds.every((id) => visible.includes(id));
          if (stillValid) {
            onTransferOutRef.current(current.chatId, {
              clientX: up.clientX,
              clientY: up.clientY,
            });
          }
        } else if (current.from !== current.over) {
          const visible = rowsRef.current.map((row) => row.chat.id);
          const stillValid =
            current.snapshotIds.includes(current.chatId) &&
            current.snapshotIds.every((id) => visible.includes(id));
          if (stillValid) {
            onCommitRef.current(current.from, current.over);
          }
        }
      }
      if (moved) {
        suppressClickRef.current = true;
        window.setTimeout(() => {
          suppressClickRef.current = false;
        }, 0);
      }
    };
    const cancel = (): void => {
      teardown();
      setDragState(null);
    };
    const onKey = (key: KeyboardEvent): void => {
      if (key.key === "Escape") {
        key.preventDefault();
        key.stopPropagation();
        cancel();
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", onKey, true);
  }

  const draggedIndex = drag === null ? -1 : rows.findIndex((row) => row.chat.id === drag.chatId);
  return (
    <section
      className="sidebar-pinned-section"
      data-testid="sidebar-pinned-section"
      ref={sectionRef}
    >
      <SidebarDisclosureHeader
        id="pinned-toggle"
        label={open ? "Pinned" : `Pinned (${count})`}
        open={open}
        withRule={false}
        chevronRef={chevronRef}
        onToggle={() => {
          // The header click kills any live drag before the section moves —
          // the desktop's toggle leads with `cancel_pinned_session_drag`.
          teardownRef.current?.();
          setDragState(null);
          // The motion begins on the CURRENT height before the flip; the
          // parent bumps the resort reset epoch so the rows below adopt the
          // new order without a second (FLIP) animation of this movement.
          toggle();
          onToggle();
        }}
      />
      <SidebarDisclosureBody bodyRef={bodyRef}>
        <div className="sidebar-pinned-pad">
          <div className="sidebar-pinned" ref={groupRef} data-testid="sidebar-pinned-sessions">
            {rows.map((row, index) => {
              const offset =
                drag === null || draggedIndex < 0 || drag.overRegular
                  ? 0
                  : index === draggedIndex
                    ? (drag.over - drag.from) * SIDEBAR_SESSION_SLOT
                    : slideOffset(index, drag.from, drag.over) * SIDEBAR_SESSION_SLOT;
              return (
                <div
                  key={row.chat.id}
                  className="pinned-row"
                  data-dragging={drag !== null ? "1" : undefined}
                  data-transfer={drag?.overRegular ? "1" : undefined}
                  style={offset === 0 ? undefined : { transform: `translateY(${offset}px)` }}
                  onPointerDown={(event) => armDrag(event, row.chat.id, index)}
                  // The row's anchor is natively draggable; a press that
                  // moves must stay OUR drag (see sidebar-drag-events.ts).
                  onDragStart={preventNativeSidebarRowDrag}
                  onClickCapture={(event) => {
                    if (suppressClickRef.current) {
                      event.preventDefault();
                      event.stopPropagation();
                    }
                  }}
                >
                  {items[index]}
                </div>
              );
            })}
          </div>
        </div>
      </SidebarDisclosureBody>
      {hasDivider ? (
        <div
          ref={dividerRef}
          className="sidebar-pinned-divider-frame"
          data-testid={SIDEBAR_PINNED_DIVIDER_KEY}
        >
          <div className="sidebar-pinned-divider">
            <div className="sidebar-pinned-divider-rule" />
          </div>
        </div>
      ) : null}
    </section>
  );
}
