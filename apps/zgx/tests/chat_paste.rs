//! A paste into the composer, as Zeron's takes one (crates/ui composer.rs
//! `paste`): text goes in; a picture on the clipboard (a screenshot) becomes
//! an image the message carries, the host writing it out (src/pasted.rs,
//! stubbed by the chat fixture as `store.paste_image`). The page is the
//! chat fixture (`ui/chat_fixture.luau`), mounted headless; a paste arrives
//! as the platform hands it to the field (`Ui::paste_into`, with no text
//! for an image), which the Lua test kit can't send.

use std::path::PathBuf;

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The chat fixture on session `s2`, 960 by 700, the message box focused.
fn session() -> TestApp {
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"), "the fixture opens s2");
    app.frames(30);
    app.click(r#"role:textbox "Message""#);
    app.frames(5);
    app
}

/// A paste into the focused field: `text`, or none (a picture).
fn paste(app: &mut TestApp, text: Option<&str>) {
    let editor = app.ui.focused_editor().expect("the message box has the keyboard");
    app.ui.paste_into(&editor, text);
    app.frames(10);
}

fn calls(app: &TestApp) -> String {
    app.host
        .states()
        .into_iter()
        .find_map(|(key, value)| match value {
            Value::Str(s) if key == "calls" => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn a_pasted_picture_is_carried_as_an_image() {
    let mut app = session();
    paste(&mut app, None);
    assert!(calls(&app).contains("paste_image"), "the host was asked for the picture: {}", calls(&app));
    let tree = app.tree();
    let images = |tree: &str| tree.matches("image.png").count();
    assert!(images(&tree) >= 1, "the picture rides on the composer:\n{tree}");
    // A second is a second image, not the first again.
    let one = images(&tree);
    paste(&mut app, None);
    assert!(images(&app.tree()) > one, "a second picture, beside the first");
    assert_eq!(app.error(), None);
}

#[test]
fn pasted_text_goes_in_and_an_empty_clipboard_adds_nothing() {
    let mut app = session();
    paste(&mut app, Some("words"));
    assert!(app.tree().contains(r#"textbox "Message" ="words""#), "the words went in");
    assert!(!calls(&app).contains("paste_image"), "text never asks for a picture");
    assert!(app.host.run_command("clipboard_empty"));
    paste(&mut app, None);
    assert!(calls(&app).contains("paste_image"));
    assert!(!app.tree().contains("image.png"), "no picture, no chip");
    assert_eq!(app.error(), None);
}
