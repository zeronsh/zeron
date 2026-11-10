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
        words: [app.host.shared("test.words.1", ""), app.host.shared("test.words.2", "")],
        writing: RefCell::default(),
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

fn held_send(id: &str, failure: Option<&str>) -> Json {
    json!({
        "id": format!("send-{id}"),
        "chatId": "other-chat",
        "messageId": id,
        "prompt": "look at this\n\nAttached images (local files \u{2014} open them to view):\n- pending://u1/a.png",
        "createdAt": 1_000,
        "failure": failure,
    })
}

#[test]
fn a_send_in_the_engines_outbox_shows_even_after_a_restart() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    view.heard_frame(json!({ "reset": [] }));

    // Nothing here sent it: this window only hears of it from the engine.
    view.follow_held(&[held_send("m1", None)]);
    let shown = messages(&view);
    assert_eq!(shown[0]["id"], "m1");
    assert_eq!(shown[0]["text"], "look at this");
    assert_eq!(shown[0]["waiting"], "sending");

    // Hearing the same again adds nothing.
    view.follow_held(&[held_send("m1", None)]);
    assert_eq!(messages(&view).as_array().unwrap().len(), 1);
}

#[test]
fn a_failed_send_in_the_outbox_reads_failed_and_comes_back() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    view.follow_held(&[held_send("m1", Some("the file is too large"))]);
    assert_eq!(messages(&view)[0]["waiting"], "failed");
    {
        let pending = view.pending.borrow();
        let prompt = &pending["other-chat"][0];
        assert!(prompt.held, "the engine retries it, not a command from here");
        assert!(prompt.retry.is_none());
    }
    view.follow_held(&[held_send("m1", None)]);
    assert_eq!(messages(&view)[0]["waiting"], "sending");
}

#[test]
fn a_send_leaving_the_outbox_waits_for_the_transcript() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    view.follow_held(&[held_send("m1", None)]);
    view.follow_held(&[]);
    assert_eq!(messages(&view)[0]["waiting"], "sending", "it has only just gone out");
    assert!(!view.pending.borrow()["other-chat"][0].held);
    let saved = json!({ "id": "m1", "role": "user", "createdAt": now_seconds() * 1000,
        "parts": [{ "id": "text", "kind": "text", "text": "look at this" }] });
    view.heard_frame(json!({ "reset": [saved] }));
    assert!(!view.pending.borrow().contains_key("other-chat"));
}

#[test]
fn a_row_leaving_the_outbox_is_the_queues_to_show() {
    let (_app, view, _sending) = pending_chat();
    view.open_chat(Some("other-chat".into()));
    view.heard_frame(json!({ "reset": [] }));
    let mut row = held_send("r1", None);
    row["queueRow"] = json!(true);
    view.follow_held(&[row]);
    assert_eq!(messages(&view)[0]["id"], "r1");
    view.follow_held(&[]);
    assert_eq!(messages(&view), json!([]));
}

#[test]
fn only_a_cloud_chat_on_an_engine_that_knows_the_store_goes_through_it() {
    let knows = json!(["message-queue-v1", "attachment-store-v1"]);
    let older = json!(["message-queue-v1"]);
    assert!(store_takes_files(Some("cloud-machine"), &knows));
    assert!(!store_takes_files(Some("cloud-machine"), &older));
    assert!(!store_takes_files(Some("cloud-machine"), &Json::Null));
    assert!(!store_takes_files(Some("desktop-at-home"), &knows));
    assert!(!store_takes_files(None, &knows));
}

#[test]
fn a_stored_file_is_named_the_way_the_engine_resolves_it() {
    let (reference, transfer) = stored_file("u1", "/Users/me/shots/a b.png");
    assert_eq!(reference, "pending://u1/a b.png");
    assert_eq!(transfer, json!({ "uploadId": "u1", "fileName": "a b.png" }));
    let prompt = with_attachments("look", std::slice::from_ref(&reference), 1);
    assert_eq!(split_attachments(&prompt), ("look".to_owned(), vec![reference]));
}

/// A reply's text part being written reaches its words state a chunk at a
/// time, its math read into code; a new part writes the other state, the
/// last part's left as it was; the draft names the part and the state, and
/// stays as it is while words come.
#[test]
fn a_text_part_being_written_grows_its_words_state_alone() {
    let app = TestApp::new(|runtime| {
        ScriptHost::new(
            runtime,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/activity_fixture.luau"),
        )
    });
    let view = Rc::new(View {
        session: app.host.shared("w.session", Value::Nil),
        messages: app.host.shared("w.messages", Value::Nil),
        draft: app.host.shared("w.draft", Value::Nil),
        words: [app.host.shared("w.words.1", ""), app.host.shared("w.words.2", "")],
        writing: RefCell::default(),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        subagents: app.host.shared("w.subagents", Value::Nil),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    let entry = |parts: Json| {
        Entry::new(json!({ "id": "r", "role": "assistant", "status": "streaming", "parts": parts }))
    };
    let set = |parts: Json| {
        *view.open.borrow_mut() = Some(Open { id: "c".into(), entries: vec![entry(parts)], ..Open::default() });
    };
    let words = |slot: usize| view.words[slot].with_untracked(|v| v.to_string());
    let text = "Say $x^2$ here, and $$a$$ then more words `code $y$` and $z";
    let draft_writes = Rc::new(Cell::new(0));
    let counting = draft_writes.clone();
    let draft = view.draft.signal();
    let _watch = app.ui.runtime.effect(move || {
        draft.with(|_| ());
        counting.set(counting.get() + 1);
    });
    for (writes, end) in (1..=text.len()).step_by(3).chain([text.len()]).enumerate() {
        set(json!([{ "id": "t1", "kind": "text", "text": &text[..end] }]));
        view.publish_draft(writes > 0);
        assert_eq!(words(0), crate::math_source::prepare(&text[..end]), "at {end}");
    }
    let d = view.draft.with_untracked(Clone::clone);
    assert_eq!(d.field("part").map(|v| v.to_string()).as_deref(), Some("t1"));
    assert_eq!(d.field("words").and_then(Value::as_f64), Some(1.));
    assert!(d.field("part_text").is_none() && d.field("text").is_none(), "no words in the draft");
    app.frames(1);
    assert!(draft_writes.get() <= 3, "the draft woke readers {} times", draft_writes.get());
    // A new part: the other state, the first left as it was.
    let first = words(0);
    set(json!([
        { "id": "t1", "kind": "text", "text": text },
        { "id": "tool", "kind": "tool" },
        { "id": "t2", "kind": "text", "text": "And on." }
    ]));
    view.publish_draft(false);
    assert_eq!(words(1), "And on.");
    assert_eq!(words(0), first);
    let d = view.draft.with_untracked(Clone::clone);
    assert_eq!(d.field("words").and_then(Value::as_f64), Some(2.));
}
