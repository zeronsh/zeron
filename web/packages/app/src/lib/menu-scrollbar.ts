/**
 * The floating menu scrollbar's visibility model — the web peer of the
 * desktop's `MenuScrollbarState` (`crates/ui/src/popover.rs`). Pure; the
 * React rail in `components/ui/Scrollbar.tsx` feeds it pointer/scroll events
 * and reads `visible`/`fade`/`nextWakeMs` back.
 *
 * Semantics (popover.rs:1286-1293):
 * - The rail paints while a drag or a track-hover holds it open, or while
 *   scroll motion is recent — scrolling shows it with or without hover;
 *   hovering the list alone shows nothing.
 * - When the motion stops the rail lingers `MENU_SCROLLBAR_LINGER_MS`, fades
 *   over `MENU_SCROLLBAR_FADE_MS`, and disappears — but hopping onto the
 *   track mid-linger freezes it open, and leaving the track restarts the
 *   wait (it never hides under the pointer).
 */

/** How long the rail stays fully visible after the last scroll motion. */
export const MENU_SCROLLBAR_LINGER_MS = 1400;
/** How long the rail takes to fade out after the linger window. */
export const MENU_SCROLLBAR_FADE_MS = 260;
/** Repaint cadence through the fade window. */
export const MENU_SCROLLBAR_FADE_FRAME_MS = 16;

export interface MenuScrollbarVisibility {
  listHovered: boolean;
  barHovered: boolean;
  grabbing: boolean;
  /** Last seen scroll position — a change marks fresh scroll activity. */
  lastScrollY: number | null;
  /** When that change was seen (ms timestamp, the caller's clock). */
  lastScrollAt: number | null;
}

export function createMenuScrollbarVisibility(): MenuScrollbarVisibility {
  return {
    listHovered: false,
    barHovered: false,
    grabbing: false,
    lastScrollY: null,
    lastScrollAt: null,
  };
}

/**
 * Record a scroll observation. The first observation establishes the
 * baseline only (a fresh mount always reports its initial offset, and that
 * is not scrolling); a later change marks motion at `now`. Returns whether
 * this observation marked fresh motion.
 */
export function noteScrollOffset(
  state: MenuScrollbarVisibility,
  position: number,
  now: number,
): boolean {
  if (state.lastScrollY === null) {
    state.lastScrollY = position;
    return false;
  }
  if (state.lastScrollY === position) {
    return false;
  }
  state.lastScrollY = position;
  state.lastScrollAt = now;
  return true;
}

/**
 * Forget the baseline: the next observation becomes the fresh baseline
 * without marking motion (popover.rs `clear_scroll_baseline` — used after a
 * programmatic jump like `scroll_to_item`).
 */
export function clearScrollBaseline(state: MenuScrollbarVisibility): void {
  state.lastScrollY = null;
  state.lastScrollAt = null;
}

/** Within the linger+fade window of the last motion. */
export function scrollCountdown(state: MenuScrollbarVisibility, now: number): boolean {
  return (
    state.lastScrollAt !== null &&
    now - state.lastScrollAt < MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS
  );
}

/**
 * Whether the rail paints at all: a drag or a track-hover holds it open,
 * otherwise recent scroll motion does (popover.rs `visible`).
 */
export function railVisible(state: MenuScrollbarVisibility, now: number): boolean {
  return state.grabbing || state.barHovered || scrollCountdown(state, now);
}

/**
 * 1 → 0 across the fade window once the scroll motion stops; full while a
 * drag or a track-hover holds the rail open (popover.rs `fade`).
 */
export function railFade(state: MenuScrollbarVisibility, now: number): number {
  if (state.grabbing || state.barHovered) {
    return 1;
  }
  if (state.lastScrollAt === null) {
    return 0;
  }
  const elapsed = now - state.lastScrollAt;
  const fade = 1 - (elapsed - MENU_SCROLLBAR_LINGER_MS) / MENU_SCROLLBAR_FADE_MS;
  return Math.min(1, Math.max(0, fade));
}

/**
 * The pointer entered/left the LIST. Leaving the host straight off the
 * track (no grab) restarts the linger so the rail doesn't vanish under a
 * departing pointer (popover.rs `set_list_hovered`).
 */
export function setListHovered(
  state: MenuScrollbarVisibility,
  hovered: boolean,
  now: number,
): void {
  if (state.listHovered === hovered) {
    return;
  }
  state.listHovered = hovered;
  if (!hovered && !state.grabbing && state.barHovered) {
    state.barHovered = false;
    state.lastScrollAt = now;
  }
}

/**
 * The pointer entered/left the RAIL. Keeps the active treatment while a
 * captured drag travels outside (the hover callback correctly reports false
 * there, so the effective state is `hovered || grabbing`). Leaving the track
 * restarts the wait — the rail hides only a beat later, not mid-hover
 * (popover.rs `set_bar_hovered`).
 */
export function setBarHovered(
  state: MenuScrollbarVisibility,
  hovered: boolean,
  now: number,
): void {
  const active = hovered || state.grabbing;
  if (state.barHovered === active) {
    return;
  }
  state.barHovered = active;
  if (!active) {
    state.lastScrollAt = now;
  }
}

/**
 * Engage/drop the drag (popover.rs `begin_press`/`end_press`). Releasing a
 * drag lingers like a stopped scroll; releasing it while the list is no
 * longer hovered drops the track hover with it.
 */
export function setGrabbing(
  state: MenuScrollbarVisibility,
  grabbing: boolean,
  now: number,
): void {
  if (state.grabbing === grabbing) {
    return;
  }
  state.grabbing = grabbing;
  if (!grabbing) {
    state.lastScrollAt = now;
    if (!state.listHovered && state.barHovered) {
      state.barHovered = false;
    }
  }
}

/** Whether the thumb carries the expanded/stronger treatment. */
export function railActive(state: MenuScrollbarVisibility): boolean {
  return state.barHovered || state.grabbing;
}

/**
 * When the countdown next needs a repaint, as a delay from `now`: the rest
 * of the linger (the wake lands as the fade starts), then frame steps
 * through the fade so intermediate fade values actually get painted. `null`
 * when nothing is winding down or the window has fully elapsed (popover.rs
 * `next_wake_at`).
 */
export function nextWakeMs(state: MenuScrollbarVisibility, now: number): number | null {
  if (state.grabbing || state.barHovered || !scrollCountdown(state, now)) {
    return null;
  }
  if (state.lastScrollAt === null) {
    return null;
  }
  const elapsed = now - state.lastScrollAt;
  if (elapsed < MENU_SCROLLBAR_LINGER_MS) {
    return MENU_SCROLLBAR_LINGER_MS - elapsed;
  }
  if (elapsed < MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS) {
    return MENU_SCROLLBAR_FADE_FRAME_MS;
  }
  return null;
}
