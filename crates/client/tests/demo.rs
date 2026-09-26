//! Demo mode end to end: the offline dataset through the real registry and
//! session pipeline (view derivation, ledger commands, echo adoption,
//! streaming, the shared queue, questions, interrupts).

use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_client::events::NullListener;
use zeron_client::{
    ChatIndicator, Client, ClientConfig, Credentials, DemoFixture, DemoOptions, MessageRole,
    MessageStatus, SendOutcome, SendRequest, StreamSpeed, TranscriptScale,
};

fn demo(options: DemoOptions) -> (Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "ios-test".into();
    config.device_name = "Test iPhone".into();
    let client = Client::new(config, Credentials::Demo(options), Arc::new(NullListener)).unwrap();
    (client, dir)
}

fn fast() -> DemoOptions {
    DemoOptions {
        stream_speed: StreamSpeed::Fast,
        ..Default::default()
    }
}

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn ids(rows: &[Arc<zeron_client::SessionRow>]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

#[test]
fn front_page_mirrors_the_desktop_sidebar() {
    let (client, _dir) = demo(fast());
    let ws = client.workspace();
    assert!(ws.synced && ws.pins_ready);
    assert_eq!(ids(&ws.front.pinned), ["chat-veil", "chat-picker"]);
    let sections: Vec<(&str, Vec<&str>)> = ws
        .front
        .sections
        .iter()
        .map(|s| (s.name.as_str(), ids(&s.sessions)))
        .collect();
    assert_eq!(
        sections,
        [
            ("P0", vec!["chat-tabs", "chat-errored"]),
            ("Mobile", vec!["chat-ios-scroll", "chat-ios-keyboard"]),
        ]
    );
    // Recency order; archived + child chats never appear.
    assert_eq!(
        ids(&ws.front.recent),
        ["chat-cjk", "chat-home", "chat-deploy", "chat-blog"]
    );
    assert_eq!(ids(&ws.archived), ["chat-oklch", "chat-presence"]);
    assert_eq!(ids(ws.children("chat-veil")), ["chat-side"]);

    let veil = ws.session("chat-veil").unwrap();
    assert_eq!(veil.indicator, ChatIndicator::Working);
    assert!(veil.working_since_ms.is_some());
    assert!(veil.pinned);
    assert_eq!(veil.project.as_ref().unwrap().name, "zeron");
    assert_eq!(veil.device_name.as_deref(), Some("MacBook Pro"));
    assert!(veil.device_online);
    assert_eq!(veil.model_label.as_deref(), Some("Fable 5"));
    assert_eq!(veil.branch.as_deref(), Some("veil-fade"));
    assert_eq!(veil.time_label, "now");
    assert_eq!(
        ws.session("chat-picker").unwrap().indicator,
        ChatIndicator::AwaitingInput
    );
    assert_eq!(
        ws.session("chat-errored").unwrap().indicator,
        ChatIndicator::Errored
    );
    assert_eq!(
        ws.session("chat-tabs").unwrap().section_id.as_deref(),
        Some("section-p0")
    );

    let prs = &ws.pull_requests;
    assert_eq!(ids(&prs.open), ["chat-veil", "chat-ios-scroll"]);
    assert_eq!(ids(&prs.merged), ["chat-picker"]);
    assert_eq!(ids(&prs.closed), ["chat-tabs"]);

    let zeron = ws.project("space-zeron").unwrap();
    assert_eq!(zeron.indicator, ChatIndicator::AwaitingInput);
    assert!(zeron.unseen_count >= 2);
    assert_eq!(ids(&ws.projectless), ["chat-home"]);
    assert!(ws.devices.iter().any(|d| d.is_self && d.id == "ios-test"));
    assert!(!ws.device("dev-studio").unwrap().online);

    let hits = client.search("scroll", 5);
    assert_eq!(hits[0].session.id, "chat-ios-scroll");
}

#[test]
fn sidebar_writes_reorder_the_front_page() {
    let (client, _dir) = demo(fast());
    client.pin_session("chat-home").unwrap();
    assert_eq!(
        ids(&client.workspace().front.pinned),
        ["chat-veil", "chat-picker", "chat-home"]
    );
    client
        .move_pin("chat-home", None, Some("chat-veil".into()))
        .unwrap();
    assert_eq!(
        ids(&client.workspace().front.pinned),
        ["chat-home", "chat-veil", "chat-picker"]
    );
    client.unpin_session("chat-veil").unwrap();
    let section = client.create_section("Later").unwrap();
    client
        .assign_section("chat-deploy", Some(section.clone()))
        .unwrap();
    let ws = client.workspace();
    let later = ws.front.sections.iter().find(|s| s.id == section).unwrap();
    assert_eq!(ids(&later.sessions), ["chat-deploy"]);
    assert!(!ids(&ws.front.recent).contains(&"chat-deploy"));
    client.set_section_collapsed(&section, true).unwrap();
    client.rename_section(&section, "Someday").unwrap();
    let ws = client.workspace();
    let later = ws.front.sections.iter().find(|s| s.id == section).unwrap();
    assert!(later.collapsed);
    assert_eq!(later.name, "Someday");
    client.delete_section(&section).unwrap();
    assert!(ids(&client.workspace().front.recent).contains(&"chat-deploy"));

    client.archive_session("chat-cjk").unwrap();
    assert!(ids(&client.workspace().archived).contains(&"chat-cjk"));
    client.unarchive_session("chat-cjk").unwrap();
    client.rename_session("chat-cjk", "Renamed").unwrap();
    assert_eq!(
        client.workspace().session("chat-cjk").unwrap().title,
        "Renamed"
    );
    assert!(client.workspace().session("chat-tabs").unwrap().unseen);
    client.mark_seen("chat-tabs");
    assert!(!client.workspace().session("chat-tabs").unwrap().unseen);
    client.delete_session("chat-blog").unwrap();
    assert!(client.workspace().session("chat-blog").is_none());
}

#[test]
fn send_streams_a_reply_and_adopts_the_echo() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-home").unwrap();
    // On screen: the finished turn is stamped seen (Idle, not Completed).
    session.set_view_attached(true);
    let before = session.snapshot();
    assert!(before.hydrated);
    assert_eq!(before.transcript_len, 2);
    let first_message = before.entries[0].message.clone();

    let outcome = session
        .send(SendRequest::text("Explain the pipeline"))
        .unwrap();
    let SendOutcome::Started { message_id } = outcome else {
        panic!("idle chat starts a turn: {outcome:?}");
    };
    // Optimistic echo, instantly.
    let echo = session.snapshot();
    assert_eq!(echo.pending.len(), 1);
    assert_eq!(echo.pending[0].message_id, message_id);
    assert!(echo.entry(&message_id).unwrap().echo.is_some());
    assert!(echo.working, "a send in flight reads as working");

    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = session.watch(move |snap| {
        let _ = tx.send(snap.revision);
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(1)).is_ok(),
        "watch delivers the current snapshot"
    );
    wait_for("adoption", Duration::from_secs(5), || {
        session
            .snapshot()
            .entry(&message_id)
            .is_some_and(|e| e.echo.is_none())
    });
    wait_for("reply completes", Duration::from_secs(20), || {
        let snap = session.snapshot();
        let last = snap.transcript().last().unwrap();
        last.message.role == MessageRole::Assistant
            && last.message.status == Some(MessageStatus::Complete)
            && !snap.working
    });
    let after = session.snapshot();
    assert!(rx.try_iter().count() > 0, "watch follows the stream");
    assert!(after.pending.is_empty());
    assert_eq!(after.transcript_len, 4);
    // Settled entries keep their shared message across every update.
    assert!(Arc::ptr_eq(&after.entries[0].message, &first_message));
    let reply = after.transcript().last().unwrap();
    assert!(reply.message.parts.len() >= 4, "reasoning + text + tools");
    let row = client.workspace().session("chat-home").unwrap().clone();
    assert_eq!(row.indicator, ChatIndicator::Idle);
    assert_eq!(row.preview.as_deref(), Some("| Stage | Per token |"));
}

#[test]
fn sends_while_working_park_on_the_queue_and_drain() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-deploy").unwrap();
    session.send(SendRequest::text("first")).unwrap();
    wait_for("working", Duration::from_secs(5), || {
        session.composer().live.indicator == ChatIndicator::Working && session.snapshot().streaming
    });
    let outcome = session.send(SendRequest::text("second")).unwrap();
    let SendOutcome::Queued { queue_id } = outcome else {
        panic!("busy chat queues: {outcome:?}");
    };
    assert_eq!(session.composer().queue.len(), 1);
    assert_eq!(session.composer().queue[0].id, queue_id);
    assert!(session.composer().queue[0].from_this_device);
    wait_for("queue drains into a turn", Duration::from_secs(30), || {
        session.composer().queue.is_empty() && session.snapshot().entry(&queue_id).is_some()
    });
    wait_for("idle", Duration::from_secs(30), || {
        !session.snapshot().working
    });
}

#[test]
fn questions_answer_through_respond_input() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-deploy").unwrap();
    session
        .send(SendRequest::text("please ?ask first"))
        .unwrap();
    wait_for("question", Duration::from_secs(10), || {
        session.composer().open_input.is_some()
    });
    assert_eq!(
        client.workspace().session("chat-deploy").unwrap().indicator,
        ChatIndicator::AwaitingInput
    );
    let input = session.composer().open_input.clone().unwrap();
    session
        .respond_input(
            &input.request_id,
            vec![zeron_proto_answer(&input.questions[0].id, "iOS only")],
        )
        .unwrap();
    wait_for("answered", Duration::from_secs(10), || {
        session.composer().open_input.is_none() && !session.snapshot().working
    });
    let last = session.snapshot().transcript().last().unwrap().clone();
    assert!(format!("{:?}", last.message.parts).contains("iOS only"));
}

fn zeron_proto_answer(question_id: &str, label: &str) -> zeron_client::UserInputAnswer {
    zeron_client::UserInputAnswer {
        question_id: question_id.into(),
        labels: vec![label.into()],
    }
}

#[test]
fn interrupt_aborts_the_live_turn() {
    let (client, _dir) = demo(DemoOptions {
        stream_speed: StreamSpeed::Realistic,
        ..Default::default()
    });
    let session = client.open_session("chat-deploy").unwrap();
    session.send(SendRequest::text("long one")).unwrap();
    wait_for("streaming", Duration::from_secs(5), || {
        session.snapshot().streaming
    });
    session.interrupt().unwrap();
    wait_for("aborted", Duration::from_secs(5), || {
        let snap = session.snapshot();
        snap.transcript().last().unwrap().message.status == Some(MessageStatus::Aborted)
            && !snap.working
    });
}

#[test]
fn opening_the_live_chat_finishes_its_stream() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-veil").unwrap();
    assert!(session.snapshot().streaming);
    wait_for("veil completes", Duration::from_secs(20), || {
        !session.snapshot().streaming && !session.snapshot().working
    });
}

#[test]
fn synthetic_transcripts_scale() {
    let (client, _dir) = demo(DemoOptions {
        transcript_scale: TranscriptScale::Huge,
        ..fast()
    });
    let session = client.open_session("chat-veil").unwrap();
    assert_eq!(session.snapshot().transcript_len, 1200);
}

#[test]
fn empty_fixtures() {
    let (client, _dir) = demo(DemoOptions {
        fixture: DemoFixture::NoProjects,
        ..fast()
    });
    let ws = client.workspace();
    assert!(ws.projects.is_empty() && ws.front.recent.is_empty());
    assert!(ws.devices.iter().any(|d| d.is_execution_host));
    let (client, _dir) = demo(DemoOptions {
        fixture: DemoFixture::IosOnly,
        ..fast()
    });
    assert!(client.workspace().execution_devices().is_empty());
}

#[test]
fn new_sessions_are_born_on_chat2() {
    let (client, _dir) = demo(fast());
    let id = client
        .create_session(zeron_client::NewSession {
            target: zeron_client::SessionTarget::Project {
                space_id: "space-edge".into(),
            },
            config: None,
            branch: None,
            cwd: None,
            title: None,
        })
        .unwrap();
    let row = client.workspace().session(&id).unwrap().clone();
    assert_eq!(row.room_gen, 2);
    assert_eq!(row.title, "New session");
    assert_eq!(client.workspace().front.recent[0].id, id);
    let session = client.open_session(&id).unwrap();
    assert_eq!(session.snapshot().transcript_len, 0);
    session
        .send(SendRequest::text("Deploy the landing page"))
        .unwrap();
    wait_for(
        "titled from the first prompt",
        Duration::from_secs(5),
        || client.workspace().session(&id).unwrap().title == "Deploy the landing page",
    );
}

#[test]
fn warm_sessions_are_capped_and_preload_follows_the_front_page() {
    let (client, _dir) = demo(fast());
    client.preload_sessions();
    let open = client.open_session_ids();
    assert_eq!(open.len(), zeron_client::PRELOAD_CAP);
    for id in ["chat-veil", "chat-picker", "chat-tabs", "chat-errored"] {
        assert!(open.iter().any(|o| o == id), "{id} preloaded");
    }
    let attached = client.open_session("chat-home").unwrap();
    attached.set_view_attached(true);
    for id in [
        "chat-cjk",
        "chat-deploy",
        "chat-blog",
        "chat-ios-scroll",
        "chat-ios-keyboard",
    ] {
        client.open_session(id).unwrap();
        client.close_session(id);
    }
    let open = client.open_session_ids();
    assert!(open.len() <= zeron_client::WARM_SESSION_CAP + 2, "{open:?}");
    assert!(
        open.iter().any(|o| o == "chat-home"),
        "attached sessions stay"
    );
    assert!(
        open.iter().any(|o| o == "chat-veil"),
        "streaming sessions stay"
    );
}

#[test]
fn offline_sends_queue_durably_and_deliver_on_recovery() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-deploy").unwrap();
    client.set_network_online(false);
    wait_for("graced offline", Duration::from_secs(8), || {
        client.connectivity().state == zeron_client::ConnectivityState::Offline
    });
    let SendOutcome::Started { message_id } =
        session.send(SendRequest::text("while offline")).unwrap()
    else {
        panic!("idle chat starts a turn");
    };
    std::thread::sleep(Duration::from_millis(400));
    let composer = session.composer();
    assert_eq!(composer.send_state, Some(zeron_client::SendState::Queued));
    assert!(composer.delivery_degraded);
    assert_eq!(
        client
            .workspace()
            .session("chat-deploy")
            .unwrap()
            .send_state,
        Some(zeron_client::SendState::Queued)
    );
    assert!(
        session
            .snapshot()
            .entry(&message_id)
            .unwrap()
            .echo
            .is_some()
    );
    client.set_network_online(true);
    wait_for("delivered after recovery", Duration::from_secs(5), || {
        session
            .snapshot()
            .entry(&message_id)
            .is_some_and(|e| e.echo.is_none())
    });
    wait_for("send state clears", Duration::from_secs(5), || {
        session.composer().send_state.is_none()
    });
}

#[test]
fn idle_chat_send_completes_and_stops_working() {
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-deploy").unwrap();
    session.set_view_attached(true);
    assert!(!session.snapshot().working);
    session
        .send(SendRequest::text(
            "Summarize the launch post in three bullets.",
        ))
        .unwrap();
    wait_for("reply streams", Duration::from_secs(5), || {
        session.snapshot().streaming
    });
    wait_for(
        "turn completes, working clears",
        Duration::from_secs(15),
        || {
            let snap = session.snapshot();
            let composer = session.composer();
            !snap.working
                && !snap.streaming
                && !composer.live.turn_running
                && !composer.live.can_interrupt
                && snap.transcript().last().unwrap().message.status == Some(MessageStatus::Complete)
        },
    );
    assert_eq!(
        client.workspace().session("chat-deploy").unwrap().indicator,
        ChatIndicator::Idle
    );
}

#[test]
fn queueing_while_busy_at_realistic_speed() {
    let (client, _dir) = demo(DemoOptions::default());
    let session = client.open_session("chat-home").unwrap();
    session.send(SendRequest::text("first")).unwrap();
    wait_for("turn running", Duration::from_secs(5), || {
        session.composer().live.turn_running
    });
    let SendOutcome::Queued { queue_id } = session.send(SendRequest::text("second")).unwrap()
    else {
        panic!("a running turn parks the send on the queue");
    };
    assert_eq!(session.composer().queue.len(), 1);
    // The queued row delivers once the first turn ends.
    wait_for("queued row delivered", Duration::from_secs(60), || {
        session.composer().queue.is_empty() && session.snapshot().entry(&queue_id).is_some()
    });
    wait_for("idle again", Duration::from_secs(60), || {
        !session.snapshot().working && !session.composer().live.turn_running
    });
}

#[test]
fn sends_to_an_offline_host_park_as_queued_not_working() {
    // chat-blog lives on the Mac Studio, which is offline in the demo.
    let (client, _dir) = demo(fast());
    let session = client.open_session("chat-blog").unwrap();
    assert!(!session.composer().host.online);
    let SendOutcome::Started { message_id } =
        session.send(SendRequest::text("Pick a tone ?ask")).unwrap()
    else {
        panic!("idle chat starts a turn");
    };
    std::thread::sleep(Duration::from_millis(400));
    let snap = session.snapshot();
    let composer = session.composer();
    assert_eq!(snap.pending[0].message_id, message_id);
    assert_eq!(snap.pending[0].state, zeron_client::SendState::Queued);
    assert!(!snap.working, "a parked send is not a running turn");
    assert!(!composer.live.turn_running && !composer.live.can_interrupt);
    assert!(composer.delivery_degraded);
    // The sidebar still shows the send in flight (desktop display_status_for).
    let row = client.workspace().session("chat-blog").unwrap().clone();
    assert_eq!(row.send_state, Some(zeron_client::SendState::Queued));
    assert_eq!(row.host_indicator, ChatIndicator::Idle);
}
