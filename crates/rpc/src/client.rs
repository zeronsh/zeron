//! Client side: request/stream multiplexing over string frames + the WebSocket dialer.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{ClientFrame, RpcError, ServerFrame};

/// Negotiated credits prevent a stalled stream from blocking the connection reader.
pub(crate) const STREAM_QUEUE_CAP: usize = 8;
const CONTROL_QUEUE_CAP: usize = 1024;

type StreamError = Arc<Mutex<Option<RpcError>>>;
struct StreamSender {
    tx: mpsc::Sender<serde_json::Value>,
    error: StreamError,
    credits: Arc<AtomicBool>,
}

enum Pending {
    Call(oneshot::Sender<Result<serde_json::Value, RpcError>>),
    Stream(StreamSender),
    CheckedStream {
        items: StreamSender,
        ready: Option<oneshot::Sender<Result<(), RpcError>>>,
    },
}

impl Pending {
    fn fail(self, error: RpcError) {
        match self {
            Self::Call(tx) => {
                let _ = tx.send(Err(error));
            }
            Self::Stream(items) => {
                *items.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
            }
            Self::CheckedStream { items, ready } => {
                *items.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.clone());
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error));
                }
            }
        }
    }
}

#[derive(Default)]
struct Controls {
    queued: HashMap<u64, ClientFrame>,
    order: VecDeque<u64>,
}

struct Shared {
    pending: Mutex<HashMap<u64, Pending>>,
    controls: Mutex<Controls>,
    control_ready: Notify,
    shutdown: CancellationToken,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn control(&self, id: u64, cancel: bool) {
        let mut controls = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        if self.shutdown.is_cancelled() {
            return;
        }
        if !controls.queued.contains_key(&id) {
            if controls.queued.len() == CONTROL_QUEUE_CAP {
                // A stalled transport must not create unbounded cancellation
                // tasks or control metadata. Closing fails all calls explicitly.
                self.shutdown.cancel();
                return;
            }
            controls.order.push_back(id);
        }
        let frame = controls.queued.entry(id).or_insert_with(|| ClientFrame {
            id,
            ..Default::default()
        });
        if cancel {
            frame.cancel = true;
            frame.stream_credit = 0;
        } else if !frame.cancel {
            frame.stream_credit = frame
                .stream_credit
                .saturating_add(1)
                .min(STREAM_QUEUE_CAP as u32);
        }
        drop(controls);
        self.control_ready.notify_one();
    }

    fn fail_all(&self) {
        let pending: Vec<_> = self.lock().drain().map(|(_, p)| p).collect();
        for entry in pending {
            entry.fail(RpcError::Closed);
        }
        let mut controls = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        controls.queued.clear();
        controls.order.clear();
    }
}

async fn write_controls(shared: Arc<Shared>, out: mpsc::Sender<String>) {
    loop {
        let notified = shared.control_ready.notified();
        let frame = {
            let mut controls = shared
                .controls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            controls
                .order
                .pop_front()
                .and_then(|id| controls.queued.remove(&id))
        };
        if let Some(frame) = frame {
            let json = serde_json::to_string(&frame).expect("RPC control frame serializes");
            tokio::select! {
                biased;
                _ = shared.shutdown.cancelled() => return,
                sent = out.send(json) => if sent.is_err() { shared.shutdown.cancel(); return; }
            }
        } else {
            tokio::select! {
                _ = shared.shutdown.cancelled() => return,
                _ = out.closed() => { shared.shutdown.cancel(); return; }
                _ = notified => {}
            }
        }
    }
}

/// A multiplexing RPC client over any string-frame duplex ([`crate::memory_client`] or
/// [`connect_ws`]). Cheap to clone-by-Arc internally; use one per connection.
pub struct RpcClient {
    out: mpsc::Sender<String>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
    control_writer: tokio::task::JoinHandle<()>,
}

/// Owned stream receiver whose drop schedules cancellation of its server task.
pub struct RpcSubscription {
    id: u64,
    items: mpsc::Receiver<serde_json::Value>,
    shared: Arc<Shared>,
    error: StreamError,
    credits: Arc<AtomicBool>,
}

// Own cleanup before the first await, including a cancelled/backpressured send.
struct RequestGuard {
    id: u64,
    shared: Arc<Shared>,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        cancel_pending(self.id, &self.shared);
    }
}

impl RpcSubscription {
    pub async fn recv(&mut self) -> Option<serde_json::Value> {
        match self.recv_result().await {
            Ok(item) => item,
            Err(error) => {
                tracing::warn!(id = self.id, %error, "RPC subscription failed");
                None
            }
        }
    }

    /// Cancellation-safe receive with explicit legacy-peer overflow/errors.
    pub async fn recv_result(&mut self) -> Result<Option<serde_json::Value>, RpcError> {
        match self.items.recv().await {
            Some(item) => {
                if self.credits.load(Ordering::Acquire) && self.shared.lock().contains_key(&self.id)
                {
                    self.shared.control(self.id, false);
                }
                Ok(Some(item))
            }
            None => match self.error() {
                Some(error) => Err(error),
                None => Ok(None),
            },
        }
    }

    pub fn error(&self) -> Option<RpcError> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for RpcSubscription {
    fn drop(&mut self) {
        cancel_pending(self.id, &self.shared);
    }
}

fn cancel_pending(id: u64, shared: &Arc<Shared>) {
    if shared.lock().remove(&id).is_some() {
        shared.control(id, true);
    }
}

impl RpcClient {
    /// Wrap an existing duplex: `out` carries client frames, `inbound` server frames.
    pub fn new(out: mpsc::Sender<String>, mut inbound: mpsc::Receiver<String>) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            controls: Mutex::new(Controls::default()),
            control_ready: Notify::new(),
            shutdown: CancellationToken::new(),
        });
        let reader_shared = shared.clone();
        let control_writer = tokio::spawn(write_controls(shared.clone(), out.clone()));
        let reader = tokio::spawn(async move {
            loop {
                let payload = tokio::select! {
                    biased;
                    _ = reader_shared.shutdown.cancelled() => break,
                    payload = inbound.recv() => match payload { Some(payload) => payload, None => break }
                };
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
                    route_frame(&reader_shared, frame);
                }
            }
            reader_shared.shutdown.cancel();
            reader_shared.fail_all();
        });
        Self {
            out,
            shared,
            next_id: AtomicU64::new(1),
            reader,
            control_writer,
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
        let _guard = RequestGuard {
            id,
            shared: self.shared.clone(),
        };
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
            ..Default::default()
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

    /// Compatibility receiver. One bounded forwarder owns the scoped lease;
    /// even a quiet receiver drop now cancels its server-side subscription.
    pub async fn subscribe(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<mpsc::Receiver<serde_json::Value>, RpcError> {
        let mut subscription = self.subscribe_scoped(method, params).await?;
        let (tx, rx) = mpsc::channel(STREAM_QUEUE_CAP);
        tokio::spawn(async move {
            loop {
                let Ok(permit) = tx.reserve().await else {
                    break;
                };
                tokio::select! {
                    biased;
                    _ = tx.closed() => break,
                    item = subscription.recv_result() => match item {
                        Ok(Some(item)) => { permit.send(item); }
                        Ok(None) => break,
                        Err(error) => { tracing::warn!(%error, "legacy RPC subscription failed"); break; }
                    }
                }
            }
        });
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
        let error = Arc::new(Mutex::new(None));
        let credits = Arc::new(AtomicBool::new(false));
        self.shared.lock().insert(
            id,
            Pending::Stream(StreamSender {
                tx,
                error: error.clone(),
                credits: credits.clone(),
            }),
        );
        // Own cancellation before the send, including cancellation while the
        // outbound channel is backpressured.
        let subscription = RpcSubscription {
            id,
            items: rx,
            shared: self.shared.clone(),
            error,
            credits,
        };
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
            stream_window: Some(STREAM_QUEUE_CAP as u32),
            ..Default::default()
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
        let error = Arc::new(Mutex::new(None));
        let credits = Arc::new(AtomicBool::new(false));
        self.shared.lock().insert(
            id,
            Pending::CheckedStream {
                items: StreamSender {
                    tx: items_tx,
                    error: error.clone(),
                    credits: credits.clone(),
                },
                ready: Some(ready_tx),
            },
        );
        let subscription = RpcSubscription {
            id,
            items: items_rx,
            shared: self.shared.clone(),
            error,
            credits,
        };
        self.send(ClientFrame {
            id,
            method: Some(method.into()),
            params,
            cancel: false,
            stream_window: Some(STREAM_QUEUE_CAP as u32),
            ..Default::default()
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
        tokio::select! {
            biased;
            _ = self.shared.shutdown.cancelled() => Err(RpcError::Closed),
            sent = self.out.send(json) => sent.map_err(|_| RpcError::Closed)
        }
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.shared.shutdown.cancel();
        self.reader.abort();
        self.control_writer.abort();
        self.shared.fail_all();
    }
}

fn route_frame(shared: &Arc<Shared>, frame: ServerFrame) {
    let id = frame.id;
    if let Some(window) = frame.stream_window {
        let pending = shared.lock();
        let items = match pending.get(&id) {
            Some(Pending::Stream(items)) | Some(Pending::CheckedStream { items, .. }) => {
                Some(items)
            }
            _ => None,
        };
        if let Some(items) = items {
            if window == 0 || window as usize > STREAM_QUEUE_CAP {
                drop(pending);
                if let Some(pending) = shared.lock().remove(&id) {
                    pending.fail(RpcError::Transport(
                        "invalid RPC stream credit window".into(),
                    ));
                }
                shared.control(id, true);
            } else {
                items.credits.store(true, Ordering::Release);
            }
        }
        return;
    }
    if let Some(err) = frame.err {
        let error = wire_error(err);
        if let Some(pending) = shared.lock().remove(&id) {
            pending.fail(error);
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
        // Never await one stream from the shared connection reader.
        let tx = match shared.lock().get(&id) {
            Some(Pending::Stream(items)) => Some(items.tx.clone()),
            Some(Pending::CheckedStream { items, .. }) => Some(items.tx.clone()),
            _ => None,
        };
        if let Some(tx) = tx
            && let Err(error) = tx.try_send(item)
        {
            let error = match error {
                    mpsc::error::TrySendError::Full(_) => RpcError::Failed("RPC stream overflow: peer did not honor per-stream flow control; resubscribe from a durable cursor".into()),
                    mpsc::error::TrySendError::Closed(_) => RpcError::Closed,
                };
            if let Some(pending) = shared.lock().remove(&id) {
                pending.fail(error);
            }
            shared.control(id, true);
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

/// Dial a WebSocket RPC server (`ws://127.0.0.1:{ipc_port}`).
pub async fn connect_ws(url: &str) -> Result<RpcClient, RpcError> {
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| RpcError::Transport(format!("timed out dialing {url}")))?
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    let (out_tx, out_rx) = mpsc::channel::<String>(crate::FRAME_QUEUE_CAP);
    let (in_tx, in_rx) = mpsc::channel::<String>(crate::FRAME_QUEUE_CAP);
    tokio::spawn(crate::server::pump_socket(ws, out_rx, in_tx));
    Ok(RpcClient::new(out_tx, in_rx))
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn controls_coalesce_and_stalled_overflow_closes_instead_of_spawning_tasks() {
        let shared = Shared {
            pending: Mutex::new(HashMap::new()),
            controls: Mutex::new(Controls::default()),
            control_ready: Notify::new(),
            shutdown: CancellationToken::new(),
        };
        for _ in 0..STREAM_QUEUE_CAP {
            shared.control(1, false);
        }
        shared.control(1, true);
        let controls = shared.controls.lock().unwrap();
        assert_eq!(controls.order.len(), 1);
        assert!(controls.queued[&1].cancel);
        assert_eq!(controls.queued[&1].stream_credit, 0);
        drop(controls);
        for id in 2..=CONTROL_QUEUE_CAP as u64 {
            shared.control(id, true);
        }
        assert!(!shared.shutdown.is_cancelled());
        shared.control(CONTROL_QUEUE_CAP as u64 + 1, true);
        assert!(shared.shutdown.is_cancelled());
        assert_eq!(
            shared.controls.lock().unwrap().queued.len(),
            CONTROL_QUEUE_CAP
        );
    }
}
