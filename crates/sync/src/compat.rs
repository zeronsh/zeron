//! Version compatibility with the edge (docs/compat.md).
//!
//! Every client announces [`PROTOCOL`] when it reaches the edge (`proto=` on
//! room URLs). Builds from before this existed announce nothing, which the
//! edge reads as protocol 0: it hides newer devices' presence from them and
//! can refuse them outright.
//!
//! A refusal is HTTP 426 carrying [`UPDATE_HEADER`]: `app` when the edge needs
//! a newer app for everything (its minimum protocol moved past ours), `room`
//! when one room does (it moved to a generation this build can't open).
//! Clients stop redialing at their normal cadence, retry every
//! [`REFUSED_RETRY`] (an operator can lower the minimum again), and tell the
//! user to update instead of showing a connection error.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_tungstenite::tungstenite::error::Error as WsError;

/// The sync protocol this build speaks. Bump it when a change needs every
/// device that touches it to understand it, and teach the edge what older
/// protocols may still do.
///
/// 1: chat3 (docs/chat3-sync.md). Protocol-0 builds see protocol ≥ 1 devices
/// as offline and are refused by a chat3 chat's old `chat2/{id}` room.
pub const PROTOCOL: u32 = 1;

/// Request header carrying [`PROTOCOL`] on HTTP (sockets use `proto=`).
pub const PROTO_HEADER: &str = "x-zeron-proto";

/// Response header on an edge refusal: `app` or `room`.
pub const UPDATE_HEADER: &str = "x-zeron-update-required";

/// How often a refused client tries again.
pub const REFUSED_RETRY: Duration = Duration::from_secs(30 * 60);

/// What the edge refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Everything: the edge's minimum protocol is above ours.
    App,
    /// This room only.
    Room,
}

impl Refusal {
    fn parse(status: u16, header: Option<&str>) -> Option<Self> {
        if status != 426 {
            return None;
        }
        match header?.trim() {
            "app" => Some(Self::App),
            "room" => Some(Self::Room),
            _ => None,
        }
    }
}

static APP_REFUSED: AtomicBool = AtomicBool::new(false);

/// The edge refused this build as a whole. Latched by any `app` refusal and
/// cleared when a registry session joins again.
pub fn app_update_required() -> bool {
    APP_REFUSED.load(Ordering::Relaxed)
}

pub(crate) fn clear_app_refusal() {
    APP_REFUSED.store(false, Ordering::Relaxed);
}

fn note(refusal: Refusal) -> Refusal {
    if refusal == Refusal::App {
        APP_REFUSED.store(true, Ordering::Relaxed);
    }
    refusal
}

/// A refusal in an HTTP response, if it is one: its status and its
/// [`UPDATE_HEADER`] value.
pub fn from_http(status: u16, header: Option<&str>) -> Option<Refusal> {
    Refusal::parse(status, header).map(note)
}

/// `Err(UpdateRequired)` when an HTTP response is a refusal.
pub fn check_http(status: u16, header: Option<&str>) -> Result<(), crate::SyncError> {
    match from_http(status, header) {
        Some(refusal) => Err(crate::SyncError::UpdateRequired(refusal)),
        None => Ok(()),
    }
}

/// A refusal in a failed WebSocket upgrade, if it is one.
pub fn from_ws(error: &WsError) -> Option<Refusal> {
    let WsError::Http(response) = error else {
        return None;
    };
    let header = response.headers().get(UPDATE_HEADER).and_then(|v| v.to_str().ok());
    Refusal::parse(response.status().as_u16(), header).map(note)
}

/// A dial failure as a [`crate::SyncError`]: refusals keep their meaning.
pub(crate) fn dial_error(error: WsError) -> crate::SyncError {
    match from_ws(&error) {
        Some(refusal) => crate::SyncError::UpdateRequired(refusal),
        None => crate::SyncError::WebSocket(error.to_string()),
    }
}

/// `url` with `proto=` appended (every edge dial).
pub fn with_protocol(url: &str) -> String {
    let mut out = url.to_owned();
    // `ws://host?x` is not a request target: the path needs its `/`.
    let start = url.find("://").map_or(0, |i| i + 3);
    let rest = &url[start..];
    match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => {}
        Some(i) => out.insert(start + i, '/'),
        None => out.push('/'),
    }
    let sep = if out.contains('?') { '&' } else { '?' };
    format!("{out}{sep}proto={PROTOCOL}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::http;

    fn ws_refusal(status: u16, header: Option<&str>) -> WsError {
        let mut response = http::Response::builder().status(status);
        if let Some(value) = header {
            response = response.header(UPDATE_HEADER, value);
        }
        WsError::Http(response.body(None).unwrap())
    }

    #[test]
    fn only_a_426_with_the_header_is_a_refusal() {
        assert_eq!(from_ws(&ws_refusal(426, Some("room"))), Some(Refusal::Room));
        // The edge's plain "expected websocket" 426 is not one.
        assert_eq!(from_ws(&ws_refusal(426, None)), None);
        assert_eq!(from_ws(&ws_refusal(403, Some("room"))), None);
        assert_eq!(from_ws(&ws_refusal(426, Some("later"))), None);
        assert!(matches!(
            dial_error(ws_refusal(426, Some("room"))),
            crate::SyncError::UpdateRequired(Refusal::Room)
        ));
    }

    #[test]
    fn an_app_refusal_latches_until_a_registry_join() {
        clear_app_refusal();
        from_ws(&ws_refusal(426, Some("room")));
        assert!(!app_update_required());
        from_ws(&ws_refusal(426, Some("app")));
        assert!(app_update_required());
        clear_app_refusal();
        assert!(!app_update_required());
    }

    #[test]
    fn urls_announce_the_protocol() {
        assert_eq!(with_protocol("wss://e/chat3/c/ws?token=t"), "wss://e/chat3/c/ws?token=t&proto=1");
        assert_eq!(with_protocol("https://e/chat3/c/checkpoint"), "https://e/chat3/c/checkpoint?proto=1");
        assert_eq!(with_protocol("ws://127.0.0.1:9"), "ws://127.0.0.1:9/?proto=1");
        assert_eq!(with_protocol("ws://127.0.0.1:9?device=d"), "ws://127.0.0.1:9/?device=d&proto=1");
    }
}
