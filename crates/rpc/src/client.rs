//! Client side: request/stream multiplexing over string frames + the WebSocket dialer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{mpsc, oneshot};

use crate::{ClientFrame, RpcError, ServerFrame};

/// Per-stream queue depth. Bounded: route_frame awaits a full queue, pausing
/// the connection reader — transport backpressure instead of unbounded growth
/// when a consumer stalls behind a fast producer (watch frames every 120ms
/// during streaming used to pile up whole-transcript payloads here).
const STREAM_QUEUE_CAP: usize = 256;

enum Pending {
    Call(oneshot::Sender<Result<serde_json::Value, RpcError>>),
    Stream(mpsc::Sender<serde_json::Value>),
    CheckedStream {
        items: mpsc::Sender<serde_json::Value>,
        ready: Option<oneshot::Sender<Result<(), RpcError>>>,
    },
}

struct Shared {
    pending: Mutex<HashMap<u64, Pending>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A multiplexing RPC client over any string-frame duplex ([`crate::memory_client`] or
/// [`connect_ws`]). Cheap to clone-by-Arc internally; use one per connection.
pub struct RpcClient {
    out: mpsc::Sender<String>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
    /// Flips to true once the server side of the duplex is gone.
    closed: tokio::sync::watch::Receiver<bool>,
    /// Why the transport failed, when it did; set before `closed` flips.
    close_reason: Arc<std::sync::OnceLock<String>>,
}

/// Owned stream receiver whose drop immediately cancels the server task.
pub struct RpcSubscription {
    id: u64,
    items: mpsc::Receiver<serde_json::Value>,
    out: mpsc::Sender<String>,
    shared: Arc<Shared>,
}

impl RpcSubscription {
    pub async fn recv(&mut self) -> Option<serde_json::Value> {
        self.items.recv().await
    }
}

impl Drop for RpcSubscription {
    fn drop(&mut self) {
        if self.shared.lock().remove(&self.id).is_none() {
            return;
        }
        let Ok(frame) = serde_json::to_string(&ClientFrame {
            id: self.id,
            method: None,
            params: serde_json::Value::Null,
            cancel: true,
        }) else {
            return;
        };
        match self.out.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(frame)) => {
                let out = self.out.clone();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = out.send(frame).await;
                    });
                }
            }
        }
    }
}

impl RpcClient {
    /// Wrap an existing duplex: `out` carries client frames, `inbound` server frames.
    pub fn new(out: mpsc::Sender<String>, mut inbound: mpsc::Receiver<String>) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
        });
        let reader_shared = shared.clone();
        let reader_out = out.clone();
        let (closed_tx, closed) = tokio::sync::watch::channel(false);
        let reader = tokio::spawn(async move {
            while let Some(payload) = inbound.recv().await {
                for line in payload.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let frame: ServerFrame = match serde_json::from_str(line) {
                        Ok(frame) => frame,
                        Err(err) => {
                            tracing::warn!(error = %err, "rpc: dropping malformed server frame");
                            continue;
                        }
                    };
                    route_frame(&reader_shared, &reader_out, frame).await;
                }
            }
            // Connection closed: fail everything still pending.
            let drained: Vec<Pending> = {
                let mut pending = reader_shared.lock();
                pending.drain().map(|(_, p)| p).collect()
            };
            for entry in drained {
                match entry {
                    Pending::Call(tx) => {
                        let _ = tx.send(Err(RpcError::Closed));
                    }
                    Pending::CheckedStream {
                        ready: Some(ready), ..
                    } => {
                        let _ = ready.send(Err(RpcError::Closed));
                    }
                    Pending::Stream(_) | Pending::CheckedStream { ready: None, .. } => {}
                }
                // Stream item receivers end by sender drop.
            }
            let _ = closed_tx.send(true);
        });
        Self {
            out,
            shared,
            next_id: AtomicU64::new(1),
            reader,
            closed,
            close_reason: Arc::default(),
        }
    }

    /// Why the connection failed (an oversized frame, a reset socket), once
    /// [`Self::closed`] resolved; `None` for a clean close.
    pub fn close_reason(&self) -> Option<String> {
        self.close_reason.get().cloned()
    }

    /// Resolves once the connection has closed (the server went away). An
    /// in-memory client's never does while its service lives.
    pub async fn closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// Unary request.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared.lock().insert(id, Pending::Call(tx));
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
        })
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        rx.await.map_err(|_| RpcError::Closed)?
    }

    /// Typed unary request.
    pub async fn call_as<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| RpcError::BadParams(e.to_string()))
    }

    /// Legacy streaming request. Cancellation on receiver drop is only detected
    /// when another item arrives. Prefer `subscribe_scoped` for owned streams.
    pub async fn subscribe(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<mpsc::Receiver<serde_json::Value>, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(STREAM_QUEUE_CAP);
        self.shared.lock().insert(id, Pending::Stream(tx));
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
        })
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        Ok(rx)
    }

    /// A drop-cancelled stream without waiting for its first item. Unlike a
    /// checked subscription this also works for legitimately silent streams.
    pub async fn subscribe_scoped(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcSubscription, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(STREAM_QUEUE_CAP);
        self.shared.lock().insert(id, Pending::Stream(tx));
        // Own cancellation before the send, including cancellation while the
        // outbound channel is backpressured.
        let subscription = RpcSubscription {
            id,
            items: rx,
            out: self.out.clone(),
            shared: self.shared.clone(),
        };
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
        })
        .await?;
        Ok(subscription)
    }

    /// Streaming request with a server acknowledgement before returning.
    ///
    /// Use this for optional/versioned stream methods: an older server's
    /// `unknown method` is returned distinctly instead of looking like an empty stream.
    pub async fn subscribe_checked(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcSubscription, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (items_tx, items_rx) = mpsc::channel(STREAM_QUEUE_CAP);
        let (ready_tx, ready_rx) = oneshot::channel();
        self.shared.lock().insert(
            id,
            Pending::CheckedStream {
                items: items_tx,
                ready: Some(ready_tx),
            },
        );
        let subscription = RpcSubscription {
            id,
            items: items_rx,
            out: self.out.clone(),
            shared: self.shared.clone(),
        };
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
        })
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        ready_rx.await.map_err(|_| RpcError::Closed)??;
        Ok(subscription)
    }

    async fn send(&self, frame: ClientFrame) -> Result<(), RpcError> {
        let json = serde_json::to_string(&frame)
            .map_err(|e| RpcError::Transport(format!("serialize frame: {e}")))?;
        self.out.send(json).await.map_err(|_| RpcError::Closed)
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn route_frame(shared: &Arc<Shared>, out: &mpsc::Sender<String>, frame: ServerFrame) {
    let id = frame.id;
    if let Some(err) = frame.err {
        let error = wire_error(err);
        match shared.lock().remove(&id) {
            Some(Pending::Call(tx)) => {
                let _ = tx.send(Err(error));
            }
            Some(Pending::CheckedStream {
                ready: Some(ready), ..
            }) => {
                let _ = ready.send(Err(error));
            }
            Some(Pending::Stream(_)) | Some(Pending::CheckedStream { ready: None, .. }) | None => {
                // Stream errored: the sender drop closes the receiver.
                tracing::debug!(id, %error, "rpc: stream ended with error");
            }
        }
        return;
    }
    if let Some(value) = frame.ok {
        let mut pending = shared.lock();
        if matches!(pending.get(&id), Some(Pending::Call(_))) {
            if let Some(Pending::Call(tx)) = pending.remove(&id) {
                let _ = tx.send(Ok(value));
            }
        } else if let Some(Pending::CheckedStream { ready, .. }) = pending.get_mut(&id)
            && let Some(ready) = ready.take()
        {
            let _ = ready.send(Ok(()));
        }
        return;
    }
    if let Some(item) = frame.item {
        let ready = match shared.lock().get_mut(&id) {
            Some(Pending::CheckedStream { ready, .. }) => ready.take(),
            _ => None,
        };
        if let Some(ready) = ready {
            let _ = ready.send(Ok(()));
        }
        // Clone the sender out of the lock: the bounded send must await
        // (backpressure) without holding `shared`.
        let tx = match shared.lock().get(&id) {
            Some(Pending::Stream(tx)) => Some(tx.clone()),
            Some(Pending::CheckedStream { items, .. }) => Some(items.clone()),
            _ => None,
        };
        let dead = match tx {
            Some(tx) => tx.send(item).await.is_err(),
            None => false,
        };
        if dead {
            // Receiver was dropped — cancel server-side and forget the stream.
            shared.lock().remove(&id);
            if let Ok(json) = serde_json::to_string(&ClientFrame {
                id,
                method: None,
                params: serde_json::Value::Null,
                cancel: true,
            }) {
                let _ = out.send(json).await;
            }
        }
        return;
    }
    if frame.done
        && let Some(Pending::CheckedStream {
            ready: Some(ready), ..
        }) = shared.lock().remove(&id)
    {
        let _ = ready.send(Ok(()));
    }
}

fn wire_error(error: String) -> RpcError {
    if let Some(method) = error.strip_prefix("unknown method: ") {
        RpcError::UnknownMethod(method.to_owned())
    } else if let Some(code) = error.strip_prefix("capability error: ") {
        RpcError::Capability(code.to_owned())
    } else {
        RpcError::Failed(error)
    }
}

/// How long a dial may take before we give up.
///
/// This is localhost: a real engine answers in milliseconds. Without a bound,
/// *any* other process holding the port accepts the TCP connection and then
/// never completes the WebSocket handshake, and the caller waits forever — a
/// stranger on port 27654 would hang the app at boot rather than degrade it.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Dial the engine's localhost IPC on `port`, presenting the bearer the
/// engine published ([`crate::ipc_auth`]). Tokens are resolved at dial time
/// — `$ZERON_IPC_TOKEN`, `$ZERON_IPC_TOKEN_FILE`, then
/// `{data_dir}/ipc-token` — and tried in that order, so a stale explicit token
/// falls back to the file the restarted engine just rewrote. With no token at
/// all the dial goes out bare (an engine that predates IPC auth accepts it).
pub async fn connect_ipc(
    port: u16,
    data_dir: Option<&std::path::Path>,
) -> Result<RpcClient, RpcError> {
    let url = format!("ws://127.0.0.1:{port}");
    let tokens = crate::ipc_auth::client_tokens(data_dir);
    if tokens.is_empty() {
        return connect_ws(&url, None).await;
    }
    for token in &tokens {
        match dial(&url, Some(token)).await {
            // A stale or garbled candidate: try the next one.
            Err(DialError::Unauthorized | DialError::Malformed) => continue,
            Err(DialError::Other(error)) => return Err(error),
            Ok(client) => return Ok(client),
        }
    }
    let file = data_dir
        .map(|dir| crate::ipc_auth::token_path(dir).display().to_string())
        .unwrap_or_else(|| "the engine's ipc-token file".into());
    Err(RpcError::Transport(format!(
        "the engine on 127.0.0.1:{port} rejected this client's IPC token (401) — \
         it must match {file} of the running engine"
    )))
}

/// Dial a WebSocket RPC server (`ws://127.0.0.1:{ipc_port}`), presenting
/// `token` as `Authorization: Bearer` when given. Prefer [`connect_ipc`] for
/// the engine's own port: it reads the current token for you.
pub async fn connect_ws(url: &str, token: Option<&str>) -> Result<RpcClient, RpcError> {
    dial(url, token).await.map_err(|error| match error {
        DialError::Unauthorized => {
            RpcError::Transport(format!("{url} rejected the IPC token (401 Unauthorized)"))
        }
        DialError::Malformed => RpcError::Transport("malformed IPC token".into()),
        DialError::Other(error) => error,
    })
}

enum DialError {
    /// The server answered the handshake with 401: wrong or missing bearer.
    Unauthorized,
    /// The token can't be sent as a header value (a garbled token file).
    Malformed,
    Other(RpcError),
}

async fn dial(url: &str, token: Option<&str>) -> Result<RpcClient, DialError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, header};

    let mut request = url
        .into_client_request()
        .map_err(|e| DialError::Other(RpcError::Transport(e.to_string())))?;
    if let Some(token) = token {
        let value =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| DialError::Malformed)?;
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    let (ws, _) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        // Nagle off: terminal keystrokes are tiny frames that must not wait
        // for the previous one's ACK.
        tokio_tungstenite::connect_async_with_config(request, Some(crate::ws::config()), true),
    )
    .await
    .map_err(|_| DialError::Other(RpcError::Transport(format!("timed out dialing {url}"))))?
    .map_err(|e| match e {
        tokio_tungstenite::tungstenite::Error::Http(response)
            if response.status() == StatusCode::UNAUTHORIZED =>
        {
            DialError::Unauthorized
        }
        other => DialError::Other(RpcError::Transport(other.to_string())),
    })?;
    let (out_tx, out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);
    let client = RpcClient::new(out_tx, in_rx);
    let close_reason = client.close_reason.clone();
    let url = url.to_owned();
    tokio::spawn(async move {
        if let Some(reason) = crate::ws::pump(ws, out_rx, in_tx).await {
            tracing::warn!(%reason, %url, "rpc: lost the connection");
            let _ = close_reason.set(reason);
        }
    });
    Ok(client)
}
