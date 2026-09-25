//! Peer-relay delivery fallback (durable-by-design §Phase 3): engine A queues
//! a command for a chat hosted on engine B, A's chat2 rows can NEVER reach an
//! edge (none is configured — the 03:45 incident shape, rows dark while the
//! peer link lives), so after the rows grace A relay-forwards the entry over
//! the device-room link. B claims the client-minted id in its processed
//! ledger before executing — so when the doc row later "arrives" (simulated
//! by writing the same entry into B's doc), the drain dedupes it to a no-op:
//! exactly-once across both roads.

// tungstenite's `accept_hdr_async` callback signature fixes the Err type as a
// full `Response` — its size is not ours to shrink.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, Device, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_rpc::{
    DeviceFrameHeader, LinkCache, LinkCacheConfig, StaticToken, decode_device_frame,
    encode_device_frame, methods,
};

const CHAT: &str = "chat-relay-fallback";

// Minimal in-memory device room (route-only subset of the DO semantics) —
// same shape as device_routing.rs.
#[derive(Default)]
struct RelayState {
    host: Option<mpsc::UnboundedSender<Vec<u8>>>,
    clients: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
}

async fn fake_device_room() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let url = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let state = Arc::new(Mutex::new(RelayState::default()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |req: &WsRequest, res: WsResponse| {
                        uri = req.uri().to_string();
                        Ok(res)
                    },
                )
                .await
                else {
                    return;
                };
                let query = uri.split_once('?').map(|(_, q)| q).unwrap_or("");
                let is_host = query.contains("role=host");
                let conn_id = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("connId="))
                    .unwrap_or("anon")
                    .to_string();
                let (mut sink, mut ws_stream) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                {
                    let mut st = state.lock().expect("lock");
                    if is_host {
                        st.host = Some(tx);
                    } else {
                        st.clients.insert(conn_id.clone(), tx);
                    }
                }
                let writer = tokio::spawn(async move {
                    while let Some(bytes) = rx.recv().await {
                        if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = ws_stream.next().await {
                    let WsMessage::Binary(bytes) = message else {
                        continue;
                    };
                    let Ok((header, payload)) = decode_device_frame(&bytes) else {
                        break;
                    };
                    let st = state.lock().expect("lock");
                    if is_host {
                        let Some(to) = header.to else { continue };
                        if let Some(client) = st.clients.get(&to) {
                            let stripped = DeviceFrameHeader::new(header.s, header.k);
                            let _ = client
                                .send(encode_device_frame(&stripped, &payload).expect("encode"));
                        }
                    } else if let Some(host) = &st.host {
                        let mut routed = DeviceFrameHeader::new(header.s, header.k);
                        routed.from = Some(conn_id.clone());
                        let _ = host.send(encode_device_frame(&routed, &payload).expect("encode"));
                    }
                }
                writer.abort();
            });
        }
    });
    (url, task)
}

struct InstantHarness;

#[async_trait]
impl Harness for InstantHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Instant"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        assert!(!request.prompt.contains("pending://"));
        if !request.attachments.is_empty() {
            assert_eq!(request.attachments.len(), attachment_fixtures().len());
        }
        for (path, (_, expected)) in request.attachments.iter().zip(attachment_fixtures()) {
            assert_eq!(
                std::fs::read(path).expect("host file exists before run"),
                expected
            );
            assert!(
                request.prompt.contains(path),
                "prompt names the host's file"
            );
        }
        Ok(futures::stream::iter([
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "instant-1".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "hs-relay".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: "relayed reply".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("hs-relay".into()),
            }),
        ])
        .boxed())
    }
}

fn registry() -> Arc<HarnessRegistry> {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(InstantHarness));
    Arc::new(registry)
}

fn assemble(dir: &std::path::Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("create data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("write device id");
    EngineCore::assemble(dir, registry(), HarnessId::Mock, None).expect("engine assembles")
}

fn complete_assistant_count(core: &EngineCore) -> usize {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_dark_command_delivers_over_the_peer_relay_exactly_once() {
    relay_delivery_roundtrip(Vec::new()).await;
}

fn attachment_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "notes with spaces.md",
            b"# Markdown\nCross-device attachment.\n".to_vec(),
        ),
        ("data.json", br#"{"attached":true}"#.to_vec()),
        ("document.pdf", b"%PDF-1.4\nfixture".to_vec()),
        (
            "archive.bin",
            (0..1_100_003).map(|i| (i % 251) as u8).collect(),
        ),
        ("LICENSE", b"No extension".to_vec()),
        ("empty.txt", Vec::new()),
        ("image.png", b"image transfer fixture".to_vec()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arbitrary_files_transfer_to_remote_host_before_run_and_read_back() {
    relay_delivery_roundtrip(attachment_fixtures()).await;
}

async fn relay_delivery_roundtrip(files: Vec<(&str, Vec<u8>)>) {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");

    // Engine B hosts its device room on the fake relay.
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    let _host = core_b.start_host_relay(&relay_url);

    // Engine A dials peers through the same relay — and has NO edge, so its
    // chat2 rows can never flush (the rows-dark half of the incident shape).
    let core_a = assemble(&dirs.path().join("a"), "device-a");
    let mut link_config =
        LinkCacheConfig::new(relay_url.clone(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));

    // A knows the chat is hosted on B (local registry writes), and knows B's
    // stamped version passes the relay gate.
    core_a.workspace.upsert_device_row(&Device {
        id: "device-b".into(),
        name: "b".into(),
        platform: "linux".into(),
        last_seen_at: Some(chrono::Utc::now()),
        created_at: None,
        version: Some("0.2.12".into()),
        cursor_sdk_version: None,
        capabilities: zeron_proto::capabilities::current(),
    });
    let client_a = zeron_rpc::memory_client(core_a.rpc_service());
    client_a
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": "device-b" }),
        )
        .await
        .expect("createChat on A");
    core_b
        .workspace
        .rename_chat(CHAT, "Pre-titled")
        .expect("pre-title on B (no auto-title harness run)");

    let mut refs = Vec::new();
    let mut transfers = Vec::new();
    for (index, (name, bytes)) in files.iter().enumerate() {
        let upload_id = format!("{index:08}-attachment");
        client_a
            .call(
                methods::UPLOAD_CHUNK,
                serde_json::json!({
                    "uploadId": upload_id, "seq": 0, "data": BASE64.encode(bytes),
                }),
            )
            .await
            .expect("stage on sender");
        client_a
            .call(
                methods::UPLOAD_COMMIT,
                serde_json::json!({
                    "uploadId": upload_id, "fileName": name,
                }),
            )
            .await
            .expect("commit on sender");
        refs.push(zeron_engine::uploads::pending_ref(&upload_id, name));
        transfers.push(serde_json::json!({"uploadId": upload_id, "fileName": name}));
    }
    let prompt = if refs.is_empty() {
        "over the relay".into()
    } else {
        format!(
            "Read the files\n\nAttached files (local files — open them to view):\n{}",
            refs.iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    // The send: a durable local write on A. Rows go nowhere; the escort's
    // grace elapses; the entry crosses the peer link instead.
    let command = serde_json::to_value(SessionCommandPayload::Run {
        request: RunRequest {
            prompt,
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "~".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: refs.clone(),
            worktree: None,
            resume: None,
        },
        message_id: "msg-relay-1".into(),
    })
    .expect("command json");
    client_a
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": CHAT, "command": command, "transfers": transfers }),
        )
        .await
        .expect("queue on A");

    // B executes it — allow the 10s rows grace plus relay dial time.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if complete_assistant_count(&core_b) == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "relayed command never executed on B"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let entries = core_b
        .doc_host
        .open(CHAT)
        .expect("open on B")
        .doc()
        .read_entries()
        .expect("read entries");
    assert!(
        entries
            .iter()
            .any(|e| e.id == "msg-relay-1" && e.role == MessageRole::User),
        "B persisted the user message under the client-minted id"
    );

    for ((_, expected), reference) in files.iter().zip(&refs) {
        let host_path = core_b
            .uploads
            .resolve_pending(reference)
            .expect("file delivered to host");
        let sender_path = core_a
            .uploads
            .resolve_pending(reference)
            .expect("sender keeps original");
        assert_ne!(host_path, sender_path, "separate device stores");
        assert_eq!(std::fs::read(&host_path).unwrap(), *expected);
        // Read back through A's routed RPC, as another device's queue editor does.
        let mut offset = 0;
        let mut actual = Vec::new();
        loop {
            let chunk = client_a
                .call(
                    methods::READ_ATTACHMENT_CHUNK,
                    serde_json::json!({
                        "targetDeviceId": "device-b", "path": host_path, "offset": offset,
                    }),
                )
                .await
                .expect("remote attachment readback");
            assert_eq!(
                chunk["name"].as_str().unwrap(),
                std::path::Path::new(&host_path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
            actual.extend(BASE64.decode(chunk["data"].as_str().unwrap()).unwrap());
            if chunk["done"] == true {
                break;
            }
            offset = chunk["nextOffset"].as_u64().unwrap();
        }
        assert_eq!(&actual, expected);
    }

    // Exactly-once: the doc row "arrives" later over chat2 sync — simulate by
    // writing A's exact entry into B's doc, which kicks B's drain. The
    // processed ledger must dedupe it to a no-op.
    let entry = core_a
        .doc_host
        .open(CHAT)
        .expect("open on A")
        .doc()
        .read_commands()
        .expect("read A's commands")
        .into_iter()
        .next()
        .expect("A queued exactly one command");
    let handle_b = core_b.doc_host.open(CHAT).expect("open on B");
    handle_b
        .doc()
        .queue_command(&entry)
        .expect("simulate the synced doc row");
    // A second relay attempt (a retrying sender) must also dedupe.
    let dup = core_b
        .doc_host
        .ingest_relayed_command(CHAT, entry)
        .await
        .expect("duplicate relay accepted");
    assert_eq!(dup, "duplicate");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        complete_assistant_count(&core_b),
        1,
        "the doc row + a duplicate relay must not double-run the command"
    );

    core_a.shutdown().await;
    core_b.shutdown().await;
}
