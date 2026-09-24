import type { ChangeRequestSummary, Chat, Device, Space } from "@zeron/proto";
import type { ChatStatus } from "@zeron/engine-client";
import { parseScopedId } from "@zeron/engine-client";
import type { SidebarOrganization, SidebarSection, SidebarSort } from "../state/ui-settings";
import { projectPinnedFirst } from "./sidebar-pins";

/**
 * The desktop's view derivations, ported 1:1 from `zeron_proto::view` and
 * `zeron_proto::entities::chat_indicator` (crates/proto/src/view.rs) so the
 * sidebar reads identically on both surfaces: same status dots, same
 * recency order, same attention buckets. Pure; unit-tested against the
 * Rust cases.
 */

export const SESSION_STALE_MS = 45_000;

export type { SidebarOrganization, SidebarSort };

/** `settings/devices.rs DEVICE_ONLINE_WINDOW_SECS` — presence staleness. */
export const DEVICE_ONLINE_WINDOW_SECS = 70;

/** A registry engine's live connection state, keyed by engine key. */
export type EnginePresence = ReadonlyMap<string, "connected" | "reconnecting" | "off">;

/**
 * Presence: last-seen within the online window (future timestamps count).
 * A device row that is MISSING reads online, not offline — the desktop's
 * `state.rs::device_online` resolves unknown ids to `true` so a row that
 * has not streamed yet never renders a spurious "offline" glyph. A device
 * backed by a known REGISTRY engine (a scoped id whose engine is in
 * `engineStates`) reports that engine's live connection state instead —
 * the supervised connection is more accurate than a heartbeat timestamp
 * (state.rs:1382-1399). Pure.
 */
export function deviceOnline(
  device: Device | undefined,
  now: number,
  engineStates?: EnginePresence,
): boolean {
  if (device !== undefined && engineStates !== undefined) {
    try {
      const scoped = parseScopedId(device.id);
      if (scoped.engine !== null && engineStates.has(scoped.engine)) {
        return engineStates.get(scoped.engine) === "connected";
      }
    } catch {
      // A malformed id falls through to the last-seen heuristic.
    }
  }
  if (device === undefined) {
    return true;
  }
  const lastSeen = device.lastSeenAt;
  if (lastSeen === null || lastSeen === undefined) {
    return false;
  }
  const at = Date.parse(lastSeen);
  if (!Number.isFinite(at)) {
    return false;
  }
  return now - at <= DEVICE_ONLINE_WINDOW_SECS * 1000;
}

/**
 * The "@ device" tag with presence (`state.rs::space_device_tag`,
 * state.rs:1405-1411): `label = "@ {name ?? 'Unknown device'}"`,
 * `offline = !deviceOnline(...)`. Staleness renders as a disconnected
 * GLYPH at the call sites, never words in the tag. Pure.
 */
export function spaceDeviceTag(
  space: { readonly deviceId: string },
  devices: readonly Device[],
  now: number,
  engineStates?: EnginePresence,
): { tag: string; offline: boolean } {
  const device = devices.find((row) => row.id === space.deviceId);
  return {
    tag: `@ ${device?.name ?? "Unknown device"}`,
    offline: !deviceOnline(device, now, engineStates),
  };
}

export type ChatIndicator = "working" | "awaitingInput" | "errored" | "completed" | "idle";

/** The live status dot: none | working | awaitingInput | errored. */
export type Indicator = "none" | "working" | "awaitingInput" | "errored";

/** One sidebar row, ready to draw: statuses, project line, branch, time. */
export interface ChatRow {
  readonly chat: Chat;
  readonly status: ChatIndicator;
  /** Line 1 left — the space's display name, or the cwd label, or "~". */
  readonly project: string;
  /**
   * The space's path — the monogram's stable seed (project_icon.rs).
   * Null for project-less sessions (seed "home", name "Home").
   */
  readonly projectPath: string | null;
  /**
   * Line 1 left as the desktop writes it: `"project @ device"`, or bare
   * `project` when the device is unknown (shell/spaces.rs render_active_rows
   * — an unknown device contributes no fragment, same as the archived list).
   */
  readonly folder: string;
  /** Line 2's brand mark — the chat's configured harness, when it has one. */
  readonly harness: string | null;
  /** Line 3 — `chat.sourceContext.branch` (trimmed), when present. */
  readonly branch: string | null;
  /** The corner's relative time, shown while idle. */
  readonly timeAgo: string;
  /** The chat's host device — the ByDevice grouping key. */
  readonly deviceId: string;
  /** The host device's name; null when the device row is unknown. */
  readonly deviceName: string | null;
  /** The host device's presence — drives the offline glyph (ticket 31). */
  readonly deviceOffline: boolean;
  /** The chat's current PR summary, when one is resolved (line 3, right). */
  readonly changeRequest: ChangeRequestSummary | null;
}

/** The view options that shape the sidebar's rows before layout (§2.1). */
export interface SidebarRowOptions {
  readonly sort?: SidebarSort;
  readonly showHarness?: boolean;
  readonly showBranch?: boolean;
  readonly showPullRequest?: boolean;
  /** PR summaries per chat id, from the sidebar's change-request watches. */
  readonly changeRequests?: ReadonlyMap<string, ChangeRequestSummary>;
  /** Registry engine connection states, for the live-presence override. */
  readonly engineStates?: EnginePresence;
  /**
   * Whether the spaces list is authoritative — the spaces RowSet's
   * `loaded` flag at the call site. A dangling spaceId hides the chat
   * only when true; while the frame is still out (lag or stream error)
   * the row renders with the "?" label (ticket 43 — the desktop's
   * one-drive-loop registry is never transiently wrong, state.rs:1438).
   */
  readonly spacesLoaded?: boolean;
}

/** The corner's status word, mirroring the desktop (Idle shows time-ago). */
export function statusWord(status: ChatIndicator): string | null {
  switch (status) {
    case "working":
      return "Working";
    case "awaitingInput":
      return "Input";
    case "errored":
      return "Failed";
    case "completed":
      return "Done";
    case "idle":
      return null;
  }
}

/** True when a chat has activity the user hasn't seen on any device. */
export function unseen(chat: Chat): boolean {
  const message = chat.lastMessageAt;
  if (message === null || message === undefined) {
    return false;
  }
  const seen = chat.lastSeenAt;
  return seen === null || seen === undefined || compareIso(message, seen) > 0;
}

/**
 * Staleness-checked indicator: a Working/AwaitingInput session row older
 * than SESSION_STALE_MS is dead — a crashed backend must never show an
 * eternal "Working". Errored is exempt; Idle is none.
 */
export function effectiveIndicator(session: ChatStatus | undefined, now: number): Indicator {
  if (session === undefined) {
    return "none";
  }
  switch (session.status) {
    case "idle":
      return "none";
    case "errored":
      return "errored";
    case "working":
    case "awaitingInput": {
      const updated = Date.parse(session.updatedAt);
      if (!Number.isFinite(updated) || now - updated > SESSION_STALE_MS) {
        return "none";
      }
      return session.status;
    }
  }
}

/** chat_indicator: live states win, then the seen marker decides. */
export function chatIndicator(chat: Chat, live: ChatStatus | undefined): ChatIndicator {
  switch (live?.status) {
    case "working":
      return "working";
    case "awaitingInput":
      return "awaitingInput";
    case "errored":
      return unseen(chat) ? "errored" : "idle";
    default:
      return unseen(chat) ? "completed" : "idle";
  }
}

/** The full display status for a chat row: live, staleness-gated, derived. */
export function displayStatus(chat: Chat, session: ChatStatus | undefined, now: number): ChatIndicator {
  const live = session !== undefined && effectiveIndicator(session, now) !== "none" ? session : undefined;
  return chatIndicator(chat, live);
}

/** Attention bucket — lower is more urgent (view.rs attention_rank). */
export function attentionRank(status: ChatIndicator): number {
  switch (status) {
    case "awaitingInput":
      return 0;
    case "errored":
      return 1;
    case "working":
      return 2;
    case "completed":
      return 3;
    case "idle":
      return 4;
  }
}

/**
 * The most attention-demanding status among rows (min rank) — the same
 * aggregation the desktop's space rows use for their urgency dot.
 */
export function mostUrgent(statuses: readonly ChatIndicator[]): ChatIndicator | null {
  let best: ChatIndicator | null = null;
  let bestRank = Number.POSITIVE_INFINITY;
  for (const status of statuses) {
    const rank = attentionRank(status);
    if (rank < bestRank) {
      best = status;
      bestRank = rank;
    }
  }
  return best;
}

function recencyKey(chat: Chat): string {
  return chat.lastMessageAt ?? chat.createdAt;
}

/**
 * Sidebar order (sort_active): pure recency — `lastMessageAt` desc with
 * `createdAt` fallback, id tiebreak so the sort is total. Status drives
 * the dot, never the position.
 */
export function sortRows<T extends { chat: Chat }>(rows: readonly T[]): T[] {
  return [...rows].sort((a, b) => {
    const byRecency = compareIso(recencyKey(b.chat), recencyKey(a.chat));
    if (byRecency !== 0) {
      return byRecency;
    }
    return a.chat.id < b.chat.id ? -1 : a.chat.id > b.chat.id ? 1 : 0;
  });
}

/**
 * The sidebar's chat comparator (`spaces.rs::compare_sidebar_chats`) — the
 * ONE ordering the active list, the keyboard/jump order, and the archived
 * shelf all share. `lastUpdated` sorts by `lastMessageAt ?? createdAt`
 * descending; `created` by `createdAt` alone; the chat id breaks ties
 * ascending so the sort is total and stable.
 */
export function compareSidebarChats(sort: SidebarSort, left: Chat, right: Chat): number {
  const primary =
    sort === "created"
      ? compareIso(right.createdAt, left.createdAt)
      : compareIso(right.lastMessageAt ?? right.createdAt, left.lastMessageAt ?? left.createdAt);
  return primary !== 0 ? primary : left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
}

/**
 * Put this machine's device group first without disturbing the order of any
 * remote group (`spaces.rs::promote_local_device_group`). No-op when the
 * local device id is null or matches nothing.
 */
export function promoteLocalDeviceGroup<T>(
  groups: readonly SidebarBucket<T>[],
  localDeviceId: string | null,
): SidebarBucket<T>[] {
  if (localDeviceId === null) {
    return groups as SidebarBucket<T>[];
  }
  const index = groups.findIndex(
    (bucket) => bucket.group !== null && bucket.group.kind === "device" && bucket.group.key === localDeviceId,
  );
  if (index <= 0) {
    return groups as SidebarBucket<T>[];
  }
  const next = [...groups];
  const [local] = next.splice(index, 1);
  next.unshift(local!);
  return next;
}

/** One grouping key: identity for collapsing, a label, and its flavor. */
export interface SidebarGroup {
  /** The collapse key's raw id — a device id or a space id. */
  readonly key: string;
  /** The disclosure header's label — the device or project display name. */
  readonly label: string;
  /** Which grouping produced this bucket. */
  readonly kind: "device" | "project";
}

/** One disclosure bucket: the group identity plus its rows in draw order. */
export interface SidebarBucket<T> {
  readonly group: SidebarGroup | null;
  readonly rows: readonly T[];
}

/**
 * Bucket the sorted rows for drawing: under `byDevice`, chats group by host
  * device preserving first-seen order, the local device's bucket promoted to
  * the top (`render_active_rows`); under `byProject` (upstream 78e9e6ae),
  * chats group by their space — `home:{device}` for project-less sessions —
  * with the project's display name as the label. Every other organization is
  * one flat, header-less bucket.
 */
export function sidebarGroups(
  rows: readonly ChatRow[],
  organization: SidebarOrganization,
  localDeviceId: string | null,
): SidebarBucket<ChatRow>[] {
  const buckets: SidebarBucket<ChatRow>[] = [];
  for (const row of rows) {
    let group: SidebarGroup | null = null;
    if (organization === "byDevice") {
      group = { key: row.deviceId, label: row.deviceName ?? "Unknown device", kind: "device" };
    } else if (organization === "byProject") {
      group = {
        key: row.chat.spaceId ?? `home:${row.deviceId}`,
        label: row.project,
        kind: "project",
      };
    }
    const existing = buckets.find((bucket) =>
      bucket.group === null
        ? group === null
        : group !== null && bucket.group.key === group.key && bucket.group.kind === group.kind,
    );
    if (existing !== undefined) {
      (existing.rows as ChatRow[]).push(row);
    } else {
      buckets.push({ group, rows: [row] });
    }
  }
  if (organization === "byDevice") {
    return promoteLocalDeviceGroup(buckets, localDeviceId);
  }
  return buckets;
}

/**
 * The flat, top-to-bottom chat ids exactly as the sidebar draws them —
 * pins first, then OPEN custom sections' members in section order, then the
 * grouping and local-device promotion of the UNCLAIMED rows, headers not
 * counted (`spaces.rs::sidebar_visible_order`, upstream 86249cf0's custom
 * sections). The jump shortcuts and session cycling read THIS order so
 * keyboard order never drifts from the screen. While the pinned disclosure
 * is collapsed the hidden pins hold no slot (they are not on the screen),
 * and a collapsed section's members hold no slot either.
 */
export function sidebarVisibleOrder(
  rows: readonly ChatRow[],
  organization: SidebarOrganization,
  localDeviceId: string | null,
  pinnedIds: readonly string[] = [],
  pinnedOpen = true,
  sections: readonly SidebarSection[] = [],
): string[] {
  const claimed = new Set(sections.flatMap((section) => section.sessionIds));
  const unclaimed = rows.filter((row) => !claimed.has(row.chat.id));
  const flat = sidebarGroups(unclaimed, organization, localDeviceId).flatMap((bucket) =>
    bucket.rows.map((row) => row.chat.id),
  );
  const ids = new Set(rows.map((row) => row.chat.id));
  const customOrder = sections
    .filter((section) => !section.collapsed)
    .flatMap((section) => section.sessionIds.filter((id) => ids.has(id)));
  const visible = projectPinnedFirst([...customOrder, ...flat], pinnedIds);
  if (!pinnedOpen) {
    const pins = new Set(pinnedIds);
    return visible.filter((id) => !pins.has(id));
  }
  return visible;
}

/**
 * Exact active-row height (`shell.rs::chat_row_height`): 45 compact, 61
 * branch-only, 63 with a PR badge (with or without a branch). The FLIP
 * resort diff and the disclosure body estimates both key off these.
 */
export function chatRowHeight(showsBranch: boolean, showsPullRequest: boolean): number {
  let metadataHeight = 0;
  if (showsBranch) {
    metadataHeight = Math.max(metadataHeight, 14);
  }
  if (showsPullRequest) {
    metadataHeight = Math.max(metadataHeight, 16);
  }
  return metadataHeight === 0 ? 45 : 47 + metadataHeight;
}

/**
 * `shell.rs::sidebar_row_height` (upstream 78e9e6ae): compact rows are
 * one-line 29px cards; detailed rows are the classic card, losing 16px
 * when the "project @ device" label is hidden.
 */
export function sidebarRowHeight(
  compact: boolean,
  showLabel: boolean,
  showsBranch: boolean,
  showsPullRequest: boolean,
): number {
  if (compact) {
    return 29;
  }
  return chatRowHeight(showsBranch, showsPullRequest) - (showLabel ? 0 : 16);
}

/** A keyed sidebar list entry: identity plus its FLIP height estimate. */
export interface SidebarKeyed {
  readonly key: string;
  readonly height: number;
}

/**
 * FLIP diff for a keyed list (`shell.rs::resort_offsets`): lay both orders
 * out as `y += height + gap` and emit each surviving key's paint-only start
 * offset `oldY - newY`, only when it moved more than half a pixel.
 */
export function resortOffsets(
  old: readonly SidebarKeyed[],
  next: readonly SidebarKeyed[],
  gap: number,
): Map<string, number> {
  const oldY = new Map<string, number>();
  let y = 0;
  for (const { key, height } of old) {
    oldY.set(key, y);
    y += height + gap;
  }
  const offsets = new Map<string, number>();
  y = 0;
  for (const { key, height } of next) {
    const prev = oldY.get(key);
    if (prev !== undefined) {
      const dy = prev - y;
      if (Math.abs(dy) > 0.5) {
        offsets.set(key, dy);
      }
    }
    y += height + gap;
  }
  return offsets;
}

/**
 * Height changes do not constitute a list reorder — a disclosure animating
 * its own body height must not also trigger FLIP offsets on every following
 * keyed section (`shell.rs::sidebar_key_order_changed`).
 */
export function sidebarKeyOrderChanged(old: readonly SidebarKeyed[], next: readonly SidebarKeyed[]): boolean {
  return (
    old.length !== next.length || old.some((entry, index) => entry.key !== next[index]?.key)
  );
}

/**
 * The sidebar's chat list (`overview_chats` + `render_active_rows`): every
 * non-archived chat of a live space — or no space at all — idle included,
 * display statuses and project lines attached, sorted by the user's sidebar
 * preference with the show-toggle fields cleared before layout. Chats whose
 * spaceId points at a missing space row hide only once the spaces RowSet is
 * loaded (`options.spacesLoaded`); until then they render with the "?"
 * label (ticket 43).
 */
export function chatListRows(
  chats: readonly Chat[],
  spaces: readonly Space[],
  statuses: readonly ChatStatus[],
  now: number,
  devices: readonly Device[] = [],
  options: SidebarRowOptions = {},
): ChatRow[] {
  const statusByChat = new Map(statuses.map((row) => [row.chatId, row]));
  const spaceById = new Map(spaces.map((space) => [space.id, space]));
  const deviceById = new Map(devices.map((device) => [device.id, device]));
  const rows: ChatRow[] = [];
  for (const chat of chats) {
    if (chat.archived) {
      continue;
    }
    const row = toChatRow(chat, spaceById, statusByChat, now, deviceById, options);
    if (row !== null) {
      rows.push(row);
    }
  }
  const sort = options.sort ?? "lastUpdated";
  return rows.sort((left, right) => compareSidebarChats(sort, left.chat, right.chat));
}

/**
 * One chat's row by id — the chat page's lookup. Unlike the sidebar list
 * this includes archived chats (archiving never closes an open chat);
 * chats whose spaceId dangles hide under the same loaded-only gate as
 * the list (`spacesLoaded`, ticket 43).
 */
export function chatPageRow(
  chatId: string,
  chats: readonly Chat[],
  spaces: readonly Space[],
  statuses: readonly ChatStatus[],
  now: number,
  devices: readonly Device[] = [],
  engineStates?: EnginePresence,
  spacesLoaded = true,
): ChatRow | undefined {
  const chat = chats.find((candidate) => candidate.id === chatId);
  if (chat === undefined) {
    return undefined;
  }
  const spaceById = new Map(spaces.map((space) => [space.id, space]));
  const statusByChat = new Map(statuses.map((row) => [row.chatId, row]));
  const deviceById = new Map(devices.map((device) => [device.id, device]));
  return toChatRow(chat, spaceById, statusByChat, now, deviceById, { engineStates, spacesLoaded }) ?? undefined;
}

function toChatRow(
  chat: Chat,
  spaceById: ReadonlyMap<string, Space>,
  statusByChat: ReadonlyMap<string, ChatStatus>,
  now: number,
  deviceById: ReadonlyMap<string, Device> = new Map(),
  options: SidebarRowOptions = {},
): ChatRow | null {
  const space =
    chat.spaceId !== null && chat.spaceId !== undefined ? spaceById.get(chat.spaceId) : undefined;
  const dangling = chat.spaceId !== null && chat.spaceId !== undefined && space === undefined;
  // Hide only once the spaces list is loaded and the id is truly missing
  // (desktop parity, state.rs:1438). While the spaces frame is still out
  // — lagging or errored — the row renders with the "?" label until the
  // space resolves; a spaces-stream error must never blank
  // space-attached chats (ticket 43).
  if (dangling && (options.spacesLoaded ?? true)) {
    return null;
  }
  // Only conversation-owned source context is trusted (`conversation_branch`):
  // the legacy scalar `branch` cannot prove a worktree has not switched since
  // it was written.
  const rawBranch = chat.sourceContext?.branch ?? null;
  const branch = rawBranch !== null && rawBranch.trim().length > 0 ? rawBranch.trim() : null;
  const project = space !== undefined ? spaceDisplayName(space) : dangling ? "?" : "~";
  const device = deviceById.get(chat.deviceId);
  const harness =
    options.showHarness === false ? null : (chat.config?.harness ?? null);
  return {
    chat,
    status: displayStatus(chat, statusByChat.get(chat.id), now),
    project,
    projectPath: space?.path ?? null,
    folder: device !== undefined ? `${project} @ ${device.name}` : project,
    harness,
    branch: options.showBranch === false ? null : branch,
    timeAgo: timeAgo(recencyKey(chat), now),
    deviceId: chat.deviceId,
    deviceName: device !== undefined ? device.name : null,
    deviceOffline: !deviceOnline(device, now, options.engineStates),
    changeRequest:
      options.showPullRequest === false ? null : (options.changeRequests?.get(chat.id) ?? null),
  };
}

/** Display name of a space: the rename, else the folder basename. */
export function spaceDisplayName(space: Space): string {
  const name = space.name;
  if (name !== null && name !== undefined && name.trim().length > 0) {
    return name;
  }
  return basename(space.path) ?? space.path;
}

/**
 * Spaces in display order — case-insensitive display name, id tiebreak
 * (state.rs spaces_sorted). The order both space selectors list rows in.
 */
export function spacesSorted(spaces: readonly Space[]): Space[] {
  return [...spaces].sort((a, b) => {
    const an = spaceDisplayName(a).toLowerCase();
    const bn = spaceDisplayName(b).toLowerCase();
    if (an !== bn) {
      return an < bn ? -1 : 1;
    }
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

/**
 * The spaces list with the add-space palette's optimistic rows folded in
 * (the desktop pushes them straight into `AppState.spaces`; the web keeps
 * them on `addSpaceStore` because the watch cache has no row-injection
 * API). Merged by id so a watch-frame-confirmed row REPLACES its
 * optimistic twin — never a duplicate. Pure.
 */
export function mergePendingSpaces(
  spaces: readonly Space[],
  pending: readonly Space[],
): readonly Space[] {
  if (pending.length === 0) {
    return spaces;
  }
  const confirmed = new Set(spaces.map((space) => space.id));
  return [...spaces, ...pending.filter((space) => !confirmed.has(space.id))];
}

/**
 * Dangling-filter healing (shell.rs): a filter naming a space that no
 * longer exists — deleted, or from another engine after a switch — reads
 * as "All projects" rather than filtering everything out.
 */
export function healedSpaceFilter(filter: string | null, spaces: readonly Space[]): string | null {
  if (filter === null) {
    return null;
  }
  return spaces.some((space) => space.id === filter) ? filter : null;
}

/** Whitespace-collapsed single line (proto view::single_line). */
export function singleLine(text: string): string {
  return text
    .split(/\s+/)
    .filter((part) => part.length > 0)
    .join(" ");
}

/** One archived-shelf row: single-line title + relative time. */
export interface ArchivedRow {
  readonly chat: Chat;
  readonly title: string;
  readonly timeAgo: string;
}

/**
 * The sidebar's archived shelf (render_archived_section): archived chats of
 * the filter scope — all spaces under "All" — in the user's sidebar sort
 * (`compareSidebarChats`, the same comparator the active list uses — never
 * its own fixed recency order).
 */
export function archivedRows(
  chats: readonly Chat[],
  spaceFilter: string | null,
  now: number,
  sort: SidebarSort = "lastUpdated",
): ArchivedRow[] {
  const rows = chats.filter(
    (chat) =>
      chat.archived &&
      (spaceFilter === null || (chat.spaceId !== undefined && chat.spaceId === spaceFilter)),
  );
  rows.sort((left, right) => compareSidebarChats(sort, left, right));
  return rows.map((chat) => {
    const title = chat.title === null ? "" : singleLine(chat.title);
    return {
      chat,
      title: title.length > 0 ? title : "New session",
      timeAgo: timeAgo(recencyKey(chat), now),
    };
  });
}

/**
 * The archived shelf's SHARED row data (upstream dfd2fc0c's
 * `sidebar_chat_data`): archived sessions derive the same `ChatRow` shape
 * the active list draws — project @ device folder, branch, PR, harness,
 * statuses — so the shelf shares layout and metadata in every sidebar
 * mode. Same filter scope and comparator as `archivedRows`.
 */
export function archivedChatRows(
  chats: readonly Chat[],
  spaces: readonly Space[],
  spaceFilter: string | null,
  statuses: readonly ChatStatus[],
  now: number,
  devices: readonly Device[] = [],
  options: SidebarRowOptions = {},
): ChatRow[] {
  const spaceById = new Map(spaces.map((space) => [space.id, space]));
  const statusByChat = new Map(statuses.map((row) => [row.chatId, row]));
  const deviceById = new Map(devices.map((device) => [device.id, device]));
  const rows: ChatRow[] = [];
  for (const chat of chats) {
    if (
      !chat.archived ||
      (spaceFilter !== null && (chat.spaceId === undefined || chat.spaceId !== spaceFilter))
    ) {
      continue;
    }
    // The shelf never hides a dangling-space row (the desktop's
    // `sidebar_chat_data` renders the "?" project label) — so the shared
    // row derivation's loaded-only hide stays off here.
    const row = toChatRow(chat, spaceById, statusByChat, now, deviceById, {
      ...options,
      spacesLoaded: false,
    });
    if (row !== null) {
      rows.push(row);
    }
  }
  const sort = options.sort ?? "lastUpdated";
  return rows.sort((left, right) => compareSidebarChats(sort, left.chat, right.chat));
}

/** Project label from a cwd (project_label): its basename, or null. */
export function projectLabel(cwd: string | null | undefined): string | null {
  const trimmed = cwd?.trim();
  if (trimmed === undefined || trimmed.length === 0 || trimmed === "~" || trimmed === "~/") {
    return null;
  }
  return basename(trimmed) ?? trimmed;
}

function basename(path: string): string | null {
  const withoutTrailing = path.replace(/[\\/]+$/, "");
  const slash = Math.max(withoutTrailing.lastIndexOf("/"), withoutTrailing.lastIndexOf("\\"));
  const name = slash >= 0 ? withoutTrailing.slice(slash + 1) : withoutTrailing;
  return name.length > 0 ? name : null;
}

/**
 * Compact relative time — "now", "5m", "3h", "2d", "1w", "2mo", "1y" —
 * port of view.rs format_time_ago (no "ago" suffix; negative ages clamp
 * to "now").
 */
export function timeAgo(iso: string, now: number): string {
  const then = Date.parse(iso);
  if (!Number.isFinite(then)) {
    return "";
  }
  const seconds = Math.max(0, Math.floor((now - then) / 1000));
  if (seconds < 60) {
    return "now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  if (days < 7) {
    return `${days}d`;
  }
  const weeks = Math.floor(days / 7);
  if (weeks < 5) {
    return `${weeks}w`;
  }
  const months = Math.floor(days / 30);
  if (months < 12) {
    return `${months}mo`;
  }
  return `${Math.floor(days / 365)}y`;
}

/**
 * Chronological comparison of the engine's RFC 3339 timestamps: epoch-ms
 * first, lexicographic fallback for sub-millisecond ties.
 */
function compareIso(a: string, b: string): number {
  const ta = Date.parse(a);
  const tb = Date.parse(b);
  if (!Number.isFinite(ta) || !Number.isFinite(tb)) {
    return a === b ? 0 : a < b ? -1 : 1;
  }
  if (ta !== tb) {
    return ta - tb;
  }
  return a === b ? 0 : a < b ? -1 : 1;
}
