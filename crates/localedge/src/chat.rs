//! chat2 rooms — the port of `edge/src/chat-room.ts` + `chat-log.ts`
//! (docs/chat2-sync.md workstream B): an append-only log of opaque Loro
//! update rows per chat, one client-built checkpoint blob, host-published
//! sidecars, and a live relay. No CRDT semantics live here, exactly like the
//! DO: every byte is opaque.
//!
//! Single-tenant: the DO's claim-on-first-join owner check has no second
//! user to exclude, so it is omitted.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use hyper::Request;
use hyper::body::Incoming;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use zeron_sync::chat_frames::{self, frame_type};

use crate::http::{self, Peer, Reply, Ws, now_ms};
use crate::{Edge, State};

/// Per-row byte cap (chat-log.ts `MAX_ROW_BYTES`).
pub(crate) const MAX_ROW_BYTES: usize = 1024 * 1024;
/// Inbound frame budget: one pushed row (+ header slack).
pub(crate) const MAX_FRAME_BYTES: usize = MAX_ROW_BYTES + 8192;
/// Host-published sidecar budget (tail JSON / diff payload).
const MAX_SIDECAR_BYTES: usize = 4 * 1024 * 1024;
/// Checkpoint upload cap (chat-room.ts `MAX_CHECKPOINT_BYTES`).
const MAX_CHECKPOINT_BYTES: usize = 32 * 1024 * 1024;
/// `GET /rows` buffers its body; past this it ends without `rowsDone`
/// (pagination by truncation — the client's cursor resumes).
const ROWS_BODY_CAP: usize = 4 * 1024 * 1024;
/// Presence beats older than this are swept before relay/stats.
const PRESENCE_TTL_MS: i64 = 30_000;
/// Per-device push quota, rolling window, memory-only — it contains a
/// runaway client loop, it does not meter honest traffic.
const QUOTA_WINDOW_MS: i64 = 60_000;
const QUOTA_MAX_PUSHES: u32 = 300;
const QUOTA_MAX_BYTES: usize = 8 * 1024 * 1024;

const CHECKPOINT_BLOB: &str = "checkpoint";
const FRONTIER_BLOB: &str = "checkpoint-frontier";

struct ChatSocket {
    peer: Peer,
    device: String,
    /// Set once a valid hello established the session.
    ready: bool,
}

struct QuotaWindow {
    since: i64,
    pushes: u32,
    bytes: usize,
}

/// A room's memory-only state: sockets, presence, quotas, attribution.
#[derive(Default)]
pub(crate) struct ChatLive {
    sockets: Vec<ChatSocket>,
    presence: HashMap<String, i64>,
    quotas: HashMap<String, QuotaWindow>,
    push_outcomes: BTreeMap<String, PushOutcome>,
}

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PushOutcome {
    ok: u64,
    rejected: u64,
    last_ok_at: i64,
}

impl ChatLive {
    /// No sockets left: the room's memory-only state (presence, quotas,
    /// attribution) goes with it, as it does when a DO is evicted.
    fn is_idle(&self) -> bool {
        self.sockets.is_empty()
    }

    fn sweep_presence(&mut self) {
        let horizon = now_ms() - PRESENCE_TTL_MS;
        self.presence.retain(|_, at| *at >= horizon);
    }

    /// Rolling per-device quota. True = admitted.
    fn admit_quota(&mut self, device: &str, bytes: usize) -> bool {
        let now = now_ms();
        let key = if device.is_empty() {
            "(unknown)"
        } else {
            device
        };
        let window = self.quotas.entry(key.to_owned()).or_insert(QuotaWindow {
            since: now,
            pushes: 0,
            bytes: 0,
        });
        if now - window.since > QUOTA_WINDOW_MS {
            *window = QuotaWindow {
                since: now,
                pushes: 0,
                bytes: 0,
            };
        }
        window.pushes += 1;
        window.bytes += bytes;
        window.pushes <= QUOTA_MAX_PUSHES && window.bytes <= QUOTA_MAX_BYTES
    }

    fn record_push(&mut self, device: &str, ok: bool) {
        let key = if device.is_empty() {
            "(unknown)"
        } else {
            device
        };
        let entry = self.push_outcomes.entry(key.to_owned()).or_default();
        if ok {
            entry.ok += 1;
            entry.last_ok_at = now_ms();
        } else {
            entry.rejected += 1;
        }
    }

    /// Relay one appended row to every ready socket except `skip`.
    fn relay_row(&self, skip: Option<u64>, seq: u64, device: &str, batch_id: &str, bytes: &[u8]) {
        let frame = row_frame(seq, device, batch_id, bytes);
        for socket in &self.sockets {
            if !socket.ready || Some(socket.peer.id) == skip {
                continue;
            }
            socket.peer.send_binary(frame.clone());
        }
    }
}

// ── log storage (chat-log.ts) ───────────────────────────────────────────────

fn get_meta(db: &Connection, chat: &str, key: &str) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT value FROM chat_meta WHERE chat_id = ?1 AND key = ?2",
        params![chat, key],
        |row| row.get(0),
    )
    .optional()
}

fn set_meta(db: &Connection, chat: &str, key: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO chat_meta (chat_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(chat_id, key) DO UPDATE SET value = excluded.value",
        params![chat, key, value],
    )?;
    Ok(())
}

fn meta_u64(db: &Connection, chat: &str, key: &str) -> rusqlite::Result<u64> {
    Ok(get_meta(db, chat, key)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn head_seq(db: &Connection, chat: &str) -> rusqlite::Result<u64> {
    meta_u64(db, chat, "headSeq")
}

fn get_blob(db: &Connection, chat: &str, name: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    db.query_row(
        "SELECT bytes FROM chat_blobs WHERE chat_id = ?1 AND name = ?2",
        params![chat, name],
        |row| row.get(0),
    )
    .optional()
}

fn put_blob(db: &Connection, chat: &str, name: &str, bytes: &[u8]) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO chat_blobs (chat_id, name, bytes) VALUES (?1, ?2, ?3)
         ON CONFLICT(chat_id, name) DO UPDATE SET bytes = excluded.bytes",
        params![chat, name, bytes],
    )?;
    Ok(())
}

enum Append {
    Ok { seq: u64, dup: bool },
    Rejected(&'static str),
}

/// Append one update row. `batch_id` UNIQUE dedupes reconnect re-pushes: a
/// replay acks the ORIGINAL seq and appends nothing.
fn append_row(
    db: &mut Connection,
    chat: &str,
    device: &str,
    batch_id: &str,
    bytes: &[u8],
) -> rusqlite::Result<Append> {
    if bytes.is_empty() {
        return Ok(Append::Rejected("empty"));
    }
    if bytes.len() > MAX_ROW_BYTES {
        return Ok(Append::Rejected("too_large"));
    }
    let tx = db.transaction()?;
    let existing: Option<u64> = tx
        .query_row(
            "SELECT seq FROM chat_rows WHERE chat_id = ?1 AND batch_id = ?2",
            params![chat, batch_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(seq) = existing {
        return Ok(Append::Ok { seq, dup: true });
    }
    let seq = head_seq(&tx, chat)? + 1;
    tx.execute(
        "INSERT INTO chat_rows (chat_id, seq, device, batch_id, bytes, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![chat, seq, device, batch_id, bytes, now_ms()],
    )?;
    set_meta(&tx, chat, "headSeq", &seq.to_string())?;
    tx.commit()?;
    Ok(Append::Ok { seq, dup: false })
}

struct LogRow {
    seq: u64,
    device: String,
    batch_id: String,
    bytes: Vec<u8>,
}

/// Rows `seq > after`, in order, optionally excluding one device's writes.
fn rows_after(
    db: &Connection,
    chat: &str,
    after: u64,
    exclude: Option<&str>,
) -> rusqlite::Result<Vec<LogRow>> {
    let mut stmt = db.prepare_cached(
        "SELECT seq, device, batch_id, bytes FROM chat_rows
         WHERE chat_id = ?1 AND seq > ?2 AND (?3 IS NULL OR device != ?3) ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![chat, after, exclude], |row| {
        Ok(LogRow {
            seq: row.get(0)?,
            device: row.get(1)?,
            batch_id: row.get(2)?,
            bytes: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// Commit a client-built checkpoint covering rows `seq <= seq_covered`.
/// Floor-monotonic, never ahead of head; covered rows are pruned in the same
/// transaction so floor and rows can never disagree.
fn commit_checkpoint(
    db: &mut Connection,
    chat: &str,
    seq_covered: u64,
    frontier: &[u8],
    bytes: &[u8],
) -> rusqlite::Result<Result<(u64, usize), &'static str>> {
    if bytes.is_empty() {
        return Ok(Err("empty"));
    }
    let tx = db.transaction()?;
    if seq_covered < meta_u64(&tx, chat, "seqFloor")? {
        return Ok(Err("floor_regression"));
    }
    if seq_covered > head_seq(&tx, chat)? {
        return Ok(Err("ahead_of_head"));
    }
    put_blob(&tx, chat, CHECKPOINT_BLOB, bytes)?;
    put_blob(&tx, chat, FRONTIER_BLOB, frontier)?;
    let pruned = tx.execute(
        "DELETE FROM chat_rows WHERE chat_id = ?1 AND seq <= ?2",
        params![chat, seq_covered],
    )?;
    set_meta(&tx, chat, "seqFloor", &seq_covered.to_string())?;
    set_meta(&tx, chat, "checkpointSeq", &seq_covered.to_string())?;
    set_meta(&tx, chat, "checkpointSize", &bytes.len().to_string())?;
    set_meta(&tx, chat, "checkpointAt", &now_ms().to_string())?;
    tx.commit()?;
    Ok(Ok((seq_covered, pruned)))
}

/// The hello/stats surface (`logStats`).
fn log_stats(db: &Connection, chat: &str) -> rusqlite::Result<Value> {
    let (count, bytes): (u64, u64) = db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(LENGTH(bytes)), 0) FROM chat_rows WHERE chat_id = ?1",
        params![chat],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(json!({
        "headSeq": head_seq(db, chat)?,
        "seqFloor": meta_u64(db, chat, "seqFloor")?,
        "rowCount": count,
        "rowBytes": bytes,
        "checkpointSeq": meta_u64(db, chat, "checkpointSeq")?,
        "checkpointSize": meta_u64(db, chat, "checkpointSize")?,
        "checkpointAt": meta_u64(db, chat, "checkpointAt")?,
    }))
}

/// The `state` frame: stats (minus `checkpointAt`) + the frontier payload.
fn state_frame(db: &Connection, chat: &str) -> rusqlite::Result<Vec<u8>> {
    let mut stats = log_stats(db, chat)?;
    if let Some(stats) = stats.as_object_mut() {
        stats.remove("checkpointAt");
    }
    let frontier = get_blob(db, chat, FRONTIER_BLOB)?.unwrap_or_default();
    Ok(chat_frames::encode(frame_type::STATE, &stats, &frontier))
}

fn row_frame(seq: u64, device: &str, batch_id: &str, bytes: &[u8]) -> Vec<u8> {
    chat_frames::encode(
        frame_type::ROW,
        &json!({ "seq": seq, "device": device, "batchId": batch_id }),
        bytes,
    )
}

fn error_frame(code: &str, message: &str, batch_id: Option<&str>) -> Vec<u8> {
    let mut header = json!({ "code": code, "message": message });
    if let Some(batch_id) = batch_id {
        header["batchId"] = json!(batch_id);
    }
    chat_frames::encode(frame_type::ERROR, &header, &[])
}

fn known_frame_type(kind: u8) -> bool {
    (frame_type::HELLO..=frame_type::ERROR).contains(&kind)
}

// ── WebSocket protocol ──────────────────────────────────────────────────────

pub(crate) async fn serve_ws(edge: Arc<Edge>, chat: String, device: String, ws: Ws) {
    let (peer, rx) = edge.new_peer();
    edge.with_state(|state| {
        state
            .chats
            .entry(chat.clone())
            .or_default()
            .sockets
            .push(ChatSocket {
                peer: peer.clone(),
                device,
                ready: false,
            });
    });
    http::pump(ws, &peer, rx, &edge.shutdown, |message| {
        let bytes = match message {
            Message::Binary(bytes) => bytes,
            _ => {
                peer.close(1003, "binary frames only");
                return false;
            }
        };
        if bytes.len() > MAX_FRAME_BYTES {
            peer.close(1009, "frame too large");
            return false;
        }
        edge.with_state(|state| on_frame(state, &chat, &peer, &bytes));
        true
    })
    .await;
    edge.with_state(|state| {
        if let Some(room) = state.chats.get_mut(&chat) {
            room.sockets.retain(|s| s.peer.id != peer.id);
            if room.is_idle() {
                state.chats.remove(&chat);
            }
        }
    });
}

fn on_frame(state: &mut State, chat: &str, peer: &Peer, bytes: &[u8]) {
    let frame = chat_frames::decode(bytes).filter(|f| known_frame_type(f.kind));
    let Some(frame) = frame else {
        // A corrupt frame from a flaky link must not cost a reconnect cycle.
        peer.send_binary(error_frame("bad_frame", "malformed frame", None));
        return;
    };
    let result = match frame.kind {
        frame_type::HELLO => on_hello(state, chat, peer, &frame.header),
        frame_type::ROWS_REQ => on_rows_req(state, chat, peer, &frame.header),
        frame_type::PUSH => on_push(state, chat, peer, &frame.header, &frame.payload),
        frame_type::PRESENCE => {
            on_presence(state, chat, peer, &frame.header, &frame.payload);
            Ok(())
        }
        frame_type::PROBE => head_seq(&state.db, chat).map(|head| {
            peer.send_binary(chat_frames::encode(
                frame_type::PROBE_OK,
                &json!({ "headSeq": head }),
                &[],
            ));
        }),
        other => {
            peer.send_binary(error_frame(
                "bad_frame",
                &format!("unexpected type {other}"),
                None,
            ));
            Ok(())
        }
    };
    if let Err(err) = result {
        tracing::error!(chat, error = %err, "local edge: chat2 storage failed");
        peer.send_binary(error_frame("storage", "storage failure", None));
    }
}

fn socket_mut<'a>(state: &'a mut State, chat: &str, peer: &Peer) -> Option<&'a mut ChatSocket> {
    state
        .chats
        .get_mut(chat)?
        .sockets
        .iter_mut()
        .find(|s| s.peer.id == peer.id)
}

fn on_hello(state: &mut State, chat: &str, peer: &Peer, header: &Value) -> rusqlite::Result<()> {
    if let Some(socket) = socket_mut(state, chat, peer) {
        if let Some(device) = header["device"].as_str().filter(|d| !d.is_empty()) {
            socket.device = device.to_owned();
        }
        socket.ready = true;
    }
    // Metadata + frontier only — the CLIENT decides what to load next.
    peer.send_binary(state_frame(&state.db, chat)?);
    Ok(())
}

fn on_rows_req(state: &mut State, chat: &str, peer: &Peer, header: &Value) -> rusqlite::Result<()> {
    let Some(socket) = socket_mut(state, chat, peer).filter(|s| s.ready) else {
        peer.send_binary(error_frame("hello_first", "rows before hello", None));
        return Ok(());
    };
    let exclude = (header["excludeOwn"] == json!(true)).then(|| socket.device.clone());
    let after = header["after"].as_u64().unwrap_or(0);
    for row in rows_after(&state.db, chat, after, exclude.as_deref())? {
        peer.send_binary(row_frame(row.seq, &row.device, &row.batch_id, &row.bytes));
    }
    peer.send_binary(chat_frames::encode(
        frame_type::ROWS_DONE,
        &json!({ "headSeq": head_seq(&state.db, chat)? }),
        &[],
    ));
    Ok(())
}

fn on_push(
    state: &mut State,
    chat: &str,
    peer: &Peer,
    header: &Value,
    payload: &[u8],
) -> rusqlite::Result<()> {
    let batch_id = header["batchId"].as_str().unwrap_or_default().to_owned();
    let (ready, device) = socket_mut(state, chat, peer)
        .map(|s| (s.ready, s.device.clone()))
        .unwrap_or_default();
    let room = state.chats.entry(chat.to_owned()).or_default();
    // Push errors carry the batchId so clients can RETIRE permanently
    // rejected batches from their replay queues.
    if !ready || batch_id.is_empty() || batch_id.len() > 128 {
        room.record_push(&device, false);
        peer.send_binary(error_frame(
            "bad_push",
            "hello first / malformed push",
            Some(&batch_id),
        ));
        return Ok(());
    }
    if !room.admit_quota(&device, payload.len()) {
        room.record_push(&device, false);
        peer.send_binary(error_frame(
            "quota",
            "per-device push quota exceeded",
            Some(&batch_id),
        ));
        return Ok(());
    }
    let outcome = append_row(&mut state.db, chat, &device, &batch_id, payload)?;
    let room = state.chats.entry(chat.to_owned()).or_default();
    match outcome {
        Append::Rejected(code) => {
            room.record_push(&device, false);
            peer.send_binary(error_frame(
                code,
                &format!("push rejected: {code}"),
                Some(&batch_id),
            ));
        }
        Append::Ok { seq, dup } => {
            room.record_push(&device, true);
            if !dup {
                // Live relay to every OTHER ready socket — the sender has its
                // own bytes; it gets the ack.
                room.relay_row(Some(peer.id), seq, &device, &batch_id, payload);
            }
            peer.send_binary(chat_frames::encode(
                frame_type::ACK,
                &json!({ "batchId": batch_id, "seq": seq, "dup": dup }),
                &[],
            ));
        }
    }
    Ok(())
}

fn on_presence(state: &mut State, chat: &str, peer: &Peer, header: &Value, payload: &[u8]) {
    let Some(socket) = socket_mut(state, chat, peer) else {
        return;
    };
    if !socket.ready || socket.device.is_empty() {
        return;
    }
    let device = socket.device.clone();
    let at = header["at"].as_i64().unwrap_or_else(now_ms);
    let room = state.chats.entry(chat.to_owned()).or_default();
    room.presence.insert(device.clone(), at);
    room.sweep_presence();
    // Broadcast-only relay of the opaque payload; never stored.
    let frame = chat_frames::encode(
        frame_type::PRESENCE,
        &json!({ "device": device, "at": at }),
        payload,
    );
    for socket in &room.sockets {
        if socket.ready && socket.peer.id != peer.id {
            socket.peer.send_binary(frame.clone());
        }
    }
}

// ── HTTP surface ────────────────────────────────────────────────────────────

/// `/chat2/{chat}/{route}` (every route but `ws`).
pub(crate) async fn serve_http(
    edge: &Edge,
    chat: &str,
    route: &str,
    request: Request<Incoming>,
) -> Reply {
    let method = request.method().as_str().to_owned();
    let query = http::query(&request);
    match (route, method.as_str()) {
        ("checkpoint", "POST") => {
            let seq_covered = query.get("seqCovered").and_then(|v| v.parse::<u64>().ok());
            let Some(seq_covered) = seq_covered else {
                return http::error(400, "bad_seq_covered");
            };
            let frontier = http::header(&request, "x-chat2-frontier").unwrap_or("");
            let Some(frontier) = decode_base64(frontier) else {
                return http::error(400, "bad_frontier");
            };
            // A checkpoint that claims to cover rows must name its state:
            // empty stays legal only for the seqCovered-0 seed.
            if frontier.is_empty() && seq_covered > 0 {
                return http::json(
                    &json!({ "error": "bad_frontier", "message": "empty frontier on a content checkpoint" }),
                    400,
                );
            }
            let body = match http::read_body(request.into_body(), MAX_CHECKPOINT_BYTES).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            edge.with_state(|state| {
                match commit_checkpoint(&mut state.db, chat, seq_covered, &frontier, &body) {
                    Ok(Ok((floor, pruned))) => http::json(
                        &json!({ "ok": true, "seqFloor": floor, "pruned": pruned }),
                        200,
                    ),
                    Ok(Err(code)) => http::error(409, code),
                    Err(err) => storage_error(chat, err),
                }
            })
        }
        ("checkpoint", "GET") => {
            let range = parse_range_start(http::header(&request, "range"));
            edge.with_state(|state| checkpoint_get(&state.db, chat, range))
                .unwrap_or_else(|err| storage_error(chat, err))
        }
        ("rows", "GET") => {
            let after = query
                .get("after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let device = query.get("device").cloned().unwrap_or_default();
            let exclude = (query.get("excludeOwn").map(String::as_str) == Some("1")
                && !device.is_empty())
            .then_some(device);
            edge.with_state(|state| rows_pull(&state.db, chat, after, exclude.as_deref()))
                .unwrap_or_else(|err| storage_error(chat, err))
        }
        ("rows", "POST") => {
            let device = query.get("device").cloned().unwrap_or_default();
            let batch_id = query.get("batchId").cloned().unwrap_or_default();
            if batch_id.is_empty() || batch_id.len() > 128 {
                edge.with_state(|state| {
                    state
                        .chats
                        .entry(chat.to_owned())
                        .or_default()
                        .record_push(&device, false)
                });
                return http::error(400, "bad_push");
            }
            let body = match http::read_body(request.into_body(), MAX_ROW_BYTES + 4096).await {
                Ok(body) => body,
                Err(reply) => {
                    edge.with_state(|state| {
                        state
                            .chats
                            .entry(chat.to_owned())
                            .or_default()
                            .record_push(&device, false)
                    });
                    return reply;
                }
            };
            edge.with_state(|state| rows_push(state, chat, &device, &batch_id, &body))
        }
        ("tail" | "diff", "PUT") => {
            let name = sidecar_name(route);
            let content_type = http::header(&request, "content-type")
                .unwrap_or("application/json")
                .to_owned();
            let body = match http::read_body(request.into_body(), MAX_SIDECAR_BYTES).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            edge.with_state(|state| {
                let stored = put_blob(&state.db, chat, name, &body).and_then(|()| {
                    set_meta(&state.db, chat, &format!("{name}-type"), &content_type)
                });
                match stored {
                    Ok(()) => http::json(&json!({ "ok": true, "bytes": body.len() }), 200),
                    Err(err) => storage_error(chat, err),
                }
            })
        }
        ("tail" | "diff", "GET") => {
            let name = sidecar_name(route);
            edge.with_state(|state| {
                let bytes = get_blob(&state.db, chat, name)?;
                let content_type = get_meta(&state.db, chat, &format!("{name}-type"))?;
                Ok(match bytes {
                    None => http::error(404, "not_found"),
                    Some(bytes) => http::reply(
                        200,
                        &[(
                            "content-type",
                            content_type.as_deref().unwrap_or("application/json"),
                        )],
                        bytes,
                    ),
                })
            })
            .unwrap_or_else(|err| storage_error(chat, err))
        }
        ("stats", "GET") => edge.with_state(|state| {
            let stats = match log_stats(&state.db, chat) {
                Ok(stats) => stats,
                Err(err) => return storage_error(chat, err),
            };
            let room = state.chats.entry(chat.to_owned()).or_default();
            room.sweep_presence();
            let mut body = stats;
            body["connectedSockets"] = json!(room.sockets.len());
            body["presence"] = json!(room.presence);
            body["pushOutcomes"] = json!(room.push_outcomes);
            body["lastBackupSeq"] = json!(0);
            http::json(&body, 200)
        }),
        ("reset", "POST") => edge.with_state(|state| {
            // Operator wipe; the host re-seeds via checkpoint on its next
            // hello (`headSeq < cursor`).
            let wiped = (|| -> rusqlite::Result<()> {
                let tx = state.db.transaction()?;
                tx.execute("DELETE FROM chat_rows WHERE chat_id = ?1", params![chat])?;
                tx.execute("DELETE FROM chat_meta WHERE chat_id = ?1", params![chat])?;
                tx.execute("DELETE FROM chat_blobs WHERE chat_id = ?1", params![chat])?;
                tx.commit()
            })();
            if let Err(err) = wiped {
                return storage_error(chat, err);
            }
            if let Some(room) = state.chats.get(chat) {
                for socket in &room.sockets {
                    socket.peer.close(4410, "chat room reset");
                }
            }
            http::json(&json!({ "ok": true }), 200)
        }),
        _ => http::error(404, "not found"),
    }
}

fn sidecar_name(route: &str) -> &'static str {
    if route == "tail" {
        "sidecar-tail"
    } else {
        "sidecar-diff"
    }
}

fn storage_error(chat: &str, err: rusqlite::Error) -> Reply {
    tracing::error!(chat, error = %err, "local edge: chat2 storage failed");
    http::error(500, "storage")
}

/// Range-resumable checkpoint read (`bytes=N-` only; anything else → 200).
fn checkpoint_get(db: &Connection, chat: &str, range: Option<usize>) -> rusqlite::Result<Reply> {
    let Some(bytes) = get_blob(db, chat, CHECKPOINT_BLOB)? else {
        return Ok(http::error(404, "not_found"));
    };
    if let Some(start) = range
        && start >= bytes.len()
    {
        return Ok(http::reply(
            416,
            &[("content-range", &format!("bytes */{}", bytes.len()))],
            Vec::new(),
        ));
    }
    let seq = meta_u64(db, chat, "checkpointSeq")?.to_string();
    let total = bytes.len();
    let (status, body, content_range) = match range {
        Some(start) => (
            206,
            bytes[start..].to_vec(),
            Some(format!("bytes {start}-{}/{total}", total - 1)),
        ),
        None => (200, bytes, None),
    };
    let length = body.len().to_string();
    let mut headers = vec![
        ("content-type", "application/octet-stream"),
        ("content-length", length.as_str()),
        ("accept-ranges", "bytes"),
        ("x-chat2-checkpoint-seq", seq.as_str()),
    ];
    if let Some(content_range) = &content_range {
        headers.push(("content-range", content_range));
    }
    Ok(http::reply(status, &headers, body))
}

/// `GET /rows`: u32-LE length-prefixed frames — state, rows after `after`,
/// rowsDone — byte-identical to the WS encoding.
fn rows_pull(
    db: &Connection,
    chat: &str,
    after: u64,
    exclude: Option<&str>,
) -> rusqlite::Result<Reply> {
    let mut frames = vec![state_frame(db, chat)?];
    let mut body_bytes = 4 + frames[0].len();
    let mut truncated = false;
    for row in rows_after(db, chat, after, exclude)? {
        let frame = row_frame(row.seq, &row.device, &row.batch_id, &row.bytes);
        body_bytes += 4 + frame.len();
        if body_bytes > ROWS_BODY_CAP {
            truncated = true;
            break;
        }
        frames.push(frame);
    }
    if !truncated {
        frames.push(chat_frames::encode(
            frame_type::ROWS_DONE,
            &json!({ "headSeq": head_seq(db, chat)? }),
            &[],
        ));
    }
    let mut body = Vec::with_capacity(frames.iter().map(|f| 4 + f.len()).sum());
    for frame in frames {
        body.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        body.extend_from_slice(&frame);
    }
    Ok(http::reply(
        200,
        &[("content-type", "application/octet-stream")],
        body,
    ))
}

/// `POST /rows` — the WS push's batchId-deduped HTTPS twin; relays to every
/// ready socket (a same-device socket re-imports as a Loro no-op).
fn rows_push(state: &mut State, chat: &str, device: &str, batch_id: &str, payload: &[u8]) -> Reply {
    let room = state.chats.entry(chat.to_owned()).or_default();
    if !room.admit_quota(device, payload.len()) {
        room.record_push(device, false);
        return http::error(429, "quota");
    }
    let outcome = match append_row(&mut state.db, chat, device, batch_id, payload) {
        Ok(outcome) => outcome,
        Err(err) => return storage_error(chat, err),
    };
    let room = state.chats.entry(chat.to_owned()).or_default();
    match outcome {
        Append::Rejected(code) => {
            room.record_push(device, false);
            http::error(if code == "too_large" { 413 } else { 400 }, code)
        }
        Append::Ok { seq, dup } => {
            room.record_push(device, true);
            if !dup {
                room.relay_row(None, seq, device, batch_id, payload);
            }
            http::json(&json!({ "batchId": batch_id, "seq": seq, "dup": dup }), 200)
        }
    }
}

/// `bytes=N-` (open-ended resume, N > 0) only.
fn parse_range_start(header: Option<&str>) -> Option<usize> {
    let start = header?.strip_prefix("bytes=")?.strip_suffix('-')?;
    if start.is_empty() || !start.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    start.parse::<usize>().ok().filter(|n| *n > 0)
}

/// Standard base64 (empty ⇒ empty frontier); `None` = malformed.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing_matches_the_do() {
        assert_eq!(parse_range_start(Some("bytes=10-")), Some(10));
        assert_eq!(parse_range_start(Some("bytes=0-")), None);
        assert_eq!(parse_range_start(Some("bytes=1-5")), None);
        assert_eq!(parse_range_start(Some("bytes=-5")), None);
        assert_eq!(parse_range_start(None), None);
    }

    #[test]
    fn log_appends_dedupes_and_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::store::open(dir.path()).unwrap();
        let first = append_row(&mut db, "c", "d1", "b1", b"one").unwrap();
        assert!(matches!(first, Append::Ok { seq: 1, dup: false }));
        let replay = append_row(&mut db, "c", "d1", "b1", b"one").unwrap();
        assert!(matches!(replay, Append::Ok { seq: 1, dup: true }));
        assert!(matches!(
            append_row(&mut db, "c", "d1", "b2", b"").unwrap(),
            Append::Rejected("empty")
        ));
        append_row(&mut db, "c", "d2", "b3", b"two").unwrap();
        // Other rooms are independent logs.
        append_row(&mut db, "other", "d1", "b1", b"x").unwrap();
        assert_eq!(rows_after(&db, "c", 0, None).unwrap().len(), 2);
        assert_eq!(rows_after(&db, "c", 0, Some("d1")).unwrap().len(), 1);

        assert_eq!(
            commit_checkpoint(&mut db, "c", 5, b"f", b"snap").unwrap(),
            Err("ahead_of_head")
        );
        assert_eq!(
            commit_checkpoint(&mut db, "c", 1, b"f", b"snap").unwrap(),
            Ok((1, 1))
        );
        assert_eq!(
            commit_checkpoint(&mut db, "c", 0, b"f", b"snap").unwrap(),
            Err("floor_regression")
        );
        let stats = log_stats(&db, "c").unwrap();
        assert_eq!(stats["seqFloor"], 1);
        assert_eq!(stats["rowCount"], 1);
        assert_eq!(stats["checkpointSize"], 4);
        let rows = rows_after(&db, "c", 0, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seq, 2);
    }
}
