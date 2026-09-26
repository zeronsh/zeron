//! Live mode: the edge transports behind [`crate::Client`].
//!
//! - registry: `RegistryClient` over the shared [`zeron_doc::RegistryDoc`]
//!   (WS + HTTPS pull/push), persisted in the docs store, presence beats,
//!   sidebar-pin reconciliation once synced;
//! - rooms ([`room`]): one chat2 `ChatClient` per open session;
//! - relay ([`relay`]): host RPCs over the device room (`LinkCache`),
//!   chunked uploads / attachment reads, PR-status watches;
//! - escorts ([`escort`]): `pending://` attachment bytes pushed to the host.
//!
//! Every loop here retries forever with bounded backoff that event wakes
//! (system resume, any successful dial, OS path restored, token change) cut
//! short — the legacy client's one-shot chat join left chats dark until a
//! relaunch.

pub(crate) mod escort;
pub(crate) mod relay;
pub(crate) mod room;
pub(crate) mod urls;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zeron_doc::{REGISTRY_DOC_ID, RegistryDoc};
use zeron_sync::{
    DocsStore, RegistryClient, RegistryEvent, RegistryTransport, RegistryTuning, SyncError,
    UrlProvider,
};

use crate::client::ClientInner;
use crate::error::{ClientError, Result};
use crate::{lock, now_ms};

/// First-join retry curve (engine workspace_host JOIN_RETRY_*).
pub(crate) const JOIN_RETRY_BASE: Duration = Duration::from_millis(500);
pub(crate) const JOIN_RETRY_CAP: Duration = Duration::from_secs(16);
/// This device's presence beat cadence.
const PRESENCE_INTERVAL: Duration = Duration::from_secs(15);
/// Registry snapshot debounce.
const REGISTRY_SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// While the OS says there is no path, backoffs park at least this long
/// (any online event still cuts them short).
const OFFLINE_PARK: Duration = Duration::from_secs(30);

/// Sleep `wait` (jittered), cut short by wake/online events. `false` when
/// cancelled.
pub(crate) async fn wait_backoff(cancel: &CancellationToken, wait: Duration) -> bool {
    let mut wake = zeron_sync::wake::subscribe();
    let mut online = zeron_sync::wake::subscribe_online();
    while online.try_recv().is_ok() {}
    let jitter = Duration::from_millis((now_ms().unsigned_abs() % 250) + 1);
    let wait = if zeron_sync::wake::path_is_offline() {
        wait.max(OFFLINE_PARK)
    } else {
        wait + jitter
    };
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(wait) => true,
        _ = wake.recv() => true,
        _ = online.recv() => true,
    }
}

/// Fresh bearer for every dial/request, from the client's token provider.
#[derive(Clone)]
pub(crate) struct Bearer(pub(crate) Weak<ClientInner>);

impl Bearer {
    pub(crate) async fn get(&self) -> std::result::Result<String, SyncError> {
        let Some(inner) = self.0.upgrade() else {
            return Err(SyncError::Closed);
        };
        match inner.tokens.bearer().await {
            Ok(token) => Ok(token),
            Err(ClientError::Auth(reason)) => Err(SyncError::Auth(reason)),
            Err(err) => Err(SyncError::TemporarilyUnavailable(err.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl zeron_rpc::TokenSource for Bearer {
    async fn token(&self) -> std::result::Result<String, zeron_rpc::TokenError> {
        match self.get().await {
            Ok(token) => Ok(token),
            Err(SyncError::Auth(_) | SyncError::Closed) => Err(zeron_rpc::TokenError::SignedOut),
            Err(err) => Err(zeron_rpc::TokenError::TemporarilyUnavailable(
                err.to_string(),
            )),
        }
    }
}

struct RegistryUrl {
    bearer: Bearer,
    edge: String,
    org_id: String,
    device_id: String,
}

impl UrlProvider for RegistryUrl {
    fn url(&self) -> BoxFuture<'static, std::result::Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let (edge, org, device) = (
            self.edge.clone(),
            self.org_id.clone(),
            self.device_id.clone(),
        );
        Box::pin(async move {
            let token = bearer.get().await?;
            Ok(urls::registry_ws(&edge, &org, &token, &device))
        })
    }
}

/// HTTPS pull (doubles as a presence beat) / push.
struct RegistryHttp {
    bearer: Bearer,
    edge: String,
    org_id: String,
    device_id: String,
}

fn http_err(err: reqwest::Error) -> SyncError {
    SyncError::WebSocket(err.to_string())
}

impl RegistryTransport for RegistryHttp {
    fn fetch(&self, since: u64) -> BoxFuture<'static, std::result::Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::registry_rows(&self.edge, &self.org_id, &self.device_id, since);
        Box::pin(async move {
            let token = bearer.get().await?;
            let response = crate::auth::http()
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(http_err)?;
            if !response.status().is_success() {
                return Err(SyncError::Protocol(format!(
                    "registry pull http {}",
                    response.status()
                )));
            }
            response.text().await.map_err(http_err)
        })
    }

    fn push(&self, body: String) -> BoxFuture<'static, std::result::Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::registry_push(&self.edge, &self.org_id, &self.device_id);
        Box::pin(async move {
            let token = bearer.get().await?;
            let response = crate::auth::http()
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(http_err)?;
            if !response.status().is_success() {
                return Err(SyncError::Protocol(format!(
                    "registry push http {}",
                    response.status()
                )));
            }
            response.text().await.map_err(http_err)
        })
    }
}

/// Raw registry posture sampled by the connectivity recompute.
#[derive(Debug, Clone, Default)]
pub(crate) struct RegistryPosture {
    pub connected: bool,
    pub synced: bool,
    pub retry_at_ms: Option<i64>,
    pub last_failure: Option<String>,
}

pub(crate) struct LiveBackend {
    pub(crate) edge: String,
    pub(crate) store: Arc<DocsStore>,
    registry: Mutex<Option<RegistryClient>>,
    /// First-join retry deadline while the registry has never joined.
    registry_retry_at: AtomicI64,
    registry_failure: Mutex<Option<String>>,
    registry_save: Arc<Notify>,
    registry_dirty: Arc<AtomicBool>,
    last_liveness_probe: AtomicI64,
    pub(crate) relay: relay::Relay,
    pub(crate) escorts: escort::Escorts,
    cancel: CancellationToken,
}

impl LiveBackend {
    /// Open the docs store and restore the registry replica (instant, local).
    pub(crate) fn open(
        data_dir: &std::path::Path,
        device_id: &str,
    ) -> Result<(DocsStore, RegistryDoc)> {
        std::fs::create_dir_all(data_dir).map_err(|e| ClientError::Storage(e.to_string()))?;
        let store = DocsStore::open(data_dir).map_err(|e| ClientError::Storage(e.to_string()))?;
        let registry = match store.load_snapshot(REGISTRY_DOC_ID) {
            Ok(Some(bytes)) => RegistryDoc::from_bytes(&bytes, device_id).unwrap_or_else(|err| {
                tracing::warn!(error = %err, "registry snapshot unreadable; starting fresh");
                RegistryDoc::new(device_id.to_owned())
            }),
            _ => RegistryDoc::new(device_id.to_owned()),
        };
        Ok((store, registry))
    }

    pub(crate) fn new(inner: &Arc<ClientInner>, store: DocsStore) -> Self {
        let bearer = Bearer(Arc::downgrade(inner));
        let edge = inner.config.edge_base().to_owned();
        let store = Arc::new(store);
        Self {
            relay: relay::Relay::new(inner, &edge, bearer),
            escorts: escort::Escorts::new(&inner.config.data_dir),
            edge,
            store,
            registry: Mutex::new(None),
            registry_retry_at: AtomicI64::new(0),
            registry_failure: Mutex::new(None),
            registry_save: Arc::new(Notify::new()),
            registry_dirty: Arc::new(AtomicBool::new(false)),
            last_liveness_probe: AtomicI64::new(0),
            cancel: inner.cancel.child_token(),
        }
    }

    pub(crate) fn bearer(inner: &Arc<ClientInner>) -> Bearer {
        Bearer(Arc::downgrade(inner))
    }

    /// Spawn the registry join/event loop, the presence beat, and the
    /// registry saver.
    pub(crate) fn start(&self, inner: &Arc<ClientInner>) {
        self.spawn_registry(inner);
        self.spawn_registry_saver(inner);
        self.escorts.respawn(inner);
    }

    pub(crate) fn stop(&self) {
        self.cancel.cancel();
        if let Some(client) = lock(&self.registry).take() {
            crate::runtime::shared().spawn(client.shutdown());
        }
        self.relay.shutdown();
        self.flush_registry_now_with(None);
    }

    pub(crate) fn registry_posture(&self) -> RegistryPosture {
        match lock(&self.registry).as_ref() {
            Some(client) => {
                let stats = client.stats();
                let reconnect = client.reconnect_state();
                RegistryPosture {
                    connected: stats.connected,
                    synced: stats.synced,
                    retry_at_ms: (reconnect.retry_at_ms > 0).then_some(reconnect.retry_at_ms),
                    last_failure: reconnect.last_failure,
                }
            }
            None => {
                let at = self.registry_retry_at.load(Ordering::Acquire);
                RegistryPosture {
                    connected: false,
                    synced: false,
                    retry_at_ms: (at > 0).then_some(at),
                    last_failure: lock(&self.registry_failure).clone(),
                }
            }
        }
    }

    /// Remote devices' live presence (within the registry's TTL).
    pub(crate) fn presence(&self) -> Option<HashMap<String, i64>> {
        lock(&self.registry).as_ref().map(RegistryClient::presence)
    }

    /// Local registry write landed: push now, persist soon.
    pub(crate) fn registry_written(&self) {
        if let Some(client) = lock(&self.registry).as_ref() {
            client.nudge();
        }
        self.mark_registry_dirty();
    }

    pub(crate) fn mark_registry_dirty(&self) {
        self.registry_dirty.store(true, Ordering::Release);
        self.registry_save.notify_one();
    }

    /// Cheap edge reachability check (`GET /health`, 3s): a 200 un-parks
    /// every backoff in the process at once (legacy `probeEdgeHealth`).
    pub(crate) fn probe_health(&self) {
        let url = urls::health(&self.edge);
        crate::runtime::shared().spawn(async move {
            let ok = crate::auth::http()
                .get(url)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if ok {
                zeron_sync::wake::notify_online();
            }
        });
    }

    /// Deaf-socket guard (called from the 1 Hz tick): a connected registry
    /// that hasn't PUSHED anything for a minute — while other devices beat
    /// every 15s — gets a deadline-checked probe (≤ one per minute).
    pub(crate) fn check_registry_liveness(&self, peers_known: bool) {
        if !peers_known {
            return;
        }
        let guard = lock(&self.registry);
        let Some(client) = guard.as_ref() else { return };
        let stats = client.stats();
        let now = now_ms();
        let last_probe = self.last_liveness_probe.load(Ordering::Acquire);
        if stats.connected
            && stats.last_pushed_ms > 0
            && now - stats.last_pushed_ms > 60_000
            && now - last_probe > 60_000
        {
            self.last_liveness_probe.store(now, Ordering::Release);
            client.probe();
        }
    }

    /// Foreground / network restored: probe the registry now.
    pub(crate) fn kick(&self) {
        if let Some(client) = lock(&self.registry).as_ref() {
            if client.stats().connected {
                client.probe();
            } else {
                client.redial();
            }
        }
        zeron_sync::wake::notify_online();
    }

    fn flush_registry_now_with(&self, inner: Option<&ClientInner>) {
        if !self.registry_dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let Some(inner) = inner else { return };
        let bytes = inner.workspace.mutate(|doc| doc.to_bytes());
        match bytes {
            Ok(bytes) => {
                if let Err(err) = self.store.save_snapshot(REGISTRY_DOC_ID, &bytes) {
                    tracing::warn!(error = %err, "registry snapshot save failed");
                }
            }
            Err(err) => tracing::warn!(error = %err, "registry export failed"),
        }
    }

    /// Persist the registry now (backgrounding).
    pub(crate) fn flush_registry(&self, inner: &ClientInner) {
        self.flush_registry_now_with(Some(inner));
    }

    fn spawn_registry_saver(&self, inner: &Arc<ClientInner>) {
        let weak = Arc::downgrade(inner);
        let cancel = self.cancel.clone();
        let wake = self.registry_save.clone();
        let dirty = self.registry_dirty.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                // Register before checking so a notify can't slip by.
                let wait = wake.notified();
                if !dirty.load(Ordering::Acquire) {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = wait => {}
                    }
                }
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = tokio::time::sleep(REGISTRY_SAVE_DEBOUNCE) => {}
                }
                let Some(inner) = weak.upgrade() else { return };
                let weak_inner = Arc::downgrade(&inner);
                drop(inner);
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(inner) = weak_inner.upgrade()
                        && let Some(live) = inner.live()
                    {
                        live.flush_registry(&inner);
                    }
                })
                .await;
                if cancel.is_cancelled() {
                    return;
                }
            }
        });
    }

    fn spawn_registry(&self, inner: &Arc<ClientInner>) {
        let weak = Arc::downgrade(inner);
        let cancel = self.cancel.clone();
        let provider: Arc<dyn UrlProvider> = Arc::new(RegistryUrl {
            bearer: Self::bearer(inner),
            edge: self.edge.clone(),
            org_id: inner.credentials.org_id().to_owned(),
            device_id: inner.config.device_id.clone(),
        });
        let transport: Arc<dyn RegistryTransport> = Arc::new(RegistryHttp {
            bearer: Self::bearer(inner),
            edge: self.edge.clone(),
            org_id: inner.credentials.org_id().to_owned(),
            device_id: inner.config.device_id.clone(),
        });
        let doc = inner.workspace.doc().clone();
        let device_id = inner.config.device_id.clone();
        crate::runtime::shared().spawn(async move {
            let mut backoff = JOIN_RETRY_BASE;
            let client = loop {
                let attempt = RegistryClient::connect_via_transport(
                    provider.clone(),
                    doc.clone(),
                    &device_id,
                    RegistryTuning::default(),
                    transport.clone(),
                );
                let result = tokio::select! {
                    _ = cancel.cancelled() => return,
                    result = tokio::time::timeout(Duration::from_secs(60), attempt) => result,
                };
                let failure = match result {
                    Ok(Ok(client)) => break client,
                    Ok(Err(err)) => err.to_string(),
                    Err(_) => "registry join timed out".to_owned(),
                };
                let Some(inner) = weak.upgrade() else { return };
                if let Some(live) = inner.live() {
                    *lock(&live.registry_failure) = Some(failure);
                    live.registry_retry_at
                        .store(now_ms() + backoff.as_millis() as i64, Ordering::Release);
                }
                drop(inner);
                if !wait_backoff(&cancel, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(JOIN_RETRY_CAP);
            };
            client.set_presence(now_ms());
            let mut events = client.events();
            {
                let Some(inner) = weak.upgrade() else { return };
                let Some(live) = inner.live() else { return };
                live.registry_retry_at.store(0, Ordering::Release);
                *lock(&live.registry_failure) = None;
                *lock(&live.registry) = Some(client);
                drop(inner);
            }
            if let Some(inner) = weak.upgrade() {
                inner.registry_changed();
            }
            let mut beat = tokio::time::interval(PRESENCE_INTERVAL);
            beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            beat.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = beat.tick() => {
                        let Some(inner) = weak.upgrade() else { return };
                        if let Some(live) = inner.live()
                            && let Some(client) = lock(&live.registry).as_ref()
                        {
                            client.set_presence(now_ms());
                        }
                        inner.presence_changed();
                    }
                    event = events.recv() => {
                        let Some(inner) = weak.upgrade() else { return };
                        match event {
                            Ok(RegistryEvent::Applied | RegistryEvent::Connected) => inner.registry_changed(),
                            Ok(RegistryEvent::Presence) => inner.presence_changed(),
                            Ok(RegistryEvent::Disconnected) => inner.connectivity_changed(),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => inner.registry_changed(),
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            }
        });
    }
}

/// Minimal standard base64 (chunked uploads / attachment reads).
pub(crate) mod b64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(crate) fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    pub(crate) fn decode(input: &str) -> Option<Vec<u8>> {
        fn value(c: u8) -> Option<u32> {
            Some(match c {
                b'A'..=b'Z' => (c - b'A') as u32,
                b'a'..=b'z' => (c - b'a' + 26) as u32,
                b'0'..=b'9' => (c - b'0' + 52) as u32,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                _ => return None,
            })
        }
        let mut out = Vec::with_capacity(input.len() / 4 * 3);
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for &c in input.as_bytes() {
            if c == b'=' {
                break;
            }
            if c.is_ascii_whitespace() {
                continue;
            }
            buffer = (buffer << 6) | value(c)?;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
                buffer &= (1 << bits) - 1;
            }
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn round_trips() {
            for len in 0..20 {
                let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
                assert_eq!(super::decode(&super::encode(&bytes)).unwrap(), bytes);
            }
            assert_eq!(super::encode(b"hello"), "aGVsbG8=");
        }
    }
}
