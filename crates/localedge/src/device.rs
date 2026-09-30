//! Device rooms — the port of `edge/src/device-room.ts` +
//! `device-nudges.ts`: a byte relay between one host socket and any number
//! of client sockets, relay-level bounces (`host_offline`, `host_closed`,
//! `client_gone`, `client_closed`), host-published sidecar slots, and the
//! durable command nudge queue that wakes the host engine for a cold chat.
//!
//! The room is claimed by the first host join, as in the DO; clients and the
//! HTTP routes answer as the DO does for an unclaimed room. Single-tenant, so
//! the claim records only that a host has existed.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use hyper::Request;
use hyper::body::Incoming;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use zeron_rpc::device_room::{
    CLIENT_CLOSED, CLIENT_GONE, HOST_CLOSED, HOST_OFFLINE, NUDGE_KIND, RELAY_KIND,
};
use zeron_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};

use crate::http::{self, Peer, Reply, Ws, now_ms};
use crate::{Edge, State};

/// A host socket that has not proven liveness within this window stops
/// being routed to (clients get `host_offline` instead of hanging on a
/// corpse). Hosts ping every 10s.
const HOST_LIVENESS_MS: i64 = 75_000;
const NUDGE_ACK_KIND: &str = "nudgeAck";
/// Nudges delivered per replay page / queue bound (device-nudges.ts).
const NUDGE_PAGE: usize = 64;
const NUDGE_CAP: u64 = 4096;
/// Unacknowledged nudges are re-sent after this (the DO's 5s alarm).
const NUDGE_REDELIVERY: Duration = Duration::from_secs(5);
const MAX_SIDECAR_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Host,
    Client,
}

struct DeviceSocket {
    peer: Peer,
    role: Role,
    conn_id: String,
    nudge_ack: bool,
    nudge_inflight: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct DeviceLive {
    sockets: Vec<DeviceSocket>,
    alarm: Option<tokio::task::AbortHandle>,
}

impl DeviceLive {
    /// The freshest host socket alive inside the liveness window.
    /// `exclude` drops the socket whose close is being handled.
    fn live_host(&self, exclude: Option<u64>) -> Option<usize> {
        let now = now_ms();
        self.sockets
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.role == Role::Host && Some(s.peer.id) != exclude && !s.peer.is_closed()
            })
            .max_by_key(|(_, s)| s.peer.last_seen())
            .filter(|(_, s)| now - s.peer.last_seen() <= HOST_LIVENESS_MS)
            .map(|(ix, _)| ix)
    }
}

fn deliver(peer: &Peer, header: &DeviceFrameHeader, payload: &[u8]) {
    match encode_device_frame(header, payload) {
        Ok(frame) => {
            peer.send_binary(frame);
        }
        Err(err) => tracing::error!(error = %err, "local edge: device frame encode failed"),
    }
}

fn relay_error(code: &str) -> Vec<u8> {
    json!({ "error": code }).to_string().into_bytes()
}

fn valid_chat_id(chat: &str) -> bool {
    (1..=64).contains(&chat.len())
        && chat
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ── storage: claim, sidecars, nudges ────────────────────────────────────────

fn claimed(db: &Connection, device: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT 1 FROM device_meta WHERE device_id = ?1 AND key = 'owner'",
        params![device],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
}

fn claim(db: &Connection, device: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO device_meta (device_id, key, value) VALUES (?1, 'owner', 'local')",
        params![device],
    )?;
    Ok(())
}

fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Queue a wake for `chat`. `false` = the queue is full: a durable
/// reconcile marker is set instead (never silently discard a wake).
fn enqueue_nudge(db: &Connection, device: &str, chat: &str) -> rusqlite::Result<bool> {
    let exists = db
        .query_row(
            "SELECT 1 FROM pending_nudges WHERE device_id = ?1 AND chat_id = ?2",
            params![device, chat],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let count: u64 = db.query_row(
        "SELECT COUNT(*) FROM pending_nudges WHERE device_id = ?1",
        params![device],
        |row| row.get(0),
    )?;
    if !exists && count >= NUDGE_CAP {
        db.execute(
            "INSERT INTO nudge_reconcile (device_id, token) VALUES (?1, ?2)
             ON CONFLICT(device_id) DO UPDATE SET token = excluded.token",
            params![device, new_token()],
        )?;
        return Ok(false);
    }
    db.execute(
        "INSERT INTO pending_nudges (device_id, chat_id, queued_at, token) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(device_id, chat_id) DO UPDATE SET queued_at = excluded.queued_at, token = excluded.token",
        params![device, chat, now_ms(), new_token()],
    )?;
    Ok(true)
}

/// The reconcile marker (chat `*`) first, then the oldest queued wakes.
fn pending_nudges(db: &Connection, device: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    if let Some(token) = db
        .query_row(
            "SELECT token FROM nudge_reconcile WHERE device_id = ?1",
            params![device],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        out.push(("*".to_owned(), token));
    }
    let mut stmt = db.prepare_cached(
        "SELECT chat_id, token FROM pending_nudges WHERE device_id = ?1
         ORDER BY queued_at, chat_id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![device, (NUDGE_PAGE * 2) as i64], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Retire a wake — only if `token` still names it (fences an ACK against a
/// newer re-queue of the same chat).
fn acknowledge_nudge(
    db: &Connection,
    device: &str,
    chat: &str,
    token: &str,
) -> rusqlite::Result<()> {
    if chat == "*" {
        db.execute(
            "DELETE FROM nudge_reconcile WHERE device_id = ?1 AND token = ?2",
            params![device, token],
        )?;
    } else {
        db.execute(
            "DELETE FROM pending_nudges WHERE device_id = ?1 AND chat_id = ?2 AND token = ?3",
            params![device, chat, token],
        )?;
    }
    Ok(())
}

/// Deliver pending nudges to the host socket at `host`. Hosts that joined
/// with `nudgeAck=1` keep them in flight until they ACK durable admission;
/// older hosts retire them on delivery. `true` = arm the redelivery alarm.
fn replay_nudges(state: &mut State, device: &str, host: usize) -> bool {
    let pending = match pending_nudges(&state.db, device) {
        Ok(pending) => pending,
        Err(err) => {
            tracing::error!(error = %err, "local edge: nudge storage failed");
            return false;
        }
    };
    let Some(room) = state.devices.get_mut(device) else {
        return false;
    };
    let socket = &mut room.sockets[host];
    let mut sent = 0;
    for (chat, token) in pending {
        if socket.nudge_ack && socket.nudge_inflight.contains(&token) {
            continue;
        }
        let full = if socket.nudge_ack {
            socket.nudge_inflight.len() >= NUDGE_PAGE
        } else {
            sent >= NUDGE_PAGE
        };
        if full {
            break;
        }
        if chat == "*" && !socket.nudge_ack {
            continue;
        }
        let payload = json!({ "chatId": chat, "token": token }).to_string();
        deliver(
            &socket.peer,
            &DeviceFrameHeader::new(chat.as_str(), NUDGE_KIND),
            payload.as_bytes(),
        );
        if socket.nudge_ack {
            socket.nudge_inflight.insert(token);
        } else if let Err(err) = acknowledge_nudge(&state.db, device, &chat, &token) {
            tracing::error!(error = %err, "local edge: nudge storage failed");
        }
        sent += 1;
    }
    sent > 0 || !socket.nudge_inflight.is_empty()
}

/// (Re)arm the room's redelivery alarm: after [`NUDGE_REDELIVERY`], in-flight
/// nudges are forgotten and replayed to whichever host is live.
fn arm_alarm(edge: &Arc<Edge>, device: &str) {
    let task_edge = edge.clone();
    let task_device = device.to_owned();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(NUDGE_REDELIVERY) => {}
                _ = task_edge.shutdown.cancelled() => return,
            }
            let again = task_edge.with_state(|state| {
                let Some(host) = state
                    .devices
                    .get(&task_device)
                    .and_then(|r| r.live_host(None))
                else {
                    return false; // the next join replays durable receipts
                };
                if let Some(room) = state.devices.get_mut(&task_device) {
                    room.sockets[host].nudge_inflight.clear();
                }
                replay_nudges(state, &task_device, host)
            });
            if !again {
                return;
            }
        }
    });
    let previous = edge.with_state(|state| {
        state
            .devices
            .entry(device.to_owned())
            .or_default()
            .alarm
            .replace(task.abort_handle())
    });
    if let Some(previous) = previous {
        previous.abort();
    }
}

// ── WebSocket relay ─────────────────────────────────────────────────────────

pub(crate) async fn serve_ws(
    edge: Arc<Edge>,
    device: String,
    host: bool,
    conn_id: String,
    nudge_ack: bool,
    ws: Ws,
) {
    let (peer, rx) = edge.new_peer();
    let role = if host { Role::Host } else { Role::Client };
    let arm = edge.with_state(|state| {
        let room = state.devices.entry(device.clone()).or_default();
        if role == Role::Host {
            // One live host socket: close any predecessor (engine restart).
            for stale in room.sockets.iter().filter(|s| s.role == Role::Host) {
                stale.peer.close(4409, "superseded by new host connection");
            }
        }
        room.sockets.push(DeviceSocket {
            peer: peer.clone(),
            role,
            conn_id: conn_id.clone(),
            nudge_ack,
            nudge_inflight: HashSet::new(),
        });
        let index = room.sockets.len() - 1;
        role == Role::Host && replay_nudges(state, &device, index)
    });
    if arm {
        arm_alarm(&edge, &device);
    }
    http::pump(ws, &peer, rx, &edge.shutdown, |message| {
        let Message::Binary(bytes) = message else {
            return true; // text is only the ping/pong keepalive
        };
        let Ok((header, payload)) = decode_device_frame(&bytes) else {
            peer.close(1002, "Frame error");
            return false;
        };
        let arm = edge.with_state(|state| on_frame(state, &device, &peer, header, &payload));
        if arm {
            arm_alarm(&edge, &device);
        }
        true
    })
    .await;
    edge.with_state(|state| on_close(state, &device, &peer));
}

/// Route one inbound frame. `true` = arm the nudge redelivery alarm.
fn on_frame(
    state: &mut State,
    device: &str,
    peer: &Peer,
    header: DeviceFrameHeader,
    payload: &[u8],
) -> bool {
    let Some(room) = state.devices.get_mut(device) else {
        return false;
    };
    let Some(index) = room.sockets.iter().position(|s| s.peer.id == peer.id) else {
        return false;
    };
    let socket = &room.sockets[index];
    if socket.role == Role::Client {
        match room.live_host(None) {
            Some(host) => {
                let mut routed = DeviceFrameHeader::new(header.s, header.k);
                routed.from = Some(socket.conn_id.clone());
                deliver(&room.sockets[host].peer, &routed, payload);
            }
            // Host offline: bounce so the client can surface "device is
            // asleep" instead of hanging.
            None => deliver(
                peer,
                &DeviceFrameHeader::new(header.s, RELAY_KIND),
                &relay_error(HOST_OFFLINE),
            ),
        }
        return false;
    }
    if header.k == NUDGE_ACK_KIND && socket.nudge_ack {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Ack {
            chat_id: String,
            token: String,
        }
        // A malformed ACK cannot retire work.
        let Ok(ack) = serde_json::from_slice::<Ack>(payload) else {
            return false;
        };
        if let Err(err) = acknowledge_nudge(&state.db, device, &ack.chat_id, &ack.token) {
            tracing::error!(error = %err, "local edge: nudge storage failed");
        }
        if let Some(room) = state.devices.get_mut(device) {
            room.sockets[index].nudge_inflight.remove(&ack.token);
        }
        return replay_nudges(state, device, index);
    }
    // Host frame: route by `to`.
    let Some(to) = header.to else {
        return false;
    };
    let target = room
        .sockets
        .iter()
        .find(|s| s.role == Role::Client && s.conn_id == to);
    match target {
        Some(client) => deliver(
            &client.peer,
            &DeviceFrameHeader::new(header.s, header.k),
            payload,
        ),
        None => deliver(
            peer,
            &DeviceFrameHeader::new(header.s, RELAY_KIND).with_to(to),
            &relay_error(CLIENT_GONE),
        ),
    }
    false
}

fn on_close(state: &mut State, device: &str, peer: &Peer) {
    let Some(room) = state.devices.get_mut(device) else {
        return;
    };
    let Some(index) = room.sockets.iter().position(|s| s.peer.id == peer.id) else {
        return;
    };
    let socket = room.sockets.remove(index);
    match socket.role {
        Role::Client => {
            // Tell the host so it can tear down per-client streams.
            if let Some(host) = room.live_host(None) {
                let mut header = DeviceFrameHeader::new("", RELAY_KIND);
                header.from = Some(socket.conn_id);
                deliver(
                    &room.sockets[host].peer,
                    &header,
                    &relay_error(CLIENT_CLOSED),
                );
            }
        }
        Role::Host => {
            // Only tear clients down when NO live host is left: a superseded
            // predecessor closing must not knock clients off its successor.
            if room.live_host(None).is_none() {
                for client in room.sockets.iter().filter(|s| s.role == Role::Client) {
                    deliver(
                        &client.peer,
                        &DeviceFrameHeader::new("", RELAY_KIND),
                        &relay_error(HOST_CLOSED),
                    );
                }
            }
        }
    }
    if room.sockets.is_empty() {
        // Nothing to redeliver to; the next host join replays the queue.
        if let Some(alarm) = room.alarm.take() {
            alarm.abort();
        }
        state.devices.remove(device);
    }
}

// ── HTTP surface ────────────────────────────────────────────────────────────

/// Room claim + host join: the `/ws` gate. Clients may only join a claimed
/// room (the DO's owner check).
pub(crate) fn admit_ws(edge: &Edge, device: &str, host: bool) -> Result<(), Reply> {
    edge.with_state(|state| {
        let result = if host {
            claim(&state.db, device)
        } else {
            match claimed(&state.db, device) {
                Ok(true) => Ok(()),
                Ok(false) => return Err(http::reply(403, &[], b"forbidden".to_vec())),
                Err(err) => Err(err),
            }
        };
        result.map_err(|err| {
            tracing::error!(error = %err, "local edge: device storage failed");
            http::error(500, "storage")
        })
    })
}

/// `/device/{id}/{route…}` (every route but `ws`).
pub(crate) async fn serve_http(
    edge: &Arc<Edge>,
    device: &str,
    route: &[&str],
    request: Request<Incoming>,
) -> Reply {
    let claimed = edge.with_state(|state| claimed(&state.db, device));
    let claimed = match claimed {
        Ok(claimed) => claimed,
        Err(err) => {
            tracing::error!(error = %err, "local edge: device storage failed");
            return http::error(500, "storage");
        }
    };
    let method = request.method().as_str().to_owned();
    match (route, method.as_str()) {
        (["sidecar", name], _) if valid_sidecar_name(name) => {
            if !claimed {
                return http::error(404, "forbidden");
            }
            if method == "GET" {
                let stored = edge.with_state(|state| {
                    state
                        .db
                        .query_row(
                            "SELECT json FROM device_sidecars WHERE device_id = ?1 AND name = ?2",
                            params![device, name],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                });
                return match stored {
                    Ok(Some(text)) => http::reply(
                        200,
                        &[("content-type", "application/json")],
                        text.into_bytes(),
                    ),
                    Ok(None) => http::error(404, "not_found"),
                    Err(_) => http::error(500, "storage"),
                };
            }
            if method == "POST" {
                let body = match http::read_body(request.into_body(), MAX_SIDECAR_BYTES).await {
                    Ok(body) => body,
                    Err(reply) => return reply,
                };
                let Ok(value) = serde_json::from_slice::<Value>(&body) else {
                    return http::error(400, "bad_json");
                };
                let stored = edge.with_state(|state| {
                    state.db.execute(
                        "INSERT INTO device_sidecars (device_id, name, json) VALUES (?1, ?2, ?3)
                         ON CONFLICT(device_id, name) DO UPDATE SET json = excluded.json",
                        params![device, name, value.to_string()],
                    )
                });
                return match stored {
                    Ok(_) => http::json(&json!({ "ok": true }), 200),
                    Err(_) => http::error(500, "storage"),
                };
            }
            http::reply(404, &[], b"not found".to_vec())
        }
        (["status"], "GET") => {
            if !claimed {
                return http::error(404, "forbidden");
            }
            edge.with_state(|state| {
                let (connected, sockets) = state.devices.get(device).map_or((false, 0), |room| {
                    (
                        room.live_host(None).is_some(),
                        room.sockets.iter().filter(|s| s.role == Role::Host).count(),
                    )
                });
                http::json(
                    &json!({ "hostConnected": connected, "hostSockets": sockets }),
                    200,
                )
            })
        }
        (["nudge"], "POST") => {
            if !claimed {
                return http::error(404, "forbidden");
            }
            let body = http::read_body(request.into_body(), 64 * 1024).await.ok();
            let chat = body
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v["chatId"].as_str().map(str::to_owned))
                .filter(|chat| valid_chat_id(chat));
            let Some(chat) = chat else {
                return http::error(400, "bad_chat_id");
            };
            let outcome = edge.with_state(|state| {
                let queued = enqueue_nudge(&state.db, device, &chat)?;
                let host = state.devices.get(device).and_then(|r| r.live_host(None));
                let arm = host.is_some_and(|host| replay_nudges(state, device, host));
                Ok::<_, rusqlite::Error>((queued, host.is_some(), arm))
            });
            let (queued, delivered, arm) = match outcome {
                Ok(outcome) => outcome,
                Err(err) => {
                    tracing::error!(error = %err, "local edge: nudge storage failed");
                    return http::error(500, "storage");
                }
            };
            if arm {
                arm_alarm(edge, device);
            }
            if !queued {
                return http::json(
                    &json!({ "error": "nudge_queue_full", "retryable": true }),
                    503,
                );
            }
            http::json(&json!({ "delivered": delivered, "queued": true }), 200)
        }
        _ => http::reply(404, &[], b"not found".to_vec()),
    }
}

fn valid_sidecar_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudge_queue_is_durable_fenced_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::store::open(dir.path()).unwrap();
        assert!(enqueue_nudge(&db, "host", "chat-1").unwrap());
        let first = pending_nudges(&db, "host").unwrap();
        assert_eq!(first.len(), 1);
        // Re-queueing mints a new token: an ACK for the old one is fenced off.
        assert!(enqueue_nudge(&db, "host", "chat-1").unwrap());
        acknowledge_nudge(&db, "host", "chat-1", &first[0].1).unwrap();
        let second = pending_nudges(&db, "host").unwrap();
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].1, first[0].1);
        acknowledge_nudge(&db, "host", "chat-1", &second[0].1).unwrap();
        assert!(pending_nudges(&db, "host").unwrap().is_empty());

        for i in 0..NUDGE_CAP {
            enqueue_nudge(&db, "host", &format!("c{i}")).unwrap();
        }
        assert!(!enqueue_nudge(&db, "host", "overflow").unwrap());
        let pending = pending_nudges(&db, "host").unwrap();
        assert_eq!(pending[0].0, "*", "full queue arms the reconcile marker");
        assert_eq!(pending.len(), 1 + NUDGE_PAGE * 2);
        // Other devices' queues are independent.
        assert!(pending_nudges(&db, "other").unwrap().is_empty());
    }
}
