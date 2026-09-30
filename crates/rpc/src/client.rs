//! Client side: request/stream multiplexing over string frames + the WebSocket dialer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{ClientFrame, RpcError, ServerFrame};

/// Per-stream queue depth. Bounded: route_frame awaits a full queue, pausing
/// the connection reader — transport backpressure instead of unbounded growth
/// when a consumer stalls behind a fast producer (watch frames every 120ms
/// during streaming used to pile up whole-transcript payloads here).
const STREAM_QUEUE_CAP: usize = 256;

/// One dialed connection: frames out, frames in.
pub type Transport = (mpsc::Sender<String>, mpsc::Receiver<String>);
/// Re-establishes a dropped connection.
pub type Dial = Arc<dyn Fn() -> BoxFuture<'static, Result<Transport, RpcError>> + Send + Sync>;

/// A connection that lasted at least this long counts as healthy: the next
/// redial after it starts from the shortest backoff again.
const REDIAL_STABLE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// Redial delays for a localhost engine: it is either restarting (sub-second
/// to a few seconds) or gone, so back off quickly and cap low.
const REDIAL_BACKOFF: [std::time::Duration; 5] = [
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(250),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
];

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
///
/// Built with [`RpcClient::new`] or [`connect_ws`] it dies with its transport;
/// built with [`connect_ws_redialing`] it redials instead. Requests in flight
/// when the transport drops fail with [`RpcError::Closed`] either way.
pub struct RpcClient {
    out: Arc<Mutex<mpsc::Sender<String>>>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
    reconnects: watch::Sender<u64>,
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
    pub fn new(out: mpsc::Sender<String>, inbound: mpsc::Receiver<String>) -> Self {
        Self::with_transport(out, inbound, None)
    }

    /// Like [`RpcClient::new`], but when the transport closes and `dial` is
    /// given, keep redialing (with backoff) and resume on the new transport.
    pub fn with_transport(
        out: mpsc::Sender<String>,
        inbound: mpsc::Receiver<String>,
        dial: Option<Dial>,
    ) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
        });
        let out = Arc::new(Mutex::new(out));
        let (reconnects, _) = watch::channel(0u64);
        let reader = tokio::spawn({
            let shared = shared.clone();
            let out = out.clone();
            let reconnects = reconnects.clone();
            async move {
                let mut inbound = inbound;
                // Consecutive connections that died young: the next redial waits
                // (a server that accepts and immediately closes must not be
                // redialed in a hot loop).
                let mut short_lived = 0usize;
                let mut connected_at = std::time::Instant::now();
                loop {
                    let sender = out.lock().unwrap_or_else(PoisonError::into_inner).clone();
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
                            route_frame(&shared, &sender, frame).await;
                        }
                    }
                    fail_pending(&shared);
                    let Some(dial) = dial.as_ref() else { break };
                    if connected_at.elapsed() < REDIAL_STABLE_AFTER {
                        let delay = REDIAL_BACKOFF[short_lived.min(REDIAL_BACKOFF.len() - 1)];
                        short_lived += 1;
                        tokio::time::sleep(delay).await;
                    } else {
                        short_lived = 0;
                    }
                    let mut attempt = 0usize;
                    let (new_out, new_in) = loop {
                        match dial().await {
                            Ok(transport) => break transport,
                            Err(err) => {
                                tracing::debug!(error = %err, "rpc: redial failed");
                                let delay = REDIAL_BACKOFF[attempt.min(REDIAL_BACKOFF.len() - 1)];
                                attempt += 1;
                                tokio::time::sleep(delay).await;
                            }
                        }
                    };
                    *out.lock().unwrap_or_else(PoisonError::into_inner) = new_out;
                    inbound = new_in;
                    connected_at = std::time::Instant::now();
                    reconnects.send_modify(|n| *n += 1);
                }
            }
        });
        Self {
            out,
            shared,
            next_id: AtomicU64::new(1),
            reader,
            reconnects,
        }
    }

    fn current_out(&self) -> mpsc::Sender<String> {
        self.out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Ticks once per successful redial. Consumers re-run their handshake
    /// (`EngineInfo`, capability discovery) when it changes.
    pub fn reconnected(&self) -> watch::Receiver<u64> {
        self.reconnects.subscribe()
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
            out: self.current_out(),
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
            out: self.current_out(),
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
        self.current_out()
            .send(json)
            .await
            .map_err(|_| RpcError::Closed)
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Connection closed: fail everything still pending.
fn fail_pending(shared: &Arc<Shared>) {
    let drained: Vec<Pending> = {
        let mut pending = shared.lock();
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
    error
        .strip_prefix("unknown method: ")
        .map(|method| RpcError::UnknownMethod(method.to_owned()))
        .unwrap_or(RpcError::Failed(error))
}

/// How long a dial may take before we give up.
///
/// This is localhost: a real engine answers in milliseconds. Without a bound,
/// *any* other process holding the port accepts the TCP connection and then
/// never completes the WebSocket handshake, and the caller waits forever — a
/// stranger on port 27654 would hang the app at boot rather than degrade it.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Dial a WebSocket RPC server (`ws://127.0.0.1:{ipc_port}`). The client dies
/// with its connection; see [`connect_ws_redialing`] for one that outlives it.
pub async fn connect_ws(url: &str) -> Result<RpcClient, RpcError> {
    let (out, inbound) = dial_ws(url).await?;
    Ok(RpcClient::new(out, inbound))
}

/// Like [`connect_ws`], but redials with backoff when the connection drops
/// (for example an engine restart), so one client outlives many connections.
/// The first dial must succeed; watch [`RpcClient::reconnected`] for redials.
pub async fn connect_ws_redialing(url: &str) -> Result<RpcClient, RpcError> {
    let (out, inbound) = dial_ws(url).await?;
    let url = url.to_owned();
    let dial: Dial = Arc::new(move || {
        let url = url.clone();
        Box::pin(async move { dial_ws(&url).await })
    });
    Ok(RpcClient::with_transport(out, inbound, Some(dial)))
}

/// One WebSocket dial: the handshake plus the pump between socket and channels.
async fn dial_ws(url: &str) -> Result<Transport, RpcError> {
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| RpcError::Transport(format!("timed out dialing {url}")))?
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                frame = out_rx.recv() => match frame {
                    Some(text) => {
                        if sink.send(WsMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                },
                message = stream.next() => match message {
                    Some(Ok(WsMessage::Text(text))) => {
                        if in_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
            }
        }
    });
    Ok((out_tx, in_rx))
}

#[cfg(test)]
mod redial_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A server that accepts a connection and closes it at once must not be
    /// redialed in a hot loop: connections that die young back off.
    #[tokio::test]
    async fn a_server_that_accepts_and_immediately_closes_is_not_redialed_in_a_hot_loop() {
        let dials = Arc::new(AtomicUsize::new(0));
        let dial: Dial = {
            let dials = dials.clone();
            Arc::new(move || {
                dials.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let (out, _out_rx) = mpsc::channel::<String>(8);
                    let (_in_tx, inbound) = mpsc::channel::<String>(8);
                    // `_in_tx` drops here: the connection is closed before it starts.
                    Ok((out, inbound))
                })
            })
        };
        let (out, _rx) = mpsc::channel::<String>(8);
        let (_tx, inbound) = mpsc::channel::<String>(8);
        let client = RpcClient::with_transport(out, inbound, Some(dial));
        drop(_tx);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let count = dials.load(Ordering::SeqCst);
        // 100 + 250 + 500 ms of backoff fit in the window: a handful of dials,
        // not the thousands a hot loop would make.
        assert!((1..=8).contains(&count), "{count} dials in 1.5 s");
        drop(client);
    }
}
