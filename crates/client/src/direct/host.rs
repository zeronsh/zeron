//! [`DirectHost`] — the Direct-mode backend, modelled on `DemoHost`: the
//! engine's IPC watch streams are mirrored into the local registry replica
//! (settled in-process, never pushed anywhere) and into per-chat shadow
//! session docs; commands the client writes into a doc are forwarded with
//! `RelayCommand` (keeping the phone-minted ids) and marked applied locally.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use zeron_doc::transcript_delta::apply_transcript_frame;
use zeron_doc::{
    RegistryDoc, SegmentWriter, SessionCommandStatus, SessionDoc, SessionMessageEntry,
    TranscriptFrame, TranscriptUpdate,
};
use zeron_proto::{Chat, Device, Session, Space};

use super::lenient::{
    Substitution, decode_rows, parse_version, sanitize_transcript_update, short_id,
};
use super::ssh::{self, SshSession};
use super::{
    DirectLogLine, DirectPhase, DirectStatus, EndpointStat, SshEndpoint, SshError, SshTarget,
    StreamStat,
};
use crate::client::ClientInner;
use crate::demo::DemoServer;
use crate::error::{ClientError, Result};
use crate::session::SessionCore;
use crate::{lock, now_ms};
use futures::StreamExt;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long transcripts wait for the catalog prefetch after a link comes up.
const CATALOG_HEAD_START: Duration = Duration::from_secs(3);
/// Presence beat while the link is up (well inside PRESENCE_FRESH_MS).
const BEAT: Duration = Duration::from_secs(5);
/// Probe a feed that has been quiet this long (see `probe_feed`); well
/// inside `workspace::view::STATUS_HOLD_MS`.
const PROBE_AFTER: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// Host RPC budget (generous: mobile networks, big replies).
const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a call waits for a (re)connecting link.
const LINK_WAIT: Duration = Duration::from_secs(20);
/// How often the desktop's sidebar pins are read again while linked (see
/// [`super::desktop_pins`]).
const DESKTOP_PINS_EVERY: Duration = Duration::from_secs(60);
/// Phone-created rows survive a mirror pass this long before the engine's
/// echo must include them.
const LOCAL_ROW_GRACE_MS: i64 = 20_000;
const REGISTRY_FILE: &str = "direct-registry.bin";
/// Every registry stream must deliver its first frame this soon after the
/// tunnel opens; otherwise the link is reported and recycled (a silent,
/// half-working tunnel used to leave an empty sessions page).
#[cfg(not(test))]
const SYNC_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const SYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// Tests: `host` → plain `ws://` URL of an in-process engine (no SSH).
#[cfg(test)]
pub(crate) static TEST_ENGINES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// Tests: `host` → how often the CLI/model lists are re-read while the
/// link is up (instead of [`crate::client::CATALOG_REFRESH`]).
#[cfg(test)]
pub(crate) static TEST_CATALOG_REFRESH: Mutex<Option<HashMap<String, Duration>>> = Mutex::new(None);
/// Link log depth kept for the diagnostics readout.
const LOG_LINES: usize = 40;
/// Engine version this build was checked against. Newer patch releases are
/// expected and silent; a newer minor/major gets a non-blocking notice.
pub(crate) const TESTED_ENGINE: (u32, u32, u32) = (0, 2, 98);
const REGISTRY_STREAMS: [&str; 4] = [
    zeron_rpc::methods::WATCH_DEVICES,
    zeron_rpc::methods::WATCH_SPACES,
    zeron_rpc::methods::WATCH_CHATS,
    zeron_rpc::methods::WATCH_SESSIONS,
];

struct Link {
    /// `None` only for the in-process test engine (plain WebSocket).
    ssh: Option<SshSession>,
    /// The registry feeds (devices, chats, sessions — the running status
    /// heartbeats ride here).
    rpc: zeron_rpc::RpcClient,
    /// One-off requests (ListModels, Mutate, folder listings…) on their own
    /// tunnel channel and WebSocket. On the feed connection a reply waits
    /// behind every byte queued before it, and a working chat's transcript
    /// can be megabytes (a 7.6 MB first snapshot was measured), so over a
    /// slow relay ListHarnesses/ListModels timed out though the engine
    /// answered in well under a second. `None`: share `rpc`.
    calls: Option<zeron_rpc::RpcClient>,
    /// A spare tunnel channel/WebSocket for the next transcript opened.
    /// Every on-screen transcript streams on its own channel, closed when
    /// it leaves the screen: the engine writes each connection's frames in
    /// order (and queues up to 256 of them), so on a shared channel a chat
    /// opened next waited behind the rest of the previous chat's multi-MB
    /// reset and queued deltas — a cancel can't recall frames already
    /// queued, but closing the channel makes sshd drop them. The spare is
    /// opened ahead so an open costs no extra round trips. `None` with no
    /// SSH (test engine) or when the machine won't open more channels:
    /// transcripts share `rpc`.
    transcript_spare: Mutex<Option<TranscriptChannel>>,
    /// Per-chat transcript channels are possible (SSH, and the machine
    /// opened the extra channels at connect).
    transcript_channels: bool,
    engine_port: u16,
    cancel: CancellationToken,
    engine_device_id: String,
    /// When the feed connection last delivered a registry frame or a probe
    /// reply (phone ms), and whether a probe is out.
    heard_ms: std::sync::atomic::AtomicI64,
    probing: std::sync::atomic::AtomicBool,
}

impl Link {
    fn heard(&self) {
        self.heard_ms
            .store(now_ms(), std::sync::atomic::Ordering::Release);
    }

    fn heard_ms(&self) -> i64 {
        self.heard_ms.load(std::sync::atomic::Ordering::Acquire)
    }

    fn calls(&self) -> &zeron_rpc::RpcClient {
        self.calls.as_ref().unwrap_or(&self.rpc)
    }

    /// A channel of its own for one transcript (the spare, refilled in the
    /// background), or `None`: stream on the feed connection.
    async fn transcript_channel(self: &Arc<Self>) -> Option<TranscriptChannel> {
        if !self.transcript_channels {
            return None;
        }
        let spare = lock(&self.transcript_spare).take();
        let channel = match spare {
            Some(channel) => Some(channel),
            None => self.open_transcript_channel().await,
        };
        // Refill the spare for the next chat.
        let link = self.clone();
        crate::runtime::shared().spawn(async move {
            if lock(&link.transcript_spare).is_some() || link.cancel.is_cancelled() {
                return;
            }
            if let Some(channel) = link.open_transcript_channel().await {
                let mut slot = lock(&link.transcript_spare);
                if slot.is_none() && !link.cancel.is_cancelled() {
                    *slot = Some(channel);
                }
            }
        });
        channel
    }

    async fn open_transcript_channel(&self) -> Option<TranscriptChannel> {
        let ssh = self.ssh.as_ref()?;
        let (rpc, received) = ssh.open_engine_counted(self.engine_port).await.ok()?;
        Some(TranscriptChannel { rpc, received })
    }
}

/// One transcript's own tunnel channel, and how many bytes have come in on
/// it (the complete history's download progress).
struct TranscriptChannel {
    rpc: zeron_rpc::RpcClient,
    received: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) struct DirectHost {
    /// Credentials and pinned key; the addresses live in `endpoints`.
    target: SshTarget,
    /// The machine's addresses in dial order (the app re-orders them when
    /// the network changes).
    endpoints: Mutex<Vec<SshEndpoint>>,
    client: Weak<ClientInner>,
    server: Mutex<DemoServer>,
    link: Mutex<Option<Arc<Link>>>,
    link_changed: tokio::sync::watch::Sender<u64>,
    status: Mutex<DirectStatus>,
    engine_device: Mutex<Option<String>>,
    /// Transcript mirrors: chat → (core it feeds, stop token).
    mirrors: Mutex<HashMap<String, (Weak<SessionCore>, CancellationToken)>>,
    /// Phone-side transcript snapshots the mirrors keep current; opening a
    /// chat hydrates from them (instant paint) while the engine's tail and
    /// reset settle the newest rows on top.
    shadow_store: Option<zeron_sync::DocsStore>,
    drains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    local_rows: Mutex<HashMap<String, i64>>,
    /// Session freshness re-stamped on the phone's clock.
    session_clock: Mutex<super::clock::SessionClock>,
    wake: tokio::sync::Notify,
    cancel: Mutex<Option<CancellationToken>>,
}

fn fresh_stat(endpoint: &SshEndpoint) -> EndpointStat {
    EndpointStat {
        host: endpoint.host.trim().to_owned(),
        port: endpoint.port,
        kind: endpoint.kind.clone(),
        ..EndpointStat::default()
    }
}

fn doc_err(err: zeron_doc::DocError) -> ClientError {
    ClientError::Internal(err.to_string())
}

impl DirectHost {
    pub(crate) fn new(client: &Arc<ClientInner>, target: SshTarget) -> Arc<Self> {
        let endpoints = target.addresses();
        let status = DirectStatus {
            endpoints: endpoints.iter().map(fresh_stat).collect(),
            ..DirectStatus::default()
        };
        let shadow_store = zeron_sync::DocsStore::open(client.config.data_dir.join("transcripts"))
            .map_err(|err| {
                tracing::warn!(error = %err, "transcript snapshot store unavailable")
            })
            .ok();
        Arc::new(Self {
            target,
            endpoints: Mutex::new(endpoints),
            client: Arc::downgrade(client),
            server: Mutex::new(DemoServer::default()),
            link: Mutex::new(None),
            link_changed: tokio::sync::watch::channel(0).0,
            status: Mutex::new(status),
            engine_device: Mutex::new(None),
            mirrors: Mutex::new(HashMap::new()),
            shadow_store,
            drains: Mutex::new(HashMap::new()),
            local_rows: Mutex::new(HashMap::new()),
            session_clock: Mutex::new(Default::default()),
            wake: tokio::sync::Notify::new(),
            cancel: Mutex::new(None),
        })
    }

    /// Cached replica from the last session (instant first paint; also
    /// carries the phone-only sidebar sections).
    pub(crate) fn load_registry(data_dir: &std::path::Path, device_id: &str) -> RegistryDoc {
        std::fs::read(data_dir.join(REGISTRY_FILE))
            .ok()
            .and_then(|bytes| RegistryDoc::from_bytes(&bytes, device_id).ok())
            .unwrap_or_else(|| RegistryDoc::new(device_id.to_owned()))
    }

    pub(crate) fn save_registry(&self) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let bytes = client.workspace.mutate(|doc| doc.to_bytes());
        match bytes {
            Ok(bytes) => {
                let dir = &client.config.data_dir;
                let _ = std::fs::create_dir_all(dir);
                let tmp = dir.join(format!("{REGISTRY_FILE}.tmp"));
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, dir.join(REGISTRY_FILE));
                }
            }
            Err(err) => tracing::debug!(error = %err, "direct registry snapshot failed"),
        }
    }

    pub(crate) fn status(&self) -> DirectStatus {
        let mut status = lock(&self.status).clone();
        status.clock_offset_ms = lock(&self.session_clock).offset_ms();
        status
    }

    /// New dial order (network changed, addresses edited). Keeps what each
    /// address did before; the link in use is not dropped (see [Self::reconnect]).
    pub(crate) fn set_endpoints(&self, endpoints: Vec<SshEndpoint>) {
        if endpoints.is_empty() {
            return;
        }
        let order = endpoints
            .iter()
            .map(SshEndpoint::describe)
            .collect::<Vec<_>>()
            .join(", ");
        let changed = {
            let mut current = lock(&self.endpoints);
            let changed = *current != endpoints;
            *current = endpoints.clone();
            changed
        };
        if !changed {
            return;
        }
        self.note(format!("addresses: {order}"));
        self.set_status(|s| {
            let old = std::mem::take(&mut s.endpoints);
            s.endpoints = endpoints
                .iter()
                .map(|e| {
                    let mut stat = old
                        .iter()
                        .find(|o| e.same_address(&o.host, o.port))
                        .cloned()
                        .unwrap_or_else(|| fresh_stat(e));
                    stat.kind = e.kind.clone();
                    stat
                })
                .collect();
        });
    }

    fn endpoint_stat(&self, endpoint: &SshEndpoint, f: impl FnOnce(&mut EndpointStat)) {
        self.set_status(|s| {
            if let Some(stat) = s
                .endpoints
                .iter_mut()
                .find(|st| endpoint.same_address(&st.host, st.port))
            {
                f(stat);
            }
        });
    }

    fn clear_active_endpoint(&self) {
        self.set_status(|s| s.endpoints.iter_mut().for_each(|e| e.active = false));
    }

    /// Drop the current link (even a stalled one) and dial again now.
    pub(crate) fn reconnect(&self) {
        if let Some(link) = self.current_link() {
            self.note("reconnect requested");
            link.cancel.cancel();
        }
        self.kick();
    }

    /// Append to the link log (also traced).
    pub(crate) fn note(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::info!(target: "zeron_client::direct", "{message}");
        self.push_log(message);
    }

    /// A failure the user should be able to find in 连接详情 › 日志 (Connection Details › Log; logged
    /// at warn, not just debug).
    pub(crate) fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(target: "zeron_client::direct", "{message}");
        self.push_log(message);
    }

    fn push_log(&self, message: String) {
        let mut status = lock(&self.status);
        status.log.push(DirectLogLine {
            at_ms: now_ms(),
            message,
        });
        let excess = status.log.len().saturating_sub(LOG_LINES);
        status.log.drain(..excess);
    }

    fn stream_stat(&self, method: &str, f: impl FnOnce(&mut StreamStat)) {
        let mut status = lock(&self.status);
        if let Some(stat) = status.streams.iter_mut().find(|s| s.name == method) {
            f(stat);
        }
    }

    /// Apply + ack local registry writes in-process (phone-only state such
    /// as sidebar sections lives here; engine rows arrive via the mirror).
    pub(crate) fn settle_registry(&self, client: &ClientInner) {
        let mut server = lock(&self.server);
        client.workspace.mutate(|doc| server.settle(doc));
    }

    pub(crate) fn note_local_row(&self, id: &str) {
        lock(&self.local_rows).insert(id.to_owned(), now_ms());
    }

    // ── lifecycle ──────────────────────────────────────────────────────────

    pub(crate) fn start(self: &Arc<Self>, cancel: CancellationToken) {
        *lock(&self.cancel) = Some(cancel.clone());
        if let Some(client) = self.client.upgrade() {
            // Rows cached from the last session render at once.
            self.settle_registry(&client);
        }
        let host = self.clone();
        crate::runtime::shared().spawn(async move { host.supervise(cancel).await });
    }

    pub(crate) fn stop(&self) {
        self.save_registry();
        if let Some(link) = lock(&self.link).take() {
            link.cancel.cancel();
        }
        for (_, (_, token)) in lock(&self.mirrors).drain() {
            token.cancel();
        }
    }

    /// Reconnect now (foreground, network back, user retry).
    pub(crate) fn kick(&self) {
        self.wake.notify_one();
    }

    fn set_status(&self, f: impl FnOnce(&mut DirectStatus)) {
        f(&mut lock(&self.status));
        if let Some(client) = self.client.upgrade() {
            client.recompute_connectivity();
        }
    }

    async fn supervise(self: Arc<Self>, cancel: CancellationToken) {
        let mut backoff = BACKOFF_MIN;
        loop {
            if cancel.is_cancelled() {
                return;
            }
            self.set_status(|s| {
                s.phase = DirectPhase::Connecting;
                s.retry_at_ms = None;
            });
            let attempt = tokio::select! {
                _ = cancel.cancelled() => return,
                r = self.open_link() => r,
            };
            let wait = match attempt {
                Ok(link) => {
                    let synced = self.run_link(link, &cancel).await;
                    self.clear_active_endpoint();
                    if cancel.is_cancelled() {
                        return;
                    }
                    // A link that never synced backs off like a failed dial,
                    // so a half-working tunnel doesn't spin.
                    let wait = if synced { BACKOFF_MIN } else { backoff };
                    backoff = if synced {
                        BACKOFF_MIN
                    } else {
                        (backoff * 2).min(BACKOFF_MAX)
                    };
                    self.set_status(|s| s.retry_at_ms = Some(now_ms() + wait.as_millis() as i64));
                    Some(wait)
                }
                Err(err) => {
                    let wait = (!err.needs_user()).then_some(backoff);
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    let message = err.to_string();
                    self.note(format!("connect failed: {message}"));
                    self.set_status(|s| {
                        s.phase = DirectPhase::Failed;
                        s.last_error = Some(message);
                        s.retry_at_ms = wait.map(|w| now_ms() + w.as_millis() as i64);
                    });
                    wait
                }
            };
            // Needs-user failures park until kicked (e.g. the app restarts
            // the client with a trusted key).
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = self.wake.notified() => {}
                _ = async {
                    match wait {
                        Some(w) => tokio::time::sleep(w).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    }

    async fn open_link(&self) -> std::result::Result<Link, SshError> {
        let (ssh, rpc) = self.dial_any().await?;
        // EngineInfo and the extra channels (requests, the first transcript)
        // at once: one after the other they cost ~5 round trips (1-3 s each
        // over a DERP relay) before anything else could start.
        let port = self.target.engine_port;
        let open_extra = || async {
            match &ssh {
                Some(ssh) => Some(ssh.open_engine_counted(port).await),
                None => None,
            }
        };
        let (info, calls, transcripts) = tokio::join!(
            tokio::time::timeout(
                Duration::from_secs(20),
                rpc.call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({})),
            ),
            open_extra(),
            open_extra(),
        );
        let calls = calls.map(|c| c.map(|(rpc, _)| rpc));
        let transcripts =
            transcripts.map(|c| c.map(|(rpc, received)| TranscriptChannel { rpc, received }));
        let info = info
            .map_err(|_| SshError::Engine("EngineInfo timed out".into()))?
            .map_err(|e| SshError::Engine(e.to_string()))?;
        let engine_device_id = info
            .get("deviceId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        if engine_device_id.is_empty() {
            return Err(SshError::Engine(
                "the engine's EngineInfo reply has no deviceId".into(),
            ));
        }
        let version = info
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        self.note(format!(
            "engine answered (version {}, device {})",
            version.as_deref().unwrap_or("unknown"),
            short_id(&engine_device_id)
        ));
        self.set_status(|s| {
            // EngineInfo carries no version on current engines; the engine's
            // own Device row fills it in (mirror_devices).
            if version.is_some() {
                s.notice = engine_notice(version.as_deref());
                s.engine_version = version;
            }
            s.engine_device_id = Some(engine_device_id.clone());
        });
        // One-off requests on their own channel (see `Link::calls`); if the
        // machine won't open one, requests share the feed connection.
        let calls = match calls {
            Some(Ok(calls)) => Some(calls),
            Some(Err(err)) => {
                self.note(format!("requests share the feed connection ({err})"));
                None
            }
            None => None,
        };
        // The first transcript's channel (see `Link::transcript_spare`).
        let transcripts = match transcripts {
            Some(Ok(channel)) if calls.is_some() => Some(channel),
            Some(Err(err)) => {
                self.note(format!("transcripts share the feed connection ({err})"));
                None
            }
            _ => None,
        };
        let transcript_channels = transcripts.is_some();
        Ok(Link {
            ssh,
            rpc,
            calls,
            transcript_spare: Mutex::new(transcripts),
            transcript_channels,
            engine_port: self.target.engine_port,
            cancel: CancellationToken::new(),
            engine_device_id,
            heard_ms: std::sync::atomic::AtomicI64::new(now_ms()),
            probing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Dial the machine's addresses Happy-Eyeballs style: the first alone
    /// for its head start, then the next alongside it (or at once when one
    /// fails), and so on; the first to finish SSH + tunnel wins and the
    /// others are dropped. Each address's outcome lands in `endpoints`.
    async fn dial_any(
        &self,
    ) -> std::result::Result<(Option<SshSession>, zeron_rpc::RpcClient), SshError> {
        let endpoints = lock(&self.endpoints).clone();
        let n = endpoints.len();
        let user = self.target.user.trim().to_owned();
        let dial = |i: usize| {
            let endpoint = endpoints[i].clone();
            let target = self.target.at(&endpoint);
            let user = user.clone();
            async move {
                self.note(format!("connecting to {user}@{}", endpoint.describe()));
                self.endpoint_stat(&endpoint, |s| s.last_attempt_ms = Some(now_ms()));
                let began = std::time::Instant::now();
                let reach = (endpoint.connect_timeout_ms > 0)
                    .then(|| Duration::from_millis(u64::from(endpoint.connect_timeout_ms)));
                let result = self.dial(&target, reach).await;
                (i, result, began.elapsed())
            }
        };
        let head_start = |i: usize| Duration::from_millis(u64::from(endpoints[i].head_start_ms));
        let mut racing = futures::stream::FuturesUnordered::new();
        racing.push(dial(0));
        let mut next = 1;
        let timer = tokio::time::sleep(head_start(0));
        tokio::pin!(timer);
        let mut errors = Vec::new();
        loop {
            let staggered = next < n && endpoints[next - 1].head_start_ms > 0;
            tokio::select! {
                Some((i, result, took)) = racing.next() => {
                    let endpoint = &endpoints[i];
                    match result {
                        Ok(pair) => {
                            if n > 1 {
                                self.note(format!("using {}", endpoint.describe()));
                            }
                            let latency = took.as_millis() as u64;
                            self.set_status(|s| {
                                for stat in s.endpoints.iter_mut() {
                                    stat.active = endpoint.same_address(&stat.host, stat.port);
                                    if stat.active {
                                        stat.last_ok_ms = Some(now_ms());
                                        stat.last_error = None;
                                        stat.latency_ms = Some(latency);
                                    }
                                }
                            });
                            return Ok(pair);
                        }
                        Err(err) => {
                            if n > 1 {
                                self.note(format!("{} failed: {err}", endpoint.describe()));
                            }
                            let text = err.to_string();
                            self.endpoint_stat(endpoint, |s| s.last_error = Some(text));
                            errors.push((i, err));
                            if next < n {
                                racing.push(dial(next));
                                timer.as_mut().reset(tokio::time::Instant::now() + head_start(next));
                                next += 1;
                            } else if racing.is_empty() {
                                return Err(SshError::most_telling(errors)
                                    .unwrap_or_else(|| SshError::Connect("no address to dial".into())));
                            }
                        }
                    }
                }
                _ = &mut timer, if staggered => {
                    racing.push(dial(next));
                    timer.as_mut().reset(tokio::time::Instant::now() + head_start(next));
                    next += 1;
                }
            }
        }
    }

    /// One address: SSH (reaching it within `reach`, default 20 s), then
    /// the tunnel to the engine.
    async fn dial(
        &self,
        target: &SshTarget,
        reach: Option<Duration>,
    ) -> std::result::Result<(Option<SshSession>, zeron_rpc::RpcClient), SshError> {
        #[cfg(test)]
        {
            let url = lock(&TEST_ENGINES)
                .as_ref()
                .and_then(|m| m.get(&target.host).cloned());
            match url.as_deref() {
                // Test doubles: an address that never answers, one that
                // refuses, one owned by some other SSH server.
                Some("hang") => match reach {
                    Some(reach) => {
                        tokio::time::sleep(reach).await;
                        return Err(SshError::Connect(format!(
                            "timed out reaching {}:{}",
                            target.host, target.port
                        )));
                    }
                    None => std::future::pending::<()>().await,
                },
                Some("refuse") => {
                    return Err(SshError::Connect(format!(
                        "{}:{} refused the connection",
                        target.host, target.port
                    )));
                }
                Some("stranger") => {
                    return Err(SshError::HostKeyMismatch {
                        expected: target.host_key_fingerprint.clone().unwrap_or_default(),
                        actual: "SHA256:stranger".into(),
                        algorithm: "ssh-ed25519".into(),
                    });
                }
                _ => {}
            }
            if let Some(url) = url {
                let rpc = zeron_rpc::connect_ws(&url)
                    .await
                    .map_err(|e| SshError::Engine(e.to_string()))?;
                return Ok((None, rpc));
            }
        }
        let ssh = match reach {
            Some(reach) => ssh::connect_within(target, reach).await?,
            None => ssh::connect(target).await?,
        };
        self.note(format!(
            "SSH ready ({}); opening tunnel to 127.0.0.1:{}",
            ssh.host_algorithm, target.engine_port
        ));
        let rpc = ssh.open_engine(target.engine_port).await?;
        Ok((Some(ssh), rpc))
    }

    fn catalog_refresh_every(&self) -> Duration {
        #[cfg(test)]
        if let Some(every) = lock(&TEST_CATALOG_REFRESH)
            .as_ref()
            .and_then(|m| m.get(&self.target.host).copied())
        {
            return every;
        }
        crate::client::CATALOG_REFRESH
    }

    /// Re-read the CLI/model lists in the background every
    /// [`crate::client::CATALOG_REFRESH`] while this link is up. Each read
    /// replaces the saved lists only when it succeeds.
    fn refresh_catalog_while_up(&self, link: &Arc<Link>, cancel: &CancellationToken) {
        let every = self.catalog_refresh_every();
        let client = self.client.clone();
        let device = link.engine_device_id.clone();
        let link_gone = link.cancel.clone();
        let stopped = cancel.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(every) => {}
                    _ = link_gone.cancelled() => return,
                    _ = stopped.cancelled() => return,
                }
                let Some(inner) = client.upgrade() else {
                    return;
                };
                tokio::select! {
                    _ = crate::client::Client::prefetch_catalog(inner, device.clone()) => {}
                    _ = link_gone.cancelled() => return,
                    _ = stopped.cancelled() => return,
                }
            }
        });
    }

    /// Mirror the desktop's sidebar pins once the registry is in, then every
    /// [`DESKTOP_PINS_EVERY`] while the link is up.
    fn desktop_pins_while_up(self: &Arc<Self>, link: &Arc<Link>, cancel: &CancellationToken) {
        let host = self.clone();
        let link = link.clone();
        let stopped = cancel.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                let ready = match host.client.upgrade() {
                    Some(client) => client.workspace.state().1.is_some(),
                    None => return,
                };
                if ready {
                    tokio::select! {
                        _ = host.mirror_desktop_pins(&link) => {}
                        _ = link.cancel.cancelled() => return,
                        _ = stopped.cancelled() => return,
                    }
                }
                let wait = if ready {
                    DESKTOP_PINS_EVERY
                } else {
                    Duration::from_millis(500)
                };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = link.cancel.cancelled() => return,
                    _ = stopped.cancelled() => return,
                }
            }
        });
    }

    /// The desktop's pins: the engine's own when it holds them (a synced
    /// profile), else the local profile's from the desktop's settings file,
    /// read (never written) over the SSH session. `None`: unknown, leave the
    /// phone's pins alone.
    async fn read_desktop_pins(link: &Link, platform: Option<&str>) -> Option<Vec<String>> {
        let engine = async {
            let mut sub = link
                .calls()
                .subscribe_scoped(
                    zeron_rpc::methods::WATCH_SIDEBAR_PREFERENCES,
                    serde_json::json!({}),
                )
                .await
                .ok()?;
            let first = sub.recv().await?;
            super::desktop_pins::parse_engine_preferences(&first)
        };
        if let Ok(Some(pins)) = tokio::time::timeout(Duration::from_secs(15), engine).await {
            return Some(pins);
        }
        let ssh = link.ssh.as_ref()?;
        let text = ssh
            .exec_read(
                super::desktop_pins::settings_command(platform),
                4 * 1024 * 1024,
                Duration::from_secs(20),
            )
            .await?;
        super::desktop_pins::parse_ui_settings(&String::from_utf8_lossy(&text))
    }

    async fn mirror_desktop_pins(&self, link: &Link) {
        use super::desktop_pins::{apply_pins, load_mirrored, merge_pins, save_mirrored};
        let platform = self.client.upgrade().and_then(|client| {
            client
                .workspace
                .state()
                .0
                .devices
                .iter()
                .find(|d| d.id == link.engine_device_id)
                .map(|d| d.platform.clone())
        });
        let Some(desktop) = Self::read_desktop_pins(link, platform.as_deref()).await else {
            return;
        };
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let (state, prefs) = client.workspace.state();
        let Some(prefs) = prefs else { return };
        let known: HashSet<String> = state.chats.iter().map(|c| c.id.clone()).collect();
        let desktop: Vec<String> = desktop
            .into_iter()
            .filter(|id| known.contains(id))
            .collect();
        let dir = client.config.data_dir.clone();
        let last = load_mirrored(&dir, &link.engine_device_id);
        if last.as_deref() == Some(desktop.as_slice()) {
            // Unchanged on the desktop: the phone's own edits stand.
            return;
        }
        let target = merge_pins(&desktop, last.as_deref(), &prefs.pinned_session_ids, &known);
        if target != prefs.pinned_session_ids
            && let Err(err) = client.registry_write(|doc| {
                apply_pins(doc, &target);
                Ok(())
            })
        {
            tracing::warn!(error = %err, "direct: desktop pins not applied");
            return;
        }
        save_mirrored(&dir, &link.engine_device_id, &desktop);
        self.note(format!("desktop pins mirrored ({})", desktop.len()));
    }

    /// Drive one link until it drops. Returns whether it ever fully synced.
    async fn run_link(self: &Arc<Self>, link: Link, cancel: &CancellationToken) -> bool {
        let link = Arc::new(link);
        let Some(client) = self.client.upgrade() else {
            return false;
        };
        *lock(&self.engine_device) = Some(link.engine_device_id.clone());
        client
            .workspace
            .set_presence(&link.engine_device_id, now_ms());
        *lock(&self.link) = Some(link.clone());
        self.link_changed.send_modify(|g| *g += 1);
        self.set_status(|s| {
            s.phase = DirectPhase::Syncing;
            s.retry_at_ms = None;
            s.last_error = None;
            s.connected_at_ms = Some(now_ms());
            s.synced_at_ms = None;
            s.streams = REGISTRY_STREAMS
                .iter()
                .map(|name| StreamStat {
                    name: (*name).to_owned(),
                    ..StreamStat::default()
                })
                .collect();
        });
        drop(client);

        for method in REGISTRY_STREAMS {
            let host = self.clone();
            let link = link.clone();
            crate::runtime::shared().spawn(async move {
                host.watch_registry(&link, method).await;
                // A registry stream ending means the tunnel is gone.
                link.cancel.cancel();
            });
        }
        // Re-attach every open session and flush commands queued offline.
        // The transcript on screen first: it streams on its own channel, and
        // holding it back for the CLI/model lists (below) only delayed the
        // newest rows by up to CATALOG_HEAD_START after every reconnect.
        if let Some(client) = self.client.upgrade() {
            for core in client.cores() {
                if Self::wants_mirror(&core) {
                    self.restart_mirror(&core, &link);
                }
                self.on_command(&core.chat_id);
            }
        }
        // The computer's CLI/model lists (on the request channel), before
        // anything else is asked for: New Session then has live lists saved
        // even if the link gets busy later.
        if let Some(client) = self.client.upgrade() {
            let (done, read) = tokio::sync::oneshot::channel::<()>();
            let device = link.engine_device_id.clone();
            crate::runtime::shared().spawn(async move {
                crate::client::Client::prefetch_catalog(client, device).await;
                let _ = done.send(());
            });
            tokio::select! {
                _ = read => {}
                _ = tokio::time::sleep(CATALOG_HEAD_START) => {
                    self.note("the CLI/model lists are still loading; starting transcripts anyway");
                }
                _ = cancel.cancelled() => {}
                _ = link.cancel.cancelled() => {}
            }
        }
        self.refresh_catalog_while_up(&link, cancel);
        self.desktop_pins_while_up(&link, cancel);

        let mut beat = tokio::time::interval(BEAT);
        let sync_deadline = tokio::time::sleep(SYNC_TIMEOUT);
        tokio::pin!(sync_deadline);
        let mut sync_checked = false;
        let mut lost_reason = "connection to the machine lost".to_owned();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = link.cancel.cancelled() => break,
                _ = &mut sync_deadline, if !sync_checked => {
                    sync_checked = true;
                    let missing = self.unsynced_streams();
                    if !missing.is_empty() {
                        let message = format!(
                            "connected to the engine, but it sent no {} within {} s",
                            missing.join(" / "),
                            SYNC_TIMEOUT.as_secs()
                        );
                        self.note(message.clone());
                        lost_reason = message;
                        break;
                    }
                }
                _ = beat.tick() => {
                    if link.ssh.as_ref().is_some_and(|s| s.is_closed()) {
                        lost_reason = "the SSH session closed".into();
                        break;
                    }
                    self.probe_feed(&link);
                    self.reconcile_mirrors();
                    if let Some(client) = self.client.upgrade() {
                        client.workspace.set_presence(&link.engine_device_id, now_ms());
                    }
                }
            }
        }
        link.cancel.cancel();
        {
            let mut slot = lock(&self.link);
            if slot.as_ref().is_some_and(|l| Arc::ptr_eq(l, &link)) {
                *slot = None;
            }
        }
        self.link_changed.send_modify(|g| *g += 1);
        if let Some(ssh) = &link.ssh {
            ssh.close().await;
        }
        self.save_registry();
        if let Some(client) = self.client.upgrade() {
            // Presence decays on its own; drop it now so the dot flips.
            client.workspace.set_presence(&link.engine_device_id, 0);
            client.workspace.set_heard(&link.engine_device_id, None);
            client.recompute_workspace();
        }
        let synced = lock(&self.status).synced_at_ms.is_some();
        if !cancel.is_cancelled() {
            let stream_error = lock(&self.status)
                .streams
                .iter()
                .find_map(|s| s.error.clone().map(|e| format!("{}: {e}", s.name)));
            let message = match stream_error {
                Some(e) if !synced => format!("{lost_reason} ({e})"),
                _ => lost_reason,
            };
            self.note(format!("link down: {message}"));
            self.set_status(|s| {
                s.phase = DirectPhase::Failed;
                s.last_error = Some(message);
            });
        }
        synced
    }

    /// A quiet feed (nothing for [`PROBE_AFTER`]) gets a tiny request on the
    /// feed connection itself: its reply comes back behind everything queued
    /// before it, so it proves the phone is caught up and the engine alive.
    /// Session rows age only up to the last such proof (see
    /// `workspace::view::status_now`), so a run whose own heartbeat stopped
    /// still goes stale while a stalled or backlogged link does not turn
    /// running sessions into "done". Never drops the link.
    fn probe_feed(self: &Arc<Self>, link: &Arc<Link>) {
        use std::sync::atomic::Ordering;
        if now_ms() - link.heard_ms() < PROBE_AFTER.as_millis() as i64
            || link.probing.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let host = self.clone();
        let link = link.clone();
        crate::runtime::shared().spawn(async move {
            let reply = tokio::time::timeout(
                PROBE_TIMEOUT,
                link.rpc
                    .call(zeron_rpc::methods::LOCAL_DEVICE, serde_json::json!({})),
            )
            .await;
            link.probing.store(false, Ordering::Release);
            // Any answer (even an error) came through the feed in order.
            let answered = matches!(
                reply,
                Ok(Ok(_))
                    | Ok(Err(zeron_rpc::RpcError::UnknownMethod(_)
                        | zeron_rpc::RpcError::BadParams(_)
                        | zeron_rpc::RpcError::Failed(_)))
            );
            if answered && !link.cancel.is_cancelled() {
                link.heard();
                if let Some(client) = host.client.upgrade() {
                    client
                        .workspace
                        .set_heard(&link.engine_device_id, Some(link.heard_ms()));
                }
            }
        });
    }

    fn unsynced_streams(&self) -> Vec<String> {
        lock(&self.status)
            .streams
            .iter()
            .filter(|s| s.frames == 0)
            .map(|s| s.name.clone())
            .collect()
    }

    /// First frame on every stream → live.
    fn note_frame(&self, method: &str, rows: usize, skipped: usize) {
        let now = now_ms();
        let became_live = {
            let mut status = lock(&self.status);
            if let Some(stat) = status.streams.iter_mut().find(|s| s.name == method) {
                stat.frames += 1;
                stat.rows = rows as u32;
                stat.skipped_rows = skipped as u32;
                stat.last_frame_ms = Some(now);
            }
            let all = !status.streams.is_empty() && status.streams.iter().all(|s| s.frames > 0);
            if all && status.phase == DirectPhase::Syncing {
                status.phase = DirectPhase::Live;
                status.synced_at_ms = Some(now);
                true
            } else {
                false
            }
        };
        if became_live {
            self.note("workspace synced");
            if let Some(client) = self.client.upgrade() {
                client.recompute_connectivity();
            }
        }
    }

    fn current_link(&self) -> Option<Arc<Link>> {
        lock(&self.link)
            .clone()
            .filter(|l| !l.cancel.is_cancelled())
    }

    async fn wait_link(&self, budget: Duration) -> Result<Arc<Link>> {
        let mut rx = self.link_changed.subscribe();
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            if let Some(link) = self.current_link() {
                return Ok(link);
            }
            self.kick();
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => continue,
                _ => {
                    let why = self
                        .status()
                        .last_error
                        .unwrap_or_else(|| "not connected".into());
                    return Err(ClientError::HostUnavailable(why));
                }
            }
        }
    }

    /// One engine RPC over the tunnel.
    pub(crate) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let link = self.wait_link(LINK_WAIT).await?;
        // Same per-method budget as the relay (a cold `ListModels` probes the
        // harness CLI and can take well past the default), never below ours.
        let budget = crate::live::relay::deadline(method).max(CALL_TIMEOUT);
        match tokio::time::timeout(budget, link.calls().call(method, params)).await {
            Err(_) => Err(ClientError::HostUnavailable(format!("{method} timed out"))),
            Ok(Err(zeron_rpc::RpcError::Closed)) | Ok(Err(zeron_rpc::RpcError::Transport(_))) => {
                link.cancel.cancel();
                Err(ClientError::HostUnavailable("connection lost".into()))
            }
            Ok(Err(zeron_rpc::RpcError::UnknownMethod(m))) => Err(ClientError::Unsupported(m)),
            Ok(Err(err)) => Err(ClientError::HostError(err.to_string())),
            Ok(Ok(value)) => Ok(value),
        }
    }

    /// The chunked-transfer RPC surface as a transport-agnostic call so
    /// `attachments::upload_chunks`/`read_chunks` run unchanged over the
    /// tunnel (the engine whitelists those methods on the link too).
    pub(crate) fn rpc_call(self: &Arc<Self>) -> crate::attachments::RpcCall {
        let host = self.clone();
        std::sync::Arc::new(move |method, params| {
            let host = host.clone();
            Box::pin(async move { host.call(method, params).await })
        })
    }

    /// Fire-and-forget `Mutate` (the local optimistic write already landed).
    pub(crate) fn mutate(self: &Arc<Self>, op: serde_json::Value) {
        let host = self.clone();
        crate::runtime::shared().spawn(async move {
            for attempt in 0..3u64 {
                match host.call(zeron_rpc::methods::MUTATE, op.clone()).await {
                    Ok(_) => return,
                    Err(ClientError::HostUnavailable(_)) => {
                        tokio::time::sleep(Duration::from_secs(2 * (attempt + 1))).await;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, op = %op, "direct Mutate failed");
                        return;
                    }
                }
            }
        });
    }

    // ── registry mirror ────────────────────────────────────────────────────

    async fn watch_registry(self: &Arc<Self>, link: &Arc<Link>, method: &'static str) {
        let mut sub = match link
            .rpc
            .subscribe_scoped(method, serde_json::json!({}))
            .await
        {
            Ok(sub) => sub,
            Err(err) => {
                tracing::warn!(method, error = %err, "direct watch failed");
                let message = format!("subscribe failed: {err}");
                self.note(format!("{method} {message}"));
                self.stream_stat(method, |s| s.error = Some(message));
                return;
            }
        };
        loop {
            let item = tokio::select! {
                _ = link.cancel.cancelled() => return,
                item = sub.recv() => item,
            };
            let Some(mut value) = item else {
                if !link.cancel.is_cancelled() {
                    self.note(format!("{method} stream ended by the engine"));
                    self.stream_stat(method, |s| {
                        s.error.get_or_insert_with(|| "stream ended".into());
                    });
                }
                return;
            };
            // Every frame is a full snapshot: skip straight to the newest
            // one queued so a slow phone never falls behind the engine.
            while let Some(newer) = sub.try_recv() {
                value = newer;
            }
            let Some(client) = self.client.upgrade() else {
                return;
            };
            // Registry frames arrive in order behind everything the engine
            // queued before them: this is how far the phone has caught up.
            link.heard();
            client
                .workspace
                .set_heard(&link.engine_device_id, Some(link.heard_ms()));
            let applied = match method {
                zeron_rpc::methods::WATCH_DEVICES => {
                    self.apply_frame(method, value, &[], |rows: Vec<Device>, _| {
                        self.mirror_devices(&client, rows)
                    })
                }
                zeron_rpc::methods::WATCH_SPACES => {
                    self.apply_frame(method, value, &[], |rows: Vec<Space>, keep| {
                        self.mirror_spaces(&client, rows, keep)
                    })
                }
                zeron_rpc::methods::WATCH_CHATS => {
                    self.apply_frame(method, value, &[], |rows: Vec<Chat>, keep| {
                        self.mirror_chats(&client, rows, keep)
                    })
                }
                _ => self.apply_frame(
                    method,
                    value,
                    &session_fallbacks(),
                    |rows: Vec<Session>, _| self.mirror_sessions(&client, rows),
                ),
            };
            match applied {
                Some(true) => {
                    self.settle_registry(&client);
                    client.mark_direct_synced();
                    client.recompute_workspace();
                }
                Some(false) => client.mark_direct_synced(),
                None => {}
            }
        }
    }

    /// Lenient frame decode: rows that don't parse are skipped (and never
    /// treated as deleted) instead of failing the whole snapshot, so a newer
    /// engine's shape drift degrades one row, not the workspace.
    fn apply_frame<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        value: serde_json::Value,
        substitutions: &[Substitution],
        mirror: impl FnOnce(Vec<T>, &HashSet<String>) -> bool,
    ) -> Option<bool> {
        match decode_rows::<T>(value, substitutions) {
            Ok(decoded) => {
                if let Some(first) = decoded.repaired.first() {
                    tracing::info!(method, repaired = decoded.repaired.len(), note = %first, "direct watch: repaired rows");
                }
                let repaired = decoded.repaired.len() as u32;
                self.stream_stat(method, |s| s.repaired_rows = repaired);
                if let Some(first) = decoded.errors.first() {
                    tracing::warn!(method, skipped = decoded.errors.len(), error = %first, "direct watch: skipped rows");
                    let message = format!(
                        "skipped {} unreadable row(s): {first}",
                        decoded.errors.len()
                    );
                    self.stream_stat(method, |s| s.error = Some(message));
                } else {
                    self.stream_stat(method, |s| s.error = None);
                }
                let (rows, skipped) = (decoded.rows.len(), decoded.errors.len());
                let changed = mirror(decoded.rows, &decoded.ids);
                self.note_frame(method, rows, skipped);
                Some(changed)
            }
            Err(err) => {
                tracing::warn!(method, error = %err, "direct watch: unreadable frame");
                self.note(format!("{method}: unreadable frame: {err}"));
                self.stream_stat(method, |s| {
                    s.error = Some(format!("unreadable frame: {err}"))
                });
                self.set_status(|s| {
                    s.last_error = Some(format!("couldn't read {method} from the engine: {err}"));
                });
                None
            }
        }
    }

    fn recently_local(&self, id: &str) -> bool {
        let now = now_ms();
        let mut rows = lock(&self.local_rows);
        rows.retain(|_, at| now - *at < LOCAL_ROW_GRACE_MS);
        rows.contains_key(id)
    }

    fn mirror_devices(&self, client: &ClientInner, rows: Vec<Device>) -> bool {
        let engine = lock(&self.engine_device).clone();
        if let Some(version) = rows
            .iter()
            .find(|d| Some(&d.id) == engine.as_ref())
            .and_then(|d| d.version.clone())
        {
            let notice = engine_notice(Some(&version));
            let fresh = {
                let mut status = lock(&self.status);
                let fresh = status.engine_version.as_deref() != Some(version.as_str());
                if fresh {
                    status.engine_version = Some(version.clone());
                    status.notice = notice.clone();
                }
                fresh
            };
            if fresh {
                self.note(format!("engine version {version}"));
                if let Some(notice) = notice {
                    self.note(notice);
                }
            }
        }
        let (state, _) = client.workspace.state();
        let changed: Vec<&Device> = rows
            .iter()
            .filter(|row| !state.devices.iter().any(|d| d == *row))
            .collect();
        if changed.is_empty() {
            return false;
        }
        client.workspace.mutate(|doc| {
            for row in changed {
                if let Err(err) = doc.upsert_device(row) {
                    tracing::warn!(error = %err, "mirror device");
                }
            }
        });
        true
    }

    fn mirror_spaces(
        &self,
        client: &ClientInner,
        rows: Vec<Space>,
        keep: &HashSet<String>,
    ) -> bool {
        let (state, _) = client.workspace.state();
        let changed: Vec<&Space> = rows
            .iter()
            .filter(|row| !state.spaces.iter().any(|s| s == *row))
            .collect();
        let gone: Vec<String> = state
            .spaces
            .iter()
            .filter(|s| !keep.contains(&s.id) && !self.recently_local(&s.id))
            .map(|s| s.id.clone())
            .collect();
        if changed.is_empty() && gone.is_empty() {
            return false;
        }
        client.workspace.mutate(|doc| {
            for row in changed {
                if let Err(err) = doc.upsert_space(row) {
                    tracing::warn!(error = %err, "mirror space");
                }
            }
            for id in gone {
                let _ = doc.delete_space(&id);
            }
        });
        true
    }

    fn mirror_chats(&self, client: &ClientInner, rows: Vec<Chat>, keep: &HashSet<String>) -> bool {
        let (state, _) = client.workspace.state();
        let changed: Vec<&Chat> = rows
            .iter()
            .filter(|row| !state.chats.iter().any(|c| c == *row))
            .collect();
        let gone: Vec<String> = state
            .chats
            .iter()
            .filter(|c| !keep.contains(&c.id) && !self.recently_local(&c.id))
            .map(|c| c.id.clone())
            .collect();
        if changed.is_empty() && gone.is_empty() {
            return false;
        }
        client.workspace.mutate(|doc| {
            for row in changed {
                if let Err(err) = doc.upsert_chat(row) {
                    tracing::warn!(error = %err, "mirror chat");
                }
            }
            for id in gone {
                let _ = doc.delete_chat(&id);
            }
        });
        true
    }

    fn mirror_sessions(&self, client: &ClientInner, rows: Vec<Session>) -> bool {
        let rows = lock(&self.session_clock).rebase(rows, chrono::Utc::now());
        let (state, _) = client.workspace.state();
        let changed: Vec<&Session> = rows
            .iter()
            .filter(|row| !state.sessions.iter().any(|s| s == *row))
            .collect();
        if changed.is_empty() {
            return false;
        }
        client.workspace.mutate(|doc| {
            for row in changed {
                if let Err(err) = doc.upsert_session(row) {
                    tracing::warn!(error = %err, "mirror session");
                }
            }
        });
        true
    }

    // ── transcripts ────────────────────────────────────────────────────────

    /// Direct sessions hydrate from the phone's last mirrored snapshot —
    /// a reopened chat shows its history at once, and the engine's tail
    /// splices the newest rows on top (`splice_tail`) before the complete
    /// reset settles it. Only a chat never mirrored (or an unreadable
    /// snapshot) starts empty.
    pub(crate) fn session_doc(&self, chat_id: &str) -> Result<(SessionDoc, bool)> {
        if let Some(store) = &self.shadow_store {
            match store.load_snapshot(chat_id) {
                Ok(Some(bytes)) => {
                    let raw = loro::LoroDoc::new();
                    match raw.import(&bytes) {
                        Ok(_) => {
                            let doc = SessionDoc::from_doc(raw);
                            let rows = doc.read_entries().map(|e| e.len()).unwrap_or(0);
                            self.note(format!(
                                "transcript {}: {} cached rows",
                                short_id(chat_id),
                                rows
                            ));
                            return Ok((doc, true));
                        }
                        Err(err) => tracing::warn!(error = %err, chat = %chat_id,
                            "transcript snapshot unreadable; starting fresh"),
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, chat = %chat_id,
                    "transcript snapshot read failed"),
            }
        }
        Ok((SessionDoc::init(chat_id).map_err(doc_err)?, false))
    }

    /// Persist the shadow transcript so a later open paints its cached
    /// history instantly (see [`Self::session_doc`]).
    fn save_shadow(&self, core: &SessionCore) {
        let Some(store) = &self.shadow_store else { return };
        let Ok(bytes) = core.write(|doc| doc.export_snapshot()) else {
            return;
        };
        if let Err(err) = store.save_snapshot(&core.chat_id, &bytes) {
            tracing::warn!(error = %err, chat = %core.chat_id, "transcript snapshot save failed");
        }
    }

    /// Over a direct link only the transcript on screen (or one with a send
    /// still in flight) streams. Each subscribed transcript costs the whole
    /// live message per update (hundreds of KB) and a multi-MB first
    /// snapshot, all on the one feed the session heartbeats ride; warm
    /// sessions streaming in the background backlogged that feed on slow
    /// links. A session that leaves the screen stops streaming within a
    /// beat; it keeps what it already showed — except one left while its
    /// complete history was still downloading (see [`finishing_history`]).
    fn wants_mirror(core: &SessionCore) -> bool {
        wants_mirror(core)
    }

    /// A session's view came on screen (or it gained a pending send). A
    /// history still finishing for a chat left earlier stops: the one on
    /// screen gets the link.
    pub(crate) fn session_attached(self: &Arc<Self>, core: &Arc<SessionCore>) {
        self.session_opened(core);
        self.reconcile_mirrors();
    }

    /// A session's view left the screen: stop its stream now rather than at
    /// the next beat (its channel closes, dropping whatever the engine had
    /// queued on it).
    pub(crate) fn session_detached(&self) {
        self.reconcile_mirrors();
    }

    /// Stop mirrors nobody needs any more (see [`Self::wants_mirror`]).
    fn reconcile_mirrors(&self) {
        let mut mirrors = lock(&self.mirrors);
        mirrors.retain(|_, (core, token)| {
            let keep = !token.is_cancelled()
                && core.upgrade().is_some_and(|core| Self::wants_mirror(&core));
            if !keep {
                token.cancel();
            }
            keep
        });
    }

    pub(crate) fn session_opened(self: &Arc<Self>, core: &Arc<SessionCore>) {
        if !Self::wants_mirror(core) {
            return;
        }
        if let Some(link) = self.current_link() {
            let mirrors = lock(&self.mirrors);
            if let Some((existing, token)) = mirrors.get(&core.chat_id)
                && !token.is_cancelled()
                && existing.upgrade().is_some_and(|c| Arc::ptr_eq(&c, core))
            {
                return;
            }
            drop(mirrors);
            self.restart_mirror(core, &link);
        } else {
            // The link-up loop starts this mirror once connected (the
            // `wants_mirror` sweep) — that wait is part of a cold open.
            self.note(format!(
                "transcript {} waits for the link",
                short_id(&core.chat_id)
            ));
        }
    }

    fn restart_mirror(self: &Arc<Self>, core: &Arc<SessionCore>, link: &Arc<Link>) {
        let token = link.cancel.child_token();
        if let Some((_, old)) =
            lock(&self.mirrors).insert(core.chat_id.clone(), (Arc::downgrade(core), token.clone()))
        {
            old.cancel();
        }
        let weak = Arc::downgrade(core);
        let link = link.clone();
        let chat_id = core.chat_id.clone();
        let host = self.clone();
        crate::runtime::shared().spawn(async move {
            mirror_transcript(host, link, weak, chat_id, token).await;
        });
    }

    // ── commands ───────────────────────────────────────────────────────────

    pub(crate) fn on_command(self: &Arc<Self>, chat_id: &str) {
        let host = self.clone();
        let chat_id = chat_id.to_owned();
        crate::runtime::shared().spawn(async move { host.drain(&chat_id).await });
    }

    async fn drain(self: &Arc<Self>, chat_id: &str) {
        let gate = lock(&self.drains)
            .entry(chat_id.to_owned())
            .or_default()
            .clone();
        let _serial = gate.lock().await;
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let Some(core) = client.session_core(chat_id) else {
            return;
        };
        let device_id = client.config.device_id.clone();
        // Upload ids whose bytes are still on the wire (staged, mid-transfer,
        // or in an escort retry window). Forwarding a command that names one
        // gets refused by the host — and the refusal marks it Rejected — so
        // hold it here until its escort commits instead.
        let in_flight: HashSet<String> = client
            .escorts
            .pending_for(chat_id)
            .into_iter()
            .collect();
        drop(client);
        let now = now_ms();
        let pending: Vec<_> = core
            .doc()
            .read_commands()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| {
                c.issued_by == device_id
                    && c.status == SessionCommandStatus::Pending
                    && now < c.effective_expiry()
            })
            .collect();
        for command in pending {
            // Bytes still in flight: hold this command — and everything after
            // it, so sends stay ordered — until its escort commits; the
            // escort's nudge re-runs this drain.
            if crate::attachments::command_pending_uploads(&command)
                .iter()
                .any(|id| in_flight.contains(id))
            {
                break;
            }
            let params = serde_json::json!({ "chatId": chat_id, "entry": command });
            // RelayCommand is idempotent on the command id: retry freely.
            let mut outcome = None;
            for attempt in 0..3u64 {
                match self
                    .call(zeron_rpc::methods::RELAY_COMMAND, params.clone())
                    .await
                {
                    Ok(value) => {
                        outcome = Some(
                            value
                                .get("outcome")
                                .and_then(|v| v.as_str())
                                .unwrap_or("executed")
                                .to_owned(),
                        );
                        break;
                    }
                    Err(ClientError::HostUnavailable(_)) => {
                        // Stays pending; the next link replays it.
                        if attempt == 2 {
                            return;
                        }
                        tokio::time::sleep(Duration::from_secs(1 + attempt)).await;
                    }
                    Err(err) => {
                        let message = err.to_string();
                        let _ = core.write(|doc| {
                            doc.set_command_status(
                                &command.id,
                                SessionCommandStatus::Rejected,
                                Some(&message),
                            )
                        });
                        outcome = None;
                        break;
                    }
                }
            }
            let Some(outcome) = outcome else { continue };
            let status = match outcome.as_str() {
                "expired" => SessionCommandStatus::Expired,
                "superseded" => SessionCommandStatus::Superseded,
                _ => SessionCommandStatus::Applied,
            };
            let _ = core.write(|doc| doc.set_command_status(&command.id, status, None));
        }
    }
}

/// How long a chat left while its complete history was still downloading
/// keeps downloading it (so coming back shows it, instead of starting the
/// multi-MB reset over: minutes over a relay, measured in
/// `tests/slow_link_sim.rs` `slow_link_history`).
const HISTORY_FINISH_MS: i64 = 10 * 60_000;

/// See [`DirectHost::wants_mirror`].
fn wants_mirror(core: &SessionCore) -> bool {
    core.view_attached() || core.has_pending_sends() || finishing_history(core)
}

/// Left while only its opening tail had arrived: the complete history keeps
/// coming (then the stream stops) for [`HISTORY_FINISH_MS`], unless another
/// transcript is on screen — that one comes first on a slow link.
fn finishing_history(core: &SessionCore) -> bool {
    let left = core.detached_ms();
    left > 0
        && core.history_pending()
        && now_ms() - left < HISTORY_FINISH_MS
        && !core.other_view_attached()
}

/// Stream one chat's transcript into its shadow doc until the link drops,
/// the core is evicted, or the mirror is replaced (or, off screen, once
/// what it was kept for is done: see [`finishing_history`]).
async fn mirror_transcript(
    host: Arc<DirectHost>,
    link: Arc<Link>,
    core: Weak<SessionCore>,
    chat_id: String,
    token: CancellationToken,
) {
    let opened_ms = now_ms();
    let chat = short_id(&chat_id);
    let elapsed = |from: i64| (now_ms() - from).max(0) as f64 / 1000.0;
    // What the shadow doc holds, entry by entry (index = list position).
    // Seeded from the doc: after a reconnect it already holds the transcript,
    // and starting from empty would append every entry a second time.
    let mut written: Vec<SessionMessageEntry> = match core.upgrade() {
        Some(core) => core.doc().read_entries().unwrap_or_default(),
        None => return,
    };
    let seeded = !written.is_empty();
    // This transcript's own channel (see `Link::transcript_spare`); dropped
    // (closing the channel) when the mirror ends, or swapped for a fresh one
    // when it falls behind (see `lag_check`).
    let mut own = tokio::select! {
        _ = token.cancelled() => return,
        own = link.transcript_channel() => own,
    };
    host.note(format!(
        "transcript {chat}: {} in {:.1}s",
        if own.is_some() {
            "own channel"
        } else {
            "sharing the feed"
        },
        elapsed(opened_ms)
    ));
    let mut tail_logged = seeded;
    let mut history_logged = false;
    // Persist what the mirror writes so the next open paints it instantly.
    let mut shadow = ShadowSaver::new(host.clone(), core.clone());
    let mut save_tick = tokio::time::interval(SHADOW_SAVE_EVERY);
    save_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    save_tick.tick().await; // the interval's first tick is immediate
    loop {
        let rpc = own.as_ref().map_or(&link.rpc, |own| &own.rpc);
        // Bytes in on this channel (its own only), for the history's progress.
        let received = own.as_ref().map(|own| own.received.clone());
        let mut history_from: Option<u64> = None;
        let mut progress = tokio::time::interval(HISTORY_PROGRESS_EVERY);
        progress.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // `openingTail`: the engine first sends the newest ~128 parts
        // (`historyPending: true`), then the complete reset, then deltas.
        // Engines without it ignore the flag and open with the reset.
        let params = serde_json::json!({ "chatId": chat_id, "openingTail": true });
        let subscribe_ms = now_ms();
        let mut sub = match rpc
            .subscribe_scoped(zeron_rpc::methods::WATCH_DOC_MESSAGES, params)
            .await
        {
            Ok(sub) => {
                host.note(format!(
                    "transcript {chat}: subscribed in {:.1}s",
                    elapsed(subscribe_ms)
                ));
                sub
            }
            Err(zeron_rpc::RpcError::Closed) | Err(zeron_rpc::RpcError::Transport(_)) => {
                // The transcript channel died: redial rather than go blank.
                link.cancel.cancel();
                return;
            }
            Err(_) => return,
        };
        let mut entries: Vec<SessionMessageEntry> = Vec::new();
        // Armed on a channel of its own once the complete reset is in.
        let mut lag: Option<
            std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>>,
        > = None;
        let mut fresh = None;
        // A frame taken while batching that must go through the loop itself.
        let mut carry: Option<serde_json::Value> = None;
        loop {
            let item = if let Some(value) = carry.take() {
                Some(value)
            } else {
                tokio::select! {
                _ = token.cancelled() => return,
                behind = async {
                    match lag.as_mut() {
                        Some(check) => check.await,
                        None => std::future::pending().await,
                    }
                } => {
                    lag = None;
                    if !behind {
                        lag = Some(Box::pin(lag_check(rpc)));
                        continue;
                    }
                    // A fresh channel starts with an empty engine queue: its
                    // opening tail brings the newest rows at once.
                    let next = tokio::select! {
                        _ = token.cancelled() => return,
                        next = link.transcript_channel() => next,
                    };
                    match next {
                        Some(next) => {
                            tracing::info!(chat = %chat_id, "direct transcript behind; fresh channel");
                            fresh = Some(next);
                            break;
                        }
                        None => continue,
                    }
                }
                _ = save_tick.tick(), if shadow.dirty => {
                    shadow.flush_now();
                    continue;
                }
                _ = progress.tick(), if history_from.is_some() => {
                    // How much of the complete history has come in so far.
                    if let (Some(from), Some(received), Some(core)) =
                        (history_from, received.as_ref(), core.upgrade())
                    {
                        let now = received.load(std::sync::atomic::Ordering::Relaxed);
                        core.set_history_received(now.saturating_sub(from));
                    }
                    continue;
                }
                item = sub.recv() => item,
                }
            };
            let Some(value) = item else {
                // Stream closed (link gone, or the engine dropped it).
                if token.is_cancelled() || link.cancel.is_cancelled() {
                    return;
                }
                break;
            };
            let Some(core) = core.upgrade() else { return };
            let mut value = value;
            let history_pending = value
                .get("historyPending")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if history_pending && !written.is_empty() {
                // The session already shows its history (kept from the last
                // visit, or a resubscribe): never swap it for the tail. The
                // newest rows go on top of it at once (see `splice_tail`);
                // the complete reset follows and settles what's in between.
                let mut value = value;
                sanitize_transcript_update(&mut value);
                if let Ok(TranscriptUpdate {
                    frame: TranscriptFrame::Reset { reset: tail },
                    ..
                }) = serde_json::from_value::<TranscriptUpdate>(value)
                    && let Some(target) = splice_tail(&written, &tail)
                    && target != written
                {
                    if let Err(err) = core.write(|doc| reconcile(doc, &mut written, &target)) {
                        tracing::warn!(error = %err, "direct transcript tail write failed");
                        written.clear();
                        let _ = core.write(|doc| doc.truncate_messages(0));
                    }
                    core.schedule_refresh();
                    shadow.mark();
                }
                history_from = received
                    .as_ref()
                    .map(|r| r.load(std::sync::atomic::Ordering::Relaxed));
                continue;
            }
            let repaired = sanitize_transcript_update(&mut value);
            if repaired > 0 {
                tracing::info!(repaired, chat = %chat_id, "direct transcript: repaired items");
            }
            let update: TranscriptUpdate = match serde_json::from_value(value) {
                Ok(update) => update,
                Err(err) => {
                    tracing::warn!(error = %err, "direct transcript: bad frame");
                    continue;
                }
            };
            if history_pending {
                // The newest rows, shown at once; `entries` (the complete
                // list later deltas apply to) waits for the full reset.
                let TranscriptFrame::Reset { reset: tail } = update.frame else {
                    continue;
                };
                if let Err(err) = core.write(|doc| reconcile(doc, &mut written, &tail)) {
                    tracing::warn!(error = %err, "direct transcript tail write failed");
                    written.clear();
                    let _ = core.write(|doc| doc.truncate_messages(0));
                    continue;
                }
                if let Some(usage) = update.context_usage {
                    let _ = core.write(|doc| doc.update_context_usage(usage.tokens, usage.window));
                }
                core.set_hydrated();
                // Older rows follow with the complete reset: the transcript's
                // head says so until then.
                core.set_history_pending(true);
                core.schedule_refresh();
                if !tail_logged {
                    tail_logged = true;
                    host.note(format!(
                        "transcript {chat}: newest {} rows in {:.1}s",
                        written.len(),
                        elapsed(opened_ms)
                    ));
                }
                shadow.mark();
                history_from = received
                    .as_ref()
                    .map(|r| r.load(std::sync::atomic::Ordering::Relaxed));
                continue;
            }
            if let Err(err) = apply_transcript_frame(&mut entries, update.frame) {
                tracing::info!(error = %err, "direct transcript desync; resubscribing");
                break;
            }
            // Frames that queued up meanwhile go into one write: a busy turn
            // re-sends its whole live row per tick, and writing every copy
            // to the doc kept the phone behind what it had already received.
            let mut usage = update.context_usage;
            let mut desync = None;
            for _ in 0..TRANSCRIPT_BATCH {
                let Some(mut next) = sub.try_recv() else {
                    break;
                };
                if next
                    .get("historyPending")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    carry = Some(next);
                    break;
                }
                sanitize_transcript_update(&mut next);
                let Ok(next) = serde_json::from_value::<TranscriptUpdate>(next) else {
                    continue;
                };
                if let Err(err) = apply_transcript_frame(&mut entries, next.frame) {
                    desync = Some(err);
                    break;
                }
                if next.context_usage.is_some() {
                    usage = next.context_usage;
                }
            }
            if let Some(err) = desync {
                tracing::info!(error = %err, "direct transcript desync; resubscribing");
                break;
            }
            if let Err(err) = core.write(|doc| reconcile(doc, &mut written, &entries)) {
                tracing::warn!(error = %err, "direct transcript write failed");
                written.clear();
                let _ = core.write(|doc| doc.truncate_messages(0));
                break;
            }
            if let Some(usage) = usage {
                let _ = core.write(|doc| doc.update_context_usage(usage.tokens, usage.window));
            }
            core.set_hydrated();
            // The complete transcript is in (the first frame after the tail
            // is the complete reset).
            history_from = None;
            core.set_history_pending(false);
            core.schedule_refresh();
            shadow.mark();
            if !history_logged {
                history_logged = true;
                // The complete history just landed: keep the snapshot for
                // the next open without waiting out the save tick.
                shadow.flush_now();
                host.note(format!(
                    "transcript {chat}: full history ({} rows) in {:.1}s",
                    written.len(),
                    elapsed(opened_ms)
                ));
            }
            if !wants_mirror(&core) {
                // Kept streaming off screen only to finish the history.
                token.cancel();
                return;
            }
            // The backlog check starts only now: never while the complete
            // history is still coming (swapping the channel then would throw
            // the partial download away and start it over).
            if lag.is_none() && own.is_some() {
                lag = Some(Box::pin(lag_check(rpc)));
            }
        }
        drop(lag);
        drop(sub);
        if let Some(next) = fresh {
            // Closing the old channel drops whatever the engine still had
            // queued on it.
            own = Some(next);
            continue;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// How often the complete history's download progress is published.
const HISTORY_PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// How often a dirty shadow transcript goes to disk while its mirror runs
/// (the complete history flushes on arrival; the saver's drop flushes the
/// rest when the mirror ends).
const SHADOW_SAVE_EVERY: Duration = Duration::from_secs(30);

/// Tracks transcript writes a mirror still owes the snapshot store. Marks
/// on every doc write; flushes on the save tick, when the complete history
/// lands, and on drop (the mirror ending — link down, chat left, core
/// evicted).
struct ShadowSaver {
    host: Arc<DirectHost>,
    core: Weak<SessionCore>,
    dirty: bool,
}

impl ShadowSaver {
    fn new(host: Arc<DirectHost>, core: Weak<SessionCore>) -> Self {
        Self {
            host,
            core,
            dirty: false,
        }
    }

    fn mark(&mut self) {
        self.dirty = true;
    }

    fn flush_now(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(core) = self.core.upgrade() {
            self.host.save_shadow(&core);
        }
    }
}

impl Drop for ShadowSaver {
    fn drop(&mut self) {
        self.flush_now();
    }
}

/// At most this many queued transcript frames go into one doc write (the
/// rest wait for the next round, so the mirror still yields to its checks).
const TRANSCRIPT_BATCH: usize = 32;

/// How often an open transcript's own channel is checked for backlog, and
/// how long its check may take before it counts as behind. Settable for
/// tests ([`super::set_transcript_lag_check`]).
static LAG_EVERY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);
static LAG_LIMIT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(15_000);

pub(crate) fn set_lag_check(every: Duration, limit: Duration) {
    use std::sync::atomic::Ordering;
    LAG_EVERY_MS.store(every.as_millis() as u64, Ordering::Relaxed);
    LAG_LIMIT_MS.store(limit.as_millis() as u64, Ordering::Relaxed);
}

/// After a pause, a tiny request on a transcript's own channel: its reply
/// comes back behind every frame the engine queued before it. `true` when
/// it doesn't come back in time: the channel is that far behind.
///
/// The engine queues up to 256 frames per connection, and a busy agentic
/// turn re-sends its whole live row on every tool update: measured on
/// Villa's 0.2.101 engine (2026-10-02), a running chat whose last row has
/// 1290 parts produced 19 frames (5.2 MB, ~1.2 MB deflated) a minute,
/// ~484 KB each — more than a relay carries. The phone then showed the
/// chat as it was further and further back (minutes), never its newest
/// rows. A fresh channel drops that queue; the newest rows come first.
async fn lag_check(rpc: &zeron_rpc::RpcClient) -> bool {
    use std::sync::atomic::Ordering;
    tokio::time::sleep(Duration::from_millis(LAG_EVERY_MS.load(Ordering::Relaxed))).await;
    let limit = Duration::from_millis(LAG_LIMIT_MS.load(Ordering::Relaxed));
    let reply = tokio::time::timeout(
        limit,
        rpc.call(zeron_rpc::methods::LOCAL_DEVICE, serde_json::json!({})),
    )
    .await;
    // Any answer, even an error, came through in order; a dead channel
    // ends the stream itself.
    reply.is_err()
}

/// A kept transcript with the engine's opening tail (the newest rows, the
/// first one possibly cut to its last parts) laid over its end. `None` only
/// for an empty tail.
///
/// When more changed while away than the tail covers (its first row isn't
/// kept, or that row grew by more parts than the tail carries — a busy
/// agentic turn adds hundreds), the tail still goes on top: the newest rows
/// show now, with a gap before them that the complete reset fills. Waiting
/// for the reset instead left every newest row missing for as long as the
/// reset took (megabytes: minutes over a relay), or for good if the chat
/// was left before it arrived.
pub(crate) fn splice_tail(
    kept: &[SessionMessageEntry],
    tail: &[SessionMessageEntry],
) -> Option<Vec<SessionMessageEntry>> {
    let first = tail.first()?;
    let Some(at) = kept.iter().position(|e| e.id == first.id) else {
        // Nothing kept lines up: the kept rows, then the newest ones.
        let newest: std::collections::HashSet<&str> = tail.iter().map(|e| e.id.as_str()).collect();
        return Some(
            kept.iter()
                .filter(|e| !newest.contains(e.id.as_str()))
                .cloned()
                .chain(tail.iter().cloned())
                .collect(),
        );
    };
    let mut joined = first.clone();
    if let Some(head) = first.parts.first() {
        joined.parts = match kept[at].parts.iter().position(|p| p.id() == head.id()) {
            // The kept row's parts before the tail's window, then the window.
            Some(from) => kept[at].parts[..from]
                .iter()
                .cloned()
                .chain(first.parts.iter().cloned())
                .collect(),
            // The row grew past the window: what was kept, then the window.
            None => {
                let window: std::collections::HashSet<&str> =
                    first.parts.iter().map(|p| p.id()).collect();
                kept[at]
                    .parts
                    .iter()
                    .filter(|p| !window.contains(p.id()))
                    .cloned()
                    .chain(first.parts.iter().cloned())
                    .collect()
            }
        };
    } else {
        joined.parts = kept[at].parts.clone();
    }
    Some(
        kept[..at]
            .iter()
            .cloned()
            .chain(std::iter::once(joined))
            .chain(tail[1..].iter().cloned())
            .collect(),
    )
}

/// Bring the shadow doc from `written` to `target` with minimal writes:
/// streaming text grows through `SegmentWriter` (LoroText appends), other
/// edits rewrite one entry, and anything non-prefix rebuilds.
fn reconcile(
    doc: &SessionDoc,
    written: &mut Vec<SessionMessageEntry>,
    target: &[SessionMessageEntry],
) -> std::result::Result<(), zeron_doc::DocError> {
    let common = written
        .iter()
        .zip(target)
        .take_while(|(w, t)| w.id == t.id)
        .count();
    if common < written.len() {
        doc.truncate_messages(common)?;
        written.truncate(common);
    }
    for (i, next) in target.iter().enumerate() {
        match written.get(i) {
            None => {
                doc.push_message(next)?;
                written.push(next.clone());
            }
            Some(prev) if prev == next => {}
            Some(prev) => {
                let appendable = prev.parts.len() <= next.parts.len()
                    && prev.parts.iter().zip(&next.parts).all(|(a, b)| {
                        a.id() == b.id() && std::mem::discriminant(a) == std::mem::discriminant(b)
                    });
                if appendable {
                    if prev.parts != next.parts {
                        let mut writer = SegmentWriter::resume(doc, i, prev.parts.clone());
                        writer.sync(&next.parts)?;
                    }
                    if prev.status != next.status
                        || prev.duration_ms != next.duration_ms
                        || prev.created_at != next.created_at
                    {
                        doc.set_entry_scalars(i, next)?;
                    }
                } else {
                    doc.replace_message(i, next)?;
                }
                written[i] = next.clone();
            }
        }
    }
    Ok(())
}

/// Unknown enum values in session rows map to a safe default.
fn session_fallbacks() -> Vec<Substitution> {
    vec![("status", serde_json::Value::String("idle".into()))]
}

/// The notice for an engine newer than [`TESTED_ENGINE`] by minor/major.
pub(crate) fn engine_notice(version: Option<&str>) -> Option<String> {
    let (major, minor, _) = parse_version(version?)?;
    let (tm, tn, tp) = TESTED_ENGINE;
    ((major, minor) > (tm, tn)).then(|| {
        format!(
            "This computer runs Zeron {}, newer than this app was checked with ({tm}.{tn}.{tp}). \
             Everything should keep working; if something looks wrong, check for an app update.",
            version.unwrap_or_default()
        )
    })
}
