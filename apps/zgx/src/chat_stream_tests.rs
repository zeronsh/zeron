//! A streaming reply's frames, fed through the view as the engine sends them.

use super::*;
use zgx_script::testing::TestApp;

fn open_chat() -> (TestApp, Rc<View>) {
    let app = TestApp::new(|runtime| {
        ScriptHost::new(
            runtime,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/activity_fixture.luau"),
        )
    });
    let view = Rc::new(View {
        session: app.host.shared("test.session", Value::Nil),
        messages: app.host.shared("test.messages", Value::Nil),
        draft: app.host.shared("test.draft", Value::Nil),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        subagents: app.host.shared("test.subagents", Value::Nil),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    CHATS.with(|chats| chats.made("stream-chat", json!({ "id": "stream-chat" })));
    view.open_chat(Some("stream-chat".into()));
    (app, view)
}

fn json_of(state: &Shared) -> Json {
    state.with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

/// The reply row's text blocks: (id, its words).
fn text_blocks(view: &View) -> Vec<(String, String)> {
    let messages = json_of(&view.messages);
    let reply = messages.as_array().unwrap().iter().find(|m| m["id"] == "a").expect("the reply has a row");
    reply["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "text")
        .map(|b| (b["id"].as_str().unwrap().to_owned(), b["text"].as_str().unwrap().to_owned()))
        .collect()
}

/// A text part the engine wrote empty (an empty text delta folded after a
/// thought, say) whose words then come as appends: its block shows them as
/// they come, not only once the reply settles.
#[test]
fn a_text_part_that_begins_empty_shows_its_words_while_they_stream() {
    let (_app, view) = open_chat();
    let user = json!({ "id": "u", "role": "user", "createdAt": 1000,
        "parts": [{ "id": "p", "kind": "text", "text": "hi" }] });
    let reply = json!({ "id": "a", "role": "assistant", "status": "streaming", "createdAt": 2000, "parts": [
        { "kind": "reasoning", "id": "r0", "text": "Thinking it over." },
        { "kind": "text", "id": "t1", "text": "" },
    ] });
    view.heard_frame(json!({ "reset": [user, reply] }));
    assert!(text_blocks(&view).is_empty(), "an empty text part has no block yet");

    let append = |text: &str, len: usize| {
        json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t1", "text": text, "len": len }], "count": 2 })
    };
    view.heard_frame(append("Hello", 5));
    assert_eq!(text_blocks(&view), vec![("t1".to_owned(), "Hello".to_owned())], "its first words make its block");
    let draft = json_of(&view.draft);
    assert_eq!((draft["part"].as_str(), draft["part_text"].as_str()), (Some("t1"), Some("Hello")));

    // More words are the draft's alone (the row isn't rebuilt per word).
    let before = json_of(&view.messages);
    view.heard_frame(append(" there", 11));
    assert_eq!(json_of(&view.messages), before);
    assert_eq!(json_of(&view.draft)["part_text"], "Hello there");
}
