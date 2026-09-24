/**
 * The new-chat model picker's measured geometry (pickers.rs:3235-3252,
 * 4683-4703, 4704-4714 — upstream e0c1e936, ported as 9b5a70b6): on the
 * new-chat canvas the card opens BELOW the chip and the model list band
 * sizes to the room actually available below the composer, clamped 30–216.
 * Everywhere else the band is the fixed 216 and the card opens above-end.
 * Pure math only, so the placement/height computation stays unit-testable
 * in the node vitest environment.
 */

import type { AnchorHelperId } from "../components/base/positioning";

/**
 * `LIST_HEIGHT` (pickers.rs:3143) — the model list band's resting height,
 * the in-chat value and the clamp ceiling.
 */
export const MODEL_LIST_HEIGHT = 216;
/**
 * `model_space_below.unwrap_or(640.0)` (pickers.rs:3249) — the unmeasured
 * stand-in; through the clamp it yields the full 216 band, so the card opens
 * at the resting height and re-sizes once the room is measured.
 */
export const MODEL_SPACE_FALLBACK = 640;
/**
 * The card's own chrome budget (pickers.rs:3249's `- 82.0`): the tab strip,
 * search row, and insets the card paints above the list band.
 */
export const MODEL_CARD_CHROME = 82;
/**
 * The traits tray's 236px cap (pickers.rs:3432) — the band budgets the whole
 * cap whenever the tray is present, so the list scrolls within the room
 * below while the tray stays visible (pickers.rs:3241-3248).
 */
export const MODEL_TRAY_CAP = 236;
/** The band's clamp floor (pickers.rs:3249). */
export const MODEL_BAND_MIN = 30;
/** The band's clamp ceiling — the resting `LIST_HEIGHT`. */
export const MODEL_BAND_MAX = MODEL_LIST_HEIGHT;
/**
 * The chip-bottom → viewport-bottom margin (pickers.rs:4696's `- 14.0`) —
 * the trigger gap plus the breathing room below the card.
 */
export const MODEL_SPACE_GAP = 14;

/**
 * `model_space_below` (pickers.rs:4686-4698): the room under the chip —
 * `viewport height − chip bottom − 14`, floored at zero.
 */
export function modelSpaceBelow(viewportHeight: number, anchorBottom: number): number {
  return Math.max(viewportHeight - anchorBottom - MODEL_SPACE_GAP, 0);
}

/**
 * The new-chat band (pickers.rs:3245-3252):
 * `(space_below − 82 − tray).clamp(30, 216)` — the tray budget is the traits
 * tray's full 236 cap when the chip has a ladder or options, else nothing.
 * An unmeasured room (`null`) falls back to 640, which clamps to the resting
 * 216.
 */
export function modelListBandHeight(spaceBelow: number | null, trayPresent: boolean): number {
  const room = spaceBelow ?? MODEL_SPACE_FALLBACK;
  const tray = trayPresent ? MODEL_TRAY_CAP : 0;
  return Math.min(Math.max(room - MODEL_CARD_CHROME - tray, MODEL_BAND_MIN), MODEL_BAND_MAX);
}

/**
 * The card's placement (pickers.rs:4704-4714): below-end on the new-chat
 * canvas (`anchored_menu_below_end`), above-end everywhere else.
 */
export function modelPickerPlacement(newChat: boolean): AnchorHelperId {
  return newChat ? "anchorBelowEnd" : "anchorAboveEnd";
}
