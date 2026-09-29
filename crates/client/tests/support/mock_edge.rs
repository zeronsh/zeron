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
        });
        let accept_shared = shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let shared = accept_shared.clone();
                let registry_url = registry_url.clone();
                tokio::spawn(async move {
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
    let mut device = String::new();
    let mut joined = false;
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
                        let head_seq = head(&shared);
                        let state = chat_frames::encode(
                            frame_type::STATE,
                            &json!({
                                "headSeq": head_seq, "seqFloor": 0, "checkpointSeq": 0,
                                "checkpointSize": 0, "rowCount": head_seq, "rowBytes": 0,
                            }),
                            &[],
                        );
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
                    _ => {}
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
