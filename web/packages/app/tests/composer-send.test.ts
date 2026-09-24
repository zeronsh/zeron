import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  beginInterrupt,
  composerHasContent,
  interruptParams,
  messageEnterBindings,
  modifiedSubmitTarget,
  platformModifierCombo,
  resolveEnterAction,
  resolveSendCwd,
  retainLiveInterrupts,
  sendBlocked,
  sendButtonMode,
  shouldPublishOptimisticEcho,
  type EnterKeyContext,
} from "../src/lib/composer-send";
import { buildRunRequest, sendRun, type DraftConfig } from "../src/lib/composer-actions";

/**
 * The composer's send-path decisions — each describe named after the
 * `composer.rs` unit test it mirrors: the Send/Queue/Stop button mode, the
 * content rule that keeps a comment-only stage from interrupting a live
 * run, the Mod+Enter target, the optimistic-echo gate, and the interrupt
 * tracking.
 */

class FakeCaller {
  readonly calls: { method: string; params: unknown }[] = [];
  replies: Map<string, unknown> = new Map();

  async call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    const byMethod = this.replies.get(method);
    if (byMethod !== undefined) {
      return byMethod as T;
    }
    return {} as T;
  }
}

const DRAFT: DraftConfig = {
  harness: "claude-code",
  model: "claude-3-5-sonnet",
  reasoning: "high",
  sandbox: "workspace-write",
  modelOptions: {},
};

describe("staged_comments_alone_are_content", () => {
  it("attachments and comments each count as content on their own", () => {
    expect(composerHasContent("   ", 0, 0)).toBe(false);
    expect(composerHasContent("hi", 0, 0)).toBe(true);
    expect(composerHasContent("", 1, 0)).toBe(true);
    expect(composerHasContent("", 0, 1)).toBe(true);
  });
});

describe("send_button_morph", () => {
  it("Send / Queue / Stop follow (run_live, has_text)", () => {
    expect(sendButtonMode(false, false)).toBe("send");
    expect(sendButtonMode(false, true)).toBe("send");
    expect(sendButtonMode(true, true)).toBe("queue");
    expect(sendButtonMode(true, false)).toBe("stop");
  });
});

describe("a_comment_only_stage_queues_during_a_live_run", () => {
  it("comment-only submit must queue without interrupting the run", () => {
    const live = true;
    const commentOnly = composerHasContent("", 0, 2);
    expect(sendButtonMode(live, commentOnly)).toBe("queue");
    // Nothing staged at all is still the stop square.
    expect(sendButtonMode(live, composerHasContent("", 0, 0))).toBe("stop");
  });
});

describe("sendBlocked", () => {
  it("blocks on any of the four conditions", () => {
    const open = {
      queueEditFinishing: false,
      requestTargetDisconnected: false,
      reviewCommentFlushPending: false,
      newChatNoAgents: false,
    };
    expect(sendBlocked(open)).toBe(false);
    expect(sendBlocked({ ...open, queueEditFinishing: true })).toBe(true);
    expect(sendBlocked({ ...open, requestTargetDisconnected: true })).toBe(true);
    expect(sendBlocked({ ...open, reviewCommentFlushPending: true })).toBe(true);
    expect(sendBlocked({ ...open, newChatNoAgents: true })).toBe(true);
  });
});

describe("modified_submit_sends_content_and_activates_latest_queue_row_when_empty", () => {
  it("content submits; a truly empty composer activates the latest queued row", () => {
    expect(modifiedSubmitTarget(composerHasContent("message", 0, 0))).toBe("submitContent");
    expect(modifiedSubmitTarget(composerHasContent("", 1, 0))).toBe("submitContent");
    expect(modifiedSubmitTarget(composerHasContent("", 0, 1))).toBe("submitContent");
    expect(modifiedSubmitTarget(composerHasContent("  ", 0, 0))).toBe("activateLatestQueued");
  });
});

describe("queued_submit_does_not_publish_an_optimistic_transcript_echo", () => {
  it("queue rows are the queue panel's until dispatch", () => {
    expect(shouldPublishOptimisticEcho(false)).toBe(true);
    expect(shouldPublishOptimisticEcho(true)).toBe(false);
  });
});

describe("message_enter_bindings_cover_both_platform_modifiers", () => {
  it("the setting picks the bare-Enter policy; the modifier combo is verbatim", () => {
    expect(messageEnterBindings("enter", "cmd-enter")).toEqual([
      { keystroke: "enter", action: "submit" },
      { keystroke: "cmd-enter", action: "modifiedSubmit" },
    ]);
    expect(messageEnterBindings("modEnter", "cmd-enter")).toEqual([
      { keystroke: "enter", action: "newlineOrAccept" },
      { keystroke: "cmd-enter", action: "modifiedSubmit" },
    ]);
    expect(messageEnterBindings("modEnter", "ctrl-enter")).toEqual([
      { keystroke: "enter", action: "newlineOrAccept" },
      { keystroke: "ctrl-enter", action: "modifiedSubmit" },
    ]);
    expect(platformModifierCombo(true)).toBe("cmd-enter");
    expect(platformModifierCombo(false)).toBe("ctrl-enter");
  });
});

describe("message_enter_never_adds_extra_modifier_bindings", () => {
  it("exactly two bindings, no shift/alt variants", () => {
    const bindings = messageEnterBindings("modEnter", "cmd-enter");
    expect(bindings).toHaveLength(2);
    expect(
      bindings.some((binding) =>
        ["ctrl-enter", "shift-cmd-enter", "alt-cmd-enter", "shift-enter"].includes(binding.keystroke),
      ),
    ).toBe(false);
  });
});

/**
 * Ticket 75 §3 — the phone Enter policy matrix against `resolveEnterAction`,
 * the Enter branch's single decision owner. `nativeNewline`/`imeNative`
 * leave the event's default to the textarea (the app never injects a
 * newline); every other action consumes the event exactly once. Vitest runs
 * node here: no synthetic KeyboardEvent can prove a browser's native
 * newline insertion, so these pin the DECISION only.
 */
const enterKey = (overrides: Partial<EnterKeyContext>): EnterKeyContext => ({
  phone: false,
  composing: false,
  completionSelected: false,
  wizardActive: false,
  mod: false,
  alt: false,
  shift: false,
  sendBehavior: "enter",
  ...overrides,
});

describe("phone_bare_enter_is_always_a_native_newline", () => {
  it("phone bare Enter is native at either saved preference — the setting is never overridden", () => {
    expect(resolveEnterAction(enterKey({ phone: true, sendBehavior: "enter" }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ phone: true, sendBehavior: "modEnter" }))).toBe("nativeNewline");
  });

  it("phone bare Enter in the focused wizard input is a newline — no advance, no response", () => {
    expect(resolveEnterAction(enterKey({ phone: true, wizardActive: true }))).toBe("nativeNewline");
  });

  it("phone bare Enter in a queue-edit draft is a newline — the edit stays open", () => {
    // Queue-edit state never reaches the resolver: a native newline can
    // never finish the edit; only the explicit finish/submit controls do.
    expect(resolveEnterAction(enterKey({ phone: true, sendBehavior: "enter" }))).toBe("nativeNewline");
  });

  it("phone Shift/Alt Enter stays native; Mod+Enter keeps ModifiedSubmit at any width", () => {
    expect(resolveEnterAction(enterKey({ phone: true, shift: true }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ phone: true, alt: true }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ phone: true, mod: true }))).toBe("modifiedSubmit");
    expect(resolveEnterAction(enterKey({ phone: false, mod: true }))).toBe("modifiedSubmit");
    // Alt drops the modified-submit binding (never Stop); Shift does not.
    expect(resolveEnterAction(enterKey({ phone: true, mod: true, alt: true }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ phone: true, mod: true, shift: true }))).toBe("modifiedSubmit");
  });
});

describe("phone_enter_keeps_composition_and_completion_precedence", () => {
  it("an IME composition owns the event at any width — no app action", () => {
    expect(resolveEnterAction(enterKey({ phone: true, composing: true }))).toBe("imeNative");
    expect(resolveEnterAction(enterKey({ phone: false, composing: true }))).toBe("imeNative");
  });

  it("a selected completion is accepted exactly once, before the phone newline", () => {
    // `enter_accepts_a_completion_before_submit_or_newline` (composer.rs:8384).
    expect(resolveEnterAction(enterKey({ phone: true, completionSelected: true }))).toBe("acceptCompletion");
    expect(resolveEnterAction(enterKey({ phone: false, completionSelected: true }))).toBe("acceptCompletion");
    expect(
      resolveEnterAction(enterKey({ phone: true, completionSelected: true, wizardActive: true })),
    ).toBe("acceptCompletion");
  });
});

describe("phone_wizard_enter_policy", () => {
  it("the wizard's Mod+Enter suppression is preserved at any width", () => {
    expect(resolveEnterAction(enterKey({ phone: true, wizardActive: true, mod: true }))).toBe("wizardSuppress");
    expect(resolveEnterAction(enterKey({ phone: false, wizardActive: true, mod: true }))).toBe(
      "wizardSuppress",
    );
  });

  it("desktop wizard bare Enter still submits the page; Shift/Alt stay native", () => {
    expect(resolveEnterAction(enterKey({ wizardActive: true }))).toBe("wizardSubmit");
    expect(resolveEnterAction(enterKey({ wizardActive: true, shift: true }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ wizardActive: true, alt: true }))).toBe("nativeNewline");
  });
});

describe("desktop_enter_policy_is_unchanged", () => {
  it("saved enter submits; saved modEnter inserts a newline", () => {
    expect(resolveEnterAction(enterKey({ sendBehavior: "enter" }))).toBe("submit");
    expect(resolveEnterAction(enterKey({ sendBehavior: "modEnter" }))).toBe("nativeNewline");
  });

  it("the saved preference survives a live 768↔769 media crossing", () => {
    // The policy flips with the live `phone` input while the stored setting
    // object is untouched — nothing is persisted across the boundary.
    const behavior: EnterKeyContext["sendBehavior"] = "enter";
    expect(resolveEnterAction(enterKey({ phone: true, sendBehavior: behavior }))).toBe("nativeNewline");
    expect(resolveEnterAction(enterKey({ phone: false, sendBehavior: behavior }))).toBe("submit");
    expect(resolveEnterAction(enterKey({ phone: true, sendBehavior: behavior }))).toBe("nativeNewline");
    expect(behavior).toBe("enter");
  });
});

describe("resolve_send_cwd", () => {
  // composer.rs:6433-6440: the exact rule — a NEW chat runs from the picked
  // space's path else "~"; an EXISTING chat from its stored cwd else ".".
  // There is no error path.
  it("a new chat runs from the space's path, else ~", () => {
    expect(resolveSendCwd(true, "/Users/me/proj", null)).toBe("/Users/me/proj");
    expect(resolveSendCwd(true, null, null)).toBe("~");
    expect(resolveSendCwd(true, undefined, undefined)).toBe("~");
  });

  it("an existing chat runs from its stored cwd, else .", () => {
    expect(resolveSendCwd(false, "/ignored-new-path", "/Users/me/proj")).toBe("/Users/me/proj");
    expect(resolveSendCwd(false, "/ignored-new-path", null)).toBe(".");
    expect(resolveSendCwd(false, "/ignored-new-path", undefined)).toBe(".");
  });

  it("blank and whitespace-only paths count as absent", () => {
    expect(resolveSendCwd(true, "   ", null)).toBe("~");
    expect(resolveSendCwd(false, null, "  ")).toBe(".");
  });
});

describe("projectless_composer_allows_send_and_enter_submission", () => {
  // composer.rs:8281, the web mirror: a projectless canvas send is legal —
  // it reaches the QUEUE_COMMAND step with cwd "~" and never surfaces the
  // deleted web-only working-directory failure.
  it("a projectless canvas send reaches QueueCommand with cwd ~", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });
    // The canvas stub's cwd is null with no space picked → resolveSendCwd.
    const sendCwd = resolveSendCwd(true, null, null);
    await sendRun(caller, "chat-1", DRAFT, "Hello without a project", sendCwd, {
      mintMessageId: () => "msg-1",
    });
    expect(caller.calls.map((entry) => entry.method)).toEqual(["QueueCommand"]);
    const queue = caller.calls[0]!.params as { command: { request: { cwd: string } } };
    expect(queue.command.request.cwd).toBe("~");
  });

  it("an existing chat with a blank stored cwd sends with cwd .", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueCommand", { commandId: "cmd-2" });
    await sendRun(caller, "chat-1", DRAFT, "hi", resolveSendCwd(false, null, "   "), {
      mintMessageId: () => "msg-2",
    });
    const queue = caller.calls[0]!.params as { command: { request: { cwd: string } } };
    expect(queue.command.request.cwd).toBe(".");
  });
});

describe("expand_home parity", () => {
  // §3.2: the engine at sessions.rs:1303-1313 is authoritative — the web
  // only ever sends the LITERAL "~" and "." (never expands client-side).
  it("buildRunRequest carries cwd ~ verbatim", () => {
    expect(buildRunRequest(DRAFT, "hi", "~").cwd).toBe("~");
    expect(buildRunRequest(DRAFT, "hi", ".").cwd).toBe(".");
  });
});

describe("interrupt_tracking_is_idempotent_per_chat", () => {
  it("a second Stop for the same chat is a no-op", () => {
    const pending = new Set<string>();
    expect(beginInterrupt(pending, "chat-a")).toBe(true);
    expect(beginInterrupt(pending, "chat-a")).toBe(false);
    expect(beginInterrupt(pending, "chat-b")).toBe(true);
    expect(pending.size).toBe(2);
  });
});

describe("interrupt_tracking_releases_only_settled_chats", () => {
  it("the set retains chats still live and frees the settled one", () => {
    const pending = new Set<string>(["chat-a", "chat-b"]);
    retainLiveInterrupts(pending, (chatId) => chatId === "chat-b");
    expect([...pending]).toEqual(["chat-b"]);
    expect(beginInterrupt(pending, "chat-a")).toBe(true);
  });
});

describe("interrupt_payload_keeps_the_captured_chat", () => {
  it("the params carry the chat id and the interrupt command", () => {
    const params = interruptParams("chat-a");
    expect(params["chatId"]).toBe("chat-a");
    expect((params["command"] as Record<string, unknown>)["kind"]).toBe("interrupt");
  });
});

// ---------------------------------------------------------------------------
// enter_on_empty_composer_during_a_live_run_never_interrupts (composer.rs
// on_submit, ported � issue #406)
// ---------------------------------------------------------------------------

describe("enter_on_empty_composer_during_a_live_run_never_interrupts", () => {
  it("the double-Enter window reads as the Stop square", () => {
    // A live run with a truly empty composer � the moment right after the
    // habitual extra Enter � resolves to Stop, never Send/Queue.
    expect(sendButtonMode(true, composerHasContent("", 0, 0))).toBe("stop");
  });

  it("the wiring pin: submit's Stop arm is a no-op, not an interrupt", () => {
    // Stop stays on the button and on Esc's setting; Enter must never
    // dispatch an interrupt (the composer-reasoning suite's source-pin
    // idiom: a reverted submit arm cannot pass unnoticed).
    const source = readFileSync(join(process.cwd(), "src/components/composer.tsx"), "utf8");
    const submit = /const submit = useCallback\(async \(\) => \{([\s\S]*?)await send\(/.exec(source);
    if (submit === null) {
      throw new Error("submit callback not found");
    }
    const stopArm = /if \(mode === "stop"\) \{([\s\S]*?)return;/.exec(submit[1]!);
    if (stopArm === null) {
      throw new Error("submit stop arm not found");
    }
    expect(stopArm[1]!).not.toMatch(/\binterrupt\(/);
    // The button keeps its own Stop path � the only click that interrupts.
    expect(source).toContain(`onClick={() => (mode === "stop" ? void interrupt() : void submit())}`);
  });
});
