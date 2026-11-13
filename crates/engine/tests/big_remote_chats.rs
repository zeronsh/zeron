//! Chats messaging a very long chat on another device, through real engines
//! and the real MCP tools over a relay that behaves as the edge's does: it
//! delivers each message as one frame, and past 32 MiB in one message it closes
//! the host's socket (measured on workerd). A transcript used to be one message,
//! which ended the link: `WatchDocMessages: stream ended before the transcript
//! arrived`, for every send to that chat since a send read the transcript first.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_mcp::{Origin, Tools, Zeron};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, Session, SessionStatus,
    SteeringMode,
};
use zeron_rpc::{
    DeviceFrameHeader, LinkCache, LinkCacheConfig, StaticToken, decode_device_frame,
    encode_device_frame,
};

/// What workerd does with a message from a socket, measured: past this it
/// closes that socket.
const ROOM_MESSAGE_CAP: usize = 32 << 20;

#[derive(Default)]
struct Room {
    host: Option<mpsc::UnboundedSender<Vec<u8>>>,
    clients: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
}

#[derive(Default)]
struct Counters {
    largest_host_message: AtomicUsize,
    host_sockets: AtomicUsize,
    client_sockets: AtomicUsize,
}

async fn device_room(counters: Arc<Counters>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let room = Arc::new(Mutex::new(Room::default()));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let room = room.clone();
            let counters = counters.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let config = WebSocketConfig {
                    max_message_size: Some(1 << 30),
                    max_frame_size: Some(1 << 30),
                    ..Default::default()
                };
                let Ok(ws) = tokio_tungstenite::accept_hdr_async_with_config(
                    stream,
                    |req: &WsRequest, res: WsResponse| {
                        uri = req.uri().to_string();
                        Ok(res)
                    },
                    Some(config),
                )
                .await
                else {
                    return;
                };
                let query = uri.split_once('?').map(|(_, q)| q).unwrap_or("").to_owned();
                let is_host = query.contains("role=host");
                let conn = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("connId="))
                    .unwrap_or("anon")
                    .to_owned();
                let (mut sink, mut reader) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                if is_host {
                    counters.host_sockets.fetch_add(1, Ordering::SeqCst);
                    room.lock().unwrap().host = Some(tx);
                } else {
                    counters.client_sockets.fetch_add(1, Ordering::SeqCst);
                    room.lock().unwrap().clients.insert(conn.clone(), tx);
                }
                tokio::spawn(async move {
                    while let Some(bytes) = rx.recv().await {
                        if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = reader.next().await {
                    let WsMessage::Binary(bytes) = message else {
                        continue;
                    };
                    if is_host {
                        counters
                            .largest_host_message
                            .fetch_max(bytes.len(), Ordering::SeqCst);
                    }
                    if bytes.len() > ROOM_MESSAGE_CAP {
                        break;
                    }
                    let Ok((header, payload)) = decode_device_frame(&bytes) else {
                        break;
                    };
                    let room = room.lock().unwrap();
                    if is_host {
                        let Some(to) = header.to else { continue };
                        if let Some(client) = room.clients.get(&to) {
                            let plain = DeviceFrameHeader::new(header.s, header.k);
                            let _ = client.send(encode_device_frame(&plain, &payload).unwrap());
                        }
                    } else if let Some(host) = &room.host {
                        let mut routed = DeviceFrameHeader::new(header.s, header.k);
                        routed.from = Some(conn.clone());
                        let _ = host.send(encode_device_frame(&routed, &payload).unwrap());
                    }
                }
                if is_host {
                    room.lock().unwrap().host = None;
                } else {
                    room.lock().unwrap().clients.remove(&conn);
                }
            });
        }
    });
    url
}

struct Replies;

#[async_trait::async_trait]
impl Harness for Replies {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Replies"
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
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "m".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "hs".into(),
                assistant_message_id: "reply".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: "the whale answers".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("hs".into()),
            }),
        ])
        .boxed())
    }
}

fn assemble(dir: &std::path::Path, device: &str) -> EngineCore {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("device-id"), device).unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Replies));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).expect("engine")
}

const CHAT: &str = "whale";
const MESSAGES: usize = 20_000;
/// 20,000 of these is 40 MiB of transcript: past the room's cap and any frame's.
const MESSAGE_BYTES: usize = 2 * 1024;

/// A chat with its harness set, like any chat that has run: sending to it
/// needs nothing from its host.
fn config() -> zeron_proto::ChatConfig {
    serde_json::from_value(
        json!({"harness": "mock", "model": null, "reasoning": null, "sandbox": "workspace-write"}),
    )
    .unwrap()
}

fn seed(core: &EngineCore) {
    let handle = core.doc_host.open(CHAT).expect("open chat");
    let text = "x".repeat(MESSAGE_BYTES);
    for i in 0..MESSAGES {
        handle
            .doc()
            .push_message(&SessionMessageEntry {
                id: format!("m-{i}"),
                role: if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                parts: vec![MessagePart::Text {
                    id: "t".into(),
                    text: text.clone(),
                }],
                created_at: i as i64,
                device_id: "device-b".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .expect("seed message");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_reach_a_huge_chat_on_another_device() {
    let counters = Arc::new(Counters::default());
    let relay = device_room(counters.clone()).await;
    let dirs = tempfile::tempdir().unwrap();

    let host = assemble(&dirs.path().join("b"), "device-b");
    host.workspace
        .create_chat(CHAT, None, Some("device-b"), Some(config()), None)
        .unwrap();
    host.workspace.rename_chat(CHAT, "Whale").unwrap();
    seed(&host);
    let _relay_host = host.start_host_relay(&relay);

    // The reader knows the chat from the registry, not the doc: it holds none
    // of the transcript.
    let reader = assemble(&dirs.path().join("a"), "device-a");
    reader
        .workspace
        .create_chat(CHAT, None, Some("device-b"), Some(config()), None)
        .unwrap();
    reader.workspace.rename_chat(CHAT, "Whale").unwrap();
    let mut links = LinkCacheConfig::new(relay, Arc::new(StaticToken("user".into())));
    links.probe_timeout = Duration::from_secs(20);
    reader.set_links(LinkCache::new(links));

    let zeron = Arc::new(Zeron::with_client(
        zeron_rpc::memory_client(reader.rpc_service()),
        Origin::default(),
    ));
    let tools = Tools::new(zeron.clone());

    // read_chat limit 1 asks for one message, however long the chat. The
    // first read opens the doc and builds the host's mirror; the second is
    // what a repeat costs.
    let read_one = || tools.call("read_chat", json!({"chat": CHAT, "limit": 1}));
    let started = Instant::now();
    let first = loop {
        match read_one().await {
            Ok(read) => break read,
            Err(error) => {
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let cold = started.elapsed();
    assert_eq!(first["total"], MESSAGES);
    assert_eq!(first["returned"], 1);
    assert_eq!(first["olderRemaining"], MESSAGES - 1);
    assert_eq!(first["messages"][0]["id"], format!("m-{}", MESSAGES - 1));
    let started = Instant::now();
    read_one().await.expect("a second read");
    let warm = started.elapsed();
    eprintln!("read_chat limit 1 of {MESSAGES} messages: cold {cold:?}, warm {warm:?}");
    assert!(warm < Duration::from_secs(2), "took {warm:?}");
    let window_message = counters.largest_host_message.load(Ordering::SeqCst);
    assert!(
        window_message < 64 << 10,
        "{window_message} bytes for one message"
    );

    // get_chat: the count without the transcript.
    let summary = tools.call("get_chat", json!({"chat": CHAT})).await.unwrap();
    assert_eq!(summary["messageCount"], MESSAGES);

    // send_message returns success at once, and what it wrote is the command
    // that will reach the host.
    let started = Instant::now();
    let sent = tools
        .call(
            "send_message",
            json!({"chat": CHAT, "text": "are you there?"}),
        )
        .await
        .expect("delivered");
    eprintln!("send_message to the whale: {:?}", started.elapsed());
    assert_eq!(sent["sent"]["delivery"], "run");
    let commands = reader
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_commands()
        .unwrap();
    let command = commands
        .last()
        .expect("a command in the doc")
        .payload
        .clone();

    // The doc syncs across the chat room (not in this rig): hand the command
    // to the host as that would, and let it run the turn.
    let baseline: Session = serde_json::from_value(json!({
        "chatId": CHAT, "deviceId": "device-b", "status": "working",
        "updatedAt": chrono::Utc::now(),
    }))
    .unwrap();
    reader.workspace.record_session(&baseline);
    host.doc_host.queue_command(CHAT, command).unwrap();

    // The registry syncs too: the row turns idle when the host's turn ends.
    let row_reader = reader.workspace.clone();
    let host_sessions = host.workspace.clone();
    tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(row) = host_sessions
                .read_sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.chat_id == CHAT)
                && row.status == SessionStatus::Idle
                && row.last_completed_turn.is_some()
            {
                row_reader.record_session(&row);
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let started = Instant::now();
    let waited = tools
        .call("wait_for_turn", json!({"chat": CHAT, "timeout_secs": 60}))
        .await
        .expect("wait");
    eprintln!("wait_for_turn on the whale: {:?}", started.elapsed());
    assert_eq!(waited["turn"]["outcome"], "completed", "{waited}");
    let replies = waited["turn"]["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 1, "only this turn's reply: {waited}");
    assert_eq!(replies[0]["text"], "the whale answers");
    assert_eq!(waited["turn"].get("transcriptError"), None);

    // All of that without ever putting a transcript on the wire.
    let largest = counters.largest_host_message.load(Ordering::SeqCst);
    assert!(largest < 64 << 10, "{largest} bytes for a bounded read");

    // And a full read of all of it: over 32 MiB, whole, on the same sockets.
    let hosts_before = counters.host_sockets.load(Ordering::SeqCst);
    let clients_before = counters.client_sockets.load(Ordering::SeqCst);
    let started = Instant::now();
    let everything = zeron
        .transcript_on(CHAT, Some("device-b"))
        .await
        .expect("a full read");
    eprintln!(
        "full read of {} messages: {:?}",
        everything.len(),
        started.elapsed()
    );
    assert!(everything.len() >= MESSAGES + 2, "{}", everything.len());
    let largest = counters.largest_host_message.load(Ordering::SeqCst);
    assert!(
        largest <= 1 << 20,
        "a {largest}-byte message reached the relay"
    );
    assert_eq!(counters.host_sockets.load(Ordering::SeqCst), hosts_before);
    assert_eq!(
        counters.client_sockets.load(Ordering::SeqCst),
        clients_before
    );
}
