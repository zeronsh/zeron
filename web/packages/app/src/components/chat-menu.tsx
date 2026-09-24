import { useRef, useState, type ReactElement } from "react";
import { useNavigate, useParams } from "@tanstack/react-router";
import { Icon } from "@zeron/icons";
import { parseScopedId, type EngineRegistrySnapshot } from "@zeron/engine-client";
import type { Chat } from "@zeron/proto";
import { useEngineSessions } from "../state/session-provider";
import type { EngineSession } from "../state/engine-session";
import { sidebarNotice } from "../state/notice";
import { sidebarStore, useSidebar } from "../state/sidebar";
import { useFleetRegistry } from "../state/fleet";
import { sidebarPinProfileKey } from "../lib/sidebar-pins";
import { reviewCommentStore } from "../state/review-comments";
import { deleteChat, describeMutateError, renameChat, setChatArchived, type MutateCaller } from "../lib/chat-actions";
import { singleLine } from "../lib/view";
import {
  RbContextMenu,
  RbContextMenuPopup,
  RbContextMenuPortal,
  RbContextMenuPositioner,
  RbContextMenuTrigger,
} from "./base/menu";
import { Dialog, DialogCard, DialogTitle, DialogBody, DialogField, BtnGhost, BtnPrimary, BtnDanger } from "./ui/Dialog";
import { MenuRow, MenuSeparator } from "./ui/MenuRows";

/**
 * The chat row's management surface — the desktop's `ChatMenuState`
 * (shell.rs:5433-5608). Opened by RIGHT mouse-down at the pointer (both the
 * active list and the archived shelf reuse it), positioned clamp-only at
 * the pointer (`menu_at` — no flip) by `RbContextMenu`, 216px wide. The
 * Copy row swaps the card's content to a Copy page IN PLACE — no second
 * floating layer. Rename and delete open modal dialogs; mutation failures
 * surface in the sidebar notice strip.
 *
 * There is no kebab: the right-click is the only affordance (the ticket
 * settles the research's open question — right-click only, kebab removed).
 *
 * The menu is a Base UI `ContextMenu` — the trigger wraps the chat row
 * (`menu(row)`), the library owns the right-click/long-press open, the
 * pointer positioning, Escape and outside-press dismissal, and the exit
 * window (`.rb-popover-popup`'s `[data-closed]` motion + occluder).
 */

/** The card width (`shell.rs`'s ChatMenu card). */
const CHAT_MENU_WIDTH = 216;

export function useChatMenu(chat: Chat) {
  // The menu opens on rows from ANY engine — resolve the owning session
  // off the scoped chat id so mutations route to the right engine.
  const sessions = useEngineSessions();
  const session = chatMenuSession(sessions, chat.id);
  const [open, setOpen] = useState(false);
  const [dialog, setDialog] = useState<"rename" | "delete" | null>(null);

  function run(mutation: (caller: MutateCaller) => Promise<unknown>): void {
    if (session === null) {
      sidebarNotice.set("Engine not connected");
      return;
    }
    mutation(session.client).catch((error: unknown) => {
      sidebarNotice.set(describeMutateError(error));
    });
  }

  /**
   * Wraps a chat row element so a right-click (or long-press) opens this
   * menu at the pointer. The Trigger adopts the row via its `render` prop —
   * no wrapper div, the row's own DOM is unchanged.
   */
  function menu(row: ReactElement): ReactElement {
    return (
      <RbContextMenu open={open} onOpenChange={setOpen}>
        <RbContextMenuTrigger render={row} />
        <RbContextMenuPortal>
          <RbContextMenuPositioner>
            <RbContextMenuPopup
              className="rb-popover-popup popover-card"
              role="menu"
              aria-label="Chat actions"
              style={{ width: CHAT_MENU_WIDTH }}
            >
              <ChatMenuPages
                chat={chat}
                onRename={() => {
                  setOpen(false);
                  setDialog("rename");
                }}
                onArchive={() => {
                  setOpen(false);
                  run((caller) => setChatArchived(caller, chat.id, true));
                }}
                onDelete={() => {
                  setOpen(false);
                  setDialog("delete");
                }}
                onClose={() => setOpen(false)}
              />
            </RbContextMenuPopup>
          </RbContextMenuPositioner>
        </RbContextMenuPortal>
      </RbContextMenu>
    );
  }

  const element = (
    <>
      {dialog === "rename" && (
        <RenameChatDialog
          chat={chat}
          onSubmit={(title) => run((caller) => renameChat(caller, chat.id, title))}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "delete" && (
        <DeleteChatDialog
          chat={chat}
          onDelete={() =>
            run((caller) =>
              deleteChat(caller, chat.id).then(() => {
                // `purge_review_comments` (state.rs:754-757, wired from
                // composer.rs::purge_chat:4596): a deleted chat's staged
                // comments could never be sent again.
                reviewCommentStore.purgeChat(chat.id);
              }),
            )
          }
          onClose={() => setDialog(null)}
        />
      )}
    </>
  );

  return { menu, element };
}

function ChatMenuPages({
  chat,
  onRename,
  onArchive,
  onDelete,
  onClose,
}: {
  readonly chat: Chat;
  readonly onRename: () => void;
  readonly onArchive: () => void;
  readonly onDelete: () => void;
  readonly onClose: () => void;
}) {
  // Mounts per open (the popup's content unmounts once the exit has
  // drained), so the page resets to "root" on every open, as before.
  const [page, setPage] = useState<"root" | "copy">("root");
  // The Pin row reads the device-local pin bucket of the chat's OWNING
  // engine (shell.rs's chat menu + `active_sidebar_pin_profile_key`); null
  // is the desktop's "identity not ready" early return.
  const registry = useFleetRegistry();
  const pinProfileKey = chatPinProfileKey(registry, chat.id);
  const pinnedByProfile = useSidebar().pinnedByProfile;
  const isPinned = pinProfileKey !== null && (pinnedByProfile[pinProfileKey] ?? []).includes(chat.id);

  const codexLink = codexConversationLink(chat);
  const harnessSessionId =
    typeof chat.harnessSessionId === "string" && chat.harnessSessionId.trim().length > 0
      ? chat.harnessSessionId
      : null;

  async function copyToClipboard(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // A refused clipboard permission is not worth a crash.
    }
  }

  async function copyConversationLink(): Promise<void> {
    // The web's conversation link is its page URL — the desktop's
    // `zeron://open/chat/…` deep link has no browser handler.
    const link = typeof window === "undefined" ? null : new URL(`/chat/${chat.id}`, window.location.origin).toString();
    onClose();
    if (link === null) {
      sidebarNotice.set("Conversation link is not ready yet");
      return;
    }
    await copyToClipboard(link);
    sidebarNotice.set("Zeron conversation link copied");
  }

  async function copyText(text: string, notice: string): Promise<void> {
    onClose();
    await copyToClipboard(text);
    sidebarNotice.set(notice);
  }

  return page === "root" ? (
    <>
      <MenuRow fadeKey="rename" onClick={onRename}>
        <Icon name="pen" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">Rename…</span>
      </MenuRow>
      <MenuRow
        fadeKey="pin"
        onClick={() => {
          // `set_chat_pinned`: device-local, no engine roundtrip; the click
          // closes the menu like the desktop's `close_chat_menu`.
          onClose();
          sidebarStore.setChatPinned(pinProfileKey, chat.id, !isPinned);
        }}
      >
        <Icon name="pin" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">{isPinned ? "Unpin" : "Pin"}</span>
      </MenuRow>
      <MenuRow fadeKey="archive" onClick={onArchive}>
        <Icon name="archiveMinimalistic" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">Archive</span>
      </MenuRow>
      <MenuRow
        fadeKey="copy"
        onClick={() => {
          // The Copy page replaces the card's content IN PLACE —
          // no second floating layer, no portal remount.
          setPage("copy");
        }}
      >
        <Icon name="copy" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">Copy</span>
        <span className="chat-menu-row-spring" />
        <Icon name="altArrowRight" size={14} className="chat-menu-row-arrow" />
      </MenuRow>
      <MenuSeparator />
      <MenuRow fadeKey="delete" className="chat-menu-row-danger" onClick={onDelete}>
        <Icon name="trashBinMinimalistic" size={16} className="chat-menu-row-icon-danger" />
        <span className="menu-row-label">Delete…</span>
      </MenuRow>
    </>
  ) : (
    <>
      <MenuRow
        fadeKey="back"
        onClick={() => {
          setPage("root");
        }}
      >
        <Icon name="altArrowLeft" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">Back</span>
      </MenuRow>
      <MenuSeparator />
      <MenuRow fadeKey="zeron-link" onClick={() => void copyConversationLink()}>
        <Icon name="copy" size={16} className="chat-menu-row-icon" />
        <span className="menu-row-label">Zeron conversation link</span>
      </MenuRow>
      {codexLink !== null && (
        <MenuRow fadeKey="codex-link" onClick={() => void copyText(codexLink.url, `${codexLink.label} copied`)}>
          <Icon name="copy" size={16} className="chat-menu-row-icon" />
          <span className="menu-row-label">{codexLink.label}</span>
        </MenuRow>
      )}
      {harnessSessionId !== null && (
        <MenuRow fadeKey="harness-session" onClick={() => void copyText(harnessSessionId, "Harness session ID copied")}>
          <Icon name="copy" size={16} className="chat-menu-row-icon" />
          <span className="menu-row-label">Harness session ID</span>
        </MenuRow>
      )}
    </>
  );
}

/**
 * `links.rs::harness_conversation_link` — only schemes verified against the
 * harness app: a codex chat's session id becomes `codex://threads/{id}`
 * ("Codex conversation link"). Hermes exposes a candidate scheme, but its
 * contract is not stable enough for users' clipboards yet.
 */
function codexConversationLink(chat: Chat): { label: string; url: string } | null {
  const id = chat.harnessSessionId;
  if (typeof id !== "string" || id.trim().length === 0) {
    return null;
  }
  if (chat.config === null || chat.config.harness !== "codex") {
    return null;
  }
  return { label: "Codex conversation link", url: `codex://threads/${encodeComponent(id)}` };
}

function encodeComponent(value: string): string {
  let out = "";
  for (const byte of new TextEncoder().encode(value)) {
    if (
      (byte >= 0x30 && byte <= 0x39) ||
      (byte >= 0x41 && byte <= 0x5a) ||
      (byte >= 0x61 && byte <= 0x7a) ||
      byte === 0x2d ||
      byte === 0x5f ||
      byte === 0x2e ||
      byte === 0x7e
    ) {
      out += String.fromCharCode(byte);
    } else {
      out += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return out;
}

/**
 * The rename dialog (shell.rs open_rename_chat / submit_rename_chat):
 * prefilled single-line input, Enter submits, an empty title is a no-op.
 * Escape closes through RbDialog's escape path (`onOpenChange(false)`).
 */
function RenameChatDialog({ chat, onSubmit, onClose }: { chat: Chat; onSubmit: (title: string) => void; onClose: () => void }) {
  const [title, setTitle] = useState(chat.title ?? "");
  const inputRef = useRef<HTMLInputElement | null>(null);

  return (
    <Dialog ariaLabel="Rename session" onClose={onClose} initialFocus={inputRef}>
      <DialogCard>
        <DialogTitle>Rename session</DialogTitle>
        <form
          className="dialog-form-rows"
          onSubmit={(event) => {
            event.preventDefault();
            onSubmit(title);
            onClose();
          }}
        >
          <DialogField>
            <input
              ref={inputRef}
              type="text"
              aria-label="Session title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              spellCheck={false}
            />
          </DialogField>
          <div className="dialog-actions-row">
            <BtnGhost type="button" onClick={onClose}>
              Cancel
            </BtnGhost>
            <BtnPrimary type="submit">Rename</BtnPrimary>
          </div>
        </form>
      </DialogCard>
    </Dialog>
  );
}

/**
 * The session owning a scoped chat id — the row-level router for the menu's
 * mutations. Unscoped ids resolve to null (nothing to route to).
 */
function chatMenuSession(
  sessions: ReadonlyMap<string, EngineSession>,
  chatId: string,
): EngineSession | null {
  try {
    const engine = parseScopedId(chatId).engine;
    return engine === null ? null : sessions.get(engine) ?? null;
  } catch {
    return null;
  }
}

/**
 * The pin bucket of the engine owning this chat — the web's
 * `active_sidebar_pin_profile_key`. Null while the owning engine's identity
 * (`EngineInfo`) has not landed; pins neither show nor change against it.
 */
function chatPinProfileKey(registry: EngineRegistrySnapshot, chatId: string): string | null {
  try {
    const engineKey = parseScopedId(chatId).engine;
    if (engineKey === null) {
      return null;
    }
    const engine = registry.engines.find((entry) => entry.key === engineKey);
    if (engine === undefined) {
      return null;
    }
    return sidebarPinProfileKey(engine.info?.workspaceScope ?? null, engine.info?.deviceId ?? null);
  } catch {
    return null;
  }
}

/**
 * The delete dialog (shell.rs:5660-5701): "Delete session?" with the
 * curly-quote body — kept verbatim, it already matches the desktop.
 * Deleting the open chat navigates back to the list.
 */
function DeleteChatDialog({ chat, onDelete, onClose }: { chat: Chat; onDelete: () => void; onClose: () => void }) {
  const navigate = useNavigate();
  const params = useParams({ strict: false });
  const title = chat.title !== null && singleLine(chat.title).length > 0 ? singleLine(chat.title) : "New session";
  const openChatId = (params as { chatId?: string }).chatId;

  function confirm(): void {
    onDelete();
    onClose();
    if (openChatId === chat.id) {
      void navigate({ to: "/" });
    }
  }

  return (
    <Dialog ariaLabel="Delete session?" onClose={onClose}>
      <DialogCard>
        <DialogTitle>Delete session?</DialogTitle>
        <DialogBody>{`\u201C${title}\u201D will be permanently deleted. This can\u2019t be undone.`}</DialogBody>
        <div className="dialog-actions-row">
          <BtnGhost onClick={onClose}>Cancel</BtnGhost>
          <BtnDanger onClick={confirm}>Delete</BtnDanger>
        </div>
      </DialogCard>
    </Dialog>
  );
}
