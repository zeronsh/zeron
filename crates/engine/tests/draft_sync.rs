//! Synced composer drafts end to end: real `DraftHost`s and `EngineCore`s talking to a loopback
//! fake draft room (chat2 binary frames over websockets plus the tiny HTTP surface: `/epoch`,
//! `/discard`, `/checkpoint`, `/rows`). Composer windows are modelled the way the UI drives them:
//! a `DraftDoc` replica that sends its local commits as `EditDraft` updates and applies
//! `WatchDraft` frames.

#![allow(clippy::result_large_err)] // tungstenite's handshake callback fixes the Err type

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use zeron_doc::DraftDoc;
use zeron_engine::draft_host::{DraftHost, DraftHostConfig, DraftTuning, draft_doc_id};
use zeron_engine::{EdgeConfig, EngineCore, HarnessRegistry};
use zeron_proto::{DraftFrame, HarnessId};
use zeron_rpc::{RpcError, methods};
use zeron_sync::DocsStore;
use zeron_sync::chat_frames::{decode, encode, frame_type};

const ORG: &str = "org-1";

// ── fake draft room ─────────────────────────────────────────────────────────

enum Out {
    Frame(Vec<u8>),
    Close(u16),
    Drop,
}

struct Row {
    seq: u64,
    batch: String,
    bytes: Vec<u8>,
}

struct Room {
    epoch: u64,
    seq: u64,
    floor: u64,
    rows: Vec<Row>,
    ids: HashMap<String, u64>,
    checkpoint: Option<(Vec<u8>, Vec<u8>, u64)>,
    clients: Vec<mpsc::UnboundedSender<Out>>,
    pushes: usize,
    checkpoint_posts: usize,
    refusals: usize,
}

impl Default for Room {
    fn default() -> Self {
        Self {
            epoch: 1,
            seq: 0,
            floor: 0,
            rows: Vec::new(),
            ids: HashMap::new(),
            checkpoint: None,
            clients: Vec::new(),
            pushes: 0,
            checkpoint_posts: 0,
            refusals: 0,
        }
    }
}

impl Room {
    fn state_frame(&self) -> Vec<u8> {
        let (frontier, cp_seq, cp_size) = match &self.checkpoint {
            Some((bytes, frontier, seq)) => (frontier.clone(), *seq, bytes.len()),
            None => (Vec::new(), 0, 0),
        };
        encode(
            frame_type::STATE,
            &serde_json::json!({
                "headSeq": self.seq, "seqFloor": self.floor, "checkpointSeq": cp_seq,
                "checkpointSize": cp_size, "rowCount": self.rows.len(),
                "rowBytes": self.rows.iter().map(|r| r.bytes.len()).sum::<usize>(),
            }),
            &frontier,
        )
    }

    fn row_frame(row: &Row) -> Vec<u8> {
        encode(
            frame_type::ROW,
            &serde_json::json!({"seq": row.seq, "device": "peer", "batchId": row.batch}),
            &row.bytes,
        )
    }

    fn done_frame(&self) -> Vec<u8> {
        encode(
            frame_type::ROWS_DONE,
            &serde_json::json!({"headSeq": self.seq}),
            &[],
        )
    }

    /// Store a pushed batch (deduped by id) and fan the row out to every socket but `except`.
    fn push(
        &mut self,
        batch: &str,
        bytes: Vec<u8>,
        except: Option<&mpsc::UnboundedSender<Out>>,
    ) -> u64 {
        if let Some(seq) = self.ids.get(batch) {
            return *seq;
        }
        self.pushes += 1;
        self.seq += 1;
        let row = Row {
            seq: self.seq,
            batch: batch.to_string(),
            bytes,
        };
        let frame = Self::row_frame(&row);
        self.ids.insert(batch.to_string(), row.seq);
        self.rows.push(row);
        self.clients.retain(|c| !c.is_closed());
        for client in &self.clients {
            if except.is_none_or(|e| !e.same_channel(client)) {
                let _ = client.send(Out::Frame(frame.clone()));
            }
        }
        self.seq
    }

    /// The text the stored checkpoint + rows add up to.
    fn text(&self) -> String {
        let doc = DraftDoc::new();
        if let Some((bytes, _, _)) = &self.checkpoint {
            doc.import(bytes).unwrap();
        }
        for row in &self.rows {
            doc.import(&row.bytes).unwrap();
        }
        doc.text()
    }

    fn discard(&mut self) {
        self.rows.clear();
        self.ids.clear();
        self.checkpoint = None;
        self.floor = self.seq;
        self.epoch += 1;
        for client in self.clients.drain(..) {
            let _ = client.send(Out::Close(4411));
        }
    }
}

#[derive(Clone, Default)]
struct Rooms {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    orgs: Arc<Mutex<HashSet<String>>>,
}

impl Rooms {
    fn with<T>(&self, chat: &str, f: impl FnOnce(&mut Room) -> T) -> T {
        f(self
            .rooms
            .lock()
            .unwrap()
            .entry(chat.to_string())
            .or_default())
    }
    fn text(&self, chat: &str) -> String {
        self.with(chat, |r| r.text())
    }
    fn epoch(&self, chat: &str) -> u64 {
        self.with(chat, |r| r.epoch)
    }
    fn pushes(&self, chat: &str) -> usize {
        self.with(chat, |r| r.pushes)
    }
}

/// One listener onto the shared rooms. Several fronts model several devices' network paths, so
/// one device can be offline while the others stay connected.
struct Front {
    url: String,
    offline: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<mpsc::UnboundedSender<Out>>>>,
}

impl Front {
    async fn start(rooms: &Rooms) -> Front {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let offline = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let (rooms, flag, registry) = (rooms.clone(), offline.clone(), sockets.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if flag.load(Ordering::SeqCst) {
                    drop(stream);
                    continue;
                }
                tokio::spawn(serve(stream, rooms.clone(), registry.clone()));
            }
        });
        Front {
            url,
            offline,
            sockets,
        }
    }

    /// Cut every open socket and refuse new connections.
    fn go_offline(&self) {
        self.offline.store(true, Ordering::SeqCst);
        for socket in self.sockets.lock().unwrap().drain(..) {
            let _ = socket.send(Out::Drop);
        }
    }

    fn go_online(&self) {
        self.offline.store(false, Ordering::SeqCst);
        zeron_sync::wake::notify_online();
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn http_reply(status: u16, body: Vec<u8>, extra: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    out.extend(body);
    out
}

fn mismatch(room: &mut Room) -> Vec<u8> {
    room.refusals += 1;
    http_reply(
        409,
        serde_json::json!({"error": "epoch_mismatch", "epoch": room.epoch})
            .to_string()
            .into_bytes(),
        "",
    )
}

async fn serve(
    stream: TcpStream,
    rooms: Rooms,
    registry: Arc<Mutex<Vec<mpsc::UnboundedSender<Out>>>>,
) {
    let mut buf = vec![0u8; 16 * 1024];
    let head_end = loop {
        let n = stream.peek(&mut buf).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        if let Some(i) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut first = head.lines().next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_string();
    let target = first.next().unwrap_or_default().to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let (path, query) = (path.to_string(), query.to_string());
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let websocket = head.to_ascii_lowercase().contains("upgrade: websocket");
    let (chat, route) = match segments.as_slice() {
        ["draft", org, chat, route] => {
            rooms.orgs.lock().unwrap().insert(org.to_string());
            (chat.to_string(), route.to_string())
        }
        _ => (String::new(), String::new()),
    };
    let requested_epoch = query_param(&query, "epoch").and_then(|v| v.parse::<u64>().ok());

    if websocket {
        let checked = rooms.clone();
        let check_chat = chat.clone();
        let accepted = tokio_tungstenite::accept_hdr_async(
            stream,
            move |_: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                let refuse = |status: u16| {
                    Err(tokio_tungstenite::tungstenite::http::Response::builder()
                        .status(status)
                        .body(Some("refused".to_string()))
                        .unwrap())
                };
                if route != "ws" || check_chat.is_empty() {
                    return refuse(404);
                }
                let ok = checked.with(&check_chat, |room| {
                    let ok = requested_epoch == Some(room.epoch);
                    if !ok {
                        room.refusals += 1;
                    }
                    ok
                });
                if ok { Ok(resp) } else { refuse(409) }
            },
        )
        .await;
        let Ok(ws) = accepted else { return };
        session(ws, rooms, chat, registry).await;
        return;
    }

    // Plain HTTP: consume head + body.
    let mut stream = stream;
    let length: usize = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|l| l.strip_prefix("content-length: ").map(str::to_string))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let mut data = vec![0u8; head_end + length];
    if stream.read_exact(&mut data).await.is_err() {
        return;
    }
    let body = data[head_end..].to_vec();
    let reply = if chat.is_empty() {
        http_reply(404, b"{}".to_vec(), "")
    } else {
        rooms.with(&chat, |room| match (method.as_str(), route.as_str()) {
            ("GET", "epoch") => http_reply(
                200,
                serde_json::json!({"epoch": room.epoch})
                    .to_string()
                    .into_bytes(),
                "",
            ),
            ("POST", "discard") => {
                let discarded = requested_epoch == Some(room.epoch);
                if discarded {
                    room.discard();
                }
                http_reply(
                    200,
                    serde_json::json!({"epoch": room.epoch, "discarded": discarded})
                        .to_string()
                        .into_bytes(),
                    "",
                )
            }
            ("GET", "checkpoint") => {
                if requested_epoch != Some(room.epoch) {
                    return mismatch(room);
                }
                match &room.checkpoint {
                    Some((bytes, _, seq)) => http_reply(
                        200,
                        bytes.clone(),
                        &format!("x-chat2-checkpoint-seq: {seq}\r\n"),
                    ),
                    None => http_reply(404, b"{}".to_vec(), ""),
                }
            }
            ("POST", "checkpoint") => {
                if requested_epoch != Some(room.epoch) {
                    return mismatch(room);
                }
                let covered = query_param(&query, "seqCovered")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let frontier = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("x-chat2-frontier: ")
                            .map(|_| l)
                    })
                    .and_then(|l| l.split_once(": ").map(|(_, v)| v.trim().to_string()))
                    .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
                    .unwrap_or_default();
                room.checkpoint_posts += 1;
                room.checkpoint = Some((body.clone(), frontier, covered));
                room.rows.retain(|r| r.seq > covered);
                room.floor = covered;
                http_reply(200, b"{}".to_vec(), "")
            }
            ("GET", "rows") => {
                if requested_epoch != Some(room.epoch) {
                    return mismatch(room);
                }
                let after = query_param(&query, "after")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let mut frames = vec![room.state_frame()];
                frames.extend(
                    room.rows
                        .iter()
                        .filter(|r| r.seq > after)
                        .map(Room::row_frame),
                );
                frames.push(room.done_frame());
                let mut out = Vec::new();
                for frame in frames {
                    out.extend((frame.len() as u32).to_le_bytes());
                    out.extend(frame);
                }
                http_reply(200, out, "")
            }
            ("POST", "rows") => {
                if requested_epoch != Some(room.epoch) {
                    return mismatch(room);
                }
                let batch = query_param(&query, "batchId").unwrap_or_default();
                let seq = room.push(&batch, body.clone(), None);
                http_reply(
                    200,
                    serde_json::json!({"batchId": batch, "seq": seq})
                        .to_string()
                        .into_bytes(),
                    "",
                )
            }
            _ => http_reply(404, b"{}".to_vec(), ""),
        })
    };
    let _ = stream.write_all(&reply).await;
}

async fn session(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    rooms: Rooms,
    chat: String,
    registry: Arc<Mutex<Vec<mpsc::UnboundedSender<Out>>>>,
) {
    let (mut tx, mut rx) = ws.split();
    let (out, mut inbox) = mpsc::unbounded_channel::<Out>();
    rooms.with(&chat, |room| room.clients.push(out.clone()));
    registry.lock().unwrap().push(out.clone());
    loop {
        tokio::select! {
            item = inbox.recv() => match item {
                Some(Out::Frame(bytes)) => {
                    if tx.send(Message::Binary(bytes)).await.is_err() { break }
                }
                Some(Out::Close(code)) => {
                    let _ = tx.send(Message::Close(Some(CloseFrame {
                        code: CloseCode::Library(code),
                        reason: "draft discarded".into(),
                    }))).await;
                    break;
                }
                Some(Out::Drop) | None => break,
            },
            message = rx.next() => {
                let Some(Ok(message)) = message else { break };
                let Some(frame) = decode(&message.into_data()) else { continue };
                let replies: Vec<Vec<u8>> = rooms.with(&chat, |room| match frame.kind {
                    frame_type::HELLO => vec![room.state_frame()],
                    frame_type::ROWS_REQ => {
                        let after = frame.header["after"].as_u64().unwrap_or(0);
                        let mut frames: Vec<Vec<u8>> = room
                            .rows
                            .iter()
                            .filter(|r| r.seq > after)
                            .map(Room::row_frame)
                            .collect();
                        frames.push(room.done_frame());
                        frames
                    }
                    frame_type::PUSH => {
                        let batch = frame.header["batchId"].as_str().unwrap_or_default().to_string();
                        let seq = room.push(&batch, frame.payload.clone(), Some(&out));
                        vec![encode(
                            frame_type::ACK,
                            &serde_json::json!({"batchId": batch, "seq": seq}),
                            &[],
                        )]
                    }
                    frame_type::PROBE => vec![encode(
                        frame_type::PROBE_OK,
                        &serde_json::json!({"headSeq": room.seq}),
                        &[],
                    )],
                    _ => Vec::new(),
                });
                for reply in replies {
                    if tx.send(Message::Binary(reply)).await.is_err() { return }
                }
            }
        }
    }
}

// ── harness ─────────────────────────────────────────────────────────────────

fn fast() -> DraftTuning {
    DraftTuning {
        debounce: Duration::from_millis(40),
        idle_leave: Duration::from_secs(60),
        handle_cap: 16,
        checkpoint_rows: 1_000_000,
        tick: Duration::from_millis(100),
        epoch_recheck: Duration::from_millis(500),
        join_backoff: (Duration::from_millis(100), Duration::from_secs(1)),
        reject_backoff: Duration::from_secs(1),
    }
}

fn host(dir: &Path, front: Option<&Front>, name: &str, tuning: DraftTuning) -> DraftHost {
    DraftHost::new(
        Arc::new(DocsStore::open(dir).unwrap()),
        DraftHostConfig {
            device_id: name.into(),
            org_id: ORG.into(),
            edge: front.map(|f| EdgeConfig::with_static_token(&f.url, "tok").with_device(name)),
            tuning,
        },
    )
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while !ready() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn decode_frame(frame: &DraftFrame) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(&frame.update)
        .unwrap()
}

/// A composer window: a `DraftDoc` replica exchanging Loro updates with its engine.
struct Window {
    doc: Arc<Mutex<DraftDoc>>,
    host: DraftHost,
    chat: String,
    resets: Arc<Mutex<Vec<DraftFrame>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Window {
    async fn open(host: &DraftHost, chat: &str) -> Window {
        let mut stream = host.watch(chat).unwrap();
        let first = stream.next().await.unwrap();
        assert!(first.reset, "the first frame is always a reset snapshot");
        let doc = Arc::new(Mutex::new(
            DraftDoc::from_snapshot(&decode_frame(&first)).unwrap(),
        ));
        let resets = Arc::new(Mutex::new(vec![first]));
        let (replica, log) = (doc.clone(), resets.clone());
        let task = tokio::spawn(async move {
            while let Some(frame) = stream.next().await {
                let bytes = decode_frame(&frame);
                let mut doc = replica.lock().unwrap();
                if frame.reset {
                    *doc = DraftDoc::from_snapshot(&bytes).unwrap();
                    log.lock().unwrap().push(frame);
                } else {
                    doc.import(&bytes).unwrap();
                }
            }
        });
        Window {
            doc,
            host: host.clone(),
            chat: chat.into(),
            resets,
            task,
        }
    }

    fn text(&self) -> String {
        self.doc.lock().unwrap().text()
    }

    fn set(&self, text: &str) {
        let update = {
            let doc = self.doc.lock().unwrap();
            let before = doc.version();
            if !doc.set_text(text).unwrap() {
                return;
            }
            doc.export_since(&before).unwrap()
        };
        self.host.edit(&self.chat, &update).unwrap();
    }

    fn append(&self, suffix: &str) {
        self.set(&format!("{}{suffix}", self.text()));
    }

    fn prepend(&self, prefix: &str) {
        self.set(&format!("{prefix}{}", self.text()));
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn joined(host: &DraftHost, chat: &str) -> bool {
    host.status(chat).unwrap().joined
}

fn acked(host: &DraftHost, chat: &str) -> bool {
    !host.status(chat).unwrap().unacked
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn typing_on_one_device_shows_up_live_on_another() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        host(da.path(), Some(&fa), "dev-a", fast()),
        host(db.path(), Some(&fb), "dev-b", fast()),
    );
    let (wa, wb) = (
        Window::open(&a, "chat-live").await,
        Window::open(&b, "chat-live").await,
    );
    until("both hosts to join", || {
        joined(&a, "chat-live") && joined(&b, "chat-live")
    })
    .await;

    wa.set("hello");
    until("B to show A's text", || wb.text() == "hello").await;
    wb.append(" world");
    until("A to show B's reply", || wa.text() == "hello world").await;
    until("A's edit to be acknowledged", || acked(&a, "chat-live")).await;
    assert_eq!(rooms.text("chat-live"), "hello world");
    assert!(rooms.with("chat-live", |r| r.epoch) == 1);
    assert!(rooms.orgs.lock().unwrap().contains(ORG));
    // Drafts are rooms of their own: the chat outbox / sync-job tables stay empty.
    let store = DocsStore::open(da.path()).unwrap();
    assert!(store.pending_sync_docs("", 64).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_typing_from_both_devices_merges() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        host(da.path(), Some(&fa), "dev-a", fast()),
        host(db.path(), Some(&fb), "dev-b", fast()),
    );
    let (wa, wb) = (
        Window::open(&a, "chat-merge").await,
        Window::open(&b, "chat-merge").await,
    );
    until("both hosts to join", || {
        joined(&a, "chat-merge") && joined(&b, "chat-merge")
    })
    .await;
    wa.set("base");
    until("B to see the base", || wb.text() == "base").await;

    // No waiting between the two edits: they race through the room.
    wa.prepend("A-");
    wb.append("-B");
    until("both windows to converge on both edits", || {
        wa.text() == "A-base-B" && wb.text() == "A-base-B"
    })
    .await;
    until("the room to hold the merge", || {
        rooms.text("chat-merge") == "A-base-B"
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_made_offline_merge_after_reconnect() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        host(da.path(), Some(&fa), "dev-a", fast()),
        host(db.path(), Some(&fb), "dev-b", fast()),
    );
    let (wa, wb) = (
        Window::open(&a, "chat-off").await,
        Window::open(&b, "chat-off").await,
    );
    until("both hosts to join", || {
        joined(&a, "chat-off") && joined(&b, "chat-off")
    })
    .await;
    wa.set("base");
    until("B to see the base", || wb.text() == "base").await;
    until("the base to be acked", || acked(&a, "chat-off")).await;

    fa.go_offline();
    fb.go_offline();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wa.prepend("A-");
    wb.append("-B");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(wa.text(), "A-base", "typing keeps working offline");
    assert!(a.status("chat-off").unwrap().unacked);
    assert!(b.status("chat-off").unwrap().unacked);
    assert_eq!(
        rooms.text("chat-off"),
        "base",
        "nothing reached the room yet"
    );

    fa.go_online();
    fb.go_online();
    until("both devices to converge", || {
        wa.text() == "A-base-B" && wb.text() == "A-base-B"
    })
    .await;
    until("both edits to be acknowledged", || {
        acked(&a, "chat-off") && acked(&b, "chat-off")
    })
    .await;
    assert_eq!(rooms.text("chat-off"), "A-base-B");
}

#[tokio::test(flavor = "multi_thread")]
async fn discard_wipes_every_device_and_a_stale_offline_device_cannot_resurrect_it() {
    let rooms = Rooms::default();
    let (fa, fb, fc) = (
        Front::start(&rooms).await,
        Front::start(&rooms).await,
        Front::start(&rooms).await,
    );
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = host(dirs[0].path(), Some(&fa), "dev-a", fast());
    let b = host(dirs[1].path(), Some(&fb), "dev-b", fast());
    let c = host(dirs[2].path(), Some(&fc), "dev-c", fast());
    let chat = "chat-discard";
    let (wa, wb, wc) = (
        Window::open(&a, chat).await,
        Window::open(&b, chat).await,
        Window::open(&c, chat).await,
    );
    until("all hosts to join", || {
        joined(&a, chat) && joined(&b, chat) && joined(&c, chat)
    })
    .await;
    wa.set("the message being sent");
    until("B and C to show it", || {
        wb.text() == "the message being sent" && wc.text() == "the message being sent"
    })
    .await;
    until("A's edit to be acked", || acked(&a, chat)).await;

    // C goes dark and keeps typing on its stale copy.
    fc.go_offline();
    tokio::time::sleep(Duration::from_millis(300)).await;
    wc.append(" + offline tail");

    // A sends: the draft is consumed everywhere.
    let outcome = a.clear(chat).unwrap();
    assert_eq!(
        outcome.epoch, 1,
        "the reply carries the epoch known at clear time"
    );
    until("the room to discard", || rooms.epoch(chat) == 2).await;
    until("B's window to reset to empty", || wb.text().is_empty()).await;
    until("A's window to reset to empty", || wa.text().is_empty()).await;
    assert_eq!(rooms.text(chat), "");
    assert!(
        wb.resets.lock().unwrap().len() >= 2,
        "B saw a second reset frame"
    );
    until("A to settle its discard", || {
        let status = a.status(chat).unwrap();
        !status.pending_discard && status.epoch == 2
    })
    .await;

    // C comes back with the stale epoch: it must drop its copy, never push it.
    fc.go_online();
    until("C to adopt the new epoch and drop its copy", || {
        let status = c.status(chat).unwrap();
        status.epoch == 2 && status.text.is_empty() && !status.unacked
    })
    .await;
    until("C's window to reset", || wc.text().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rooms.text(chat), "", "the sent text never came back");
    assert_eq!(rooms.epoch(chat), 2);
    assert_eq!(wa.text(), "");
    assert_eq!(wb.text(), "");
    assert!(
        rooms.with(chat, |r| r.refusals) > 0 || c.status(chat).unwrap().epoch == 2,
        "the stale epoch was refused or noticed before any push"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_while_offline_stays_pending_and_discards_on_reconnect() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        host(da.path(), Some(&fa), "dev-a", fast()),
        host(db.path(), Some(&fb), "dev-b", fast()),
    );
    let chat = "chat-pending";
    let (wa, wb) = (Window::open(&a, chat).await, Window::open(&b, chat).await);
    until("both hosts to join", || {
        joined(&a, chat) && joined(&b, chat)
    })
    .await;
    wa.set("sent while offline");
    until("B to show it", || wb.text() == "sent while offline").await;
    until("the edit to be acked", || acked(&a, chat)).await;

    fa.go_offline();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let outcome = a.clear(chat).unwrap();
    assert!(outcome.pending);
    until("A's window to reset", || wa.text().is_empty()).await;
    // The room still holds the old draft; B keeps showing it until A reaches the room.
    assert_eq!(rooms.text(chat), "sent while offline");
    // While pending, new typing belongs to the next draft.
    wa.set("next draft");

    fa.go_online();
    until("the discard to land", || rooms.epoch(chat) == 2).await;
    until("B to drop the sent text and show the next draft", || {
        wb.text() == "next draft"
    })
    .await;
    until("the next draft to be in the room", || {
        rooms.text(chat) == "next draft"
    })
    .await;
    assert!(!a.status(chat).unwrap().pending_discard);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_survives_a_restart_online_and_offline() {
    let rooms = Rooms::default();
    let front = Front::start(&rooms).await;
    let dir = tempfile::tempdir().unwrap();
    let chat = "chat-restart";

    let a = host(dir.path(), Some(&front), "dev-a", fast());
    let w = Window::open(&a, chat).await;
    w.set("persist me");
    until("the edit to be acked", || acked(&a, chat)).await;
    drop(w);
    a.shutdown().await;
    drop(a);
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    assert_eq!(a.status(chat).unwrap().text, "persist me");
    a.shutdown().await;
    drop(a);

    // Offline: typed while the room is unreachable, then the app restarts.
    front.go_offline();
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    let w = Window::open(&a, chat).await;
    w.append(" offline");
    drop(w);
    a.shutdown().await;
    drop(a);
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    let status = a.status(chat).unwrap();
    assert_eq!(status.text, "persist me offline");
    assert!(
        status.unacked,
        "the unsent edit is remembered across the restart"
    );
    drop(a);

    // Back online and after a restart with NO composer open: resume_pending re-attaches the
    // draft, so it still reaches the room.
    front.go_online();
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    a.resume_pending();
    until("the offline edit to reach the room", || {
        rooms.text(chat) == "persist me offline"
    })
    .await;
    until("it to be acknowledged", || acked(&a, chat)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_finishes_a_discard_that_never_landed() {
    let rooms = Rooms::default();
    let front = Front::start(&rooms).await;
    let dir = tempfile::tempdir().unwrap();
    let chat = "chat-restart-discard";
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    let w = Window::open(&a, chat).await;
    w.set("sent");
    until("acked", || acked(&a, chat)).await;
    front.go_offline();
    a.clear(chat).unwrap();
    drop(w);
    a.shutdown().await;
    drop(a);
    assert_eq!(rooms.text(chat), "sent");

    front.go_online();
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    a.resume_pending();
    until("the pending discard to run after the restart", || {
        rooms.epoch(chat) == 2
    })
    .await;
    assert_eq!(rooms.text(chat), "");
    until("the flag to clear", || {
        !a.status(chat).unwrap().pending_discard
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_edge_drafts_are_local_and_windows_share_them() {
    let dir = tempfile::tempdir().unwrap();
    let chat = "chat-local";
    let h = host(dir.path(), None, "dev-a", fast());
    assert!(!h.is_synced());
    let (w1, w2) = (Window::open(&h, chat).await, Window::open(&h, chat).await);
    w1.set("typed in window one");
    until("window two to follow", || {
        w2.text() == "typed in window one"
    })
    .await;
    w2.append("!");
    until("window one to follow", || {
        w1.text() == "typed in window one!"
    })
    .await;
    let status = h.status(chat).unwrap();
    assert!(!status.joined);
    assert_eq!(status.watchers, 2);
    drop((w1, w2));
    h.shutdown().await;
    drop(h);

    let h = host(dir.path(), None, "dev-a", fast());
    assert_eq!(h.status(chat).unwrap().text, "typed in window one!");
    let w = Window::open(&h, chat).await;
    assert_eq!(w.text(), "typed in window one!");
    h.clear(chat).unwrap();
    until("the window to reset", || w.text().is_empty()).await;
    assert!(
        !DocsStore::open(dir.path())
            .unwrap()
            .has_snapshot(&draft_doc_id(chat))
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_windows_on_one_engine_and_a_remote_device_all_converge() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        host(da.path(), Some(&fa), "dev-a", fast()),
        host(db.path(), Some(&fb), "dev-b", fast()),
    );
    let chat = "chat-windows";
    let (w1, w2, wb) = (
        Window::open(&a, chat).await,
        Window::open(&a, chat).await,
        Window::open(&b, chat).await,
    );
    until("joined", || joined(&a, chat) && joined(&b, chat)).await;
    w1.set("one");
    until("all follow w1", || w2.text() == "one" && wb.text() == "one").await;
    w2.append(" two");
    wb.prepend("zero ");
    until("all converge", || {
        let expected = "zero one two";
        w1.text() == expected && w2.text() == expected && wb.text() == expected
    })
    .await;
    // Both windows share ONE room connection on device A: exactly one watcher-driven join.
    assert_eq!(a.status(chat).unwrap().watchers, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn evicting_handles_never_loses_unacknowledged_edits() {
    let rooms = Rooms::default();
    let front = Front::start(&rooms).await;
    let dir = tempfile::tempdir().unwrap();
    front.go_offline();
    let a = host(
        dir.path(),
        Some(&front),
        "dev-a",
        DraftTuning {
            handle_cap: 2,
            ..fast()
        },
    );
    for n in 0..5 {
        let w = Window::open(&a, &format!("chat-lru-{n}")).await;
        w.set(&format!("draft {n}"));
    }
    // Touch more handles to force eviction pressure; the dirty ones must stay.
    for n in 5..9 {
        a.status(&format!("chat-lru-{n}")).unwrap();
    }
    for n in 0..5 {
        assert_eq!(
            a.status(&format!("chat-lru-{n}")).unwrap().text,
            format!("draft {n}")
        );
    }
    front.go_online();
    until("every unacked draft to reach its room", || {
        (0..5).all(|n| rooms.text(&format!("chat-lru-{n}")) == format!("draft {n}"))
    })
    .await;
    until("all acked", || {
        (0..5).all(|n| acked(&a, &format!("chat-lru-{n}")))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bursts_of_typing_coalesce_into_few_pushes() {
    let rooms = Rooms::default();
    let front = Front::start(&rooms).await;
    let dir = tempfile::tempdir().unwrap();
    let a = host(
        dir.path(),
        Some(&front),
        "dev-a",
        DraftTuning {
            debounce: Duration::from_millis(250),
            ..fast()
        },
    );
    let chat = "chat-burst";
    let w = Window::open(&a, chat).await;
    until("joined", || joined(&a, chat)).await;
    for _ in 0..30 {
        w.append("x");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    until("the burst to reach the room", || {
        rooms.text(chat) == "x".repeat(30)
    })
    .await;
    until("acked", || acked(&a, chat)).await;
    let pushes = rooms.pushes(chat);
    assert!(
        (1..=4).contains(&pushes),
        "30 edits inside ~150 ms should be a handful of rows, saw {pushes}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_row_log_is_checkpointed_so_a_cold_device_loads_from_it() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = host(
        da.path(),
        Some(&fa),
        "dev-a",
        DraftTuning {
            checkpoint_rows: 3,
            ..fast()
        },
    );
    let chat = "chat-checkpoint";
    let w = Window::open(&a, chat).await;
    until("joined", || joined(&a, chat)).await;
    let mut expected = String::new();
    for n in 0..6 {
        expected.push_str(&format!("part{n} "));
        w.set(&expected);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    until("a checkpoint to be posted", || {
        rooms.with(chat, |r| r.checkpoint_posts) >= 1
    })
    .await;
    assert!(
        rooms.with(chat, |r| r.rows.len()) < 6,
        "the checkpoint trimmed the row log"
    );

    let b = host(db.path(), Some(&fb), "dev-b", fast());
    let wb = Window::open(&b, chat).await;
    until("a cold device to load the whole draft", || {
        wb.text() == expected
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_drafts_stay_local_and_are_never_pushed() {
    let rooms = Rooms::default();
    let front = Front::start(&rooms).await;
    let dir = tempfile::tempdir().unwrap();
    let a = host(dir.path(), Some(&front), "dev-a", fast());
    let chat = "chat-huge";
    let w = Window::open(&a, chat).await;
    until("joined", || joined(&a, chat)).await;
    // Random-ish text so the compressed row really is over the 64 KiB cap.
    let big: String = (0..200_000u32)
        .map(|n| char::from(b'a' + ((n.wrapping_mul(2_654_435_761) >> 13) % 26) as u8))
        .collect();
    w.set(&big);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        rooms.pushes(chat),
        0,
        "the room never sees a row it would refuse"
    );
    let status = a.status(chat).unwrap();
    assert_eq!(status.text.len(), big.len(), "the local text is kept");
    assert!(status.unacked);
    // Shrinking the draft makes it pushable again.
    w.set("small again");
    until("the small draft to reach the room", || {
        rooms.text(chat) == "small again"
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_rows_are_never_treated_as_chats() {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let doc = DraftDoc::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = doc.subscribe_local_update(move |b| sink.lock().unwrap().push(b.to_vec()));
    doc.set_text("not a chat").unwrap();
    let update = seen.lock().unwrap()[0].clone();
    client
        .call(
            methods::EDIT_DRAFT,
            serde_json::json!({"chatId": "abc", "update": base64::engine::general_purpose::STANDARD.encode(&update)}),
        )
        .await
        .unwrap();
    core.drafts.flush_all();

    // The draft is on disk under its own namespace, out of the chat sync tables...
    let store = DocsStore::open(
        dir.path()
            .join("orgs")
            .join(zeron_engine::DEFAULT_ORG_ID)
            .join(zeron_engine::DEFAULT_USER_ID),
    )
    .unwrap();
    assert!(store.has_snapshot(&draft_doc_id("abc")).unwrap());
    assert!(store.pending_sync_docs("", 64).unwrap().is_empty());
    assert!(
        store
            .pending_sync_jobs("recovery", "", 64)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.doc_ids_with_prefix("draft/").unwrap(),
        vec![draft_doc_id("abc")]
    );
    // ...and no chat-facing path resolves it.
    assert!(core.doc_host.open("draft/abc").is_err());
    assert!(core.doc_host.open_local("draft/abc").is_err());
    assert!(core.doc_host.open_local(&draft_doc_id("abc")).is_err());
    core.shutdown().await;
}

// ── RPC surface ─────────────────────────────────────────────────────────────

fn engine(dir: &Path, front: Option<&Front>, name: &str) -> EngineCore {
    let edge = front.map(|f| EdgeConfig::with_static_token(&f.url, "tok").with_device(name));
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, edge)
        .expect("engine assembles")
}

/// `BadParams` crosses the wire as a `Failed("bad params: …")` reply.
fn is_bad_params<T: std::fmt::Debug>(result: &Result<T, RpcError>) -> bool {
    matches!(result, Err(RpcError::Failed(message)) if message.starts_with("bad params"))
        || matches!(result, Err(RpcError::BadParams(_)))
}

fn update_for(doc: &DraftDoc, text: &str) -> String {
    let before = doc.version();
    doc.set_text(text).unwrap();
    base64::engine::general_purpose::STANDARD.encode(doc.export_since(&before).unwrap())
}

async fn next_frame(sub: &mut zeron_rpc::RpcSubscription) -> DraftFrame {
    let value = tokio::time::timeout(Duration::from_secs(20), sub.recv())
        .await
        .expect("a draft frame arrives")
        .expect("stream open");
    serde_json::from_value(value).expect("DraftFrame")
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_validation_local_scope_and_frame_ordering() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path(), None, "dev-a");
    let client = zeron_rpc::memory_client(core.rpc_service());
    let chat = "chat-rpc";

    // The stream opens with a reset, then carries every change.
    let mut sub = client
        .subscribe_scoped(methods::WATCH_DRAFT, serde_json::json!({"chatId": chat}))
        .await
        .unwrap();
    let first = next_frame(&mut sub).await;
    assert!(first.reset);
    let window = DraftDoc::from_snapshot(&decode_frame(&first)).unwrap();
    assert_eq!(window.text(), "");

    let update = update_for(&window, "hello");
    // `targetDeviceId` is ignored: drafts are always served by the local engine.
    let reply = client
        .call(
            methods::EDIT_DRAFT,
            serde_json::json!({"chatId": chat, "update": update, "targetDeviceId": "some-other-device"}),
        )
        .await
        .unwrap();
    assert_eq!(reply["changed"], true);
    let echo = next_frame(&mut sub).await;
    assert!(!echo.reset);
    window.import(&decode_frame(&echo)).unwrap();
    assert_eq!(window.text(), "hello");

    // Bad input is a proper RPC error and leaves the draft alone.
    let bad_b64 = client
        .call(
            methods::EDIT_DRAFT,
            serde_json::json!({"chatId": chat, "update": "***not base64***"}),
        )
        .await;
    assert!(is_bad_params(&bad_b64), "{bad_b64:?}");
    let bad_bytes = client
        .call(
            methods::EDIT_DRAFT,
            serde_json::json!({"chatId": chat, "update": base64::engine::general_purpose::STANDARD.encode(b"garbage")}),
        )
        .await;
    assert!(is_bad_params(&bad_bytes), "{bad_bytes:?}");
    for method in [methods::EDIT_DRAFT, methods::CLEAR_DRAFT] {
        let result = client
            .call(
                method,
                serde_json::json!({"chatId": "../etc/passwd", "update": update}),
            )
            .await;
        assert!(is_bad_params(&result), "{method}: {result:?}");
    }
    let watch = client
        .subscribe_checked(methods::WATCH_DRAFT, serde_json::json!({"chatId": "a/b"}))
        .await;
    assert!(watch.is_err(), "an invalid chat id cannot be watched");
    assert!(is_bad_params(
        &client
            .call(methods::CLEAR_DRAFT, serde_json::json!({}))
            .await
    ));
    assert_eq!(core.drafts.status(chat).unwrap().text, "hello");

    // Local scope: nothing to discard remotely.
    let cleared = client
        .call(methods::CLEAR_DRAFT, serde_json::json!({"chatId": chat}))
        .await
        .unwrap();
    assert_eq!(cleared["pending"], false);
    let reset = next_frame(&mut sub).await;
    assert!(reset.reset);
    assert_eq!(
        DraftDoc::from_snapshot(&decode_frame(&reset))
            .unwrap()
            .text(),
        ""
    );
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_engines_sync_and_discard_through_the_rpc_surface() {
    let rooms = Rooms::default();
    let (fa, fb) = (Front::start(&rooms).await, Front::start(&rooms).await);
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (core_a, core_b) = (
        engine(da.path(), Some(&fa), "dev-a"),
        engine(db.path(), Some(&fb), "dev-b"),
    );
    let (ca, cb) = (
        zeron_rpc::memory_client(core_a.rpc_service()),
        zeron_rpc::memory_client(core_b.rpc_service()),
    );
    let chat = "chat-two-engines";
    let mut sub_a = ca
        .subscribe_scoped(methods::WATCH_DRAFT, serde_json::json!({"chatId": chat}))
        .await
        .unwrap();
    let mut sub_b = cb
        .subscribe_scoped(methods::WATCH_DRAFT, serde_json::json!({"chatId": chat}))
        .await
        .unwrap();
    let window_a = DraftDoc::from_snapshot(&decode_frame(&next_frame(&mut sub_a).await)).unwrap();
    let window_b = DraftDoc::from_snapshot(&decode_frame(&next_frame(&mut sub_b).await)).unwrap();
    until("both engines to join the room", || {
        core_a.drafts.status(chat).unwrap().joined && core_b.drafts.status(chat).unwrap().joined
    })
    .await;

    ca.call(
        methods::EDIT_DRAFT,
        serde_json::json!({"chatId": chat, "update": update_for(&window_a, "typed on A")}),
    )
    .await
    .unwrap();
    while window_b.text() != "typed on A" {
        let frame = next_frame(&mut sub_b).await;
        assert!(!frame.reset);
        window_b.import(&decode_frame(&frame)).unwrap();
    }

    // B answers; A sees it (A's own echo frames import as no-ops on the way).
    cb.call(
        methods::EDIT_DRAFT,
        serde_json::json!({"chatId": chat, "update": update_for(&window_b, "typed on A + B")}),
    )
    .await
    .unwrap();
    while window_a.text() != "typed on A + B" {
        let frame = next_frame(&mut sub_a).await;
        assert!(!frame.reset);
        window_a.import(&decode_frame(&frame)).unwrap();
    }

    // Sending on A discards everywhere: B's stream delivers a reset with an empty snapshot.
    ca.call(methods::CLEAR_DRAFT, serde_json::json!({"chatId": chat}))
        .await
        .unwrap();
    let reset = loop {
        let frame = next_frame(&mut sub_b).await;
        if frame.reset {
            break frame;
        }
        window_b.import(&decode_frame(&frame)).unwrap();
    };
    assert_eq!(
        DraftDoc::from_snapshot(&decode_frame(&reset))
            .unwrap()
            .text(),
        ""
    );
    assert_eq!(reset.epoch, 2);
    assert_eq!(rooms.epoch(chat), 2);
    assert!(rooms.orgs.lock().unwrap().contains("dev-org"));
    core_a.shutdown().await;
    core_b.shutdown().await;
}
