//! Prompts waiting for the engine must outlive navigation between chats.

use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Waker},
};
use zgx_script::testing::TestApp;

type Sending = Pin<Box<dyn Future<Output = Result<(), Outcome>>>>;

fn pending_chat() -> (TestApp, Rc<View>, Sending) {
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
    CHATS.with(|chats| {
        chats.made(
            "new-chat",
            json!({ "id": "new-chat", "config": { "harness": "codex" } }),
        );
        chats.made("other-chat", json!({ "id": "other-chat" }));
    });
    view.open_chat(Some("new-chat".into()));
    let sender = view.clone();
    let mut sending: Sending = Box::pin(async move {
        sender
            .send(Value::record([
                ("session", Value::from("new-chat")),
                ("message_id", Value::from("first-message")),
                ("text", Value::from("Find the startup bug")),
                ("fresh", Value::Bool(true)),
                ("harness", Value::from("codex")),
                (
                    "attachments",
                    Value::List(
                        vec![Value::record([
                            ("path", Value::from("/tmp/startup.txt")),
                            ("on_host", Value::Bool(true)),
                        ])]
                        .into(),
                    ),
                ),
            ]))
            .await
    });
    // No engine connection in this test: stop at the first asynchronous
    // handoff, with the real sender's prompt waiting to be persisted.
    assert!(
        sending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    view.heard_frame(json!({ "reset": [] }));
    (app, view, sending)
}

fn messages(view: &View) -> Json {
    view.messages
        .with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

#[test]
fn first_prompt_survives_switching_chats_before_the_engine_saves_it() {
    let (_app, view, _sending) = pending_chat();
    let original = messages(&view);
    assert_eq!(original[0]["id"], "first-message");

    view.open_chat(Some("other-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    assert_eq!(
        messages(&view),
        json!([]),
        "the prompt belongs only to its chat"
    );

    view.open_chat(Some("new-chat".into()));
    assert_eq!(
        messages(&view),
        original,
        "restore the prompt before the next transcript frame"
    );
    view.heard_frame(json!({ "reset": [] }));
    assert_eq!(
        messages(&view),
        original,
        "the first prompt and its attachments must survive navigation"
    );

    view.open_chat(None);
    view.open_chat(Some("new-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    assert_eq!(
        messages(&view),
        original,
        "visiting the new-chat page must also preserve the prompt"
    );

    let saved = json!({ "id": "first-message", "role": "user", "createdAt": now_seconds() * 1000,
        "parts": [{ "id": "text", "kind": "text", "text": "Find the startup bug" }] });
    view.heard_frame(json!({ "reset": [saved] }));
    assert_eq!(
        messages(&view).as_array().unwrap().len(),
        1,
        "the engine's copy replaces the local prompt"
    );
    assert!(!view.pending.borrow().contains_key("new-chat"));
    // Once acknowledged, a later empty transcript must not resurrect it.
    view.heard_frame(json!({ "reset": [] }));
    assert_eq!(messages(&view), json!([]));
}

#[test]
fn a_send_that_fails_while_away_keeps_its_prompt_and_retry() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    let run = json!({ "kind": "run", "messageId": "first-message", "request": { "prompt": "Find the startup bug" } });
    view.mark_failed("new-chat", "first-message", Some(run.clone()));
    assert_eq!(messages(&view), json!([]));

    view.open_chat(Some("new-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    assert_eq!(messages(&view)[0]["waiting"], "failed");
    assert_eq!(view.pending.borrow()["new-chat"][0].retry, Some(run));

    let mut retry = Box::pin(view.retry(Value::record([
        ("id", Value::from("new-chat")),
        ("message", Value::from("first-message")),
    ])));
    assert!(
        retry
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert_eq!(messages(&view)[0]["waiting"], "sending");
}

#[test]
fn a_saved_first_prompt_stays_visible_after_the_sending_indicator_expires() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    view.open_chat(Some("new-chat".into()));
    view.heard_frame(json!({ "reset": [{
        "id": "first-message", "role": "user",
        "createdAt": (now_seconds() - SEND_GRACE_SECONDS as i64 - 1) * 1000,
        "parts": [{ "id": "text", "kind": "text", "text": "Find the startup bug" }]
    }] }));
    assert_eq!(messages(&view)[0]["text"], "Find the startup bug");
    assert!(messages(&view)[0]["waiting"].is_null());
    assert!(!view.pending.borrow().contains_key("new-chat"));
}
