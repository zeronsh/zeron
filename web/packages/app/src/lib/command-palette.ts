import type { ChangeRequestSummary, Chat, Device, Space } from "@zeron/proto";
import type { ChatStatus } from "@zeron/engine-client";
import type { IconName } from "@zeron/icons";
import type { AppearanceMode } from "./appearance-store";
import { appearanceModeIcon } from "./appearance-store";
import type { ChatIndicator, SidebarSort } from "./view";
import { compareSidebarChats, displayStatus, singleLine, spaceDisplayName, timeAgo } from "./view";

/**
 * The command palette's derivations — the web port of the pure half of
 * `crates/ui/src/shell/command_palette.rs`: `matches_query`, `actions_for`,
 * and the global chat-history search (title + project + device + branch + PR
 * metadata, archived included, capped at 30 rows after sorting).
 *
 * The row shape mirrors what the desktop passes into `render_chat_row`:
 * project/folder/branch/PR/harness arrive UNGATED here (search reads them
 * all); the component applies the sidebar's show-toggle settings when it
 * renders, exactly as the palette's row call does on the desktop.
 */

/** `HISTORY_RESULT_LIMIT` (command_palette.rs): the matching-chat cap. */
export const HISTORY_RESULT_LIMIT = 30;

export type CommandActionId = "new-chat" | "new-project" | "settings" | "theme";

/** One palette action row: id, label (the search target), and icon. */
export interface CommandAction {
  readonly id: CommandActionId;
  readonly label: string;
  readonly icon: IconName;
  /** The theme the "theme" action switches to (it targets the OPPOSITE of
   *  the resolved appearance and keeps the palette open). */
  readonly theme?: AppearanceMode;
}

/**
 * `actions_for(query, is_dark)`: the actions whose labels match the query,
 * order kept. The theme action targets the opposite of the resolved
 * appearance ("Switch to light theme" while dark, and vice versa) —
 * upstream b4dd24d7's quick theme action.
 */
export function actionsFor(query: string, isDark: boolean): CommandAction[] {
  const theme: AppearanceMode = isDark ? "light" : "dark";
  const actions: readonly CommandAction[] = [
    { id: "new-chat", label: "New chat", icon: "penNewSquare" },
    { id: "new-project", label: "New project", icon: "folder" },
    { id: "settings", label: "Open settings", icon: "settingsMinimalistic" },
    {
      id: "theme",
      label: isDark ? "Switch to light theme" : "Switch to dark theme",
      icon: appearanceModeIcon(theme),
      theme,
    },
  ];
  return actions.filter((action) => matchesQuery(query, action.label));
}

/**
 * `matches_query` (command_palette.rs): every whitespace-separated word
 * appears somewhere in the text, case-insensitively. An empty/blank query
 * matches everything.
 */
export function matchesQuery(query: string, text: string): boolean {
  const haystack = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter((word) => word.length > 0)
    .every((word) => haystack.includes(word));
}

/** The search haystack the desktop builds per chat (command_entries). */
function chatHaystack(
  chat: Chat,
  project: string,
  device: string,
  branch: string,
  pr: string,
): string {
  const title = chat.title === null ? "New session" : singleLine(chat.title);
  return `${title} ${project} ${device} ${branch} ${pr}`;
}

/** The PR fragment the desktop searches: `#number title headRef baseRef`. */
function prHaystack(summary: ChangeRequestSummary | undefined): string {
  if (summary === undefined) {
    return "";
  }
  return `#${summary.number} ${summary.title} ${summary.headRef} ${summary.baseRef}`;
}

/**
 * `conversation_branch` (change_requests.rs): only the conversation-owned
 * source context's branch is trusted, trimmed; an empty branch reads as
 * none.
 */
function conversationBranch(chat: Chat): string {
  const raw = chat.sourceContext?.branch ?? "";
  const trimmed = raw.trim();
  return trimmed.length > 0 ? trimmed : "";
}

export interface PaletteChatRow {
  readonly chat: Chat;
  /** Single-line title; "New session" when untitled. */
  readonly title: string;
  readonly timeAgo: string;
  /** The project label: the space's display name, "~" without one, "?" for a
   *  dangling space id (the desktop's render_chat_row mapping). */
  readonly project: string;
  /** "project @ device" when the device is known, else the bare project. */
  readonly folder: string;
  /** Raw trimmed branch — the caller gates display on the sidebar setting. */
  readonly branch: string | null;
  /** Raw PR summary — the caller gates display on the sidebar setting. */
  readonly changeRequest: ChangeRequestSummary | null;
  /** Raw harness id — the caller gates display on the sidebar setting. */
  readonly harness: string | null;
  readonly status: ChatIndicator;
  readonly archived: boolean;
}

/**
 * The palette's chat history: EVERY chat (archived included — the desktop's
 * global history deliberately ignores the sidebar's filters and collapsed
 * groups) whose metadata matches the query, sorted with the sidebar's
 * comparator and capped at [`HISTORY_RESULT_LIMIT`] after filtering and
 * sorting so every chat remains searchable.
 */
export function paletteChats(input: {
  readonly chats: readonly Chat[];
  readonly spaces: readonly Space[];
  readonly statuses: readonly ChatStatus[];
  readonly devices: readonly Device[];
  readonly changeRequests: ReadonlyMap<string, ChangeRequestSummary>;
  readonly now: number;
  readonly query: string;
  readonly sort?: SidebarSort;
}): PaletteChatRow[] {
  const spaceById = new Map(input.spaces.map((space) => [space.id, space]));
  const deviceById = new Map(input.devices.map((device) => [device.id, device]));
  const statusByChat = new Map(input.statuses.map((row) => [row.chatId, row]));
  const rows: PaletteChatRow[] = [];
  for (const chat of input.chats) {
    const space =
      chat.spaceId !== null && chat.spaceId !== undefined ? spaceById.get(chat.spaceId) : undefined;
    const project =
      space !== undefined
        ? spaceDisplayName(space)
        : chat.spaceId !== null && chat.spaceId !== undefined
          ? "?"
          : "~";
    const device = deviceById.get(chat.deviceId);
    const branch = conversationBranch(chat);
    const changeRequest = input.changeRequests.get(chat.id) ?? null;
    if (
      !matchesQuery(
        input.query,
        chatHaystack(
          chat,
          project,
          device?.name ?? "",
          branch,
          prHaystack(changeRequest ?? undefined),
        ),
      )
    ) {
      continue;
    }
    rows.push({
      chat,
      title: singleLine(chat.title ?? "") || "New session",
      timeAgo: timeAgo(chat.lastMessageAt ?? chat.createdAt, input.now),
      project,
      folder: device !== undefined ? `${project} @ ${device.name}` : project,
      branch,
      changeRequest,
      harness: chat.config?.harness ?? null,
      status: displayStatus(chat, statusByChat.get(chat.id), input.now),
      archived: chat.archived,
    });
  }
  const sort = input.sort ?? "lastUpdated";
  rows.sort((left, right) => compareSidebarChats(sort, left.chat, right.chat));
  return rows.slice(0, HISTORY_RESULT_LIMIT);
}
