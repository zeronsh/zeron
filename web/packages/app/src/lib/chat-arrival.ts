/**
 * The chat-switch arrival window (ticket 58) — the ONE predicate every
 * arrival gate consults. The desktop's switch is ATOMIC: `select_chat`
 * re-derives the rows and applies the restored viewport in a single frame
 * (crates/ui/src/state.rs:1740-1792), the pane tweens snap on the chat-key
 * change (shell.rs:1837-1862), and the composer's ROUTE_SNAP kills the
 * morph for a session/route change (composer.rs:5849-5874) — a switch
 * renders its destination state; motion belongs to live streams.
 *
 * The web's switch is a REMOUNT (`key={active.docId}`, transcript.tsx): the
 * virtualizer rebuilds its height model from estimates and rows measure over
 * the following frames (the settle cascade). This window covers exactly that
 * cascade so the three arrival gates can suppress it:
 *
 * - the scroller: the restored offset is a hard assignment, and the
 *   per-commit kicks write the end directly instead of arming the spring
 *   (no "scrolling down" glide);
 * - the tool groups: the `noteRendered` rendered-open flip records its
 *   endpoint without seeding a fold tween (no "closing the group tabs");
 * - the shimmer: `sync` does not arm `shimmerStartedAt` (no shimmer restart
 *   on the first paint of an already-loaded transcript).
 *
 * Armed at the surface's first loaded commit (the remount IS the chat
 * change — the outlet hands the view the new store only once its first
 * frame has landed), refreshed by every measurement batch, cleared once the
 * batches quiesce, hard-capped so live streaming always re-owns the motion.
 * Pure by design (a `now` parameter everywhere), so the unit tests drive
 * the timeline directly.
 */

/** Measurement quiesce: ~2 frames with no new heights closes the window. */
export const ARRIVAL_QUIESCE_MS = 50;

/** The settle cascade's hard upper bound — live motion re-owns after this. */
export const ARRIVAL_HARD_CAP_MS = 500;

export class ChatArrivalWindow {
  #armedAt: number | null = null;
  #lastMeasureAt: number | null = null;

  /**
   * The chat switch lands: arm the window. The surface arms this exactly
   * once per mount, at the first loaded commit.
   */
  arm(now: number): void {
    this.#armedAt = now;
    this.#lastMeasureAt = null;
  }

  /**
   * A measurement batch landed (a ResizeObserver bump while armed): the
   * settle cascade is still running, so the window extends to cover the
   * corrections that follow it.
   */
  noteMeasure(now: number): void {
    if (this.#armedAt === null) {
      return;
    }
    this.#lastMeasureAt = now;
  }

  /**
   * The ONE predicate. True while this surface is inside a chat-switch
   * arrival: armed, before the hard cap, and either still waiting for the
   * first measurement or within the quiesce window of the last one. False
   * for every same-chat frame (the ordinary live case).
   */
  isArrival(now: number): boolean {
    const armedAt = this.#armedAt;
    if (armedAt === null || now < armedAt) {
      return false;
    }
    if (now - armedAt > ARRIVAL_HARD_CAP_MS) {
      return false;
    }
    const lastMeasureAt = this.#lastMeasureAt;
    return lastMeasureAt === null || now - lastMeasureAt <= ARRIVAL_QUIESCE_MS;
  }
}
