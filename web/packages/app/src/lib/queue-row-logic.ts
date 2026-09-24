/**
 * The queue row's pure logic — the web port of `crates/ui/src/queue.rs`'s
 * helper block (`one_line`, `queue_visible_text`, `queue_attachment_labels`,
 * `available_queue_primary_action`, `queue_latest_shortcut_visible`,
 * `queue_drop_index`, `queue_drag_offsets`, `visible_queue_rows`) plus
 * `queue_preview_limit` (composer.rs:4392) and the terminal panel's
 * `drop_index`/`slide_offset` (terminal/panel.rs:92-109) that the drag math
 * builds on. Decision logic only — the RPCs live in `./queue-actions.ts`
 * and the row rendering in `../components/queue-panel.tsx`.
 *
 * Appshots are desktop-only (web parity decision 6), so the Appshot halves
 * of `queue_visible_text` / `queue_attachment_labels` (context-marker
 * stripping, `{app} Appshot` labels) are deliberately not ported: every
 * web attachment label is the bare filename.
 */

import { ATTACHMENT_ONLY_TEXT, parseUserMessageImages } from "./attachments";
import { COMPOSER_MAX_WIDTH } from "./composer-flip";

/** `ROW_HEIGHT` (queue.rs:75). */
export const QUEUE_ROW_HEIGHT = 36;
/** `ROW_GAP` (queue.rs:77) — rows are flush; only a hover wash separates them. */
export const QUEUE_ROW_GAP = 0;
/** `ROW_SLOT` (queue.rs:78) — one row's slot in the drag math. */
export const QUEUE_ROW_SLOT = QUEUE_ROW_HEIGHT + QUEUE_ROW_GAP;
/** `PANEL_PAD_TOP` (queue.rs:82) — zero; `panel_y` is already list-relative. */
export const QUEUE_PANEL_PAD_TOP = 0;

/**
 * `one_line` (queue.rs:174): collapse a multi-line message to one visual
 * line — every newline and whitespace run becomes a single space. (The
 * pre-rewrite web panel called this `collapseWhitespace`; the desktop name
 * wins for greppability.)
 */
export function oneLine(text: string): string {
  return text.split(/\s+/).filter((part) => part.length > 0).join(" ");
}

/**
 * `queue_visible_text` (queue.rs:182-205), minus the Appshot halves: hides
 * the legacy attachment-refs trailer ONLY when its parsed paths exactly
 * match the row's `attachments` field (protects against a rolling-upgrade
 * mismatch), and falls back to `ATTACHMENT_ONLY_TEXT` when the remaining
 * text is empty but attachments exist.
 */
export function queueVisibleText(text: string, attachments: readonly string[]): string {
  if (text.trim().length === 0 && attachments.length > 0) {
    return ATTACHMENT_ONLY_TEXT;
  }
  if (attachments.length === 0) {
    return text;
  }
  const parsed = parseUserMessageImages(text);
  const pathsMatch =
    parsed.attachments.length === attachments.length &&
    parsed.attachments.every((parsedAttachment, ix) => parsedAttachment.path === attachments[ix]);
  if (!pathsMatch) {
    return text;
  }
  if (parsed.text.trim().length === 0) {
    return ATTACHMENT_ONLY_TEXT;
  }
  return parsed.text;
}

/** `Path::file_name` — the path's final component, or `"Image"` when it has none. */
function fileName(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
  return parts[parts.length - 1] ?? "Image";
}

/**
 * `queue_attachment_labels` (queue.rs:208-221), Appshot half dropped: each
 * path becomes its bare filename.
 */
export function queueAttachmentLabels(paths: readonly string[]): string[] {
  return paths.map(fileName);
}

/**
 * The row's second line (`queue_row`'s `summary`): `"{N} attachments · "`
 * prefixed when N>1, labels joined with `" · "`.
 */
export function queueAttachmentSummary(labels: readonly string[]): string {
  if (labels.length > 1) {
    return `${labels.length} attachments · ${labels.join(" · ")}`;
  }
  return labels.join(" · ");
}

/** `QueuePrimaryAction` (queue.rs:85-102) — the row's single primary action. */
export type QueuePrimaryAction = "sendNow";

/**
 * `available_queue_primary_action` (queue.rs:103-108): `Some` iff the row is
 * not delivery-blocked and the host supports queue actions. All providers
 * use Send now.
 */
export function availableQueuePrimaryAction(
  deliveryBlocked: boolean,
  hostSupportsActions: boolean,
): QueuePrimaryAction | null {
  return !deliveryBlocked && hostSupportsActions ? "sendNow" : null;
}

/**
 * `queue_latest_shortcut_visible` (queue.rs:110-117): only the LAST row, only
 * when the reveal is requested and the action is available. `count === 0`
 * can never match (no out-of-range index).
 */
export function queueLatestShortcutVisible(
  index: number,
  count: number,
  revealRequested: boolean,
  actionAvailable: boolean,
): boolean {
  return index + 1 === count && revealRequested && actionAvailable;
}

/**
 * `drop_index` (terminal/panel.rs:92): quantize a pointer coordinate to a
 * slot, clamped to `0..count-1`. The top pad belongs to slot zero; the
 * bottom pad clamps to the final row.
 */
function dropIndex(relY: number, slot: number, count: number): number {
  if (count === 0 || slot <= 0) {
    return 0;
  }
  return Math.min(Math.max(Math.floor(relY / slot), 0), count - 1);
}

/**
 * `queue_drop_index` (queue.rs:125-127): translate a pointer inside the
 * panel into a row slot. `PANEL_PAD_TOP` is 0, so the subtraction is a
 * no-op kept for the desktop's shape.
 */
export function queueDropIndex(panelY: number, count: number): number {
  return dropIndex(panelY - QUEUE_PANEL_PAD_TOP, QUEUE_ROW_SLOT, count);
}

/**
 * `slide_offset` (terminal/panel.rs:101): the slot-unit displacement of row
 * `ix` while `from` is dragged over `over` — rows between the two shift one
 * slot toward the vacated gap.
 */
export function slideOffset(ix: number, from: number, over: number): number {
  if (from < over && ix > from && ix <= over) {
    return -1;
  }
  if (over < from && ix >= over && ix < from) {
    return 1;
  }
  return 0;
}

/**
 * `queue_drag_offsets` (queue.rs:132-144): the paint-only start and target
 * positions for the reorder slide. The dragged row travels
 * `(prev_over→over − from)` slots; every displaced row slides
 * `slide_offset` slots into the space it leaves behind.
 */
export function queueDragOffsets(
  ix: number,
  from: number,
  prevOver: number,
  over: number,
): [number, number] {
  if (ix === from) {
    return [(prevOver - from) * QUEUE_ROW_SLOT, (over - from) * QUEUE_ROW_SLOT];
  }
  return [
    slideOffset(ix, from, prevOver) * QUEUE_ROW_SLOT,
    slideOffset(ix, from, over) * QUEUE_ROW_SLOT,
  ];
}

/**
 * `visible_queue_rows` (queue.rs:274-278): the row-index range whose
 * thumbnails are worth decoding, given the list's scroll offset and
 * viewport height.
 */
export function visibleQueueRows(offset: number, height: number, count: number): [number, number] {
  const first = Math.min(Math.floor(Math.max(-offset, 0) / QUEUE_ROW_SLOT), count);
  const last = Math.min(
    first + Math.ceil(Math.max(height, 0) / QUEUE_ROW_SLOT) + 1,
    count,
  );
  return [first, last];
}

/**
 * `queue_preview_limit` (composer.rs:4392-4398): **1 preview when the
 * composer's available width is below 520px, else 2** — measured width
 * clamped to the composer column, `COMPOSER_MAX_WIDTH` when unmeasured.
 */
export function queuePreviewLimit(composerWidth: number | null): number {
  return (composerWidth ?? COMPOSER_MAX_WIDTH) < 520 ? 1 : 2;
}

/**
 * `modifier_send_label` (settings/shortcuts.rs:407): the last row's
 * primary button shows the platform combo while the modifier is held.
 */
export function modifierSendLabel(isMac: boolean): string {
  return isMac ? "⌘ Enter" : "Ctrl Enter";
}

/** The compact key-cap form (queue.rs:1013-1032's `⌘↵`/`⌃↵`). */
export function modifierSendCompactLabel(isMac: boolean): string {
  return isMac ? "⌘↵" : "⌃↵";
}
