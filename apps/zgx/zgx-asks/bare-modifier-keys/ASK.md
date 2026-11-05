# Ask: report a modifier pressed alone

**Priority: low.** Zeron's shipping app shows ⌘1-⌘9 jump hints on the sidebar's sessions while ⌘ is held alone; zgx Zeron can't.

**Today.** `crates/zgx-desktop/src/application.rs:4234` handles `WindowEvent::ModifiersChanged` by storing `self.modifiers` only; nothing is dispatched, so no `on_key`, root `on_key` or command hears it.

**Repro:** `ui.div { focusable = true, on_key = function(e) print(e.key, e.meta) end }`, focus it, press and release ⌘ alone: nothing prints; ⌘1 prints.

**Ask:** an event when a modifier alone goes down or up (or a reactive `ui.modifiers` state), cleared on window blur so a ⌘-Tab away doesn't leave it set.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the shell chat's notes.

**Stopgap in the app (Nov 2):** `src/command_hold.rs` hears ⌘ alone from an AppKit local event monitor and publishes the shared state `keys.command_alone`, which the sidebar's ⌘1–9 hints read (`screens/shell/sidebar.luau`, `M.jump_hints`). Swap it for zgx's event once this lands, and delete the monitor.

**Adopted (Nov 2, pin `2e22d99d3`).** The sidebar reads `ui.modifiers`; `src/command_hold.rs` is gone. Follow-ups for the zgx lane:
- A key whose command has it (⌘E, ⌘N…) goes to its menu item before any `on_key` hears it, so the shell root can't tell the hints "a key went down". Zeron's shell commands tell them themselves (screens/shell/init); a plugin's command key, or the browser's (⌘T, ⌘L), leaves them up while ⌘ stays down. Wanted: a key-down the menu bar takes still reported (an `on_key` that sees it, observe-only), or `ui.modifiers` saying a key went down meanwhile.
- From source, not checked by hand: while a web page (WKWebView) has focus, its `flagsChanged:` doesn't reach winit, so `ui.modifiers.meta` may stay up and the hints not show. The AppKit monitor heard it.
