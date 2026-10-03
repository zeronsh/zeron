//! In-process registry server speaking the same JSON WS protocol as
//! `edge/src/registry-room.ts`, built on the SAME merge fn the client uses
//! (`zeron_doc::apply_op`). Test infrastructure only (`mock-server` feature):
//! zeron-sync's client tests and zeron-engine's two-engine integration tests
//! run against this; TS↔Rust interop is proven separately against a real DO
//! by the `--ignored` live-edge tests and scripts/e2e-smoke.sh.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use zeron_doc::{RegistryRow, RowOp, apply_op};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Shared {
    rows: HashMap<(String, String), RegistryRow>,
    seq: u64,
    gc_floor: u64,
    presence: HashMap<String, i64>,
    /// Pushes accepted per device (assertion surface for tests).
    pushes: HashMap<String, u64>,
}

/// A running mock registry server on a local TCP port.
pub struct MockRegistryServer {
    addr: std::net::SocketAddr,
    state: Arc<Mutex<Shared>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockRegistryServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(Shared::default()));
        let (tx, _) = broadcast::channel(1024);
        let accept_state = state.clone();
        let accept_tx = tx.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = accept_state.clone();
                let bcast = accept_tx.clone();
                tokio::spawn(async move {
                    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    serve(ws, state, bcast).await;
                });
            }
        });
        Self { addr, state, task }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub fn seq(&self) -> u64 {
        lock(&self.state).seq
    }

    pub fn row(&self, kind: &str, id: &str) -> Option<RegistryRow> {
        lock(&self.state)
            .rows
            .get(&(kind.to_string(), id.to_string()))
            .cloned()
    }

    pub fn row_count(&self) -> usize {
        lock(&self.state).rows.len()
    }

    pub fn pushes(&self, device: &str) -> u64 {
        lock(&self.state).pushes.get(device).copied().unwrap_or(0)
    }

    /// Simulate an operator wipe (`POST /reset`): clients re-seed on rejoin.
    pub fn reset(&self) {
        let mut state = lock(&self.state);
        state.rows.clear();
        state.seq = 0;
        state.gc_floor = 0;
    }

    /// Simulate a tombstone-GC horizon jump: cursors below `floor` full-resync.
    pub fn set_gc_floor(&self, floor: u64) {
        lock(&self.state).gc_floor = floor;
    }
}

impl Drop for MockRegistryServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    state: Arc<Mutex<Shared>>,
    bcast: broadcast::Sender<String>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut rx = bcast.subscribe();
    let mut device = String::new();
    let mut ready = false;
    loop {
        tokio::select! {
            frame = stream.next() => {
                let text = match frame {
                    Some(Ok(WsMessage::Text(text))) => text.to_string(),
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => continue,
                };
                if text == "ping" {
                    if sink.send(WsMessage::Text("pong".into())).await.is_err() {
                        return;
                    }
                    continue;
                }
                let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                    return;
                };
                match frame.get("t").and_then(Value::as_str) {
                    Some("hello") => {
                        let cursor = frame.get("cursor").and_then(Value::as_u64);
                        device = frame
                            .get("device")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        ready = true;
                        let reply = {
                            let state = lock(&state);
                            let full = match cursor {
                                None => true,
                                Some(c) => c < state.gc_floor || c > state.seq,
                            };
                            let rows: Vec<&RegistryRow> = state
                                .rows
                                .values()
                                .filter(|r| full || r.seq > cursor.unwrap_or(0))
                                .collect();
                            json!({
                                "t": "state",
                                "seq": state.seq,
                                "full": full,
                                "gcFloor": state.gc_floor,
                                "rows": rows,
                                "presence": state.presence, "presenceNow": zeron_proto::time::now_ms(),
                            })
                            .to_string()
                        };
                        if sink.send(WsMessage::Text(reply)).await.is_err() {
                            return;
                        }
                    }
                    Some("push") if ready => {
                        let batch = frame
                            .get("batch")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let Ok(ops) = serde_json::from_value::<Vec<RowOp>>(
                            frame.get("ops").cloned().unwrap_or(Value::Null),
                        ) else {
                            let _ = sink
                                .send(WsMessage::Text(
                                    json!({"t":"error","code":"invalid_op","message":"bad ops"})
                                        .to_string(),
                                ))
                                .await;
                            continue;
                        };
                        let (ack, rows_frame) = {
                            let mut state = lock(&state);
                            let next_seq = state.seq + 1;
                            let mut touched: Vec<RegistryRow> = Vec::new();
                            let mut applied = 0u64;
                            for op in &ops {
                                let key = (op.kind.clone(), op.id.clone());
                                let before = touched
                                    .iter()
                                    .find(|r| r.kind == op.kind && r.id == op.id)
                                    .or_else(|| state.rows.get(&key));
                                let (next, changed) = apply_op(before, op);
                                let Some(mut next) = next else { continue };
                                if !changed {
                                    continue;
                                }
                                applied += 1;
                                next.seq = next_seq;
                                touched.retain(|r| !(r.kind == next.kind && r.id == next.id));
                                touched.push(next);
                            }
                            if applied > 0 {
                                for row in &touched {
                                    state
                                        .rows
                                        .insert((row.kind.clone(), row.id.clone()), row.clone());
                                }
                                state.seq = next_seq;
                            }
                            *state.pushes.entry(device.clone()).or_default() += 1;
                            let seq = state.seq;
                            let ack =
                                json!({"t":"ack","batch":batch,"seq":seq,"applied":applied})
                                    .to_string();
                            let rows_frame = (applied > 0)
                                .then(|| json!({"t":"rows","seq":seq,"rows":touched}).to_string());
                            (ack, rows_frame)
                        };
                        // Same ordering as the DO: rows to everyone (this
                        // socket included, via the broadcast), then the ack.
                        if let Some(rows) = rows_frame {
                            let _ = bcast.send(rows);
                            // Drain our own broadcast copy first so ordering
                            // matches (rows before ack) on this socket too.
                            while let Ok(text) = rx.try_recv() {
                                if sink.send(WsMessage::Text(text)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        if sink.send(WsMessage::Text(ack)).await.is_err() {
                            return;
                        }
                    }
                    Some("presence") if ready => {
                        let at = zeron_proto::time::now_ms();
                        lock(&state).presence.insert(device.clone(), at);
                        let _ = bcast.send(
                            json!({"t":"presence","device":device,"at":at}).to_string(),
                        );
                    }
                    Some("probe") => {
                        let seq = lock(&state).seq;
                        if sink
                            .send(WsMessage::Text(
                                json!({"t":"probe-ok","seq":seq}).to_string(),
                            ))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    _ => {
                        let _ = sink
                            .send(WsMessage::Text(
                                json!({"t":"error","code":"bad_frame","message":"unknown"})
                                    .to_string(),
                            ))
                            .await;
                    }
                }
            }
            text = rx.recv() => {
                match text {
                    Ok(text) => {
                        if ready && sink.send(WsMessage::Text(text)).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}
