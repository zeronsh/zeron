/**
 * Pinned sidebar sessions — the web peer of the desktop's pin machinery
 * (`shell.rs` + `shell/spaces.rs`, upstream zeron fd42e2ab…da041aab, ported
 * local-only: NO registry sync). Pins are a device-local, presentation-only
 * preference persisted in `ui-settings.ts`, bucketed per workspace profile
 * (3cad1c25); the pure projection, reorder, cleanup, and drag-geometry rules
 * live here so the components stay thin and the unit tests mirror the Rust
 * `pinned_session_tests` one-for-one.
 */
import type { WorkspaceScope } from "@zeron/proto";

/**
 * `settings.rs::sidebar_pin_profile_key` — the bucket one workspace
 * profile's pins live under. Zeron has no account sign-in, so a non-local
 * scope keys on the engine's device id; null means "identity not ready"
 * (callers skip destructive cleanup against it). Every zeron engine is
 * local-scoped in practice, so the fleet usually shares the one "local"
 * bucket — scoped chat ids keep engines apart inside it.
 */
export function sidebarPinProfileKey(
  scope: WorkspaceScope | null,
  deviceId: string | null,
): string | null {
  if (scope === null) {
    return null;
  }
  if (scope === "local") {
    return "local";
  }
  if (deviceId === null) {
    return null;
  }
  return `${scope}:${deviceId}`;
}

/** `shell.rs::SIDEBAR_LIST_GAP` — the flex gap between sidebar rows. */
export const SIDEBAR_LIST_GAP = 2;
/**
 * `shell.rs::SIDEBAR_SESSION_SLOT` — the pinned drag's quantization unit
 * (61px branch-row height + gap).
 */
export const SIDEBAR_SESSION_SLOT = 61 + SIDEBAR_LIST_GAP;
/** `shell.rs::SIDEBAR_DRAG_SCROLL_*` — edge autoscroll geometry. */
export const SIDEBAR_DRAG_SCROLL_BAND = 48;
export const SIDEBAR_DRAG_SCROLL_MAX = 12;
export const SIDEBAR_DRAG_SCROLL_FRAME_MS = 16;
/** The scroll region's top padding (`SIDEBAR_LIST_PAD_TOP`). */
export const SIDEBAR_LIST_PAD_TOP = 4;
/** `shell.rs::SIDEBAR_PINNED_DIVIDER_*` — the hairline box between sections. */
export const SIDEBAR_PINNED_DIVIDER_HEIGHT = 13;
export const SIDEBAR_PINNED_DIVIDER_KEY = "sidebar-pinned-divider";
/**
 * `render_pinned_divider`'s animated frame: the hairline box plus its 2px
 * top gap, both clipped by the frame so neither survives a collapse.
 */
export const SIDEBAR_PINNED_DIVIDER_FRAME_HEIGHT =
  SIDEBAR_PINNED_DIVIDER_HEIGHT + SIDEBAR_LIST_GAP;
/** `spaces.rs::SIDEBAR_DISCLOSURE_HEADER_HEIGHT` — the section header row. */
export const SIDEBAR_DISCLOSURE_HEADER_HEIGHT = 28;
/** `spaces.rs::SIDEBAR_DISCLOSURE_BODY_INSET` — the body's handoff padding. */
export const SIDEBAR_DISCLOSURE_BODY_INSET = 4;
/** The pinned disclosure's keyed header entry (the FLIP diff's phantom). */
export const SIDEBAR_PINNED_HEADER_KEY = "sidebar-pinned-header";

/**
 * `shell.rs`'s `pinned_body_height`: inset + rows + intra-list gaps — the
 * height the disclosure tween collapses to 0. Pure.
 */
export function pinnedSectionBodyHeight(rowHeights: readonly number[]): number {
  let total = SIDEBAR_DISCLOSURE_BODY_INSET;
  for (const height of rowHeights) {
    total += height;
  }
  return total + SIDEBAR_LIST_GAP * Math.max(rowHeights.length - 1, 0);
}

/**
 * The header's keyed height for the FLIP diff (`render_chat_sidebar`'s order
 * vec): the header row, plus the body inset minus one list gap while open —
 * the phantom entry that keeps the rows below accounting for the section.
 * Pure.
 */
export function pinnedHeaderKeyedHeight(open: boolean): number {
  return (
    SIDEBAR_DISCLOSURE_HEADER_HEIGHT +
    (open ? SIDEBAR_DISCLOSURE_BODY_INSET - SIDEBAR_LIST_GAP : 0)
  );
}

/**
 * `spaces.rs::project_pinned_first` — promote the locally ordered pins above
 * the untouched activity projection. Every unpinned id keeps exactly the
 * relative order supplied by recency.
 */
export function projectPinnedFirst(recencyIds: readonly string[], pinnedIds: readonly string[]): string[] {
  const active = new Set(recencyIds);
  const pinned = new Set(pinnedIds);
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of pinnedIds) {
    if (active.has(id) && !seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  }
  for (const id of recencyIds) {
    if (!pinned.has(id) && !seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  }
  return out;
}

/**
 * `spaces.rs::reorder_visible_pins` (68306a17's per-item moves): move ONLY
 * the dragged pin — every other pin, including hidden/archived pins, keeps
 * its relative order; no other position is written.
 */
export function reorderVisiblePins(
  pinnedIds: readonly string[],
  visibleIds: readonly string[],
  from: number,
  to: number,
): string[] {
  if (from >= visibleIds.length || to >= visibleIds.length || from === to) {
    return [...pinnedIds];
  }
  const moved = visibleIds[from]!;
  const anchor = visibleIds[to]!;
  const result = pinnedIds.filter((id) => id !== moved);
  const index = result.indexOf(anchor);
  if (index < 0) {
    return [...pinnedIds];
  }
  result.splice(index + (from < to ? 1 : 0), 0, moved);
  return result;
}

/**
 * `shell/sidebar_pins.rs::SidebarPinChange` — one per-item pin intent,
 * anchored to neighbor session ids. Upstream fed these to the engine's
 * registry op; zeron's web projects them onto the ui-local store, so the
 * type never crosses a wire.
 */
export type SidebarPinChange =
  | { readonly action: "pin"; readonly sessionId: string; readonly after: string | null; readonly before: string | null }
  | { readonly action: "move"; readonly sessionId: string; readonly after: string | null; readonly before: string | null }
  | { readonly action: "unpin"; readonly sessionId: string };

/**
 * `SidebarPinChange::project` — rebase an intent on the latest committed
 * projection. A stale move never resurrects an unpinned item. A surviving
 * right anchor wins; otherwise use the left anchor, or append when both
 * disappeared.
 */
export function projectSidebarPinChange(ids: readonly string[], change: SidebarPinChange): string[] {
  if (change.action === "move" && !ids.includes(change.sessionId)) {
    return [...ids];
  }
  const next = ids.filter((id) => id !== change.sessionId);
  if (change.action === "unpin") {
    return next;
  }
  const beforeIndex = change.before === null ? -1 : next.indexOf(change.before);
  const afterIndex = change.after === null ? -1 : next.indexOf(change.after);
  const index =
    beforeIndex >= 0 ? beforeIndex : afterIndex >= 0 ? afterIndex + 1 : next.length;
  next.splice(index, 0, change.sessionId);
  return next;
}

/**
 * `finish_sidebar_session_transfer`'s intent construction: the drop's next
 * list becomes ONE per-item change — a Move for an existing pin, a Pin for
 * a new one, an Unpin when the id is absent — anchored to the drop's
 * neighbors.
 */
export function sidebarSessionDropChange(
  saved: readonly string[],
  next: readonly string[],
  chatId: string,
): SidebarPinChange {
  const index = next.indexOf(chatId);
  if (index < 0) {
    return { action: "unpin", sessionId: chatId };
  }
  const after = index > 0 ? (next[index - 1] ?? null) : null;
  const before = next[index + 1] ?? null;
  return {
    action: saved.includes(chatId) ? "move" : "pin",
    sessionId: chatId,
    after,
    before,
  };
}

/**
 * `commit_pinned_session_drag` over the fleet: reorder the visible pins in
 * the merged projection (hidden pins hold their slots, exactly like the
 * desktop's single-bucket `reorder_visible_pins`), then settle every id back
 * into its own profile's bucket. Membership never crosses buckets — a drag
 * between two engines' pins reads as bucket blocks on the next projection.
 */
export function commitVisiblePinReorder(
  buckets: Readonly<Record<string, readonly string[]>>,
  visibleIds: readonly string[],
  from: number,
  to: number,
): Record<string, string[]> {
  const merged = Object.values(buckets).flat();
  const next = reorderVisiblePins(merged, visibleIds, from, to);
  const out: Record<string, string[]> = {};
  for (const [key, ids] of Object.entries(buckets)) {
    const members = new Set(ids);
    out[key] = next.filter((id) => members.has(id));
  }
  return out;
}

/**
 * `spaces.rs::retain_known_pins` — remove only ids absent from the workspace.
 * Archived sessions remain known so unarchiving restores their local pin and
 * position. Returns true when the list changed.
 */
export function retainKnownPins(pinnedIds: string[], knownChatIds: ReadonlySet<string>): string[] | null {
  const seen = new Set<string>();
  const next = pinnedIds.filter((id) => {
    if (!knownChatIds.has(id) || seen.has(id)) {
      return false;
    }
    seen.add(id);
    return true;
  });
  return next.length === pinnedIds.length ? null : next;
}

/** `spaces.rs::SidebarSessionDrop` — where a sidebar drag landed. */
export type SidebarSessionDrop =
  | { readonly kind: "pinned"; readonly index: number }
  | { readonly kind: "regular" };

/**
 * `spaces.rs::sidebar_session_drop_pins` — a drop can change pin membership
 * or pinned order, never activity ordering. Unpinning removes the id; a
 * pinned drop reorders an existing pin, leaves a saved-but-hidden id alone,
 * or inserts a new pin before the visible anchor at the drop index (falling
 * back to after the last visible anchor, then the end).
 */
export function sidebarSessionDropPins(
  saved: readonly string[],
  visible: readonly string[],
  chatId: string,
  target: SidebarSessionDrop,
): string[] {
  if (target.kind === "regular") {
    return saved.filter((id) => id !== chatId);
  }
  const index = target.index;
  const from = visible.indexOf(chatId);
  if (from >= 0) {
    return reorderVisiblePins(saved, visible, from, Math.min(index, visible.length - 1));
  }
  if (saved.includes(chatId)) {
    return [...saved];
  }
  const next = [...saved];
  const anchorAt = (anchor: string | undefined): number =>
    anchor === undefined ? -1 : next.indexOf(anchor);
  const anchor = visible[index];
  let insertion = anchorAt(anchor);
  if (insertion < 0) {
    const tail = visible[visible.length - 1];
    const tailIndex = anchorAt(tail);
    insertion = tailIndex < 0 ? next.length : tailIndex + 1;
  }
  next.splice(insertion, 0, chatId);
  return next;
}

/**
 * `spaces.rs::sidebar_gap_offset` — preview geometry only: which neighbors
 * slide, and how far, while a drag's vacancy moves to its destination.
 * Regular ordering is never persisted by a drag.
 */
export function sidebarGapOffset(
  row: number,
  source: number | null,
  boundary: number,
  height: number,
): number {
  if (source === null) {
    return row >= boundary ? height : 0;
  }
  if (row === source) {
    return 0;
  }
  if (row < source && row >= boundary) {
    return height;
  }
  if (row > source && row < boundary) {
    return -height;
  }
  return 0;
}

/**
 * `finish_sidebar_session_transfer` over the profile buckets: apply one
 * drop to the merged pin projection, then settle every id back into its own
 * bucket (membership never crosses buckets, like
 * `commitVisiblePinReorder`). A pinned drop keeps the chat in the bucket
 * that already owns it — or the first bucket when a foreign id pins in.
 */
export function commitSessionDrop(
  buckets: Readonly<Record<string, readonly string[]>>,
  visibleIds: readonly string[],
  chatId: string,
  target: SidebarSessionDrop,
): Record<string, readonly string[]> {
  const merged = Object.values(buckets).flat();
  const next = sidebarSessionDropPins(merged, visibleIds, chatId, target);
  const owner = Object.keys(buckets).find((key) => buckets[key]!.includes(chatId));
  const out: Record<string, readonly string[]> = {};
  let adopted = false;
  for (const [key, ids] of Object.entries(buckets)) {
    const members = new Set(ids);
    const pinnedHere = target.kind === "pinned" && (key === owner || (owner === undefined && !adopted));
    if (pinnedHere && next.includes(chatId)) {
      adopted = true;
    }
    out[key] = next.filter((id) => members.has(id) || (pinnedHere && id === chatId));
  }
  return out;
}

/** `spaces.rs::pinned_session_drop_index` — strict in-section slot. */
export function pinnedSessionDropIndex(relY: number, count: number): number | null {
  if (count === 0) {
    return null;
  }
  const height = count * SIDEBAR_SESSION_SLOT - SIDEBAR_LIST_GAP;
  if (relY < 0 || relY > height) {
    return null;
  }
  return Math.min(Math.floor(relY / SIDEBAR_SESSION_SLOT), count - 1);
}

/**
 * `spaces.rs::pinned_session_clamped_index` — keep a sidebar-wide drag
 * physically bounded to the pinned section: the nearest valid pinned slot
 * while the pointer is over regular sessions (the strict helper above still
 * identifies whether the pointer is actually inside).
 */
export function pinnedSessionClampedIndex(relY: number, count: number): number | null {  if (count === 0) {
    return null;
  }
  return Math.min(Math.floor(Math.max(relY, 0) / SIDEBAR_SESSION_SLOT), count - 1);
}

/**
 * `render_active_rows`'s pin split: re-sort rows by the projected order
 * (stable — a rank tie keeps the incoming order), then split the leading
 * pinned block off so regular rows group without them.
 */
export function pinOrderedRows<T extends { chat: { id: string } }>(
  rows: readonly T[],
  pinnedIds: readonly string[],
): { pinned: T[]; regular: T[] } {
  if (pinnedIds.length === 0 || rows.length === 0) {
    return { pinned: [], regular: [...rows] };
  }
  const orderedIds = projectPinnedFirst(
    rows.map((row) => row.chat.id),
    pinnedIds,
  );
  const rank = new Map(orderedIds.map((id, ix) => [id, ix] as const));
  const sorted = [...rows].sort(
    (a, b) => (rank.get(a.chat.id) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.chat.id) ?? Number.MAX_SAFE_INTEGER),
  );
  const active = new Set(rows.map((row) => row.chat.id));
  const pinnedCount = new Set(pinnedIds.filter((id) => active.has(id))).size;
  return { pinned: sorted.slice(0, pinnedCount), regular: sorted.slice(pinnedCount) };
}

/** `spaces.rs::pinned_drag_scroll_delta` — proportional edge autoscroll. */
export function pinnedDragScrollDelta(pointerY: number, viewportTop: number, viewportBottom: number): number {
  if (viewportBottom <= viewportTop) {
    return 0;
  }
  if (pointerY < viewportTop + SIDEBAR_DRAG_SCROLL_BAND) {
    const penetration = Math.min(Math.max((viewportTop + SIDEBAR_DRAG_SCROLL_BAND - pointerY) / SIDEBAR_DRAG_SCROLL_BAND, 0), 1);
    return -SIDEBAR_DRAG_SCROLL_MAX * penetration;
  }
  if (pointerY > viewportBottom - SIDEBAR_DRAG_SCROLL_BAND) {
    const penetration = Math.min(Math.max((pointerY - (viewportBottom - SIDEBAR_DRAG_SCROLL_BAND)) / SIDEBAR_DRAG_SCROLL_BAND, 0), 1);
    return SIDEBAR_DRAG_SCROLL_MAX * penetration;
  }
  return 0;
}

/**
 * `spaces.rs::pinned_drag_scroll_step` — one autoscroll tick: null when the
 * drag ended, the loop is stale, or the edge scroll ran out of room.
 */
export function pinnedDragScrollStep(
  dragActive: boolean,
  loopGeneration: number,
  dragGeneration: number,
  current: number,
  max: number,
  delta: number,
): number | null {
  if (!dragActive || loopGeneration !== dragGeneration || delta === 0) {
    return null;
  }
  const next = Math.min(Math.max(current + delta, 0), Math.max(max, 0));
  return next === current ? null : next;
}

/**
 * `spaces.rs::pinned_drag_snapshot_is_valid` — a drag started against a
 * snapshot of the visible pins stays live only while every original pin is
 * still around (a deletion or external unpin cancels it).
 */
export function pinnedDragSnapshotIsValid(
  draggedId: string,
  snapshotIds: readonly string[],
  currentIds: ReadonlySet<string>,
): boolean {
  return snapshotIds.includes(draggedId) && snapshotIds.every((id) => currentIds.has(id));
}
