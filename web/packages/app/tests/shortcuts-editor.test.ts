import { describe, expect, it } from "vitest";
import { defaultKeymap } from "../src/state/ui-settings";
import type { KeymapConfig, ShortcutId } from "../src/state/shortcuts";
import {
  conflictNotice,
  conflictOwner,
  modifierSendLabel,
  recordKey,
  reservedNotice,
  sendComboIsReserved,
  shortcutDescription,
  shortcutIdEquals,
} from "../src/lib/shortcuts-editor";

const MAC = false;

function keyEvent(key: string, mods: { ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean } = {}) {
  return { key, ctrlKey: mods.ctrl ?? false, altKey: mods.alt ?? false, shiftKey: mods.shift ?? false, metaKey: mods.meta ?? false };
}

describe("recordKey (shortcuts.rs:32-40)", () => {
  it("cancels on Escape regardless of modifiers", () => {
    expect(recordKey(keyEvent("Escape"), MAC)).toEqual({ kind: "cancelled" });
    expect(recordKey(keyEvent("escape"), MAC)).toEqual({ kind: "cancelled" });
    expect(recordKey(keyEvent("Escape", { ctrl: true }), MAC)).toEqual({ kind: "cancelled" });
  });

  it("records full combos in the canonical part order", () => {
    expect(recordKey(keyEvent("s", { meta: true }), MAC)).toEqual({ kind: "set", combo: "mod-s" });
    expect(recordKey(keyEvent("k", { meta: true, alt: true, shift: true }), MAC)).toEqual({
      kind: "set",
      combo: "mod-alt-shift-k",
    });
    // Off-mac, ctrl IS the primary and records as "mod".
    expect(recordKey(keyEvent("s", { ctrl: true }), MAC)).toEqual({ kind: "set", combo: "mod-s" });
    // The space bar spells "space".
    expect(recordKey(keyEvent(" ", { meta: true }), MAC)).toEqual({ kind: "set", combo: "mod-space" });
  });

  it("stays recording on a bare modifier", () => {
    expect(recordKey(keyEvent("Shift"), MAC)).toEqual({ kind: "ignored" });
    expect(recordKey(keyEvent("Control"), MAC)).toEqual({ kind: "ignored" });
    expect(recordKey(keyEvent("Meta"), MAC)).toEqual({ kind: "ignored" });
  });
});

describe("sendComboIsReserved (shortcuts.rs:403-405)", () => {
  it("modEnterAlwaysReserved", () => {
    expect(sendComboIsReserved("mod-enter")).toBe(true);
    expect(sendComboIsReserved("mod-shift-enter")).toBe(false);
    expect(sendComboIsReserved("enter")).toBe(false);
  });
});

describe("conflictOwner (shortcuts.rs:397-401)", () => {
  it("conflictOwnerFindsFirstMatchingBinding", () => {
    const keymap: KeymapConfig = defaultKeymap(MAC);
    // "mod-r" is ToggleChanges' default — rebinding ToggleSidebar onto it
    // names ToggleChanges as the owner.
    expect(conflictOwner(keymap, "toggleSidebar", "mod-r")).toEqual("toggleChanges");
    // Re-recording a shortcut's own combo is not a conflict.
    expect(conflictOwner(keymap, "toggleChanges", "mod-r")).toEqual(null);
    // A free combo conflicts with nothing.
    expect(conflictOwner(keymap, "toggleSidebar", "mod-shift-x")).toEqual(null);
    // Unavailable ids never participate: the embedded browser's reload and
    // the appshots capture are not bindable on web, so their default combos
    // read as free even though the default table still lists them.
    expect(conflictOwner(keymap, "saveFile", "mod-shift-r")).toEqual(null);
    expect(conflictOwner(keymap, "newSession", "mod-alt-space")).toEqual(null);
  });

  it("checks jump slots structurally", () => {
    const keymap: KeymapConfig = defaultKeymap(MAC);
    const slot: ShortcutId = { jumpSession: 0 };
    expect(conflictOwner(keymap, "newSession", "mod-1")).toEqual(slot);
    expect(conflictOwner(keymap, slot, "mod-1")).toEqual(null);
    expect(shortcutIdEquals(slot, { jumpSession: 0 })).toBe(true);
    expect(shortcutIdEquals(slot, { jumpSession: 1 })).toBe(false);
  });
});

describe("refusal notices (shortcuts.rs:228-248)", () => {
  it("names the reserved combo and the owning label verbatim", () => {
    expect(reservedNotice("mod-enter", MAC)).toBe("Ctrl+Enter is reserved for the composer.");
    expect(reservedNotice("mod-enter", true)).toBe("Cmd+Enter is reserved for the composer.");
    expect(conflictNotice("mod-r", "toggleChanges", MAC)).toBe("Ctrl+R is already assigned to Toggle right sidebar.");
  });
});

describe("page copy (shortcuts.rs:407-464)", () => {
  it("modifierSendLabel is platform-specific", () => {
    expect(modifierSendLabel(true)).toBe("⌘ Enter");
    expect(modifierSendLabel(false)).toBe("Ctrl Enter");
  });

  it("describes every id once", () => {
    const ids: ShortcutId[] = [
      "captureAppshot",
      "saveFile",
      "browserReload",
      "toggleSidebar",
      "toggleChanges",
      "toggleTerminal",
      "newSession",
      "openModelPicker",
      "nextSession",
      "prevSession",
      "archiveSession",
      { jumpSession: 3 },
    ];
    for (const id of ids) {
      expect(shortcutDescription(id).length).toBeGreaterThan(0);
    }
    expect(shortcutDescription("openModelPicker")).toBe(
      "Open the model picker for the current session.",
    );
    expect(shortcutDescription({ jumpSession: 3 })).toBe("Open the session at this place in the sidebar list.");
  });
});
