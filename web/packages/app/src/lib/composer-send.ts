/**
 * The composer's send-path pure logic — `crates/ui/src/composer.rs`
 * §2.11–§2.13 (send_button_mode, send_blocked, composer_has_content,
 * modified_submit_target, should_publish_optimistic_echo, the interrupt
 * tracking, and the `ComposerSendBehavior` Enter bindings, composer.rs:491-599
 * and 1347-1390).
 *
 * Everything here is decision logic only; the RPCs themselves live in
 * `./composer-actions.ts` (run/queue) and `./queue-actions.ts` (the queue
 * family).
 */

import type { ComposerSendBehavior } from "../state/ui-settings";

// ---------------------------------------------------------------------------
// The send button (composer.rs:491-599)
// ---------------------------------------------------------------------------

/** `SendButtonMode` — what the send button is right now. */
export type SendButtonMode = "send" | "queue" | "stop";

/**
 * `send_button_mode` (composer.rs:573): no live run → Send; live run with
 * content → Queue; live run, nothing typed → Stop. The button_mode wrapper
 * additionally forces Send while a queued row is being edited (composer.rs:5969).
 */
export function sendButtonMode(runLive: boolean, hasText: boolean): SendButtonMode {
  if (!runLive) {
    return "send";
  }
  return hasText ? "queue" : "stop";
}

/**
 * `composer_has_content` (composer.rs:505): a staged image or a staged diff
 * comment counts — both synthesize their own prompt body, so either alone is
 * a legal send, and during a live run reads as Queue, not Stop.
 */
export function composerHasContent(text: string, attachments: number, comments: number): boolean {
  return text.trim() !== "" || attachments > 0 || comments > 0;
}

/** `send_blocked`'s four conditions (composer.rs:5944-5966). */
export interface SendBlockedConditions {
  /** Condition 1: a queued-row edit is finishing (its RPC is in flight). */
  readonly queueEditFinishing: boolean;
  /**
   * Condition 2: the engine registry exists and the selected request target
   * is missing or not connected.
   */
  readonly requestTargetDisconnected: boolean;
  /** Condition 3: review comments are flushing for this chat (ticket 23). */
  readonly reviewCommentFlushPending: boolean;
  /**
   * Condition 4: the new-chat canvas with a LOADED catalog that reports no
   * agents — offline/loading must not block.
   */
  readonly newChatNoAgents: boolean;
}

/**
 * `send_blocked` (composer.rs:5944) — true when any of the four conditions
 * holds. Desktop's Stop is never blocked (the caller checks the mode first).
 */
export function sendBlocked(conditions: SendBlockedConditions): boolean {
  return (
    conditions.queueEditFinishing ||
    conditions.requestTargetDisconnected ||
    conditions.reviewCommentFlushPending ||
    conditions.newChatNoAgents
  );
}

// ---------------------------------------------------------------------------
// Modified submit (composer.rs:509-521)
// ---------------------------------------------------------------------------

/** `ModifiedSubmitTarget` (composer.rs:510). */
export type ModifiedSubmitTarget = "submitContent" | "activateLatestQueued";

/**
 * `modified_submit_target`: Cmd/Ctrl+Enter with content submits it; with a
 * truly empty composer it activates the most recently queued row and NEVER
 * turns into Stop.
 */
export function modifiedSubmitTarget(hasContent: boolean): ModifiedSubmitTarget {
  return hasContent ? "submitContent" : "activateLatestQueued";
}

// ---------------------------------------------------------------------------
// The optimistic echo gate (composer.rs:583)
// ---------------------------------------------------------------------------

/**
 * `should_publish_optimistic_echo`: queue rows are represented by the queue
 * panel until the host promotes them into the transcript — a queued send
 * must never publish (or refresh) a local echo.
 */
export function shouldPublishOptimisticEcho(queue: boolean): boolean {
  return !queue;
}

// ---------------------------------------------------------------------------
// The send cwd (composer.rs:6433-6440)
// ---------------------------------------------------------------------------

/**
 * `resolve_send_cwd` (composer.rs:6433-6440): a NEW chat runs from the
 * picked space's path, else `"~"` — the host's home, expanded by the ENGINE
 * when the run spawns (`sessions.rs::expand_home`, 1303-1313; the web never
 * expands it client-side); an EXISTING chat runs from its stored cwd, else
 * `"."`. Blank/whitespace counts as absent (the deleted web-only guard's
 * trim rule folds in here). There is no error path — the desktop has none
 * for a projectless send.
 */
export function resolveSendCwd(
  isNew: boolean,
  spacePath: string | null | undefined,
  existingCwd: string | null | undefined,
): string {
  const path = isNew ? spacePath : existingCwd;
  if (typeof path === "string" && path.trim().length > 0) {
    return path;
  }
  return isNew ? "~" : ".";
}

// ---------------------------------------------------------------------------
// Interrupt tracking (composer.rs:587-599, 6707-6741)
// ---------------------------------------------------------------------------

/**
 * `begin_interrupt` (composer.rs:587): a set insert — idempotent per chat.
 * Returns whether the chat was newly inserted.
 */
export function beginInterrupt(pending: Set<string>, chatId: string): boolean {
  const size = pending.size;
  pending.add(chatId);
  return pending.size !== size;
}

/**
 * `retain_live_interrupts` (composer.rs:591): keeps only chats still
 * Working/AwaitingInput — the set is released only when the chat settles.
 */
export function retainLiveInterrupts(
  pending: Set<string>,
  isLive: (chatId: string) => boolean,
): void {
  for (const chatId of [...pending]) {
    if (!isLive(chatId)) {
      pending.delete(chatId);
    }
  }
}

/** `interrupt_params` (composer.rs:595) — the captured chat's interrupt command. */
export function interruptParams(chatId: string): Record<string, unknown> {
  return { chatId, command: { kind: "interrupt" } };
}

// ---------------------------------------------------------------------------
// Enter bindings (composer.rs:1347-1390)
// ---------------------------------------------------------------------------

/** `MessageEnterBindingAction` (composer.rs:1352). */
export type MessageEnterAction = "submit" | "modifiedSubmit" | "newlineOrAccept";

/** `MessageEnterBinding` (composer.rs:1358). */
export interface MessageEnterBinding {
  readonly keystroke: string;
  readonly action: MessageEnterAction;
}

/**
 * `message_enter_bindings` (composer.rs:1364): exactly two bindings — the
 * bare Enter policy from the `ComposerSendBehavior` setting plus the one
 * platform modifier combo. Never extra modifier variants; Shift+Enter is
 * always a newline (it is never listed — it is not a submit binding).
 */
export function messageEnterBindings(
  behavior: ComposerSendBehavior,
  modifierCombo: string,
): readonly MessageEnterBinding[] {
  return behavior === "enter"
    ? [
        { keystroke: "enter", action: "submit" },
        { keystroke: modifierCombo, action: "modifiedSubmit" },
      ]
    : [
        { keystroke: "enter", action: "newlineOrAccept" },
        { keystroke: modifierCombo, action: "modifiedSubmit" },
      ];
}

/**
 * The web spell of the modifier combo (the desktop's `modifiers.platform`):
 * Cmd on macOS, Ctrl elsewhere.
 */
export function platformModifierCombo(isMac: boolean): "cmd-enter" | "ctrl-enter" {
  return isMac ? "cmd-enter" : "ctrl-enter";
}

// ---------------------------------------------------------------------------
// The Enter key's resolved action (ticket 75 — the phone newline policy)
// ---------------------------------------------------------------------------

/** Everything the composer's Enter branch needs to decide (ticket 75 §2.1). */
export interface EnterKeyContext {
  /** The live phone layer (`useIsPhone`, state/media.ts: `max-width: 768px`). */
  readonly phone: boolean;
  /** An IME composition is active — the event belongs to the IME. */
  readonly composing: boolean;
  /** A completion (slash/mention) is open with a live selection. */
  readonly completionSelected: boolean;
  /** The question wizard is borrowing the shared input. */
  readonly wizardActive: boolean;
  /** The platform modifier (Ctrl or Cmd). */
  readonly mod: boolean;
  readonly alt: boolean;
  readonly shift: boolean;
  /**
   * The saved `ComposerSendBehavior` desktop preference — read, never
   * mutated; the phone policy is derived in memory per keypress.
   */
  readonly sendBehavior: ComposerSendBehavior;
}

/**
 * What an Enter press becomes. `"imeNative"` and `"nativeNewline"` leave the
 * event's default intact (the textarea performs the edit natively); every
 * other action consumes the event exactly once.
 */
export type EnterKeyAction =
  | "imeNative"
  | "acceptCompletion"
  | "nativeNewline"
  | "wizardSuppress"
  | "wizardSubmit"
  | "modifiedSubmit"
  | "submit";

/**
 * The single decision owner of the composer's Enter branch (ticket 75
 * §2.2's order): IME composition → a selected completion (accepted exactly
 * once) → a PHONE bare Enter, which is a native newline before both the
 * wizard and message submit branches, at every saved preference → the
 * wizard policy (ModifiedSubmit suppressed, bare Enter submits the page,
 * Shift/Alt stay native) → ModifiedSubmit → the saved bare-Enter
 * preference. Shift+Enter and Alt+Enter are a native newline at any width;
 * Mod+Enter's submit/activate-latest-queued target is the caller's
 * (`modifiedSubmitTarget`).
 */
export function resolveEnterAction(context: EnterKeyContext): EnterKeyAction {
  if (context.composing) {
    return "imeNative";
  }
  if (context.completionSelected) {
    return "acceptCompletion";
  }
  const bare = !context.mod && !context.alt && !context.shift;
  if (context.phone && bare) {
    return "nativeNewline";
  }
  if (context.wizardActive) {
    if (context.mod) {
      return "wizardSuppress";
    }
    return bare ? "wizardSubmit" : "nativeNewline";
  }
  if (context.mod && !context.alt) {
    return "modifiedSubmit";
  }
  if (bare && context.sendBehavior === "enter") {
    return "submit";
  }
  return "nativeNewline";
}
