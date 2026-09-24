import { describe, expect, it } from "vitest";
import { titlebarNewSessionAlpha } from "../src/state/layout";

/**
 * `titlebar_new_session_alpha` (shell.rs:193-199), the web port of the
 * desktop's `new_session_action_lives_in_the_titlebar_only_when_useful`
 * (shell.rs:8408): the `+` earns its slot only while an existing chat is
 * selected on the chat route — never on the blank canvas, never in Settings.
 */

describe("titlebarNewSessionAlpha", () => {
  it("new_session_action_lives_in_the_titlebar_only_when_useful", () => {
    // A chat is open and the route is the chat route: the `+` is live.
    expect(titlebarNewSessionAlpha(true, true)).toBe(1);
    // The blank canvas (chat route, nothing selected): no `+`.
    expect(titlebarNewSessionAlpha(true, false)).toBe(0);
    // Settings: no `+` whatever the selection state reads.
    expect(titlebarNewSessionAlpha(false, true)).toBe(0);
    expect(titlebarNewSessionAlpha(false, false)).toBe(0);
  });
});
