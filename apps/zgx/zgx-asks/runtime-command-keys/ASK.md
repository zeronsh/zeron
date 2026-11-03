# zgx ask: let an app rebind its commands' shortcuts while it runs

## What we're building
Settings → Shortcuts in the Zeron app on zgx (apps/zgx), at parity with
the shipping Zeron app. That page lists every shortcut, and rebinding one
records a key combination: click a combo, press keys, Esc cancels, and a
conflict with another command shows while you record. The new binding
applies at once, everywhere.

## What stops it today
`ui.command { key = "cmd+j" }` takes the key as a plain string, read when
the command is declared. Commands are collected per load and published
once the load succeeds (zgx-script `src/command.rs`: `Commands::begin`
clears `pending`, `Commands::publish` builds the list and runners). A
second declaration of the same id is an error ("declared twice"). So a
rebind can only take effect after the scripts load again. A real app has
no reason to reload for it, and reloading for it would rebuild every node.

## Minimal repro
`repro/app.luau` beside this file (run with `ZGX_CHECK=0 zgx run .`):
1. ⌘J runs the command ("ran 1 times").
2. Click "Bind to ⌘L". The label says ⌘L.
3. ⌘L does nothing, and ⌘J still runs the command.
4. Save the file (a hot reload), and ⌘L works.

## What would do it (any one)
1. **`key` takes a state or a function**, as most props do:
   `ui.command { id, title, key = function() return keymap:get()[id] or "cmd+j" end, run }`.
   When what it reads changes, the host re-publishes the list (the
   `Signal<Vec<Command>>` already exists), and the menu's key equivalents
   follow. This is the most zgx-like option.
2. **A handle:** `local c = ui.command { … }`, then `c:set_key("cmd+l")`
   updates the published command in place.
3. **`ui.commands_changed()`:** asks the host to re-run declarations
   outside a load. This is the least preferred, since it re-runs every
   declaration.

## Also wanted for the recording UI (smaller)
- `ui.shortcut_owner(keys) -> id?`: which command (the app's, or a
  plugin's) has a combination now, to show a conflict while recording.
  `publish` already builds the `taken` set.
- A way to capture the next key combination without it running a command
  while recording: a modal "recording" flag, or `on_key` seeing ⌘-combos
  before the menu does. Today a ⌘-combo goes to the menu's key equivalent
  first.

## Acceptance
In the repro, clicking "Bind to ⌘L" makes ⌘L run the command and ⌘J stop,
with no reload. Rebinding to a key another command has is either refused
or reported, so the menu bar is never rejected for a duplicate
accelerator (see the unique-accelerator trap: `set_menus` rejects a whole
bar with a duplicate).

**Adopted (Nov 2, pin `2e22d99d3`).** Settings → Shortcuts rebinds live (screens/shell/keys `key`, `bind`; src/shortcut_tests.rs `a_rebound_shortcut_takes_effect_at_once`); a key another command holds is refused by `ui.shortcut_owner`, as the shipping app refuses it. Still open: `capture_keys`, so a ⌘ key a command holds records instead of running (a TODO in screens/settings/shortcuts).
