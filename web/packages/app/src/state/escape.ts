/**
 * The shell's Escape model — a verbatim port of the desktop's two-phase
 * handling (`shell.rs::capture_escape_surface` 5325-5369 and
 * `resolve_shell_escape` 942-971).
 *
 * **Capture phase** — the ladder below. Shell-owned surfaces (menus, dialogs,
 * the add-space palette) resolve Escape BEFORE any focused descendant — an
 * open terminal, a focused textarea — can eat the key. Some surfaces close,
 * others deliberately BLOCK without closing (they already have a Cancel
 * path). This module owns the fixed priority order; surfaces register
 * themselves so later tickets (09-11) can join without this module importing
 * them.
 *
 * **Bubble phase** — `resolveShellEscape`: if no surface consumed the key,
 * Escape interrupts the live chat, but only when the user opted in via the
 * `escapeStopsActiveAgent` client setting (default `false`).
 *
 * One capture-phase `document` listener implements the ladder
 * (`installEscapeLadder`, called once by the shell). A consumed key is
 * `stopPropagation()`d — exactly the desktop's `cx.stop_propagation()` — so no
 * descendant or bubble listener (including the shell's own interrupt pass)
 * sees it.
 */

export type EscapeOutcome =
  | { kind: "otherKey" }
  | { kind: "blocked" }
  | { kind: "ignored" }
  | { kind: "interruptChat"; chatId: string };

/** The desktop's `Indicator` — the chat's live, staleness-gated status. */
export type EscapeIndicator = "working" | "awaitingInput" | "errored" | "completed" | "idle" | null;

export interface ShellEscapeInput {
  readonly key: string;
  readonly blockingOverlay: boolean;
  readonly escapeStopsActiveAgent: boolean;
  readonly route: "chat" | "settings";
  readonly interrupting: boolean;
  readonly indicator: EscapeIndicator;
  readonly selectedChatId: string | null;
}

/**
 * `resolve_shell_escape` — the bubble-phase decision. The call site passes
 * `blockingOverlay: false` on the desktop because a blocking overlay already
 * consumed the key in capture phase; the parameter exists so the pure function
 * mirrors the Rust signature (and its `Blocked` test case).
 */
export function resolveShellEscape(input: ShellEscapeInput): EscapeOutcome {
  // Case-insensitive: the desktop's gpui events spell the key `"escape"`,
  // DOM `KeyboardEvent`s `"Escape"` — neither spelling may dodge the match.
  if (input.key.toLowerCase() !== "escape") {
    return { kind: "otherKey" };
  }
  if (input.blockingOverlay) {
    return { kind: "blocked" };
  }
  if (!input.escapeStopsActiveAgent || input.route !== "chat" || input.interrupting) {
    return { kind: "ignored" };
  }
  if (input.indicator === "working" || input.indicator === "awaitingInput") {
    return input.selectedChatId === null
      ? { kind: "ignored" }
      : { kind: "interruptChat", chatId: input.selectedChatId };
  }
  return { kind: "ignored" };
}

// ---------------------------------------------------------------------------
// The capture-phase surface registry
// ---------------------------------------------------------------------------

/**
 * The fixed ladder — `capture_escape_surface`'s check order. Lower priority
 * runs first; the first handler returning `true` consumes the key. Handlers
 * perform their own close (or deliberately nothing, to block) and return
 * `true` either way.
 *
 * Priorities 10-70 are the desktop's surfaces, in its exact order, reserved
 * for their web counterparts. `webDrawer` (12) slots the web-only
 * phone-sidebar drawer just under the blocking overlays — a judgment call,
 * since the desktop has no drawer.
 */
export const ESCAPE_PRIORITY = {
  /**
   * The command palette: close, FIRST on the ladder (the desktop checks it
   * before `capture_escape_surface`'s own ladder). (Ticket 16.)
   */
  commandPalette: 5,
  /** Delete confirms + chat/space/user menus: BLOCK, nothing closes. */
  blockingOverlay: 10,
  /** The phone sidebar drawer (web-only chrome). */
  webDrawer: 12,
  /** The rename-chat dialog: close. (Ticket 09/10.) */
  renameDialog: 20,
  /** The rename-space dialog: close. (Ticket 09/10.) */
  renameSpaceDialog: 30,
  /** The add-space palette: close. (Ticket 11.) */
  addSpace: 40,
  /** The spaces menu: close / block-while-closing. (Ticket 10.) */
  spacesMenu: 50,
  spacesMenuClosing: 51,
  /** The right pane's `+` menu: close / block-while-closing. (Ticket 07/10.) */
  rightPlus: 60,
  rightPlusClosing: 61,
  /** The active Changes surface's own `handle_escape`. (Ticket 22.) */
  changesSurface: 70,
  /**
   * The composer's queued-row edit cancel (composer.rs:7473-7483): the
   * desktop binds Escape on the composer CONTAINER and stops propagation,
   * which sits after the shell surfaces but before the bubble-phase
   * interrupt. (Ticket 13.)
   */
  composerQueueEdit: 75,
} as const;

export type EscapeSurfaceHandler = () => boolean;

interface EscapeSurface {
  readonly priority: number;
  readonly handler: EscapeSurfaceHandler;
}

class EscapeStack {
  readonly #surfaces: EscapeSurface[] = [];

  register(priority: number, handler: EscapeSurfaceHandler): () => void {
    const surface: EscapeSurface = { priority, handler };
    this.#surfaces.push(surface);
    this.#surfaces.sort((a, b) => a.priority - b.priority);
    return () => {
      const ix = this.#surfaces.indexOf(surface);
      if (ix >= 0) {
        this.#surfaces.splice(ix, 1);
      }
    };
  }

  /**
   * `capture_escape_surface`: walk the ladder in priority order; the first
   * `true` wins.
   */
  capture(): boolean {
    for (const surface of [...this.#surfaces]) {
      if (surface.handler()) {
        return true;
      }
    }
    return false;
  }
}

export const escapeStack = new EscapeStack();

/**
 * Push a surface onto the ladder. Returns its unregister function — a surface
 * that is closed should not sit on the ladder answering for other keys.
 */
export function registerEscapeSurface(priority: number, handler: EscapeSurfaceHandler): () => void {
  return escapeStack.register(priority, handler);
}

let installed = false;

/**
 * The ONE capture-phase `document` keydown listener. Idempotent; the shell
 * calls it on mount. A consumed key stops propagating, which on the web also
 * silences every bubble-phase listener (window included) — the observable
 * contract of the desktop's `stop_propagation`. The key name compares
 * case-insensitively: the desktop's gpui events spell it `"escape"`, DOM
 * `KeyboardEvent`s `"Escape"`.
 */
export function installEscapeLadder(): void {
  if (installed || typeof document === "undefined") {
    return;
  }
  installed = true;
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key.toLowerCase() !== "escape" || !escapeStack.capture()) {
        return;
      }
      event.stopPropagation();
    },
    { capture: true },
  );
}
