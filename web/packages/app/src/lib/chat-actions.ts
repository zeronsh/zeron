import { methods } from "@zeron/engine-client";
import type { WatchCacheSnapshot } from "@zeron/engine-client";
import type { ChatConfig } from "@zeron/proto";
import { mintId } from "./id";

/**
 * The chat-management surface of the `Mutate` RPC (crates/ui/src/shell.rs
 * sidebar mutations): create, rename, archive, delete. Every op lands on
 * the connected engine; failures surface in the sidebar notice strip, so
 * callers only get a rejected promise to describe.
 *
 * Functions take the minimal caller shape so tests drive them without a
 * socket; `EngineClient` satisfies it.
 */

export interface MutateCaller {
  call<T>(method: string, params?: unknown): Promise<T>;
}

/** Where a new chat lands: a space fixes host device + cwd; else the device. */
export interface CreateChatTarget {
  readonly spaceId?: string;
  readonly deviceId?: string;
}

export interface CreateChatOptions extends CreateChatTarget {
  /** Id factory — client-minted like the desktop's `Uuid::new_v4`. */
  readonly mintId?: () => string;
  /**
   * The resolved draft config (composer.rs:6528-6533): only a genuinely NEW
   * chat writes one; the field is inserted only when present.
   */
  readonly config?: ChatConfig;
  /**
   * The checkout plan's picked ref (composer.rs:6523-6527): inserted only
   * when present. The engine stamps it as the chat's branch.
   */
  readonly branch?: string;
  /**
   * A worktree-reuse path (composer.rs:6515-6521) — inserted only when
   * present. The projectless `"~"` NEVER rides here: it lives on the
   * `RunRequest`, where the engine expands it host-side.
   */
  readonly cwd?: string;
}

/**
 * Create a chat and return its id. The engine writes the row immediately
 * (workspace_host create_chat is idempotent — a retry never duplicates).
 * `spaceId`/`deviceId`/`cwd`/`branch`/`config` are inserted only when
 * present, exactly like the desktop's `Mutate` assembly (composer.rs:6494-6544).
 */
export async function createChat(caller: MutateCaller, options: CreateChatOptions = {}): Promise<string> {
  const chatId = (options.mintId ?? mintId)();
  await caller.call(methods.MUTATE, {
    op: "createChat",
    chatId,
    ...(options.spaceId !== undefined ? { spaceId: options.spaceId } : {}),
    ...(options.spaceId === undefined && options.deviceId !== undefined ? { deviceId: options.deviceId } : {}),
    ...(options.cwd !== undefined ? { cwd: options.cwd } : {}),
    ...(options.branch !== undefined ? { branch: options.branch } : {}),
    ...(options.config !== undefined ? { config: options.config } : {}),
  });
  return chatId;
}

/**
 * Rename a chat. An empty (whitespace-only) title is a no-op, mirroring the
 * desktop's submit_rename_chat. Returns whether a mutation was sent.
 */
export async function renameChat(caller: MutateCaller, chatId: string, title: string): Promise<boolean> {
  const trimmed = title.trim();
  if (trimmed.length === 0) {
    return false;
  }
  await caller.call(methods.MUTATE, { op: "renameChat", chatId, title: trimmed });
  return true;
}

/** Archive or unarchive a chat. Archiving never closes an open chat. */
export async function setChatArchived(caller: MutateCaller, chatId: string, archived: boolean): Promise<void> {
  await caller.call(methods.MUTATE, { op: "setChatArchived", chatId, archived });
}

/** Permanently delete a chat. */
export async function deleteChat(caller: MutateCaller, chatId: string): Promise<void> {
  await caller.call(methods.MUTATE, { op: "deleteChat", chatId });
}

/**
 * The local half of "this chat has been read" — the optimistic stamp the
 * desktop's `mark_chat_seen` writes before it touches the wire. Keyed by chat
 * id, epoch-ms. Module-scoped like the desktop's `AppState` field: it has to
 * outlive whichever chat page happens to be mounted.
 */
const seenAt = new Map<string, number>();

/** The optimistic `lastSeenAt` for a chat, or null if it has not been stamped. */
export function chatSeenAt(chatId: string): number | null {
  return seenAt.get(chatId) ?? null;
}

/**
 * Mark a chat read (`state.rs`'s `mark_chat_seen`).
 *
 * Idempotent — a chat already stamped at or after `nowMs` is a no-op, so the
 * repeated calls a chat page makes while it is open cost nothing. The stamp
 * lands LOCALLY first and is never rolled back: the `Mutate` is
 * fire-and-forget, and a dropped mutation must not make a chat the user
 * plainly looked at flash unread again. Returns whether a mutation was sent.
 */
export function markChatSeen(caller: MutateCaller, chatId: string, nowMs = Date.now()): boolean {
  const previous = seenAt.get(chatId);
  if (previous !== undefined && previous >= nowMs) {
    return false;
  }
  seenAt.set(chatId, nowMs);
  void caller.call(methods.MUTATE, { op: "markChatSeen", chatId }).catch(() => {
    // Fire-and-forget: the optimistic stamp stands either way.
  });
  return true;
}

/** Test seam — drops every optimistic seen stamp. */
export function resetChatSeen(): void {
  seenAt.clear();
}

/** The notice-strip text for a failed mutation (desktop shows `{err}`). */
export function describeMutateError(error: unknown): string {
  return error instanceof Error ? error.message : "The change could not be applied.";
}

interface ChatRowSource {
  getSnapshot(): WatchCacheSnapshot;
  subscribe(listener: () => void): () => void;
}

/**
 * Best-effort wait for a freshly created chat to arrive on the watch
 * stream before navigating to it, so the chat page never flashes its
 * not-found state. Resolves false on timeout — the caller navigates anyway.
 */
export function waitForChatRow(cache: ChatRowSource, chatId: string, timeoutMs = 5_000): Promise<boolean> {
  if (cache.getSnapshot().chats.rows.some((row) => row.id === chatId)) {
    return Promise.resolve(true);
  }
  return new Promise((resolve) => {
    const unsubscribe = cache.subscribe(() => {
      if (cache.getSnapshot().chats.rows.some((row) => row.id === chatId)) {
        finish(true);
      }
    });
    const timer = setTimeout(() => finish(false), timeoutMs);
    function finish(found: boolean): void {
      clearTimeout(timer);
      unsubscribe();
      resolve(found);
    }
  });
}
