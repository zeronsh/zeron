//! Live mode against an in-process edge (tests/support/mock_edge.rs): the
//! registry through zeron-sync's mock server (the production merge fn) and a
//! chat2 room speaking the real frame protocol. The test plays the host
//! engine: it seeds registry rows through its own `RegistryClient` and
//! answers commands by injecting Loro updates into the room.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use loro::{ExportMode, LoroDoc};
use support::mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{
    ChatIndicator, Client, ClientConfig, ConnectivityState, Credentials, MessageRole, SendOutcome,
    SendRequest,
};
use zeron_doc::{
    MessagePart, RegistryDoc, SessionCommandPayload, SessionCommandStatus, SessionDoc,
    SessionMessageEntry,
};
use zeron_proto::{Chat, ChatConfig, Device, HarnessId, SandboxLevel, Space};
use zeron_sync::RegistryClient;

const HOST: &str = "dev-mac";
const CHAT: &str = "chat-live";
const SPACE: &str = "space-live";

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn host_rows(now: chrono::DateTime<Utc>) -> (Device, Space, Chat) {
    let device = Device {
        id: HOST.into(),
        name: "MacBook Pro".into(),
        platform: "macos".into(),
        last_seen_at: Some(now),
        created_at: Some(now),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        cursor_sdk_version: None,
        capabilities: zeron_client::rpc::capability::ALL_QUEUE
            .iter()
            .map(|c| (*c).to_owned())
            .collect(),
    };
    let space = Space {
        id: SPACE.into(),
        device_id: HOST.into(),
        path: "/Users/dev/live".into(),
        name: Some("Live".into()),
        git_detected: true,
        git_checked_at: None,
        checkout_id: None,
        created_at: now,
    };
    let chat = Chat {
        id: CHAT.into(),
        device_id: HOST.into(),
        title: Some("Live chat".into()),
        archived: false,
        cwd: Some("/Users/dev/live".into()),
        branch: Some("main".into()),
        checkout_id: None,
        source_context: Some(zeron_proto::ConversationSourceContext {
            checkout_id: "co-live".into(),
            repo_root: "/Users/dev/live".into(),
            cwd: "/Users/dev/live".into(),
            branch: "feature/live".into(),
            head_sha: None,
            observed_at: now,
        }),
        config: Some(ChatConfig {
            harness: HarnessId::ClaudeCode,
            model: Some("claude-opus-5".into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
        }),
        last_message_preview: None,
        last_message_at: Some(now),
        created_at: now,
        harness_session_id: None,
        harness_session_cwd: None,
        space_id: Some(SPACE.into()),
        last_seen_at: Some(now),
        room_gen: Some(2),
        parent_chat_id: None,
    };
    (device, space, chat)
}

/// The "host engine" side of the registry.
struct HostRegistry {
    doc: Arc<Mutex<RegistryDoc>>,
    client: RegistryClient,
}

impl HostRegistry {
    async fn start(edge: &MockEdge) -> Self {
        let doc = Arc::new(Mutex::new(RegistryDoc::new(HOST)));
        {
            let (device, space, chat) = host_rows(Utc::now());
            let mut doc = doc.lock().unwrap();
            doc.upsert_device(&device).unwrap();
            doc.upsert_space(&space).unwrap();
            doc.upsert_chat(&chat).unwrap();
        }
        let client = RegistryClient::connect(&edge.registry.url(), doc.clone(), HOST)
            .await
            .unwrap();
        client.nudge();
        client.set_presence(zeron_client_now());
        Self { doc, client }
    }
}

fn zeron_client_now() -> i64 {
    Utc::now().timestamp_millis()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_mirrors_writes_both_ways_and_persists() {
    let edge = MockEdge::start().await;
    let host = HostRegistry::start(&edge).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());

    tokio::task::spawn_blocking({
        let client = client.clone();
        move || {
            wait_for("registry sync", Duration::from_secs(10), || {
                let ws = client.workspace();
                ws.synced && ws.pins_ready && ws.session(CHAT).is_some()
            })
        }
    })
    .await
    .unwrap();
    let ws = client.workspace();
    let row = ws.session(CHAT).unwrap();
    assert_eq!(row.project.as_ref().unwrap().name, "Live");
    assert_eq!(row.room_gen, 2);
    assert_eq!(ws.front.recent[0].id, CHAT);
    assert!(ws.device(HOST).unwrap().is_execution_host);
    // The host's presence beat reached the phone.
    tokio::task::spawn_blocking({
        let client = client.clone();
        move || {
            wait_for("host online", Duration::from_secs(10), || {
                client.workspace().device(HOST).is_some_and(|d| d.online)
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(client.connectivity().state, ConnectivityState::Connected);

    // Phone writes reach the host.
    client.pin_session(CHAT).unwrap();
    client.rename_session(CHAT, "Renamed on phone").unwrap();
    let doc = host.doc.clone();
    tokio::task::spawn_blocking(move || {
        wait_for(
            "host sees the phone's writes",
            Duration::from_secs(10),
            || {
                let doc = doc.lock().unwrap();
                doc.chat(CHAT).unwrap().and_then(|c| c.title).as_deref() == Some("Renamed on phone")
                    && doc
                        .sidebar_preferences()
                        .is_some_and(|p| p.pinned_session_ids == vec![CHAT.to_owned()])
            },
        )
    })
    .await
    .unwrap();

    // Host writes reach the phone.
    {
        let mut doc = host.doc.lock().unwrap();
        doc.set_chat_archived(CHAT, true).unwrap();
    }
    host.client.nudge();
    tokio::task::spawn_blocking({
        let client = client.clone();
        move || {
            wait_for(
                "phone sees the host's archive",
                Duration::from_secs(10),
                || client.workspace().archived.iter().any(|r| r.id == CHAT),
            )
        }
    })
    .await
    .unwrap();

    // Restart offline: the cached replica renders instantly.
    client.on_background();
    client.shutdown();
    drop(client);
    let mut config = ClientConfig::new("http://127.0.0.1:9", dir.path());
    config.device_id = "ios-live".into();
    let offline = Client::new(
        config,
        Credentials::Dev {
            user_id: "user-1".into(),
            org_id: "org-1".into(),
        },
        Arc::new(NullListener),
    )
    .unwrap();
    let ws = offline.workspace();
    let row = ws.session(CHAT).expect("restored from disk");
    assert!(row.archived && row.title == "Renamed on phone");
    assert!(ws.pins_ready, "sidebar prefs persisted");
    offline.shutdown();
}

/// Host side of one turn: adopt the phone's run command, write the user
/// entry under the client-minted id and a finished assistant reply.
fn host_answers(edge: &MockEdge, host_doc: &LoroDoc) -> Option<String> {
    for row in edge.rows(CHAT) {
        let _ = host_doc.import(&row.bytes);
    }
    let session = SessionDoc::from_doc(host_doc.clone());
    let commands = session.read_commands().ok()?;
    let (command_id, message_id, prompt) = commands.iter().find_map(|c| match &c.payload {
        SessionCommandPayload::Run {
            request,
            message_id,
        } if c.status == SessionCommandStatus::Pending => {
            Some((c.id.clone(), message_id.clone(), request.prompt.clone()))
        }
        _ => None,
    })?;
    let before = host_doc.oplog_vv();
    session
        .set_command_status(&command_id, SessionCommandStatus::Applied, None)
        .unwrap();
    let now = zeron_client_now();
    let entry = |id: &str, role, device: &str, text: &str| SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: text.into(),
        }],
        created_at: now,
        device_id: device.into(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    };
    session
        .push_message(&entry(&message_id, MessageRole::User, "ios-live", &prompt))
        .unwrap();
    session
        .push_message(&entry(
            "reply-1",
            MessageRole::Assistant,
            HOST,
            "Hello from the host.",
        ))
        .unwrap();
    host_doc.commit();
    let update = host_doc.export(ExportMode::updates(&before)).unwrap();
    edge.inject(CHAT, HOST, update);
    Some(message_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_send_round_trips_through_the_chat_room() {
    let edge = MockEdge::start().await;
    let _host = HostRegistry::start(&edge).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    tokio::task::spawn_blocking({
        let client = client.clone();
        move || {
            wait_for("chat row", Duration::from_secs(10), || {
                client.workspace().device(HOST).is_some_and(|d| d.online)
                    && client.workspace().session(CHAT).is_some()
            })
        }
    })
    .await
    .unwrap();

    let session = client.open_session(CHAT).unwrap();
    session.set_view_attached(true);
    tokio::task::spawn_blocking({
        let session = session.clone();
        move || {
            wait_for("hydrated from the room", Duration::from_secs(10), || {
                session.snapshot().hydrated
            })
        }
    })
    .await
    .unwrap();

    let SendOutcome::Started { message_id } = session.send(SendRequest::text("hi host")).unwrap()
    else {
        panic!("idle chat starts a turn");
    };
    assert_eq!(session.snapshot().pending.len(), 1);
    assert!(session.snapshot().working, "in flight to a live host");

    // The command row reaches the room; the host answers.
    let host_doc = LoroDoc::new();
    let start = Instant::now();
    let answered = loop {
        if let Some(id) = host_answers(&edge, &host_doc) {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "command never reached the room"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(answered, message_id);

    tokio::task::spawn_blocking({
        let session = session.clone();
        let message_id = message_id.clone();
        move || {
            wait_for("echo adopted + reply", Duration::from_secs(10), || {
                let snap = session.snapshot();
                snap.pending.is_empty()
                    && snap.entry(&message_id).is_some_and(|e| e.echo.is_none())
                    && snap.entry("reply-1").is_some()
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(session.composer().send_state, None);
    let composer = session.composer();
    assert!(composer.room.connected);
    assert_eq!(composer.live.indicator, ChatIndicator::Idle);

    // Reopen offline: the persisted snapshot renders instantly.
    client.on_background();
    client.shutdown();
    drop(session);
    drop(client);
    let mut config = ClientConfig::new("http://127.0.0.1:9", dir.path());
    config.device_id = "ios-live".into();
    let offline = Client::new(
        config,
        Credentials::Dev {
            user_id: "user-1".into(),
            org_id: "org-1".into(),
        },
        Arc::new(NullListener),
    )
    .unwrap();
    let reopened = offline.open_session(CHAT).unwrap();
    let snap = reopened.snapshot();
    assert!(snap.hydrated);
    assert!(
        snap.entry("reply-1").is_some(),
        "transcript restored from disk"
    );
    assert!(snap.entry(&message_id).is_some_and(|e| e.echo.is_none()));
    offline.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_survive_a_room_outage_and_deliver_on_rejoin() {
    let edge = MockEdge::start().await;
    let _host = HostRegistry::start(&edge).await;
    edge.refuse_chat_joins(true);
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    tokio::task::spawn_blocking({
        let client = client.clone();
        move || {
            wait_for("chat row", Duration::from_secs(10), || {
                client.workspace().session(CHAT).is_some()
            })
        }
    })
    .await
    .unwrap();
    let session = client.open_session(CHAT).unwrap();
    session
        .send(SendRequest::text("while the room is down"))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(edge.rows(CHAT).is_empty(), "room refused");
    assert_eq!(session.snapshot().pending.len(), 1, "durable echo stays");

    // The room comes back: the join loop retries (never one-shot) and the
    // outbox delivers.
    edge.refuse_chat_joins(false);
    let start = Instant::now();
    while edge.rows(CHAT).is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "outbox never delivered"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let host_doc = LoroDoc::new();
    let start = Instant::now();
    while host_answers(&edge, &host_doc).is_none() {
        assert!(start.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::task::spawn_blocking({
        let session = session.clone();
        move || {
            wait_for("adopted after rejoin", Duration::from_secs(10), || {
                session.snapshot().pending.is_empty()
            })
        }
    })
    .await
    .unwrap();
    client.shutdown();
}

// ── device relay ─────────────────────────────────────────────────────────

fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn unb64(input: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            _ => 63,
        }
    };
    let (mut out, mut buf, mut bits) = (Vec::new(), 0u32, 0u32);
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        buf = (buf << 6) | value(c);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

/// The host engine's relay surface (shapes from crates/engine/src/rpc.rs).
#[derive(Default)]
struct HostService {
    chunks: Mutex<std::collections::BTreeMap<(String, u64), String>>,
    committed: Mutex<Vec<(String, String, Vec<u8>)>>,
    spaces: Mutex<Vec<String>>,
}

const HOST_IMAGE: &str = "/Users/dev/.zeron/uploads/host.png";

fn host_image() -> Vec<u8> {
    (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect()
}

#[async_trait::async_trait]
impl zeron_rpc::RpcService for HostService {
    async fn handle(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        use serde_json::json;
        use zeron_rpc::{RpcReply, methods as m};
        let value = match method {
            m::LIST_HARNESSES => json!([
                {"id": "claude-code", "name": "Claude Code", "supportsSteering": true,
                 "steeringMode": "step-boundary", "reasoningLevels": ["high"], "installed": true,
                 "canInstall": false, "enabled": true},
                {"id": "mock", "name": "Mock", "supportsSteering": false,
                 "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": true, "canInstall": false},
            ]),
            m::LIST_MODELS => json!([
                {"id": "claude-opus-5", "label": "claude-opus-5", "reasoningLevels": ["high"], "options": []},
                {"id": "claude-opus-5[1m]", "label": "Opus 5 (1M)", "reasoningLevels": ["high"], "options": []},
            ]),
            m::LIST_REFS => {
                json!([{ "name": "main", "current": true }, { "name": "feature/live", "current": false, "worktreePath": "/wt/live" }])
            }
            m::LIST_FOLDERS => {
                json!({ "path": params["path"].as_str().unwrap_or("/Users/dev"), "entries": [{ "name": "live", "isDir": true, "isRepo": true }], "truncated": false })
            }
            m::UPLOAD_CHUNK => {
                let id = params["uploadId"].as_str().unwrap_or_default().to_owned();
                let seq = params["seq"].as_u64().unwrap_or(0);
                let data = params["data"].as_str().unwrap_or_default().to_owned();
                self.chunks.lock().unwrap().insert((id, seq), data);
                json!({ "ok": true })
            }
            m::UPLOAD_COMMIT => {
                let id = params["uploadId"].as_str().unwrap_or_default().to_owned();
                let name = params["fileName"].as_str().unwrap_or_default().to_owned();
                let data: String = self
                    .chunks
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|((u, _), _)| *u == id)
                    .map(|(_, d)| d.clone())
                    .collect();
                self.committed
                    .lock()
                    .unwrap()
                    .push((id.clone(), name.clone(), unb64(&data)));
                json!({ "path": format!("/Users/dev/.zeron/uploads/{}-{name}", &id[..8]) })
            }
            m::READ_ATTACHMENT_CHUNK => {
                let offset = params["offset"].as_u64().unwrap_or(0) as usize;
                let bytes = host_image();
                let end = (offset + 45_000).min(bytes.len());
                json!({ "name": "host.png", "mimeType": "image/png", "data": b64(&bytes[offset..end]),
                        "nextOffset": end, "done": end == bytes.len() })
            }
            m::MUTATE => {
                self.spaces
                    .lock()
                    .unwrap()
                    .push(params["spaceId"].as_str().unwrap_or_default().to_owned());
                json!({ "ok": true })
            }
            m::WATCH_CHECKOUT_CHANGE_REQUEST => {
                let item = json!({
                    "checkoutId": "co-live", "deviceId": HOST, "cwd": params["cwd"], "branch": "feature/live",
                    "changeRequest": { "provider": "github", "number": 7, "title": "Live PR",
                        "url": "https://github.com/x/y/pull/7", "state": "open",
                        "baseRef": "main", "headRef": "feature/live" },
                    "updatedAt": Utc::now(),
                });
                let stream = futures::StreamExt::chain(
                    futures::stream::once(async move { item }),
                    futures::stream::pending(),
                );
                return Ok(RpcReply::Stream(Box::pin(stream)));
            }
            other => return Err(zeron_rpc::RpcError::UnknownMethod(other.to_owned())),
        };
        Ok(RpcReply::Value(value))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_rpcs_ride_the_device_relay() {
    let edge = MockEdge::start().await;
    let _host = HostRegistry::start(&edge).await;
    let service = Arc::new(HostService::default());
    let _relay = zeron_rpc::HostRelay::spawn(
        zeron_rpc::HostRelayConfig::new(
            edge.edge_url(),
            HOST,
            Arc::new(zeron_rpc::StaticToken("t".into())),
        ),
        service.clone(),
        Arc::new(|_| true),
    );
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let start = Instant::now();
    while !(edge.relay_host_connected(HOST)
        && client.workspace().device(HOST).is_some_and(|d| d.online))
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "host never reachable"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let harnesses = zeron_client::runtime::run({
        let client = client.clone();
        async move { Ok(client.list_harnesses(HOST).await) }
    })
    .await
    .unwrap();
    assert_eq!(
        harnesses.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        ["claude-code"]
    );
    assert_eq!(harnesses[0].mid_turn_steering(), Some(true));

    let c = client.clone();
    let models =
        zeron_client::runtime::run(async move { Ok(c.list_models(HOST, "claude-code").await) })
            .await
            .unwrap();
    assert_eq!(models.len(), 1, "[1m] variant folded: {models:?}");
    assert_eq!(models[0].label, "Opus 5");

    let c = client.clone();
    let refs =
        zeron_client::runtime::run(async move { c.list_refs(HOST, "/Users/dev/live").await })
            .await
            .unwrap();
    assert_eq!(refs[1].worktree_path.as_deref(), Some("/wt/live"));

    let c = client.clone();
    let folders = zeron_client::runtime::run(async move { c.list_folders(HOST, None).await })
        .await
        .unwrap();
    assert!(folders.entries[0].is_repo);

    // Chunked upload (> one 510KB slice) with progress.
    let data: Vec<u8> = (0..1_200_000u32).map(|i| (i % 253) as u8).collect();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let c = client.clone();
    let upload = data.clone();
    let path = zeron_client::runtime::run(async move {
        c.upload_attachment(
            HOST,
            "photo 1.png",
            upload,
            Some(Arc::new(move |f| seen.lock().unwrap().push(f))),
        )
        .await
    })
    .await
    .unwrap();
    assert!(path.ends_with("-photo_1.png"), "{path}");
    let committed = service.committed.lock().unwrap().last().cloned().unwrap();
    assert_eq!(committed.1, "photo_1.png");
    assert_eq!(committed.2, data);
    assert_eq!(progress.lock().unwrap().last().copied(), Some(1.0));

    // Chunked attachment read.
    let c = client.clone();
    let image =
        zeron_client::runtime::run(async move { c.read_attachment(HOST, HOST_IMAGE).await })
            .await
            .unwrap();
    assert_eq!(*image, host_image());

    // PR status streams in for the chat's checkout.
    let start = Instant::now();
    loop {
        let pr = client
            .workspace()
            .session(CHAT)
            .and_then(|r| r.pull_request.clone());
        if let Some(pr) = pr {
            assert_eq!(pr.number, 7);
            assert_eq!(client.workspace().pull_requests.open[0].id, CHAT);
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "PR status never arrived"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A send with an image: `pending://` ref in the command, bytes escorted
    // to the host under the same upload id + name.
    let session = client.open_session(CHAT).unwrap();
    let start = Instant::now();
    while !session.composer().host.capabilities.queued_attachments {
        assert!(start.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    session
        .send(SendRequest {
            text: "look at this".into(),
            attachments: vec![zeron_client::OutgoingAttachment {
                name: "shot.png".into(),
                mime_type: "image/png".into(),
                data: vec![1, 2, 3, 4, 5],
            }],
            worktree: None,
            busy: Default::default(),
        })
        .unwrap();
    let pending = session.snapshot().pending[0].clone();
    assert_eq!(pending.visible_text, "look at this");
    let reference = pending.images[0].clone();
    let (upload_id, name) = zeron_client::attachments::parse_pending_ref(&reference).unwrap();
    let (upload_id, name) = (upload_id.to_owned(), name.to_owned());
    let start = Instant::now();
    loop {
        let found = service
            .committed
            .lock()
            .unwrap()
            .iter()
            .any(|(id, n, bytes)| *id == upload_id && *n == name && bytes == &vec![1, 2, 3, 4, 5]);
        if found {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "escort never delivered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // New project: asked of the owning host (Mutate createSpace).
    let c = client.clone();
    let space_id =
        zeron_client::runtime::run(
            async move { c.create_project(HOST, "/Users/dev/new", true).await },
        )
        .await
        .unwrap();
    assert!(service.spaces.lock().unwrap().contains(&space_id));
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_born_sessions_reach_the_host_before_their_first_command() {
    let edge = MockEdge::start().await;
    let host = HostRegistry::start(&edge).await;
    let dir = tempfile::tempdir().unwrap();
    let client = phone(&edge, dir.path());
    let start = Instant::now();
    while !client.workspace().synced || client.workspace().project(SPACE).is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "sync");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let chat_id = client
        .create_session(zeron_client::NewSession {
            target: zeron_client::SessionTarget::Project {
                space_id: SPACE.into(),
            },
            config: None,
            branch: None,
            cwd: None,
            title: None,
        })
        .unwrap();
    let session = client.open_session(&chat_id).unwrap();
    session.send(SendRequest::text("first words")).unwrap();
    // The host learns about the chat (born on chat2) …
    let start = Instant::now();
    loop {
        let row = host.doc.lock().unwrap().chat(&chat_id).unwrap();
        if let Some(row) = row {
            assert_eq!(row.room_gen, Some(2));
            assert_eq!(row.device_id, HOST);
            assert_eq!(row.cwd.as_deref(), Some("/Users/dev/live"));
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "chat row never reached the host"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // … and the first command lands in its chat2 room.
    let start = Instant::now();
    while edge.rows(&chat_id).is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "command never reached the room"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    client.shutdown();
}
