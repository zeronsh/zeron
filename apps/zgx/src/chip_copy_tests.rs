//! A sent message's mentions are chips (ds/markdown `mention_chip`): one
//! pill each, its room the span's own padding (zgx d6582b32), not thin
//! spaces in its text. So a selection across the message copies the names
//! as written, with no U+2009 around them. Mounted headless on the chat
//! fixture, whose `link_turn` sends "Look at [chat.rs](zeron-file:…) please,
//! as in [Long chat](zeron-chat:s1), and [the site](https://…)".

use zgx_script::{ScriptHost, testing::TestApp};

#[test]
fn copying_a_sent_message_with_chips_copies_the_names_alone() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    assert!(app.host.run_command("link_turn"));
    app.frames(30);
    let prompt = app.target("#lu1").expect("the prompt with mentions");
    assert!(app.ui.select_all_text(prompt), "its text selects");
    let copied = app.ui.selected_text();
    assert!(copied.contains("chat.rs") && copied.contains("Long chat"), "the names are copied: {copied:?}");
    assert!(!copied.contains('\u{2009}'), "no thin spaces: {copied:?}");
    assert!(copied.contains("Look at chat.rs please, as in Long chat,"), "the words read as written: {copied:?}");
}
