# Ask: say when a plugin command's shortcut is dropped

**Priority: low.** A plugin's shortcut that collides with the app's by name is cleared without a word, so its owner never learns why it doesn't work.

**Today.** Only alias collisions (`meta+n` vs `cmd+n`), which Zeron catches itself, reach the Plugins row.

**Ask:** mark the command (e.g. `Command.key_taken: Option<String>`) so the app can tell the plugin's owner.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the plugin chat's notes.
