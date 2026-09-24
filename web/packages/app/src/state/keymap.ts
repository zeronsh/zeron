import { useSyncExternalStore } from "react";
import { uiSettings, type KeymapConfig } from "./ui-settings";

export type { KeymapConfig };

/**
 * The keymap's read/write surface over ticket 03's settings store. The
 * struct, defaults and healing live in `state/ui-settings.ts` (the desktop
 * persists `KeymapConfig` inside `ui-settings.json` too); this module is the
 * slice the keyboard and the Shortcuts page (ticket 29) consume — the same
 * shape the desktop's `shell.rs::apply_keymap` reads.
 *
 * Writes are immediate discrete choices (a rebind), healed synchronously by
 * the settings store's `update`, so a stored keymap is always whole.
 */

export const keymapStore = {
  /** The live keymap slice. */
  get: (): KeymapConfig => uiSettings.getSnapshot().keymap,

  /** Re-render when the keymap changes (the whole settings store notifies). */
  subscribe: (listener: () => void): (() => void) => uiSettings.subscribe(listener),

  /** Patch the keymap — the Shortcuts page's rebind and reset path. */
  update(patch: Partial<KeymapConfig>): void {
    uiSettings.updateImmediate({ keymap: { ...keymapStore.get(), ...patch } });
  },
};

/** The live keymap, re-rendered on every rebind. */
export function useKeymap(): KeymapConfig {
  return useSyncExternalStore(keymapStore.subscribe, keymapStore.get, keymapStore.get);
}

// ---------------------------------------------------------------------------
// overlay_owns_keyboard (shell.rs:3681-3683)
// ---------------------------------------------------------------------------

/**
 * Whether an overlay that owns the keyboard is up — the add-space palette
 * (ticket 11) or a composer picker popover. Session-nav shortcuts
 * (cycle/jump/archive) go quiet underneath one: on the desktop gpui runs a
 * matched binding before any `on_key_down`, so an unguarded jump would
 * switch sessions UNDER the open popover, stranding it over a session the
 * user never picked.
 *
 * Sources register by name while open; the flag is a count, so several
 * overlays cannot un-register each other.
 */
class OverlayKeyboardStore {
  readonly #sources = new Set<string>();
  readonly #listeners = new Set<() => void>();

  set(source: string, owns: boolean): void {
    const before = this.#sources.size > 0;
    if (owns) {
      this.#sources.add(source);
    } else {
      this.#sources.delete(source);
    }
    const after = this.#sources.size > 0;
    if (before !== after) {
      for (const listener of this.#listeners) {
        listener();
      }
    }
  }

  owns = (): boolean => this.#sources.size > 0;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };
}

export const overlayKeyboard = new OverlayKeyboardStore();

/** `overlay_owns_keyboard` — read live from a handler. */
export function overlayOwnsKeyboard(): boolean {
  return overlayKeyboard.owns();
}

/** The flag as a hook — the jump-hints render-time recheck reads this. */
export function useOverlayKeyboard(): boolean {
  return useSyncExternalStore(overlayKeyboard.subscribe, overlayKeyboard.owns, overlayKeyboard.owns);
}

// ---------------------------------------------------------------------------
// Keystroke interception (the recorder's cx.intercept_keystrokes)
// ---------------------------------------------------------------------------

/**
 * The Shortcuts page's recorder (`cx.intercept_keystrokes`, web-shaped):
 * while any source holds the intercept, EVERY keystroke belongs to it — the
 * shell's binding dispatch declines to run (a conflicting chord is
 * recorded/refused instead of firing its action, shortcuts.rs's
 * `recorder_refuses_bound_actions_before_they_can_run`) and the recorder's
 * own capture listener consumes the event. Registration is by name so a
 * crashed recorder cannot wedge the keyboard permanently.
 */
const keystrokeInterceptors = new Set<string>();

/** Register/unregister one interception owner; idempotent by source name. */
export function setKeystrokeIntercept(source: string, owns: boolean): void {
  if (owns) {
    keystrokeInterceptors.add(source);
  } else {
    keystrokeInterceptors.delete(source);
  }
}

/** Whether a keystroke interceptor owns the keyboard right now. */
export function keystrokesIntercepted(): boolean {
  return keystrokeInterceptors.size > 0;
}
