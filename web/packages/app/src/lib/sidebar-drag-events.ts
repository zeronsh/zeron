import type { DragEvent } from "react";

/**
 * The web half of `contain_pinned_session_drag` the desktop never needed:
 * every sidebar row is an `<a>` (the chat row's Link), and anchors — like
 * images — are draggable by default under HTML drag-and-drop. A left press
 * that moves a few pixels therefore starts the BROWSER's drag, not the
 * row's own gesture: the user agent fires `pointercancel` "immediately
 * before drag operation starts" for the pointer that caused it (Pointer
 * Events §4.2.7), the window-level drag handlers obey it as a cancel, and
 * the reorder/transfer dies before any preview or commit — the reported
 * "the web version doesn't have the sorting".
 *
 * The same spec section names the remedy: "If the start of the drag
 * operation is prevented through any means (e.g. through calling
 * preventDefault on the dragstart event) there will be no pointercancel
 * event." Cancelling the bubbled `dragstart` on the wrapper that armed the
 * drag keeps the pointer stream ours — pointermove/pointerup keep flowing
 * to the window listeners and the gesture runs to its commit.
 *
 * This must be a STATIC handler on the drag-armed wrapper, registered
 * before the press: the browser's drag threshold is unspecified and can
 * fire `dragstart` before (or after) the gesture's own 4px arm, so the
 * guard cannot ride the armed state. The wrapper is also the right seam
 * semantically — a press on a row that moves is the app's drag, never the
 * browser's link-drag (the desktop's rows are commands, not draggable
 * links; dropping a row URL somewhere is not a Zeron affordance).
 */
export function preventNativeSidebarRowDrag(event: DragEvent<HTMLElement>): void {
  event.preventDefault();
}
