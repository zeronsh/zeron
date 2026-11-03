//! A session's companions in demo mode, end to end through the client's
//! real paths: terminals (open, type, resume after a drop, close),
//! subagents (listed off the transcript, followed live and frozen) and side
//! chats (fresh and forked).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_client::events::NullListener;
use zeron_client::{
    Client, ClientConfig, Credentials, DemoOptions, MessageRole, MessageStatus,
    SessionMessageEntry, StreamSpeed, SubagentSink, SubagentState, TerminalSink, TerminalUpdate,
};

fn demo() -> (Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "ios-test".into();
    config.device_name = "Test iPhone".into();
    let options = DemoOptions {
        stream_speed: StreamSpeed::Fast,
        ..Default::default()
    };
    let client = Client::new(config, Credentials::Demo(options), Arc::new(NullListener)).unwrap();
    (client, dir)
}

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[derive(Default)]
struct Screen {
    bytes: Mutex<Vec<u8>>,
    updates: Mutex<Vec<TerminalUpdate>>,
}

impl Screen {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
    }
    fn exited(&self) -> bool {
        self.updates
            .lock()
            .unwrap()
            .iter()
            .any(|u| matches!(u, TerminalUpdate::Exited { .. }))
    }
}

impl TerminalSink for Screen {
    fn update(&self, update: TerminalUpdate) {
        if let TerminalUpdate::Output(bytes) = &update {
            self.bytes.lock().unwrap().extend_from_slice(bytes);
        }
        self.updates.lock().unwrap().push(update);
    }
}

#[test]
fn a_terminal_opens_on_the_chats_host_runs_commands_and_closes() {
    let (client, _dir) = demo();
    let info = block_on(client.open_terminal("chat-veil", 80, 24)).unwrap();
    assert_eq!(info.device_id, "dev-mac", "the chat's host runs it");
    assert_eq!(info.shell, "zsh");
    let screen = Arc::new(Screen::default());
    let link = client.attach_terminal(info.clone(), 0, screen.clone());
    wait_for("the prompt", Duration::from_secs(5), || {
        screen.text().contains("❯")
    });
    // Keys typed one by one go out batched, in order.
    for b in "echo hi from the phone\r".bytes() {
        link.write(vec![b]);
    }
    wait_for("the echo's output", Duration::from_secs(5), || {
        screen.text().matches("hi from the phone").count() >= 2
    });

    // A second viewer resuming after everything seen gets only what's new.
    let late = Arc::new(Screen::default());
    let seen = screen.updates.lock().unwrap().len() as u64;
    let _resumed = client.attach_terminal(info.clone(), seen, late.clone());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !late.text().contains("Last login"),
        "no replay of what came before"
    );

    link.close();
    wait_for("the exit", Duration::from_secs(5), || screen.exited());
    // The host forgot it: reattaching says so.
    let gone = Arc::new(Screen::default());
    let _again = client.attach_terminal(info, 0, gone.clone());
    wait_for("gone", Duration::from_secs(5), || {
        gone.updates
            .lock()
            .unwrap()
            .iter()
            .any(|u| *u == TerminalUpdate::Gone)
    });
}

#[derive(Default)]
struct Transcript(Mutex<Vec<Vec<Arc<SessionMessageEntry>>>>);

impl SubagentSink for Transcript {
    fn transcript(&self, entries: Vec<Arc<SessionMessageEntry>>) {
        self.0.lock().unwrap().push(entries);
    }
    fn unavailable(&self, message: String) {
        panic!("host unavailable: {message}");
    }
}

#[test]
fn subagents_list_off_the_transcript_and_open_live_or_frozen() {
    let (client, _dir) = demo();
    let session = client.open_session("chat-veil").unwrap();
    wait_for("hydrated", Duration::from_secs(5), || {
        session.snapshot().hydrated
    });
    let agents = zeron_client::subagents::subagents(&session.snapshot().transcript_messages());
    let titles: Vec<(&str, SubagentState)> =
        agents.iter().map(|a| (a.title.as_str(), a.state)).collect();
    assert_eq!(
        titles,
        [
            ("Audit grapheme splits in veil.rs", SubagentState::Done),
            ("Review the fade-curve tests", SubagentState::Done),
            ("Benchmark the veil on 600 turns", SubagentState::Running),
        ]
    );
    assert_eq!(agents[0].model.as_deref(), Some("claude-haiku-5"));
    assert_eq!(agents[0].kind.as_deref(), Some("Explore"));
    assert_eq!(agents[2].kind, None, "general-purpose isn't worth naming");

    // Finished: its uploaded copy, whole, once.
    let frozen = Arc::new(Transcript::default());
    let _a = client
        .watch_subagent(
            "chat-veil",
            agents[0].doc_id.as_deref().unwrap(),
            true,
            frozen.clone(),
        )
        .unwrap();
    wait_for("the frozen copy", Duration::from_secs(5), || {
        !frozen.0.lock().unwrap().is_empty()
    });
    let first = frozen.0.lock().unwrap()[0].clone();
    assert_eq!(first[0].role, MessageRole::User, "it opens with its task");
    assert_eq!(first.last().unwrap().status, Some(MessageStatus::Complete));

    // Running: the live doc, growing.
    let live = Arc::new(Transcript::default());
    let _b = client
        .watch_subagent(
            "chat-veil",
            agents[2].doc_id.as_deref().unwrap(),
            false,
            live.clone(),
        )
        .unwrap();
    wait_for("a live frame", Duration::from_secs(5), || {
        !live.0.lock().unwrap().is_empty()
    });
    let parts = |t: &Transcript| {
        t.0.lock()
            .unwrap()
            .last()
            .unwrap()
            .last()
            .unwrap()
            .parts
            .len()
    };
    let before = parts(&live);
    assert_eq!(
        live.0.lock().unwrap()[0].last().unwrap().status,
        Some(MessageStatus::Streaming)
    );
    wait_for("it to grow", Duration::from_secs(6), || {
        parts(&live) > before
    });
}

#[test]
fn side_chats_start_fresh_or_fork_the_conversation() {
    let (client, _dir) = demo();
    let fresh = client.create_side_chat("chat-veil").unwrap();
    let ws = client.workspace();
    let row = ws.session(&fresh).unwrap();
    assert_eq!(row.parent_chat_id.as_deref(), Some("chat-veil"));
    assert_eq!(row.device_id, "dev-mac", "same host as its parent");
    assert!(
        !ws.front.recent.iter().any(|r| r.id == fresh),
        "side chats stay off the front page"
    );

    // A side chat's own side chat hangs off the root.
    let nested = client.create_side_chat(&fresh).unwrap();
    assert_eq!(
        client
            .workspace()
            .session(&nested)
            .unwrap()
            .parent_chat_id
            .as_deref(),
        Some("chat-veil")
    );

    let fork = block_on(client.fork_side_chat("chat-veil")).unwrap();
    wait_for("the fork's row", Duration::from_secs(5), || {
        client.workspace().session(&fork).is_some()
    });
    assert_eq!(
        client
            .workspace()
            .session(&fork)
            .unwrap()
            .parent_chat_id
            .as_deref(),
        Some("chat-veil")
    );
    let copy = client.open_session(&fork).unwrap();
    wait_for("the copied conversation", Duration::from_secs(5), || {
        copy.snapshot().transcript_len > 0
    });
    let entries = copy.snapshot().transcript_messages();
    let last = entries.last().unwrap();
    assert_eq!(last.role, MessageRole::Assistant);
    assert_eq!(
        last.status,
        Some(MessageStatus::Complete),
        "through the latest completed reply"
    );
}

#[test]
fn a_close_still_reaches_the_host_when_the_link_is_dropped_at_once() {
    let (client, _dir) = demo();
    let info = block_on(client.open_terminal("chat-veil", 80, 24)).unwrap();
    let screen = Arc::new(Screen::default());
    let link = client.attach_terminal(info.clone(), 0, screen.clone());
    wait_for("the prompt", Duration::from_secs(5), || {
        screen.text().contains("❯")
    });
    // What the phone does on Close Terminal: close, then forget the link.
    link.write(b"echo bye".to_vec());
    link.close();
    drop(link);
    let gone = Arc::new(Screen::default());
    wait_for("the host to end it", Duration::from_secs(5), || {
        let probe = Arc::new(Screen::default());
        let _l = client.attach_terminal(info.clone(), 0, probe.clone());
        std::thread::sleep(Duration::from_millis(150));
        let ended = probe
            .updates
            .lock()
            .unwrap()
            .iter()
            .any(|u| *u == TerminalUpdate::Gone);
        if ended {
            gone.updates.lock().unwrap().push(TerminalUpdate::Gone);
        }
        ended
    });
}

#[test]
fn a_paste_past_the_hosts_write_cap_arrives_whole_and_in_order() {
    let (client, _dir) = demo();
    let info = block_on(client.open_terminal("chat-veil", 80, 24)).unwrap();
    let screen = Arc::new(Screen::default());
    let link = client.attach_terminal(info, 0, screen.clone());
    wait_for("the prompt", Duration::from_secs(5), || {
        screen.text().contains("❯")
    });
    let before = screen.text().len();
    // 150 KB in one write, then a marker: three host writes, in order.
    let mut paste = "x".repeat(150_000);
    paste.push('Z');
    link.write(paste.into_bytes());
    wait_for("the echo", Duration::from_secs(10), || {
        screen.text().ends_with('Z')
    });
    let echoed = &screen.text()[before..];
    assert_eq!(echoed.matches('x').count(), 150_000);
}
