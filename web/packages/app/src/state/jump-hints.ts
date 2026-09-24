import { useSyncExternalStore } from "react";
import { useRouterState } from "@tanstack/react-router";
import {
  badgeCombo,
  isMacPlatform,
  jumpHintsVisible,
  modifierSendHintVisible,
  JUMP_SLOTS,
} from "./shortcuts";
import { useKeymap, useOverlayKeyboard } from "./keymap";

/**
 * The jump-hint modifier lifecycle — `shell.rs::update_jump_hints`
 * (3691-3709) and the modifiers tracking beneath it.
 *
 * There is no `modifierschanged` event on the web, so the held state is read
 * off EVERY keydown/keyup (capture phase, so a handler that stops
 * propagation cannot hide a release). Window deactivation clears it — a
 * Cmd+Tab away swallows the key-up, and without the clear the chips would
 * stick for good (gap N12) — so `blur` and `visibilitychange` both reset.
 *
 * The store holds only the RAW held modifiers; the `visible` predicate (the
 * route, the overlay guard, the exact-triple match against the keymap's jump
 * combos) is applied at render time by `useJumpHints`, so the chips drop the
 * FRAME a popover opens, not on the next modifier event
 * (`spaces.rs::render_active_rows` re-checks at render).
 */

export interface HeldModifiers {
  /** Cmd on macOS, Ctrl elsewhere. */
  readonly primary: boolean;
  readonly alt: boolean;
  readonly shift: boolean;
}

const NONE: HeldModifiers = { primary: false, alt: false, shift: false };

class JumpHintStore {
  #held: HeldModifiers = NONE;
  readonly #listeners = new Set<() => void>();

  getSnapshot = (): HeldModifiers => this.#held;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  #set(held: HeldModifiers): void {
    if (
      held.primary === this.#held.primary &&
      held.alt === this.#held.alt &&
      held.shift === this.#held.shift
    ) {
      return;
    }
    this.#held = held;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /** Read the modifiers off a live keyboard event. */
  modifiersFromEvent(event: KeyboardEvent): void {
    this.#set({
      primary: isMacPlatform() ? event.metaKey : event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
    });
  }

  /** Window deactivation: the key-up may never arrive. */
  clear(): void {
    this.#set(NONE);
  }
}

export const jumpHintStore = new JumpHintStore();

let installed = false;

/**
 * The modifier-hold listeners, installed once by the shell. Never
 * `preventDefault()` — they only observe.
 */
export function installJumpHintModifierListeners(): void {
  if (installed || typeof window === "undefined") {
    return;
  }
  installed = true;
  window.addEventListener("keydown", (event) => jumpHintStore.modifiersFromEvent(event), {
    capture: true,
  });
  window.addEventListener("keyup", (event) => jumpHintStore.modifiersFromEvent(event), {
    capture: true,
  });
  window.addEventListener("blur", () => jumpHintStore.clear());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      jumpHintStore.clear();
    }
  });
}

export interface JumpHints {
  readonly visible: boolean;
  /**
   * `queue_shortcut_revealed` — the composer's modified-submit hint is
   * revealed while the primary modifier is held by itself. Ticket 13's
   * send button consumes it.
   */
  readonly queueShortcutRevealed: boolean;
  /** `badgeCombo` per slot, in slot order; empty while hidden. */
  readonly combos: readonly string[];
}

const NO_HINTS: JumpHints = { visible: false, queueShortcutRevealed: false, combos: [] };

/**
 * The jump hints for the sidebar's first nine rows. Ticket 08's
 * `ChatListRow` consumes this: `combos[slot]` is the chip text for the row
 * at that slot in the visible order (`visibleJumpOrder`).
 */
export function useJumpHints(): JumpHints {
  const held = useSyncExternalStore(jumpHintStore.subscribe, jumpHintStore.getSnapshot);
  const keymap = useKeymap();
  const overlay = useOverlayKeyboard();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const route = pathname.startsWith("/settings") ? "settings" : "chat";
  if (route !== "chat" || overlay) {
    return NO_HINTS;
  }
  const visible = jumpHintsVisible(keymap.jumpSession, held.primary, held.alt, held.shift);
  return {
    visible,
    queueShortcutRevealed: modifierSendHintVisible(held.primary, held.alt, held.shift),
    combos: visible
      ? keymap.jumpSession.map((combo) => badgeCombo(combo, isMacPlatform()))
      : [],
  };
}

/**
 * The ordered slot → chat mapping: the first nine ids of the sidebar's
 * displayed order. `jump_to_session` reads the same order, so the chip on a
 * row always names the key that opens it.
 */
export function visibleJumpOrder(order: readonly string[]): readonly string[] {
  return order.slice(0, JUMP_SLOTS);
}
