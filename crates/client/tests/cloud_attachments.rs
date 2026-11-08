//! A phone's send to a Cloud-hosted chat (docs/design/cloud-attachments.md):
//! the bytes reach the edge's store before the message reaches the chat, the
//! echo shows meanwhile, and a relaunch picks up where it left off.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use loro::LoroDoc;
use support::mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials, SendRequest, SessionHandle};
use zeron_doc::{RegistryDoc, SessionCommandPayload, SessionDoc};
use zeron_proto::{Chat, Device};
use zeron_sync::RegistryClient;

const HOST: &str = "cloud-machine";
const CHAT: &str = "chat-cloud";
const IMAGE: &[u8] = &[1, 2, 3, 4, 5];

fn device(capabilities: Vec<String>) -> Device {
    Device {
        id: HOST.into(),
        name: "Cloud".into(),
        platform: "cloud".into(),
        last_seen_at: Some(Utc::now()),
        created_at: Some(Utc::now()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        cursor_sdk_version: None,
        capabilities,
    }
}

fn chat() -> Chat {
    let now = Utc::now();
    Chat {
        id: CHAT.into(),
        device_id: HOST.into(),
        title: Some("Cloud chat".into()),
        archived: false,
        cwd: Some("/home/user/live".into()),
        branch: None,
        checkout_id: None,
        source_context: None,
        config: None,
        last_message_preview: None,
        last_message_at: Some(now),
        created_at: now,
        harness_session_id: None,
        harness_session_cwd: None,
        harness_session_harness: None,
        space_id: None,
        last_seen_at: Some(now),
        room_gen: Some(2),
        parent_chat_id: None,
        child_kind: None,
        sleep: None,
    }
}

async fn host_registry(edge: &MockEdge, capabilities: Vec<String>) -> RegistryClient {
    let doc = Arc::new(Mutex::new(RegistryDoc::new(HOST)));
    {
        let mut doc = doc.lock().unwrap();
        doc.upsert_device(&device(capabilities)).unwrap();
        doc.upsert_chat(&chat()).unwrap();
    }
    let client = RegistryClient::connect(&edge.registry.url(), doc, HOST)
        .await
        .unwrap();
    client.nudge();
    client.set_presence(Utc::now().timestamp_millis());
    client
}

fn store_capable() -> Vec<String> {
    zeron_proto::capabilities::current()
}

fn peer_only() -> Vec<String> {
    let mut caps = store_capable();
    caps.retain(|cap| cap != zeron_proto::capabilities::ATTACHMENT_STORE_V1);
    caps
}

fn phone(edge: &MockEdge, dir: &std::path::Path) -> Client {
    let mut config = ClientConfig::new(edge.edge_url(), dir);
    config.device_id = "ios-live".into();
    config.device_name = "Live iPhone".into();
    Client::new(
        config,
        Credentials::Dev {
            user_id: "user-1".into(),
            org_id: "org-1".into(),
        },
        Arc::new(NullListener),
    )
    .unwrap()
}

async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The phone in a joined chat, the host's capabilities known.
async fn open(client: &Client, attachment_store: bool) -> SessionHandle {
    wait_for("the chat row", || {
        client.workspace().session(CHAT).is_some()
    })
    .await;
    let session = client.open_session(CHAT).unwrap();
    session.set_view_attached(true);
    wait_for("the room", || session.snapshot().hydrated).await;
    wait_for("the host's capabilities", || {
        let caps = session.composer().host.capabilities.clone();
        caps.queued_attachments && caps.attachment_store == attachment_store
    })
    .await;
    session
}

fn send_image(session: &SessionHandle) {
    session
        .send(SendRequest {
            text: "look at this".into(),
            attachments: vec![zeron_client::OutgoingAttachment {
                name: "shot.png".into(),
                mime_type: "image/png".into(),
                data: IMAGE.to_vec(),
            }],
            worktree: None,
            busy: Default::default(),
        })
        .unwrap();
}

fn send_text(session: &SessionHandle, text: &str) {
    session
        .send(SendRequest {
            text: text.into(),
            attachments: Vec::new(),
            worktree: None,
            busy: Default::default(),
        })
        .unwrap();
}

/// What the host would see: every Run and Steer the room has, in order.
fn room_prompts(edge: &MockEdge) -> Vec<String> {
    let doc = LoroDoc::new();
    for row in edge.rows(CHAT) {
        let _ = doc.import(&row.bytes);
    }
    let session = SessionDoc::from_doc(doc);
    let commands = session
        .read_commands()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| match c.payload {
            SessionCommandPayload::Run { request, .. } => Some(request.prompt),
            SessionCommandPayload::Steer { prompt, .. } => Some(prompt),
            _ => None,
        });
    let rows = session
        .read_queue()
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.text);
    commands.chain(rows).collect()
}

fn room_commands(edge: &MockEdge) -> Vec<zeron_doc::SessionCommandEntry> {
    let doc = LoroDoc::new();
    for row in edge.rows(CHAT) {
        let _ = doc.import(&row.bytes);
    }
    SessionDoc::from_doc(doc)
        .read_commands()
        .unwrap_or_default()
}

/// The host's word on the room's first command: its status, and (when it ran)
/// the user's message in the transcript.
fn host_marks(edge: &MockEdge, status: zeron_doc::SessionCommandStatus, ran: bool) {
    let doc = LoroDoc::new();
    for row in edge.rows(CHAT) {
        let _ = doc.import(&row.bytes);
    }
    let before = doc.oplog_vv();
    let session = SessionDoc::from_doc(doc.clone());
    let command = session.read_commands().unwrap().remove(0);
    session
        .set_command_status(&command.id, status, None)
        .unwrap();
    if ran
        && let SessionCommandPayload::Run {
            message_id,
            request,
        } = &command.payload
    {
        session
            .push_message(&zeron_doc::SessionMessageEntry {
                id: message_id.clone(),
                role: zeron_doc::MessageRole::User,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: "t0".into(),
                    text: request.prompt.clone(),
                }],
                created_at: Utc::now().timestamp_millis(),
                device_id: "ios-live".into(),
                status: Some(zeron_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
    }
    doc.commit();
    edge.inject(
        CHAT,
        HOST,
        doc.export(loro::ExportMode::updates(&before)).unwrap(),
    );
}

fn upload_of(session: &SessionHandle) -> String {
    let reference = session.snapshot().pending[0].images[0].clone();
    zeron_client::attachments::parse_pending_ref(&reference)
        .unwrap()
        .0
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_waits_for_its_bytes_to_be_stored_and_shows_meanwhile() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[503]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;

    send_image(&session);

    // The echo is up with the image; nothing has reached the room.
    let pending = session.snapshot().pending[0].clone();
    assert_eq!(pending.visible_text, "look at this");
    assert_eq!(pending.state, zeron_client::SendState::Sending);
    assert!(room_prompts(&edge).is_empty());
    let upload = upload_of(&session);

    // The first PUT fails; the retry lands, and only then the message goes.
    wait_for("the message", || !room_prompts(&edge).is_empty()).await;
    assert_eq!(edge.stored(&upload).as_deref(), Some(IMAGE));
    assert!(edge.store_puts() >= 2, "retried");
    assert!(room_prompts(&edge)[0].contains(&format!("pending://{upload}/")));
    let command = room_commands(&edge).remove(0);
    assert_eq!(
        command.expires_at.unwrap() - command.issued_at,
        zeron_doc::CLOUD_COMMAND_TTL_MS,
        "a Cloud machine's message waits a week"
    );
    // Kept until the message has run, for a retry after it expired.
    let stash = dir.path().join(format!("uploads/{upload}.bin"));
    assert!(stash.exists());
    host_marks(&edge, zeron_doc::SessionCommandStatus::Applied, true);
    wait_for("the kept file to go once it ran", || !stash.exists()).await;
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_upload_reads_as_failed_and_a_retry_sends_it() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[413]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;

    send_image(&session);
    wait_for("the failed state", || {
        session
            .snapshot()
            .pending
            .first()
            .is_some_and(|p| p.state == zeron_client::SendState::Failed)
    })
    .await;
    assert!(room_prompts(&edge).is_empty(), "not sent, and not dropped");
    assert_eq!(session.snapshot().pending.len(), 1);

    session.retry_delivery().unwrap();
    wait_for("the retried message", || !room_prompts(&edge).is_empty()).await;
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relaunch_resumes_a_held_send() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[503, 503, 503, 503]);
    let dir = tempfile::tempdir().unwrap();
    {
        let client = phone(&edge, dir.path());
        let session = open(&client, true).await;
        send_image(&session);
        assert!(room_prompts(&edge).is_empty());
        client.shutdown();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    // Still showing, with the relaunch's fresh state.
    if room_prompts(&edge).is_empty() {
        assert_eq!(session.snapshot().pending.len(), 1);
    }
    wait_for("the resumed message", || !room_prompts(&edge).is_empty()).await;
    assert!(room_prompts(&edge)[0].contains("look at this"));
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_send_does_not_overtake_a_held_one() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[503, 503]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;

    send_image(&session);
    send_text(&session, "and also this");
    assert!(room_prompts(&edge).is_empty());
    assert_eq!(session.snapshot().pending.len(), 2);

    wait_for("both messages", || room_prompts(&edge).len() == 2).await;
    let prompts = room_prompts(&edge);
    assert!(prompts[0].contains("look at this"), "{prompts:?}");
    assert_eq!(prompts[1], "and also this");
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_upload_backs_off_instead_of_hammering_the_edge() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[503; 1000]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    // Past the startup dials, whose success cuts backoffs short on purpose.
    tokio::time::sleep(Duration::from_millis(500)).await;

    send_image(&session);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Alone, this is exactly 1: still waiting out the first backoff. Any
    // successful dial in the process cuts a backoff short on purpose, and this
    // binary's other tests dial in parallel, so how many more depends on them
    // (12 seen on CI). A hot loop against a local edge makes thousands.
    let puts = edge.store_puts();
    assert!((1..=50).contains(&puts), "{puts} uploads in 1.5 s");
    assert_eq!(
        session.snapshot().pending[0].state,
        zeron_client::SendState::Sending
    );
    assert!(room_prompts(&edge).is_empty());
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_too_old_for_the_store_gets_the_original_flow() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, peer_only()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, false).await;

    send_image(&session);
    wait_for("the message", || !room_prompts(&edge).is_empty()).await;
    assert_eq!(edge.store_puts(), 0);
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_without_the_store_falls_back_to_the_peer_path() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[404]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;

    send_image(&session);
    let upload = upload_of(&session);
    wait_for("the message", || !room_prompts(&edge).is_empty()).await;
    assert_eq!(edge.stored(&upload), None);
    // The bytes stay stashed for the escort to push once the host is up.
    let meta = std::fs::read_to_string(dir.path().join(format!("uploads/{upload}.json"))).unwrap();
    assert!(meta.contains("\"viaStore\":false"), "{meta}");
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_send_never_goes_out_and_stays_gone() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    edge.script_store(&[413]);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    send_image(&session);
    let upload = upload_of(&session);
    wait_for("the failed state", || {
        session
            .snapshot()
            .pending
            .first()
            .is_some_and(|p| p.state == zeron_client::SendState::Failed)
    })
    .await;
    let pending = session.snapshot().pending[0].clone();
    assert!(pending.removable);

    assert!(session.remove_send(&pending.message_id).unwrap());
    wait_for("the row to go", || session.snapshot().pending.is_empty()).await;
    assert!(!dir.path().join(format!("uploads/{upload}.bin")).exists());
    assert!(
        !session.remove_send(&pending.message_id).unwrap(),
        "already gone"
    );
    session.retry_delivery().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(room_prompts(&edge).is_empty(), "never sent");
    client.shutdown();

    tokio::time::sleep(Duration::from_millis(100)).await;
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    assert!(
        session.snapshot().pending.is_empty(),
        "not back after a relaunch"
    );
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_month_old_failed_send_is_still_there_to_retry_or_remove() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    let dir = tempfile::tempdir().unwrap();
    {
        let store = zeron_sync::DocsStore::open(dir.path()).unwrap();
        let payload = zeron_sync::attachment_store::HeldPayload {
            body: zeron_sync::attachment_store::HeldBody::Command {
                payload: SessionCommandPayload::Steer {
                    prompt: "from a month ago".into(),
                    message_id: Some("m-old".into()),
                },
            },
            uploads: Vec::new(),
            also_peer: false,
        };
        let month_ago = Utc::now().timestamp_millis() - 30 * 24 * 60 * 60 * 1000;
        store.hold_send("old", CHAT, &payload, month_ago).unwrap();
        store.fail_held_send("old", "the file was refused").unwrap();
    }
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    let pending = session.snapshot().pending.clone();
    assert_eq!(pending.len(), 1, "kept");
    assert_eq!(pending[0].message_id, "m-old");
    assert_eq!(pending[0].state, zeron_client::SendState::Failed);
    assert!(pending[0].removable);
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_after_expiry_uploads_the_kept_file_again() {
    let edge = MockEdge::start().await;
    let _host = host_registry(&edge, store_capable()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let session = open(&client, true).await;
    send_image(&session);
    wait_for("the message", || !room_prompts(&edge).is_empty()).await;
    assert_eq!(edge.store_puts(), 1);

    // The machine never came back in time.
    host_marks(&edge, zeron_doc::SessionCommandStatus::Expired, false);
    wait_for("the failed state", || {
        session
            .snapshot()
            .pending
            .first()
            .is_some_and(|p| p.state == zeron_client::SendState::Failed)
    })
    .await;

    session.retry_delivery().unwrap();
    wait_for("the file to go up again", || edge.store_puts() == 2).await;
    wait_for("a fresh attempt", || room_commands(&edge).len() == 2).await;
    client.shutdown();
}
