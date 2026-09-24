import { describe, expect, it } from "vitest";
import { resolveShellEscape, type ShellEscapeInput } from "../src/state/escape";

/**
 * `resolve_shell_escape` (shell.rs:942-971) — the bubble-phase half of the
 * shell's Escape model. These three suites mirror the desktop's tests
 * (shell.rs:8300, 8328, 8380):
 *
 * - the interrupt fires only for the live chat under the pointer of history
 *   (selected, on the chat route, Working or AwaitingInput);
 * - every other view — blank canvas, Settings, no selection, an interrupt
 *   already in flight, a non-Escape key — resolves to Ignored/OtherKey;
 * - the whole behaviour is opt-in through `escape_stops_active_agent`
 *   (default false: Escape does not stop the agent).
 */

function input(over: Partial<ShellEscapeInput> = {}): ShellEscapeInput {
  return {
    // The DOM's spelling — real `KeyboardEvent.key` is `"Escape"`.
    key: "Escape",
    blockingOverlay: false,
    escapeStopsActiveAgent: true,
    route: "chat",
    interrupting: false,
    indicator: "working",
    selectedChatId: "chat-1",
    ...over,
  };
}

describe("resolveShellEscape", () => {
  it("matches the key case-insensitively (DOM \"Escape\", gpui \"escape\")", () => {
    // The DOM spelling interrupts…
    expect(resolveShellEscape(input({ key: "Escape" }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
    // …and so does the desktop's lowercase spelling — the comparison is
    // case-insensitive, never an exact match on one variant.
    expect(resolveShellEscape(input({ key: "escape" }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
    expect(resolveShellEscape(input({ key: "ESCAPE" }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
  });

  it("escape_interrupts_only_the_active_live_chat", () => {
    // Working and AwaitingInput are the live states an Escape can stop.
    expect(resolveShellEscape(input({ indicator: "working" }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
    expect(resolveShellEscape(input({ indicator: "awaitingInput" }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
    // Everything else is not live: completed, errored, idle, unknown.
    expect(resolveShellEscape(input({ indicator: "completed" }))).toEqual({ kind: "ignored" });
    expect(resolveShellEscape(input({ indicator: "errored" }))).toEqual({ kind: "ignored" });
    expect(resolveShellEscape(input({ indicator: "idle" }))).toEqual({ kind: "ignored" });
    expect(resolveShellEscape(input({ indicator: null }))).toEqual({ kind: "ignored" });
    // A live chat with no selection has nothing to interrupt.
    expect(resolveShellEscape(input({ selectedChatId: null }))).toEqual({ kind: "ignored" });
  });

  it("escape_ignores_non_live_or_ineligible_views", () => {
    // Not the chat route: the pane's own surfaces own Escape there.
    expect(resolveShellEscape(input({ route: "settings" }))).toEqual({ kind: "ignored" });
    // An interrupt is already on the wire for this chat — a second Escape
    // must not stack a second Stop.
    expect(resolveShellEscape(input({ interrupting: true }))).toEqual({ kind: "ignored" });
    // A blocking overlay consumed the key in capture phase already; the
    // bubble pass sees it only as Blocked (stop propagation, do nothing).
    expect(resolveShellEscape(input({ blockingOverlay: true }))).toEqual({ kind: "blocked" });
    // Any other key falls through untouched.
    expect(resolveShellEscape(input({ key: "tab" }))).toEqual({ kind: "otherKey" });
    expect(resolveShellEscape(input({ key: "enter" }))).toEqual({ kind: "otherKey" });
  });

  it("escape_interrupt_is_opt_in", () => {
    // `escape_stops_active_agent` defaults to false: plain Escape leaves a
    // working chat alone until the user turns the setting on.
    expect(resolveShellEscape(input({ escapeStopsActiveAgent: false }))).toEqual({
      kind: "ignored",
    });
    // Opted in, the same view interrupts.
    expect(resolveShellEscape(input({ escapeStopsActiveAgent: true }))).toEqual({
      kind: "interruptChat",
      chatId: "chat-1",
    });
    // The setting gates even a live, selected, working chat.
    expect(
      resolveShellEscape(
        input({ escapeStopsActiveAgent: false, indicator: "awaitingInput", selectedChatId: "chat-2" }),
      ),
    ).toEqual({ kind: "ignored" });
  });
});
