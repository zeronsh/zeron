/**
 * The nested-flyout side probe — the ONE shared shape behind the desktop's
 * `setting_on_left` / view-submenu canvas probes (pickers.rs:3969,
 * spaces.rs:2412): a flyout opens LEFT when its reach past the anchor's
 * right edge would cross the window's right edge. Each caller keeps its
 * own reach constant (the card span + offset it actually pins) and its
 * own desktop citation; this module owns only the probe shape, so the
 * per-caller spellings can never drift apart.
 */

/**
 * Reads the anchor's live bounds: `true` when the flyout should open on
 * the LEFT — `rect.right + reach` crossing `window.innerWidth` leaves no
 * room to the right. The comparison is strictly greater, so a reach that
 * exactly meets the edge still opens right.
 */
export function flyoutOpensLeft(rect: DOMRect, reach: number): boolean {
  return rect.right + reach > window.innerWidth;
}
