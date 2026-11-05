# Ask: an editor's selection stays drawn while its context menu is open

**Priority: high (users see it).** Select text in Zeron's composer (or any field), right-click inside the selection: the menu opens and the selection highlight disappears. It comes back when the menu closes. In AppKit the text view's selection stays highlighted the whole time the menu is up, which tells you what Cut, Copy and Look Up will act on.

**Seen on:** zgx `a2c9cf1` (Zeron's pin), macOS.

**Why:** the menu panel takes the focus (it has to, for its arrow keys and typeahead), so the editor blurs. An editable field draws its selection only while focused (`widgets.rs` ~5386: `shows_selection = focus || window_blurred || read_only`). The selection itself is kept (Zeron's host checks: `(0, 5)` before, while open, and after). Only its drawing goes. zgx's own context menu blurs the editor the same way.

**Ask:** a way to keep an editor's selection drawn while a popup it opened (its context menu) holds the focus. For example:

- the editor counts as "shown as focused" while a menu or popup anchored in it, or opened from its `context_menu`, is open; or
- a public `EditorHandle::keep_selection_shown(bool)` (or a `keep_selection` prop) that a host sets while its own menu is open, and clears when the menu closes. The selection is drawn in the inactive colour, as AppKit draws it under a menu.

**Zeron meanwhile:** src/editor_menu.rs records the editor and its range when the menu opens. Each item gives the editor the focus back and restores that range before acting, so the actions are right. The highlight still vanishes while the menu is open.

**Repro:** `cargo test --bin zeron-zgx editor_menu::tests::each_item_acts_on_the_selection_the_menu_opened_on_the_first_time` in apps/zgx. While the menu is open, `ui.focused_editor()` is `None` and `editor.selection()` is still `(0, 5)`.
