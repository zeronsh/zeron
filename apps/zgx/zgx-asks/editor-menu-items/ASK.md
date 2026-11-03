# Ask: the spelling checker's guesses, for an app's own text menu

**Priority: low.** Zeron draws its own text menu on every editor (ds/editor_menu: Look Up, Cut, Copy, Paste, Select All, through the host's `EditorHandle` and `Ui::paste_into`), so it looks like the app's other menus. The one thing zgx's own menu had that it can't reach is the spelling checker's guesses for a misspelled word: `editor_spell::menu_items` is `pub(crate)`. Zeron's menu has no spelling guesses until this lands.

**Seen on:** zgx `0e6c32f` (Zeron's pin), macOS.

**Ask:** a public way, for a host, to get the checker's guesses for the misspelled word at a point (or under the caret) in an editor, and to apply one:

```rust
// For the editor and a window point (or the caret when None):
fn spelling_guesses(&self, editor: &EditorHandle, at: Option<(f32, f32)>, platform: &PlatformService)
    -> Option<SpellingGuesses>; // { word_range, guesses: Vec<String> }, None when the word there is spelled right
fn apply_guess(&self, editor: &EditorHandle, guesses: &SpellingGuesses, which: usize);
// (and, as AppKit's menu has them, Learn / Ignore Spelling for that word)
```

**Done so far (this ask's first form, "the built-in entries as data"):** no longer needed. The host reaches every other entry already.
