import type { ChatConfig, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, WorktreeSpec } from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import type { EngineClient } from "@zeron/engine-client";
import { describeMutateError } from "./chat-actions";
import type { StagedAttachment, UploadedAttachment } from "./attachments";
import { uploadAttachments, withAttachments } from "./attachments";
import { withComments, type ReviewComment } from "./review-comments";
import { queueMessage as queueMessageRpc } from "./queue-actions";
import { mintId } from "./id";

/**
 * The composer's working draft — what the user has picked for the next send.
 * Mirrors `ChatConfig` plus the `modelOptions` field the harness options picker
 * mutates separately. The desktop calls this `DraftConfig` (`crates/ui/src/pickers.rs`).
 *
 * `harness` is locked once a chat has a non-null `ChatConfig`; the picker
 * chip dims for existing chats. The web keeps the same UX: when the chat row
 * already carries a `ChatConfig`, harness changes are gated behind the user
 * opening the harness picker explicitly (the desktop's picker UI greys the
 * rail — for the web v1 we just render the picker inert).
 */
export interface DraftConfig {
  readonly harness: HarnessId;
  readonly model: string | null;
  readonly reasoning: ReasoningLevel | null;
  readonly sandbox: SandboxLevel;
  readonly modelOptions: Readonly<Record<string, unknown>>;
}

/**
 * What a picker may change. `sandbox` is deliberately absent: the desktop
 * writes `SandboxLevel::WorkspaceWrite` when the chat is created and preserves
 * it thereafter — it is never a user choice, so no update can carry it.
 */
export interface DraftConfigUpdate {
  harness?: HarnessId;
  model?: string | null;
  reasoning?: ReasoningLevel | null;
  modelOptions?: Record<string, unknown>;
}

export function buildChatConfig(draft: DraftConfig): ChatConfig {
  return {
    harness: draft.harness,
    model: draft.model,
    reasoning: draft.reasoning,
    sandbox: draft.sandbox,
    modelOptions: { ...draft.modelOptions },
  };
}

/**
 * Shape of a Run payload the engine accepts (the wire's `RunRequest`).
 *
 * The message id is deliberately NOT a parameter here: `RunRequest` carries no
 * id field on the wire (`crates/proto/src/agent.rs:94-128`). The id rides the
 * command envelope — `SessionCommandPayload::Run { request, message_id }`
 * (`crates/doc/src/commands.rs:42-46`) — and that is the id the host writes the
 * user entry under (`doc_host.rs:326-349`), so it is the only one worth
 * threading. This function used to take a `messageId` it never read, which is
 * what hid the three-ids bug in `sendRun` below.
 */
export function buildRunRequest(
  draft: DraftConfig,
  prompt: string,
  cwd: string,
  attachments: readonly string[] = [],
  worktree: WorktreeSpec | null = null,
): RunRequest {
  const request: RunRequest = {
    prompt,
    harness: draft.harness,
    model: draft.model,
    reasoning: draft.reasoning,
    modelOptions: { ...draft.modelOptions },
    cwd,
    sandbox: draft.sandbox,
    autoApprove: false,
    resume: null,
    attachments: [...attachments],
    worktree,
  };
  return request;
}

/** The minimal caller shape — `EngineClient` satisfies it. */
export interface CommandCaller {
  call<T>(method: string, params?: unknown): Promise<T>;
}

/** Result of a successful Send: the message id the engine will claim for the user bubble. */
export interface SendResult {
  readonly messageId: string;
  readonly commandId: string;
  /** The host-resolved paths the engine reports for each uploaded attachment
   *  (in send order). The web mirror of the desktop's `attachment_paths` —
   *  exposed so the strip can hand them to `seedAttachment` for instant
   *  bubble rendering. */
  readonly attachmentPaths: readonly string[];
  /** The prompt as sent (the typed body with the attachment refs folded in) —
   *  the caller refreshes its optimistic echo in place with this after the
   *  upload (`refreshed`, composer.rs:6401-6423). */
  readonly finalPrompt: string;
}

/** Optional inputs for send. `stagedAttachments` is the bytes the user
 *  dropped/picked into the composer — the sender uploads them, embeds the
 *  refs in the prompt, and populates `transfers` so the chat's host device
 *  knows about them. `uploadProgress` is called per-chunk.
 *  `reviewComments` is the staged comment set, folded into the prompt as
 *  plain text (the ONLY transport — there is no structured wire field,
 *  composer.rs:6130-6137). */
export interface SendAttachmentsOptions {
  readonly stagedAttachments?: readonly StagedAttachment[];
  readonly uploadProgress?: (uploadedBytes: number, totalBytes: number) => void;
  readonly stagedReviewComments?: readonly ReviewComment[];
}

/** Mint a client-side message id (the optimistic-echo dedupe key). */
export function mintMessageId(mint: () => string = mintId): string {
  return mint();
}

/**
 * Send a message to the harness: one `QueueCommand` with a Run payload.
 * Returns the message id the engine will claim for the user bubble.
 *
 * No `Mutate setChatConfig` rides ahead of it. The desktop carries
 * model/reasoning/options ON the `RunRequest` itself (which `buildRunRequest`
 * already does); only a genuinely NEW chat persists a `ChatConfig`, via
 * `Mutate createChat`.
 *
 * When `stagedAttachments` is non-empty, the caller uploads the bytes to
 * the chat's host device first, folds the returned paths into the prompt
 * (via [`withAttachments`]), and ships the upload identity as
 * `transfers` on the `QueueCommand` call. The engine rewrites the
 * `pending://` refs to durable paths on dispatch.
 */
export async function sendRun(
  caller: CommandCaller,
  chatId: string,
  draft: DraftConfig,
  prompt: string,
  chatCwd: string,
  options: { mintMessageId?: () => string } = {},
  attachments: SendAttachmentsOptions = {},
): Promise<SendResult> {
  const trimmed = prompt.trim();
  const staged = attachments.stagedAttachments ?? [];
  const comments = attachments.stagedReviewComments ?? [];
  const hasContent = trimmed.length > 0 || staged.length > 0 || comments.length > 0;
  if (!hasContent) {
    throw new Error("Cannot send an empty message");
  }
  // `chatCwd` is the RESOLVED send cwd (composer.rs:6433-6440, resolved by
  // the caller through `resolveSendCwd`): a NEW chat's space path else `"~"`
  // — delivered literally, expanded by the engine host-side
  // (sessions.rs:342-352) — or an existing chat's stored cwd else `"."`.
  // There is no error path for a projectless send; "~"/"." are legal wire
  // cwd values.
  // ONE id per send, minted once and stored. It is the dedupe key shared by
  // the command envelope, the entry the host writes back, the caller's
  // optimistic echo and any failure cleanup — three separate `messageId()`
  // calls used to produce three unrelated uuids, so nothing downstream could
  // ever say "this specific sent message".
  const messageId = (options.mintMessageId ?? mintId)();
  const uploaded: readonly UploadedAttachment[] = await uploadStage(
    caller,
    staged,
    attachments.uploadProgress,
  );
  // The comment block folds in BEFORE the attachment trailer (composer.rs:
  // 6137 with_comments, then 6393 with_attachments wraps it) — the
  // transcript strips the attachment refs first, so the comment block is
  // still the trailing block the badge extractor matches.
  const finalPrompt = withAttachments(
    withComments(trimmed, comments),
    uploaded.map((entry) => entry.path),
  );
  const command = {
    kind: "run" as const,
    request: buildRunRequest(
      draft,
      finalPrompt,
      chatCwd,
      uploaded.map((entry) => entry.path),
      null,
    ),
    messageId,
  };
  const reply = (await caller.call(methods.QUEUE_COMMAND, {
    chatId,
    command,
    transfers: uploaded.map((entry) => ({ uploadId: entry.uploadId, fileName: entry.fileName })),
  })) as { commandId: string };
  return {
    messageId,
    commandId: reply.commandId,
    attachmentPaths: uploaded.map((entry) => entry.path),
    finalPrompt,
  };
}

/**
 * The composer's Queue path (`Composer::send` with `queue: true`,
 * composer.rs:6547-6577): `methods::QUEUE_MESSAGE` with
 * `{chatId, text, attachments, holdForTurnEnd: true}`. The reply's `id` is
 * the queue row id; a missing id raises the verbatim failure
 * "Send failed: queue did not return an id".
 *
 * The queue row's text stays FREE of the attachment-path trailer (the host
 * rebuilds that transport when it promotes the row) — the caller passes the
 * typed body (or `ATTACHMENT_ONLY_TEXT` for an image-only send) and the
 * uploaded absolute paths.
 */
export async function queueMessage(
  caller: CommandCaller,
  chatId: string,
  text: string,
  attachments: readonly string[] = [],
): Promise<string> {
  let id: string;
  try {
    id = await queueMessageRpc(caller, chatId, text, {
      attachments,
      holdForTurnEnd: true,
    });
  } catch (error) {
    throw new Error(`Send failed: ${describeMutateError(error)}`);
  }
  if (typeof id !== "string" || id.length === 0) {
    throw new Error("Send failed: queue did not return an id");
  }
  return id;
}

/** Upload staged attachments to the chat's host device. Returns `[]`
 *  for an empty stage (the common case). */
async function uploadStage(
  caller: CommandCaller,
  staged: readonly StagedAttachment[],
  progress: ((uploaded: number, total: number) => void) | undefined,
): Promise<readonly UploadedAttachment[]> {
  if (staged.length === 0) {
    return [];
  }
  return uploadAttachments(caller, staged, progress ?? null);
}

/**
 * Interrupt the live run (composer.rs:6707-6737). No payload beyond the
 * captured chat id — the engine knows what to stop. Callers track
 * idempotency per chat through `lib/composer-send.ts`'s
 * `beginInterrupt`/`retainLiveInterrupts` and build the params with its
 * `interruptParams`.
 */
export async function sendInterrupt(caller: CommandCaller, chatId: string): Promise<void> {
  await caller.call(methods.QUEUE_COMMAND, {
    chatId,
    command: { kind: "interrupt" },
    transfers: [],
  });
}

/**
 * The composer's only place where ChatConfig drift lands on the server — every
 * mutation flows through here, so chip updates and chat-row repaints stay in
 * sync. Only a picker's mid-session change persists through here; a send
 * never does (the desktop sends model/reasoning/options on the `RunRequest`
 * itself, and only a NEW chat writes config, via `Mutate createChat`).
 */
export async function persistChatConfig(
  caller: CommandCaller,
  chatId: string,
  draft: DraftConfig,
): Promise<void> {
  await caller.call(methods.MUTATE, {
    op: "setChatConfig",
    chatId,
    config: buildChatConfig(draft),
  });
}

/** A session-scoped caller shape — `EngineClient` matches. */
export type { EngineClient };

/** User-facing mutation failure copy (mirrors chat-actions describeMutateError). */
export const describeSendError = describeMutateError;
