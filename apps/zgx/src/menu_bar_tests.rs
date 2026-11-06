//! The app's one menu bar (src/menu_bar.rs), as the desktop serves it to
//! every window: zgx-desktop's `set_menus` replaces the bar of the whole
//! application, refuses a whole bar for one bad item (and keeps the last),
//! and each window binds the bar's keymap at its root, so a shortcut is a
//! menu item's or nothing. Played here headless: `Desktop` validates a bar
//! exactly as zgx-desktop's app_menu.rs `validate` does and keeps the last
//! it took; `Window` mounts a script, binds that bar's keys at its root and
//! hears the menu action a key reaches, as zgx-desktop's `MenuFallback`.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};

use zgx::{
    actions::{Action, KeyBinding, Keymap, KeymapBinding},
    input::InputEvent,
};
use zgx_desktop::{AppMenu, AppMenuEntry, AppMenuError, MenuAccelerator, MenuAction};
use zgx_script::{ScriptHost, testing::TestApp};

use crate::menu_bar::{MenuBar, Role, Served};

/// Why zgx-desktop's `set_menus` refuses `menus`, if it does: its own
/// check (app_menu.rs `validate`, which `app_menu_bar` runs first).
pub(crate) fn refusal(menus: &[AppMenu]) -> Option<String> {
    zgx_desktop::app_menu_bar(menus.to_vec(), |_| {}).err().map(|e| e.0)
}

/// zgx-desktop's application as far as menus go: one bar for every window.
#[derive(Default)]
pub(crate) struct Desktop {
    /// The bar it took last (a refused one leaves it).
    pub menus: RefCell<Vec<AppMenu>>,
    /// Each bar it refused, and why.
    pub refusals: RefCell<Vec<String>>,
    /// How many bars it took.
    pub installs: Cell<usize>,
    /// Refuse any bar with a plugin's item (a platform refusing what it
    /// shouldn't: the fallback's case).
    pub refuse_plugins: Cell<bool>,
}

impl Desktop {
    pub fn new() -> Rc<Self> {
        Rc::default()
    }
    /// A menu bar for these windows (as `menu_bar::of` is the app's).
    pub fn bar(self: &Rc<Self>) -> Rc<MenuBar> {
        MenuBar::new(Platform(self.clone()))
    }
    /// The action of the item `keys` is the shortcut of, in the bar now.
    pub fn action_of(&self, keys: &str) -> Option<String> {
        let (key, modifiers) = zgx_script::driver::parse_keys(keys).ok()?;
        let stroke = zgx::actions::Keystroke::new(key, modifiers);
        items(&self.menus.borrow()).into_iter().find(|(_, _, k)| k.as_ref().is_some_and(|k| same_stroke(&k.0, &stroke))).map(|(_, action, _)| action)
    }
}

fn same_stroke(a: &zgx::actions::Keystroke, b: &zgx::actions::Keystroke) -> bool {
    let lower = |key: &zgx::input::Key| match key {
        zgx::input::Key::Character(c) => zgx::input::Key::Character(c.to_ascii_lowercase()),
        key => key.clone(),
    };
    a.modifiers == b.modifiers && lower(&a.key) == lower(&b.key)
}

/// Every item of `menus`: its label, its action and its shortcut.
pub(crate) fn items(menus: &[AppMenu]) -> Vec<(String, String, Option<MenuAccelerator>)> {
    let mut out = Vec::new();
    for menu in menus {
        for entry in &menu.entries {
            match entry {
                AppMenuEntry::Item { label, action, accelerator, .. } => out.push((label.clone(), action.0.clone(), accelerator.clone())),
                AppMenuEntry::Submenu(menu) => out.extend(items(std::slice::from_ref(menu))),
                _ => {}
            }
        }
    }
    out
}

struct Platform(Rc<Desktop>);

impl crate::menu_bar::Platform for Platform {
    fn set_menus(&self, menus: Vec<AppMenu>) -> Result<(), AppMenuError> {
        let plugins = items(&menus).iter().any(|(_, action, _)| action.contains('/'));
        if let Some(why) = refusal(&menus).or_else(|| (plugins && self.0.refuse_plugins.get()).then(|| "refused (the test's)".to_string())) {
            self.0.refusals.borrow_mut().push(why.clone());
            return Err(AppMenuError(why));
        }
        *self.0.menus.borrow_mut() = menus;
        self.0.installs.set(self.0.installs.get() + 1);
        Ok(())
    }
    fn set_reserved_shortcuts(&self, _keys: Vec<MenuAccelerator>) -> Result<(), AppMenuError> {
        Ok(())
    }
}

/// A window the desktop serves: its scripts, its part in the menu bar, and
/// the bar's keys bound at its root.
pub(crate) struct Window {
    // (Fields drop in order: its part in the bar before its runtime.)
    _served: Served,
    keys: Option<KeymapBinding>,
    heard: Rc<RefCell<Vec<String>>>,
    pub app: TestApp,
}

impl Window {
    /// `entry` (under ui/ if relative) in a window, its host made by `host`
    /// (plugins set, actions stubbed), served in `bar` as `role`.
    pub fn open(bar: &Rc<MenuBar>, entry: &str, role: Role, ready: impl FnOnce(ScriptHost) -> ScriptHost) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui").join(entry);
        let app = TestApp::new(move |runtime| {
            let host = ScriptHost::new(runtime, path);
            host.action("close", |_| {});
            ready(host)
        });
        Self::serve(bar, app, role)
    }

    /// `app`, a window mounted, served in `bar` as `role`.
    pub fn serve(bar: &Rc<MenuBar>, app: TestApp, role: Role) -> Self {
        app.ui.scene.borrow_mut().resize(1200., 800.);
        app.frames(30);
        let heard: Rc<RefCell<Vec<String>>> = Rc::default();
        let into = heard.clone();
        let root = app.ui.scene.borrow().root();
        let listener = app.ui.input.listen(root, move |event| {
            if event.phase == zgx::input::EventPhase::Capture || event.default_prevented() {
                return;
            }
            if let InputEvent::Action(action) = &event.event
                && let Some(action) = action.downcast_ref::<MenuAction>()
            {
                into.borrow_mut().push(action.0.clone());
                event.prevent_default();
            }
        });
        std::mem::forget(listener);
        let preview = app.ui.runtime.signal(None);
        let served = bar.serve(&app.ui.runtime, &app.host, preview, role);
        Window { _served: served, keys: None, heard, app }
    }

    /// The bar's keymap at the root, as zgx-desktop binds it in every window
    /// as the bar changes (`Event::MenusChanged`).
    pub fn bind(&mut self, desktop: &Desktop) {
        let mut map = Keymap::new();
        for (_, action, key) in items(&desktop.menus.borrow()) {
            if let Some(key) = key {
                let mut stroke = key.0;
                if let zgx::input::Key::Character(s) = &mut stroke.key {
                    *s = s.to_ascii_lowercase();
                }
                map = map.bind(KeyBinding::new([stroke], Action::new(MenuAction(action))).expect("a shortcut"));
            }
        }
        self.keys = None;
        let root = self.app.ui.scene.borrow().root();
        self.keys = Some(self.app.ui.input.bind_keys(root, map));
    }

    /// The menu action `keys` reached in this window, the bar's keys bound
    /// afresh first; nothing runs.
    pub fn reach(&mut self, desktop: &Desktop, keys: &str) -> Option<String> {
        self.bind(desktop);
        self.heard.borrow_mut().clear();
        self.app.keys(keys);
        self.app.frames(2);
        self.heard.borrow_mut().drain(..).next()
    }
}

/// The shell (ui/shell_fixture.luau) in a window that has the menu bar.
pub(crate) fn shell(bar: &Rc<MenuBar>, plugins: Vec<zgx_script::Plugin>) -> Window {
    let w = Window::open(bar, "shell_fixture.luau", Role::Bar, move |host| {
        if !plugins.is_empty() {
            host.set_plugins(plugins);
        }
        host
    });
    assert_eq!(w.app.error(), None, "the shell failed");
    w
}

/// Zeron's own shortcuts in `w` now: each command's id and key.
pub(crate) fn zerons(w: &Window) -> Vec<(String, String)> {
    w.app.host.commands().get().into_iter().filter(|c| !c.id.contains('/')).filter_map(|c| c.key.map(|k| (c.id, k))).collect()
}

/// Each of `expected` (id, key) the key doesn't reach in `w`.
pub(crate) fn unreached(w: &mut Window, desktop: &Desktop, expected: &[(String, String)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (id, key) in expected {
        match w.reach(desktop, key) {
            Some(got) if got == *id => {}
            other => wrong.push(format!("{key} reached {other:?}, not {id}")),
        }
    }
    wrong
}

/// A plugin `name` whose init.luau is `body`, written under `dir`.
pub(crate) fn plugin(dir: &Path, name: &str, body: &str) -> zgx_script::Plugin {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("init.luau"), body).unwrap();
    zgx_script::Plugin { name: name.into(), entry: d.join("init.luau"), grants: vec![] }
}

/// A scratch folder for this test's plugins.
pub(crate) fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-menu-bar-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The reported bug (Nov 3): a plugin added or updated asks for consent in
/// a sheet of its own (src/plugins/consent.rs), and from then on no
/// shortcut works until Zeron is relaunched. The sheet's window is served
/// as any window was: its scripts' commands (consent.luau has none) made
/// the app's menu bar, for every window, so the main window's ⌘K, ⌘N, ⌘W…
/// left with it; closed, nothing gave the main window's back.
#[test]
fn a_consent_sheet_never_takes_the_app_s_shortcuts() {
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let mut main = shell(&bar, Vec::new());
    let expected = zerons(&main);
    assert!(expected.len() > 20, "the shell's shortcuts: {expected:?}");
    let opened = unreached(&mut main, &desktop, &expected);
    assert!(opened.is_empty(), "the shell alone:\n{}", opened.join("\n"));

    // The consent sheet opens over it.
    let sheet = Window::open(&bar, "consent.luau", Role::Sheet, |host| {
        // (What src/plugins/consent.rs gives it.)
        host.action("appearance", |_| {});
        host
    });
    assert_eq!(sheet.app.error(), None, "the sheet failed");
    let over = unreached(&mut main, &desktop, &expected);

    // Answered: it closes.
    drop(sheet);
    let after = unreached(&mut main, &desktop, &expected);
    assert!(
        over.is_empty() && after.is_empty(),
        "{} of {} shortcuts lost with the consent sheet open:\n{}\n\n{} still lost after it closed:\n{}",
        over.len(),
        expected.len(),
        over.join("\n"),
        after.len(),
        after.join("\n")
    );
    // And ⌘K still runs its command (its VM the live one).
    assert!(main.app.host.run_command("shell.palette"));
    assert!(desktop.refusals.borrow().is_empty(), "refused: {:?}", desktop.refusals.borrow());
}

/// What the bar of `w`'s commands leaves off its plugins' items, and why.
pub(crate) fn dropped(w: &Window) -> Vec<crate::menu_bar::Dropped> {
    let host = &w.app.host;
    crate::menu_bar::Bar::of(&host.commands().get(), &host.reserved_keys().get(), &host.dropped_shortcuts(), None).dropped
}

/// ⌘K as the platform writes it with the other modifier name: `meta` is ⌘
/// on a Mac (as `cmd` is), Control elsewhere.
fn alias(key: &str) -> String {
    key.replace("cmd", if cfg!(target_os = "macos") { "meta" } else { "ctrl" })
}

/// A plugin's shortcut Zeron has (spelled as Zeron spells it, or another
/// way), one no menu can show, and one in a menu with no name: Zeron keeps
/// all of its own, the plugin's free key works, its other commands stay in
/// the menu bar without theirs, and its row says why.
#[test]
fn a_plugin_s_key_zeron_has_stays_zeron_s_and_its_row_says_so() {
    let dir = scratch("zerons");
    let body = r#"
ui.command { id = "mine", title = "Mine", menu = "Keys", key = "cmd+k", run = function() end }
ui.command { id = "alias", title = "Also new", menu = "Keys", key = "ALIAS", run = function() end }
ui.command { id = "free", title = "Free", menu = "Keys", key = "cmd+shift+y", run = function() end }
ui.command { id = "odd", title = "Odd", menu = "Keys", key = "cmd+shift+?", run = function() end }
ui.command { id = "accent", title = "Accent", menu = "Keys", key = "cmd+é", run = function() end }
ui.command { id = "nameless", title = "Nameless", menu = "", key = "cmd+shift+u", run = function() end }
"#
    .replace("ALIAS", &alias("cmd+n"));
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let mut w = shell(&bar, vec![plugin(&dir, "keys", &body)]);
    let expected = zerons(&w);
    assert!(desktop.refusals.borrow().is_empty(), "refused: {:?}", desktop.refusals.borrow());
    let wrong = unreached(&mut w, &desktop, &expected);
    assert!(wrong.is_empty(), "Zeron's:\n{}", wrong.join("\n"));
    for (keys, id) in [("cmd+shift+y", "keys/free"), ("cmd+shift+u", "keys/nameless")] {
        assert_eq!(w.reach(&desktop, keys).as_deref(), Some(id), "the plugin's {keys}");
        assert!(w.app.host.run_command(id), "{id} runs");
    }
    // Each stays in the menu bar, without the key it couldn't have.
    let shown = items(&desktop.menus.borrow());
    for id in ["keys/mine", "keys/alias", "keys/odd", "keys/accent"] {
        let item = shown.iter().find(|(_, action, _)| action == id).unwrap_or_else(|| panic!("{id} isn't in the menu bar"));
        assert!(item.2.is_none(), "{id} kept a key: {:?}", item.2);
    }
    use crate::menu_bar::Why;
    let lost = dropped(&w);
    let why = |title: &str| lost.iter().find(|d| d.title == title).map(|d| d.why.clone());
    assert_eq!(why("Mine"), Some(Why::Zerons("Search".into())), "{lost:?}");
    assert_eq!(why("Also new"), Some(Why::Zerons("New chat".into())), "{lost:?}");
    assert_eq!(why("Odd"), Some(Why::NoKey), "{lost:?}");
    assert_eq!(why("Accent"), Some(Why::NoKey), "{lost:?}");
    assert_eq!(lost.len(), 4, "{lost:?}");
    // Its row on the Plugins page.
    let notice = crate::plugins::shortcut_notice(&lost, |id| id.to_string());
    assert_eq!(notice, "Its ⌘K for “Mine” is already Zeron's (Search), so it's in the menu bar without one. So are 3 more of its commands.");
}

/// Two plugins with one key: the one loaded first keeps it, the other's
/// command stays in the menu bar without it (and still runs), and its row
/// names the plugin that has it.
#[test]
fn two_plugins_with_one_key_the_first_keeps_it() {
    let dir = scratch("rivals");
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let first = plugin(&dir, "first", r#"ui.command { id = "go", title = "Go", menu = "First", key = "cmd+shift+y", run = function() end }"#);
    let second = plugin(&dir, "second", r#"ui.command { id = "go", title = "Go too", menu = "Second", key = "meta+shift+y", run = function() end }"#);
    let mut w = shell(&bar, vec![first, second]);
    let expected = zerons(&w);
    assert!(desktop.refusals.borrow().is_empty(), "refused: {:?}", desktop.refusals.borrow());
    assert!(unreached(&mut w, &desktop, &expected).is_empty());
    assert_eq!(w.reach(&desktop, "cmd+shift+y").as_deref(), Some("first/go"));
    assert!(w.app.host.run_command("second/go"), "the other still runs (from the menu bar)");
    let lost = dropped(&w);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].why, crate::menu_bar::Why::Plugins("first".into()));
    let notice = crate::plugins::shortcut_notice(&lost, |id| if id == "first" { "First".into() } else { id.into() });
    assert!(notice.starts_with("Its ⇧⌘Y for “Go too” is already First's"), "{notice}");
}

/// R27: a shortcut that's a key alone, from a function: the command stays,
/// without it, and its row says why (written plainly, it fails as it loads:
/// `loading_tests`).
#[test]
fn a_key_alone_from_a_function_is_dropped_and_said() {
    let dir = scratch("bare");
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let keys = plugin(&dir, "keys", r#"ui.command { id = "go", title = "Go", menu = "Keys", key = function() return "k" end, run = function() end }"#);
    let mut w = shell(&bar, vec![keys]);
    assert_eq!(w.reach(&desktop, "k"), None);
    assert!(w.app.host.run_command("keys/go"), "it still runs (from the menu bar)");
    let lost = dropped(&w);
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].why, crate::menu_bar::Why::NoModifier);
    let notice = crate::plugins::shortcut_notice(&lost, |id| id.to_string());
    assert!(notice.starts_with("Its K for “Go” has no ⌘, ⌃ or ⌥"), "{notice}");
}

/// A plugin's commands loaded, reloaded and unloaded: never twice, never a
/// refused bar, and gone with it.
#[test]
fn a_plugin_s_commands_come_and_go_with_it() {
    let dir = scratch("comes-and-goes");
    let entry = plugin(&dir, "keys", r#"
ui.command { id = "mine", title = "Mine", menu = "Keys", key = "cmd+n", run = function() end }
ui.command { id = "free", title = "Free", menu = "Keys", key = "cmd+shift+y", run = function() end }
"#);
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let mut w = shell(&bar, vec![entry]);
    let expected = zerons(&w);
    let check = |w: &mut Window, when: &str, free: bool| {
        assert!(desktop.refusals.borrow().is_empty(), "{when}: refused: {:?}", desktop.refusals.borrow());
        let wrong = unreached(w, &desktop, &expected);
        assert!(wrong.is_empty(), "{when}:\n{}", wrong.join("\n"));
        assert_eq!(w.reach(&desktop, "cmd+shift+y").as_deref(), free.then_some("keys/free"), "{when}: the plugin's free key");
    };
    check(&mut w, "loaded", true);
    w.app.host.reload().unwrap();
    w.app.frames(10);
    check(&mut w, "reloaded", true);
    w.app.host.set_plugins(vec![]);
    w.app.host.reload().unwrap();
    w.app.frames(10);
    check(&mut w, "unloaded", false);
    assert!(!items(&desktop.menus.borrow()).iter().any(|(_, a, _)| a.starts_with("keys/")), "its items stayed");
}

/// The platform refusing a bar anyway (it shouldn't: `Bar::of` makes one
/// it takes): Zeron's own goes in, never nothing.
#[test]
fn a_bar_refused_anyway_leaves_zeron_s_own() {
    let dir = scratch("refused");
    let desktop = Desktop::new();
    desktop.refuse_plugins.set(true);
    let bar = desktop.bar();
    let mut w = shell(&bar, vec![plugin(&dir, "keys", r#"ui.command { id = "free", title = "Free", menu = "Keys", key = "cmd+shift+y", run = function() end }"#)]);
    let expected = zerons(&w);
    assert!(!desktop.refusals.borrow().is_empty(), "the test's platform refused it");
    let wrong = unreached(&mut w, &desktop, &expected);
    assert!(wrong.is_empty(), "Zeron's own after a refusal:\n{}", wrong.join("\n"));
    assert!(items(&desktop.menus.borrow()).iter().any(|(_, a, _)| a == crate::plugins::USE_ZERON_TEMPLATE));
}

/// A window with a bar of its own opened over the main window (setup over
/// it, say) has the bar while it's open; closed, the main window's is back.
#[test]
fn the_last_window_opened_has_the_bar_and_the_one_under_it_gets_it_back() {
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let mut main = shell(&bar, Vec::new());
    let expected = zerons(&main);
    let dir = scratch("over");
    std::fs::write(dir.join("over.luau"), r#"
ui.command { id = "over.only", title = "Only here", menu = "File", key = "cmd+k", run = function() end }
return ui.column {}
"#).unwrap();
    let over = Window::open(&bar, dir.join("over.luau").to_str().unwrap(), Role::Bar, |host| host);
    assert_eq!(desktop.action_of("cmd+k").as_deref(), Some("over.only"), "the window opened last has the bar");
    drop(over);
    let wrong = unreached(&mut main, &desktop, &expected);
    assert!(wrong.is_empty(), "after it closed:\n{}", wrong.join("\n"));
}

/// The host's own View items: "Use Zeron v3" always, "Keep <template>…"
/// while one is in preview, after the scripts' View items; a script's
/// command with a host id (`zeron.`) is left out, never a second item.
#[test]
fn the_host_s_own_items_follow_a_preview_and_no_script_takes_their_ids() {
    let command = |id: &str, title: &str| zgx_script::Command { id: id.into(), title: title.into(), menu: "View".into(), key: None, separator: false };
    let commands = vec![command("shell.toggle_sidebar", "Toggle sidebar"), command(crate::plugins::KEEP_TEMPLATE, "Mine")];
    let preview = crate::plugins::preview::Preview { id: "t".into(), name: "Dusk".into() };
    let bar = crate::menu_bar::Bar::of(&commands, &[], &[], Some(&preview));
    assert_eq!(refusal(&bar.menus), None);
    let view: Vec<(String, String)> = items(&bar.menus).into_iter().map(|(label, action, _)| (label, action)).collect();
    assert_eq!(
        view,
        vec![
            ("Toggle sidebar".into(), "shell.toggle_sidebar".into()),
            ("Keep Dusk…".into(), crate::plugins::KEEP_TEMPLATE.into()),
            ("Use Zeron v3".into(), crate::plugins::USE_ZERON_TEMPLATE.into()),
        ]
    );
    let none = crate::menu_bar::Bar::of(&commands, &[], &[], None);
    assert!(!items(&none.menus).iter().any(|(label, _, _)| label.starts_with("Keep")), "no preview, no Keep");
}
