# Ask: a menu bar that one bad item, or one more window, can't take away

**Priority: medium.** Zeron works around all of it now (apps/zgx `src/menu_bar.rs`, Nov 3), but each of these cost every shortcut in the app at least once, and any other zgx app will hit them too.

**Seen on:** zgx `99a50ad5` (Zeron's pin), macOS.

## What happened in Zeron
A shortcut reaches an app only as a menu item's key equivalent. Zeron's users kept reporting that "every shortcut dies when a plugin loads or unloads, until a relaunch". The cause: the consent sheet a plugin asks for opens in a window of its own. That window served its commands as every window did, and `WindowFactory::set_menus` is the **whole application's**. So the sheet's bar, which had none of Zeron's commands, replaced the main window's bar and the keymap bound at its root. When the sheet closed, nothing set the main window's bar back. `set_menus` succeeded each time, so nothing was logged.

## Asks (any one helps; 1 and 2 matter most)

1. **`set_menus` drops what it can't show instead of refusing the whole bar.** Today one duplicate accelerator, one accelerator `MenuAccelerator::name` can't name (`⌘?`, `⌘é`: a single character outside ASCII letters/digits and `` `\[],=-.';/ ``), one empty menu label, or one action twice rejects everything, and the previous bar stays (with none before it, there are no shortcuts at all). Please install the bar with the offending accelerator (or item) left off, and say what was left off: e.g. `Result<MenuReport, AppMenuError>` with a `dropped: Vec<(MenuAction, Reason)>`. Keep `Err` for when nothing can be installed.

2. **Menus per window, or at least a way to tell a sheet from the window that has the bar.** For example, `WindowOptions::menus: MenuSource::{App, Own(Vec<AppMenu>), Inherit}`, where macOS switches the main menu as the key window changes and Linux draws each window's own. A window that inherits never touches the app's bar. Failing that, put a loud doc line on `set_menus`: "replaces every window's bar; a secondary window calling it takes the main window's shortcuts away."

3. **Public validation.** `zgx_desktop::validate_menus(&[AppMenu]) -> Result<(), AppMenuError>` and `MenuAccelerator::is_nameable()`. Zeron uses `label().is_empty()` as a proxy for `name()` failing, and `app_menu_bar(menus, |_| {})` in tests just to reach `validate`.

4. **zgx-script: a script that fails to load leaves its commands published.** A plugin that declares a command and then errors is set aside, but its `ui.command` stays in `ScriptHost::commands()` (with its key, so it's in the menu bar), and `run_command` on it returns `false`, because its runner holds the dropped VM. Please discard what a failed script declared (`pending`, `pending_keys`) when it's left out of the load. Repro: `repro/plugin.luau` loaded as a plugin beside any app. After the load, `commands()` has `flaky/flake` and `run_command("flaky/flake")` is `false`.

5. **(Low) A native-menu build error exits the app.** `NativeMenu::new` failing in `Event::MenusChanged` or `install_menu_bar` calls `fail`, which ends the event loop, for example if muda can't parse an accelerator name that `validate` accepted. Please drop that item and log instead.

## Acceptance
- With ask 1, a bar where two items share `⌘J` installs. The second item has no accelerator, and the report names it.
- With ask 2, a window opened with `Inherit` and closed again leaves the app's bar as it was.
- With ask 4, the repro's `commands()` has no `flaky/flake`.

## Zeron, the day each lands
1: `Bar::of` keeps its own conflict rule (Zeron's keys first), but the `MENU BAR REFUSED` fallback becomes unreachable. 2: consent sheets open with `Inherit`, and `MenuBar`'s owner stack can go. 3: `Bar::of` calls the public check per item. 4: the bridge's withdraw-on-failure stops being what keeps a dead command out of the menu bar.
