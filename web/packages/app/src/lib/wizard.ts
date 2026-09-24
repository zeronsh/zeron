import type { MessagePart, SessionMessageEntry, UserInputAnswer, UserInputQuestion } from "@zeron/proto";

/**
 * The question-wizard library — a port of the desktop's composer.rs wizard
 * core: the `Wizard` reducer (`:676-802`), `pending_input_request`
 * (`:622`), `input_request_resolved` (`:644`), `enter_outcome` (`:1407`),
 * `escape_dismisses_completion` (`:602`), `wizard_escape_goes_back`
 * (`:606`), and `message_input_context` (`:1392`). The key contexts
 * ("Composer" while the wizard borrows the input, "MessageComposer"
 * otherwise) are names only on the web — the textarea's two Enter policies
 * are what they select — but the pure mapping stays testable exactly as on
 * the desktop.
 */

/** `AUTO_ADVANCE_MS` (composer.rs:103): single-select auto-advance dwell. */
export const AUTO_ADVANCE_MS = 220;

/** The wizard latch's post-queue safety net (composer.rs:6849). */
export const WIZARD_SAFETY_NET_MS = 2000;

/** The generic context the input borrows while the wizard is active. */
export const GENERIC_COMPOSER_CONTEXT = "Composer";
/** The message composer's own context. */
export const MESSAGE_COMPOSER_CONTEXT = "MessageComposer";

/** `WizardStep` (composer.rs:665-671). */
export type WizardStep =
  | { readonly kind: "stay" }
  | { readonly kind: "autoAdvance" }
  | { readonly kind: "done"; readonly answers: UserInputAnswer[] };

/**
 * Paged question state ("1/3") — `Wizard` (composer.rs:676-802). Single
 * select auto-advances, multi-select and typed answers advance explicitly,
 * number keys 1-9 select, Back pages back. Mutating: the React host keeps
 * one instance and bumps a render generation per mutation.
 */
export class Wizard {
  readonly requestId: string;
  readonly questions: UserInputQuestion[];
  page = 0;
  readonly #picked: number[][];
  readonly #typed: string[];

  constructor(requestId: string, questions: UserInputQuestion[]) {
    this.requestId = requestId;
    this.questions = questions;
    this.#picked = questions.map(() => []);
    this.#typed = questions.map(() => "");
  }

  /** `counter()` (composer.rs:696): `"{page+1}/{max(len,1)}"`. */
  counter(): string {
    return `${this.page + 1}/${Math.max(this.questions.length, 1)}`;
  }

  current(): UserInputQuestion | null {
    return this.questions[this.page] ?? null;
  }

  isPicked(optionIx: number): boolean {
    return this.#picked[this.page]?.includes(optionIx) === true;
  }

  /** Whether the current page has any picked option (composer.rs:711). */
  pageHasPick(): boolean {
    const picked = this.#picked[this.page];
    return picked !== undefined && picked.length > 0;
  }

  /** `select` (composer.rs:716): multi toggles and stays; single replaces
   * the pick and auto-advances. Out-of-range options are ignored. */
  select(optionIx: number): WizardStep {
    const question = this.questions[this.page];
    if (question === undefined || optionIx >= question.options.length) {
      return { kind: "stay" };
    }
    const picked = this.#picked[this.page]!;
    if (question.multiSelect) {
      const at = picked.indexOf(optionIx);
      if (at >= 0) {
        picked.splice(at, 1);
      } else {
        picked.push(optionIx);
      }
      return { kind: "stay" };
    }
    this.#picked[this.page] = [optionIx];
    return { kind: "autoAdvance" };
  }

  /** `press_number` (composer.rs:742): `1`-`9` map to options; `0` and
   * out-of-range stay. */
  pressNumber(number: number): WizardStep {
    if (number === 0) {
      return { kind: "stay" };
    }
    return this.select(number - 1);
  }

  /** `set_typed` (composer.rs:749): per-page free text. */
  setTyped(text: string): void {
    const slot = this.#typed[this.page];
    if (slot !== undefined) {
      this.#typed[this.page] = text;
    }
  }

  /** `advance` (composer.rs:756): the next page, or the completed answers. */
  advance(): WizardStep {
    if (this.page + 1 < this.questions.length) {
      this.page += 1;
      return { kind: "stay" };
    }
    return { kind: "done", answers: this.answers() };
  }

  /** `back` (composer.rs:766): false when already on the first page. */
  back(): boolean {
    if (this.page > 0) {
      this.page -= 1;
      return true;
    }
    return false;
  }

  /** `answers` (composer.rs:776): per question, a trimmed typed answer wins
   * as a single label; otherwise the picked option labels in pick order. */
  answers(): UserInputAnswer[] {
    return this.questions.map((question, ix) => {
      const typed = (this.#typed[ix] ?? "").trim();
      const labels =
        typed.length > 0
          ? [typed]
          : (this.#picked[ix] ?? [])
              .map((pick) => question.options[pick])
              .filter((label): label is string => label !== undefined);
      return { questionId: question.id, labels };
    });
  }
}

/**
 * `pending_input_request` (composer.rs:622): the LAST assistant entry's
 * first unresolved input part, regardless of that entry's run status — a
 * dead run's question stays answerable until answered. Assistant-entry
 * scoped, not last-entry scoped: a steer prompt appended behind the
 * streaming entry must not vanish the panel.
 */
export function pendingInputRequest(
  entries: readonly SessionMessageEntry[],
): { requestId: string; questions: UserInputQuestion[] } | null {
  for (let ix = entries.length - 1; ix >= 0; ix -= 1) {
    if (entries[ix]!.role !== "assistant") {
      continue;
    }
    for (const part of entries[ix]!.parts) {
      if (part.kind === "input" && !part.resolved) {
        return { requestId: part.requestId, questions: part.questions };
      }
    }
    return null;
  }
  return null;
}

/**
 * `input_request_resolved` (composer.rs:644): any entry carrying the
 * request id with `resolved: true` — the latch's release condition.
 */
export function inputRequestResolved(
  entries: readonly SessionMessageEntry[],
  requestId: string,
): boolean {
  return entries.some((entry) =>
    entry.parts.some((part: MessagePart) => part.kind === "input" && part.resolved && part.requestId === requestId),
  );
}

/**
 * The commit-then-advance sequence of the wizard's borrowed-input Enter
 * (composer.rs:5984-5992's `on_submit` wizard arm) — and, on the phone
 * layer, of the explicit advance button and unfocused panel Enter (ticket
 * 75 §2.2.1): the trimmed text becomes the CURRENT page's typed answer,
 * committed even when EMPTY so a stale typed override cannot leak into an
 * option-only answer, and only then does the host's advance run — exactly
 * once per call. Internal newlines are preserved (`answers` trims only the
 * edges). The host's advance owns the done/stay tail (finish, render tick,
 * input clear); the phone caller additionally cancels any pending option
 * auto-advance timer first so the page moves once.
 */
export function wizardCommitThenAdvance<T>(wizard: Wizard, typedText: string, advance: () => T): T {
  wizard.setTyped(typedText.trim());
  return advance();
}

/**
 * `enter_outcome` (composer.rs:1407): a live completion selection always
 * wins over the send-behavior fallback.
 */
export function enterOutcome(
  hasCompletion: boolean,
  fallback: "submit" | "newline",
): "acceptCompletion" | "submit" | "newline" {
  return hasCompletion ? "acceptCompletion" : fallback;
}

/** `escape_dismisses_completion` (composer.rs:602). */
export function escapeDismissesCompletion(key: string, completionOpen: boolean): boolean {
  return key.toLowerCase() === "escape" && completionOpen;
}

/** `wizard_escape_goes_back` (composer.rs:606). */
export function wizardEscapeGoesBack(key: string, inputFocused: boolean, inputEmpty: boolean): boolean {
  return key.toLowerCase() === "escape" && (!inputFocused || inputEmpty);
}

/**
 * `message_input_context` (composer.rs:1392): the input borrows the generic
 * `"Composer"` context (bare-Enter-submits) only while the wizard is active.
 */
export function messageInputContext(wizardActive: boolean): "Composer" | "MessageComposer" {
  return wizardActive ? GENERIC_COMPOSER_CONTEXT : MESSAGE_COMPOSER_CONTEXT;
}

/**
 * The shared input's placeholder while the wizard is mounted (the
 * "nothing picked" / "an option picked" pair, composer.rs:6751-6760).
 */
export function wizardPlaceholder(pageHasPick: boolean): string {
  return pageHasPick
    ? "Type your own answer, or leave this blank to use the selected option"
    : "Type your own answer, or pick an option above";
}

/** The composer's rest placeholder, restored when the wizard finishes. */
export const COMPOSER_REST_PLACEHOLDER = "Do anything…";
