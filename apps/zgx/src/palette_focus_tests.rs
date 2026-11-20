//! Focus after the command palette (⌘K) closes, every way it opens and
//! closes: back on what had it as it opened (the composer, a session's row,
//! the sidebar's Search when a press opened it), or nothing; never another
//! control, and no ring after Escape. Mounted headless on the shell's fixture.

use zgx::scene::NodeKind;
use zgx_script::{ScriptHost, testing::TestApp};

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1240., 820.);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    app
}

/// What has focus, as the tree shows it (its line), if anything.
fn focused(app: &mut TestApp) -> Option<String> {
    app.ui.input.focused()?;
    app.tree().lines().find(|l| l.split(' ').any(|w| w == "focused")).map(|l| l.trim().to_owned())
}

fn paint(app: &TestApp, id: zgx::scene::NodeId) -> Option<zgx::scene::QuadStyle> {
    match app.ui.scene.borrow().kind(id) {
        NodeKind::Panel { quad, .. } | NodeKind::Quad(quad) => Some(quad.clone()),
        _ => None,
    }
}

/// Whether what has focus draws a ring now (it paints otherwise than it
/// does unfocused); focus is put back as it was.
fn ring(app: &mut TestApp) -> bool {
    let Some(id) = app.ui.input.focused() else {
        return false;
    };
    app.frames(2);
    let focused = paint(app, id);
    app.ui.input.focus(&app.ui.scene, None);
    app.frames(2);
    let plain = paint(app, id);
    app.ui.input.focus(&app.ui.scene, Some(id));
    app.frames(2);
    focused != plain
}

const FIELD: &str = "#palette-search";

/// Whether the palette shows (its field, not hidden: a closed one may stay
/// mounted as it leaves).
fn is_open(app: &mut TestApp) -> bool {
    app.find(FIELD).is_some() && app.tree().lines().any(|l| l.contains("#palette-search") && !l.contains(" hidden"))
}

/// Where focus starts.
#[derive(Clone, Copy, Debug)]
enum From {
    Nothing,
    Composer,
    Row,
}
fn start(app: &mut TestApp, from: From) {
    match from {
        From::Nothing => {
            app.ui.input.focus(&app.ui.scene, None);
        }
        From::Composer => app.click("#chat-composer >> role:textbox"),
        From::Row => crate::focus_tests::keys_on_row(app, "#shell-sidebar >> role:button \"Fix the build\""),
    }
    app.frames(20);
}

/// How it opens.
#[derive(Clone, Copy, Debug)]
enum Open {
    /// ⌘K: the menu bar's item runs the shell's command.
    Command,
    /// The sidebar's Search, pressed.
    Search,
}
fn open(app: &mut TestApp, how: Open) {
    match how {
        Open::Command => assert!(app.host.run_command("shell.palette")),
        Open::Search => app.click("#shell-sidebar >> role:button \"Search\""),
    }
    app.wait_until(|app| app.find(FIELD).is_some());
    app.frames(30);
    assert!(app.find(FIELD).is_some(), "the palette never opened ({how:?})");
}

/// How it closes.
#[derive(Clone, Copy, Debug)]
enum Close {
    /// Escape from its empty field.
    Escape,
    /// Something typed (with the mouse: a click into the field, then the
    /// text), Escape (clears it), Escape (closes).
    Typed,
    /// Keys typed into the field as it opened (no click), Escape (clears
    /// it), Escape (closes): the keyboard in use since.
    KeysTyped,
    /// A page of it opened ("New chat in…"), Escape (back), Escape.
    Page,
    /// ⌘K again: the shell's command shuts it.
    Toggle,
    /// Escape, the scripts having reloaded meanwhile (a save while it's
    /// open, as other chats save Zeron's files).
    Reloaded,
    /// Escape, the sessions having changed meanwhile (one starts working,
    /// one finishes, one renamed elsewhere), as the engine's do.
    Changed,
}
fn close(app: &mut TestApp, how: Close) {
    match how {
        Close::Escape => {}
        Close::Typed => {
            app.type_text(FIELD, "zz");
            app.frames(10);
            app.key(zgx::input::Key::Escape);
            app.frames(10);
            assert!(is_open(app), "the first Escape clears, not closes");
        }
        Close::Page => {
            app.click("role:option ~\"New chat in\"");
            app.frames(30);
            app.key(zgx::input::Key::Escape);
            app.frames(20);
            assert!(is_open(app), "Escape on a page goes back, not out");
        }
        Close::KeysTyped => {
            // (A key goes down, then its text, as a window has them.)
            for _ in 0..2 {
                app.keys("z");
                app.ui.dispatch(zgx::input::InputEvent::Text("z".into()));
            }
            app.frames(10);
            app.key(zgx::input::Key::Escape);
            app.frames(10);
            assert!(is_open(app), "the first Escape clears, not closes");
        }
        Close::Reloaded => {
            app.host.reload().expect("the reload");
            app.frames(40);
            assert!(is_open(app), "a reload keeps it open");
        }
        Close::Changed => {
            for change in ["work", "finish", "rename_elsewhere"] {
                assert!(app.host.run_command(change), "{change}");
                app.frames(10);
            }
        }
        Close::Toggle => {
            assert!(app.host.run_command("shell.palette"));
            app.frames(40);
            return;
        }
    }
    app.key(zgx::input::Key::Escape);
    app.frames(40);
}

/// What should have focus once it's closed: what had it as it opened (a
/// press on Search gives it focus first), else nothing.
fn want(from: From, how: Open) -> Option<&'static str> {
    match (how, from) {
        (Open::Search, _) => Some("button \"Search\""),
        (_, From::Nothing) => None,
        (_, From::Composer) => Some("textbox \"Message\""),
        (_, From::Row) => Some("button \"Fix the build\""),
    }
}

fn run(from: From, how: Open, shut: Close, faults: &mut Vec<String>) {
    let mut app = mount();
    start(&mut app, from);
    open(&mut app, how);
    close(&mut app, shut);
    let what = format!("{from:?}, opened by {how:?}, closed by {shut:?}");
    if is_open(&mut app) {
        faults.push(format!("{what}: still open"));
        return;
    }
    let after = focused(&mut app);
    let right = match (want(from, how), &after) {
        (None, None) => true,
        (Some(w), Some(a)) => a.contains(w) && !a.contains(" hidden "),
        _ => false,
    };
    eprintln!("{what}: focus {after:?}");
    if !right {
        faults.push(format!("{what}: focus is {after:?}, wanted {:?}", want(from, how)));
    } else if !after.as_deref().is_some_and(|a| a.contains(" textbox ")) && ring(&mut app) {
        faults.push(format!("{what}: a ring on {after:?}"));
    }
}

#[test]
fn every_way_the_palette_closes_gives_focus_back() {
    let mut faults = Vec::new();
    for from in [From::Nothing, From::Composer, From::Row] {
        for how in [Open::Command, Open::Search] {
            for shut in [Close::Escape, Close::Typed, Close::KeysTyped, Close::Page, Close::Toggle, Close::Changed] {
                run(from, how, shut, &mut faults);
            }
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}


/// A save (another chat's, or yours) reloads the window: focus stays on the
/// composer or the row that had it, so ⌘K's Escape gives it back there.
/// Today zgx drops focus on a reload to the window's first Tab stop (the
/// title bar's Toggle sidebar), silently, and Escape out of ⌘K then gives
/// focus back to that: the "random control". Fixed by zgx's #1389
/// (798aac78, "A reload keeps focus where it was").
#[test]
fn a_save_before_the_palette_keeps_focus_to_give_back() {
    let mut faults = Vec::new();
    for from in [From::Composer, From::Row] {
        let mut app = mount();
        start(&mut app, from);
        app.host.reload().expect("the reload");
        app.frames(40);
        let after_reload = focused(&mut app);
        if !after_reload.as_deref().is_some_and(|a| a.contains(want(from, Open::Command).unwrap())) {
            faults.push(format!("{from:?}: after the reload, focus is {after_reload:?}"));
        }
        open(&mut app, Open::Command);
        close(&mut app, Close::Escape);
        let after = focused(&mut app);
        if !after.as_deref().is_some_and(|a| a.contains(want(from, Open::Command).unwrap())) {
            faults.push(format!("{from:?}: after ⌘K and Escape, focus is {after:?}"));
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// A save while ⌘K is open: Escape still gives focus back to what had it as
/// it opened. (Once to nothing: the palette remembered the node the reload
/// replaced. Fixed by zgx's #1408, 7e8de187.)
#[test]
fn a_save_while_the_palette_is_open_keeps_focus_to_give_back() {
    let mut faults = Vec::new();
    for from in [From::Composer, From::Row] {
        for how in [Open::Command, Open::Search] {
            run(from, how, Close::Reloaded, &mut faults);
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// The smallest form, for zgx (sent to its coordinator): a field focused, a
/// reload, then a modal opened and closed with Escape. Focus should stay on
/// the field throughout; today the reload moves it to the first button.
const BARE: &str = r#"
local open = ui.state("open", false)
ui.command { id = "open", title = "Open", run = function() open:set(true) end }
return ui.column {
    w = 400, h = 300,
    ui.button { id = "first", "First" },
    ui.button { id = "second", "Second" },
    ui.input { id = "field", value = "" },
    ui.modal {
        open = open,
        build = function()
            return ui.input { id = "search", value = "" }
        end,
    },
}
"#;

#[test]
fn zgx_repro_a_reload_keeps_focus_and_a_modal_gives_it_back() {
    let mut app = TestApp::inline(BARE);
    app.frames(5);
    app.click("field");
    app.frames(5);
    assert!(focused(&mut app).is_some_and(|f| f.contains("#field")), "the field has focus");
    app.host.reload().expect("the reload");
    app.frames(10);
    let after_reload = focused(&mut app);
    assert!(app.host.run_command("open"));
    app.frames(10);
    app.key(zgx::input::Key::Escape);
    app.frames(10);
    let after_escape = focused(&mut app);
    assert!(
        after_reload.as_deref().is_some_and(|f| f.contains("#field")) && after_escape.as_deref().is_some_and(|f| f.contains("#field")),
        "after the reload: {after_reload:?}; after the modal's Escape: {after_escape:?}"
    );
}

/// A save reloads the window while a row of the files panel's tree has the
/// keyboard: that row has it again (zgx's #1389 gives a list's row focus
/// back by its place; the tree no longer keeps the row itself).
#[test]
fn a_save_keeps_the_file_trees_row_focused() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    // (The panel's files, as ⌘E shows them.)
    if app.find("#shell-files >> role:treeitem \"README.md\"").is_none() {
        assert!(app.host.run_command("shell.toggle_files_tab"));
        app.frames(30);
    }
    let readme = "#shell-files >> role:treeitem \"README.md\"";
    assert!(app.wait_until(|app| app.find(readme).is_some()), "README.md never showed in the panel");
    app.frames(20);
    app.click(readme);
    app.frames(10);
    // (To the next row with the keyboard, as a person goes down the tree.)
    app.key(zgx::input::Key::ArrowDown);
    app.frames(10);
    let before = focused(&mut app).expect("a row has the keyboard");
    assert!(before.contains("treeitem"), "a tree row has focus: {before}");
    let name = before.split('"').nth(1).unwrap_or_default().to_owned();
    app.host.reload().expect("the reload");
    app.frames(60);
    let after = focused(&mut app);
    assert!(
        after.as_deref().is_some_and(|a| a.contains("treeitem") && a.contains(&format!("\"{name}\""))),
        "after the reload, focus is {after:?}, was {before}"
    );
}
