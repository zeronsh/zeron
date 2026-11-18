//! The model picker stops waiting on a device that doesn't answer: while
//! another device's catalog is looked at, its label is a breathing
//! skeleton (ds/skeleton, a `ui.loop`), and an offline device's look never
//! answers. The host settles it (src/chat.rs `settle_remote`); settled, the
//! skeleton goes and the window rests, no frames asked for. Mounted headless
//! on the chat fixture, the open session moved to the box.

use zgx_script::{ScriptHost, testing::TestApp};

fn on_the_box() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    assert!(app.host.run_command("session_on_box"));
    app.frames(30);
    app
}

#[test]
fn a_device_that_doesnt_answer_leaves_the_picker_at_rest_once_settled() {
    let mut app = on_the_box();
    assert!(app.host.run_command("catalog_box_looking"));
    app.frames(60);
    assert!(app.clock.wants_frame(), "looking: the picker's skeleton breathes");
    assert!(app.host.run_command("catalog_box_settled"));
    app.frames(60);
    assert_eq!(app.error(), None);
    assert!(!app.clock.wants_frame(), "settled: nothing breathes, no frames asked for");
    assert!(app.target("role:button \"Model\"").is_ok(), "the model chip still there");
}

/// New project's pages show breathing rows while a device lists a folder:
/// once it has, and the palette closed, nothing breathes on. (Each page left
/// its five breathing for good, so the window asked for every frame of the
/// display for as long as it was open: a project added through the palette
/// kept an idle window waking ~120 times a second.)
#[test]
fn new_projects_loading_rows_stop_breathing_once_loaded() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    // (No chat working: its ring would breathe throughout.)
    assert!(app.host.run_command("fixture.no_sessions"));
    app.frames(60);
    assert!(!app.clock.wants_frame(), "at rest before: no frames asked for");
    assert!(app.host.run_command("fixture.slow_folders"));
    assert!(app.host.run_command("shell.new_project"));
    app.frames(30);
    let option = |label: &str| format!("#palette-list >> role:option \"{label}\"");
    // This Mac's places, then Home's folders: each page listed slowly, its
    // loading rows breathing meanwhile.
    for (pick, then) in [("MacBook Pro", "Home"), ("Home", "code")] {
        app.click(&option(pick));
        app.frames(20);
        assert!(app.clock.wants_frame(), "loading {pick}: its rows breathe");
        let next = option(then);
        assert!(app.wait_until(|app| app.find(&next).is_some()), "{then} listed");
    }
    app.frames(60);
    assert_eq!(app.error(), None);
    for _ in 0..4 {
        app.key(zgx::input::Key::Escape);
        app.frames(30);
    }
    app.frames(60);
    assert!(!app.clock.wants_frame(), "loaded and closed: nothing breathes, no frames asked for");
}
