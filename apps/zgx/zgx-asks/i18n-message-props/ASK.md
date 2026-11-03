# Ask: messages (`ui.t`) where only strings go today: menus, tooltips, a plugin's own catalog

**Priority: medium.** Zeron speaks Simplified Chinese as well as English (Settings → General → Language, `ui/locales`). A language change applies at once to everything that takes a message, but not to:

1. **Menu items.** `ui.command { title }` takes a string, read when the script loads, so the menu bar keeps the old language until the next launch (Zeron says so under the setting). The top-level menus are named by `menu = "File"`, and zgx-desktop finds the Edit, View and Window menus by those English names (`app_menu.rs`), so a menu's own title can't be translated at all.
2. **Tooltips.** `tooltip` takes a string or a node, not a message, so a tooltip shows the old language until its node is built again. A node tooltip draws its own frame (see [tooltip-function](../tooltip-function/ASK.md)).
3. **A plugin's own words.** One catalog per host (`Locales`): a plugin that calls `ui.i18n` would replace the app's catalog, and its `dir` has to be inside the plugin's folder anyway. Plugins have no way to ship their own locales beside the app's.

**Ask:**
1. `ui.command` takes a message for `title`, relabelling the item when the locale changes. A `menu` takes an id and a separate (message) title, so Edit/View/Window keep their identity in any language.
2. `tooltip` takes a message (and a function or state, as text does).
3. A per-plugin catalog: `ui.i18n { dir }` in a plugin's VM loads `<plugin>/locales` under the plugin's own namespace, following the host's locale (falling back to the plugin's source locale), without touching the app's.

**Seen on:** zgx `c9d85c6` (Zeron's pin), macOS.
