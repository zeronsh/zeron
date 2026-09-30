//! zeron-localedge — a single-tenant, self-hostable edge: the Rust port of
//! the TypeScript Worker + Durable Objects in `edge/src`, so engines sync,
//! relay RPCs and signal WebRTC through the exact production protocols with
//! no Cloudflare in the loop. `zeron local-edge` serves it standalone for
//! development and cross-device tests (docs/file-transfer.md).
//!
//! What moved and what didn't:
//! - chat2 rooms ([`chat`]), the registry room ([`registry`]), device-room
//!   relays with durable nudges ([`device`]) and preview signaling
//!   ([`preview`]) keep their wire protocols byte for byte; the Worker's
//!   routes keep their paths, status codes and error bodies.
//! - Every DO's storage becomes one SQLite file under the data dir, so an
//!   edge restart keeps every room's history.
//! - Auth is one shared secret instead of WorkOS JWTs: every request carries
//!   it as `Authorization: Bearer` or `?token=` (what the Worker accepts),
//!   compared in constant time. Nothing but `/health` and the public release
//!   feed is served without it.
//! - Single tenant: the identity every room would be scoped to is the one
//!   token holder, so owner claims and `orgId`/`userId` room partitioning
//!   collapse; `/auth/*` answers like a Worker without WorkOS configured.
//!   Engines join as user/org `local` (`EngineConfig::with_local_edge`).
//!
//! The listener binds 127.0.0.1 unless [`LocalEdgeConfig::bind`] says
//! otherwise (`zeron local-edge --bind 0.0.0.0` lets other machines join).

// `Result<_, Reply>`: the error IS the HTTP answer (status + body), returned
// straight to hyper on a cold path; boxing it buys nothing.
#![allow(clippy::result_large_err)]

mod chat;
mod device;
mod http;
mod preview;
mod registry;
mod store;

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hyper::Request;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::http::{Peer, Reply, Ws};

/// The tokens a local edge accepts: long enough to not be guessable, and
/// URL-safe because the engine splices it into WebSocket URLs unencoded.
pub const MIN_TOKEN_LEN: usize = 16;

/// Tool-output sidecar cap (`MAX_TOOL_BLOB_BYTES` in edge/src/index.ts).
const MAX_TOOL_BLOB_BYTES: usize = 1024 * 1024;
/// Legacy `/diff/{chatId}` sidecar cap (the SessionRoom's sidecar budget).
const MAX_LEGACY_DIFF_BYTES: usize = 4 * 1024 * 1024;
/// How often tombstone GC runs (the registry DO's daily alarm).
const GC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct LocalEdgeConfig {
    /// Directory for the edge's SQLite state (created if missing).
    pub data_dir: PathBuf,
    /// Listen address: loopback by default; `0.0.0.0` when engines on other
    /// machines join.
    pub bind: IpAddr,
    /// Port; 0 picks an ephemeral one (tests).
    pub port: u16,
    /// The shared secret every request must present.
    pub token: String,
}

impl LocalEdgeConfig {
    /// An edge on `127.0.0.1:{port}` (tests, single-machine development).
    pub fn loopback(data_dir: impl Into<PathBuf>, port: u16, token: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            token: token.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocalEdgeError {
    #[error("local edge token must be at least {MIN_TOKEN_LEN} URL-safe characters")]
    InvalidToken,
    #[error("local edge io: {0}")]
    Io(#[from] std::io::Error),
    #[error("local edge storage: {0}")]
    Storage(#[from] rusqlite::Error),
}

/// A running local edge. Dropping it stops the listener and closes every
/// socket; state stays on disk for the next start.
pub struct LocalEdge {
    addr: SocketAddr,
    edge: Arc<Edge>,
    accept: Option<tokio::task::JoinHandle<()>>,
}

impl LocalEdge {
    /// Open (or create) the state under `data_dir` and start serving on
    /// `{bind}:{port}`. Returns once the listener is bound.
    pub async fn start(config: LocalEdgeConfig) -> Result<Self, LocalEdgeError> {
        if !valid_token(&config.token) {
            return Err(LocalEdgeError::InvalidToken);
        }
        std::fs::create_dir_all(&config.data_dir)?;
        let mut db = store::open(&config.data_dir)?;
        registry::gc_tombstones(&mut db)?;
        let listener = TcpListener::bind((config.bind, config.port)).await?;
        let addr = listener.local_addr()?;
        let edge = Arc::new(Edge {
            token: config.token,
            state: Mutex::new(State {
                db,
                chats: HashMap::new(),
                registry: registry::RegistryLive::default(),
                devices: HashMap::new(),
                preview: preview::PreviewLive::default(),
            }),
            shutdown: CancellationToken::new(),
            next_peer: AtomicU64::new(1),
        });
        tokio::spawn(gc_loop(edge.clone()));
        let accept = tokio::spawn(accept_loop(listener, edge.clone()));
        tracing::info!(%addr, dir = %config.data_dir.display(), "local edge listening");
        Ok(Self {
            addr,
            edge,
            accept: Some(accept),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://{addr}` — the edge URL engines on this machine use.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Stop serving and wait until the port is released.
    pub async fn shutdown(mut self) {
        self.edge.shutdown.cancel();
        if let Some(accept) = self.accept.take() {
            let _ = accept.await;
        }
    }
}

impl Drop for LocalEdge {
    fn drop(&mut self) {
        self.edge.shutdown.cancel();
        if let Some(accept) = &self.accept {
            accept.abort();
        }
    }
}

fn valid_token(token: &str) -> bool {
    token.len() >= MIN_TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

/// Shared edge state. One lock serializes every room, which gives each room
/// the DO's single-threaded event semantics (and cross-room ordering for
/// free); nothing awaits while holding it.
pub(crate) struct Edge {
    token: String,
    state: Mutex<State>,
    pub(crate) shutdown: CancellationToken,
    next_peer: AtomicU64,
}

pub(crate) struct State {
    pub(crate) db: rusqlite::Connection,
    pub(crate) chats: HashMap<String, chat::ChatLive>,
    pub(crate) registry: registry::RegistryLive,
    pub(crate) devices: HashMap<String, device::DeviceLive>,
    pub(crate) preview: preview::PreviewLive,
}

impl Edge {
    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut state)
    }

    pub(crate) fn new_peer(
        &self,
    ) -> (
        Peer,
        tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
    ) {
        Peer::new(self.next_peer.fetch_add(1, Ordering::Relaxed))
    }

    /// `Authorization: Bearer` first, then `?token=` (sockets can't always
    /// set headers) — the Worker's `bearerFromRequest`.
    fn authorized(&self, request: &Request<Incoming>) -> bool {
        let header = http::header(request, "authorization").and_then(|value| {
            let (scheme, token) = value.split_at_checked(7)?;
            scheme
                .eq_ignore_ascii_case("bearer ")
                .then(|| token.trim().to_owned())
        });
        let presented = header.or_else(|| http::query(request).remove("token"));
        presented.is_some_and(|token| bool::from(token.as_bytes().ct_eq(self.token.as_bytes())))
    }
}

async fn accept_loop(listener: TcpListener, edge: Arc<Edge>) {
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(err) => {
                    tracing::warn!(error = %err, "local edge: accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            _ = edge.shutdown.cancelled() => return,
        };
        let edge = edge.clone();
        tokio::spawn(async move {
            let service_edge = edge.clone();
            let service = hyper::service::service_fn(move |request| {
                let edge = service_edge.clone();
                async move { Ok::<_, Infallible>(route(edge, request).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            tokio::select! {
                _ = connection => {}
                _ = edge.shutdown.cancelled() => {}
            }
        });
    }
}

async fn gc_loop(edge: Arc<Edge>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(GC_INTERVAL) => {}
            _ = edge.shutdown.cancelled() => return,
        }
        if let Err(err) = edge.with_state(|state| registry::gc_tombstones(&mut state.db)) {
            tracing::warn!(error = %err, "local edge: tombstone gc failed");
        }
    }
}

/// `^[A-Za-z0-9_-]{1,128}$` — room ids (`ID_RE` in edge/src/index.ts).
fn valid_id(value: &str) -> bool {
    preview::valid_id(value)
}

/// `^[A-Za-z0-9._:#~-]{1,200}$` — tool part ids (`PART_RE`).
fn valid_part(value: &str) -> bool {
    (1..=200).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:#~-".contains(&b))
}

/// Upgrade to a WebSocket and hand the socket to `serve` once hyper
/// releases the connection.
fn upgrade<F, Fut>(edge: &Arc<Edge>, mut request: Request<Incoming>, max: usize, serve: F) -> Reply
where
    F: FnOnce(Arc<Edge>, Ws) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    match http::accept_upgrade(&mut request) {
        Err(reply) => reply,
        Ok((reply, on_upgrade)) => {
            let edge = edge.clone();
            tokio::spawn(async move {
                if let Some(ws) = http::into_websocket(on_upgrade, max).await {
                    serve(edge, ws).await;
                }
            });
            reply
        }
    }
}

/// The Worker's router (edge/src/index.ts), minus the routes that only
/// exist for pre-chat2 clients.
async fn route(edge: Arc<Edge>, request: Request<Incoming>) -> Reply {
    let path = request.uri().path().to_owned();
    let method = request.method().as_str().to_owned();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();

    // ── public: liveness and the release feed ───────────────────────────────
    if path == "/health" {
        return http::json(&json!({ "ok": true, "auth": "local" }), 200);
    }
    if parts.first() == Some(&"releases") && matches!(method.as_str(), "GET" | "HEAD") {
        return releases(&parts[1..], &method);
    }

    if !edge.authorized(&request) {
        return http::error(401, "unauthenticated");
    }

    match parts.as_slice() {
        // A Worker without WorkOS configured: exchange/refresh/orgs are 501.
        ["auth", ..] => http::error(501, "workos not configured"),
        ["preview", org, "ws"] if valid_id(org) => {
            let device = http::query(&request).remove("device").unwrap_or_default();
            if let Err(reply) = preview::admit(&edge, &device) {
                return reply;
            }
            upgrade(&edge, request, 2 * 1024 * 1024, move |edge, ws| {
                preview::serve_ws(edge, device, ws)
            })
        }
        ["chat2", chat, route] if valid_id(chat) => {
            let chat = (*chat).to_owned();
            if *route == "ws" {
                let device = http::query(&request).remove("device").unwrap_or_default();
                return upgrade(
                    &edge,
                    request,
                    2 * chat::MAX_FRAME_BYTES,
                    move |edge, ws| chat::serve_ws(edge, chat, device, ws),
                );
            }
            chat::serve_http(&edge, &chat, route, request).await
        }
        ["chat2", chat, ..] if valid_id(chat) => http::error(404, "not found"),
        ["registry", org, rest @ ..] if valid_id(org) => match rest {
            ["ws"] => {
                let device = http::query(&request).remove("device").unwrap_or_default();
                upgrade(
                    &edge,
                    request,
                    2 * registry::MAX_FRAME_BYTES,
                    move |edge, ws| registry::serve_ws(edge, device, ws),
                )
            }
            [route] => registry::serve_http(&edge, route, request).await,
            _ => http::error(404, "not_found"),
        },
        ["device", device, "ws"] if valid_id(device) => {
            let mut query = http::query(&request);
            let host = query.get("role").map(String::as_str) == Some("host");
            let conn_id = query
                .remove("connId")
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let nudge_ack = query.get("nudgeAck").map(String::as_str) == Some("1");
            if !http::wants_upgrade(&request) {
                return http::error(426, "expected websocket");
            }
            if let Err(reply) = device::admit_ws(&edge, device, host) {
                return reply;
            }
            let device = (*device).to_owned();
            upgrade(&edge, request, 16 * 1024 * 1024, move |edge, ws| {
                device::serve_ws(edge, device, host, conn_id, nudge_ack, ws)
            })
        }
        ["device", device, rest @ ..] if valid_id(device) && !rest.is_empty() => {
            let device = (*device).to_owned();
            let rest: Vec<String> = rest.iter().map(|s| (*s).to_owned()).collect();
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            device::serve_http(&edge, &device, &rest, request).await
        }
        ["blob", chat, part] if valid_id(chat) => {
            // Decode-then-validate: `#` arrives percent-encoded, and `%2F`
            // decodes to `/`, which fails the part pattern.
            let Some(part) = http::decode_segment(part).filter(|p| valid_part(p)) else {
                return http::error(400, "bad part id");
            };
            tool_blob(&edge, &format!("blob/{chat}/{part}"), request).await
        }
        // Retired attachment mirror: acknowledge and discard.
        ["attachments", _, ..] if method == "PUT" => http::json(&json!({ "ok": true }), 200),
        // Pre-chat2 diff sidecar slot, still published by the checkout diff
        // sync; kept (and readable) so the publish succeeds.
        ["diff", chat] if valid_id(chat) => legacy_diff(&edge, chat, request).await,
        _ => {
            tracing::debug!(%method, %path, "local edge: no such route");
            http::error(404, "not_found")
        }
    }
}

/// The release feed. A local edge carries no artifacts; `latest.txt`
/// names this build so the engine's updater reads "up to date" instead of
/// surfacing a fetch error, and everything else is absent.
fn releases(key: &[&str], method: &str) -> Reply {
    if key == ["latest.txt"] {
        let body = if method == "HEAD" {
            Vec::new()
        } else {
            format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes()
        };
        return http::reply(
            200,
            &[
                ("content-type", "text/plain; charset=utf-8"),
                ("cache-control", "public, max-age=60"),
            ],
            body,
        );
    }
    http::error(404, "not_found")
}

async fn tool_blob(edge: &Edge, key: &str, request: Request<Incoming>) -> Reply {
    match request.method().as_str() {
        "PUT" => {
            let content_type = http::header(&request, "content-type")
                .unwrap_or("text/plain; charset=utf-8")
                .to_owned();
            let body = match http::read_body(request.into_body(), MAX_TOOL_BLOB_BYTES).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            match edge.with_state(|state| store::put_blob(&state.db, key, &content_type, &body)) {
                Ok(()) => http::json(&json!({ "ok": true, "bytes": body.len() }), 200),
                Err(err) => storage_error(err),
            }
        }
        method @ ("GET" | "HEAD") => match edge.with_state(|state| store::get_blob(&state.db, key))
        {
            Ok(Some((content_type, bytes))) => http::reply(
                200,
                &[
                    ("content-type", &content_type),
                    ("cache-control", "private, max-age=300"),
                ],
                if method == "GET" { bytes } else { Vec::new() },
            ),
            Ok(None) => http::error(404, "not_found"),
            Err(err) => storage_error(err),
        },
        _ => http::error(404, "not_found"),
    }
}

async fn legacy_diff(edge: &Edge, chat: &str, request: Request<Incoming>) -> Reply {
    let key = format!("diff/{chat}");
    match request.method().as_str() {
        "POST" => {
            let body = match http::read_body(request.into_body(), MAX_LEGACY_DIFF_BYTES).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            if serde_json::from_slice::<serde_json::Value>(&body).is_err() {
                return http::error(400, "bad_json");
            }
            match edge
                .with_state(|state| store::put_blob(&state.db, &key, "application/json", &body))
            {
                Ok(()) => http::json(&json!({ "ok": true }), 200),
                Err(err) => storage_error(err),
            }
        }
        "GET" => match edge.with_state(|state| store::get_blob(&state.db, &key)) {
            Ok(Some((content_type, bytes))) => {
                http::reply(200, &[("content-type", &content_type)], bytes)
            }
            Ok(None) => http::error(404, "not_found"),
            Err(err) => storage_error(err),
        },
        _ => http::error(404, "not_found"),
    }
}

fn storage_error(err: rusqlite::Error) -> Reply {
    tracing::error!(error = %err, "local edge: storage failed");
    http::error(500, "storage")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_must_be_long_and_url_safe() {
        assert!(valid_token("0123456789abcdef"));
        assert!(!valid_token("short"));
        assert!(!valid_token("0123456789abcdef+"));
        assert!(!valid_token("0123456789abcdef@org"));
    }

    #[test]
    fn part_ids_match_the_worker_pattern() {
        assert!(valid_part("m1#c1"));
        assert!(valid_part("call_x.diff"));
        assert!(!valid_part("a/b"));
        assert!(!valid_part(""));
    }
}
