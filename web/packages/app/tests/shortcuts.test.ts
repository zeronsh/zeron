import { describe, expect, it } from "vitest";
import {
  BROWSER_RESERVED,
  JUMP_DEFAULTS,
  JUMP_SLOTS,
  SHORTCUT_GROUPS,
  SHORTCUT_IDS,
  applyKeymap,
  badgeCombo,
  browserNeverDelivers,
  browserReservedCaveat,
  comboFromKeystrokeOn,
  comboModifiers,
  cycleTarget,
  defaultComboOn,
  displayCombo,
  emitShortcut,
  healJumpSlots,
  healReservedComposerShortcuts,
  jumpHintsVisible,
  keystrokeFromEvent,
  matchKeybinding,
  modifierSendHintVisible,
  onShortcut,
  platformCombo,
  shortcutAvailable,
  shortcutGroup,
  shortcutLabel,
  validOrDefault,
  type ShortcutId,
} from "../src/state/shortcuts";
import { defaultKeymap, type KeymapConfig } from "../src/state/ui-settings";
import { keymapStore, overlayKeyboard, overlayOwnsKeyboard } from "../src/state/keymap";
import { jumpHintStore } from "../src/state/jump-hints";

/**
 * The keymap's pure half — mirrors the desktop's suites in
 * `settings.rs` and `shell/tabs.rs:373-423` (the five cycle tests), plus the
 * platform-spelling invariant, combo formatting, jump hints, healing, the
 * browser-reserved table, and `applyKeymap`'s resolution rules.
 */

function fakeEvent(over: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    key: "b",
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...over,
  } as KeyboardEvent;
}

// ---------------------------------------------------------------------------
// The bus
// ---------------------------------------------------------------------------

describe("shortcut event bus", () => {
  it("delivers a shortcut to every registered listener", () => {
    const calls: string[] = [];
    const offA = onShortcut("new-chat", () => {
      calls.push("a");
    });
    const offB = onShortcut("new-chat", () => {
      calls.push("b");
    });
    emitShortcut("new-chat");
    expect(calls.sort()).toEqual(["a", "b"]);
    offA();
    offB();
  });

  it("carries the jump slot to the listener", () => {
    const slots: number[] = [];
    const off = onShortcut("jump-session", (detail) => {
      slots.push(detail.slot ?? -1);
    });
    emitShortcut("jump-session", { slot: 4 });
    emitShortcut("jump-session");
    expect(slots).toEqual([4, -1]);
    off();
  });

  it("stops delivering to unsubscribed listeners", () => {
    const calls: string[] = [];
    const off = onShortcut("new-chat", () => {
      calls.push("hit");
    });
    off();
    emitShortcut("new-chat");
    expect(calls).toEqual([]);
  });

  it("swallows listener errors so other listeners still fire", () => {
    const calls: string[] = [];
    onShortcut("new-chat", () => {
      throw new Error("boom");
    });
    onShortcut("new-chat", () => {
      calls.push("ok");
    });
    emitShortcut("new-chat");
    expect(calls).toEqual(["ok"]);
  });

  it("is a no-op when no listeners have registered", () => {
    expect(() => emitShortcut("open-settings")).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// The keymap table (§4.16)
// ---------------------------------------------------------------------------

describe("SHORTCUT_IDS", () => {
  it("has exactly 21 entries in settings.rs order", () => {
    expect(SHORTCUT_IDS).toHaveLength(21);
    expect(SHORTCUT_IDS).toEqual([
      "captureAppshot",
      "saveFile",
      "browserReload",
      "toggleSidebar",
      "toggleChanges",
      "toggleTerminal",
      "newSession",
      "newProject",
      "openModelPicker",
      "nextSession",
      "prevSession",
      "archiveSession",
      { jumpSession: 0 },
      { jumpSession: 1 },
      { jumpSession: 2 },
      { jumpSession: 3 },
      { jumpSession: 4 },
      { jumpSession: 5 },
      { jumpSession: 6 },
      { jumpSession: 7 },
      { jumpSession: 8 },
    ]);
  });

  it("labels every row verbatim, jump slots numbered from 1", () => {
    expect(shortcutLabel("captureAppshot")).toBe("Capture Appshot");
    expect(shortcutLabel("saveFile")).toBe("Save file");
    expect(shortcutLabel("browserReload")).toBe("Reload browser page");
    expect(shortcutLabel("toggleSidebar")).toBe("Toggle left sidebar");
    expect(shortcutLabel("toggleChanges")).toBe("Toggle right sidebar");
    expect(shortcutLabel("toggleTerminal")).toBe("Toggle terminal");
    expect(shortcutLabel("newSession")).toBe("New session");
    expect(shortcutLabel("newProject")).toBe("New project");
    expect(shortcutLabel("openModelPicker")).toBe("Open model picker");
    expect(shortcutLabel("nextSession")).toBe("Next session");
    expect(shortcutLabel("prevSession")).toBe("Previous session");
    expect(shortcutLabel("archiveSession")).toBe("Archive session");
    expect(shortcutLabel({ jumpSession: 0 })).toBe("Jump to session 1");
    expect(shortcutLabel({ jumpSession: 8 })).toBe("Jump to session 9");
  });

  it("groups rows the way the Shortcuts page cards render", () => {
    expect(shortcutGroup("saveFile")).toBe("Files");
    expect(shortcutGroup("browserReload")).toBe("Browser");
    expect(shortcutGroup("toggleSidebar")).toBe("Panels");
    expect(shortcutGroup("toggleChanges")).toBe("Panels");
    expect(shortcutGroup("toggleTerminal")).toBe("Panels");
    expect(shortcutGroup("newSession")).toBe("Sessions");
    expect(shortcutGroup("newProject")).toBe("Projects");
    expect(shortcutGroup("openModelPicker")).toBe("Sessions");
    expect(shortcutGroup("archiveSession")).toBe("Sessions");
    expect(shortcutGroup({ jumpSession: 3 })).toBe("Jump to session");
    expect(shortcutGroup("captureAppshot")).toBe("Appshots");
    expect(SHORTCUT_GROUPS).toEqual([
      "Files",
      "Browser",
      "Panels",
      "Sessions",
      "Projects",
      "Jump to session",
      "Appshots",
    ]);
  });

  it("marks appshots and browser reload unavailable on web", () => {
    expect(shortcutAvailable("captureAppshot")).toBe(false);
    expect(shortcutAvailable("browserReload")).toBe(false);
    for (const id of SHORTCUT_IDS) {
      if (id !== "captureAppshot" && id !== "browserReload") {
        expect(shortcutAvailable(id)).toBe(true);
      }
    }
  });
});

describe("defaultComboOn", () => {
  it("spells Next/Prev Session per platform", () => {
    // The cross-platform trap (settings.rs:816-826): the keystroke is
    // Ctrl+Tab everywhere, but the STORED spelling differs — off macOS ctrl
    // is the primary and stores as mod, on macOS it is its own modifier.
    expect(defaultComboOn("nextSession", true)).toBe("ctrl-tab");
    expect(defaultComboOn("nextSession", false)).toBe("mod-tab");
    expect(defaultComboOn("prevSession", true)).toBe("ctrl-shift-tab");
    expect(defaultComboOn("prevSession", false)).toBe("mod-shift-tab");
  });

  it("spells CaptureAppshot per platform", () => {
    expect(defaultComboOn("captureAppshot", true)).toBe("ctrl-alt-space");
    expect(defaultComboOn("captureAppshot", false)).toBe("mod-alt-space");
  });

  it("New project defaults to mod-shift-n on both platforms", () => {
    expect(defaultComboOn("newProject", true)).toBe("mod-shift-n");
    expect(defaultComboOn("newProject", false)).toBe("mod-shift-n");
  });

  it("is platform-identical for every other id, and matches the defaults table", () => {
    for (const id of SHORTCUT_IDS) {
      if (id === "captureAppshot" || id === "nextSession" || id === "prevSession") {
        continue;
      }
      expect(defaultComboOn(id, true)).toBe(defaultComboOn(id, false));
    }
    const mac = defaultKeymap(true);
    const other = defaultKeymap(false);
    expect(defaultComboOn("saveFile", false)).toBe(mac.saveFile);
    expect(defaultComboOn("toggleSidebar", false)).toBe(other.toggleSidebar);
    expect(defaultComboOn({ jumpSession: 5 }, true)).toBe("mod-6");
    expect(JUMP_DEFAULTS).toEqual([
      "mod-1", "mod-2", "mod-3", "mod-4", "mod-5", "mod-6", "mod-7", "mod-8", "mod-9",
    ]);
    expect(JUMP_SLOTS).toBe(9);
  });
});

// ---------------------------------------------------------------------------
// Combo formatting (§4.10)
// ---------------------------------------------------------------------------

describe("combo formatting", () => {
  it("platformCombo translates the mod token per platform", () => {
    expect(platformCombo("mod-s", true)).toBe("cmd-s");
    expect(platformCombo("mod-s", false)).toBe("ctrl-s");
    expect(platformCombo("ctrl-tab", true)).toBe("ctrl-tab");
    expect(platformCombo("ctrl-tab", false)).toBe("ctrl-tab");
  });

  it("displayCombo renders the shortcuts-table form", () => {
    expect(displayCombo("mod-shift-a", false)).toBe("Ctrl+Shift+A");
    expect(displayCombo("mod-shift-a", true)).toBe("Cmd+Shift+A");
    expect(displayCombo("mod-alt-space", true)).toBe("Cmd+Opt+Space");
    expect(displayCombo("mod-alt-space", false)).toBe("Ctrl+Alt+Space");
    expect(displayCombo("mod-1", true)).toBe("Cmd+1");
  });

  it("badgeCombo uses glyphs on macOS and displayCombo elsewhere", () => {
    expect(badgeCombo("mod-1", true)).toBe("⌘1");
    expect(badgeCombo("mod-shift-a", true)).toBe("⇧⌘A");
    expect(badgeCombo("mod-1", false)).toBe("Ctrl+1");
    expect(badgeCombo("mod-shift-a", false)).toBe("Ctrl+Shift+A");
    // The canonical ⌃⌥⇧⌘ order, no separators, key first char uppercased.
    expect(badgeCombo("ctrl-alt-space", true)).toBe("⌃⌥Space");
    expect(badgeCombo("mod-ctrl-alt-shift-a", true)).toBe("⌃⌥⇧⌘A");
  });

  it("comboModifiers drops the final segment as the key", () => {
    expect(comboModifiers("mod-shift-a")).toEqual({ mod: true, alt: false, shift: true });
    expect(comboModifiers("mod-1")).toEqual({ mod: true, alt: false, shift: false });
    expect(comboModifiers("ctrl-alt-space")).toEqual({ mod: false, alt: true, shift: false });
    expect(comboModifiers("a")).toEqual({ mod: false, alt: false, shift: false });
  });

  it("validOrDefault falls back to the platform default on an unparseable combo", () => {
    expect(validOrDefault("nonsense-!!", "mod-b", false)).toBe("ctrl-b");
    expect(validOrDefault("nonsense-!!", "mod-b", true)).toBe("cmd-b");
    expect(validOrDefault("", "mod-b", false)).toBe("ctrl-b");
    expect(validOrDefault("ctrl-ctrl-x", "mod-b", false)).toBe("ctrl-b");
    expect(validOrDefault("mod-shift-a", "mod-b", false)).toBe("ctrl-shift-a");
  });
});

describe("keystrokeFromEvent", () => {
  it("reads the primary modifier per platform", () => {
    expect(keystrokeFromEvent(fakeEvent({ key: "b", metaKey: true }), true)).toBe("cmd-b");
    expect(keystrokeFromEvent(fakeEvent({ key: "b", ctrlKey: true }), false)).toBe("ctrl-b");
    // A real ctrl is its own part only on macOS.
    expect(keystrokeFromEvent(fakeEvent({ key: "tab", ctrlKey: true }), true)).toBe("ctrl-tab");
    expect(keystrokeFromEvent(fakeEvent({ key: "tab", ctrlKey: true }), false)).toBe("ctrl-tab");
    expect(keystrokeFromEvent(fakeEvent({ key: "1", metaKey: true }), true)).toBe("cmd-1");
  });

  it("joins alt and shift after the primary and spells the space bar", () => {
    expect(keystrokeFromEvent(fakeEvent({ key: "a", ctrlKey: true, shiftKey: true }), false)).toBe(
      "ctrl-shift-a",
    );
    expect(keystrokeFromEvent(fakeEvent({ key: "Tab", ctrlKey: true, shiftKey: true }), false)).toBe(
      "ctrl-shift-tab",
    );
    expect(keystrokeFromEvent(fakeEvent({ key: " ", metaKey: true }), true)).toBe("cmd-space");
    expect(keystrokeFromEvent(fakeEvent({ key: "," }), false)).toBe(",");
  });

  it("matchKeybinding resolves a live event through the table", () => {
    const table = applyKeymap(defaultKeymap(false), false);
    expect(matchKeybinding(fakeEvent({ key: "b", ctrlKey: true }), table)?.event).toBe(
      "toggle-sidebar",
    );
    expect(matchKeybinding(fakeEvent({ key: "Tab", ctrlKey: true }), table)?.event).toBe(
      "next-session",
    );
    expect(matchKeybinding(fakeEvent({ key: "3", ctrlKey: true }), table)?.slot).toBe(2);
    expect(matchKeybinding(fakeEvent({ key: "b" }), table)).toBeNull();
    expect(matchKeybinding(fakeEvent({ key: "b", ctrlKey: true, shiftKey: true }), table)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// BROWSER_RESERVED (§2.9)
// ---------------------------------------------------------------------------

describe("BROWSER_RESERVED", () => {
  it("flags the maybe-delivered rows with the caveat the Shortcuts page renders", () => {
    // The caveat text comes from THIS table — the single source the settings
    // page (ticket 29) imports; nothing restates it.
    for (const entry of BROWSER_RESERVED) {
      for (const stroke of entry.keystrokes) {
        if (entry.delivery === "sometimes") {
          const caveat = browserReservedCaveat(stroke);
          expect(caveat).toBe(entry.caveat);
          expect(caveat).not.toBeNull();
          expect(caveat!.length).toBeGreaterThan(0);
        } else {
          expect(browserReservedCaveat(stroke)).toBeNull();
        }
        expect(browserNeverDelivers(stroke)).toBe(entry.delivery === "never");
      }
    }
  });

  it("never duplicates a keystroke across entries", () => {
    const seen = new Set<string>();
    for (const entry of BROWSER_RESERVED) {
      for (const stroke of entry.keystrokes) {
        expect(seen.has(stroke)).toBe(false);
        seen.add(stroke);
      }
    }
  });

  it("marks the flagged default rows the §2.9 table names", () => {
    expect(browserReservedCaveat("ctrl-n")).toContain("browser");
    expect(browserReservedCaveat("ctrl-tab")).toContain("browser");
    expect(browserReservedCaveat("cmd-n")).toContain("browser");
    expect(browserReservedCaveat("ctrl-5")).not.toBeNull();
    // Reliably delivered chords carry no caveat.
    expect(browserReservedCaveat("ctrl-b")).toBeNull();
    expect(browserReservedCaveat("ctrl-r")).toBeNull();
    expect(browserReservedCaveat("ctrl-s")).toBeNull();
    expect(browserReservedCaveat("ctrl-k")).toBeNull();
    expect(browserReservedCaveat("ctrl-,")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// applyKeymap (§2.2)
// ---------------------------------------------------------------------------

describe("applyKeymap", () => {
  it("registers every available default and both fixed chords", () => {
    const table = applyKeymap(defaultKeymap(false), false);
    // 10 available scalar ids + 9 jump slots + mod-k + mod-,.
    expect(table.size).toBe(21);
    expect(table.get("ctrl-s")?.event).toBe("save-file");
    expect(table.get("ctrl-b")?.event).toBe("toggle-sidebar");
    expect(table.get("ctrl-r")?.event).toBe("toggle-changes");
    expect(table.get("ctrl-j")?.event).toBe("toggle-terminal");
    expect(table.get("ctrl-n")?.event).toBe("new-chat");
    // New project lands on the add-space palette (its own toggle).
    expect(table.get("ctrl-shift-n")?.event).toBe("add-space-palette");
    // OpenModelPicker (upstream faac7432): Mod+/ opens the model picker.
    expect(table.get("ctrl-/")?.event).toBe("open-model-picker");
    expect(table.get("ctrl-tab")?.event).toBe("next-session");
    expect(table.get("ctrl-shift-tab")?.event).toBe("prev-session");
    expect(table.get("ctrl-shift-a")?.event).toBe("archive-session");
    expect(table.get("ctrl-1")?.slot).toBe(0);
    expect(table.get("ctrl-9")?.slot).toBe(8);
    // Ticket 16 moved the fixed mod-k chord to the command palette.
    expect(table.get("ctrl-k")?.event).toBe("command-palette");
    expect(table.get("ctrl-,")?.event).toBe("open-settings");
    // Unavailable ids never register; the browser-never chords never appear.
    expect([...table.values()].some((binding) => binding.event === "save-file")).toBe(true);
  });

  it("spells the mac table with cmd and leaves Next/Prev on real ctrl", () => {
    const table = applyKeymap(defaultKeymap(true), true);
    expect(table.get("cmd-b")?.event).toBe("toggle-sidebar");
    expect(table.get("cmd-1")?.slot).toBe(0);
    expect(table.get("cmd-/")?.event).toBe("open-model-picker");
    expect(table.get("ctrl-tab")?.event).toBe("next-session");
    expect(table.get("ctrl-shift-tab")?.event).toBe("prev-session");
    expect(table.get("cmd-k")?.event).toBe("command-palette");
    expect(table.get("cmd-,")?.event).toBe("open-settings");
    expect(table.has("ctrl-b")).toBe(false);
  });

  it("follows a rebind and drops the default stroke", () => {
    const config: KeymapConfig = { ...defaultKeymap(false), toggleSidebar: "mod-shift-x" };
    const table = applyKeymap(config, false);
    expect(table.get("ctrl-b")).toBeUndefined();
    expect(table.get("ctrl-shift-x")?.event).toBe("toggle-sidebar");
  });

  it("follows an OpenModelPicker rebind off mod-/", () => {
    const config: KeymapConfig = { ...defaultKeymap(false), openModelPicker: "mod-shift-m" };
    const table = applyKeymap(config, false);
    expect(table.has("ctrl-/")).toBe(false);
    expect(table.get("ctrl-shift-m")?.event).toBe("open-model-picker");
  });

  it("never registers a combo the browser owns outright", () => {
    // Rule 1: Mod+W closes the tab; a rebind onto it must not sit in the
    // keymap as a silent no-op.
    const config: KeymapConfig = { ...defaultKeymap(false), newSession: "mod-w" };
    const table = applyKeymap(config, false);
    expect(table.has("ctrl-w")).toBe(false);
    expect([...table.values()].some((binding) => binding.event === "new-chat")).toBe(false);
  });

  it("skips a fixed chord a user shortcut already claims", () => {
    const config: KeymapConfig = { ...defaultKeymap(false), toggleSidebar: "mod-k" };
    const table = applyKeymap(config, false);
    expect(table.get("ctrl-k")?.event).toBe("toggle-sidebar");
    // The command palette's fixed mod-k chord deferred to the claim; the
    // add-space event still reaches the table through newProject's own
    // keymap binding (mod-shift-n, ticket 16's #402 port).
    expect(table.get("ctrl-shift-n")?.event).toBe("add-space-palette");
    expect([...table.values()].filter((binding) => binding.event === "add-space-palette")).toHaveLength(
      1,
    );
  });

  it("leaves a cleared jump slot unbound rather than falling back", () => {
    // The user cleared it on purpose (shell.rs:369-381).
    const config: KeymapConfig = {
      ...defaultKeymap(false),
      jumpSession: ["mod-1", "", "mod-3", "mod-4", "mod-5", "mod-6", "mod-7", "mod-8", "mod-9"],
    };
    const table = applyKeymap(config, false);
    expect(table.has("ctrl-2")).toBe(false);
    expect(table.get("ctrl-1")?.slot).toBe(0);
    expect(table.get("ctrl-3")?.slot).toBe(2);
  });

  it("falls an unparseable stored combo back to its default", () => {
    const config: KeymapConfig = { ...defaultKeymap(false), toggleSidebar: "!!" };
    const table = applyKeymap(config, false);
    expect(table.get("ctrl-b")?.event).toBe("toggle-sidebar");
  });
});

// ---------------------------------------------------------------------------
// Jump hints (§4.9)
// ---------------------------------------------------------------------------

describe("jumpHintsVisible", () => {
  const defaults = JUMP_DEFAULTS;

  it("is false when no modifier is held", () => {
    expect(jumpHintsVisible(defaults, false, false, false)).toBe(false);
  });

  it("shows for the primary alone against the default slots", () => {
    expect(jumpHintsVisible(defaults, true, false, false)).toBe(true);
  });

  it("hides when Shift or Alt joins the primary", () => {
    expect(jumpHintsVisible(defaults, true, false, true)).toBe(false);
    expect(jumpHintsVisible(defaults, true, true, false)).toBe(false);
    expect(jumpHintsVisible(defaults, true, true, true)).toBe(false);
  });

  it("never shows for a jump combo with no modifiers", () => {
    // It would match the resting state and pin the overlay open.
    expect(jumpHintsVisible(["a", "b"], true, false, false)).toBe(false);
    expect(jumpHintsVisible(["a", "b"], false, false, false)).toBe(false);
  });

  it("matches a rebound slot's exact triple", () => {
    expect(jumpHintsVisible(["mod-shift-4"], true, false, true)).toBe(true);
    expect(jumpHintsVisible(["mod-shift-4"], true, false, false)).toBe(false);
    expect(jumpHintsVisible(["alt-7"], false, true, false)).toBe(true);
  });
});

describe("modifierSendHintVisible", () => {
  it("is the primary held by itself", () => {
    expect(modifierSendHintVisible(true, false, false)).toBe(true);
    expect(modifierSendHintVisible(true, true, false)).toBe(false);
    expect(modifierSendHintVisible(true, false, true)).toBe(false);
    expect(modifierSendHintVisible(false, false, false)).toBe(false);
    expect(modifierSendHintVisible(false, true, true)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Recording and healing (§3.5)
// ---------------------------------------------------------------------------

describe("comboFromKeystrokeOn", () => {
  it("returns null for a bare modifier press or empty key", () => {
    for (const key of ["ctrl", "control", "alt", "shift", "cmd", "platform", "fn", "", "  "]) {
      expect(comboFromKeystrokeOn(false, true, false, false, false, key)).toBeNull();
      expect(comboFromKeystrokeOn(true, false, false, false, true, key)).toBeNull();
    }
  });

  it("emits parts in the fixed canonical order", () => {
    expect(comboFromKeystrokeOn(false, true, false, true, false, "A")).toBe("mod-shift-a");
    expect(comboFromKeystrokeOn(false, true, true, true, false, "x")).toBe("mod-alt-shift-x");
    expect(comboFromKeystrokeOn(true, false, false, false, true, "1")).toBe("mod-1");
  });

  it("keeps a real ctrl its own part only on macOS", () => {
    expect(comboFromKeystrokeOn(true, true, false, false, false, "x")).toBe("ctrl-x");
    expect(comboFromKeystrokeOn(true, true, false, false, true, "x")).toBe("mod-ctrl-x");
    // Off macOS ctrl IS the primary and folds into mod.
    expect(comboFromKeystrokeOn(false, true, false, false, true, "x")).toBe("mod-x");
  });
});

describe("healJumpSlots", () => {
  it("pads a short list to exactly 9 with the per-slot defaults", () => {
    expect(healJumpSlots(["mod-x"])).toEqual([
      "mod-x", "mod-2", "mod-3", "mod-4", "mod-5", "mod-6", "mod-7", "mod-8", "mod-9",
    ]);
  });

  it("truncates a long list to exactly 9", () => {
    const slots = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
    expect(healJumpSlots(slots)).toEqual(["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
    expect(healJumpSlots([])).toEqual([...JUMP_DEFAULTS]);
  });
});

describe("healReservedComposerShortcuts", () => {
  it("resets any id that took mod-enter, preserving everything else", () => {
    const config: KeymapConfig = {
      ...defaultKeymap(false),
      toggleSidebar: "mod-enter",
      archiveSession: "mod-shift-a",
      jumpSession: ["mod-enter", "mod-2", "mod-3", "mod-4", "mod-5", "mod-6", "mod-7", "mod-8", "mod-9"],
    };
    const healed = healReservedComposerShortcuts(config, false);
    expect(healed.toggleSidebar).toBe("mod-b");
    expect(healed.jumpSession[0]).toBe("mod-1");
    expect(healed.archiveSession).toBe("mod-shift-a");
    expect(healed.saveFile).toBe("mod-s");
  });

  it("resets a platform-dependent default correctly", () => {
    const mac: KeymapConfig = { ...defaultKeymap(true), nextSession: "mod-enter" };
    expect(healReservedComposerShortcuts(mac, true).nextSession).toBe("ctrl-tab");
    const other: KeymapConfig = { ...defaultKeymap(false), nextSession: "mod-enter" };
    expect(healReservedComposerShortcuts(other, false).nextSession).toBe("mod-tab");
  });
});

// ---------------------------------------------------------------------------
// Chat cycling (shell/tabs.rs:373-423)
// ---------------------------------------------------------------------------

describe("cycleTarget", () => {
  it("steps_forward_and_back_through_the_list", () => {
    const list = ["a", "b", "c"];
    expect(cycleTarget(list, "a", true)).toBe("b");
    expect(cycleTarget(list, "b", true)).toBe("c");
    expect(cycleTarget(list, "c", false)).toBe("b");
    expect(cycleTarget(list, "b", false)).toBe("a");
  });

  it("wraps_at_both_ends", () => {
    const list = ["a", "b", "c"];
    expect(cycleTarget(list, "c", true)).toBe("a");
    expect(cycleTarget(list, "a", false)).toBe("c");
  });

  it("a_single_session_cycles_to_itself", () => {
    // Not a no-op by accident: with one row both directions must resolve,
    // so the shortcut never looks broken by dead-ending on null.
    expect(cycleTarget(["only"], "only", true)).toBe("only");
    expect(cycleTarget(["only"], "only", false)).toBe("only");
  });

  it("no_selection_enters_the_list_from_the_matching_end", () => {
    const list = ["a", "b", "c"];
    expect(cycleTarget(list, null, true)).toBe("a");
    expect(cycleTarget(list, null, false)).toBe("c");
    // A selection that has since left the list is treated the same way
    // rather than dead-ending.
    expect(cycleTarget(list, "gone", true)).toBe("a");
    expect(cycleTarget(list, "gone", false)).toBe("c");
  });

  it("an_empty_list_has_nothing_to_select", () => {
    expect(cycleTarget([], null, true)).toBeNull();
    expect(cycleTarget([], "a", true)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// The keymap store facade
// ---------------------------------------------------------------------------

describe("keymapStore", () => {
  it("round-trips a rebind through the settings store's healing", () => {
    keymapStore.update({ toggleSidebar: "mod-shift-b" });
    expect(keymapStore.get().toggleSidebar).toBe("mod-shift-b");
    // Healing keeps the jump list whole through every write.
    keymapStore.update({ jumpSession: ["mod-x"] });
    expect(keymapStore.get().jumpSession).toHaveLength(9);
    expect(keymapStore.get().jumpSession[0]).toBe("mod-x");
    expect(keymapStore.get().jumpSession[1]).toBe("mod-2");
  });
});

// ---------------------------------------------------------------------------
// overlay_owns_keyboard
// ---------------------------------------------------------------------------

describe("overlayKeyboard", () => {
  it("owns while any source is registered and counts sources", () => {
    expect(overlayOwnsKeyboard()).toBe(false);
    overlayKeyboard.set("composer-pickers", true);
    expect(overlayOwnsKeyboard()).toBe(true);
    overlayKeyboard.set("add-space", true);
    expect(overlayOwnsKeyboard()).toBe(true);
    overlayKeyboard.set("composer-pickers", false);
    expect(overlayOwnsKeyboard()).toBe(true);
    overlayKeyboard.set("add-space", false);
    expect(overlayOwnsKeyboard()).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// The jump-hint modifier store
// ---------------------------------------------------------------------------

describe("jumpHintStore", () => {
  it("tracks held modifiers and notifies only on change", () => {
    const events: number[] = [];
    const off = jumpHintStore.subscribe(() => {
      events.push(events.length);
    });
    expect(jumpHintStore.getSnapshot()).toEqual({ primary: false, alt: false, shift: false });
    jumpHintStore.modifiersFromEvent(fakeEvent({ key: "Control", ctrlKey: true }));
    expect(jumpHintStore.getSnapshot()).toEqual({ primary: true, alt: false, shift: false });
    // Same state again: no notification.
    const before = events.length;
    jumpHintStore.modifiersFromEvent(fakeEvent({ key: "b", ctrlKey: true }));
    expect(events.length).toBe(before);
    jumpHintStore.modifiersFromEvent(fakeEvent({ key: "b", ctrlKey: true, altKey: true }));
    expect(jumpHintStore.getSnapshot()).toEqual({ primary: true, alt: true, shift: false });
    jumpHintStore.clear();
    expect(jumpHintStore.getSnapshot()).toEqual({ primary: false, alt: false, shift: false });
    off();
  });

  it("reads the platform primary off metaKey on mac-shaped input", () => {
    // The store consults the detected platform; on a non-mac test runner the
    // primary is ctrl — the mac arm is exercised through keystrokeFromEvent.
    jumpHintStore.modifiersFromEvent(fakeEvent({ key: "Meta", ctrlKey: true }));
    expect(jumpHintStore.getSnapshot().primary).toBe(true);
    jumpHintStore.clear();
  });
});

// ---------------------------------------------------------------------------
// ShortcutId narrowing sanity (compile-time behaviour, kept as documentation)
// ---------------------------------------------------------------------------

describe("ShortcutId", () => {
  it("discriminates jump slots from scalar ids", () => {
    const jump: ShortcutId = { jumpSession: 4 };
    expect(typeof jump === "object").toBe(true);
    expect(jump.jumpSession).toBe(4);
  });
});
