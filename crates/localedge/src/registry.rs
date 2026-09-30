//! The registry room — the port of `edge/src/registry-room.ts`
//! (docs/registry-sync.md): the authoritative row table, merged with the
//! SAME `zeron_doc::apply_op` every client uses (the 1:1 mirror of
//! registry-core.ts), one monotonic `seq` per accepted batch, and merged
//! rows broadcast to every socket.
//!
//! Single-tenant: the Worker derives the room from the caller's own user id
//! (`reg1/{orgId}/{userId}`); here every authenticated caller IS that user,
//! so one room serves every `/registry/{orgId}/…` path. APNs push targets
//! are accepted and dropped — a local edge has no APNs credentials.

use std::collections::HashMap;
use std::sync::Arc;

use hyper::Request;
use hyper::body::Incoming;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use zeron_doc::{RegistryRow, RowOp, apply_op};

use crate::http::{self, Peer, Reply, Ws, now_ms};
use crate::{Edge, State};

/// Tombstones older than this are purged; older cursors full-resync.
const TOMBSTONE_RETAIN_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Ops per push batch (clients chunk below this).
const MAX_BATCH_OPS: usize = 500;
/// Serialized inbound frame budget.
pub(crate) const MAX_FRAME_BYTES: usize = 1_000_000;
/// `POST /push` pre-read cap.
const MAX_PUSH_BODY: usize = 2 * 1024 * 1024;
/// Per-op serialized budget (registry-core.ts `MAX_OP_BYTES`).
const MAX_OP_BYTES: usize = 16 * 1024;

struct RegistrySocket {
    peer: Peer,
    device: String,
    ready: bool,
}

#[derive(Default)]
pub(crate) struct RegistryLive {
    sockets: Vec<RegistrySocket>,
    /// device → last presence beat (epoch ms). Memory-only.
    presence: HashMap<String, i64>,
    push_outcomes: std::collections::BTreeMap<String, Value>,
}

impl RegistryLive {
    fn broadcast(&self, frame: &Value, skip: Option<u64>) {
        let text = frame.to_string();
        for socket in &self.sockets {
            if socket.ready && Some(socket.peer.id) != skip {
                socket.peer.send_text(text.clone());
            }
        }
    }

    fn record_push(&mut self, device: &str, ok: bool) {
        let key = if device.is_empty() {
            "(unknown)"
        } else {
            device
        };
        let entry = self
            .push_outcomes
            .entry(key.to_owned())
            .or_insert_with(|| json!({ "ok": 0, "rejected": 0, "lastOkAt": 0 }));
        if ok {
            entry["ok"] = json!(entry["ok"].as_u64().unwrap_or(0) + 1);
            entry["lastOkAt"] = json!(now_ms());
        } else {
            entry["rejected"] = json!(entry["rejected"].as_u64().unwrap_or(0) + 1);
        }
    }
}

// ── storage ─────────────────────────────────────────────────────────────────

fn get_meta(db: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT value FROM registry_meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

fn set_meta(db: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO registry_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn meta_u64(db: &Connection, key: &str) -> rusqlite::Result<u64> {
    Ok(get_meta(db, key)?.and_then(|v| v.parse().ok()).unwrap_or(0))
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RegistryRow> {
    let fields: String = row.get(5)?;
    let clocks: String = row.get(6)?;
    Ok(RegistryRow {
        kind: row.get(0)?,
        id: row.get(1)?,
        seq: row.get(2)?,
        deleted: row.get::<_, i64>(3)? == 1,
        del_hlc: row.get(4)?,
        fields: serde_json::from_str(&fields).unwrap_or_default(),
        clocks: serde_json::from_str(&clocks).unwrap_or_default(),
    })
}

fn load_row(db: &Connection, kind: &str, id: &str) -> rusqlite::Result<Option<RegistryRow>> {
    db.query_row(
        "SELECT kind, id, seq, deleted, del_hlc, fields, clocks FROM registry_rows
         WHERE kind = ?1 AND id = ?2",
        params![kind, id],
        read_row,
    )
    .optional()
}

fn save_row(db: &Connection, row: &RegistryRow) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO registry_rows (kind, id, seq, deleted, del_hlc, fields, clocks)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(kind, id) DO UPDATE SET seq = excluded.seq, deleted = excluded.deleted,
           del_hlc = excluded.del_hlc, fields = excluded.fields, clocks = excluded.clocks",
        params![
            row.kind,
            row.id,
            row.seq,
            row.deleted as i64,
            row.del_hlc,
            serde_json::to_string(&row.fields).unwrap_or_else(|_| "{}".into()),
            serde_json::to_string(&row.clocks).unwrap_or_else(|_| "{}".into()),
        ],
    )?;
    Ok(())
}

fn rows_since(db: &Connection, cursor: u64) -> rusqlite::Result<Vec<RegistryRow>> {
    let mut stmt = db.prepare_cached(
        "SELECT kind, id, seq, deleted, del_hlc, fields, clocks FROM registry_rows
         WHERE seq > ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![cursor], read_row)?;
    rows.collect()
}

/// The hello / `GET /rows` answer: a delta for a valid cursor, the full table
/// for none, a pre-GC cursor, or a cursor AHEAD of the server (a wipe — the
/// client re-seeds from its rows with original clocks).
fn state_body(state: &State, cursor: Option<u64>) -> rusqlite::Result<Value> {
    let seq = meta_u64(&state.db, "seq")?;
    let gc_floor = meta_u64(&state.db, "gcFloor")?;
    let full = match cursor {
        None => true,
        Some(c) => c < gc_floor || c > seq,
    };
    let rows = rows_since(&state.db, if full { 0 } else { cursor.unwrap_or(0) })?;
    Ok(json!({
        "seq": seq,
        "full": full,
        "gcFloor": gc_floor,
        "rows": rows,
        "presence": state.registry.presence,
    }))
}

/// Purge tombstones past the retention horizon, raising `gcFloor` so any
/// cursor that might have missed a purged delete full-resyncs (the DO's
/// daily alarm).
pub(crate) fn gc_tombstones(db: &mut Connection) -> rusqlite::Result<usize> {
    let horizon = format!("{:013}-", now_ms() - TOMBSTONE_RETAIN_MS);
    let tx = db.transaction()?;
    let purged_max: Option<u64> = tx.query_row(
        "SELECT MAX(seq) FROM registry_rows WHERE deleted = 1 AND del_hlc < ?1",
        params![horizon],
        |row| row.get(0),
    )?;
    let Some(purged_max) = purged_max else {
        return Ok(0);
    };
    let purged = tx.execute(
        "DELETE FROM registry_rows WHERE deleted = 1 AND del_hlc < ?1",
        params![horizon],
    )?;
    let floor = meta_u64(&tx, "gcFloor")?.max(purged_max);
    set_meta(&tx, "gcFloor", &floor.to_string())?;
    set_meta(&tx, "lastGcAt", &now_ms().to_string())?;
    tx.commit()?;
    Ok(purged)
}

// ── op validation (registry-core.ts `validateOp`) ───────────────────────────

fn chars_match(s: &str, min: usize, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    let len = s.chars().count();
    (min..=max).contains(&len) && s.chars().all(allowed)
}

fn valid_id(s: &str) -> bool {
    chars_match(s, 1, 256, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '/' | '-')
    })
}

fn valid_kind(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && chars_match(s, 1, 32, |c| c.is_ascii_alphanumeric())
}

fn valid_field(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars_match(s, 1, 64, |c| c.is_ascii_alphanumeric())
}

fn valid_hlc(s: &str) -> bool {
    let mut parts = s.splitn(3, '-');
    let (Some(ms), Some(counter), Some(device)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    ms.len() == 13
        && ms.bytes().all(|b| b.is_ascii_digit())
        && counter.len() == 6
        && counter.bytes().all(|b| b.is_ascii_digit())
        && chars_match(device, 1, 128, |c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
        })
}

/// Structural validation of a wire op. `Err` carries the DO's message.
fn validate_op(raw: &Value) -> Result<RowOp, String> {
    let Some(op) = raw.as_object() else {
        return Err("bad op".into());
    };
    let text = |key: &str| op.get(key).and_then(Value::as_str).unwrap_or("");
    if !valid_kind(text("kind")) {
        return Err("bad kind".into());
    }
    if !valid_id(text("id")) {
        return Err("bad id".into());
    }
    let kind = text("op");
    if !matches!(kind, "upsert" | "update" | "delete") {
        return Err("bad op".into());
    }
    if !valid_hlc(text("hlc")) {
        return Err("bad hlc".into());
    }
    let set = op.get("set").filter(|v| !v.is_null());
    if kind == "delete" {
        if set.is_some() {
            return Err("delete carries set".into());
        }
    } else {
        let Some(set) = set.and_then(Value::as_object) else {
            return Err("missing set".into());
        };
        if let Some(key) = set.keys().find(|k| !valid_field(k)) {
            return Err(format!("bad field: {key}"));
        }
    }
    if let Some(clocks) = op.get("clocks").filter(|v| !v.is_null()) {
        let Some(clocks) = clocks.as_object() else {
            return Err("bad clocks".into());
        };
        for (key, hlc) in clocks {
            if !valid_field(key) {
                return Err(format!("bad clock field: {key}"));
            }
            if !hlc.as_str().is_some_and(valid_hlc) {
                return Err(format!("bad clock: {key}"));
            }
        }
    }
    // JS `JSON.stringify(op).length` counts UTF-16 units.
    if raw.to_string().encode_utf16().count() > MAX_OP_BYTES {
        return Err("op too large".into());
    }
    if text("kind") == "sidebarPins" {
        if kind == "delete" {
            return Err("pins use explicit membership, not row deletion".into());
        }
        for (key, value) in set.and_then(Value::as_object).into_iter().flatten() {
            let ok = match (key.as_str(), value) {
                ("pinned", Value::Bool(_)) => true,
                ("orderKey", Value::String(order)) => {
                    order.len() <= 8192
                        && order
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        && order.bytes().last().is_some_and(|b| b != b'0')
                }
                _ => false,
            };
            if !ok {
                return Err("invalid sidebar pin field".into());
            }
        }
    }
    serde_json::from_value(raw.clone()).map_err(|e| format!("bad op: {e}"))
}

// ── batch apply ─────────────────────────────────────────────────────────────

struct Accepted {
    batch: String,
    seq: u64,
    applied: u64,
}

/// Validate + atomically apply one op batch and broadcast merged rows to
/// every ready socket — shared by the WS push and `POST /push`. The caller
/// delivers the ack or error on its own transport.
fn apply_push_batch(
    state: &mut State,
    device: &str,
    frame: &Value,
) -> Result<Accepted, (&'static str, String)> {
    let batch = frame["batch"].as_str().unwrap_or_default().to_owned();
    let Some(ops) = frame["ops"].as_array().filter(|_| !batch.is_empty()) else {
        state.registry.record_push(device, false);
        return Err(("bad_push", "malformed push".into()));
    };
    if ops.is_empty() || ops.len() > MAX_BATCH_OPS {
        state.registry.record_push(device, false);
        return Err(("bad_push", format!("batch of {} ops", ops.len())));
    }
    let mut parsed = Vec::with_capacity(ops.len());
    for raw in ops {
        match validate_op(raw) {
            Ok(op) => parsed.push(op),
            Err(invalid) => {
                // Batches are transactional (cascade deletes): one bad op
                // rejects the whole batch.
                state.registry.record_push(device, false);
                let kind = raw["kind"].as_str().unwrap_or("?");
                let id = raw["id"].as_str().unwrap_or("?");
                return Err(("invalid_op", format!("{kind}/{id}: {invalid}")));
            }
        }
    }
    let applied = (|| -> rusqlite::Result<(u64, Vec<RegistryRow>)> {
        let tx = state.db.transaction()?;
        let next_seq = meta_u64(&tx, "seq")? + 1;
        let mut touched: Vec<RegistryRow> = Vec::new();
        let mut applied = 0u64;
        for op in &parsed {
            let before = match touched
                .iter()
                .position(|r| r.kind == op.kind && r.id == op.id)
            {
                Some(ix) => Some(touched[ix].clone()),
                None => load_row(&tx, &op.kind, &op.id)?,
            };
            let (next, changed) = apply_op(before.as_ref(), op);
            let Some(mut next) = next.filter(|_| changed) else {
                continue;
            };
            applied += 1;
            next.seq = next_seq;
            touched.retain(|r| !(r.kind == next.kind && r.id == next.id));
            touched.push(next);
        }
        if applied > 0 {
            for row in &touched {
                save_row(&tx, row)?;
            }
            set_meta(&tx, "seq", &next_seq.to_string())?;
        }
        tx.commit()?;
        Ok((applied, touched))
    })();
    let (applied, touched) = match applied {
        Ok(result) => result,
        Err(err) => {
            tracing::error!(error = %err, "local edge: registry storage failed");
            return Err(("storage", "storage failure".into()));
        }
    };
    state.registry.record_push(device, true);
    let seq = meta_u64(&state.db, "seq").unwrap_or(0);
    if applied > 0 {
        // Merged full rows to EVERY ready socket (sender included — its op
        // may have lost LWW), before the ack retires its pending batch.
        state
            .registry
            .broadcast(&json!({ "t": "rows", "seq": seq, "rows": touched }), None);
    }
    Ok(Accepted {
        batch,
        seq,
        applied,
    })
}

// ── WebSocket protocol (JSON text frames) ───────────────────────────────────

pub(crate) async fn serve_ws(edge: Arc<Edge>, device: String, ws: Ws) {
    let (peer, rx) = edge.new_peer();
    edge.with_state(|state| {
        state.registry.sockets.push(RegistrySocket {
            peer: peer.clone(),
            device,
            ready: false,
        });
    });
    http::pump(ws, &peer, rx, &edge.shutdown, |message| {
        let Message::Text(text) = message else {
            peer.close(1003, "text frames only");
            return false;
        };
        if text.len() > MAX_FRAME_BYTES {
            peer.close(1009, "frame too large");
            return false;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            peer.close(1002, "bad json");
            return false;
        };
        edge.with_state(|state| on_frame(state, &peer, &frame));
        true
    })
    .await;
    edge.with_state(|state| state.registry.sockets.retain(|s| s.peer.id != peer.id));
}

fn on_frame(state: &mut State, peer: &Peer, frame: &Value) {
    let socket = state
        .registry
        .sockets
        .iter_mut()
        .find(|s| s.peer.id == peer.id);
    let Some(socket) = socket else { return };
    match frame["t"].as_str() {
        Some("hello") => {
            if let Some(device) = frame["device"].as_str().filter(|d| !d.is_empty()) {
                socket.device = device.to_owned();
            }
            socket.ready = true;
            let cursor = frame["cursor"].as_u64();
            match state_body(state, cursor) {
                Ok(mut body) => {
                    body["t"] = json!("state");
                    peer.send_text(body.to_string());
                }
                Err(err) => {
                    tracing::error!(error = %err, "local edge: registry storage failed");
                    peer.send_text(
                        json!({ "t": "error", "code": "storage", "message": "storage failure" })
                            .to_string(),
                    );
                }
            }
        }
        Some("push") => {
            let (ready, device) = (socket.ready, socket.device.clone());
            if !ready {
                state.registry.record_push(&device, false);
                peer.send_text(
                    json!({ "t": "error", "code": "bad_push", "message": "hello first / malformed push" })
                        .to_string(),
                );
                return;
            }
            let reply = match apply_push_batch(state, &device, frame) {
                Ok(ok) => {
                    json!({ "t": "ack", "batch": ok.batch, "seq": ok.seq, "applied": ok.applied })
                }
                Err((code, message)) => json!({ "t": "error", "code": code, "message": message }),
            };
            peer.send_text(reply.to_string());
        }
        Some("presence") => {
            if !socket.ready || socket.device.is_empty() {
                return;
            }
            let device = socket.device.clone();
            let at = frame["at"].as_i64().unwrap_or_else(now_ms);
            state.registry.presence.insert(device.clone(), at);
            state.registry.broadcast(
                &json!({ "t": "presence", "device": device, "at": at }),
                Some(peer.id),
            );
        }
        Some("probe") => {
            let seq = meta_u64(&state.db, "seq").unwrap_or(0);
            peer.send_text(json!({ "t": "probe-ok", "seq": seq }).to_string());
        }
        other => {
            let message = format!("unknown frame: {}", other.unwrap_or("undefined"));
            peer.send_text(
                json!({ "t": "error", "code": "bad_frame", "message": message }).to_string(),
            );
        }
    }
}

// ── HTTP surface ────────────────────────────────────────────────────────────

/// `/registry/{orgId}/{route}` (every route but `ws`).
pub(crate) async fn serve_http(edge: &Edge, route: &str, request: Request<Incoming>) -> Reply {
    let method = request.method().as_str().to_owned();
    let query = http::query(&request);
    let device = query.get("device").cloned().unwrap_or_default();
    match (route, method.as_str()) {
        ("rows", "GET") => {
            // With `?since=` this is the WS hello's exact delta answer;
            // without it, the full-table repair read. `?device=&beat=1`
            // doubles as a presence beat for HTTP-only clients.
            let cursor = query.get("since").and_then(|v| v.parse::<u64>().ok());
            edge.with_state(|state| {
                if !device.is_empty() && query.get("beat").map(String::as_str) == Some("1") {
                    let at = now_ms();
                    state.registry.presence.insert(device.clone(), at);
                    state.registry.broadcast(
                        &json!({ "t": "presence", "device": device, "at": at }),
                        None,
                    );
                }
                match state_body(state, cursor) {
                    Ok(body) => http::json(&body, 200),
                    Err(err) => {
                        tracing::error!(error = %err, "local edge: registry storage failed");
                        http::error(500, "storage")
                    }
                }
            })
        }
        ("push", "POST") => {
            let body = match http::read_body(request.into_body(), MAX_PUSH_BODY).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            let Ok(frame) = serde_json::from_slice::<Value>(&body) else {
                return http::json(
                    &json!({ "error": "bad_push", "message": "malformed body" }),
                    400,
                );
            };
            edge.with_state(|state| match apply_push_batch(state, &device, &frame) {
                Ok(ok) => http::json(
                    &json!({ "batch": ok.batch, "seq": ok.seq, "applied": ok.applied }),
                    200,
                ),
                Err((code, message)) => {
                    http::json(&json!({ "error": code, "message": message }), 400)
                }
            })
        }
        ("push-target", "POST" | "DELETE") => {
            if device.is_empty() {
                return http::json(
                    &json!({ "error": "bad_request", "message": "device required" }),
                    400,
                );
            }
            // No APNs on a local edge: registration is acknowledged so a
            // client's notification settings flow completes, and dropped.
            http::json(&json!({ "ok": true }), 200)
        }
        ("stats", "GET") => edge.with_state(|state| {
            let counts = state.db.query_row(
                "SELECT COUNT(*), COALESCE(SUM(deleted), 0) FROM registry_rows",
                [],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            );
            let Ok((row_count, tombstones)) = counts else {
                return http::error(500, "storage");
            };
            http::json(
                &json!({
                    "seq": meta_u64(&state.db, "seq").unwrap_or(0),
                    "gcFloor": meta_u64(&state.db, "gcFloor").unwrap_or(0),
                    "rowCount": row_count,
                    "tombstones": tombstones,
                    "connectedSockets": state.registry.sockets.len(),
                    "pushOutcomes": state.registry.push_outcomes,
                    "lastBackupSeq": 0,
                    "lastGcAt": meta_u64(&state.db, "lastGcAt").unwrap_or(0),
                    "pushTargets": [],
                    "pushConfigured": false,
                    "pushLog": [],
                }),
                200,
            )
        }),
        ("reset", "POST") => edge.with_state(|state| {
            // Operator wipe: clients detect `seq < cursor` on their next
            // hello and re-seed with original clocks.
            let wiped = state
                .db
                .execute_batch("DELETE FROM registry_rows; DELETE FROM registry_meta;");
            if wiped.is_err() {
                return http::error(500, "storage");
            }
            for socket in &state.registry.sockets {
                socket.peer.close(4410, "registry reset");
            }
            http::json(&json!({ "ok": true }), 200)
        }),
        _ => http::error(404, "not_found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(value: Value) -> Result<RowOp, String> {
        validate_op(&value)
    }

    #[test]
    fn validation_mirrors_registry_core() {
        let hlc = "0000000000001-000000-dev-a";
        assert!(
            op(json!({"kind":"chats","id":"c1","op":"upsert","set":{"title":"x"},"hlc":hlc}))
                .is_ok()
        );
        assert_eq!(
            op(json!({"kind":"Chats","id":"c1","op":"upsert","set":{},"hlc":hlc})).unwrap_err(),
            "bad kind"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c 1","op":"upsert","set":{},"hlc":hlc})).unwrap_err(),
            "bad id"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"upsert","set":{},"hlc":"1-2-x"})).unwrap_err(),
            "bad hlc"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"delete","set":{},"hlc":hlc})).unwrap_err(),
            "delete carries set"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"update","hlc":hlc})).unwrap_err(),
            "missing set"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"upsert","set":{"_x":1},"hlc":hlc}))
                .unwrap_err(),
            "bad field: _x"
        );
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"upsert","set":{},"hlc":hlc,"clocks":{"a":"no"}}))
                .unwrap_err(),
            "bad clock: a"
        );
        assert!(
            op(json!({"kind":"sidebarPins","id":"c1","op":"upsert","set":{"pinned":true,"orderKey":"8"},"hlc":hlc}))
                .is_ok()
        );
        assert_eq!(
            op(json!({"kind":"sidebarPins","id":"c1","op":"upsert","set":{"orderKey":"80"},"hlc":hlc}))
                .unwrap_err(),
            "invalid sidebar pin field"
        );
        let big = "x".repeat(MAX_OP_BYTES);
        assert_eq!(
            op(json!({"kind":"chats","id":"c1","op":"upsert","set":{"title":big},"hlc":hlc}))
                .unwrap_err(),
            "op too large"
        );
    }

    #[test]
    fn tombstones_past_the_horizon_raise_the_gc_floor() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::store::open(dir.path()).unwrap();
        let old = RegistryRow {
            kind: "chats".into(),
            id: "gone".into(),
            seq: 3,
            deleted: true,
            del_hlc: Some("0000000000001-000000-d".into()),
            fields: Default::default(),
            clocks: Default::default(),
        };
        save_row(&db, &old).unwrap();
        let fresh = RegistryRow {
            id: "recent".into(),
            seq: 4,
            del_hlc: Some(format!("{:013}-000000-d", now_ms())),
            ..old.clone()
        };
        save_row(&db, &fresh).unwrap();
        assert_eq!(gc_tombstones(&mut db).unwrap(), 1);
        assert_eq!(meta_u64(&db, "gcFloor").unwrap(), 3);
        assert!(load_row(&db, "chats", "recent").unwrap().is_some());
        assert_eq!(gc_tombstones(&mut db).unwrap(), 0);
    }
}
