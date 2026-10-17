//! Client side: request/stream multiplexing over string frames + the WebSocket dialer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

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

/// What a redialing transport feeds the reader. One ordered channel, so a
/// `Reset` can never overtake frames from the connection it ends, nor fail
/// requests already sent on the next one.
enum Inbound {
    Frame(String),
    /// The socket dropped: fail everything pending, keep the client alive.
    Reset,
    /// Request `id` was sent while disconnected and never reached a server.
    Fail(u64),
}

enum InboundSource {
    Plain(mpsc::Receiver<String>),
    Redialing(mpsc::Receiver<Inbound>),
}

impl InboundSource {
    async fn recv(&mut self) -> Option<Inbound> {
        match self {
            Self::Plain(rx) => rx.recv().await.map(Inbound::Frame),
            Self::Redialing(rx) => rx.recv().await,
        }
    }
}

impl RpcClient {
    /// Wrap an existing duplex: `out` carries client frames, `inbound` server frames.
    pub fn new(out: mpsc::Sender<String>, inbound: mpsc::Receiver<String>) -> Self {
        Self::with_source(out, InboundSource::Plain(inbound))
    }

    fn with_source(out: mpsc::Sender<String>, mut inbound: InboundSource) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
        });
        let reader_shared = shared.clone();
        let reader_out = out.clone();
        let reader = tokio::spawn(async move {
            while let Some(event) = inbound.recv().await {
                match event {
                    Inbound::Frame(payload) => {
                        for line in payload.lines() {
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }
                            let frame: ServerFrame = match serde_json::from_str(line) {
                                Ok(frame) => frame,
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "rpc: dropping malformed server frame"
                                    );
                                    continue;
                                }
                            };
                            route_frame(&reader_shared, &reader_out, frame).await;
                        }
                    }
                    Inbound::Reset => fail_pending(&reader_shared, None),
                    Inbound::Fail(id) => fail_pending(&reader_shared, Some(id)),
                }
            }
            // Connection closed: fail everything still pending.
            fail_pending(&reader_shared, None);
        });
        Self {
            out,
            shared,
            next_id: AtomicU64::new(1),
            reader,
        }
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

/// Fail one pending request (`Some(id)`) or all of them with `Closed`.
fn fail_pending(shared: &Shared, id: Option<u64>) {
    let drained: Vec<Pending> = {
        let mut pending = shared.lock();
        match id {
            Some(id) => pending.remove(&id).into_iter().collect(),
            None => pending.drain().map(|(_, p)| p).collect(),
        }
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

/// Dial a WebSocket RPC server (`ws://127.0.0.1:{ipc_port}`).
pub async fn connect_ws(url: &str) -> Result<RpcClient, RpcError> {
    let ws = dial(url).await?;
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<Inbound>(256);
    tokio::spawn(async move {
        pump(ws, &mut out_rx, &in_tx).await;
    });
    Ok(RpcClient::with_source(
        out_tx,
        InboundSource::Redialing(in_rx),
    ))
}

const REDIAL_MIN: std::time::Duration = std::time::Duration::from_millis(100);
const REDIAL_MAX: std::time::Duration = std::time::Duration::from_secs(2);

/// [`connect_ws`] for a long-lived viewport of a daemon that can restart
/// underneath it (self-update, `systemctl restart`). The first dial fails
/// like `connect_ws`; after that a dropped socket redials forever with
/// backoff instead of leaving a permanently closed client.
///
/// A drop fails everything in flight with [`RpcError::Closed`] and ends every
/// open stream, exactly as a closed socket does — callers that resubscribe on
/// stream end recover by themselves once the daemon is back. Requests made
/// while disconnected fail fast rather than queueing for a server that may
/// never return; ones made mid-dial go out on the new socket.
pub async fn connect_ws_redialing(url: &str) -> Result<RpcClient, RpcError> {
    let ws = dial(url).await?;
    let (out_tx, out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<Inbound>(256);
    tokio::spawn(redial_loop(url.to_owned(), ws, out_rx, in_tx));
    Ok(RpcClient::with_source(
        out_tx,
        InboundSource::Redialing(in_rx),
    ))
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn dial(url: &str) -> Result<WsStream, RpcError> {
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| RpcError::Transport(format!("timed out dialing {url}")))?
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    Ok(ws)
}

/// Why a socket pump stopped.
enum PumpEnd {
    /// The `RpcClient` (or its reader) is gone; nothing left to serve.
    ClientGone,
    /// The socket closed or errored.
    SocketClosed,
}

/// Shuttle frames between one socket and the client until either side ends.
async fn pump(
    ws: WsStream,
    out_rx: &mut mpsc::Receiver<String>,
    in_tx: &mpsc::Sender<Inbound>,
) -> PumpEnd {
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            frame = out_rx.recv() => match frame {
                Some(text) => {
                    if sink.send(WsMessage::Text(text)).await.is_err() {
                        return PumpEnd::SocketClosed;
                    }
                }
                None => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    return PumpEnd::ClientGone;
                }
            },
            message = stream.next() => match message {
                Some(Ok(WsMessage::Text(text))) => {
                    if in_tx.send(Inbound::Frame(text)).await.is_err() {
                        return PumpEnd::ClientGone;
                    }
                }
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => {
                    return PumpEnd::SocketClosed;
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn redial_loop(
    url: String,
    mut ws: WsStream,
    mut out_rx: mpsc::Receiver<String>,
    in_tx: mpsc::Sender<Inbound>,
) {
    loop {
        if let PumpEnd::ClientGone = pump(ws, &mut out_rx, &in_tx).await {
            return;
        }
        if in_tx.send(Inbound::Reset).await.is_err() {
            return;
        }
        tracing::info!(%url, "rpc: connection lost; redialing");
        let mut delay = REDIAL_MIN;
        ws = loop {
            let wait = tokio::time::sleep(delay);
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    () = &mut wait => break,
                    frame = out_rx.recv() => {
                        let Some(text) = frame else {
                            return;
                        };
                        // Cancels for streams the dead server already
                        // forgot need no reply; requests fail now.
                        if let Some(id) = request_id(&text)
                            && in_tx.send(Inbound::Fail(id)).await.is_err()
                        {
                            return;
                        }
                    }
                }
            }
            match dial(&url).await {
                Ok(ws) => break ws,
                Err(err) => {
                    tracing::debug!(%url, error = %err, "rpc: redial failed");
                    delay = (delay * 2).min(REDIAL_MAX);
                }
            }
        };
        tracing::info!(%url, "rpc: reconnected");
    }
}

fn request_id(frame: &str) -> Option<u64> {
    serde_json::from_str::<ClientFrame>(frame)
        .ok()
        .filter(|frame| frame.method.is_some())
        .map(|frame| frame.id)
}
