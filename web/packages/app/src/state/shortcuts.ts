/**
 * The keyboard: the keymap table, combo formatting, the shortcut bus — the
 * web peer of `crates/ui/src/settings.rs`'s shortcut model plus the slice of
 * `shell.rs::apply_keymap` that has to exist client-side.
 *
 * Combos are stored platform-neutral ("mod-s"); `mod` means Cmd on macOS and
 * Ctrl elsewhere. The platform is derived ONCE from the navigator and every
 * spelling decision — storage, `platformCombo`, `badgeCombo`, the jump-hint
 * primary modifier — reads the same flag, exactly as the desktop reads
 * `cfg!(target_os = "macos")`.
 *
 * The bus stays the dispatch spine: `applyKeymap` resolves the stored combos
 * into a keystroke → action table, ONE capture-phase `window` listener in
 * `AppShell` consults it, and the action fans out through `emitShortcut` to
 * whichever component owns its execution path (gap N13: the bus used to
 * carry two events; it now covers every action in `ShortcutId::ALL` plus the
 * fixed `mod-k` / `mod-,` bindings).
 */

import { defaultKeymap, JUMP_DEFAULTS, JUMP_SLOTS, type KeymapConfig } from "./ui-settings";

export type { KeymapConfig };

export { JUMP_DEFAULTS, JUMP_SLOTS };

// ---------------------------------------------------------------------------
// The platform
// ---------------------------------------------------------------------------

/**
 * The web's `cfg!(target_os = "macos")`. Derived once from the navigator and
 * cached; storage spelling, `platformCombo`, `badgeCombo` and the jump-hint
 * primary modifier all read it. Falls back to non-mac off-browser (tests).
 */
let cachedIsMac: boolean | null = null;

export function isMacPlatform(): boolean {
  if (cachedIsMac === null) {
    const navigator = (globalThis as { navigator?: { platform?: string; userAgent?: string } })
      .navigator;
    cachedIsMac =
      navigator !== undefined &&
      /Mac|iPhone|iPad|iPod/i.test(`${navigator.platform ?? ""} ${navigator.userAgent ?? ""}`);
  }
  return cachedIsMac;
}

// ---------------------------------------------------------------------------
// ShortcutId (settings.rs:740-810)
// ---------------------------------------------------------------------------

export type ShortcutId =
  | "captureAppshot"
  | "saveFile"
  | "browserReload"
  | "toggleSidebar"
  | "toggleChanges"
  | "toggleTerminal"
  | "newSession"
  | "newProject"
  | "openModelPicker"
  | "nextSession"
  | "prevSession"
  | "archiveSession"
  | { jumpSession: number };

/** `ShortcutId::ALL` — 20 entries, in `settings.rs`'s order. */
export const SHORTCUT_IDS: readonly ShortcutId[] = [
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
];

/** `ShortcutId::label` — user-visible strings, verbatim (`settings.rs:783`). */
export function shortcutLabel(id: ShortcutId): string {
  if (typeof id === "object") {
    const slot = id.jumpSession + 1;
    return `Jump to session ${slot}`;
  }
  switch (id) {
    case "captureAppshot":
      return "Capture Appshot";
    case "saveFile":
      return "Save file";
    case "browserReload":
      return "Reload browser page";
    case "toggleSidebar":
      return "Toggle left sidebar";
    case "toggleChanges":
      return "Toggle right sidebar";
    case "toggleTerminal":
      return "Toggle terminal";
    case "newSession":
      return "New session";
    case "newProject":
      return "New project";
    case "openModelPicker":
      return "Open model picker";
    case "nextSession":
      return "Next session";
    case "prevSession":
      return "Previous session";
    case "archiveSession":
      return "Archive session";
  }
}

/** The section a shortcut's row renders under (`shortcuts.rs:416-430`). */
export function shortcutGroup(id: ShortcutId): string {
  if (typeof id === "object") {
    return "Jump to session";
  }
  switch (id) {
    case "captureAppshot":
      return "Appshots";
    case "saveFile":
      return "Files";
    case "browserReload":
      return "Browser";
    case "toggleSidebar":
    case "toggleChanges":
    case "toggleTerminal":
      return "Panels";
    case "newProject":
      return "Projects";
    case "newSession":
    case "openModelPicker":
    case "nextSession":
    case "prevSession":
    case "archiveSession":
      return "Sessions";
  }
}

/**
 * `GROUP_ORDER` (`shortcuts.rs:412-415`). The Shortcuts page renders these
 * cards in order and SKIPS "Appshots" (it has its own sub-page), so five
 * cards render.
 */
export const SHORTCUT_GROUPS: readonly string[] = [
  "Files",
  "Browser",
  "Panels",
  "Sessions",
  "Projects",
  "Jump to session",
  "Appshots",
];

/**
 * `ShortcutId::available` (`settings.rs:778-780`), web-shaped: appshots need
 * the desktop's OS-global hotkey and `BrowserReload` reloads the EMBEDDED
 * browser page — neither exists in a browser tab. Unavailable ids are
 * excluded from conflict detection exactly as on the desktop.
 */
export function shortcutAvailable(id: ShortcutId): boolean {
  return id !== "captureAppshot" && id !== "browserReload";
}

/** `ShortcutId::default_combo_on` — delegates to ticket 03's default table. */
export function defaultComboOn(id: ShortcutId, isMac: boolean): string {
  const defaults = defaultKeymap(isMac);
  if (typeof id === "object") {
    return defaults.jumpSession[id.jumpSession] ?? "";
  }
  return defaults[id];
}

/** `KeymapConfig::get` — the stored combo for an id ("" when unbound). */
export function keymapGet(config: KeymapConfig, id: ShortcutId): string {
  if (typeof id === "object") {
    return config.jumpSession[id.jumpSession] ?? "";
  }
  return config[id];
}

// ---------------------------------------------------------------------------
// Combo formatting (settings.rs:1065-1133)
// ---------------------------------------------------------------------------

/** `platform_combo` — translate the `mod` token into this platform's key. */
export function platformCombo(combo: string, isMac: boolean): string {
  const primary = isMac ? "cmd" : "ctrl";
  return combo
    .split("-")
    .map((part) => (part === "mod" ? primary : part))
    .join("-");
}

/** `display_combo` — the readable form for the shortcuts table. */
export function displayCombo(combo: string, isMac: boolean): string {
  return combo
    .split("-")
    .map((part) => {
      if (part === "mod") {
        return isMac ? "Cmd" : "Ctrl";
      }
      if (part === "alt") {
        return isMac ? "Opt" : "Alt";
      }
      if (part === "shift") {
        return "Shift";
      }
      const first = part.charAt(0);
      return first.toUpperCase() + part.slice(1);
    })
    .join("+");
}

const BADGE_GLYPHS: ReadonlyMap<string, string> = new Map([
  ["ctrl", "⌃"],
  ["alt", "⌥"],
  ["shift", "⇧"],
  ["mod", "⌘"],
]);

/**
 * `badge_combo` — the compact key-cap form. macOS spells the modifiers as
 * their glyphs in the canonical ⌃⌥⇧⌘ order with no separators, then the key
 * with its first char uppercased ("⌘1", "⇧⌘A"); elsewhere it is exactly
 * `displayCombo` ("Ctrl+1").
 */
export function badgeCombo(combo: string, isMac: boolean): string {
  if (!isMac) {
    return displayCombo(combo, false);
  }
  const parts = combo.split("-");
  const key = parts.pop() ?? "";
  let out = "";
  for (const name of ["ctrl", "alt", "shift", "mod"]) {
    if (parts.includes(name)) {
      out += BADGE_GLYPHS.get(name) ?? "";
    }
  }
  const first = key.charAt(0);
  return out + first.toUpperCase() + key.slice(1);
}

/**
 * `combo_modifiers` (`settings.rs:1030-1038`): everything before the final
 * segment is a modifier; the final segment is the key.
 */
export function comboModifiers(combo: string): { mod: boolean; alt: boolean; shift: boolean } {
  const parts = combo.split("-");
  parts.pop();
  return {
    mod: parts.includes("mod"),
    alt: parts.includes("alt"),
    shift: parts.includes("shift"),
  };
}

// ---------------------------------------------------------------------------
// Keystroke grammar
// ---------------------------------------------------------------------------

const NAMED_KEYS: ReadonlySet<string> = new Set([
  "space", "tab", "enter", "escape", "backspace", "delete", "home", "end",
  "pageup", "pagedown", "up", "down", "left", "right",
  "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
  ",", ".", "/", ";", "'", "[", "]", "\\", "`", "~", "=", "+", "*",
]);

/** A single printable key token: one alphanumeric char, or a named key. */
function isKeyToken(part: string): boolean {
  return (part.length === 1 && /[a-z0-9]/.test(part)) || NAMED_KEYS.has(part);
}

/**
 * The web's `Keystroke::parse` — a small parser over the
 * `mod-ctrl-alt-shift-key` grammar: non-empty parts, modifiers before the
 * key, at most one of each, the key a single printable token. Returns the
 * CANONICAL keystroke (modifiers in `cmd-ctrl-alt-shift` order, the platform
 * primary spelled `cmd`) or null when unparseable.
 */
function canonicalKeystroke(combo: string, isMac: boolean): string | null {
  const parts = combo.split("-");
  if (parts.length === 0 || parts.some((part) => part.length === 0)) {
    return null;
  }
  const key = parts[parts.length - 1]!;
  if (!isKeyToken(key)) {
    return null;
  }
  let primary = false;
  let ctrl = false;
  let alt = false;
  let shift = false;
  for (const part of parts.slice(0, -1)) {
    switch (part) {
      case "mod":
      case "cmd":
        if (primary) {
          return null;
        }
        primary = true;
        break;
      case "ctrl":
        if (ctrl) {
          return null;
        }
        ctrl = true;
        break;
      case "alt":
        if (alt) {
          return null;
        }
        alt = true;
        break;
      case "shift":
        if (shift) {
          return null;
        }
        shift = true;
        break;
      default:
        return null;
    }
  }
  const out: string[] = [];
  if (primary) {
    // Off-mac the primary IS ctrl — `platformCombo` already rewrote `mod`
    // there, so a surviving `cmd` token is a mac spelling being normalized.
    out.push(isMac || ctrl ? "cmd" : "ctrl");
  }
  if (ctrl) {
    out.push("ctrl");
  }
  if (alt) {
    out.push("alt");
  }
  if (shift) {
    out.push("shift");
  }
  out.push(key);
  return out.join("-");
}

/**
 * `valid_or_default` (`shell.rs:296-303`): resolve to the platform combo,
 * falling back to the platform default — same contract, same log line —
 * when the stored combo does not parse.
 */
export function validOrDefault(combo: string, fallback: string, isMac: boolean): string {
  const candidate = platformCombo(combo, isMac);
  if (canonicalKeystroke(candidate, isMac) !== null) {
    return candidate;
  }
  console.warn("unparseable shortcut combo; using default");
  return platformCombo(fallback, isMac);
}

/**
 * A live `KeyboardEvent` as a canonical keystroke. The primary modifier is
 * meta on macOS and ctrl elsewhere; a REAL ctrl is its own part only on
 * macOS; the space bar spells `space`.
 */
export function keystrokeFromEvent(event: KeyboardEvent, isMac: boolean): string {
  const key = event.key === " " ? "space" : event.key.toLowerCase();
  const parts: string[] = [];
  if (isMac) {
    if (event.metaKey) {
      parts.push("cmd");
    }
    if (event.ctrlKey) {
      parts.push("ctrl");
    }
  } else if (event.ctrlKey) {
    parts.push("ctrl");
  }
  if (event.altKey) {
    parts.push("alt");
  }
  if (event.shiftKey) {
    parts.push("shift");
  }
  parts.push(key);
  return parts.join("-");
}

// ---------------------------------------------------------------------------
// Browser-reserved combos (the §2.9 table)
// ---------------------------------------------------------------------------

/** Whether a chord reaches the page at all. */
export type BrowserDelivery = "always" | "sometimes" | "never";

export interface BrowserReservedEntry {
  /** Canonical keystrokes this entry covers (every platform spelling). */
  readonly keystrokes: readonly string[];
  /** What the browser does with the chord — the §2.9 "Browser behaviour" column. */
  readonly behaviour: string;
  /** The caveat the Shortcuts page renders on a flagged row; null when reliably delivered. */
  readonly caveat: string | null;
  readonly delivery: BrowserDelivery;
}

function numberStrokes(): string[] {
  const strokes: string[] = [];
  for (let slot = 1; slot <= 9; slot += 1) {
    strokes.push(`cmd-${slot}`, `ctrl-${slot}`);
  }
  return strokes;
}

/**
 * The combos the browser claims before any page listener runs. The Shortcuts
 * settings page (ticket 29) imports THIS table for its caveat column rather
 * than restating it. Rules it encodes:
 *
 * 1. A `never` combo is not registered at all — it must not sit in the
 *    keymap as a silent no-op (`applyKeymap` skips them).
 * 2. An `always` combo is registered normally and calls `preventDefault()`.
 * 3. A `sometimes` combo is registered AND flagged (non-null caveat).
 */
export const BROWSER_RESERVED: readonly BrowserReservedEntry[] = [
  {
    keystrokes: ["cmd-w", "ctrl-w"],
    behaviour: "Closes the tab or window; not interceptable in any browser.",
    caveat: null,
    delivery: "never",
  },
  {
    keystrokes: ["cmd-n", "ctrl-n"],
    behaviour:
      "Opens a new browser window in Chrome/Edge/Safari; interceptable in Firefox only.",
    caveat: "Reserved by your browser — may open a new window instead.",
    delivery: "sometimes",
  },
  {
    keystrokes: ["cmd-t", "ctrl-t"],
    behaviour: "Opens a new browser tab; not interceptable.",
    caveat: null,
    delivery: "never",
  },
  {
    keystrokes: ["ctrl-tab", "ctrl-shift-tab"],
    behaviour:
      "Cycles browser tabs in Chrome/Edge (not interceptable); reaches the page in Firefox and in Safari when its tab-switch shortcuts are off.",
    caveat: "Reserved by your browser — chats stay reachable via number keys and the sidebar.",
    delivery: "sometimes",
  },
  {
    keystrokes: numberStrokes(),
    behaviour:
      "Switches browser tabs in Chrome/Edge/Firefox by default; interceptable on a focused page. Safari honours its number-keys tab preference.",
    caveat: "Your browser may switch tabs instead when the page is not focused.",
    delivery: "sometimes",
  },
  {
    keystrokes: ["cmd-r", "ctrl-r", "cmd-shift-r", "ctrl-shift-r"],
    behaviour: "Reload / hard reload; interceptable via preventDefault in all major browsers.",
    caveat: null,
    delivery: "always",
  },
  {
    keystrokes: ["cmd-s", "ctrl-s"],
    behaviour: "Save page; interceptable.",
    caveat: null,
    delivery: "always",
  },
  {
    keystrokes: ["cmd-b", "ctrl-b", "cmd-j", "ctrl-j", "cmd-k", "ctrl-k"],
    behaviour: "All reach the page.",
    caveat: null,
    delivery: "always",
  },
  {
    keystrokes: ["cmd-,", "ctrl-,"],
    behaviour: "Nothing in any browser.",
    caveat: null,
    delivery: "always",
  },
  {
    keystrokes: ["cmd-alt-space", "ctrl-alt-space"],
    behaviour: "OS-level.",
    caveat: null,
    delivery: "never",
  },
  {
    keystrokes: ["cmd-q", "ctrl-q", "cmd-h", "ctrl-h", "cmd-alt-h", "cmd-m", "ctrl-m"],
    behaviour: "OS-level (the macOS app-menu chords); desktop-only.",
    caveat: null,
    delivery: "never",
  },
];

const RESERVED_INDEX: ReadonlyMap<string, BrowserReservedEntry> = new Map(
  BROWSER_RESERVED.flatMap((entry) => entry.keystrokes.map((stroke) => [stroke, entry] as const)),
);

/** The caveat for a keystroke, or null when it is reliably delivered. */
export function browserReservedCaveat(keystroke: string): string | null {
  return RESERVED_INDEX.get(keystroke)?.caveat ?? null;
}

/** Rule 1: a combo the browser owns outright must never be registered. */
export function browserNeverDelivers(keystroke: string): boolean {
  return RESERVED_INDEX.get(keystroke)?.delivery === "never";
}

// ---------------------------------------------------------------------------
// applyKeymap (shell.rs:295-381, web-shaped)
// ---------------------------------------------------------------------------

/**
 * The actions the bus carries. `new-chat` keeps its pre-parity name (the
 * titlebar `+`, the keyboard layer and `NewChatListener` already speak it).
 */
export type ShortcutEvent =
  | "new-chat"
  | "save-file"
  | "toggle-sidebar"
  | "toggle-changes"
  | "toggle-terminal"
  | "open-model-picker"
  | "next-session"
  | "prev-session"
  | "archive-session"
  | "jump-session"
  | "add-space-palette"
  | "command-palette"
  | "open-settings";

export interface ShortcutDetail {
  /** The jump slot for `jump-session`. */
  readonly slot?: number;
}

/** One resolved binding: the bus event a keystroke dispatches. */
export interface Keybinding {
  readonly event: ShortcutEvent;
  /** The jump slot, when the event is `jump-session`. */
  readonly slot?: number;
  /** A binding with no modifier — the one kind guarded while typing. */
  readonly bare: boolean;
}

export type KeybindingTable = ReadonlyMap<string, Keybinding>;

/** The fixed app-level chords applied alongside the keymap (§2.1). */
const FIXED_BINDINGS: readonly { readonly combo: string; readonly event: ShortcutEvent }[] = [
  // ⌘K summons/dismisses the command palette (shell.rs, ticket 16).
  { combo: "mod-k", event: "command-palette" },
  // The platform convention for Settings (app_menus.rs:142-146, ticket 28).
  { combo: "mod-,", event: "open-settings" },
];

function bindingFor(id: ShortcutId): Keybinding {
  if (typeof id === "object") {
    return { event: "jump-session", slot: id.jumpSession, bare: false };
  }
  switch (id) {
    case "saveFile":
      return { event: "save-file", bare: false };
    case "toggleSidebar":
      return { event: "toggle-sidebar", bare: false };
    case "toggleChanges":
      return { event: "toggle-changes", bare: false };
    case "toggleTerminal":
      return { event: "toggle-terminal", bare: false };
    case "newSession":
      return { event: "new-chat", bare: false };
    case "newProject":
      // The desktop binds `ShortcutId::NewProject` to the `AddSpacePalette`
      // action (the New project flow's own toggle).
      return { event: "add-space-palette", bare: false };
    case "openModelPicker":
      return { event: "open-model-picker", bare: false };
    case "nextSession":
      return { event: "next-session", bare: false };
    case "prevSession":
      return { event: "prev-session", bare: false };
    case "archiveSession":
      return { event: "archive-session", bare: false };
    default:
      // Unavailable ids never reach the table; the arm exists for totality.
      return { event: "toggle-sidebar", bare: false };
  }
}

/**
 * `apply_keymap`, web-shaped: walk `ShortcutId::ALL`, resolve each id's
 * stored combo through `validOrDefault(stored, defaultComboOn(platform))`,
 * and register the binding under its canonical keystroke. Then apply the
 * fixed `mod-k` / `mod-,` chords, skipping any whose keystroke a user
 * shortcut already claims. A combo the browser never delivers is not
 * registered at all (rule 1); the first id wins a conflicting keystroke
 * (the defaults never conflict).
 *
 * A jump slot left empty binds nothing rather than falling back — the user
 * cleared it on purpose (shell.rs:369-381).
 */
export function applyKeymap(config: KeymapConfig, isMac: boolean): KeybindingTable {
  const table = new Map<string, Keybinding>();
  for (const id of SHORTCUT_IDS) {
    if (!shortcutAvailable(id)) {
      continue;
    }
    const stored = keymapGet(config, id);
    if (stored === "" && typeof id === "object") {
      continue;
    }
    const combo = validOrDefault(stored, defaultComboOn(id, isMac), isMac);
    const stroke = canonicalKeystroke(combo, isMac);
    if (stroke === null || browserNeverDelivers(stroke) || table.has(stroke)) {
      continue;
    }
    table.set(stroke, bindingFor(id));
  }
  for (const fixed of FIXED_BINDINGS) {
    const stroke = canonicalKeystroke(platformCombo(fixed.combo, isMac), isMac);
    if (stroke === null || table.has(stroke) || browserNeverDelivers(stroke)) {
      continue;
    }
    table.set(stroke, { event: fixed.event, bare: false });
  }
  return table;
}

/** The binding a live keyboard event matches, or null. */
export function matchKeybinding(event: KeyboardEvent, table: KeybindingTable): Keybinding | null {
  const stroke = keystrokeFromEvent(event, isMacPlatform());
  return table.get(stroke) ?? null;
}

// ---------------------------------------------------------------------------
// Jump hints + the modifier-send hint (settings.rs:1048-1062)
// ---------------------------------------------------------------------------

/**
 * `jump_hints_visible`: false when no modifier is held; otherwise true iff
 * some jump-slot combo's modifier triple matches `(primary, alt, shift)`
 * EXACTLY — adding Shift or Alt hides the hints, and a combo with no
 * modifiers never shows them (it would match the resting state).
 */
export function jumpHintsVisible(
  jumpCombos: readonly string[],
  primary: boolean,
  alt: boolean,
  shift: boolean,
): boolean {
  if (!primary && !alt && !shift) {
    return false;
  }
  return jumpCombos.some((combo) => {
    const held = comboModifiers(combo);
    return held.mod === primary && held.alt === alt && held.shift === shift;
  });
}

/** `modifier_send_hint_visible`: the primary modifier held by itself. */
export function modifierSendHintVisible(primary: boolean, alt: boolean, shift: boolean): boolean {
  return primary && !alt && !shift;
}

// ---------------------------------------------------------------------------
// Recording and healing (settings.rs:939-1011)
// ---------------------------------------------------------------------------

/**
 * `combo_from_keystroke_on`: build a stored combo from a recorded keystroke.
 * Returns null (keep recording) for a bare modifier press; emits parts in
 * the fixed canonical `mod-ctrl-alt-shift-key` order, where `mod` is the
 * primary and a bare `ctrl` part only exists on macOS.
 */
export function comboFromKeystrokeOn(
  isMac: boolean,
  ctrl: boolean,
  alt: boolean,
  shift: boolean,
  cmd: boolean,
  key: string,
): string | null {
  const trimmed = key.trim().toLowerCase();
  if (
    trimmed.length === 0 ||
    ["ctrl", "control", "alt", "shift", "cmd", "platform", "fn"].includes(trimmed)
  ) {
    return null;
  }
  const parts: string[] = [];
  const ctrlIsPrimary = ctrl && !isMac;
  if (cmd || ctrlIsPrimary) {
    parts.push("mod");
  }
  if (ctrl && !ctrlIsPrimary) {
    parts.push("ctrl");
  }
  if (alt) {
    parts.push("alt");
  }
  if (shift) {
    parts.push("shift");
  }
  parts.push(trimmed);
  return parts.join("-");
}

/** `heal_jump_slots`: exactly 9 entries — surviving slots keep theirs, the rest take the default. */
export function healJumpSlots(slots: readonly string[]): string[] {
  const healed = slots.slice(0, JUMP_SLOTS);
  while (healed.length < JUMP_SLOTS) {
    healed.push(JUMP_DEFAULTS[healed.length]!);
  }
  return healed;
}

/**
 * `heal_reserved_composer_shortcuts`: Cmd/Ctrl+Enter belongs to the composer
 * on every send mode — any id whose combo took it resets to its default,
 * preserving every unrelated customization.
 */
export function healReservedComposerShortcuts(
  config: KeymapConfig,
  isMac: boolean = isMacPlatform(),
): KeymapConfig {
  const defaults = defaultKeymap(isMac);
  const heal = (combo: string, fallback: string): string =>
    combo === "mod-enter" ? fallback : combo;
  return {
    captureAppshot: heal(config.captureAppshot, defaults.captureAppshot),
    saveFile: heal(config.saveFile, defaults.saveFile),
    browserReload: heal(config.browserReload, defaults.browserReload),
    toggleSidebar: heal(config.toggleSidebar, defaults.toggleSidebar),
    toggleChanges: heal(config.toggleChanges, defaults.toggleChanges),
    toggleTerminal: heal(config.toggleTerminal, defaults.toggleTerminal),
    newSession: heal(config.newSession, defaults.newSession),
    newProject: heal(config.newProject, defaults.newProject),
    openModelPicker: heal(config.openModelPicker, defaults.openModelPicker),
    nextSession: heal(config.nextSession, defaults.nextSession),
    prevSession: heal(config.prevSession, defaults.prevSession),
    archiveSession: heal(config.archiveSession, defaults.archiveSession),
    jumpSession: config.jumpSession.map((combo, slot) =>
      heal(combo, defaults.jumpSession[slot] ?? combo),
    ),
  };
}

// ---------------------------------------------------------------------------
// Chat cycling (shell/tabs.rs:16-32)
// ---------------------------------------------------------------------------

/**
 * `cycle_target`: the chat one step from `selected` in the sidebar `order`,
 * wrapping at both ends. With nothing selected, cycling enters the list at
 * the end it would have wrapped to; a selection that has left the list is
 * treated the same way rather than dead-ending. A single-row list cycles to
 * itself in both directions. Pure.
 */
export function cycleTarget(
  order: readonly string[],
  selected: string | null,
  forward: boolean,
): string | null {
  if (order.length === 0) {
    return null;
  }
  const at = selected === null ? -1 : order.indexOf(selected);
  let next: number;
  if (at >= 0 && forward) {
    next = (at + 1) % order.length;
  } else if (at >= 0) {
    next = (at + order.length - 1) % order.length;
  } else {
    next = forward ? 0 : order.length - 1;
  }
  return order[next] ?? null;
}

// ---------------------------------------------------------------------------
// The shortcut bus
// ---------------------------------------------------------------------------

const listeners = new Map<ShortcutEvent, Set<(detail: ShortcutDetail) => void>>();

export function onShortcut(
  event: ShortcutEvent,
  listener: (detail: ShortcutDetail) => void,
): () => void {
  let set = listeners.get(event);
  if (set === undefined) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(listener);
  return () => {
    set!.delete(listener);
    if (set!.size === 0) {
      listeners.delete(event);
    }
  };
}

export function emitShortcut(event: ShortcutEvent, detail: ShortcutDetail = {}): void {
  const set = listeners.get(event);
  if (set === undefined) {
    return;
  }
  for (const listener of [...set]) {
    try {
      listener(detail);
    } catch {
      // A misbehaving listener should not stop the rest from running.
    }
  }
}
