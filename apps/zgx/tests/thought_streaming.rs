//! A thought being written streams into its row, its new words fading in as a
//! reply's do (ds/markdown's veil), colour only. Read off the scene, where zgx
//! paints it (the Lua test kit can't see colours). The page is the chat fixture
//! (`ui/chat_fixture.luau`), mounted headless, its thought's words arriving in
//! the draft as the host sends them (src/chat.rs `with_thought`).

use std::path::PathBuf;

use zgx::scene::{Color, NodeKind};
use zgx::theme::tok;
use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The pieces of the text holding `words`, each with the colour it paints.
fn pieces(app: &mut TestApp, words: &str) -> Vec<(String, Color)> {
    let node = app
        .find(&format!("~\"{words}\""))
        .unwrap_or_else(|| panic!("no {words}:\n{}", app.tree()));
    let scene = app.ui.scene.borrow();
    let mut stack = vec![node];
    while let Some(id) = stack.pop() {
        match scene.kind(id) {
            NodeKind::RichText { text } => {
                return text
                    .runs()
                    .iter()
                    .map(|run| (text.text()[run.range.clone()].to_owned(), run.color))
                    .collect();
            }
            NodeKind::Text { text, color, .. } => return vec![(text.to_string(), *color)],
            _ => stack.extend(scene.children(id).iter().rev().copied()),
        }
    }
    panic!("no text under {words}");
}

/// Whether every piece of the text holding `words` comes to paint `ink`, frames
/// paced as a window's are. (The veil times its fade by `ui.now()`, the wall
/// clock, and repaints by an animation on the frames' clock: frames run flat
/// out would end that animation with the fade barely begun.)
fn settles(app: &mut TestApp, words: &str, ink: Color) -> bool {
    for _ in 0..60 {
        app.frames(1);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    pieces(app, words).iter().all(|(_, color)| *color == ink)
}

#[test]
fn a_thought_being_written_fades_its_words_in() {
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"), "the fixture opens s2");
    app.frames(30);
    // (Past the moment a chat opens, when what's there stays as it is.)
    std::thread::sleep(std::time::Duration::from_millis(320));
    assert!(app.host.run_command("thinking_turn"));
    app.frames(2);
    assert_eq!(app.error(), None, "the thought failed to show");
    let ink = app.ui.theme().root().color_untracked(tok::TEXT_SECONDARY);

    // Its first words start faded, and settle in the thought's ink.
    let first = pieces(&mut app, "Weighing the parser first.");
    assert!(first.iter().any(|(_, color)| color.3 < ink.3), "the first words start faded: {first:?} against {ink:?}");
    assert!(settles(&mut app, "Weighing the parser first.", ink), "the first words never settled: {:?}", pieces(&mut app, "Weighing the parser first."));

    // More arrives, in the draft alone: it shows at once, on its way in, and
    // what was there stays as it was.
    assert!(app.host.run_command("thought_streams"));
    app.frames(2);
    let now = pieces(&mut app, "Then the host.");
    let (new, new_color) = now.last().expect("the words have pieces");
    assert!(new.contains("Then the host."), "the new words are their own piece: {now:?}");
    assert!(new_color.3 < ink.3, "the new words start faded: {now:?} against {ink:?}");
    let (old, old_color) = now.first().expect("the words have pieces");
    assert!(old.starts_with("Weighing the parser first."), "{now:?}");
    assert_eq!(*old_color, ink, "what had already shown doesn't fade again");

    // And settles in the thought's own ink.
    assert!(settles(&mut app, "Then the host.", ink), "the new words never settled: {:?}", pieces(&mut app, "Then the host."));

    // Written: its group folds shut, and opened, its row holds the words the
    // draft has let go.
    assert!(app.host.run_command("thought_written"));
    app.frames(30);
    assert_eq!(app.error(), None);
    app.click("role:button ~\"Thought process\"");
    app.frames(30);
    assert!(app.find("~\"Then the host.\"").is_some(), "the row keeps the words:\n{}", app.tree());
}
