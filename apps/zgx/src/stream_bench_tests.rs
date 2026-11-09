//! The stream bench, headless: a short recording replayed through the chat
//! view's real path reaches the transcript. The page is the bench's own
//! (ui/stream_bench_fixture.luau, nothing stubbed), its chat state a view made
//! as `chat::install` makes a window's, `chat.open` the same action: the page
//! opens the chat, `subscribe` hands it to the bench, and each frame goes to
//! `heard_frame`.

use super::*;
use std::{collections::VecDeque, future::Future, pin::Pin, time::Duration};
use zgx_script::testing::TestApp;

type Queued = Rc<RefCell<VecDeque<Pin<Box<dyn Future<Output = ()>>>>>>;

const TEXT: &str = "## Streamed through the host\n\n\
    The reply arrives a few words at a time, each chunk an append the engine sends.\n\n\
    - first, the row\n- then its words\n- then its end\n\n\
    Every chunk is published as the draft, and the page shows it as it comes.\n";

/// The window's chat view, as `chat::install` makes it, its `chat.open` too.
fn window_view(host: &ScriptHost) -> Rc<View> {
    let view = Rc::new(View {
        session: host.shared("chat.session", Value::Nil),
        messages: host.shared("chat.messages", Value::List(Rc::from([]))),
        subagents: host.shared("chat.subagents", Value::List(Rc::from([]))),
        draft: host.shared("chat.draft", draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    CHATS.with(|chats| chats.views.borrow_mut().push(Rc::downgrade(&view)));
    let v = view.clone();
    host.action("chat.open", move |id| {
        let id = text(&id);
        v.open_chat((!id.is_empty()).then_some(id));
    });
    // (The composer's models: an engine's, none here.)
    host.action("chat.catalog", |_| {});
    host.action("chat.catalog_on", |_| {});
    view
}

fn draft(view: &View) -> Json {
    view.draft
        .with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

fn messages(view: &View) -> Json {
    view.messages
        .with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

#[test]
fn a_recorded_stream_reaches_the_transcript_through_the_host() {
    // The recording, written as the generator writes one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frames.jsonl");
    let frames = record::frames(TEXT, 24, 1_700_000_000_000);
    let lines: Vec<String> = frames.iter().map(|f| f.to_string()).collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    // (Played on the test's executor, its frames 60 ms apart.)
    let queued: Queued = Rc::default();
    let q = queued.clone();
    set(Bench::new(
        path,
        Duration::from_millis(60),
        Duration::from_millis(30),
        Rc::new(move |work| q.borrow_mut().push_back(work)),
    ));

    let made: Rc<RefCell<Option<Rc<View>>>> = Rc::default();
    let slot = made.clone();
    let mut app = TestApp::new(|runtime| {
        let host = ScriptHost::new(
            runtime,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/stream_bench_fixture.luau"),
        );
        *slot.borrow_mut() = Some(window_view(&host));
        host
    });
    let view = made.borrow().clone().unwrap();
    app.ui.scene.borrow_mut().resize(900., 700.);
    let pump = |app: &TestApp| {
        while let Some(work) = queued.borrow_mut().pop_front() {
            app.executor.borrow_mut().spawn(work);
        }
    };
    // The page opened the chat: the bench has it, and its reset shows.
    assert!(
        app.wait_until(|app| {
            pump(app);
            messages(&view).as_array().is_some_and(|m| !m.is_empty())
        }),
        "the page never opened the bench's chat: {:?}",
        view.open_id()
    );
    assert_eq!(view.open_id().as_deref(), Some(record::CHAT));
    assert_eq!(
        view.session.with_untracked(|s| s.field("state").map(text)),
        Some("ready".into())
    );

    // Mid-stream: the draft is the reply's words so far (its text block's),
    // and the transcript shows them.
    let mut seen = None;
    assert!(
        app.wait_until(|app| {
            pump(app);
            let d = draft(&view);
            let so_far = d["part_text"].as_str().unwrap_or_default();
            if d["id"] != record::REPLY || so_far.len() < 40 {
                return false;
            }
            seen = Some(d.clone());
            app.find("~\"Streamed through the host\"").is_some()
        }),
        "the draft never showed the reply mid-stream: {}\n{}",
        draft(&view),
        app.tree()
    );
    let seen = seen.unwrap();
    assert_eq!(seen["part"], record::PART);
    let so_far = seen["part_text"].as_str().unwrap();
    assert!(
        TEXT.starts_with(so_far) && so_far.len() < TEXT.len(),
        "a prefix of the reply: {so_far:?}"
    );
    assert_eq!(seen["text"], seen["part_text"]);

    // Its end: the row holds the whole reply, the draft lets it go.
    assert!(
        app.wait_until(|app| {
            pump(app);
            messages(&view)
                .as_array()
                .is_some_and(|m| m.len() == 2 && m[1]["streaming"] != true)
        }),
        "the reply never ended: {}",
        messages(&view)
    );
    assert_eq!(draft(&view)["text"], "");
    app.frames(5);
    assert!(
        app.find("~\"each chunk an append the engine sends\"")
            .is_some(),
        "{}",
        app.tree()
    );
    assert!(
        app.find("~\"the page shows it as it comes\"").is_some(),
        "{}",
        app.tree()
    );
    assert_eq!(app.error(), None);
}
