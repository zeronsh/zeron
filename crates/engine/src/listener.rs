//! HTTP and WebSocket RPC share the engine's remote listener.
//!
//! The remote bind serves browsers and native clients. Plain requests carry
//! `Authorization: Bearer` — a WorkOS access token, verified in
//! [`crate::remote_auth`] exactly as the edge verifies relay clients. A
//! browser WebSocket upgrade cannot set headers, so it authenticates with a
//! first-frame `Auth` envelope carrying the same credential. Local IPC keeps
//! its native-only boundary in `serve_ipc` and is untouched by this module.

use crate::remote_auth::{CodeExchange, RemoteAuthorizer};
use base64::Engine as _;
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::derive_accept_key,
    protocol::{CloseFrame, Role, frame::coding::CloseCode},
};

type Reply = Response<Full<Bytes>>;

/// How long a browser connection may stay silent before its first-frame
/// `Auth` envelope arrives. The credential is sent the moment the socket
/// opens, so this only bounds abandoned sockets.
pub const FIRST_FRAME_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Serve the remote engine listener: one bind answering plain HTTP health
/// and the WebSocket RPC upgrade, every request authenticated.
pub async fn serve_remote_listener(
    listener: TcpListener,
    service: Arc<dyn zeron_rpc::RpcService>,
    authorizer: Arc<RemoteAuthorizer>,
) {
    let cancel = tokio_util::sync::CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    if let Ok(address) = listener.local_addr() {
        tracing::info!(%address, "engine remote listener serving");
    }
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let service = service.clone();
                let authorizer = authorizer.clone();
                let connection_cancel = cancel.clone();
                tokio::spawn(async move {
                    let request_cancel = connection_cancel.clone();
                    let handler = service_fn(move |request| {
                        handle(
                            request,
                            service.clone(),
                            authorizer.clone(),
                            request_cancel.clone(),
                        )
                    });
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder
                        .timer(TokioTimer::new())
                        .header_read_timeout(Duration::from_secs(10))
                        .max_buf_size(16 * 1024);
                    tokio::select! {
                        _ = connection_cancel.cancelled() => {},
                        _ = builder.serve_connection(TokioIo::new(stream), handler).with_upgrades() => {},
                    }
                });
            }
            Err(error) => {
                tracing::warn!(%error, "engine remote listener accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle(
    mut request: Request<Incoming>,
    service: Arc<dyn zeron_rpc::RpcService>,
    authorizer: Arc<RemoteAuthorizer>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Reply, Infallible> {
    // The remote listener serves browsers, so it accepts Origin-carrying
    // requests — the credential is the lock, never the origin.
    if request.uri().query().is_some() {
        return Ok(reply(
            StatusCode::BAD_REQUEST,
            "query parameters are not supported",
        ));
    }
    let path = request.uri().path();
    // A browser WebSocket upgrade carries no Authorization header; validate
    // the upgrade shape once so the session gate can let it through for
    // first-frame authentication.
    let upgrade_key = if path == "/" && request.method() == hyper::Method::GET {
        websocket_key(&request).map(str::to_owned)
    } else {
        None
    };
    // The web client shell must load before authentication, so the asset
    // routes sit ahead of the credential gate; the data behind the socket
    // and the API routes stay credential-gated. The bundle is the staged
    // Vite build (build.rs → web-staging/): the HTML shell at `/` and
    // `/pair`, hashed JS/CSS/font chunks under `/assets/`, and the app shell
    // for any client-side route (the router takes over after a hard
    // refresh). A WebSocket upgrade to `/` is the RPC channel and must NOT
    // be served HTML — it falls through to the credential gate and the
    // upgrade block below.
    if request.method() == hyper::Method::GET && upgrade_key.is_none() {
        if path == "/pair" {
            return Ok(web_page("pair.html"));
        }
        if path == "/" {
            return Ok(web_page("index.html"));
        }
        if let Some(asset) = web_static_asset(path) {
            return Ok(asset);
        }
        // SPA fallback: any non-extension path is a client-side route and
        // gets the app shell. Reserved paths still fall through to the
        // credential gate below.
        if !RESERVED_API_PATHS.contains(&path) && !path.contains('.') {
            return Ok(web_page("index.html"));
        }
    }

    // Pre-auth sign-in routes: a browser must discover the sign-in mode
    // (and exchange its authorization code) before it holds a credential.
    // GET /auth/config reports the mode plus the AuthKit authorize URL;
    // POST /auth/exchange proxies the secret-bearing code exchange to the
    // edge — the WorkOS API key lives only there, and staying same-origin
    // means the browser never needs CORS on the edge.
    if path == "/auth/config" && request.method() == hyper::Method::GET {
        return Ok(json(StatusCode::OK, &authorizer.sign_in_config()));
    }
    if path == "/auth/exchange" && request.method() == hyper::Method::POST {
        return Ok(handle_code_exchange(request, authorizer.clone()).await);
    }

    // Every remote request requires a valid credential, including health and
    // upgrades. A browser upgrade presents no Bearer credential here and
    // authenticates with a first-frame `Auth` envelope after the upgrade;
    // the native Bearer path is unchanged.
    let mut first_frame_auth = false;
    match bearer(&request).map(str::to_owned) {
        Some(credential) => {
            if authorizer.verify(&credential).await.is_none() {
                return Ok(reply(StatusCode::UNAUTHORIZED, "invalid credential"));
            }
        }
        None if upgrade_key.is_some() => first_frame_auth = true,
        None => return Ok(reply(StatusCode::UNAUTHORIZED, "invalid credential")),
    }
    if path == "/health" && request.method() == hyper::Method::GET {
        return Ok(json(StatusCode::OK, &serde_json::json!({"status":"ok"})));
    }
    if path != "/" || request.method() != hyper::Method::GET {
        return Ok(reply(StatusCode::NOT_FOUND, "not found"));
    }
    let Some(key) = upgrade_key else {
        return Ok(reply(StatusCode::BAD_REQUEST, "expected WebSocket upgrade"));
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgraded = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let Ok(io) = upgraded.await else { return };
        let (io, progress) = zeron_rpc::ProgressIo::new(TokioIo::new(io));
        let mut ws =
            tokio_tungstenite::WebSocketStream::from_raw_socket(io, Role::Server, None).await;
        if first_frame_auth {
            let authenticated = tokio::select! {
                _ = cancel.cancelled() => false,
                authenticated = authenticate_first_frame(&mut ws, &authorizer) => authenticated,
            };
            if !authenticated {
                return;
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => {},
            _ = zeron_rpc::serve_websocket(
                zeron_rpc::Connection { socket: ws, progress },
                service,
            ) => {},
        }
    });
    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(Full::new(Bytes::new()))
        .unwrap())
}

/// Gate a browser WebSocket on its first frame: browsers cannot set
/// `Authorization` on the handshake, so the credential arrives as an
/// `Auth` envelope (`{"auth":"<credential>"}`) in the first text frame.
/// A wrong, malformed, or missing credential closes the connection before any
/// RPC is served; `zeron_rpc::serve_websocket` stays agnostic so the
/// unauthenticated local IPC path is untouched.
async fn authenticate_first_frame<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    authorizer: &Arc<RemoteAuthorizer>,
) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let first = match tokio::time::timeout(FIRST_FRAME_AUTH_TIMEOUT, ws.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        Ok(Some(Ok(_))) => return refuse(ws, "invalid credential").await,
        Ok(Some(Err(_))) | Ok(None) => return false,
        Err(_) => return refuse(ws, "authentication timeout").await,
    };
    // The envelope is the first line of the frame; ndjson batching puts
    // nothing before it.
    let credential = first
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|frame| frame.get("auth")?.as_str().map(str::to_owned));
    let Some(credential) = credential else {
        return refuse(ws, "invalid credential").await;
    };
    match authorizer.verify(&credential).await {
        Some(_) => true,
        None => refuse(ws, "invalid credential").await,
    }
}

/// Close with 4401 so a browser can tell a refused session apart from a
/// network drop; the reasons match the HTTP gate's bodies. Always returns
/// false so the caller can `return refuse(...)`.
async fn refuse<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, reason: &'static str) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Library(4401),
            reason: reason.into(),
        })))
        .await;
    false
}

/// The validated WebSocket key when the request is a well-formed upgrade.
fn websocket_key(request: &Request<Incoming>) -> Option<&str> {
    let upgrade = request.headers().get("upgrade")?.to_str().ok()?;
    let connection = request
        .headers()
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let version = request
        .headers()
        .get("sec-websocket-version")
        .and_then(|v| v.to_str().ok());
    let key = request.headers().get("sec-websocket-key")?.to_str().ok()?;
    if !upgrade.eq_ignore_ascii_case("websocket")
        || !connection
            .split(',')
            .any(|v| v.trim().eq_ignore_ascii_case("upgrade"))
        || version != Some("13")
        || !base64::engine::general_purpose::STANDARD
            .decode(key)
            .is_ok_and(|bytes| bytes.len() == 16)
    {
        return None;
    }
    Some(key)
}

fn bearer(request: &Request<Incoming>) -> Option<&str> {
    if request.headers().get_all("authorization").iter().count() != 1 {
        return None;
    }
    request
        .headers()
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn reply(status: StatusCode, body: &'static str) -> Reply {
    Response::builder()
        .status(status)
        .header("cache-control", "no-store")
        .body(Full::new(Bytes::from_static(body.as_bytes())))
        .unwrap()
}

fn json(status: StatusCode, value: &impl serde::Serialize) -> Reply {
    Response::builder()
        .status(status)
        .header("cache-control", "no-store")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(value).expect("serializable response"),
        )))
        .unwrap()
}

/// One embedded web client page. The bundle is compiled in, so a missing
/// name is a build-time mistake, not a runtime case.
fn web_page(name: &str) -> Reply {
    let file = crate::web::WebAssets::get(name).expect("embedded web page");
    Response::builder()
        .status(StatusCode::OK)
        .header("cache-control", "no-store")
        .header("content-type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(file.data.into_owned())))
        .unwrap()
}

/// A static asset from the embedded web bundle: the Vite build's hashed
/// JS/CSS/font/image chunks. The path must not escape the bundle (no `..`
/// segments, no leading `/`), and the answer is `None` when the bundle has
/// no such file — the caller falls through to the SPA shell or 404.
fn web_static_asset(path: &str) -> Option<Reply> {
    if path == "/" {
        return None;
    }
    let stripped = path.strip_prefix('/').unwrap_or(path);
    if stripped.is_empty() || stripped.contains("..") {
        return None;
    }
    let file = crate::web::WebAssets::get(stripped)?;
    let cache_control = static_asset_cache_control(stripped);
    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header("cache-control", cache_control)
            .header("content-type", crate::web::content_type(stripped))
            .body(Full::new(Bytes::from(file.data.into_owned())))
            .unwrap(),
    )
}

/// `immutable` for hashed assets (Vite emits content hashes in the
/// filenames, so the URL changes whenever the bytes do) and `no-store`
/// for everything else. The HTML shell is served via [`web_page`].
fn static_asset_cache_control(name: &str) -> &'static str {
    if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    }
}

/// Paths that must NOT fall through to the SPA shell. They are real engine
/// routes — credential-gated API or the pre-auth sign-in surface — and
/// returning the app shell for them would mask their status codes.
const RESERVED_API_PATHS: &[&str] = &["/health", "/auth/config", "/auth/exchange"];

/// `POST /auth/exchange`: the browser asks the engine to exchange its
/// WorkOS authorization code for tokens. The engine is a public client —
/// the secret-bearing exchange happens at the edge, which holds the
/// WorkOS API key — so this proxies `{edge}/auth/exchange` and passes the
/// edge's token payload through unchanged.
async fn handle_code_exchange(
    request: Request<Incoming>,
    authorizer: Arc<RemoteAuthorizer>,
) -> Reply {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        code: String,
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(request.into_body(), 4096).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        _ => return reply(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let Ok(input) = serde_json::from_slice::<Input>(&body) else {
        return reply(StatusCode::BAD_REQUEST, "expected a sign-in code");
    };
    match authorizer.exchange_code(&input.code).await {
        CodeExchange::Tokens(tokens) => json(StatusCode::OK, &tokens),
        CodeExchange::NotConfigured => json(
            StatusCode::NOT_IMPLEMENTED,
            &serde_json::json!({"error": "workos not configured"}),
        ),
        CodeExchange::Rejected => json(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({"error": "sign-in code was refused or expired"}),
        ),
        CodeExchange::Unreachable => json(
            StatusCode::BAD_GATEWAY,
            &serde_json::json!({"error": "could not reach the sign-in service"}),
        ),
    }
}
