//! A minimal in-process edge for live-mode tests: one TCP port that routes
//! WebSocket upgrades by path, like the Worker:
//!
//! - `/registry/{org}/ws` → proxied to zeron-sync's `MockRegistryServer`
//!   (the SAME merge fn as production);
//! - `/chat2/{chat}/ws`   → a small chat2 room (hello/state, rowsReq/rows/
//!   rowsDone, push/ack + broadcast, probe/probeOk) with no checkpoint;
//! - `/device/{id}/ws`    → the DeviceRoom relay (host/client routing by
//!   `from`/`to`, `host_offline` bounces) — port of zeron-rpc's test fake;
//! - anything else (plain HTTP pulls, nudges) is refused, which the client
//!   must tolerate (it falls back to the sockets).
//!
//! Tests play the host by reading rows ([`MockEdge::rows`]) and injecting
//! host-authored Loro updates ([`MockEdge::inject`]).

#![allow(dead_code)]
// tungstenite fixes the handshake callback's Err type as a full Response.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use zeron_sync::chat_frames::{self, frame_type};
use zeron_sync::registry::mock_server::MockRegistryServer;

#[derive(Clone, Debug)]
pub struct Row {
    pub seq: u64,
    pub device: String,
    pub batch_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct Room {
    rows: Vec<Row>,
    batches: HashMap<String, u64>,
    /// The relay layer: the offline queue, and what a sleeping host left behind.
    queue: Vec<Row>,
    checkpoint: Option<Checkpoint>,
    /// Every `hello` header this room heard, in order.
    hellos: Vec<serde_json::Value>,
    wakes: usize,
}

#[derive(Clone)]
struct Checkpoint {
    bytes: Vec<u8>,
    frontier: Vec<u8>,
    seq: u64,
}

/// A frame for other sockets in a room. Never stored.
#[derive(Clone)]
struct RelayOut {
    chat: String,
    from: String,
    to: Option<String>,
    skip: Vec<String>,
    frame: Vec<u8>,
}

struct Shared {
    rooms: Mutex<HashMap<String, Room>>,
    /// (chat, row) broadcasts to every socket of the chat.
    tx: broadcast::Sender<(String, Row)>,
    /// Chat sockets currently open (per chat).
    sockets: Mutex<HashMap<String, usize>>,
    /// Refuse chat2 joins (outage simulation).
    refuse_chat: Mutex<bool>,
    relays: Mutex<HashMap<String, RelayState>>,
    nudge_status: Mutex<u16>,
    nudge_status_by_chat: Mutex<HashMap<String, u16>>,
    nudges: Mutex<Vec<(String, usize)>>,
    host_hints: Mutex<HashMap<String, String>>,
    /// `CHAT_RELAY_LAYER=1`: answer `relay: 1`, and serve the frames behind it.
    relay_layer: Mutex<bool>,
    /// Answer plain-HTTP chat pulls and checkpoint reads, which [`MockEdge::start`]
    /// refuses so older tests stay on their sockets.
    http: Mutex<bool>,
    relay_tx: broadcast::Sender<RelayOut>,
    /// Close every socket of this chat, as a dropped network does.
    kick_tx: broadcast::Sender<String>,
    /// Per chat: checkpoint downloads, row pulls (with their queries), and HTTP pushes.
    downloads: Mutex<HashMap<String, usize>>,
    pulls: Mutex<HashMap<String, Vec<String>>>,
    http_pushes: Mutex<HashMap<String, Vec<String>>>,
}

enum Out {
    Frame(Vec<u8>),
    Close,
}

#[derive(Default)]
struct RelayState {
    host: Option<tokio::sync::mpsc::UnboundedSender<Out>>,
    clients: HashMap<String, tokio::sync::mpsc::UnboundedSender<Out>>,
}

pub struct MockEdge {
    pub registry: MockRegistryServer,
    port: u16,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl MockEdge {
    pub async fn start() -> Self {
        let registry = MockRegistryServer::start().await;
        let registry_url = registry.url();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, _) = broadcast::channel(4096);
        let shared = Arc::new(Shared {
            rooms: Mutex::new(HashMap::new()),
            tx,
            sockets: Mutex::new(HashMap::new()),
            refuse_chat: Mutex::new(false),
            relays: Mutex::new(HashMap::new()),
            nudge_status: Mutex::new(200),
            nudge_status_by_chat: Mutex::new(HashMap::new()),
            nudges: Mutex::new(Vec::new()),
            host_hints: Mutex::new(HashMap::new()),
            relay_layer: Mutex::new(false),
            http: Mutex::new(false),
            relay_tx: broadcast::channel(4096).0,
            kick_tx: broadcast::channel(64).0,
            downloads: Mutex::new(HashMap::new()),
            pulls: Mutex::new(HashMap::new()),
            http_pushes: Mutex::new(HashMap::new()),
        });
        let accept_shared = shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let shared = accept_shared.clone();
                let registry_url = registry_url.clone();
                tokio::spawn(async move {
                    let mut first = [0u8; 4];
                    if *shared.http.lock().unwrap() && serve_plain_get(&mut stream, &shared).await {
                        return;
                    }
                    if stream.peek(&mut first).await.is_ok() && first[0] == b'P' {
                        let mut request = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            let Ok(n) = stream.read(&mut buf).await else {
                                return;
                            };
                            if n == 0 {
                                return;
                            }
                            request.extend_from_slice(&buf[..n]);
                            let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n")
                            else {
                                continue;
                            };
                            let headers = String::from_utf8_lossy(&request[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if request.len() < end + 4 + length {
                                continue;
                            }
                            if headers
                                .lines()
                                .next()
                                .is_some_and(|l| l.contains("/nudge "))
                            {
                                let mut nudge_chat = None;
                                if let Ok(body) = serde_json::from_slice::<serde_json::Value>(
                                    &request[end + 4..end + 4 + length],
                                ) {
                                    if let Some(chat) = body["chatId"].as_str() {
                                        nudge_chat = Some(chat.to_owned());
                                        let rows = shared
                                            .rooms
                                            .lock()
                                            .unwrap()
                                            .get(chat)
                                            .map_or(0, |r| r.rows.len());
                                        shared.nudges.lock().unwrap().push((chat.to_owned(), rows));
                                    }
                                }
                                let status = nudge_chat
                                    .as_ref()
                                    .and_then(|chat| {
                                        shared
                                            .nudge_status_by_chat
                                            .lock()
                                            .unwrap()
                                            .get(chat)
                                            .copied()
                                    })
                                    .unwrap_or_else(|| *shared.nudge_status.lock().unwrap());
                                let response = format!(
                                    "HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                            } else if let Some(reply) = http_push(
                                &headers,
                                &request[end + 4..end + 4 + length],
                                &shared,
                            ) {
                                let _ = stream.write_all(&reply).await;
                            }
                            return;
                        }
                    }
                    let path = Arc::new(Mutex::new(String::new()));
                    let seen = path.clone();
                    let callback = move |request: &Request, response: Response| {
                        *seen.lock().unwrap() = request.uri().to_string();
                        Ok(response)
                    };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
                        return;
                    };
                    let uri = path.lock().unwrap().clone();
                    let (path, query) = uri.split_once('?').unwrap_or((uri.as_str(), ""));
                    let query: HashMap<String, String> = query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect();
                    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
                    match parts.as_slice() {
                        ["device", device, "ws"] => {
                            serve_relay(ws, device.to_string(), query, shared).await
                        }
                        ["registry", _, "ws"] => proxy(ws, &registry_url).await,
                        ["chat2", chat, "ws"] => {
                            if *shared.refuse_chat.lock().unwrap() {
                                return;
                            }
                            if let Some(host) = query.get("hostDevice") {
                                shared
                                    .host_hints
                                    .lock()
                                    .unwrap()
                                    .insert(chat.to_string(), host.clone());
                            }
                            serve_chat(ws, chat.to_string(), shared).await
                        }
                        _ => {}
                    }
                });
            }
        });
        Self {
            registry,
            port,
            shared,
            task,
        }
    }

    /// An edge with `CHAT_RELAY_LAYER=1`, answering HTTP as the real one does.
    pub async fn start_relay() -> Self {
        let edge = Self::start().await;
        *edge.shared.relay_layer.lock().unwrap() = true;
        *edge.shared.http.lock().unwrap() = true;
        edge
    }

    /// Turn the edge flag on or off for rooms' next answers.
    pub fn set_relay_layer(&self, on: bool) {
        *self.shared.relay_layer.lock().unwrap() = on;
    }

    /// What a chat's host stored before it slept.
    pub fn post_checkpoint(&self, chat: &str, bytes: Vec<u8>, frontier: Vec<u8>) {
        let mut rooms = self.shared.rooms.lock().unwrap();
        let room = rooms.entry(chat.to_owned()).or_default();
        let seq = room.rows.len() as u64;
        room.checkpoint = Some(Checkpoint { bytes, frontier, seq });
    }

    /// Drop every socket of this chat, as a network loss does. They may redial.
    pub fn kick(&self, chat: &str) {
        let _ = self.shared.kick_tx.send(chat.to_owned());
    }

    pub fn queue_len(&self, chat: &str) -> usize {
        self.shared
            .rooms
            .lock()
            .unwrap()
            .get(chat)
            .map_or(0, |r| r.queue.len())
    }

    pub fn hellos(&self, chat: &str) -> Vec<serde_json::Value> {
        self.shared
            .rooms
            .lock()
            .unwrap()
            .get(chat)
            .map(|r| r.hellos.clone())
            .unwrap_or_default()
    }

    pub fn checkpoint_downloads(&self, chat: &str) -> usize {
        self.shared.downloads.lock().unwrap().get(chat).copied().unwrap_or(0)
    }

    /// The query string of every plain-HTTP row pull of this chat.
    pub fn pulls(&self, chat: &str) -> Vec<String> {
        self.shared.pulls.lock().unwrap().get(chat).cloned().unwrap_or_default()
    }

    /// The query string of every plain-HTTP push to this chat.
    pub fn http_pushes(&self, chat: &str) -> Vec<String> {
        self.shared.http_pushes.lock().unwrap().get(chat).cloned().unwrap_or_default()
    }

    pub fn nudge_status(&self, status: u16) {
        *self.shared.nudge_status.lock().unwrap() = status;
    }

    pub fn host_hint(&self, chat: &str) -> Option<String> {
        self.shared.host_hints.lock().unwrap().get(chat).cloned()
    }

    pub fn nudge_status_for(&self, chat: &str, status: u16) {
        self.shared
            .nudge_status_by_chat
            .lock()
            .unwrap()
            .insert(chat.to_owned(), status);
    }

    pub fn nudges(&self, chat: &str) -> usize {
        self.shared
            .nudges
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == chat)
            .count()
    }

    pub fn last_nudge_row_count(&self, chat: &str) -> Option<usize> {
        self.shared
            .nudges
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == chat)
            .map(|(_, rows)| *rows)
    }

    pub fn edge_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn rows(&self, chat: &str) -> Vec<Row> {
        self.shared
            .rooms
            .lock()
            .unwrap()
            .get(chat)
            .map(|r| r.rows.clone())
            .unwrap_or_default()
    }

    /// Append a host-authored update and broadcast it.
    pub fn inject(&self, chat: &str, device: &str, bytes: Vec<u8>) -> u64 {
        let row = {
            let mut rooms = self.shared.rooms.lock().unwrap();
            let room = rooms.entry(chat.to_owned()).or_default();
            let seq = room.rows.len() as u64 + 1;
            let row = Row {
                seq,
                device: device.to_owned(),
                batch_id: format!("host-{seq}"),
                bytes,
            };
            room.rows.push(row.clone());
            row
        };
        let seq = row.seq;
        let _ = self.shared.tx.send((chat.to_owned(), row));
        seq
    }

    pub fn chat_sockets(&self, chat: &str) -> usize {
        self.shared
            .sockets
            .lock()
            .unwrap()
            .get(chat)
            .copied()
            .unwrap_or(0)
    }

    pub fn relay_host_connected(&self, device: &str) -> bool {
        self.shared
            .relays
            .lock()
            .unwrap()
            .get(device)
            .is_some_and(|r| r.host.is_some())
    }

    pub fn refuse_chat_joins(&self, refuse: bool) {
        *self.shared.refuse_chat.lock().unwrap() = refuse;
    }
}

impl Drop for MockEdge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn proxy(client: Ws, upstream: &str) {
    let Ok((server, _)) = tokio_tungstenite::connect_async(upstream).await else {
        return;
    };
    let (mut client_tx, mut client_rx) = client.split();
    let (mut server_tx, mut server_rx) = server.split();
    let up = async {
        while let Some(Ok(message)) = client_rx.next().await {
            if server_tx.send(message).await.is_err() {
                return;
            }
        }
        let _ = server_tx.close().await;
    };
    let down = async {
        while let Some(Ok(message)) = server_rx.next().await {
            if client_tx.send(message).await.is_err() {
                return;
            }
        }
        let _ = client_tx.close().await;
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

/// `GET /chat2/{id}/rows` and `/checkpoint`. True: it was a plain GET, and it's done.
async fn serve_plain_get(stream: &mut tokio::net::TcpStream, shared: &Shared) -> bool {
    let mut peeked = [0u8; 2048];
    let Ok(n) = stream.peek(&mut peeked).await else {
        return false;
    };
    let head = String::from_utf8_lossy(&peeked[..n]).to_ascii_lowercase();
    if !head.starts_with("get ") || head.contains("upgrade: websocket") {
        return false;
    }
    let line = String::from_utf8_lossy(&peeked[..n]);
    let path = line.split_whitespace().nth(1).unwrap_or_default().to_owned();
    let (path, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf).await;
    let (status, body) = match parts.as_slice() {
        ["chat2", chat, "checkpoint"] => {
            let found = shared
                .rooms
                .lock()
                .unwrap()
                .get(*chat)
                .and_then(|r| r.checkpoint.clone());
            match found {
                Some(checkpoint) => {
                    *shared.downloads.lock().unwrap().entry(chat.to_string()).or_default() += 1;
                    (200, checkpoint.bytes)
                }
                None => (404, Vec::new()),
            }
        }
        ["chat2", chat, "rows"] => {
            shared
                .pulls
                .lock()
                .unwrap()
                .entry(chat.to_string())
                .or_default()
                .push(query.to_owned());
            let after = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("after="))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let (state, rows, head_seq) = {
                let rooms = shared.rooms.lock().unwrap();
                let room = rooms.get(*chat);
                let head_seq = room.map_or(0, |r| r.rows.len() as u64);
                let rows: Vec<Row> = room
                    .map(|r| r.rows.iter().filter(|row| row.seq > after).cloned().collect())
                    .unwrap_or_default();
                (state_frame(room, head_seq, false), rows, head_seq)
            };
            let mut frames = vec![state];
            frames.extend(rows.iter().map(row_frame));
            frames.push(chat_frames::encode(frame_type::ROWS_DONE, &json!({ "headSeq": head_seq }), &[]));
            let mut body = Vec::new();
            for frame in frames {
                body.extend_from_slice(&(frame.len() as u32).to_le_bytes());
                body.extend_from_slice(&frame);
            }
            (200, body)
        }
        _ => (404, Vec::new()),
    };
    let head = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    true
}

/// `POST /chat2/{id}/rows`: stored, as the real room stores it.
fn http_push(headers: &str, body: &[u8], shared: &Shared) -> Option<Vec<u8>> {
    let line = headers.lines().next()?;
    let target = line.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let ["chat2", chat, "rows"] = parts.as_slice() else {
        return None;
    };
    shared
        .http_pushes
        .lock()
        .unwrap()
        .entry(chat.to_string())
        .or_default()
        .push(query.to_owned());
    let field = |name: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(name))
            .unwrap_or_default()
            .to_owned()
    };
    let batch = field("batchId=");
    let device = field("device=");
    let seq = {
        let mut rooms = shared.rooms.lock().unwrap();
        let room = rooms.entry(chat.to_string()).or_default();
        match room.batches.get(&batch) {
            Some(seq) => *seq,
            None => {
                let seq = room.rows.len() as u64 + 1;
                let row = Row { seq, device, batch_id: batch.clone(), bytes: body.to_vec() };
                room.rows.push(row.clone());
                room.batches.insert(batch.clone(), seq);
                let _ = shared.tx.send((chat.to_string(), row));
                seq
            }
        }
    };
    let reply = json!({ "batchId": batch, "seq": seq, "dup": false }).to_string();
    Some(
        format!(
            "HTTP/1.1 200 Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        )
        .into_bytes(),
    )
}

fn state_frame(room: Option<&Room>, head_seq: u64, relay: bool) -> Vec<u8> {
    let checkpoint = room.and_then(|r| r.checkpoint.as_ref());
    let mut state = json!({
        "headSeq": head_seq,
        "seqFloor": checkpoint.map_or(0, |c| c.seq),
        "checkpointSeq": checkpoint.map_or(0, |c| c.seq),
        "checkpointSize": checkpoint.map_or(0, |c| c.bytes.len()),
        "rowCount": head_seq,
        "rowBytes": 0,
    });
    if relay {
        state["relay"] = json!(1);
        state["legacyPeers"] = json!(0);
    }
    chat_frames::encode(
        frame_type::STATE,
        &state,
        checkpoint.map_or(&[][..], |c| &c.frontier),
    )
}

fn row_frame(row: &Row) -> Vec<u8> {
    chat_frames::encode(
        frame_type::ROW,
        &json!({ "seq": row.seq, "device": row.device, "batchId": row.batch_id }),
        &row.bytes,
    )
}

async fn serve_chat(ws: Ws, chat: String, shared: Arc<Shared>) {
    *shared
        .sockets
        .lock()
        .unwrap()
        .entry(chat.clone())
        .or_default() += 1;
    let (mut tx, mut rx) = ws.split();
    let mut broadcasts = shared.tx.subscribe();
    let mut relayed = shared.relay_tx.subscribe();
    let mut kicks = shared.kick_tx.subscribe();
    let mut device = String::new();
    let mut joined = false;
    // This socket's hello asked for the layer, and the room has it.
    let mut relay = false;
    loop {
        tokio::select! {
            message = rx.next() => {
                let Some(Ok(message)) = message else { break };
                let bytes = match message {
                    Message::Binary(bytes) => bytes.to_vec(),
                    Message::Text(text) if text.as_str() == "ping" => {
                        let _ = tx.send(Message::Text("pong".into())).await;
                        continue;
                    }
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Some(frame) = chat_frames::decode(&bytes) else { continue };
                let head = |shared: &Shared| {
                    shared.rooms.lock().unwrap().get(&chat).map_or(0, |r| r.rows.len() as u64)
                };
                match frame.kind {
                    frame_type::HELLO => {
                        device = frame.header["device"].as_str().unwrap_or_default().to_owned();
                        relay = *shared.relay_layer.lock().unwrap() && frame.header["relay"] == 1;
                        let head_seq = head(&shared);
                        let state = {
                            let mut rooms = shared.rooms.lock().unwrap();
                            let room = rooms.entry(chat.clone()).or_default();
                            room.hellos.push(frame.header.clone());
                            if frame.header["gen"] == 3 {
                                room.wakes += 1;
                            }
                            state_frame(Some(room), head_seq, relay)
                        };
                        if tx.send(Message::Binary(state)).await.is_err() {
                            break;
                        }
                    }
                    frame_type::ROWS_REQ => {
                        let after = frame.header["after"].as_u64().unwrap_or(0);
                        let exclude_own = frame.header["excludeOwn"].as_bool().unwrap_or(false);
                        let rows: Vec<Row> = shared
                            .rooms
                            .lock()
                            .unwrap()
                            .get(&chat)
                            .map(|r| r.rows.iter().filter(|row| row.seq > after).cloned().collect())
                            .unwrap_or_default();
                        let head_seq = head(&shared);
                        let mut failed = false;
                        for row in rows {
                            if exclude_own && row.device == device {
                                continue;
                            }
                            if tx.send(Message::Binary(row_frame(&row))).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break;
                        }
                        let done = chat_frames::encode(frame_type::ROWS_DONE, &json!({ "headSeq": head_seq }), &[]);
                        if tx.send(Message::Binary(done)).await.is_err() {
                            break;
                        }
                        joined = true;
                    }
                    frame_type::PUSH => {
                        let batch_id = frame.header["batchId"].as_str().unwrap_or_default().to_owned();
                        let (seq, dup, row) = {
                            let mut rooms = shared.rooms.lock().unwrap();
                            let room = rooms.entry(chat.clone()).or_default();
                            match room.batches.get(&batch_id) {
                                Some(seq) => (*seq, true, None),
                                None => {
                                    let seq = room.rows.len() as u64 + 1;
                                    let row = Row {
                                        seq,
                                        device: device.clone(),
                                        batch_id: batch_id.clone(),
                                        bytes: frame.payload.clone(),
                                    };
                                    room.rows.push(row.clone());
                                    room.batches.insert(batch_id.clone(), seq);
                                    (seq, false, Some(row))
                                }
                            }
                        };
                        let ack = chat_frames::encode(
                            frame_type::ACK,
                            &json!({ "batchId": batch_id, "seq": seq, "dup": dup }),
                            &[],
                        );
                        if tx.send(Message::Binary(ack)).await.is_err() {
                            break;
                        }
                        if let Some(row) = row {
                            let _ = shared.tx.send((chat.clone(), row));
                        }
                    }
                    frame_type::PROBE => {
                        let ok = chat_frames::encode(frame_type::PROBE_OK, &json!({ "headSeq": head(&shared) }), &[]);
                        if tx.send(Message::Binary(ok)).await.is_err() {
                            break;
                        }
                    }
                    frame_type::RELAY | frame_type::ANNOUNCE | frame_type::SYNC_REQ | frame_type::SYNC_DATA if relay => {
                        let mut header = frame.header.clone();
                        let to = header.get("to").and_then(|v| v.as_str()).map(str::to_owned);
                        let skip = header
                            .get("skip")
                            .and_then(|v| v.as_array())
                            .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                            .unwrap_or_default();
                        if let Some(map) = header.as_object_mut() {
                            map.remove("to");
                            map.remove("skip");
                            map.insert("device".into(), json!(device));
                        }
                        let _ = shared.relay_tx.send(RelayOut {
                            chat: chat.clone(),
                            from: device.clone(),
                            to,
                            skip,
                            frame: chat_frames::encode(frame.kind, &header, &frame.payload),
                        });
                    }
                    frame_type::ENQUEUE if relay => {
                        let batch_id = frame.header["batchId"].as_str().unwrap_or_default().to_owned();
                        let dup = {
                            let mut rooms = shared.rooms.lock().unwrap();
                            let room = rooms.entry(chat.clone()).or_default();
                            let dup = room.queue.iter().any(|row| row.batch_id == batch_id);
                            if !dup {
                                room.queue.push(Row {
                                    seq: 0,
                                    device: device.clone(),
                                    batch_id: batch_id.clone(),
                                    bytes: frame.payload.clone(),
                                });
                            }
                            dup
                        };
                        if !dup {
                            let _ = shared.relay_tx.send(RelayOut {
                                chat: chat.clone(),
                                from: device.clone(),
                                to: None,
                                skip: Vec::new(),
                                frame: chat_frames::encode(
                                    frame_type::RELAY,
                                    &json!({ "device": device, "queued": true, "batchId": batch_id }),
                                    &frame.payload,
                                ),
                            });
                        }
                        let ack = chat_frames::encode(frame_type::QUEUED, &json!({ "batchId": batch_id, "dup": dup }), &[]);
                        if tx.send(Message::Binary(ack)).await.is_err() {
                            break;
                        }
                    }
                    frame_type::QUEUE_REQ if relay => {
                        let rows: Vec<Row> = shared
                            .rooms
                            .lock()
                            .unwrap()
                            .get(&chat)
                            .map(|r| r.queue.clone())
                            .unwrap_or_default();
                        let count = rows.len();
                        let mut failed = false;
                        for row in rows {
                            let out = chat_frames::encode(
                                frame_type::RELAY,
                                &json!({ "device": row.device, "queued": true, "batchId": row.batch_id }),
                                &row.bytes,
                            );
                            if tx.send(Message::Binary(out)).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        let done = chat_frames::encode(frame_type::QUEUE_DONE, &json!({ "count": count }), &[]);
                        if failed || tx.send(Message::Binary(done)).await.is_err() {
                            break;
                        }
                    }
                    frame_type::DELIVERED if relay => {
                        let ids: Vec<String> = frame.header["batchIds"]
                            .as_array()
                            .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                            .unwrap_or_default();
                        if let Some(room) = shared.rooms.lock().unwrap().get_mut(&chat) {
                            room.queue.retain(|row| !ids.contains(&row.batch_id));
                        }
                    }
                    _ => {}
                }
            }
            out = relayed.recv() => {
                let Ok(out) = out else { continue };
                if !relay || out.chat != chat || out.from == device || out.skip.contains(&device) {
                    continue;
                }
                if out.to.as_ref().is_some_and(|to| *to != device) {
                    continue;
                }
                if tx.send(Message::Binary(out.frame)).await.is_err() {
                    break;
                }
            }
            kicked = kicks.recv() => {
                if kicked.is_ok_and(|kicked| kicked == chat) {
                    let _ = tx.close().await;
                    break;
                }
            }
            broadcast = broadcasts.recv() => {
                let Ok((room, row)) = broadcast else { continue };
                if room != chat || !joined || row.device == device {
                    continue;
                }
                if tx.send(Message::Binary(row_frame(&row))).await.is_err() {
                    break;
                }
            }
        }
    }
    if let Some(count) = shared.sockets.lock().unwrap().get_mut(&chat) {
        *count = count.saturating_sub(1);
    }
}

fn relay_error(code: &str) -> Vec<u8> {
    json!({ "error": code }).to_string().into_bytes()
}

/// DeviceRoom semantics (edge/src/device-room.ts), as in zeron-rpc's fake.
async fn serve_relay(ws: Ws, device: String, query: HashMap<String, String>, shared: Arc<Shared>) {
    use zeron_rpc::device_room::{
        CLIENT_CLOSED, CLIENT_GONE, HOST_CLOSED, HOST_OFFLINE, RELAY_KIND,
    };
    use zeron_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};

    let is_host = query.get("role").map(String::as_str) == Some("host");
    let conn_id = query
        .get("connId")
        .cloned()
        .unwrap_or_else(|| "anon".into());
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Out>();
    {
        let mut relays = shared.relays.lock().unwrap();
        let st = relays.entry(device.clone()).or_default();
        if is_host {
            if let Some(old) = st.host.take() {
                let _ = old.send(Out::Close);
            }
            st.host = Some(tx.clone());
        } else {
            st.clients.insert(conn_id.clone(), tx.clone());
        }
    }
    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Frame(bytes) => {
                    if sink.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                Out::Close => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });
    while let Some(message) = stream.next().await {
        let bytes = match message {
            Ok(Message::Binary(bytes)) => bytes,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let Ok((header, payload)) = decode_device_frame(&bytes) else {
            break;
        };
        let mut relays = shared.relays.lock().unwrap();
        let st = relays.entry(device.clone()).or_default();
        if !is_host {
            match &st.host {
                Some(host) => {
                    let mut routed = DeviceFrameHeader::new(header.s, header.k);
                    routed.from = Some(conn_id.clone());
                    let _ = host.send(Out::Frame(encode_device_frame(&routed, &payload).unwrap()));
                }
                None => {
                    let bounce = DeviceFrameHeader::new(header.s, RELAY_KIND);
                    let _ = tx.send(Out::Frame(
                        encode_device_frame(&bounce, &relay_error(HOST_OFFLINE)).unwrap(),
                    ));
                }
            }
            continue;
        }
        let Some(to) = header.to else { continue };
        match st.clients.get(&to) {
            Some(client) => {
                let stripped = DeviceFrameHeader::new(header.s, header.k);
                let _ = client.send(Out::Frame(
                    encode_device_frame(&stripped, &payload).unwrap(),
                ));
            }
            None => {
                let bounce = DeviceFrameHeader::new(header.s, RELAY_KIND).with_to(to);
                let _ = tx.send(Out::Frame(
                    encode_device_frame(&bounce, &relay_error(CLIENT_GONE)).unwrap(),
                ));
            }
        }
    }
    {
        let mut relays = shared.relays.lock().unwrap();
        let st = relays.entry(device.clone()).or_default();
        if is_host {
            if st.host.as_ref().is_some_and(|h| h.same_channel(&tx)) {
                st.host = None;
            }
            for client in st.clients.values() {
                let header = DeviceFrameHeader::new("", RELAY_KIND);
                let _ = client.send(Out::Frame(
                    encode_device_frame(&header, &relay_error(HOST_CLOSED)).unwrap(),
                ));
            }
        } else {
            st.clients.remove(&conn_id);
            if let Some(host) = &st.host {
                let mut header = DeviceFrameHeader::new("", RELAY_KIND);
                header.from = Some(conn_id.clone());
                let _ = host.send(Out::Frame(
                    encode_device_frame(&header, &relay_error(CLIENT_CLOSED)).unwrap(),
                ));
            }
        }
    }
    writer.abort();
}
