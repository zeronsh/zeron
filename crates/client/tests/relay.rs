//! A phone on the relay layer (docs/design/relay-mobile.md), through the real
//! `Client` and real Loro docs against an in-process edge that speaks the relay
//! frames. The test plays the host with the same `ChatClient` and `RelayHandle` an
//! engine runs. These cover what a phone does; the room's own behaviour is proven
//! against real workerd in `edge/test/workerd/chat-relay.workerd.test.ts`.
//!
//! Tests run one at a time: the OS network path and the registry's "this process
//! runs the layer" flag are process-wide.

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::future::BoxFuture;
use loro::{ExportMode, LoroDoc, VersionVector};
use support::mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials, MessageRole, SendRequest, SessionHandle, TranscriptLoad};
use zeron_doc::{MessagePart, RegistryDoc, SessionCommandPayload, SessionCommandStatus, SessionDoc, SessionMessageEntry};
use zeron_proto::{Chat, Device, Space};
use zeron_sync::chat_client::{ChatClient, ChatDocSink, ChatTransport, CheckpointFetcher, RowImportOutcome};
use zeron_sync::chat_relay::{RelayHandle, RelayImport, RelayPolicy};
use zeron_sync::{DocsStore, RegistryClient, StaticUrl, SyncError};

const HOST: &str = "dev-mac";
const PHONE: &str = "ios-relay";
const SPACE: &str = "space-relay";
const DEAD_EDGE: &str = "http://127.0.0.1:9";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn eventually(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ── the registry a host keeps ────────────────────────────────────────────────

fn device_row(relay: bool) -> Device {
    let now = Utc::now();
    let mut capabilities: Vec<String> =
        zeron_client::rpc::capability::ALL_QUEUE.iter().map(|c| (*c).to_owned()).collect();
    if relay {
        capabilities.push(zeron_proto::relay_gen::CHAT_RELAY_CAPABILITY.to_owned());
    }
    Device {
        id: HOST.into(),
        name: "MacBook Pro".into(),
        platform: "macos".into(),
        last_seen_at: Some(now),
        created_at: Some(now),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        cursor_sdk_version: None,
        capabilities,
    }
}

fn chat_row(id: &str, generation: u32, has_messages: bool) -> Chat {
    let now = Utc::now();
    Chat {
        id: id.into(),
        device_id: HOST.into(),
        title: Some("Relayed chat".into()),
        archived: false,
        cwd: Some("/Users/dev/live".into()),
        branch: None,
        checkout_id: None,
        source_context: None,
        config: None,
        last_message_preview: None,
        last_message_at: has_messages.then_some(now),
        created_at: now,
        harness_session_id: None,
        harness_session_cwd: None,
        harness_session_harness: None,
        space_id: Some(SPACE.into()),
        last_seen_at: Some(now),
        room_gen: Some(generation),
        parent_chat_id: None,
        child_kind: None,
        sleep: None,
    }
}

struct Registry {
    doc: Arc<Mutex<RegistryDoc>>,
    client: RegistryClient,
}

impl Registry {
    /// `present`: the host beats presence. Otherwise its row says it exists, and
    /// nothing says it's here.
    async fn start(edge: &MockEdge, chats: &[Chat], present: bool) -> Self {
        let doc = Arc::new(Mutex::new(RegistryDoc::new(HOST)));
        {
            let now = Utc::now();
            let mut doc = doc.lock().unwrap();
            doc.upsert_device(&device_row(true)).unwrap();
            doc.upsert_space(&Space {
                id: SPACE.into(),
                device_id: HOST.into(),
                path: "/Users/dev/live".into(),
                name: Some("Live".into()),
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                repository_id: None,
                github_repo: None,
                icon: None,
                created_at: now,
            })
            .unwrap();
            for chat in chats {
                doc.upsert_chat(chat).unwrap();
            }
        }
        let client = RegistryClient::connect(&edge.registry.url(), doc.clone(), HOST).await.unwrap();
        client.nudge();
        if present {
            client.set_presence(Utc::now().timestamp_millis());
        }
        Self { doc, client }
    }

    fn be_present(&self) {
        self.client.set_presence(Utc::now().timestamp_millis());
    }

    fn write(&self, f: impl FnOnce(&mut RegistryDoc)) {
        f(&mut self.doc.lock().unwrap());
        self.client.nudge();
    }
}

// ── the host's chat ──────────────────────────────────────────────────────────

struct HostSink {
    doc: LoroDoc,
}

impl ChatDocSink for HostSink {
    fn cursor_is_verified(&self) -> bool {
        true
    }
    fn apply_row(&self, bytes: &[u8], _cursor: u64) -> RowImportOutcome {
        match self.doc.import(bytes) {
            Ok(status) if status.pending.is_some() => RowImportOutcome::PendingDependencies,
            _ => RowImportOutcome::Applied,
        }
    }
    fn apply_checkpoint(&self, bytes: &[u8], _cursor: u64) -> Result<(), String> {
        self.doc.import(bytes).map(|_| ()).map_err(|e| e.to_string())
    }
    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        VersionVector::decode(frontier).is_ok_and(|vv| !vv.is_empty() && self.doc.oplog_vv().includes_vv(&vv))
    }
    fn advance_cursor(&self, _cursor: u64) {}
    fn relay_version(&self) -> Option<Vec<u8>> {
        Some(self.doc.oplog_vv().encode())
    }
    fn relay_lacks(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !self.doc.oplog_vv().includes_vv(&vv))
    }
    fn relay_behind(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !vv.includes_vv(&self.doc.oplog_vv()))
    }
    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let vv = VersionVector::decode(since).ok()?;
        if vv.includes_vv(&self.doc.oplog_vv()) {
            return Some(Vec::new());
        }
        self.doc.export(ExportMode::updates(&vv)).ok()
    }
    fn relay_import(&self, bytes: &[u8], _history: bool) -> RelayImport {
        match self.doc.import(bytes) {
            Ok(status) if status.pending.is_some() => RelayImport::Parked,
            Ok(_) => RelayImport::Applied,
            Err(_) => RelayImport::Unreadable,
        }
    }
}

struct NoHttp;

impl CheckpointFetcher for NoHttp {
    fn fetch(&self) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        Box::pin(async { Err(SyncError::Protocol("no checkpoint".into())) })
    }
}

impl ChatTransport for NoHttp {
    fn fetch_rows(&self, _after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        Box::pin(async { Err(SyncError::Protocol("sockets only".into())) })
    }
    fn push(&self, _batch: String, _bytes: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>> {
        Box::pin(async { Err(SyncError::Protocol("sockets only".into())) })
    }
}

/// A host relaying a chat: stores nothing, reads and confirms the queue.
struct Host {
    doc: LoroDoc,
    relay: RelayHandle,
    client: ChatClient,
    sent: Mutex<VersionVector>,
}

impl Host {
    async fn join(edge: &MockEdge, chat: &str, doc: LoroDoc) -> Self {
        let sink = Arc::new(HostSink { doc: doc.clone() });
        let relay = RelayHandle::spawn(HOST, sink.clone(), RelayPolicy { confirm_queue: true });
        relay.set_hello(true, true);
        relay.set_host(Some(HOST.into()));
        let url = format!("{}/chat2/{chat}/ws?device={HOST}", edge.edge_url().replacen("http", "ws", 1));
        let client = ChatClient::connect_via_relay(
            Arc::new(StaticUrl(url)),
            sink,
            Arc::new(NoHttp),
            HOST,
            0,
            Arc::new(NoHttp),
            Some(relay.clone()),
        )
        .await
        .expect("host joins");
        let sent = Mutex::new(doc.oplog_vv());
        Self { doc, relay, client, sent }
    }

    /// One reply, relayed: down the room and not stored.
    fn say(&self, id: &str, text: &str) {
        SessionDoc::from_doc(self.doc.clone())
            .push_message(&message(id, MessageRole::Assistant, HOST, text))
            .unwrap();
        self.doc.commit();
        let mut sent = self.sent.lock().unwrap();
        let bytes = self.doc.export(ExportMode::updates(&sent)).unwrap();
        *sent = self.doc.oplog_vv();
        self.relay.local_update(bytes, true);
    }

    /// This host's doc holds a message with this text, however it was written: a
    /// run command, or a queue row when one was already on its way.
    fn received(&self, text: &str) -> bool {
        let session = SessionDoc::from_doc(self.doc.clone());
        let run = session.read_commands().unwrap_or_default().into_iter().any(|c| {
            c.status == SessionCommandStatus::Pending
                && matches!(c.payload, SessionCommandPayload::Run { request, .. } if request.prompt == text)
        });
        run || session.read_queue().unwrap_or_default().iter().any(|row| row.text == text)
    }

    async fn leave(self) {
        self.client.shutdown().await;
    }
}

fn message(id: &str, role: MessageRole, device: &str, text: &str) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text { id: "t0".into(), text: text.into() }],
        created_at: Utc::now().timestamp_millis(),
        device_id: device.into(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}

/// A chat's history, as a host holds it.
fn history(texts: &[&str]) -> LoroDoc {
    let doc = LoroDoc::new();
    let session = SessionDoc::from_doc(doc.clone());
    for (i, text) in texts.iter().enumerate() {
        session
            .push_message(&message(&format!("m{i}"), MessageRole::Assistant, HOST, text))
            .unwrap();
    }
    doc.commit();
    doc
}

// ── the phone ────────────────────────────────────────────────────────────────

fn phone_at(edge: &str, dir: &Path, relay_layer: bool) -> Client {
    let mut config = ClientConfig::new(edge, dir);
    config.device_id = PHONE.into();
    config.device_name = "Relay iPhone".into();
    config.relay_layer = relay_layer;
    Client::new(
        config,
        Credentials::Dev { user_id: "user-1".into(), org_id: "org-1".into() },
        Arc::new(NullListener),
    )
    .unwrap()
}

fn phone(edge: &MockEdge, dir: &Path) -> Client {
    phone_at(&edge.edge_url(), dir, true)
}

fn texts(session: &SessionHandle) -> Vec<String> {
    session
        .snapshot()
        .entries
        .iter()
        .flat_map(|entry| entry.message.parts.clone())
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn shows(session: &SessionHandle, text: &str) -> bool {
    texts(session).iter().any(|t| t == text)
}

async fn registered(client: &Client, chat: &str) {
    eventually("the chat's row", 10, || client.workspace().session(chat).is_some()).await;
}

/// The phone leaves as the OS would have it leave: flushed, sockets gone.
fn leave(client: Client) {
    client.on_background();
    client.shutdown();
    drop(client);
}

fn outbox(dir: &Path, chat: &str) -> usize {
    DocsStore::open(dir).unwrap().pending_chat_updates(chat).unwrap().len()
}

fn journal(dir: &Path, chat: &str) -> usize {
    DocsStore::open(dir).unwrap().pending_chat_inbox(chat).unwrap().0.len()
}

// ── a real edge without the layer ────────────────────────────────────────────

/// A release ships ahead of any edge flag, so a phone with the layer on must work against
/// an edge with it off, and against one from before the layer existed. Needs that edge:
///
/// ```sh
/// (cd edge && npx wrangler dev --port 27653 --var AUTH_MODE:dev)
/// ZERON_PLAIN_EDGE=http://127.0.0.1:27653 cargo test -p zeron-client --test relay -- --ignored
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live edge without the layer: set ZERON_PLAIN_EDGE"]
async fn a_phone_with_the_layer_works_on_an_edge_without_it() {
    let _serial = SERIAL.lock().await;
    let edge = std::env::var("ZERON_PLAIN_EDGE").expect("ZERON_PLAIN_EDGE");
    let user = format!("plain-{}", uuid::Uuid::new_v4().simple());
    let chat = uuid::Uuid::new_v4().to_string();
    let token = format!("{user}@org1");
    let stats = || async {
        let reply = reqwest::Client::new()
            .get(format!("{edge}/chat2/{chat}/stats"))
            .bearer_auth(&token)
            .send()
            .await;
        match reply {
            Ok(reply) if reply.status().is_success() => reply.json::<serde_json::Value>().await.ok(),
            _ => None,
        }
    };

    // A chat its row says is relayed, as a downgraded edge could leave it.
    let host = Arc::new(Mutex::new(RegistryDoc::new(HOST)));
    {
        let mut doc = host.lock().unwrap();
        doc.upsert_device(&device_row(true)).unwrap();
        doc.upsert_chat(&chat_row(&chat, 3, false)).unwrap();
    }
    let url = format!(
        "{}/registry/org1/ws?token={token}&device={HOST}",
        edge.replacen("http", "ws", 1)
    );
    let registry = RegistryClient::connect(&url, host, HOST).await.expect("the host joins");
    registry.nudge();
    registry.set_presence(Utc::now().timestamp_millis());

    let dir = tempfile::tempdir().unwrap();
    let mut config = ClientConfig::new(&edge, dir.path());
    config.device_id = PHONE.into();
    config.relay_layer = true;
    let client = Client::new(
        config,
        Credentials::Dev { user_id: user.clone(), org_id: "org1".into() },
        Arc::new(NullListener),
    )
    .unwrap();
    registered(&client, &chat).await;
    eventually("the host's row", 15, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    let session = client.open_session(&chat).unwrap();
    session.set_view_attached(true);
    eventually("the room", 15, || session.composer().room.connected).await;

    // The room has no queue, so it's a stored row, the same as ever.
    session.send(SendRequest::text("no layer on this edge")).unwrap();
    eventually("the outbox to let go", 15, || outbox(dir.path(), &chat) == 0).await;
    let state = stats().await.expect("the room's stats");
    assert!(state["headSeq"].as_u64().unwrap_or(0) >= 1, "a stored row: {state}");
    assert!(state["relayQueue"].as_u64().unwrap_or(0) == 0, "nothing queued: {state}");
    assert!(session.composer().room.connected, "and the socket stays up");
    registry.shutdown().await;
    leave(client);
}

// ── saying what it runs ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_with_the_layer_says_so_and_one_without_sends_what_shipped_phones_send() {
    let _serial = SERIAL.lock().await;
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(
        &edge,
        &[chat_row("relayed", 3, false), chat_row("stored", 2, false), chat_row("legacy", 3, false)],
        true,
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, "relayed").await;
    registered(&client, "stored").await;
    let _relayed = client.open_session("relayed").unwrap();
    let _stored = client.open_session("stored").unwrap();
    eventually("both hellos", 10, || !edge.hellos("relayed").is_empty() && !edge.hellos("stored").is_empty()).await;
    assert_eq!(edge.hellos("relayed")[0]["relay"], 1);
    assert_eq!(edge.hellos("relayed")[0]["gen"], 3, "it brings the host back for a relayed chat");
    assert!(edge.hellos("relayed")[0].get("relaying").is_none(), "a phone never hosts");
    assert_eq!(edge.hellos("stored")[0]["relay"], 1);
    assert!(edge.hellos("stored")[0].get("gen").is_none(), "a stored chat's readers need no host");
    assert_eq!(edge.host_hint("relayed").as_deref(), Some(HOST));
    eventually("a pull", 10, || !edge.pulls("relayed").is_empty()).await;
    assert!(edge.pulls("relayed")[0].contains("relay=1"), "its pull isn't an old reader's: {:?}", edge.pulls("relayed"));
    leave(client);

    let dir = tempfile::tempdir().unwrap();
    let client = phone_at(&edge.edge_url(), dir.path(), false);
    registered(&client, "legacy").await;
    let _legacy = client.open_session("legacy").unwrap();
    eventually("its hello", 10, || !edge.hellos("legacy").is_empty()).await;
    let hello = &edge.hellos("legacy")[0];
    assert!(hello.get("relay").is_none() && hello.get("gen").is_none() && hello.get("relaying").is_none(), "{hello}");
    eventually("its pull", 10, || !edge.pulls("legacy").is_empty()).await;
    assert!(!edge.pulls("legacy")[0].contains("relay="), "{:?}", edge.pulls("legacy"));
    leave(client);
}

// ── live frames, and what's kept of them ─────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relayed_frames_reach_the_transcript_are_saved_and_leave_nothing_on_the_edge() {
    let _serial = SERIAL.lock().await;
    let chat = "live-frames";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("the phone in the room", 10, || !edge.hellos(chat).is_empty() && session.composer().room.connected).await;

    host.say("r1", "first token");
    eventually("the first frame", 10, || shows(&session, "first token")).await;
    host.say("r2", "second token");
    eventually("the second frame", 10, || shows(&session, "second token")).await;
    assert!(edge.rows(chat).is_empty(), "nothing is stored on the edge");

    // Everything that came by relay is covered by a save, and the journal lets go.
    eventually("the journal to retire", 10, || journal(dir.path(), chat) == 0).await;
    host.leave().await;
    leave(client);

    // No network, no host: all of it's here, and it says nothing's wrong.
    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    offline.set_network_online(false);
    let session = offline.open_session(chat).unwrap();
    session.set_view_attached(true);
    assert!(shows(&session, "first token") && shows(&session, "second token"), "{:?}", texts(&session));
    assert_eq!(session.composer().transcript_load, TranscriptLoad::Ready);
    offline.set_network_online(true);
    leave(offline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_kill_left_in_the_journal_is_read_back_with_no_network_and_then_retired() {
    let _serial = SERIAL.lock().await;
    let chat = "killed";
    let edge = MockEdge::start_relay().await;
    let registry = Registry::start(&edge, &[chat_row(chat, 3, true)], false).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    leave(client);
    drop(registry);

    // A frame imported and journaled, and iOS kills the app before any save.
    let frame = history(&["only ever relayed"]).export(ExportMode::all_updates()).unwrap();
    DocsStore::open(dir.path()).unwrap().journal_chat_inbox(chat, &frame).unwrap();

    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    offline.set_network_online(false);
    let session = offline.open_session(chat).unwrap();
    assert!(shows(&session, "only ever relayed"), "{:?}", texts(&session));
    assert_eq!(session.composer().transcript_load, TranscriptLoad::Ready);
    // Kept until a save holds it, then let go.
    eventually("the save to cover it", 10, || journal(dir.path(), chat) == 0).await;
    offline.set_network_online(true);
    leave(offline);
    let reopened = phone_at(DEAD_EDGE, dir.path(), true);
    let session = reopened.open_session(chat).unwrap();
    assert!(shows(&session, "only ever relayed"), "the save has it now");
    leave(reopened);
}

// ── catch-up ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_joiner_loads_from_the_host_and_keeps_it_for_when_nobody_is_there() {
    let _serial = SERIAL.lock().await;
    let chat = "late-join";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, true)], true).await;
    let host = Host::join(&edge, chat, history(&["one", "two", "three"])).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    eventually("the history", 15, || texts(&session).len() == 3).await;
    assert!(edge.rows(chat).is_empty());
    assert_eq!(edge.checkpoint_downloads(chat), 0);
    host.leave().await;
    leave(client);

    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    let session = offline.open_session(chat).unwrap();
    assert_eq!(texts(&session), vec!["one", "two", "three"]);
    leave(offline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_socket_loses_nothing_of_what_went_by_meanwhile() {
    let _serial = SERIAL.lock().await;
    let chat = "dropped";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 10, || session.composer().room.connected).await;
    host.say("r1", "before");
    eventually("the first", 10, || shows(&session, "before")).await;

    // A network switch: every socket drops, and nothing can rejoin yet.
    edge.refuse_chat_joins(true);
    edge.kick(chat);
    eventually("the drop", 10, || !session.composer().room.connected).await;
    host.say("r2", "during, into nothing");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!shows(&session, "during, into nothing"));
    edge.refuse_chat_joins(false);
    eventually("version exchange to fill the gap", 30, || shows(&session, "during, into nothing")).await;
    assert!(edge.rows(chat).is_empty());
    host.leave().await;
    leave(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_network_path_checks_the_room_and_rejoins_a_room_it_lost() {
    let _serial = SERIAL.lock().await;
    let chat = "new-path";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 10, || session.composer().room.connected).await;

    // A socket on the old path may be dead without saying so. It's asked, not trusted.
    let before = edge.probes(chat);
    client.network_path_changed();
    eventually("a probe on the new path", 5, || edge.probes(chat) > before).await;

    // One already gone doesn't wait out its backoff.
    edge.refuse_chat_joins(true);
    edge.kick(chat);
    eventually("the drop", 10, || !session.composer().room.connected).await;
    host.say("r1", "said while it was gone");
    tokio::time::sleep(Duration::from_millis(200)).await;
    edge.refuse_chat_joins(false);
    client.network_path_changed();
    eventually("the rejoin and catch-up", 10, || shows(&session, "said while it was gone")).await;
    host.leave().await;
    leave(client);
}

/// Android can leave a backgrounded process running for hours, so nothing suspends its
/// sockets for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_at_rest_holds_no_socket_dials_nothing_and_catches_up_on_return() {
    let _serial = SERIAL.lock().await;
    let chat = "at-rest";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("the room", 10, || session.composer().room.connected).await;
    host.say("r1", "before resting");
    eventually("the first", 10, || shows(&session, "before resting")).await;

    client.on_background();
    client.on_background_idle();
    eventually("the room's socket to close", 5, || !session.composer().room.connected).await;

    // Long enough for several rounds of the backoff a dropped socket would go through.
    let hellos = edge.hellos(chat).len();
    let pulls = edge.pulls(chat).len();
    host.say("r2", "said while it rested");
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(edge.hellos(chat).len(), hellos, "nothing dials at rest");
    assert_eq!(edge.pulls(chat).len(), pulls, "nothing polls at rest");
    assert!(!shows(&session, "said while it rested"));

    client.on_foreground();
    eventually("the catch-up", 15, || shows(&session, "said while it rested")).await;
    assert!(
        edge.pulls(chat).iter().all(|query| !query.contains("fallback=1")),
        "resting isn't a network with no sockets",
    );
    session.send(SendRequest::text("back again")).unwrap();
    eventually("the send to arrive", 15, || host.received("back again")).await;
    host.leave().await;
    leave(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_left_behind_at_rest_waits_in_the_outbox_and_goes_on_return() {
    let _serial = SERIAL.lock().await;
    let chat = "rest-send";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    eventually("the host's capability", 10, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 10, || session.composer().room.connected).await;

    client.on_background();
    client.on_background_idle();
    eventually("the room's socket to close", 5, || !session.composer().room.connected).await;
    session.send(SendRequest::text("written just before it slept")).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(outbox(dir.path(), chat), 1, "held on disk");
    assert_eq!(edge.queue_len(chat), 0, "and not sent");

    client.on_foreground();
    eventually("the send to arrive", 15, || host.received("written just before it slept")).await;
    eventually("the outbox to let go", 10, || outbox(dir.path(), chat) == 0).await;
    host.leave().await;
    leave(client);
}

// ── sends ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_with_no_network_survives_relaunch_queues_and_reaches_a_host_that_was_away() {
    let _serial = SERIAL.lock().await;
    let chat = "offline-send";
    let edge = MockEdge::start_relay().await;
    let registry = Registry::start(&edge, &[chat_row(chat, 3, false)], false).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    eventually("the host's capability", 10, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    leave(client);

    // Airplane mode: the send lands in the outbox and on screen.
    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    offline.set_network_online(false);
    let session = offline.open_session(chat).unwrap();
    session.send(SendRequest::text("sent from the air")).unwrap();
    assert!(shows(&session, "sent from the air"), "the echo");
    assert!(outbox(dir.path(), chat) >= 1, "the outbox holds it");
    offline.set_network_online(true);
    leave(offline);
    let held = outbox(dir.path(), chat);
    assert!(held >= 1, "a relaunch still has it");

    // Back online with the host still away: the room's queue takes it.
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    assert!(shows(&session, "sent from the air"), "still on screen after a relaunch");
    eventually("the queue to hold it", 15, || edge.queue_len(chat) == held).await;
    eventually("the outbox to let go", 10, || outbox(dir.path(), chat) == 0).await;
    assert!(edge.rows(chat).is_empty(), "no stored row: {:?}", edge.rows(chat).len());
    assert!(edge.http_pushes(chat).is_empty(), "it never took the HTTPS path");

    // The host returns: it reads the queue, saves it, and confirms it.
    registry.be_present();
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    eventually("the host to read the message", 15, || host.received("sent from the air")).await;
    eventually("the queue to empty", 10, || edge.queue_len(chat) == 0).await;

    // And what it writes comes back.
    host.say("reply", "got it");
    eventually("the reply", 10, || shows(&session, "got it")).await;
    host.leave().await;
    leave(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chats_generation_decides_where_each_send_goes_and_none_is_lost() {
    let _serial = SERIAL.lock().await;
    let chat = "flips";
    let edge = MockEdge::start_relay().await;
    let registry = Registry::start(&edge, &[chat_row(chat, 2, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    eventually("the host's capability", 10, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 10, || session.composer().room.connected).await;

    // Generation 2: a stored row.
    session.send(SendRequest::text("on stored rows")).unwrap();
    eventually("a stored row", 10, || !edge.rows(chat).is_empty()).await;
    let rows = edge.rows(chat).len();

    // The host flips it: the next send goes to the queue.
    registry.write(|doc| {
        let mut row = doc.chat(chat).unwrap().unwrap();
        row.room_gen = Some(3);
        doc.upsert_chat(&row).unwrap();
    });
    eventually("the flip", 10, || client.workspace().session(chat).is_some_and(|r| r.room_gen == 3)).await;
    tokio::time::sleep(Duration::from_millis(1200)).await; // a tick applies the route
    session.send(SendRequest::text("on the queue")).unwrap();
    eventually("the host to get it", 10, || host.received("on the queue")).await;
    assert_eq!(edge.rows(chat).len(), rows, "the second send stored nothing");

    // The host goes back to a build that can't read the queue: stored again.
    registry.write(|doc| doc.upsert_device(&device_row(false)).unwrap());
    eventually("the capability to go", 10, || {
        client.workspace().device(HOST).is_some_and(|d| {
            !d.capabilities.iter().any(|c| c == zeron_proto::relay_gen::CHAT_RELAY_CAPABILITY)
        })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    session.send(SendRequest::text("back on rows")).unwrap();
    eventually("a stored row again", 10, || edge.rows(chat).len() > rows).await;

    // Back to generation 2: every send reached the host, whichever way it went.
    registry.write(|doc| {
        let mut row = doc.chat(chat).unwrap().unwrap();
        row.room_gen = Some(2);
        doc.upsert_chat(&row).unwrap();
    });
    eventually("the way back", 10, || client.workspace().session(chat).is_some_and(|r| r.room_gen == 2)).await;
    eventually("all three at the host", 15, || {
        ["on stored rows", "on the queue", "back on rows"].iter().all(|want| host.received(want))
    })
    .await;
    host.leave().await;
    leave(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_with_the_flag_off_leaves_a_relayed_chat_on_stored_rows() {
    let _serial = SERIAL.lock().await;
    let chat = "edge-off";
    let edge = MockEdge::start_relay().await;
    edge.set_relay_layer(false);
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    eventually("the host's capability", 10, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 10, || session.composer().room.connected).await;
    session.send(SendRequest::text("no layer here")).unwrap();
    eventually("a stored row", 10, || !edge.rows(chat).is_empty()).await;
    assert_eq!(edge.queue_len(chat), 0);
    eventually("the outbox to let go", 10, || outbox(dir.path(), chat) == 0).await;
    leave(client);
}

// ── checkpoints ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sleeping_machines_checkpoint_loads_once_however_often_the_phone_comes_back() {
    let _serial = SERIAL.lock().await;
    let chat = "asleep";
    let edge = MockEdge::start_relay().await;
    let doc = history(&["before it slept", "the last thing it said"]);
    edge.post_checkpoint(chat, doc.export(ExportMode::Snapshot).unwrap(), doc.oplog_vv().encode());
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, true)], false).await;

    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("the checkpoint", 15, || texts(&session).len() == 2).await;
    assert_eq!(session.composer().transcript_load, TranscriptLoad::Ready);
    let once = edge.checkpoint_downloads(chat);
    assert!(once >= 1);

    // The socket drops and returns, again and again: nothing to download.
    for _ in 0..3 {
        edge.kick(chat);
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    eventually("the room back", 15, || session.composer().room.connected).await;
    assert_eq!(edge.checkpoint_downloads(chat), once, "reconnects download nothing");
    leave(client);

    // Nor does a relaunch, and it's there with no network at all.
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    assert_eq!(texts(&session).len(), 2);
    eventually("the room", 15, || session.composer().room.connected).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(edge.checkpoint_downloads(chat), once, "a relaunch downloads nothing");
    leave(client);
    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    assert_eq!(texts(&offline.open_session(chat).unwrap()).len(), 2);
    leave(offline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_no_checkpoint_can_fill_never_sets_off_the_download_loop() {
    let _serial = SERIAL.lock().await;
    let chat = "no-loop";
    let edge = MockEdge::start_relay().await;
    // The room's checkpoint stops short. A stored row depends on ops only ever
    // relayed, so no read of that checkpoint will ever help.
    let doc = history(&["in the checkpoint"]);
    edge.post_checkpoint(chat, doc.export(ExportMode::Snapshot).unwrap(), doc.oplog_vv().encode());
    SessionDoc::from_doc(doc.clone())
        .push_message(&message("relayed", MessageRole::Assistant, HOST, "only relayed"))
        .unwrap();
    doc.commit();
    let after_relayed = doc.oplog_vv();
    SessionDoc::from_doc(doc.clone())
        .push_message(&message("stored", MessageRole::Assistant, HOST, "stored after it"))
        .unwrap();
    doc.commit();
    let row = doc.export(ExportMode::updates(&after_relayed)).unwrap();
    edge.inject(chat, HOST, row);
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, true)], false).await;

    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("the checkpoint", 15, || shows(&session, "in the checkpoint")).await;
    // Long enough for a loop to show itself: each round is a redial and a download.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let downloads = edge.checkpoint_downloads(chat);
    let hellos = edge.hellos(chat).len();
    assert_eq!(downloads, 1, "the bootstrap pull and the socket share one read, and it's not read again");
    assert_eq!(hellos, 1, "it holds its socket");
    assert!(session.composer().room.connected);
    assert!(!shows(&session, "stored after it"), "and it can't show what it can't apply");

    // The host's return fills the gap and the parked row with it.
    let host_doc = doc.clone();
    let host = Host::join(&edge, chat, host_doc).await;
    eventually("the gap to fill", 20, || shows(&session, "only relayed") && shows(&session, "stored after it")).await;
    assert_eq!(edge.checkpoint_downloads(chat), downloads, "no more checkpoints for it");
    host.leave().await;
    leave(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_off_screen_leaves_a_new_checkpoint_alone_until_someone_looks() {
    let _serial = SERIAL.lock().await;
    let chat = "deferred";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, true)], false).await;

    // The phone has what it loaded once.
    let doc = history(&["what the phone has"]);
    edge.post_checkpoint(chat, doc.export(ExportMode::Snapshot).unwrap(), doc.oplog_vv().encode());
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    eventually("what it has", 15, || shows(&session, "what the phone has")).await;
    let first = edge.checkpoint_downloads(chat);
    leave(client);

    // A Cloud machine finished a turn and moved the checkpoint on, then slept.
    SessionDoc::from_doc(doc.clone())
        .push_message(&message("next", MessageRole::Assistant, HOST, "the next turn"))
        .unwrap();
    doc.commit();
    edge.post_checkpoint(chat, doc.export(ExportMode::Snapshot).unwrap(), doc.oplog_vv().encode());

    // Warm, off screen: it knows, and spends nothing.
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    let session = client.open_session(chat).unwrap();
    eventually("the room", 15, || session.composer().room.connected).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(edge.checkpoint_downloads(chat), first, "off screen, a whole chat's bytes can wait");
    assert!(shows(&session, "what the phone has"));

    // On screen, with the host away: only the room can help, so it reads, once.
    session.set_view_attached(true);
    eventually("the next turn", 15, || shows(&session, "the next turn")).await;
    assert_eq!(edge.checkpoint_downloads(chat), first + 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(edge.checkpoint_downloads(chat), first + 1, "and only once");
    leave(client);
}

// ── saying where a chat is ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_saved_mid_stream_doesnt_claim_to_be_running_with_nobody_to_hear_from() {
    let _serial = SERIAL.lock().await;
    let chat = "frozen-tail";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, true)], true).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    leave(client);

    // What a kill left: a reply still streaming when the last frame came.
    let doc = LoroDoc::new();
    let mut tail = message("tail", MessageRole::Assistant, HOST, "half a sen");
    tail.status = Some(zeron_doc::MessageStatus::Streaming);
    SessionDoc::from_doc(doc.clone()).push_message(&tail).unwrap();
    doc.commit();
    DocsStore::open(dir.path())
        .unwrap()
        .journal_chat_inbox(chat, &doc.export(ExportMode::all_updates()).unwrap())
        .unwrap();

    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    offline.set_network_online(false);
    let session = offline.open_session(chat).unwrap();
    session.set_view_attached(true);
    assert!(shows(&session, "half a sen"));
    eventually("it to stop saying it's working", 10, || {
        !session.snapshot().working && !session.composer().live.can_interrupt
    })
    .await;
    assert!(session.composer().live.turn_running, "a send still queues rather than starts a turn");
    offline.set_network_online(true);
    leave(offline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_never_loaded_says_where_it_is_and_fills_in_when_its_host_returns() {
    let _serial = SERIAL.lock().await;
    let chat = "never-loaded";
    let edge = MockEdge::start_relay().await;
    let registry =
        Registry::start(&edge, &[chat_row(chat, 3, true), chat_row("no-messages", 3, false)], false).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    registered(&client, "no-messages").await;

    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("it to say so", 15, || session.composer().transcript_load == TranscriptLoad::HostAway).await;
    assert!(texts(&session).is_empty());

    // A chat with nothing in it isn't a chat that's somewhere else.
    let empty = client.open_session("no-messages").unwrap();
    empty.set_view_attached(true);
    eventually("an empty chat to read as empty", 15, || {
        empty.composer().transcript_load == TranscriptLoad::Ready && empty.composer().room.connected
    })
    .await;

    // Its host returns, and the state clears by itself.
    registry.be_present();
    let host = Host::join(&edge, chat, history(&["it was here all along"])).await;
    eventually("the history", 20, || shows(&session, "it was here all along")).await;
    eventually("the state to clear", 5, || session.composer().transcript_load == TranscriptLoad::Ready).await;
    host.leave().await;
    leave(client);

    // No network, never loaded: offline, whatever generation.
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    leave(client);
    let offline = phone_at(DEAD_EDGE, dir.path(), true);
    offline.set_network_online(false);
    let session = offline.open_session(chat).unwrap();
    session.set_view_attached(true);
    assert_eq!(session.composer().transcript_load, TranscriptLoad::Offline);
    offline.set_network_online(true);
    leave(offline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_going_to_the_background_and_back_catches_up_and_still_sends() {
    let _serial = SERIAL.lock().await;
    let chat = "background";
    let edge = MockEdge::start_relay().await;
    let _registry = Registry::start(&edge, &[chat_row(chat, 3, false)], true).await;
    let host = Host::join(&edge, chat, LoroDoc::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    registered(&client, chat).await;
    eventually("the host's capability", 10, || {
        client.workspace().device(HOST).is_some_and(|d| !d.capabilities.is_empty())
    })
    .await;
    let session = client.open_session(chat).unwrap();
    session.set_view_attached(true);
    eventually("the room", 10, || session.composer().room.connected).await;

    // Backgrounded, and iOS cuts every socket while the host keeps going.
    client.on_background();
    edge.refuse_chat_joins(true);
    edge.kick(chat);
    eventually("the drop", 10, || !session.composer().room.connected).await;
    host.say("while-away", "said while suspended");
    tokio::time::sleep(Duration::from_millis(200)).await;
    edge.refuse_chat_joins(false);

    client.on_foreground();
    eventually("the catch-up", 20, || shows(&session, "said while suspended")).await;
    session.send(SendRequest::text("back again")).unwrap();
    eventually("the send to arrive", 15, || host.received("back again")).await;
    eventually("the outbox to let go", 10, || outbox(dir.path(), chat) == 0).await;
    assert!(edge.rows(chat).is_empty(), "the queue took it, not the rows");
    host.leave().await;
    leave(client);
}
