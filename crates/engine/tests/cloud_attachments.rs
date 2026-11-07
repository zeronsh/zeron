//! Attachments for chats hosted on Cloud (docs/design/cloud-attachments.md):
//! the sender stores the bytes at the edge, then the message goes out, and the
//! woken machine downloads them. A message is never lost because its machine
//! slept. Run against a small in-process stand-in for the edge's blob route.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use futures::stream::BoxStream;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, QueuedMessage, SessionCommandEntry,
    SessionCommandPayload, SessionCommandStatus,
};
use zeron_engine::{EdgeConfig, EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, Device, DoneStatus, HarnessId, HeldSendStatus, Model, ReasoningLevel, RunRequest,
    SandboxLevel, SteeringMode,
};
use zeron_rpc::methods;

const CHAT: &str = "chat-cloud-att";
const HOST: &str = "cloud-machine";
const PNG: &[u8] = b"\x89PNG not really, but bytes";
const SIXTEEN_MINUTES_MS: i64 = 16 * 60 * 1000;

// ---- the edge's blob route ----

#[derive(Default)]
struct EdgeState {
    objects: HashMap<String, Vec<u8>>,
    /// (method, path) of every attachment request, bearer checked.
    log: Vec<(String, String)>,
    unauthenticated: usize,
    put_statuses: VecDeque<u16>,
    get_statuses: VecDeque<u16>,
    corrupt_next_get: bool,
    /// An edge from before the store existed.
    no_route: bool,
}

#[derive(Clone)]
struct FakeEdge {
    url: String,
    state: Arc<Mutex<EdgeState>>,
}

impl FakeEdge {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(EdgeState::default()));
        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, shared.clone()));
            }
        });
        Self { url, state }
    }

    fn config(&self) -> EdgeConfig {
        EdgeConfig::with_static_token(self.url.clone(), "test-token")
    }

    fn store(&self, upload_id: &str, bytes: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .objects
            .insert(path_of(upload_id), bytes.to_vec());
    }

    fn stored(&self, upload_id: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .objects
            .get(&path_of(upload_id))
            .cloned()
    }

    fn fail_puts(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .put_statuses
            .extend(statuses.iter().copied());
    }

    fn fail_gets(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .get_statuses
            .extend(statuses.iter().copied());
    }

    fn count(&self, method: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|(m, _)| m == method)
            .count()
    }

    fn assert_all_authenticated(&self) {
        assert_eq!(self.state.lock().unwrap().unauthenticated, 0);
    }
}

fn path_of(upload_id: &str) -> String {
    format!("/blob/{CHAT}/attachments/{upload_id}")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn serve(mut stream: tokio::net::TcpStream, state: Arc<Mutex<EdgeState>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    let header_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_string();
    let path = first.next().unwrap_or_default().to_string();
    let mut length = 0usize;
    let mut bearer = false;
    let mut checksum = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => bearer = value.trim() == "Bearer test-token",
            "x-zeron-sha256" => checksum = Some(value.trim().to_string()),
            _ => {}
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    if path.ends_with("/nudge") {
        state.lock().unwrap().log.push(("NUDGE".into(), path.clone()));
    }
    let (status, reply, sum) = if path.contains("/attachments/") {
        let mut s = state.lock().unwrap();
        s.log.push((method.clone(), path.clone()));
        if !bearer {
            s.unauthenticated += 1;
        }
        match method.as_str() {
            _ if s.no_route => (404, Vec::new(), None),
            "PUT" => {
                if let Some(status) = s.put_statuses.pop_front() {
                    (status, Vec::new(), None)
                } else if checksum.as_deref() != Some(sha256(&body).as_str()) {
                    (503, Vec::new(), None)
                } else {
                    s.objects.insert(path, body);
                    (200, b"{}".to_vec(), None)
                }
            }
            "GET" => {
                if let Some(status) = s.get_statuses.pop_front() {
                    (status, Vec::new(), None)
                } else if let Some(object) = s.objects.get(&path).cloned() {
                    let sum = sha256(&object);
                    if std::mem::take(&mut s.corrupt_next_get) {
                        (200, b"corrupted".to_vec(), Some(sum))
                    } else {
                        (200, object, Some(sum))
                    }
                } else {
                    (404, Vec::new(), None)
                }
            }
            "DELETE" => {
                s.objects.remove(&path);
                (200, b"{}".to_vec(), None)
            }
            _ => (405, Vec::new(), None),
        }
    } else {
        (404, Vec::new(), None)
    };
    let extra = sum
        .map(|sum| format!("x-zeron-sha256: {sum}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n",
        reply.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&reply).await;
}

// ---- an agent that insists on real files ----

struct AckHarness;

#[async_trait]
impl Harness for AckHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Ack"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        assert!(
            !request.prompt.contains("pending://"),
            "prompt reached the agent with unresolved refs: {}",
            request.prompt
        );
        for path in &request.attachments {
            assert_eq!(
                std::fs::read(path).expect("attachment is a real file"),
                PNG,
                "the agent got the bytes the sender sent"
            );
        }
        let events: Vec<Result<AgentEvent, HarnessError>> = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "sess".into(),
                assistant_message_id: format!("a-{}", zeron_engine_id()),
            }),
            Ok(AgentEvent::TextDelta { text: "ack".into() }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess".into()),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

fn zeron_engine_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn assemble(dir: &Path, device_id: &str, edge: Option<&FakeEdge>) -> EngineCore {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("device-id"), device_id).unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(AckHarness));
    EngineCore::assemble(
        dir,
        Arc::new(registry),
        HarnessId::Mock,
        edge.map(FakeEdge::config),
    )
    .expect("engine assembles")
}

async fn create_chat(core: &EngineCore, host: &str) {
    zeron_rpc::memory_client(core.rpc_service())
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": host }),
        )
        .await
        .expect("createChat");
    core.workspace.rename_chat(CHAT, "Pre-titled").unwrap();
}

/// A sender whose registry says the chat lives on `host`, with this version
/// and these capabilities.
async fn sender(dir: &Path, edge: Option<&FakeEdge>, host: &str, capable: bool) -> EngineCore {
    let core = assemble(dir, "laptop", edge);
    core.workspace.upsert_device_row(&Device {
        id: host.into(),
        name: "machine".into(),
        platform: "cloud".into(),
        last_seen_at: Some(chrono::Utc::now()),
        created_at: None,
        version: Some("0.2.138".into()),
        cursor_sdk_version: None,
        capabilities: if capable {
            zeron_proto::capabilities::current()
        } else {
            vec![zeron_proto::capabilities::MESSAGE_QUEUE_V1.to_string()]
        },
    });
    create_chat(&core, host).await;
    core
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn pending_ref(upload_id: &str) -> String {
    format!("pending://{upload_id}/photo.png")
}

fn run_payload(message_id: &str, upload_id: &str) -> SessionCommandPayload {
    let reference = pending_ref(upload_id);
    let mut request = text_request("look at this");
    request.prompt = format!(
        "look at this\n\nAttached images (local files — open them to view):\n- {reference}"
    );
    request.attachments = vec![reference];
    SessionCommandPayload::Run {
        request,
        message_id: message_id.into(),
    }
}

fn text_request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "~".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}

/// What reaches a host's doc over chat2, written there directly.
fn arrive(core: &EngineCore, entry: &SessionCommandEntry) {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .queue_command(entry)
        .unwrap();
}

fn entry(id: &str, payload: SessionCommandPayload, issued_at: i64) -> SessionCommandEntry {
    SessionCommandEntry {
        id: id.into(),
        payload,
        issued_by: "laptop".into(),
        issued_at,
        based_on: None,
        expires_at: None,
        status: SessionCommandStatus::Pending,
        resolution: None,
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn commands(core: &EngineCore) -> Vec<SessionCommandEntry> {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_commands()
        .unwrap()
}

fn replies(core: &EngineCore) -> usize {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

fn user_text(core: &EngineCore) -> String {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .iter()
        .filter(|e| e.role == MessageRole::User)
        .flat_map(|e| e.parts.iter())
        .find_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The UI's first step of a send: the bytes commit to the local engine.
async fn stage(core: &EngineCore, upload_id: &str) {
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            methods::UPLOAD_CHUNK,
            serde_json::json!({ "uploadId": upload_id, "seq": 0, "data": BASE64.encode(PNG) }),
        )
        .await
        .expect("stage chunk");
    client
        .call(
            methods::UPLOAD_COMMIT,
            serde_json::json!({ "uploadId": upload_id, "fileName": "photo.png" }),
        )
        .await
        .expect("stage commit");
}

/// The UI's second step: `QueueCommand` naming what to deliver.
async fn send(core: &EngineCore, message_id: &str, upload_id: &str) -> String {
    let reply = zeron_rpc::memory_client(core.rpc_service())
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({
                "chatId": CHAT,
                "command": serde_json::to_value(run_payload(message_id, upload_id)).unwrap(),
                "transfers": [{ "uploadId": upload_id, "fileName": "photo.png" }],
            }),
        )
        .await
        .expect("queue command");
    reply["commandId"].as_str().unwrap().to_string()
}

/// A send during a live turn: `QueueMessage` naming what to deliver.
async fn queue_row_with_image(core: &EngineCore, upload_id: &str) -> String {
    let reply = zeron_rpc::memory_client(core.rpc_service())
        .call(
            methods::QUEUE_MESSAGE,
            serde_json::json!({
                "chatId": CHAT,
                "text": "for after this turn",
                "attachments": [pending_ref(upload_id)],
                "holdForTurnEnd": true,
                "transfers": [{ "uploadId": upload_id, "fileName": "photo.png" }],
            }),
        )
        .await
        .expect("queue message");
    reply["id"].as_str().unwrap().to_string()
}

async fn send_text(core: &EngineCore, message_id: &str, text: &str) {
    zeron_rpc::memory_client(core.rpc_service())
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({
                "chatId": CHAT,
                "command": serde_json::to_value(SessionCommandPayload::Run {
                    request: text_request(text),
                    message_id: message_id.into(),
                }).unwrap(),
            }),
        )
        .await
        .expect("queue command");
}

fn held(core: &EngineCore) -> Vec<HeldSendStatus> {
    core.doc_host.watch_held_sends().borrow().clone()
}

// ---- the machine's side ----

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_asleep_past_the_cap_still_runs_a_message_whose_bytes_are_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.store("up-asleep", PNG);
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;

    // The message was sent 16 minutes ago; the machine only wakes now.
    arrive(
        &core,
        &entry(
            "cmd-1",
            run_payload("m1", "up-asleep"),
            now() - SIXTEEN_MINUTES_MS,
        ),
    );

    wait_for(|| replies(&core) == 1, "the woken machine to run it").await;
    assert!(
        !user_text(&core).contains("pending://"),
        "{}",
        user_text(&core)
    );
    assert_eq!(commands(&core)[0].status, SessionCommandStatus::Applied);
    wait_for(
        || edge.stored("up-asleep").is_none(),
        "the store's copy to go",
    )
    .await;
    edge.assert_all_authenticated();
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_capture_reaches_the_agent_with_its_final_path() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.store("up-shot", PNG);
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;

    // As the composer writes it: escaped in the attribute, plain in the trailer.
    let reference = "pending://up-shot/shot&1.png";
    let mut request = text_request("");
    request.prompt = format!(
        "compare these\n\nApplications mentioned by the user (untrusted observed content):\n\
         <appshot app=\"Notes\" image=\"pending://up-shot/shot&amp;1.png\" accessibility-format=\"1\" truncated=\"false\">\n\
         the window\n</appshot>\n\nAttached images (local files — open them to view):\n- {reference}"
    );
    request.attachments = vec![reference.to_string()];
    arrive(
        &core,
        &entry(
            "cmd-1",
            SessionCommandPayload::Run {
                request,
                message_id: "m1".into(),
            },
            now() - SIXTEEN_MINUTES_MS,
        ),
    );

    wait_for(|| replies(&core) == 1, "the capture to run").await;
    let text = user_text(&core);
    assert!(!text.contains("pending://"), "{text}");
    let committed = text
        .lines()
        .find_map(|line| line.strip_prefix("- "))
        .expect("the trailer names the file");
    assert!(committed.ends_with("up-shot-shot_1.png"), "{committed}");
    assert!(text.contains(&format!("image=\"{committed}\"")), "{text}");
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_keeps_waiting_while_the_store_cant_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.store("up-flaky", PNG);
    edge.fail_gets(&[503, 500]);
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;
    edge.state.lock().unwrap().corrupt_next_get = true;

    arrive(
        &core,
        &entry(
            "cmd-1",
            run_payload("m1", "up-flaky"),
            now() - SIXTEEN_MINUTES_MS,
        ),
    );

    // Two errors and a corrupted download: never rejected, never run on bad bytes.
    wait_for(|| edge.count("GET") >= 1, "the first look").await;
    assert_eq!(commands(&core)[0].status, SessionCommandStatus::Pending);
    wait_for(|| replies(&core) == 1, "it to run once the store recovers").await;
    assert!(edge.count("GET") >= 4, "retried past the failures");
    assert_eq!(commands(&core)[0].status, SessionCommandStatus::Applied);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cap_still_applies_when_nothing_is_on_the_way() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;

    // An old sender's message: the bytes were only ever coming over a peer
    // link, and its 15 minutes are up.
    arrive(
        &core,
        &entry(
            "cmd-1",
            run_payload("m1", "up-never"),
            now() - SIXTEEN_MINUTES_MS,
        ),
    );

    wait_for(
        || commands(&core)[0].status == SessionCommandStatus::Rejected,
        "the loud failure",
    )
    .await;
    assert_eq!(
        commands(&core)[0].resolution.as_deref(),
        Some("attachments never arrived")
    );
    assert_eq!(replies(&core), 0);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_senders_peer_transfer_still_lands_on_a_cloud_machine() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;

    arrive(&core, &entry("cmd-1", run_payload("m1", "up-peer"), now()));
    wait_for(|| edge.count("GET") >= 1, "the machine to check the store").await;
    assert_eq!(replies(&core), 0, "waits, inside the cap");
    assert_eq!(commands(&core)[0].status, SessionCommandStatus::Pending);

    stage(&core, "up-peer").await;
    wait_for(|| replies(&core) == 1, "UploadCommit to re-drain").await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queue_row_on_a_cloud_machine_gets_its_bytes_from_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.store("up-row", PNG);
    let core = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&core, HOST).await;
    core.workspace.set_chat_cwd(CHAT, "~").unwrap();

    let mut row = QueuedMessage::new("row-1", "what is this", "phone");
    row.attachments = vec![pending_ref("up-row")];
    let handle = core.doc_host.open(CHAT).unwrap();
    handle.doc().push_queued(&row).unwrap();

    wait_for(|| replies(&core) == 1, "the row to send").await;
    assert!(handle.doc().read_queue().unwrap().is_empty());
    assert!(!user_text(&core).contains("pending://"));
    core.shutdown().await;
}

/// Seen on a real machine: a message queued behind a turn, the machine killed,
/// and after the wake the row sat there until someone sent another prompt.
/// Frozen is right for a computer whose user restarted the app, not for a machine
/// that restarts on every wake.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_on_a_cloud_machines_disk_goes_out_when_it_comes_back() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("machine");
    let before = assemble(&dir, HOST, None);
    create_chat(&before, HOST).await;
    before.workspace.set_chat_cwd(CHAT, "~").unwrap();
    before.shutdown().await;
    drop(before);

    let doc = zeron_doc::SessionDoc::init(CHAT).unwrap();
    doc.push_queued(&QueuedMessage::new("row-1", "and then this", "phone"))
        .unwrap();
    let store = zeron_sync::DocsStore::open(dir.join("orgs/dev-org/dev-user")).unwrap();
    store
        .save_snapshot(CHAT, &doc.export_snapshot().unwrap())
        .unwrap();
    drop(store);

    let after = assemble(&dir, HOST, None);
    let handle = after.doc_host.open(CHAT).unwrap();
    wait_for(|| replies(&after) == 1, "the row to send").await;
    assert!(handle.doc().read_queue().unwrap().is_empty());
    after.shutdown().await;
}

/// Seen on a real machine right after that fix: the row went first, the killed
/// turn read that as the chat having moved on, and it never ran again, under a
/// note saying it was resuming.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_on_a_machines_disk_waits_for_the_turn_the_kill_cut_off() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("machine");
    let before = assemble(&dir, HOST, None);
    create_chat(&before, HOST).await;
    before.workspace.set_chat_cwd(CHAT, "~").unwrap();
    before.shutdown().await;
    drop(before);

    let now = now();
    let doc = zeron_doc::SessionDoc::init(CHAT).unwrap();
    doc.push_message(&zeron_doc::SessionMessageEntry {
        id: "m1".into(),
        role: MessageRole::User,
        parts: vec![MessagePart::Text { id: "t0".into(), text: "the killed turn".into() }],
        created_at: now - 60_000,
        device_id: HOST.into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })
    .unwrap();
    doc.push_message(&zeron_doc::SessionMessageEntry {
        id: "a1".into(),
        role: MessageRole::Assistant,
        parts: vec![MessagePart::Text { id: "t0".into(), text: "partial".into() }],
        created_at: now - 30_000,
        device_id: HOST.into(),
        status: Some(MessageStatus::Streaming),
        continuation_of: None,
        duration_ms: None,
    })
    .unwrap();
    doc.push_queued(&QueuedMessage::new("row-1", "and then this", "phone")).unwrap();
    let store_dir = dir.join("orgs/dev-org/dev-user");
    let store = zeron_sync::DocsStore::open(&store_dir).unwrap();
    store.save_snapshot(CHAT, &doc.export_snapshot().unwrap()).unwrap();
    drop(store);
    zeron_engine::RunJournal::open(store_dir.join("journals"))
        .unwrap()
        .append(
            CHAT,
            &AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "hs-killed".into(),
                assistant_message_id: "a-killed".into(),
            },
        )
        .unwrap();

    let after = assemble(&dir, HOST, None);
    let handle = after.doc_host.open(CHAT).unwrap();
    wait_for(
        || handle.doc().read_queue().unwrap().is_empty() && replies(&after) >= 2,
        "the killed turn, then the row",
    )
    .await;
    let users: Vec<String> = handle
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.role == MessageRole::User)
        .map(|e| e.id)
        .collect();
    assert_eq!(users, ["m1", "row-1"], "the row never overtakes the turn");
    let answered: Vec<String> = handle
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .flat_map(|e| e.parts)
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(answered, ["ack", "ack"], "both ran");
    after.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_hosting_a_chat_never_asks_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.store("up-local", PNG);
    let core = assemble(&tmp.path().join("laptop"), "my-laptop", Some(&edge));
    create_chat(&core, "my-laptop").await;

    arrive(&core, &entry("cmd-1", run_payload("m1", "up-local"), now()));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(edge.count("GET"), 0);
    assert_eq!(replies(&core), 0);

    stage(&core, "up-local").await;
    wait_for(|| replies(&core) == 1, "the peer path").await;
    core.shutdown().await;
}

// ---- the sender's side ----

#[tokio::test(flavor = "multi_thread")]
async fn the_message_stays_off_the_chat_until_its_bytes_are_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[503]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;

    let id = send(&core, "m1", "up-1").await;

    // Held, visibly, and absent from the doc that syncs to the host.
    assert!(
        commands(&core).is_empty(),
        "nothing for the host to see yet"
    );
    assert_eq!(held(&core)[0].message_id.as_deref(), Some("m1"));
    assert!(held(&core)[0].prompt.contains("look at this"));

    // The first PUT fails; the retry lands, then the message goes out.
    wait_for(|| commands(&core).len() == 1, "the message to go out").await;
    assert_eq!(edge.stored("up-1").as_deref(), Some(PNG));
    assert!(edge.count("PUT") >= 2);
    let command = &commands(&core)[0];
    assert_eq!(command.id, id, "the id the send was told");
    assert_eq!(command.status, SessionCommandStatus::Pending);
    wait_for(|| held(&core).is_empty(), "the outbox row to go").await;
    edge.assert_all_authenticated();
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_after_a_held_one_does_not_overtake_it() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[503, 503]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;

    send(&core, "m1", "up-1").await;
    send_text(&core, "m2", "and also this").await;
    assert!(commands(&core).is_empty());
    assert_eq!(held(&core).len(), 2);
    let row = core
        .doc_host
        .queue_message(CHAT, "and a queued one", Vec::new())
        .unwrap();
    assert_eq!(held(&core).len(), 3);

    wait_for(|| commands(&core).len() == 2, "both messages to go out").await;
    let order: Vec<String> = commands(&core)
        .iter()
        .map(|c| match &c.payload {
            SessionCommandPayload::Run { message_id, .. } => message_id.clone(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(order, ["m1", "m2"]);
    let handle = core.doc_host.open(CHAT).unwrap();
    wait_for(
        || {
            handle
                .doc()
                .read_queue()
                .unwrap()
                .iter()
                .any(|q| q.id == row)
        },
        "the queued row to follow",
    )
    .await;
    core.shutdown().await;
}

/// Seen on staging: a retry from the chat woke the sleeping machine while the
/// upload was still refused, for a message it had no way to see. It billed its
/// whole idle window.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_still_refused_leaves_the_machine_asleep() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[403, 403]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    wait_for(
        || held(&core).first().is_some_and(|s| s.failure.is_some()),
        "the failure to surface",
    )
    .await;

    core.doc_host.retry_delivery(CHAT).unwrap();
    wait_for(|| edge.count("PUT") == 2, "the retry's upload").await;
    wait_for(
        || held(&core).first().is_some_and(|s| s.failure.is_some()),
        "the failure again",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(edge.count("NUDGE"), 0, "nothing there for it to find");

    core.doc_host.retry_delivery(CHAT).unwrap();
    wait_for(|| commands(&core).len() == 1, "the message").await;
    wait_for(|| edge.count("NUDGE") >= 1, "the wake").await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_upload_tells_the_user_and_a_retry_sends_it() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[413]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;

    wait_for(
        || held(&core).first().is_some_and(|s| s.failure.is_some()),
        "the failure to surface",
    )
    .await;
    assert!(
        held(&core)[0]
            .failure
            .as_deref()
            .unwrap()
            .contains("too large")
    );
    assert!(commands(&core).is_empty(), "not sent, not dropped");

    // A failed send doesn't hold up the next.
    send_text(&core, "m2", "unrelated").await;
    assert_eq!(commands(&core).len(), 1, "goes straight out");

    core.doc_host.retry_delivery(CHAT).unwrap();
    wait_for(|| commands(&core).len() == 2, "the retried message").await;
    wait_for(|| held(&core).is_empty(), "the outbox row to go").await;
    assert_eq!(edge.stored("up-1").as_deref(), Some(PNG));
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_gone_from_this_device_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    // Never staged.
    send(&core, "m1", "up-gone").await;
    wait_for(
        || held(&core).first().is_some_and(|s| s.failure.is_some()),
        "the failure",
    )
    .await;
    assert!(
        held(&core)[0]
            .failure
            .as_deref()
            .unwrap()
            .contains("no longer")
    );
    assert!(commands(&core).is_empty());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_send_resumes_after_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("laptop");
    let edge = FakeEdge::start().await;
    // The first attempt fails, and this engine stops during its backoff.
    edge.fail_puts(&[503]);
    let core = sender(&dir, Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    let id = send(&core, "m1", "up-1").await;
    wait_for(|| edge.count("PUT") == 1, "the first attempt").await;
    assert!(commands(&core).is_empty());
    core.shutdown().await;
    drop(core);
    assert_eq!(edge.stored("up-1"), None);

    // The next engine finishes the job, and still shows it meanwhile.
    let core = assemble(&dir, "laptop", Some(&edge));
    wait_for(|| commands(&core).len() == 1, "the resumed send").await;
    assert_eq!(commands(&core)[0].id, id);
    assert_eq!(edge.stored("up-1").as_deref(), Some(PNG));
    wait_for(|| held(&core).is_empty(), "the outbox row to go").await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_message_goes_through_the_store_again_on_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    // A message the machine gave up on, say after the store's copy expired.
    let mut rejected = entry("cmd-old", run_payload("m1", "up-1"), now());
    rejected.status = SessionCommandStatus::Rejected;
    rejected.resolution = Some("attachments never arrived".into());
    arrive(&core, &rejected);

    core.doc_host.retry_delivery(CHAT).unwrap();

    wait_for(|| edge.stored("up-1").is_some(), "the bytes to go up again").await;
    wait_for(|| commands(&core).len() == 2, "a fresh attempt").await;
    assert_eq!(commands(&core)[1].status, SessionCommandStatus::Pending);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_queued_during_a_turn_waits_for_its_bytes_too() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[503]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;

    let id = queue_row_with_image(&core, "up-1").await;

    let handle = core.doc_host.open(CHAT).unwrap();
    assert!(handle.doc().read_queue().unwrap().is_empty(), "off the chat");
    let status = held(&core).remove(0);
    assert!(status.queue_row);
    assert_eq!(status.message_id.as_deref(), Some(id.as_str()));
    assert!(status.prompt.contains(&pending_ref("up-1")), "shows its image");

    wait_for(
        || !handle.doc().read_queue().unwrap().is_empty(),
        "the row to go out",
    )
    .await;
    assert_eq!(edge.stored("up-1").as_deref(), Some(PNG));
    let row = handle.doc().read_queue().unwrap().remove(0);
    assert_eq!(row.id, id);
    assert_eq!(row.attachments, [pending_ref("up-1")]);
    wait_for(|| held(&core).is_empty(), "the outbox row to go").await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_row_says_so_and_a_retry_sends_it() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.fail_puts(&[413]);
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    queue_row_with_image(&core, "up-1").await;
    wait_for(
        || held(&core).first().is_some_and(|s| s.failure.is_some()),
        "the failure",
    )
    .await;

    core.doc_host.retry_delivery(CHAT).unwrap();
    let handle = core.doc_host.open(CHAT).unwrap();
    wait_for(
        || !handle.doc().read_queue().unwrap().is_empty(),
        "the retried row",
    )
    .await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_for_a_computer_goes_straight_into_the_chat() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = sender(&tmp.path().join("laptop"), Some(&edge), "desktop-at-home", true).await;
    stage(&core, "up-1").await;
    queue_row_with_image(&core, "up-1").await;
    let handle = core.doc_host.open(CHAT).unwrap();
    assert_eq!(handle.doc().read_queue().unwrap().len(), 1);
    assert!(held(&core).is_empty());
    assert_eq!(edge.count("PUT"), 0);
    core.shutdown().await;
}

// ---- end to end, with the sender gone ----

#[tokio::test(flavor = "multi_thread")]
async fn the_machine_runs_a_message_after_its_sender_went_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let laptop = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&laptop, "up-1").await;
    send(&laptop, "m1", "up-1").await;
    wait_for(|| commands(&laptop).len() == 1, "the send").await;

    // The row is all that travels; the laptop is gone before the machine wakes.
    let row = commands(&laptop).remove(0);
    laptop.shutdown().await;
    drop(laptop);

    let machine = assemble(&tmp.path().join("machine"), HOST, Some(&edge));
    create_chat(&machine, HOST).await;
    arrive(&machine, &row);

    wait_for(|| replies(&machine) == 1, "the machine to run it").await;
    wait_for(|| edge.stored("up-1").is_none(), "the store's copy to go").await;
    assert!(
        user_text(&machine).contains("up-1"),
        "{}",
        user_text(&machine)
    );
    machine.shutdown().await;
}

// ---- who keeps the peer path ----

#[tokio::test(flavor = "multi_thread")]
async fn a_host_too_old_for_the_store_gets_the_original_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, false).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    // Straight into the chat, before any bytes move; nothing at the edge.
    assert_eq!(commands(&core).len(), 1);
    assert!(held(&core).is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(edge.count("PUT"), 0);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_on_another_computer_keeps_the_peer_path() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    let core = sender(
        &tmp.path().join("laptop"),
        Some(&edge),
        "desktop-at-home",
        true,
    )
    .await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    assert_eq!(commands(&core).len(), 1);
    assert!(held(&core).is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(edge.count("PUT"), 0);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edge_without_the_store_falls_back_to_the_peer_path() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    edge.state.lock().unwrap().no_route = true;
    let core = sender(&tmp.path().join("laptop"), Some(&edge), HOST, true).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    wait_for(
        || commands(&core).len() == 1,
        "the message to go out anyway",
    )
    .await;
    wait_for(|| held(&core).is_empty(), "the outbox row to go").await;
    assert_eq!(edge.count("PUT"), 1);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_has_not_reported_in_gets_the_store_and_the_peer_path() {
    let tmp = tempfile::tempdir().unwrap();
    let edge = FakeEdge::start().await;
    // No device row: a machine still being set up.
    let core = assemble(&tmp.path().join("laptop"), "laptop", Some(&edge));
    create_chat(&core, HOST).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    wait_for(|| commands(&core).len() == 1, "the message to go out").await;
    assert_eq!(edge.stored("up-1").as_deref(), Some(PNG));
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_edge_means_the_peer_path() {
    let tmp = tempfile::tempdir().unwrap();
    let core = sender(&tmp.path().join("laptop"), None, HOST, true).await;
    stage(&core, "up-1").await;
    send(&core, "m1", "up-1").await;
    assert_eq!(commands(&core).len(), 1);
    assert!(held(&core).is_empty());
    core.shutdown().await;
}
