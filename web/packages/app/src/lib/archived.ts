import type { Chat, Device } from "@zeron/proto";
import { archivedRows, projectLabel, type ArchivedRow, type SidebarSort } from "./view";

/**
 * The Settings → Archived page's row derivation (the web peer of
 * `crates/ui/src/settings/archived.rs` + proto `view::chat_location`). The
 * filter/sort is `lib/view.ts`'s `archivedRows` — the SAME comparator the
 * sidebar shelf uses — driven with no space filter, so the full page shows
 * every archived chat; this module only adds the fuller per-row content the
 * page renders (device name, location, the "Untitled session" fallback).
 */

/** One full-page archived row: the shelf row plus device · location. */
export interface ArchivedChatRow extends ArchivedRow {
  /** The host device's name; null (fragment omitted) when unknown. */
  readonly device: string | null;
  /** `chat_location` (proto view.rs:252-270): "project · branch", or either alone. */
  readonly location: string | null;
}

/**
 * `archived_chats` (archived.rs:15-17), page-shaped: every archived chat in
 * the sidebar's recency order, unscoped by space, with the device-name and
 * location fragments the page's meta line renders.
 */
export function archivedChats(
  chats: readonly Chat[],
  devices: readonly Device[],
  now: number,
  sort: SidebarSort = "lastUpdated",
): ArchivedChatRow[] {
  const deviceNames = new Map(devices.map((device) => [device.id, device.name]));
  // No space filter — the full-page view shows every archived chat, not just
  // the current space's (the page's one deliberate difference from the shelf).
  return archivedRows(chats, null, now, sort).map((row) => ({
    ...row,
    title: chatTitle(row.chat),
    device: deviceNames.get(row.chat.deviceId) ?? null,
    location: chatLocation(row.chat),
  }));
}

/** The page's title fallback: "Untitled session" (the shelf's is "New session"). */
function chatTitle(chat: Chat): string {
  return chat.title !== null && chat.title !== undefined && chat.title.trim().length > 0
    ? chat.title
    : "Untitled session";
}

/**
 * `chat_location` (proto view.rs:252-270): the cwd's project label and the
 * branch, joined by " · " — either may be missing; empty when both are.
 */
export function chatLocation(chat: Chat): string | null {
  const project = projectLabel(chat.cwd);
  const branch = chat.branch?.trim();
  if (project !== null && branch !== undefined && branch.length > 0) {
    return `${project} · ${branch}`;
  }
  if (project !== null) {
    return project;
  }
  if (branch !== undefined && branch.length > 0) {
    return branch;
  }
  return null;
}
