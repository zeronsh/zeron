//! Files carried over a chat drop anywhere on it, as Zeron's conversation
//! takes them (crates/ui shell/chat_dropzone.rs): the whole chat dims under
//! "Drop to attach" while files are over it, a drop on the transcript
//! attaches them to the composer, and a drag that isn't files shows
//! nothing. The page is the chat fixture (`ui/chat_fixture.luau`), mounted
//! headless; files from outside arrive as the platform delivers them
//! (`FileHover`, `FileDrop`, `FileHoverCancelled`), which the Lua test kit
//! can't send.

use std::path::PathBuf;

use zgx::input::{InputEvent, PointerButton};
use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The chat fixture on session `s6`, 960 by 700.
fn session() -> TestApp {
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s6"), "the fixture opens s6");
    app.frames(30);
    app
}

/// The middle of `target`'s box.
fn middle(app: &mut TestApp, target: &str) -> (f32, f32) {
    let r = app.bounds(target);
    (r.x + r.width / 2., r.y + r.height / 2.)
}

/// Over the transcript, well above the composer.
fn over_transcript(app: &mut TestApp) -> (f32, f32) {
    let r = app.bounds("#transcript");
    (r.x + r.width / 2., r.y + r.height * 0.3)
}

fn dimmed(app: &mut TestApp) -> bool {
    app.find("#chat-drop").is_some() && app.shown("#chat-drop")
}

fn has(app: &mut TestApp, target: &str) -> bool {
    app.find(target).is_some()
}

/// The draft, as the message box holds it.
fn draft(app: &mut TestApp) -> String {
    let tree = app.tree();
    tree.lines()
        .find_map(|l| l.split_once(r#"textbox "Message" =""#).map(|(_, rest)| rest.to_owned()))
        .and_then(|rest| rest.rsplit_once('"').map(|(text, _)| text.to_owned()))
        .unwrap_or_default()
}

#[test]
fn files_over_the_transcript_dim_the_chat_and_a_drop_attaches_them() {
    let mut app = session();
    assert!(!dimmed(&mut app), "nothing carried: no overlay");
    let (x, y) = over_transcript(&mut app);
    let path = PathBuf::from("/tmp/zeron-drop/notes.md");
    app.ui.dispatch(InputEvent::FileHover { x, y, path: path.clone() });
    app.frames(2);
    assert!(dimmed(&mut app), "files over the transcript dim the whole chat");
    assert!(has(&mut app, r#"#chat-drop >> ~"Drop to attach""#));
    app.ui.dispatch(InputEvent::FilesDrop { x, y, paths: vec![path].into() });
    app.frames(10);
    assert!(!dimmed(&mut app), "a drop takes the overlay away");
    assert!(
        has(&mut app, r#"#chat-composer >> ~"notes.md""#),
        "the file is a chip on the composer, as a drop on it makes"
    );
}

#[test]
fn files_leaving_the_window_take_the_overlay_with_them() {
    let mut app = session();
    let (x, y) = over_transcript(&mut app);
    app.ui.dispatch(InputEvent::FileHover { x, y, path: PathBuf::from("/tmp/zeron-drop/a.png") });
    app.frames(2);
    assert!(dimmed(&mut app));
    app.ui.dispatch(InputEvent::FileHoverCancelled);
    app.frames(2);
    assert!(!dimmed(&mut app), "a drag that leaves (or is cancelled) clears it");
    assert!(!has(&mut app, r#"#chat-composer >> ~"a.png""#), "nothing attached");
}

/// Pressed on `from`, carried past the drag threshold to `to` in steps,
/// and held there: what the chat shows mid-drag. Then released.
fn carry(app: &mut TestApp, from: (f32, f32), to: (f32, f32), during: impl FnOnce(&mut TestApp)) {
    let button = PointerButton::Primary;
    let (x, y) = from;
    app.ui.dispatch(InputEvent::PointerMove { x, y });
    app.ui.dispatch(InputEvent::PointerDown { x, y, button });
    app.ui.dispatch(InputEvent::PointerMove { x: x + 16., y: y + 8. });
    for step in 1..=8u16 {
        let part = f32::from(step) / 8.;
        app.ui.dispatch(InputEvent::PointerMove { x: x + (to.0 - x) * part, y: y + (to.1 - y) * part });
    }
    app.frames(2);
    during(app);
    let (x, y) = to;
    app.ui.dispatch(InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
    app.frames(10);
}

#[test]
fn the_files_panes_files_dropped_on_the_transcript_attach_as_chips() {
    let mut app = session();
    let from = middle(&mut app, "#fixture-drag-files");
    let to = over_transcript(&mut app);
    carry(&mut app, from, to, |app| {
        assert!(dimmed(app), "the files pane's files over the chat dim it too");
        assert!(has(app, r#"#chat-drop >> ~"Drop to attach""#), "the same words as for files from outside");
    });
    assert!(!dimmed(&mut app));
    assert!(has(&mut app, r#"#chat-composer >> ~"lib.rs""#), "the file is a chip on the composer");
    assert_eq!(draft(&mut app), "", "not a mention in the draft");
}

#[test]
fn a_drag_that_isnt_files_shows_nothing_and_drops_nothing() {
    let mut app = session();
    let from = middle(&mut app, "#fixture-drag-section");
    let to = over_transcript(&mut app);
    carry(&mut app, from, to, |app| {
        assert!(!dimmed(app), "a sidebar section carried over the chat isn't a drop for it");
    });
    assert!(!dimmed(&mut app));
    assert_eq!(draft(&mut app), "", "nothing added to the draft");
}


#[test]
fn files_dropped_on_a_new_chats_page_attach_to_its_composer() {
    let mut app = session();
    assert!(app.host.run_command("go_home"));
    app.frames(30);
    let r = app.bounds("#chat-home");
    let (x, y) = (r.x + r.width / 2., r.y + r.height * 0.15);
    let path = PathBuf::from("/tmp/zeron-drop/plan.pdf");
    app.ui.dispatch(InputEvent::FileHover { x, y, path: path.clone() });
    app.frames(2);
    assert!(dimmed(&mut app), "a new chat's page dims too");
    app.ui.dispatch(InputEvent::FilesDrop { x, y, paths: vec![path].into() });
    app.frames(10);
    assert!(!dimmed(&mut app));
    assert!(has(&mut app, r#"#chat-composer >> ~"plan.pdf""#));
}
