import { describe, expect, it } from "vitest";
import type { MessagePart, SessionMessageEntry, UserInputQuestion } from "@zeron/proto";
import {
  AUTO_ADVANCE_MS,
  WIZARD_SAFETY_NET_MS,
  enterOutcome,
  escapeDismissesCompletion,
  inputRequestResolved,
  messageInputContext,
  pendingInputRequest,
  wizardCommitThenAdvance,
  wizardEscapeGoesBack,
  Wizard,
} from "../src/lib/wizard";

/**
 * The wizard + pending-input unit tests — each describe named after the
 * `composer.rs` unit test it mirrors (8384-8407, 9433-9521).
 */

function question(id: string, options: readonly string[], multi: boolean): UserInputQuestion {
  return {
    id,
    header: "Header",
    question: `Question ${id}`,
    options: [...options],
    multiSelect: multi,
  };
}

describe("enter_accepts_a_completion_before_submit_or_newline", () => {
  it("a live completion selection always wins", () => {
    expect(enterOutcome(true, "submit")).toBe("acceptCompletion");
    expect(enterOutcome(true, "newline")).toBe("acceptCompletion");
    expect(enterOutcome(false, "submit")).toBe("submit");
    expect(enterOutcome(false, "newline")).toBe("newline");
  });
});

describe("escape_consumers_keep_completion_and_wizard_priority", () => {
  it("escape dismisses a completion; the wizard's escape pages back", () => {
    expect(escapeDismissesCompletion("escape", true)).toBe(true);
    expect(escapeDismissesCompletion("escape", false)).toBe(false);
    expect(escapeDismissesCompletion("enter", true)).toBe(false);

    expect(wizardEscapeGoesBack("escape", false, false)).toBe(true);
    expect(wizardEscapeGoesBack("escape", true, true)).toBe(true);
    expect(wizardEscapeGoesBack("escape", true, false)).toBe(false);
    expect(wizardEscapeGoesBack("enter", false, true)).toBe(false);
  });
});

describe("wizard_borrows_the_generic_enter_context_only_while_active", () => {
  it("Composer while the wizard is active, MessageComposer otherwise", () => {
    expect(messageInputContext(false)).toBe("MessageComposer");
    expect(messageInputContext(true)).toBe("Composer");
  });
});

describe("wizard_single_select_auto_advances_and_completes", () => {
  it("select auto-advances; advance walks the pages then returns the answers", () => {
    const wizard = new Wizard("req", [question("q1", ["a", "b"], false), question("q2", ["x"], false)]);
    expect(wizard.counter()).toBe("1/2");
    expect(wizard.select(1).kind).toBe("autoAdvance");
    expect(wizard.isPicked(1)).toBe(true);
    expect(wizard.advance().kind).toBe("stay");
    expect(wizard.counter()).toBe("2/2");
    expect(wizard.select(0).kind).toBe("autoAdvance");
    const step = wizard.advance();
    if (step.kind !== "done") {
      throw new Error("expected Done");
    }
    expect(step.answers).toHaveLength(2);
    expect(step.answers[0]?.labels).toEqual(["b"]);
    expect(step.answers[1]?.labels).toEqual(["x"]);
  });

  it("the auto-advance dwell and the safety net carry the ticket's numbers", () => {
    expect(AUTO_ADVANCE_MS).toBe(220);
    expect(WIZARD_SAFETY_NET_MS).toBe(2000);
  });
});

describe("wizard_multi_select_toggles_and_stays", () => {
  it("multi toggles membership and submits the picked labels in order", () => {
    const wizard = new Wizard("req", [question("q", ["a", "b", "c"], true)]);
    expect(wizard.select(0).kind).toBe("stay");
    expect(wizard.select(2).kind).toBe("stay");
    expect(wizard.isPicked(0) && wizard.isPicked(2)).toBe(true);
    expect(wizard.select(0).kind).toBe("stay");
    expect(wizard.isPicked(0)).toBe(false);
    const step = wizard.advance();
    if (step.kind !== "done") {
      throw new Error("expected Done");
    }
    expect(step.answers[0]?.labels).toEqual(["c"]);
  });
});

describe("wizard_number_keys_and_bounds", () => {
  it("1-9 map to options; 0 and out-of-range stay", () => {
    const wizard = new Wizard("req", [question("q", ["a", "b"], false)]);
    expect(wizard.pressNumber(9).kind).toBe("stay");
    expect(wizard.pressNumber(0).kind).toBe("stay");
    expect(wizard.pressNumber(2).kind).toBe("autoAdvance");
    expect(wizard.isPicked(1)).toBe(true);
    expect(wizard.select(5).kind).toBe("stay");
  });
});

describe("wizard_typed_answer_overrides_and_back_pages", () => {
  it("a trimmed typed answer wins; back pages are bounded", () => {
    const wizard = new Wizard("req", [question("q1", ["a"], false), question("q2", ["x", "y"], false)]);
    wizard.select(0);
    wizard.advance();
    expect(wizard.page).toBe(1);
    expect(wizard.back()).toBe(true);
    expect(wizard.page).toBe(0);
    expect(wizard.back()).toBe(false);
    wizard.advance();
    wizard.setTyped("  custom answer  ");
    const step = wizard.advance();
    if (step.kind !== "done") {
      throw new Error("expected Done");
    }
    expect(step.answers[0]?.labels).toEqual(["a"]);
    expect(step.answers[1]?.labels).toEqual(["custom answer"]);
  });
});

/**
 * Ticket 75 §2.2.1 — the production phone explicit-advance action's commit
 * sequence (`wizardCommitThenAdvance`, the helper the composer's
 * `wizardSubmitFromInput` runs; the phone panel button and the unfocused
 * panel Enter both route through it). Bare Enter is a newline on the phone
 * layer, so these explicit paths own committing the shared draft; the
 * component additionally cancels any pending auto-advance timer before
 * invoking it, so the page moves exactly once.
 */
describe("wizard_phone_explicit_advance_commits_then_advances", () => {
  it("commits a trimmed multiline draft to the CURRENT page, then advances once — internal newlines preserved", () => {
    const wizard = new Wizard("req", [question("q1", ["a"], false), question("q2", ["x"], false)]);
    let advances = 0;
    const step = wizardCommitThenAdvance(wizard, "  line one\nline two  ", () => {
      advances += 1;
      return wizard.advance();
    });
    expect(advances).toBe(1);
    expect(step.kind).toBe("stay");
    expect(wizard.page).toBe(1);
    const answers = wizard.answers();
    expect(answers[0]?.labels).toEqual(["line one\nline two"]);
    // The commit landed before the page moved — nothing leaks forward.
    expect(answers[1]?.labels).toEqual([]);
  });

  it("a typed override wins over the picked option on the same page", () => {
    const wizard = new Wizard("req", [question("q1", ["a", "b"], false)]);
    wizard.select(1);
    const step = wizardCommitThenAdvance(wizard, "typed instead", () => wizard.advance());
    if (step.kind !== "done") {
      throw new Error("expected Done");
    }
    expect(step.answers[0]?.labels).toEqual(["typed instead"]);
  });

  it("an emptied commit clears a stale typed override, so the picked option answers", () => {
    const wizard = new Wizard("req", [question("q1", ["a", "b"], false)]);
    wizard.select(0);
    wizard.setTyped("stale override");
    const step = wizardCommitThenAdvance(wizard, "   ", () => wizard.advance());
    if (step.kind !== "done") {
      throw new Error("expected Done");
    }
    expect(step.answers[0]?.labels).toEqual(["a"]);
  });

  it("moves exactly one page even when an option auto-advance is pending", () => {
    const wizard = new Wizard("req", [question("q1", ["a", "b"], false), question("q2", ["x"], false)]);
    // The app schedules the auto-advance timer on this return value; the
    // phone explicit advance cancels it before committing.
    expect(wizard.select(1).kind).toBe("autoAdvance");
    let advances = 0;
    const step = wizardCommitThenAdvance(wizard, "", () => {
      advances += 1;
      return wizard.advance();
    });
    expect(advances).toBe(1);
    expect(step.kind).toBe("stay");
    expect(wizard.page).toBe(1);
    // The empty commit left no typed override: page one answers by option.
    expect(wizard.answers()[0]?.labels).toEqual(["b"]);
  });
});

describe("pending_input_detection", () => {
  const inputPart: MessagePart = {
    kind: "input",
    id: "in-r1",
    requestId: "r1",
    questions: [question("q", ["a"], false)],
    resolved: false,
  };
  const entry = (
    status: "streaming" | "aborted" | "complete" | null,
    parts: MessagePart[],
    id = "m",
    role: "assistant" | "user" = "assistant",
  ): SessionMessageEntry => ({
    id,
    role,
    parts,
    createdAt: 0,
    deviceId: "d",
    status,
  });
  const resolvedPart: MessagePart = {
    kind: "input",
    id: "in-r1",
    requestId: "r1",
    questions: [],
    resolved: true,
  };

  it("a streaming entry with unresolved input yields the panel", () => {
    expect(pendingInputRequest([entry("streaming", [inputPart])])?.requestId).toBe("r1");
  });

  it("a DEAD entry still gets the panel — the question stays answerable", () => {
    expect(pendingInputRequest([entry("aborted", [inputPart])])?.requestId).toBe("r1");
  });

  it("a newer assistant entry supersedes the question", () => {
    expect(
      pendingInputRequest([
        entry("aborted", [inputPart]),
        entry("complete", [{ kind: "text", id: "t2", text: "moved on" }], "m2"),
      ]),
    ).toBeNull();
  });

  it("a resolved part and an empty transcript yield nothing", () => {
    expect(pendingInputRequest([entry("streaming", [resolvedPart])])).toBeNull();
    expect(pendingInputRequest([])).toBeNull();
  });

  it("a user entry appended behind the streaming entry does not vanish it", () => {
    expect(
      pendingInputRequest([
        entry("streaming", [inputPart]),
        entry("complete", [{ kind: "text", id: "t", text: "I answered" }], "u2", "user"),
      ])?.requestId,
    ).toBe("r1");
  });

  it("the latch releases only on an explicit resolve", () => {
    const t = [entry("streaming", [inputPart])];
    expect(inputRequestResolved(t, "r1")).toBe(false);
    const resolved = [entry("streaming", [resolvedPart])];
    expect(inputRequestResolved(resolved, "r1")).toBe(true);
    expect(inputRequestResolved(resolved, "other")).toBe(false);
  });
});
