//! The main window's shortcuts reach their commands from wherever focus is,
//! as a desktop window routes them. A command's shortcut is its menu item's
//! (src/main.rs `serve_commands`): the window binds the menus' keymap at
//! its root, a key goes first to what has focus, and one nothing takes
//! reaches the root as the menu item's action, which runs the command. This
//! mounts the shell headless (ui/shell_fixture.luau), binds the commands'
//! keymap and listens at the root as zgx-desktop does, and presses each
//! shortcut with focus on nothing, in the composer, on a sidebar row and in
//! a file opened in the panel.

use std::{cell::RefCell, rc::Rc};

use zgx::{
    actions::{Action, KeyBinding, Keymap, KeymapBinding},
    input::InputEvent,
};
use zgx_desktop::MenuAction;
use zgx_script::{ScriptHost, Value, testing::TestApp};

struct Window {
    app: TestApp,
    /// Whether the scripts asked the window to close (`close`, stubbed).
    closed: Rc<std::cell::Cell<bool>>,
    /// The commands the root heard, in order (each run as the desktop runs
    /// a menu item's).
    heard: Rc<RefCell<Vec<String>>>,
    _keys: KeymapBinding,
}

fn mount() -> Window {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let closed: Rc<std::cell::Cell<bool>> = Rc::default();
    let asked = closed.clone();
    let app = TestApp::new(|runtime| {
        let host = ScriptHost::new(runtime, path);
        host.action("close", move |_| asked.set(true));
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    // The menus' keymap at the root, as zgx-desktop binds it from the menu
    // bar src/main.rs makes of the commands (a letter in either case).
    let commands = app.host.commands().get();
    // (One shortcut, one command: zgx-desktop refuses a menu bar with two
    // alike, and then no shortcut works at all.)
    let mut seen: Vec<(String, String)> = Vec::new();
    for command in &commands {
        if let Some(key) = command.key.as_deref().map(str::to_ascii_lowercase) {
            if let Some((_, other)) = seen.iter().find(|(k, _)| *k == key) {
                panic!("`{key}` is both {other}'s and {}'s", command.id);
            }
            seen.push((key, command.id.clone()));
        }
    }
    let mut map = Keymap::new();
    for command in commands {
        let Some(key) = command.key.as_deref() else {
            continue;
        };
        // (Every shortcut a menu can show: one it can't would never fire.)
        let accelerator = crate::accelerator(key).unwrap_or_else(|| panic!("{}: `{key}` isn't a shortcut", command.id));
        let mut stroke = accelerator.0;
        if let zgx::input::Key::Character(s) = &mut stroke.key {
            *s = s.to_ascii_lowercase();
        }
        map = map.bind(KeyBinding::new([stroke], Action::new(MenuAction(command.id.clone()))).expect("a shortcut"));
    }
    let root = app.ui.scene.borrow().root();
    let keys = app.ui.input.bind_keys(root, map);
    let heard: Rc<RefCell<Vec<String>>> = Rc::default();
    let into = heard.clone();
    // What reaches the root untaken is the menu item's (zgx-desktop's
    // `MenuFallback`).
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
    Window { app, closed, heard, _keys: keys }
}

impl Window {
    /// Presses `keys` and runs the command it reached, if any; its id.
    fn press(&mut self, keys: &str) -> Option<String> {
        self.heard.borrow_mut().clear();
        self.app.keys(keys);
        let heard: Vec<String> = self.heard.borrow_mut().drain(..).collect();
        for id in &heard {
            assert!(self.app.host.run_command(id), "{id} didn't run");
        }
        self.app.frames(30);
        heard.into_iter().next()
    }
    /// A named state's value (`ui.state`, `ui.persist`), as devtools show it.
    fn state(&self, name: &str) -> Value {
        // (A module's own named state is `module.luau::name`.)
        self.app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n == name || n.ends_with(&format!("::{name}")))
            .map_or(Value::Nil, |(_, v)| v)
    }
    fn route(&mut self) -> String {
        match self.state("shell_route_path") {
            Value::Str(s) => s.to_string(),
            other => format!("{other:?}"),
        }
    }
}

/// Where focus is put before each shortcut.
#[derive(Clone, Copy, Debug)]
enum Focus {
    Nothing,
    Composer,
    Sidebar,
    Editor,
}

/// A session open, its panel showing README.md, and focus put at `focus`.
fn focus(w: &mut Window, focus: Focus) {
    let app = &mut w.app;
    app.key(zgx::input::Key::Escape);
    app.frames(10);
    match focus {
        Focus::Nothing => {
            app.ui.input.focus(&app.ui.scene, None);
            app.frames(5);
        }
        Focus::Composer => {
            app.click("#chat-composer >> role:textbox");
            app.frames(10);
        }
        Focus::Sidebar => {
            // (The session open's own row: the route stays.)
            app.click("#shell-sidebar >> role:button \"Fix the build\"");
            app.frames(10);
        }
        Focus::Editor => {
            let readme = "#shell-files >> role:treeitem \"README.md\"";
            assert!(app.wait_until(|app| app.find(readme).is_some()), "README.md never showed in the panel");
            app.click(readme);
            app.frames(20);
            let editor = app.target("#shell-files >> role:textbox").expect("the file's editor");
            assert!(app.ui.input.focus(&app.ui.scene, Some(editor)), "the editor takes focus");
            app.frames(5);
        }
    }
}

#[test]
fn the_shortcuts_reach_their_commands_from_any_focus() {
    let mut w = mount();
    // A session, its panel open (where a file opens, for the editor's case).
    w.app.click("#shell-sidebar >> role:button \"Fix the build\"");
    w.app.frames(20);
    w.app.click("#titlebar >> role:button \"Panel\"");
    w.app.frames(40);
    // A file viewer's tab (README.md in its tree).
    w.app.click("#panel-open-files");
    w.app.frames(40);
    let session = w.route();
    let mut wrong = Vec::new();
    for at in [Focus::Nothing, Focus::Composer, Focus::Sidebar, Focus::Editor] {
        // (`again`: pressed where the last press left focus, the place it
        // was put in now out of reach: a row of the sidebar just folded.)
        let mut press = |w: &mut Window, keys: &str, id: &str, again: bool, what: &dyn Fn(&mut Window) -> bool| {
            if !again {
                focus(w, at);
            }
            match w.press(keys) {
                Some(heard) if heard == id => {
                    if !what(w) {
                        wrong.push(format!("{at:?}: {keys} ran {id}, which did nothing"));
                    }
                }
                other => wrong.push(format!("{at:?}: {keys} reached {other:?}, not {id}")),
            }
        };
        // ⌘K the palette (Esc closes it as the next focus is put).
        press(&mut w, "cmd+k", "shell.palette", false, &|w| w.state("shell_palette_open") == Value::Bool(true));
        // ⌘B the sidebar, folded and back (pressed again where focus went:
        // a row folded away has none).
        press(&mut w, "cmd+b", "shell.toggle_sidebar", false, &|w| w.state("shell_collapsed") == Value::Bool(true));
        press(&mut w, "cmd+b", "shell.toggle_sidebar", true, &|w| w.state("shell_collapsed") == Value::Bool(false));
        // ⌘, settings (Esc goes back to the session).
        press(&mut w, "cmd+,", "shell.settings", false, &|w| w.route().starts_with("/settings"));
        w.app.key(zgx::input::Key::Escape);
        w.app.frames(20);
        // ⌘R the panel, closed and open again.
        press(&mut w, "cmd+r", "shell.toggle_files", false, &|w| w.app.bounds("#shell-files").width == 0.);
        press(&mut w, "cmd+r", "shell.toggle_files", true, &|w| w.app.bounds("#shell-files").width > 0.);
        // ⌃Tab the next session, ⌃⇧Tab back. (In the panel, Zeron's cycle
        // its tabs instead: not the editor's.)
        if !matches!(at, Focus::Editor) {
            press(&mut w, "ctrl+tab", "shell.next_session", false, &|w| w.route() != session);
            press(&mut w, "ctrl+shift+tab", "shell.previous_session", true, &|w| w.route() == session);
        }
        // ⌘N a new chat, then ⌘[ back, ⌘] forward and ⌘[ back again from
        // where focus went (putting it back would move the history).
        press(&mut w, "cmd+n", "shell.new_chat", false, &|w| w.route() == "/");
        press(&mut w, "cmd+[", "router.back", true, &|w| w.route() == session);
        press(&mut w, "cmd+]", "router.forward", true, &|w| w.route() == "/");
        press(&mut w, "cmd+[", "router.back", true, &|w| w.route() == session);
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn command_w_closes_the_panels_tab_while_it_has_the_keyboard_else_the_window() {
    let mut w = mount();
    w.app.click("#shell-sidebar >> role:button \"Fix the build\"");
    w.app.frames(20);
    w.app.click("#titlebar >> role:button \"Panel\"");
    w.app.frames(40);
    // A file viewer's tab (README.md in its tree).
    w.app.click("#panel-open-files");
    w.app.frames(40);
    let tabs = |w: &mut Window| w.app.find("#panel-strip >> role:tab").is_some();
    assert!(tabs(&mut w), "the panel has a tab");
    // In the file: the tab closes, the window stays.
    focus(&mut w, Focus::Editor);
    assert_eq!(w.press("cmd+w").as_deref(), Some("shell.close"));
    w.app.frames(30);
    assert!(!tabs(&mut w), "⌘W in the panel's file left its tab open");
    assert!(!w.closed.get(), "⌘W in the panel closed the window too");
    // In the composer: the panel left be, the window closes (the app with
    // it, as Zeron's). (Its last tab closed, the panel closed with it: open
    // again, a file viewer in it.)
    if w.app.bounds("#shell-files").width == 0. {
        w.app.click("#titlebar >> role:button \"Panel\"");
        w.app.frames(40);
    }
    w.app.click("#panel-open-files");
    w.app.frames(40);
    assert!(tabs(&mut w), "the panel has a tab again");
    focus(&mut w, Focus::Composer);
    assert_eq!(w.press("cmd+w").as_deref(), Some("shell.close"));
    assert!(tabs(&mut w), "⌘W in the composer closed the panel's tab");
    assert!(w.closed.get(), "⌘W in the composer didn't close the window");
}

/// What zgx-desktop's `set_menus` refuses a whole bar for (app_menu.rs
/// `validate`): an empty label or action, an action twice, or a shortcut
/// twice. Refused, the bar before it stays, and with none before it there
/// are no shortcuts at all.
fn refused(menus: &[zgx_desktop::AppMenu]) -> Option<String> {
    let (mut ids, mut keys): (Vec<String>, Vec<zgx::actions::Keystroke>) = (Vec::new(), Vec::new());
    for menu in menus {
        for entry in &menu.entries {
            if let zgx_desktop::AppMenuEntry::Item { label, action, accelerator, .. } = entry {
                if label.is_empty() || action.0.is_empty() {
                    return Some(format!("an empty label or action in {}", menu.label));
                }
                if ids.contains(&action.0) {
                    return Some(format!("the action `{}` twice", action.0));
                }
                ids.push(action.0.clone());
                if let Some(key) = accelerator {
                    if keys.contains(&key.0) {
                        return Some(format!("a shortcut twice ({}'s)", action.0));
                    }
                    keys.push(key.0.clone());
                }
            }
        }
    }
    None
}

/// The window as the desktop serves it: the bar src/main.rs makes of the
/// commands set as the platform would (kept as it was if refused), and its
/// keymap bound at the root, again after each load.
struct Served {
    app: TestApp,
    bar: Option<Vec<zgx_desktop::AppMenu>>,
    refusals: Vec<String>,
    heard: Rc<RefCell<Vec<String>>>,
    keys: Option<KeymapBinding>,
}

impl Served {
    fn new(plugin: Option<&std::path::Path>) -> Self {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
        let entry = plugin.map(std::path::Path::to_path_buf);
        let app = TestApp::new(move |runtime| {
            let host = ScriptHost::new(runtime, path);
            host.action("close", |_| {});
            if let Some(entry) = entry {
                host.set_plugins(vec![zgx_script::Plugin { name: "keys".into(), entry, grants: vec![] }]);
            }
            host
        });
        app.ui.scene.borrow_mut().resize(1200., 800.);
        app.frames(30);
        assert_eq!(app.error(), None, "the shell failed");
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
        let mut s = Served { app, bar: None, refusals: Vec::new(), heard, keys: None };
        s.serve();
        s
    }

    /// The commands' bar, set as src/main.rs sets it.
    fn serve(&mut self) {
        let bar = crate::menus(&self.app.host.commands().get());
        match refused(&bar) {
            Some(why) => self.refusals.push(why),
            None => self.bar = Some(bar),
        }
        let mut map = Keymap::new();
        for menu in self.bar.iter().flatten() {
            for entry in &menu.entries {
                if let zgx_desktop::AppMenuEntry::Item { action, accelerator: Some(key), .. } = entry {
                    map = map.bind(KeyBinding::new([key.0.clone()], Action::new(MenuAction(action.0.clone()))).expect("a shortcut"));
                }
            }
        }
        self.keys = None;
        let root = self.app.ui.scene.borrow().root();
        self.keys = Some(self.app.ui.input.bind_keys(root, map));
    }

    /// What `keys` reached, if anything.
    fn press(&mut self, keys: &str) -> Option<String> {
        self.heard.borrow_mut().clear();
        self.app.keys(keys);
        self.app.frames(5);
        self.heard.borrow_mut().drain(..).next()
    }
}

#[test]
fn a_plugin_s_commands_never_cost_the_app_its_shortcuts() {
    let dir = std::env::temp_dir().join(format!("zgx-shortcuts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("init.luau");
    // ⌘N as the app has it, ⌘N again by another name (`meta` is ⌘ on a
    // Mac, as `cmd` is), and one key free. (zgx refuses an id declared
    // twice as an error: the plugin is set aside, src/plugins/recovery.rs.)
    let alias = if cfg!(target_os = "macos") { "meta+n" } else { "ctrl+n" };
    std::fs::write(&entry, r#"
ui.command { id = "mine", title = "Mine", menu = "Keys", key = "cmd+n", run = function() end }
ui.command { id = "alias", title = "Also new", menu = "Keys", key = "ALIAS", run = function() end }
ui.command { id = "free", title = "Free", menu = "Keys", key = "cmd+shift+y", run = function() end }
"#.replace("ALIAS", alias)).unwrap();
    let mut w = Served::new(Some(&entry));
    let check = |w: &mut Served, when: &str, free: bool| {
        assert!(w.refusals.is_empty(), "{when}: the bar was refused: {:?}", w.refusals);
        assert_eq!(w.press("cmd+n").as_deref(), Some("shell.new_chat"), "{when}: ⌘N");
        assert_eq!(w.press("cmd+k").as_deref(), Some("shell.palette"), "{when}: ⌘K");
        w.app.key(zgx::input::Key::Escape);
        w.app.frames(10);
        let got = w.press("cmd+shift+y");
        assert_eq!(got.as_deref(), if free { Some("keys/free") } else { None }, "{when}: the plugin's free key");
    };
    check(&mut w, "loaded", true);
    // Reloaded: the same commands again, not twice over.
    w.app.host.reload().unwrap();
    w.app.frames(20);
    w.serve();
    check(&mut w, "reloaded", true);
    // Unloaded: its commands go with it.
    w.app.host.set_plugins(vec![]);
    w.app.host.reload().unwrap();
    w.app.frames(20);
    w.serve();
    check(&mut w, "unloaded", false);
    assert!(!w.app.host.commands().get().iter().any(|c| c.id.starts_with("keys/")), "its commands stayed");
}

/// ⌘1–9 open the sidebar's sessions as it shows them, top to bottom
/// (Zeron's jumps); a folded list's sessions are out of reach, and a digit
/// past the last does nothing.
#[test]
fn command_digits_jump_to_the_sidebars_sessions_in_order() {
    let mut w = mount();
    let ids = [
        ("Fix the build", "a1"),
        ("Write the docs", "a2"),
        ("Tidy the tests", "a3"),
        ("Ship the release", "b1"),
        ("Answer the review", "b2"),
    ];
    // The rows top to bottom, as the sidebar shows them.
    let tree = w.app.tree();
    let mut rows: Vec<(f32, &str)> = Vec::new();
    for line in tree.lines() {
        for (title, id) in ids {
            if line.contains(&format!("button \"{title}\"")) {
                let y: f32 = line.rsplit(' ').nth(1).and_then(|xy| xy.split(',').nth(1)).and_then(|y| y.parse().ok()).unwrap();
                rows.push((y, id));
            }
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(rows.len(), ids.len(), "every session shows:\n{tree}");
    for (i, (_, id)) in rows.iter().enumerate() {
        assert_eq!(w.press(&format!("cmd+{}", i + 1)).as_deref(), Some(format!("shell.session_{}", i + 1).as_str()));
        assert_eq!(w.route(), format!("/session/{id}"), "⌘{} opens the row {}", i + 1, i + 1);
    }
    // Past the last: where it was.
    let was = w.route();
    w.press(&format!("cmd+{}", rows.len() + 1));
    assert_eq!(w.route(), was, "a digit with no row does nothing");
    // A group folded: its idle sessions are out of reach. One at work stays
    // listed under it (the view's "Active when collapsed", on by default), so
    // Fix the build is still ⌘1's and Borealis's first is ⌘2's.
    w.app.click("#sidebar >> role:button \"Atlas\"");
    w.app.frames(40);
    w.press("cmd+1");
    assert_eq!(w.route(), "/session/a1", "Atlas folded, its working session is still ⌘1's");
    w.press("cmd+2");
    assert_eq!(w.route(), "/session/b1", "Atlas folded, ⌘2 is Borealis's first");
    w.press("cmd+3");
    assert_eq!(w.route(), "/session/b2", "Atlas's idle sessions are out of reach");
}

/// ⌃Tab and ⌃⇧Tab go round the sidebar's sessions in its order, from a new
/// chat to its first or last; ⌘⇧A archives the session open (and the
/// window goes to a new chat), but not one whose agent is working.
#[test]
fn the_next_previous_and_archive_shortcuts() {
    let mut w = mount();
    // (The one list, by last update: a1, a2, a3, b1, b2.)
    let order = ["a1", "a2", "a3", "b1", "b2"];
    assert_eq!(w.press("ctrl+tab").as_deref(), Some("shell.next_session"));
    assert_eq!(w.route(), "/session/a1", "from a new chat, ⌃Tab opens the first");
    for id in &order[1..] {
        w.press("ctrl+tab");
        assert_eq!(w.route(), format!("/session/{id}"));
    }
    w.press("ctrl+tab");
    assert_eq!(w.route(), "/session/a1", "past the last, round to the first");
    w.press("ctrl+shift+tab");
    assert_eq!(w.route(), "/session/b2", "before the first, round to the last");
    w.press("cmd+n");
    w.press("ctrl+shift+tab");
    assert_eq!(w.route(), "/session/b2", "from a new chat, ⌃⇧Tab opens the last");
    // ⌘⇧A: archived, and on to a new chat.
    assert_eq!(w.press("cmd+shift+a").as_deref(), Some("shell.archive"));
    assert_eq!(w.route(), "/", "the session archived, a new chat");
    // a1's agent is working: it stays.
    w.press("cmd+1");
    assert_eq!(w.route(), "/session/a1");
    w.press("cmd+shift+a");
    assert_eq!(w.route(), "/session/a1", "a working session isn't archived");
    assert!(!w.closed.get());
}

/// ⌘E and ⌘J: the session's panel on its files, on its terminal. Pressed
/// with the panel closed, it opens on that tab (a new one when the panel has
/// none, the one it has when it does); pressed while that tab is in front,
/// the panel closes.
#[test]
fn command_e_and_j_toggle_the_panels_files_and_terminal() {
    let mut w = mount();
    w.app.click("#shell-sidebar >> role:button \"Fix the build\"");
    w.app.frames(20);
    let open = |w: &mut Window| w.app.bounds("#shell-files").width > 0.;
    let tabs = |w: &mut Window, name: &str| w.app.tree().matches(&format!(" tab \"{name}\"")).count();
    // What the panel shows: its files, or (a terminal's tab in front) not.
    let front = |w: &mut Window| -> Option<String> {
        let shows = |w: &mut Window, target: &str| {
            let t = zgx_script::driver::parse(target).unwrap();
            w.app.ui.prepare_frame();
            let found = zgx_script::driver::find_all(&w.app.ui, &w.app.handle, &t).unwrap_or_default();
            let scene = w.app.ui.scene.borrow();
            found.into_iter().any(|id| scene.layout_visible(id) && scene.bounds(id).width > 0.)
        };
        if !shows(w, "#panel") {
            None
        } else if shows(w, "#files-pane") {
            Some("Files".into())
        } else {
            Some("Terminal".into())
        }
    };
    assert!(!open(&mut w), "the panel starts closed");
    assert_eq!(w.press("cmd+e").as_deref(), Some("shell.toggle_files_tab"));
    w.app.frames(30);
    assert!(open(&mut w), "⌘E opens the panel");
    assert_eq!(front(&mut w).as_deref(), Some("Files"), "on a Files tab:\n{}", w.app.tree());
    assert_eq!(tabs(&mut w, "Files"), 1, "a new one");
    w.press("cmd+e");
    w.app.frames(30);
    assert!(!open(&mut w), "⌘E again, its Files in front: closed");
    assert_eq!(w.press("cmd+j").as_deref(), Some("shell.toggle_terminal"));
    w.app.frames(30);
    assert!(open(&mut w), "⌘J opens the panel");
    assert_eq!(front(&mut w).as_deref(), Some("Terminal"), "on a terminal:\n{}", w.app.tree());
    w.press("cmd+e");
    w.app.frames(30);
    assert!(open(&mut w), "⌘E with the terminal in front: still open");
    assert_eq!(front(&mut w).as_deref(), Some("Files"), "its Files tab brought to the front");
    assert_eq!(tabs(&mut w, "Files"), 1, "the one it had, not another");
    w.press("cmd+j");
    w.app.frames(30);
    assert_eq!(front(&mut w).as_deref(), Some("Terminal"), "and back to the terminal it had");
    w.press("cmd+j");
    w.app.frames(30);
    assert!(!open(&mut w), "⌘J again: closed");
}

/// ⌘/ opens the composer's model picker from the composer and the sidebar.
/// (An editable code editor takes ⌘/ first, as Toggle Comment:
/// ds/code_editor's own keys; a read-only one lets it through.)
#[test]
fn command_slash_opens_the_model_picker() {
    let mut w = mount();
    w.app.click("#shell-sidebar >> role:button \"Fix the build\"");
    w.app.frames(20);
    let open = |app: &mut TestApp| app.tree().lines().any(|l| l.contains(" dialog ") && !l.contains("Tooltip") && !l.ends_with(" hidden"));
    for at in [Focus::Composer, Focus::Sidebar] {
        focus(&mut w, at);
        assert!(!open(&mut w.app), "{at:?}: nothing open before");
        assert_eq!(w.press("cmd+/").as_deref(), Some("shell.model_picker"), "{at:?}");
        assert!(w.app.wait_until(|app| open(app)), "{at:?}: the picker opened");
        w.app.key(zgx::input::Key::Escape);
        w.app.frames(30);
    }
}

/// Holds `meta` (and `shift`) as the window reports a modifier alone
/// (zgx's `ui.modifiers`, which `app.keys` never sets: it presses and lets
/// go at once).
fn hold(w: &mut Window, meta: bool, shift: bool) {
    let held = zgx::input::Modifiers { meta, shift, ..Default::default() };
    zgx::input::set_held_modifiers(&w.app.ui.runtime, held);
    w.app.frames(2);
}

/// Whether the first row wears its jump (`find` names one node; nine show).
fn hints(w: &mut Window) -> bool {
    w.app.tree().lines().any(|line| line.contains("text \"⌘1\""))
}

/// ⌘ held alone shows the sidebar's ⌘1–9 hints at once, kept through the
/// jumps they offer; another modifier with it, a key pressed meanwhile
/// (⌘C at the root, a command's own key such as ⌘E) or the palette open
/// hides them, until what's held changes again.
#[test]
fn jump_hints_follow_command_held_alone() {
    let mut w = mount();
    assert!(!hints(&mut w), "none before ⌘ goes down");
    hold(&mut w, true, false);
    assert!(hints(&mut w), "⌘ alone: at once");
    hold(&mut w, true, true);
    assert!(!hints(&mut w), "⌘⇧: not alone");
    hold(&mut w, true, false);
    assert!(hints(&mut w), "⇧ let go: ⌘ alone again");

    // ⌘3: a jump they offer, kept for the next.
    assert_eq!(w.press("cmd+3").as_deref(), Some("shell.session_3"));
    assert!(hints(&mut w), "kept through ⌘3");

    // ⌘C: no command's, heard at the shell's root.
    w.app.keys("cmd+c");
    w.app.frames(2);
    assert!(!hints(&mut w), "⌘C hides them");
    hold(&mut w, false, false);
    hold(&mut w, true, false);
    assert!(hints(&mut w), "⌘ down again: back");

    // ⌘E: a command's key (its menu item's, never an `on_key`'s).
    assert_eq!(w.press("cmd+e").as_deref(), Some("shell.toggle_files_tab"));
    assert!(!hints(&mut w), "a command's key hides them");
    hold(&mut w, false, false);
    assert!(!hints(&mut w), "⌘ let go: gone");

    // The palette open: none while it has the keys.
    w.app.host.set_state("shell_palette_open", Value::Bool(true)).expect("the palette's state");
    w.app.frames(5);
    hold(&mut w, true, false);
    assert!(!hints(&mut w), "none over the palette");
    hold(&mut w, false, false);
}

/// The command `id`'s shortcut as published now (the menu bar's).
fn published(w: &Window, id: &str) -> Option<String> {
    w.app.host.commands().get().iter().find(|c| c.id == id).and_then(|c| c.key.clone())
}

/// Settings → Shortcuts rebinds a command at once: the commands (the menu
/// bar's key equivalents) are published again with no reload. A key
/// another command holds is refused, saying whose it is; Reset puts it back.
#[test]
fn a_rebound_shortcut_takes_effect_at_once() {
    let mut w = mount();
    w.app.host.run_command("shell.settings");
    w.app.frames(20);
    w.app.click("#settings-nav >> role:tab \"Shortcuts\"");
    w.app.frames(30);
    assert_eq!(published(&w, "shell.toggle_terminal").as_deref(), Some("cmd+j"));

    let recorder = "#settings-page >> role:button ~\"Change Toggle terminal\"";
    w.app.click(recorder);
    w.app.frames(5);
    w.app.keys("ctrl+alt+t");
    w.app.frames(10);
    assert_eq!(published(&w, "shell.toggle_terminal").as_deref(), Some("ctrl+alt+t"), "rebound at once");
    assert!(w.app.texts().iter().any(|t| t.contains("Reset")), "a rebound row offers Reset");

    // ⌘B is Toggle sidebar's: recorded (the recorder takes every key while
    // it listens, a menu's included), refused, and said so; ⌘B runs nothing.
    w.app.click(recorder);
    w.app.frames(5);
    assert_eq!(w.press("cmd+b"), None, "⌘B goes to the recorder, not its command");
    assert_eq!(published(&w, "shell.toggle_terminal").as_deref(), Some("ctrl+alt+t"), "a taken key is refused");
    assert_eq!(published(&w, "shell.toggle_sidebar").as_deref(), Some("cmd+b"));
    let said = w.app.texts();
    assert!(
        said.iter().any(|t| t.contains("is already assigned to Toggle sidebar")),
        "the refusal names its owner: {said:?}"
    );
    // A ⌘ key no command has: recorded.
    w.app.click(recorder);
    w.app.frames(5);
    assert_eq!(w.press("cmd+shift+y"), None);
    assert_eq!(published(&w, "shell.toggle_terminal").as_deref(), Some("shift+cmd+y"), "a free ⌘ key records");

    // Reset: its default again.
    w.app.click("#settings-page >> role:button ~\"Reset Toggle terminal\"");
    w.app.frames(10);
    assert_eq!(published(&w, "shell.toggle_terminal").as_deref(), Some("cmd+j"));
    assert_eq!(w.app.error(), None);
}
