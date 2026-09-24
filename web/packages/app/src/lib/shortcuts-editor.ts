import {
  comboFromKeystrokeOn,
  defaultComboOn,
  displayCombo,
  keymapGet,
  SHORTCUT_IDS,
  shortcutAvailable,
  shortcutLabel,
  type KeymapConfig,
  type ShortcutId,
} from "../state/shortcuts";

/**
 * The Shortcuts editor's pure logic — the web peer of
 * `crates/ui/src/settings/shortcuts.rs`'s editor helpers: the record
 * outcome, the reserved-combo rule, conflict detection against ticket 12's
 * keymap table, the per-row description copy, and the platform send label.
 * The default-combo table and the combo formatters themselves are ticket
 * 12's (`state/shortcuts.ts`); this module only builds the editor around
 * them.
 */

export { defaultComboOn, displayCombo, keymapGet, shortcutLabel };
export type { KeymapConfig, ShortcutId };

// ---------------------------------------------------------------------------
// Recording (shortcuts.rs:23-40 record_key)
// ---------------------------------------------------------------------------

/** One keystroke's outcome while a binding is being recorded. */
export type RecordOutcome =
  | { readonly kind: "cancelled" }
  | { readonly kind: "ignored" }
  | { readonly kind: "set"; readonly combo: string };

/**
 * `record_key` over a live DOM event: Escape cancels (the old combo stays);
 * a bare modifier keeps recording; anything else builds the stored combo via
 * ticket 12's `comboFromKeystrokeOn`. The space bar spells `space`, and the
 * platform modifier's DOM spelling ("Meta"/legacy "OS") maps to the grammar's
 * "cmd", so it reads as the bare modifier it is — matching the desktop's
 * `Keystroke` key spellings.
 */
export function recordKey(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">,
  isMac: boolean,
): RecordOutcome {
  if (event.key.toLowerCase() === "escape") {
    return { kind: "cancelled" };
  }
  let key = event.key === " " ? "space" : event.key;
  if (key.toLowerCase() === "meta" || key.toLowerCase() === "os") {
    key = "cmd";
  }
  const combo = comboFromKeystrokeOn(isMac, event.ctrlKey, event.altKey, event.shiftKey, event.metaKey, key);
  return combo === null ? { kind: "ignored" } : { kind: "set", combo };
}

// ---------------------------------------------------------------------------
// Validation (shortcuts.rs:397-409)
// ---------------------------------------------------------------------------

/** `shortcutIdEquals` — structural, so the JumpSession slots compare by slot. */
export function shortcutIdEquals(a: ShortcutId, b: ShortcutId): boolean {
  if (typeof a === "object" || typeof b === "object") {
    return (
      typeof a === "object" &&
      typeof b === "object" &&
      a.jumpSession === b.jumpSession
    );
  }
  return a === b;
}

/**
 * `conflict_owner` (shortcuts.rs:397-401): the first OTHER available id whose
 * bound combo equals this one. Unavailable ids (appshots, the embedded
 * browser's reload on web) never participate, exactly as on the desktop.
 */
export function conflictOwner(keymap: KeymapConfig, id: ShortcutId, combo: string): ShortcutId | null {
  for (const other of SHORTCUT_IDS) {
    if (shortcutIdEquals(other, id) || !shortcutAvailable(other)) {
      continue;
    }
    if (keymapGet(keymap, other) === combo) {
      return other;
    }
  }
  return null;
}

/**
 * `send_combo_is_reserved` (shortcuts.rs:403-405): Cmd/Ctrl+Enter belongs to
 * the composer on every send mode — unconditionally refused at record time.
 */
export function sendComboIsReserved(combo: string): boolean {
  return combo === "mod-enter";
}

/** The refusal notices (shortcuts.rs:228-248), interpolated with the display combo. */
export function reservedNotice(combo: string, isMac: boolean): string {
  return `${displayCombo(combo, isMac)} is reserved for the composer.`;
}

export function conflictNotice(combo: string, owner: ShortcutId, isMac: boolean): string {
  return `${displayCombo(combo, isMac)} is already assigned to ${shortcutLabel(owner)}.`;
}

// ---------------------------------------------------------------------------
// Page copy (shortcuts.rs:407-464)
// ---------------------------------------------------------------------------

/** `modifier_send_label`: macOS "⌘ Enter", else "Ctrl Enter". */
export function modifierSendLabel(isMac: boolean): string {
  return isMac ? "⌘ Enter" : "Ctrl Enter";
}

/** `description(id)` (shortcuts.rs:444-464) — one line per shortcut, verbatim. */
export function shortcutDescription(id: ShortcutId): string {
  if (typeof id === "object") {
    return "Open the session at this place in the sidebar list.";
  }
  switch (id) {
    case "captureAppshot":
      return "Capture the focused application from anywhere on your desktop.";
    case "saveFile":
      return "Save the active workspace file.";
    case "browserReload":
      return "Reload the focused browser tab.";
    case "toggleSidebar":
      return "Show or hide sessions and settings navigation.";
    case "toggleChanges":
      return "Show or hide the right sidebar for the current session.";
    case "toggleTerminal":
      return "Show or hide the terminal for the current session.";
    case "newSession":
      return "Open a blank session canvas to start a new session.";
    case "newProject":
      return "Open the new project dialog.";
    case "openModelPicker":
      return "Open the model picker for the current session.";
    case "nextSession":
      return "Select the next session in the sidebar, wrapping at the end.";
    case "prevSession":
      return "Select the previous session in the sidebar, wrapping at the start.";
    case "archiveSession":
      return "Move the current session to the archived shelf.";
  }
}
