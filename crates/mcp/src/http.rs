//! Zeron's MCP tools served by the engine itself, over loopback Streamable
//! HTTP — so an agent's run needs no `zeron mcp` process of its own.
//!
//! **Stateless.** The server keeps nothing per client connection: every
//! request is authenticated by its run's bearer token and stands on its own,
//! so an agent stopped and resumed (`--resume`) re-establishes nothing. It
//! speaks both eras of the protocol on one endpoint (`/mcp`):
//!
//! - **Modern** (`2026-07-28`): a request whose `params._meta` carries
//!   `io.modelcontextprotocol/protocolVersion` is served per that revision —
//!   no `initialize`, `server/discover`, the `MCP-Protocol-Version` /
//!   `Mcp-Method` / `Mcp-Name` headers checked against the body (`400` +
//!   `HeaderMismatch` otherwise), `resultType` on every result, list changes
//!   on a `subscriptions/listen` stream.
//! - **Legacy** (`2024-11-05` … `2025-11-25`): `initialize` is answered
//!   without an `Mcp-Session-Id` (the 2025 transport makes sessions
//!   optional), and a session header a client sends anyway is ignored.
//!
//! Either way a call answers within its own POST: a long one (a
//! `wait_for_turn`, a send that waits) streams SSE with progress
//! notifications and keep-alive comments, then its response. Closing that
//! stream cancels the call; nothing is resumable. GET and DELETE answer
//! `405`, as the 2026 transport says (a 2025 client then has no standalone
//! stream, which that revision allows).
//!
//! **Security.** Bound to `127.0.0.1` only, on a port of its own (never the
//! IPC port). Every request must present `Authorization: Bearer <token>`,
//! a token the engine minted for ONE run ([`McpHttpServer::grant`]): it
//! names that run's chat, so whatever a request does it does as that chat,
//! and it stops working when the run ends ([`Grant`] drops). A request
//! carrying `Origin` (any browser page) is refused, and so is one whose
//! `Host` isn't the loopback address (DNS rebinding). The token reaches what
//! the old stdio server's agent token reached — the engine's
//! agent-scoped methods ([`zeron_rpc::agent_allowed`]) — and no more; the
//! methods that act on "the calling chat" are pinned to the token's chat.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::jsonrpc::{cap_reply, error_response, handle_request};
use crate::tools::Tools;
use crate::zeron::Origin;

/// The modern revisions served statelessly.
pub const MODERN_VERSIONS: [&str; 1] = ["2026-07-28"];
/// The `initialize`-era revisions answered on this endpoint (the stdio
/// server's, plus the last 2025 revision a client may ask for).
pub const LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const META_SUBSCRIPTION: &str = "io.modelcontextprotocol/subscriptionId";

/// JSON-RPC codes the 2026 revision reserves.
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
const PARSE_ERROR: i64 = -32700;

/// The largest request body read. Tool arguments are small (images and
/// visual assets travel as paths).
const MAX_BODY: usize = 8 * 1024 * 1024;
/// A call still running sends a progress notification this often (when the
/// client gave a progress token), as the stdio server does.
const PROGRESS_EVERY: Duration = Duration::from_secs(30);
/// An SSE stream with nothing to say sends a comment this often, so no idle
/// timeout on the client's side closes it (clients ignore comments).
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(15);
/// How long a client may cache `tools/list` (modern `ttlMs`): plugin tools
/// come and go, and a client not listening for changes re-lists after this.
const TOOLS_TTL_MS: u64 = 30_000;

type Body = UnsyncBoxBody<Bytes, Infallible>;

/// Builds the tools one grant's requests run: Zeron's tools speaking for
/// `origin`, over a connection to the engine scoped to `origin`'s chat.
pub type ToolsFactory = Arc<dyn Fn(&Origin) -> Arc<Tools> + Send + Sync>;

/// One run's access: who it speaks for, its tools (made at its first
/// call), and its calls in flight (so a 2025 client's
/// `notifications/cancelled` can stop one). Not connection state: the same
/// for every request the run's token makes.
struct GrantState {
    token: String,
    origin: Origin,
    tools: std::sync::OnceLock<Arc<Tools>>,
    in_flight: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

struct Inner {
    port: u16,
    factory: ToolsFactory,
    grants: Mutex<Vec<Arc<GrantState>>>,
}

/// A run's access to the endpoint: its token works until this drops.
pub struct Grant {
    inner: Weak<Inner>,
    state: Arc<GrantState>,
}

impl Grant {
    /// The bearer token (`Authorization: Bearer …`).
    pub fn token(&self) -> &str {
        &self.state.token
    }
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant").field("origin", &self.state.origin).finish()
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        lock(&inner.grants).retain(|grant| !Arc::ptr_eq(grant, &self.state));
        // Calls still running for a finished run end with it.
        for (_, call) in lock(&self.state.in_flight).drain() {
            call.abort();
        }
    }
}

/// The engine's MCP endpoint. Dropping it stops the listener.
pub struct McpHttpServer {
    inner: Arc<Inner>,
    accept: tokio::task::JoinHandle<()>,
}

impl Drop for McpHttpServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl McpHttpServer {
    /// Bind `127.0.0.1` on a free port and serve until dropped.
    pub async fn bind(factory: ToolsFactory) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            port,
            factory,
            grants: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&inner);
        let accept = tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        tracing::warn!(error = %err, "mcp http: accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Some(inner) = weak.upgrade() else { return };
                tokio::spawn(serve_socket(inner, stream, peer));
            }
        });
        tracing::info!(port, "agent MCP endpoint listening");
        Ok(Self { inner, accept })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// `http://127.0.0.1:{port}/mcp`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.inner.port)
    }

    /// A fresh token for one run speaking as `origin`.
    pub fn grant(&self, origin: Origin) -> std::io::Result<Grant> {
        let token = zeron_rpc::ipc_auth::generate_token()?;
        let state = Arc::new(GrantState {
            token,
            origin,
            tools: std::sync::OnceLock::new(),
            in_flight: Mutex::new(HashMap::new()),
        });
        lock(&self.inner.grants).push(state.clone());
        Ok(Grant {
            inner: Arc::downgrade(&self.inner),
            state,
        })
    }

    /// Tool calls running now, across runs (tests, diagnostics).
    pub fn calls_in_flight(&self) -> usize {
        lock(&self.inner.grants)
            .iter()
            .map(|grant| lock(&grant.in_flight).len())
            .sum()
    }

    /// How many runs hold a live token (tests, diagnostics).
    pub fn live_grants(&self) -> usize {
        lock(&self.inner.grants).len()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn serve_socket(inner: Arc<Inner>, stream: tokio::net::TcpStream, peer: SocketAddr) {
    if !peer.ip().is_loopback() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let service = hyper::service::service_fn(move |request| {
        let inner = inner.clone();
        async move { Ok::<_, Infallible>(handle(inner, request).await) }
    });
    let io = hyper_util::rt::TokioIo::new(stream);
    if let Err(err) = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(io, service)
        .await
    {
        tracing::debug!(error = %err, "mcp http: connection ended");
    }
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).boxed_unsync()
}

fn empty(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(full(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn json_response(status: StatusCode, value: &Value) -> Response<Body> {
    let mut response = Response::new(full(value.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

/// A JSON-RPC error with no request id, for refusals before the body is read.
fn refusal(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, &error_response(Value::Null, INVALID_REQUEST, message))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The `Host` a loopback client sends: `127.0.0.1`, `localhost` or `[::1]`,
/// on this port. Anything else came through a name that resolved here
/// (DNS rebinding).
fn host_allowed(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = header(headers, "host") else {
        return false;
    };
    let host = host.trim().to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|name| host == format!("{name}:{port}"))
}

/// Constant-time equality, so a wrong token's timing says nothing about a
/// right one.
fn same_token(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn find_grant(inner: &Inner, headers: &HeaderMap) -> Option<Arc<GrantState>> {
    let presented = header(headers, "authorization")?.strip_prefix("Bearer ")?.trim();
    if presented.is_empty() {
        return None;
    }
    // Every grant compared, so the match's position leaks nothing either.
    let grants = lock(&inner.grants);
    let mut found = None;
    for grant in grants.iter() {
        if same_token(&grant.token, presented) && found.is_none() {
            found = Some(grant.clone());
        }
    }
    found
}

/// Decode a `=?base64?…?=` header value (2026 transport, Value Encoding).
fn decode_header_value(value: &str) -> Option<String> {
    use base64::Engine as _;
    match value
        .strip_prefix("=?base64?")
        .and_then(|rest| rest.strip_suffix("?="))
    {
        Some(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok()),
        None => Some(value.to_owned()),
    }
}

/// Which era a message speaks, read from its own `_meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Era<'a> {
    Modern(&'a str),
    Legacy,
}

fn era(message: &Value) -> Era<'_> {
    match message
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(META_VERSION))
        .and_then(Value::as_str)
    {
        Some(version) => Era::Modern(version),
        None => Era::Legacy,
    }
}

fn mismatch(id: Value, message: String) -> Response<Body> {
    json_response(
        StatusCode::BAD_REQUEST,
        &error_response(id, HEADER_MISMATCH, &message),
    )
}

/// The 2026 header checks: `MCP-Protocol-Version` and `Mcp-Method` on every
/// request, `Mcp-Name` where the method names a tool, each matching the body.
fn check_modern_headers(
    headers: &HeaderMap,
    id: &Value,
    version: &str,
    method: &str,
    params: &Value,
) -> Result<(), Response<Body>> {
    match header(headers, "mcp-protocol-version") {
        Some(sent) if sent == version => {}
        Some(sent) => {
            return Err(mismatch(
                id.clone(),
                format!("Header mismatch: MCP-Protocol-Version '{sent}' does not match body '{version}'"),
            ));
        }
        None => {
            return Err(mismatch(id.clone(), "Missing MCP-Protocol-Version header".into()));
        }
    }
    match header(headers, "mcp-method") {
        Some(sent) if sent == method => {}
        Some(sent) => {
            return Err(mismatch(
                id.clone(),
                format!("Header mismatch: Mcp-Method '{sent}' does not match body '{method}'"),
            ));
        }
        None => return Err(mismatch(id.clone(), "Missing Mcp-Method header".into())),
    }
    let named = match method {
        "tools/call" | "prompts/get" => params.get("name").and_then(Value::as_str),
        "resources/read" => params.get("uri").and_then(Value::as_str),
        _ => return Ok(()),
    };
    let Some(named) = named else {
        return Ok(());
    };
    match header(headers, "mcp-name").map(decode_header_value) {
        Some(Some(sent)) if sent == named => Ok(()),
        Some(Some(sent)) => Err(mismatch(
            id.clone(),
            format!("Header mismatch: Mcp-Name header value '{sent}' does not match body value '{named}'"),
        )),
        Some(None) => Err(mismatch(id.clone(), "Malformed Mcp-Name header".into())),
        None => Err(mismatch(id.clone(), "Missing Mcp-Name header".into())),
    }
}

fn server_info() -> Value {
    json!({ "name": "zeron", "version": env!("CARGO_PKG_VERSION") })
}

/// A modern result: `resultType`, and the server named in `_meta`.
fn modernize(mut response: Value, method: &str) -> Value {
    if let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) {
        result.insert("resultType".into(), json!("complete"));
        let meta = result
            .entry("_meta")
            .or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(META_SERVER_INFO.into(), server_info());
        }
        if method == "tools/list" {
            result.insert("ttlMs".into(), json!(TOOLS_TTL_MS));
            result.insert("cacheScope".into(), json!("private"));
        }
    }
    response
}

fn tools_for(inner: &Inner, grant: &GrantState) -> Arc<Tools> {
    grant
        .tools
        .get_or_init(|| (inner.factory)(&grant.origin))
        .clone()
}

async fn handle(inner: Arc<Inner>, request: Request<hyper::body::Incoming>) -> Response<Body> {
    let headers = request.headers().clone();
    // Any browser page carries `Origin`; nothing that should reach this does.
    if headers.contains_key("origin") {
        return refusal(StatusCode::FORBIDDEN, "origin not allowed");
    }
    if !host_allowed(&headers, inner.port) {
        return refusal(StatusCode::FORBIDDEN, "host not allowed");
    }
    if request.uri().path() != "/mcp" {
        return empty(StatusCode::NOT_FOUND);
    }
    if request.method() != Method::POST {
        // No standalone stream (2026: removed) and no sessions to delete.
        let mut response = empty(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert("allow", HeaderValue::from_static("POST"));
        return response;
    }
    let Some(grant) = find_grant(&inner, &headers) else {
        let mut response = refusal(StatusCode::UNAUTHORIZED, "missing or wrong token");
        response
            .headers_mut()
            .insert("www-authenticate", HeaderValue::from_static("Bearer"));
        return response;
    };
    let body = match http_body_util::Limited::new(request.into_body(), MAX_BODY)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return refusal(StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                &error_response(Value::Null, PARSE_ERROR, &format!("parse error: {err}")),
            );
        }
    };
    if !message.is_object() {
        return refusal(StatusCode::BAD_REQUEST, "expected one JSON-RPC object");
    }
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str).map(str::to_owned) else {
        // A JSON-RPC response: the server sends no requests to answer.
        return refusal(StatusCode::BAD_REQUEST, "expected a request or notification");
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let era = era(&message);
    // What each agent speaks, for the record (which revision it negotiates).
    match (era, method.as_str()) {
        (Era::Legacy, "initialize") => tracing::info!(
            chat = ?grant.origin.chat_id,
            version = %params["protocolVersion"],
            client = %params["clientInfo"],
            header = header(&headers, "mcp-protocol-version").unwrap_or("-"),
            "agent MCP: legacy initialize"
        ),
        (Era::Modern(version), "server/discover") => tracing::info!(
            chat = ?grant.origin.chat_id,
            version,
            client = %params["_meta"]["io.modelcontextprotocol/clientInfo"],
            "agent MCP: modern discover"
        ),
        _ => tracing::debug!(chat = ?grant.origin.chat_id, method, ?era, "agent MCP request"),
    }

    let Some(id) = id else {
        // Notifications. The 2026 transport has none (closing a stream is
        // the cancel); a 2025 client cancels by id.
        if method == "notifications/cancelled"
            && let Some(key) = params.get("requestId").map(Value::to_string)
            && let Some(call) = lock(&grant.in_flight).remove(&key)
        {
            call.abort();
        }
        return empty(StatusCode::ACCEPTED);
    };

    match era {
        Era::Modern(version) => {
            if !MODERN_VERSIONS.contains(&version) {
                let supported: Vec<&str> =
                    MODERN_VERSIONS.iter().chain(LEGACY_VERSIONS.iter()).copied().collect();
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": UNSUPPORTED_VERSION,
                            "message": "Unsupported protocol version",
                            "data": { "supported": supported, "requested": version },
                        },
                    }),
                );
            }
            if let Err(refused) = check_modern_headers(&headers, &id, version, &method, &params) {
                return refused;
            }
            match method.as_str() {
                "server/discover" => {
                    let tools = tools_for(&inner, &grant);
                    json_response(
                        StatusCode::OK,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "resultType": "complete",
                                "supportedVersions": MODERN_VERSIONS,
                                "capabilities": { "tools": { "listChanged": true } },
                                "_meta": { META_SERVER_INFO: server_info() },
                                "instructions": crate::jsonrpc::instructions(
                                    tools.waits_on_background(),
                                    tools.side_chats_only(),
                                ),
                                "ttlMs": TOOLS_TTL_MS,
                                "cacheScope": "private",
                            },
                        }),
                    )
                }
                "subscriptions/listen" => listen(&inner, &grant, id, &params),
                // Gone in this revision (handshake, liveness by request).
                "initialize" | "ping" | "logging/setLevel" => json_response(
                    StatusCode::NOT_FOUND,
                    &error_response(id, METHOD_NOT_FOUND, &format!("unknown method: {method}")),
                ),
                "tools/call" => call(&inner, &grant, &headers, id, method, params, true),
                _ => {
                    let tools = tools_for(&inner, &grant);
                    let response = handle_request(&tools, id, &method, params).await;
                    let status = if response["error"]["code"] == METHOD_NOT_FOUND {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::OK
                    };
                    json_response(status, &modernize(response, &method))
                }
            }
        }
        Era::Legacy => {
            if method != "initialize"
                && let Some(sent) = header(&headers, "mcp-protocol-version")
                && !LEGACY_VERSIONS.contains(&sent)
            {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    &format!("unsupported MCP-Protocol-Version: {sent}"),
                );
            }
            match method.as_str() {
                "tools/call" => call(&inner, &grant, &headers, id, method, params, false),
                // `initialize` included: answered with no session id, so the
                // client sends none (and one sent anyway is ignored).
                _ => {
                    let tools = tools_for(&inner, &grant);
                    json_response(StatusCode::OK, &handle_request(&tools, id, &method, params).await)
                }
            }
        }
    }
}

/// Aborts the call it guards when the response stream is dropped: the
/// client closed it (the 2026 cancel) or the connection went.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// [`AbortOnDrop`] for a call: it also leaves its grant's calls in flight.
struct CallGuard {
    call: tokio::task::AbortHandle,
    grant: Weak<GrantState>,
    key: String,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.call.abort();
        if let Some(grant) = self.grant.upgrade() {
            lock(&grant.in_flight).remove(&self.key);
        }
    }
}

fn sse_headers(response: &mut Response<Body>) {
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("text/event-stream"));
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
}

fn sse_event(message: &Value) -> Bytes {
    Bytes::from(format!("event: message\ndata: {message}\n\n"))
}

/// A body fed by `rx`; `guard` lives (and its call runs) as long as the
/// body does.
fn sse_body<G: Send + 'static>(rx: mpsc::Receiver<Bytes>, guard: G) -> Body {
    let stream = futures::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<_, Infallible>(Frame::data(chunk)), (rx, guard)))
    });
    BodyExt::boxed_unsync(StreamBody::new(stream))
}

/// `tools/call`: run on a task of its own, answered on an SSE stream (with
/// progress and keep-alives while it runs) when the client takes one, else
/// as one JSON object once it ends.
fn call(
    inner: &Arc<Inner>,
    grant: &Arc<GrantState>,
    headers: &HeaderMap,
    id: Value,
    method: String,
    params: Value,
    modern: bool,
) -> Response<Body> {
    let tools = tools_for(inner, grant);
    let progress_token = params
        .get("_meta")
        .and_then(|meta| meta.get("progressToken"))
        .cloned();
    let streams = header(headers, "accept").is_some_and(|accept| accept.contains("text/event-stream"));
    let key = id.to_string();
    let (out_tx, out_rx) = mpsc::channel::<Bytes>(16);
    let registry = Arc::downgrade(grant);
    let done_key = key.clone();
    let mut in_flight = lock(&grant.in_flight);
    let task = tokio::spawn(async move {
        let work = handle_request(&tools, id, &method, params);
        tokio::pin!(work);
        let mut progress = tokio::time::interval_at(
            tokio::time::Instant::now() + PROGRESS_EVERY,
            PROGRESS_EVERY,
        );
        let mut keep_alive = tokio::time::interval_at(
            tokio::time::Instant::now() + KEEP_ALIVE_EVERY,
            KEEP_ALIVE_EVERY,
        );
        let mut beat = 0u64;
        let response = loop {
            tokio::select! {
                response = &mut work => break response,
                _ = progress.tick(), if streams && progress_token.is_some() => {
                    beat += 1;
                    let note = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": { "progressToken": progress_token, "progress": beat, "message": "still working" },
                    });
                    let _ = out_tx.send(sse_event(&note)).await;
                }
                _ = keep_alive.tick(), if streams => {
                    let _ = out_tx.send(Bytes::from_static(b": keep-alive\n\n")).await;
                }
            }
        };
        let response = cap_reply(response);
        let response = if modern { modernize(response, "tools/call") } else { response };
        let bytes = if streams {
            sse_event(&response)
        } else {
            Bytes::from(response.to_string())
        };
        let _ = out_tx.send(bytes).await;
        if let Some(grant) = registry.upgrade() {
            lock(&grant.in_flight).remove(&done_key);
        }
    });
    in_flight.insert(key.clone(), task.abort_handle());
    drop(in_flight);
    let guard = CallGuard {
        call: task.abort_handle(),
        grant: Arc::downgrade(grant),
        key,
    };
    if streams {
        let mut response = Response::new(sse_body(out_rx, guard));
        sse_headers(&mut response);
        response
    } else {
        // One JSON object: the body is the single message the task sends.
        let mut response = Response::new(sse_body(out_rx, guard));
        response
            .headers_mut()
            .insert("content-type", HeaderValue::from_static("application/json"));
        response
    }
}

/// `subscriptions/listen` (2026): acknowledge what's honoured — tool-list
/// changes only — and then send each one, tagged with the subscription, on
/// this stream until the client closes it.
fn listen(inner: &Arc<Inner>, grant: &Arc<GrantState>, id: Value, params: &Value) -> Response<Body> {
    let wants_tools = params
        .get("notifications")
        .and_then(|filter| filter.get("toolsListChanged"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tools = tools_for(inner, grant);
    let (out_tx, out_rx) = mpsc::channel::<Bytes>(16);
    let task = tokio::spawn(async move {
        let honoured = if wants_tools {
            json!({ "toolsListChanged": true })
        } else {
            json!({})
        };
        let ack = json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": { "_meta": { META_SUBSCRIPTION: id }, "notifications": honoured },
        });
        if out_tx.send(sse_event(&ack)).await.is_err() {
            return;
        }
        let (changed_tx, mut changed) = mpsc::channel::<()>(1);
        let watch = async {
            if wants_tools {
                tools.watch_plugin_tools(changed_tx).await;
            }
            std::future::pending::<()>().await;
        };
        let forward = async {
            let mut keep_alive = tokio::time::interval_at(
                tokio::time::Instant::now() + KEEP_ALIVE_EVERY,
                KEEP_ALIVE_EVERY,
            );
            loop {
                let chunk = tokio::select! {
                    Some(()) = changed.recv() => sse_event(&json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/tools/list_changed",
                        "params": { "_meta": { META_SUBSCRIPTION: id } },
                    })),
                    _ = keep_alive.tick() => Bytes::from_static(b": keep-alive\n\n"),
                };
                if out_tx.send(chunk).await.is_err() {
                    return;
                }
            }
        };
        tokio::select! {
            () = watch => {}
            () = forward => {}
        }
    });
    let mut response = Response::new(sse_body(out_rx, AbortOnDrop(task.abort_handle())));
    sse_headers(&mut response);
    response
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
