//! HTTP plumbing: hyper responses and bodies, query strings, the WebSocket
//! upgrade, and [`Peer`] — a live socket's outbound queue, the local stand-in
//! for a DO's hibernatable `WebSocket` handle.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_util::sync::CancellationToken;

pub(crate) type Reply = Response<Full<Bytes>>;
pub(crate) type Ws = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn json(value: &serde_json::Value, status: u16) -> Reply {
    reply(
        status,
        &[("content-type", "application/json")],
        serde_json::to_vec(value).unwrap_or_default(),
    )
}

/// `{"error": code}` — the edge's error body shape everywhere.
pub(crate) fn error(status: u16, code: &str) -> Reply {
    json(&serde_json::json!({ "error": code }), status)
}

pub(crate) fn reply(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> Reply {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

pub(crate) fn header<'a>(request: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    request.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Read a request body, refusing more than `cap` bytes (`413 too_large`,
/// like the Worker's pre-read `content-length` caps).
pub(crate) async fn read_body(body: Incoming, cap: usize) -> Result<Bytes, Reply> {
    match Limited::new(body, cap).collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(err)
            if err
                .downcast_ref::<http_body_util::LengthLimitError>()
                .is_some() =>
        {
            Err(error(413, "too_large"))
        }
        Err(_) => Err(error(400, "bad_body")),
    }
}

/// Percent-decoded query parameters (first value wins).
pub(crate) fn query(request: &Request<Incoming>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in request.uri().query().unwrap_or("").split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        out.entry(percent_decode(key))
            .or_insert_with(|| percent_decode(value));
    }
    out
}

/// `decodeURIComponent` semantics for query values (`+` is a space) —
/// malformed escapes stay literal.
pub(crate) fn percent_decode(value: &str) -> String {
    decode(value, true)
}

/// Path-segment decoding (`+` stays `+`). `None` = not valid UTF-8 after
/// decoding — the Worker's `safeDecode` returning `undefined`.
pub(crate) fn decode_segment(value: &str) -> Option<String> {
    let bytes = decode_bytes(value, false);
    String::from_utf8(bytes).ok()
}

fn decode(value: &str, plus_is_space: bool) -> String {
    String::from_utf8_lossy(&decode_bytes(value, plus_is_space)).into_owned()
}

fn decode_bytes(value: &str, plus_is_space: bool) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 3 <= bytes.len() => match std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    out
}

pub(crate) fn wants_upgrade(request: &Request<Incoming>) -> bool {
    header(request, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Answer a WebSocket upgrade: the 101 response to return now, and the
/// upgraded stream once hyper hands the connection over.
pub(crate) fn accept_upgrade(
    request: &mut Request<Incoming>,
) -> Result<(Reply, hyper::upgrade::OnUpgrade), Reply> {
    if !wants_upgrade(request) {
        return Err(error(426, "expected websocket"));
    }
    let Some(key) = request.headers().get("sec-websocket-key") else {
        return Err(error(400, "missing websocket key"));
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let on_upgrade = hyper::upgrade::on(request);
    let response = reply(
        101,
        &[
            ("upgrade", "websocket"),
            ("connection", "Upgrade"),
            ("sec-websocket-accept", &accept),
        ],
        Vec::new(),
    );
    Ok((response, on_upgrade))
}

/// Finish the upgrade into a server-role WebSocket. `max_message` is a
/// transport backstop only — rooms enforce their own frame budgets (and close
/// with 1009 like the DOs) below it.
pub(crate) async fn into_websocket(
    on_upgrade: hyper::upgrade::OnUpgrade,
    max_message: usize,
) -> Option<Ws> {
    let upgraded = on_upgrade.await.ok()?;
    let config = WebSocketConfig {
        max_message_size: Some(max_message),
        max_frame_size: Some(max_message),
        ..Default::default()
    };
    Some(WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(config)).await)
}

/// Bytes a socket may have queued before it is dropped as unable to keep up.
/// Dropping (not skipping) matters: row streams are gap-sensitive, and a
/// closed socket recovers exactly through its cursor on redial.
const MAX_QUEUED_BYTES: usize = 256 * 1024 * 1024;

/// One live socket's outbound side. Cloning shares the queue.
#[derive(Clone)]
pub(crate) struct Peer {
    pub(crate) id: u64,
    tx: mpsc::UnboundedSender<Message>,
    queued: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    /// Last inbound frame (epoch ms) — the local twin of the DO's
    /// auto-response timestamp, read by the device room's host liveness.
    last_seen: Arc<AtomicI64>,
}

impl Peer {
    pub(crate) fn new(id: u64) -> (Self, mpsc::UnboundedReceiver<Message>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let peer = Self {
            id,
            tx,
            queued: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
            last_seen: Arc::new(AtomicI64::new(now_ms())),
        };
        (peer, rx)
    }

    /// Queue a frame. `false` = the socket is gone (or was just dropped for
    /// falling too far behind).
    pub(crate) fn send(&self, message: Message) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let len = message.len();
        if self.queued.fetch_add(len, Ordering::AcqRel) + len > MAX_QUEUED_BYTES {
            tracing::warn!(
                peer = self.id,
                "local edge: socket fell too far behind; dropping"
            );
            self.close(1013, "too far behind");
            return false;
        }
        if self.tx.send(message).is_err() {
            self.closed.store(true, Ordering::Release);
            return false;
        }
        true
    }

    pub(crate) fn send_text(&self, text: String) -> bool {
        self.send(Message::Text(text))
    }

    pub(crate) fn send_binary(&self, bytes: Vec<u8>) -> bool {
        self.send(Message::Binary(bytes))
    }

    /// Close with a code, after anything already queued.
    pub(crate) fn close(&self, code: u16, reason: &str) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.tx.send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_owned().into(),
        })));
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn last_seen(&self) -> i64 {
        self.last_seen.load(Ordering::Acquire)
    }
}

/// Drive one socket until either side ends or the edge shuts down. Text
/// `"ping"` is answered with `"pong"` right here — the DOs' runtime
/// auto-response pair — and never reaches `on_message`.
pub(crate) async fn pump(
    ws: Ws,
    peer: &Peer,
    mut rx: mpsc::UnboundedReceiver<Message>,
    shutdown: &CancellationToken,
    mut on_message: impl FnMut(Message) -> bool,
) {
    let (mut sink, mut stream) = ws.split();
    let queued = peer.queued.clone();
    let writer = async move {
        while let Some(message) = rx.recv().await {
            let len = message.len();
            let closing = matches!(message, Message::Close(_));
            let sent = sink.send(message).await.is_ok();
            queued.fetch_sub(len, Ordering::AcqRel);
            if !sent || closing {
                break;
            }
        }
        let _ = sink.close().await;
    };
    let reader = async {
        while let Some(Ok(message)) = stream.next().await {
            peer.last_seen.store(now_ms(), Ordering::Release);
            match message {
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                Message::Text(ref text) if text == "ping" => {
                    peer.send_text("pong".into());
                    continue;
                }
                _ => {}
            }
            if !on_message(message) {
                break;
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
        _ = shutdown.cancelled() => {}
    }
    peer.closed.store(true, Ordering::Release);
}
