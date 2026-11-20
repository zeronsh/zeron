//! Focus after something closes: every popup, menu, palette, dialog and
//! drawer gives focus back to what had it when it opened (nothing, if
//! nothing had), and what gets it back draws no focus ring; the ring is for
//! the keyboard moving focus (Tab, the arrows), not for focus put back.
//! Mounted headless on the shell's fixture, each opened as a person opens
//! it, then closed with Escape.

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

/// How a node paints, if it paints a box.
fn paint(app: &TestApp, id: zgx::scene::NodeId) -> Option<zgx::scene::QuadStyle> {
    match app.ui.scene.borrow().kind(id) {
        NodeKind::Panel { quad, .. } | NodeKind::Quad(quad) => Some(quad.clone()),
        _ => None,
    }
}

/// Whether what has focus draws a ring round it now: it paints otherwise
/// than it does without focus (zgx's own ring is an accent border, the
/// design system's an outline). (Its focus put back after, as it was.)
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

/// A fresh window `setup` puts somewhere, then something opened there
/// (`open`, `shows` a target in it) and closed with Escape: focus is on what
/// had it as it opened (`want`: the opener a press focused, or what had
/// focus before, or nothing, `None`), no ring drawn. Faults in `faults`.
fn escape(
    what: &str,
    setup: impl FnOnce(&mut TestApp),
    open: impl FnOnce(&mut TestApp),
    shows: &str,
    want: Option<&str>,
    faults: &mut Vec<String>,
) {
    let mut app = mount();
    setup(&mut app);
    app.frames(20);
    open(&mut app);
    // (Some open a beat later: on the clock, or once what they list comes.)
    let target = shows.to_owned();
    app.wait_until(|app| app.find(&target).is_some());
    app.frames(30);
    if app.find(shows).is_none() {
        let tree = app.tree();
        let near: Vec<&str> = tree.lines().filter(|l| l.contains("Access") || l.contains(" tab ") || l.contains("#chat-") || l.contains("dialog")).collect();
        faults.push(format!("{what}: never opened ({shows})\n{}", near.join("\n")));
        return;
    }
    app.key(zgx::input::Key::Escape);
    app.frames(40);
    let after = focused(&mut app);
    let right = match (want, &after) {
        (None, None) => true,
        (Some(w), Some(a)) => a.contains(w) && !a.contains(" hidden "),
        _ => false,
    };
    if !right {
        faults.push(format!("{what}: closed, focus is {after:?}, wanted {want:?}"));
    } else if !after.as_deref().is_some_and(|a| a.contains(" textbox ")) && ring(&mut app) {
        // (A text field shows where typing goes, as a browser's does.)
        faults.push(format!("{what}: closed, a ring on {after:?}"));
    }
}

fn nothing(app: &mut TestApp) {
    app.ui.input.focus(&app.ui.scene, None);
}
fn composer(app: &mut TestApp) {
    app.click("#chat-composer >> role:textbox");
}
fn session(app: &mut TestApp) {
    keys_on_row(app, "#shell-sidebar >> role:button \"Fix the build\"");
}

/// A sidebar row with the keyboard: clicked (its chat opened, which gives
/// the composer the keys: c865bf3be, ui/tests/composer_focus), then the
/// keyboard put back on the row, as Tab or a second click there would, for
/// what is pressed on the row itself (Shift+F10's menu, ⌘K and back).
pub(crate) fn keys_on_row(app: &mut TestApp, row: &str) {
    app.click(row);
    app.frames(10);
    let target = app.target(row).unwrap_or_else(|e| panic!("{row}: {e:?}"));
    assert!(app.ui.input.focus(&app.ui.scene, Some(target)), "{row} takes the keyboard");
    app.frames(2);
}
fn palette(app: &mut TestApp) {
    assert!(app.host.run_command("shell.palette"));
}
fn click(target: &'static str) -> impl FnOnce(&mut TestApp) {
    move |app: &mut TestApp| app.click(target)
}

/// Today two of zgx's own rules break this (zgx 06debe9, input.rs): a
/// scope closing with nothing focused before falls back to the first
/// focusable (`pop_focus_scope`'s `next_focus`), and any plain key (Escape
/// too) makes what gets focus back draw its ring (`pointer_modality`).
/// zgx took the change (f3b6b53e, 3c47d763; Zeron's pin since 944fa878d): this checks it.
#[test]
fn escape_gives_focus_back_without_a_ring() {
    let mut f = Vec::new();
    escape("⌘K, nothing focused", nothing, palette, "#palette-search", None, &mut f);
    escape("⌘K from the composer", composer, palette, "#palette-search", Some("textbox \"Message\""), &mut f);
    escape("⌘K from a session row", session, palette, "#palette-search", Some("button \"Fix the build\""), &mut f);
    let options = "role:button \"Sidebar view options\"";
    escape("the sidebar's ⋯", nothing, click(options), "role:checkbox \"By project\"", Some("\"Sidebar view options\""), &mut f);
    let row = Some("button \"Fix the build\"");
    escape("the session menu", session, |app| app.keys("shift+F10"), "role:menuitem \"Rename\"", row, &mut f);
    escape(
        "rename",
        session,
        |app| {
            app.keys("shift+F10");
            app.wait_until(|app| app.find("role:menuitem \"Rename\"").is_some());
            app.frames(20);
            if app.find("role:menuitem \"Rename\"").is_some() {
                app.click("role:menuitem \"Rename\"");
            }
        },
        // (Renamed in its row, where the list shows it: the row's own field.)
        "role:textbox \"Session name\"",
        row,
        &mut f,
    );
    // The composer's pickers, each clicked open: a press records its button
    // as what had focus (zgx 3c47d763), so Escape gives it back, ringless.
    for (what, button, item, opener) in [
        ("the model picker", "Model", "role:option", true),
        ("the access picker", "Access", "role:listbox \"Access\"", true),
        ("Work in", "Branch", "\"Work in\"", true),
    ] {
        let target: &'static str = Box::leak(format!("role:button \"{button}\"").into_boxed_str());
        let want: &'static str = Box::leak(format!("button \"{button}\"").into_boxed_str());
        escape(what, nothing, click(target), item, opener.then_some(want), &mut f);
    }
    // (A panel tab's menu isn't here: headless, neither a secondary click nor
    // Shift+F10 on a tab opens it.)
    assert!(f.is_empty(), "{}", f.join("\n"));
}
