//! The preview signaling room — the port of `edge/src/preview-room.ts`:
//! devices publish their dev-server catalogs and exchange WebRTC SDP through
//! this relay; preview bytes never pass through it. With one engine on the
//! phone there is rarely a peer, but the engine's signaling client dials it
//! whenever sync is on, and a stable room keeps that client quiet.
//!
//! Catalogs live in memory: the DO deletes a device's catalog whenever its
//! socket goes away, so nothing it stores outlives a connection anyway.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::http::{self, Peer, Reply, Ws, now_ms};
use crate::{Edge, State};

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_DEVICES: usize = 16;
const BUDGET_WINDOW_MS: i64 = 10_000;
const BUDGET_MESSAGES: u32 = 120;

struct PreviewSocket {
    peer: Peer,
    device: String,
    budget_at: i64,
    messages: u32,
}

#[derive(Default)]
pub(crate) struct PreviewLive {
    sockets: Vec<PreviewSocket>,
    catalogs: HashMap<String, Value>,
}

impl PreviewLive {
    fn broadcast(&self, value: &Value, except: u64) {
        let text = value.to_string();
        for socket in &self.sockets {
            if socket.peer.id != except {
                socket.peer.send_text(text.clone());
            }
        }
    }

    /// Forget a socket: drop its catalog and tell the others it is gone.
    fn remove(&mut self, peer: u64) {
        let Some(index) = self.sockets.iter().position(|s| s.peer.id == peer) else {
            return;
        };
        let socket = self.sockets.remove(index);
        self.catalogs.remove(&socket.device);
        self.broadcast(&json!({ "type": "gone", "device": socket.device }), peer);
    }
}

pub(crate) fn valid_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `{label}.{label}.localhost` — the preview hostnames engines mint.
fn valid_hostname(value: &str) -> bool {
    let Some(labels) = value.strip_suffix(".localhost") else {
        return false;
    };
    let parts: Vec<&str> = labels.split('.').collect();
    parts.len() == 2
        && parts.iter().all(|label| {
            let bytes = label.as_bytes();
            (1..=63).contains(&bytes.len())
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
                && bytes[0] != b'-'
                && bytes[bytes.len() - 1] != b'-'
        })
}

/// `previewCatalog`: a device's advertised services, cleaned to the known
/// keys, or `None` when anything is malformed.
fn catalog(value: &Value, device: &str) -> Option<Value> {
    const STRINGS: [&str; 9] = [
        "id",
        "projectId",
        "projectName",
        "projectCwd",
        "deviceId",
        "deviceName",
        "hostname",
        "name",
        "cwd",
    ];
    let items = value.as_array().filter(|items| items.len() <= 256)?;
    let mut ids = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let service = item.as_object()?;
        for key in STRINGS {
            let cap = if key == "cwd" || key == "projectCwd" {
                4096
            } else {
                128
            };
            let text = service.get(key)?.as_str()?;
            if text.encode_utf16().count() > cap {
                return None;
            }
        }
        let id = service["id"].as_str()?;
        if service["deviceId"].as_str() != Some(device)
            || !valid_id(id)
            || !ids.insert(id.to_owned())
            || !valid_hostname(service["hostname"].as_str()?)
        {
            return None;
        }
        let port = service.get("port")?.as_u64()?;
        let pid = service.get("pid")?.as_u64()?;
        let started_at = service.get("startedAt")?.as_u64()?;
        if !(1..=65535).contains(&port) || pid < 1 || started_at > (1 << 53) {
            return None;
        }
        service.get("zeronOwned")?.as_bool()?;
        let mut clean = Map::new();
        for key in STRINGS
            .iter()
            .chain(&["port", "pid", "startedAt", "zeronOwned"])
        {
            clean.insert((*key).to_owned(), service[*key].clone());
        }
        out.push(Value::Object(clean));
    }
    Some(Value::Array(out))
}

/// `previewSignal`: a connect request or an SDP offer/answer.
fn signal(value: &Value) -> Option<Value> {
    let session = value["session"].as_str().filter(|s| valid_id(s))?;
    let kind = value["kind"].as_str()?;
    if kind == "connect" && value.get("sdp").is_none() {
        return Some(json!({ "kind": "connect", "session": session }));
    }
    if kind != "offer" && kind != "answer" {
        return None;
    }
    let sdp = value["sdp"].as_object()?;
    let text = sdp.get("sdp")?.as_str()?;
    if sdp.get("type")?.as_str()? != kind
        || text.encode_utf16().count() > 65536
        || !text.starts_with("v=0\r\n")
        || !text.contains("a=fingerprint:sha-256 ")
    {
        return None;
    }
    Some(json!({ "kind": kind, "session": session, "sdp": { "type": kind, "sdp": text } }))
}

/// Admit a device before the upgrade (the DO's pre-accept checks).
pub(crate) fn admit(edge: &Edge, device: &str) -> Result<(), Reply> {
    if !valid_id(device) {
        return Err(http::reply(400, &[], b"Invalid device".to_vec()));
    }
    edge.with_state(|state| {
        let room = &state.preview;
        let existing = room.sockets.iter().any(|s| s.device == device);
        if !existing && room.sockets.len() >= MAX_DEVICES {
            return Err(http::reply(429, &[], b"Device limit reached".to_vec()));
        }
        Ok(())
    })
}

pub(crate) async fn serve_ws(edge: Arc<Edge>, device: String, ws: Ws) {
    let (peer, rx) = edge.new_peer();
    edge.with_state(|state| {
        let room = &mut state.preview;
        // A reconnecting device replaces its previous socket.
        if let Some(index) = room.sockets.iter().position(|s| s.device == device) {
            let stale = room.sockets.remove(index);
            stale.peer.close(1000, "Device reconnected");
        }
        room.catalogs.remove(&device);
        room.broadcast(
            &json!({ "type": "catalog", "device": device, "services": [] }),
            peer.id,
        );
        for (other, services) in &room.catalogs {
            peer.send_text(
                json!({ "type": "catalog", "device": other, "services": services }).to_string(),
            );
        }
        room.sockets.push(PreviewSocket {
            peer: peer.clone(),
            device: device.clone(),
            budget_at: now_ms(),
            messages: 0,
        });
    });
    http::pump(ws, &peer, rx, &edge.shutdown, |message| {
        edge.with_state(|state| on_message(state, &peer, message))
    })
    .await;
    edge.with_state(|state| state.preview.remove(peer.id));
}

fn on_message(state: &mut State, peer: &Peer, message: Message) -> bool {
    let room = &mut state.preview;
    let Some(socket) = room.sockets.iter_mut().find(|s| s.peer.id == peer.id) else {
        return false;
    };
    let text = match message {
        Message::Text(text) if text.len() <= MAX_MESSAGE => text,
        _ => {
            peer.close(1008, "Signaling messages only");
            return false;
        }
    };
    let now = now_ms();
    if now - socket.budget_at > BUDGET_WINDOW_MS {
        socket.budget_at = now;
        socket.messages = 0;
    }
    socket.messages += 1;
    if socket.messages > BUDGET_MESSAGES {
        peer.close(1008, "Signaling rate exceeded");
        return false;
    }
    let device = socket.device.clone();
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        peer.close(1008, "Invalid signaling JSON");
        return false;
    };
    match value["type"].as_str() {
        Some("catalog") => {
            let Some(services) = catalog(&value["services"], &device) else {
                peer.close(1008, "Invalid preview catalog");
                return false;
            };
            room.catalogs.insert(device.clone(), services.clone());
            room.broadcast(
                &json!({ "type": "catalog", "device": device, "services": services }),
                peer.id,
            );
            true
        }
        Some("signal")
            if value["to"]
                .as_str()
                .is_some_and(|to| valid_id(to) && to != device) =>
        {
            let Some(signal) = signal(&value["signal"]) else {
                peer.close(1008, "Invalid preview signaling");
                return false;
            };
            let to = value["to"].as_str().unwrap_or_default();
            match room.sockets.iter().find(|s| s.device == to) {
                Some(target) => {
                    target.peer.send_text(
                        json!({ "type": "signal", "from": device, "signal": signal }).to_string(),
                    );
                }
                None => {
                    peer.send_text(json!({ "type": "gone", "device": to }).to_string());
                }
            }
            true
        }
        _ => {
            peer.close(1008, "Unsupported preview message");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_and_signals_are_validated_like_the_do() {
        let service = json!({
            "id": "svc-1", "projectId": "p", "projectName": "P", "projectCwd": "/p",
            "deviceId": "dev-a", "deviceName": "A", "hostname": "web.p1.localhost",
            "name": "web", "cwd": "/p", "port": 3000, "pid": 42, "startedAt": 1,
            "zeronOwned": true, "extra": "dropped"
        });
        let clean = catalog(&json!([service]), "dev-a").unwrap();
        assert!(clean[0].get("extra").is_none());
        assert!(
            catalog(&json!([service]), "dev-b").is_none(),
            "foreign deviceId"
        );
        assert!(signal(&json!({ "kind": "connect", "session": "s1" })).is_some());
        assert!(
            signal(&json!({ "kind": "offer", "session": "s1",
                "sdp": { "type": "offer", "sdp": "v=0\r\na=fingerprint:sha-256 AA" } }))
            .is_some()
        );
        assert!(signal(&json!({ "kind": "offer", "session": "s1", "sdp": { "type": "answer", "sdp": "v=0\r\n" } })).is_none());
    }
}
