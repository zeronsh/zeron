//! App state: the engine connection, entity lists, and the selected chat's
//! transcript — one gpui [`Entity`] the whole shell renders from.
//!
//! ## EngineHandle
//! The UI talks the same typed RPC whether the engine is in-process or a separate
//! daemon (ARCHITECTURE §1). [`EngineHandle::bootstrap`] probes the localhost IPC
//! port, mirroring zeron: if an engine is listening it connects over WebSocket
//! ([`RemoteEngine`]); otherwise it embeds one via [`EngineCore::assemble`] and an
//! in-memory RPC transport ([`InProcessEngine`]) — same envelopes, same dispatch.
//!
//! ## Async bridging
//! `bootstrap` runs on tokio via `gpui_tokio::Tokio::spawn`. Once an [`RpcClient`]
//! exists, its `call`/`subscribe` futures are runtime-agnostic (tokio channels),
//! so subscription pumps run on gpui's own executor via `cx.spawn` and fold each
//! frame into the entity with `this.update(...)` + `cx.notify()`.
//!
//! Pure logic (sort order, staleness, gate phase) lives in free functions with
//! unit tests; rendering reads them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gpui::{App, Context, Entity, Task};
use gpui_tokio::Tokio;
use serde::de::DeserializeOwned;

use crate::comments::ReviewComment;
use zeron_doc::{SessionMessageEntry, TranscriptDesync, TranscriptFrame};
use zeron_engine::{EngineConfig, EngineRuntime, InstanceLock};
use zeron_proto::{
    AuthState, ChangeRequestSummary, Chat, ChatIndicator, CheckoutChangeRequestStatus, Device,
    EngineInfo, HarnessId, Session, SidebarPreferencesState, Space, WorkspaceScope,
};
use zeron_rpc::{RpcClient, RpcError, RpcService, memory_client, methods};

use crate::change_requests::{
    ChangeRequestClientState, ChangeRequestWatchKey, desired_watch_targets, watch_params,
};

// Recently viewed transcripts stay renderable while a fresh watch opens. Move
// ownership on navigation; never clone whale payloads or retain live watches.
const TRANSCRIPT_CACHE_CAP: usize = 12;
const TRANSCRIPT_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct CachedTranscript {
    prepared: Option<Arc<crate::transcript::PreparedTranscript>>,
    chat_id: String,
    entries: Vec<SessionMessageEntry>,
    context_usage: Option<zeron_proto::ContextUsage>,
    bytes: usize,
}

// A cancelled GPUI watch may own a whale's mirror between updates. Its
// destructor must not free that entire object graph on the UI thread either.
struct WatchPreparation {
    worker: Option<crate::transcript::TranscriptPreparation>,
    executor: gpui::BackgroundExecutor,
}
impl WatchPreparation {
    fn new(executor: gpui::BackgroundExecutor) -> Self {
        Self {
            worker: Some(Default::default()),
            executor,
        }
    }
    fn prepare(
        &mut self,
        update: &zeron_doc::TranscriptUpdate,
    ) -> Result<Arc<crate::transcript::PreparedTranscript>, TranscriptDesync> {
        self.worker.as_mut().unwrap().prepare(update)
    }
}
impl Drop for WatchPreparation {
    fn drop(&mut self) {
        let worker = self.worker.take();
        self.executor
            .spawn(async move {
                drop(worker);
            })
            .detach();
    }
}

// ---------------------------------------------------------------------------
// Engine handle
// ---------------------------------------------------------------------------

/// Everything needed to reach (or start) an engine.
#[derive(Debug, Clone)]
pub struct EngineBootConfig {
    /// Data directory for the embedded engine (`~/.zeron`).
    pub data_dir: PathBuf,
    /// Localhost IPC port to probe / serve.
    pub ipc_port: u16,
    /// Edge base URL for the embedded engine.
    pub edge_url: String,
    /// Bearer for edge room joins; `None` runs offline.
    pub edge_token: Option<String>,
    /// Workspace org override for explicit dev-mode runs.
    pub org_id: Option<String>,
    /// WorkOS client id for production authentication.
    pub workos_client_id: Option<String>,
    /// Harness for doc-command runs until per-chat config lands (M4).
    pub default_harness: HarnessId,
}

/// How this UI reached its engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineMode {
    /// Engine embedded in this process (in-memory RPC transport).
    InProcess,
    /// Connected to a separate daemon over localhost WebSocket.
    Remote { url: String },
}

/// One of the two ways to own an engine connection. Both end at an [`RpcClient`]
/// speaking the identical protocol — the trait only differs in provenance and
/// teardown.
#[async_trait]
trait EngineBackend: Send + Sync {
    fn client(&self) -> &RpcClient;
    fn mode(&self) -> EngineMode;
    async fn media_client(&self) -> Result<RpcClient, RpcError> {
        Err(RpcError::Failed("voice media unavailable".into()))
    }
    /// Graceful teardown (drains runs / flushes docs for the in-process engine).
    async fn shutdown(&self);
    /// Resolves when the connection to the engine is lost (a separate
    /// engine exited or crashed). Never for an in-process engine.
    async fn closed(&self) {
        std::future::pending::<()>().await
    }
}

/// Embedded engine: owns the [`EngineCore`] and an in-memory RPC loop.
struct InProcessEngine {
    service: Arc<dyn RpcService>,
    runtime: Arc<tokio::sync::Mutex<Option<EngineRuntime>>>,
    boot_task: tokio::task::JoinHandle<()>,
    refresh_task: tokio::task::JoinHandle<()>,
    /// Serves this engine to other viewports over the IPC port. `None` when the
    /// port was already taken — the window still works over its own transport.
    ipc_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    client: RpcClient,
}

#[async_trait]
impl EngineBackend for InProcessEngine {
    async fn media_client(&self) -> Result<RpcClient, RpcError> {
        Ok(memory_client(self.service.clone()))
    }
    fn client(&self) -> &RpcClient {
        &self.client
    }
    fn mode(&self) -> EngineMode {
        EngineMode::InProcess
    }
    async fn shutdown(&self) {
        self.boot_task.abort();
        // Stop accepting first: a viewport must not connect midway through the
        // drain and queue work against stores that are closing.
        let ipc_task = self.ipc_task.lock().await.take();
        if let Some(ipc) = ipc_task {
            ipc.abort();
            // `abort` only requests cancellation. Observe task completion so the
            // listener is closed before bootstrap reports an assembly failure.
            let _ = ipc.await;
        }
        // As the background engine does: no token refresh may be mid-exchange
        // when another engine takes over the session file (single-use token).
        self.refresh_task.abort();
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime
                .core()
                .auth()
                .quiesce_refresh(std::time::Duration::from_secs(10))
                .await;
            runtime.shutdown().await;
        }
    }
}

use zeron_engine::deferred::{
    DeferredEngine, DeferredState as DeferredEngineState, wait as wait_for_deferred_engine,
};

/// Start the app's background engine when no engine serves the port, and
/// attach to it, rather than embedding one (set once at startup by the app;
/// tests and fixtures embed).
static LAUNCH_BACKGROUND_ENGINE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Have [`EngineHandle::bootstrap`] start the background engine
/// (`zeron_engine::managed`) instead of embedding one. Embedding stays the
/// fallback when it can't start.
pub fn launch_background_engine() {
    LAUNCH_BACKGROUND_ENGINE.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// After a background engine goes away unasked, the app starts it again —
/// first at once-ish, then backing off so a crashing engine can't spin. One
/// that keeps dying is a bug to see, not hide: past the last delay the app
/// stops and shows the failure (with Retry). Embedding wouldn't help — an
/// engine bug crashes an in-process engine too, taking the window with it.
const ENGINE_RESPAWN_BACKOFF: [std::time::Duration; 4] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
];
const ENGINE_LOSS_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Record an unasked engine loss: how long to wait before starting it
/// again, or `None` once losses come too often to keep trying.
fn record_engine_loss(
    losses: &mut Vec<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    losses.retain(|at| now.saturating_duration_since(*at) < ENGINE_LOSS_WINDOW);
    losses.push(now);
    ENGINE_RESPAWN_BACKOFF.get(losses.len() - 1).copied()
}

/// External daemon over `ws://127.0.0.1:{port}`.
struct RemoteEngine {
    client: Arc<RpcClient>,
    url: String,
    /// The daemon's IPC port and data dir: a second connection presents the
    /// same bearer the first did.
    ipc_port: u16,
    data_dir: std::path::PathBuf,
    lifecycle_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[async_trait]
impl EngineBackend for RemoteEngine {
    async fn media_client(&self) -> Result<RpcClient, RpcError> {
        zeron_rpc::connect_ipc(self.ipc_port, Some(&self.data_dir)).await
    }
    fn client(&self) -> &RpcClient {
        &self.client
    }
    fn mode(&self) -> EngineMode {
        EngineMode::Remote {
            url: self.url.clone(),
        }
    }
    async fn closed(&self) {
        self.client.closed().await;
    }
    async fn shutdown(&self) {
        // The daemon outlives this viewport; only stop our readiness probe.
        if let Some(task) = self.lifecycle_task.lock().await.take() {
            task.abort();
        }
    }
}

/// Cheaply clonable handle to whichever backend won the probe.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<dyn EngineBackend>,
    engine_info: EngineInfo,
    deferred_state: Option<tokio::sync::watch::Receiver<DeferredEngineState>>,
}

impl EngineHandle {
    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Probe the IPC port and connect (daemon listening) or embed (nothing there).
    /// Must run on the tokio runtime (`Tokio::spawn`): both transports spawn
    /// tokio tasks.
    pub async fn bootstrap(config: EngineBootConfig) -> anyhow::Result<EngineHandle> {
        // Invariant: at most one bootstrap in this process runs probe+embed at
        // a time. The winner binds the deferred IPC listener before releasing
        // the gate, so a concurrent viewport's probe finds it and attaches as
        // Remote instead of racing it for the data dir.
        static BOOTSTRAP_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _gate = BOOTSTRAP_GATE.lock().await;

        if let Some(handle) = Self::attach_to_daemon(config.ipc_port, &config.data_dir).await {
            return Ok(handle);
        }

        let engine_config = EngineConfig {
            data_dir: config.data_dir,
            edge_url: config.edge_url,
            edge_token: config.edge_token,
            ipc_port: config.ipc_port,
            default_harness: config.default_harness,
            org_id: config.org_id,
            workos_client_id: config.workos_client_id,
        };
        let launching = LAUNCH_BACKGROUND_ENGINE.load(std::sync::atomic::Ordering::SeqCst);
        if Self::should_launch_background(launching, engine_config.ipc_port).await
            && let Some(handle) = Self::launch_background(&engine_config).await
        {
            return Ok(handle);
        }

        tracing::info!(data_dir = %engine_config.data_dir.display(), "no engine on port; embedding one");
        // Own the data dir before opening anything under it or binding IPC —
        // the lock, not the port bind, is the ownership decision. A failed
        // acquire means an out-of-process engine holds the dir but was not
        // serving IPC at probe time (a daemon mid-start): wait for its
        // listener, re-trying the lock in case it dies instead.
        std::fs::create_dir_all(&engine_config.data_dir)?;
        let lock_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let lock = loop {
            match InstanceLock::acquire(&engine_config.data_dir) {
                Ok(lock) => break lock,
                Err(err) => {
                    if std::time::Instant::now() >= lock_deadline {
                        return Err(err.into());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    if let Some(handle) =
                        Self::attach_to_daemon(engine_config.ipc_port, &engine_config.data_dir)
                            .await
                    {
                        return Ok(handle);
                    }
                }
            }
        };

        let mut deferred = DeferredEngine::prepare(&engine_config).await?;
        let service = deferred.service.clone();
        let client = memory_client(service.clone());

        // Serve the same service on the IPC port so a terminal viewport can
        // attach to this window's engine with no setup. Deliberately the
        // *deferred* service, not the assembled one: a viewport that connects
        // during cloud onboarding gets EngineInfo and AuthRpc immediately, and
        // its data subscriptions wait exactly as this window's do.
        //
        // Best-effort — losing the bind race with another engine costs other
        // viewports, not this one.
        let served = zeron_engine::serve_ipc(
            engine_config.ipc_port,
            &engine_config.data_dir,
            service.clone(),
        )
        .await;
        let ipc_task = match served {
            Ok(task) => Some(task),
            Err(err) => {
                tracing::warn!(
                    port = engine_config.ipc_port,
                    error = %err,
                    "IPC port unavailable; other viewports cannot attach to this window"
                );
                None
            }
        };
        // Agents only learn a port THIS window serves: a lost bind race must
        // not point their injected MCP server at some other engine.
        let served_ipc_port = ipc_task.as_ref().map(|_| engine_config.ipc_port);
        let boot_task = deferred.spawn_boot(engine_config, lock, served_ipc_port, |_| {});
        let mut state_rx = deferred.state();
        let handle = EngineHandle {
            inner: Arc::new(InProcessEngine {
                service,
                runtime: deferred.runtime.clone(),
                boot_task,
                refresh_task: deferred.refresh_task,
                ipc_task: tokio::sync::Mutex::new(ipc_task),
                client,
            }),
            engine_info: deferred.engine_info,
            deferred_state: Some(state_rx.clone()),
        };
        // Local, development, and already-resolved synced profiles need no
        // authentication UI while assembling. Keep the viewport Connecting
        // until their stores and journals are actually open, and surface a
        // boot failure through the existing bootstrap error path.
        if deferred.profile_is_resolved
            && let Err(message) = wait_for_deferred_engine(&mut state_rx).await
        {
            handle.shutdown().await;
            return Err(anyhow::anyhow!(message));
        }
        Ok(handle)
    }

    /// Whether to start the background engine rather than embed: launching
    /// is on, the port is a real one (port 0 has no address a background
    /// engine could be found at), and nothing else holds it — a stranger on
    /// the port would keep the engine from ever serving, so embed at once
    /// as before.
    async fn should_launch_background(launching: bool, ipc_port: u16) -> bool {
        if !launching || ipc_port == 0 {
            return false;
        }
        let taken = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::net::TcpStream::connect(("127.0.0.1", ipc_port)),
        )
        .await
        .is_ok_and(|connected| connected.is_ok());
        if taken {
            tracing::warn!(
                port = ipc_port,
                "the engine port is held by something else; embedding"
            );
        }
        !taken
    }

    /// Start the app's background engine and attach to it once it serves
    /// the port. `None` falls back to embedding: it couldn't start, exited,
    /// or didn't come up in time.
    async fn launch_background(config: &EngineConfig) -> Option<EngineHandle> {
        const STARTUP: std::time::Duration = std::time::Duration::from_secs(20);
        let app = std::env::current_exe().ok()?;
        let mut child = match zeron_engine::managed::launch(config, &app) {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(error = %err, "couldn't start the background engine; embedding one");
                return None;
            }
        };
        tracing::info!(pid = child.id(), "background engine started");
        let deadline = std::time::Instant::now() + STARTUP;
        loop {
            if let Some(handle) = Self::attach_to_daemon(config.ipc_port, &config.data_dir).await {
                zeron_engine::managed::reap(child);
                return Some(handle);
            }
            if let Ok(Some(status)) = child.try_wait() {
                tracing::warn!(%status, "background engine exited during startup; embedding one");
                return None;
            }
            if std::time::Instant::now() >= deadline {
                // Ours and never served: stop it, so it lets go of the data
                // directory the embedded engine is about to take.
                tracing::warn!("background engine didn't come up in time; embedding one");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }

    /// Probe the IPC port and, if a live engine answers, attach as a remote
    /// viewport. `None` means embed: nothing listening, a non-engine listener,
    /// a listener without an identity, or an engine whose IPC bearer
    /// (`{data_dir}/ipc-token`, read at dial time) this viewport doesn't hold.
    async fn attach_to_daemon(ipc_port: u16, data_dir: &std::path::Path) -> Option<EngineHandle> {
        let url = format!("ws://127.0.0.1:{ipc_port}");
        let probe = tokio::time::timeout(
            std::time::Duration::from_millis(750),
            tokio::net::TcpStream::connect(("127.0.0.1", ipc_port)),
        )
        .await;
        if !matches!(probe, Ok(Ok(_))) {
            return None;
        }
        tracing::info!(%url, "engine daemon detected; connecting");
        match zeron_rpc::connect_ipc(ipc_port, Some(data_dir)).await {
            Ok(client) => match query_engine_info(&client).await {
                Ok(engine_info) => {
                    let client = Arc::new(client);
                    let (state_tx, state_rx) =
                        tokio::sync::watch::channel(DeferredEngineState::Waiting);
                    let lifecycle_client = client.clone();
                    let lifecycle_task = tokio::spawn(async move {
                        let state = match lifecycle_client
                            .call(methods::ENGINE_READY, serde_json::json!({}))
                            .await
                        {
                            Ok(_) => DeferredEngineState::Ready,
                            // EngineReady was added after EngineInfo. An older daemon
                            // that does not expose the barrier is already assembled.
                            Err(RpcError::UnknownMethod(method))
                                if method == methods::ENGINE_READY =>
                            {
                                DeferredEngineState::Ready
                            }
                            Err(err) => DeferredEngineState::Failed(err.to_string()),
                        };
                        state_tx.send_replace(state);
                    });
                    Some(EngineHandle {
                        inner: Arc::new(RemoteEngine {
                            client,
                            url,
                            ipc_port,
                            data_dir: data_dir.to_path_buf(),
                            lifecycle_task: tokio::sync::Mutex::new(Some(lifecycle_task)),
                        }),
                        engine_info,
                        deferred_state: Some(state_rx),
                    })
                }
                Err(err) => {
                    tracing::warn!(
                        %url,
                        error = %err,
                        "listener did not provide engine identity; embedding instead"
                    );
                    None
                }
            },
            // Something is on the port but it is not an engine (or it is
            // wedged). Fall through and embed: a stranger holding 27654
            // should cost other viewports, not this window.
            Err(err) => {
                tracing::warn!(%url, error = %err, "not an engine; embedding instead");
                None
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_client(client: RpcClient) -> Self {
        Self {
            inner: Arc::new(RemoteEngine {
                client: Arc::new(client),
                url: "memory://test".into(),
                ipc_port: 0,
                data_dir: std::path::PathBuf::new(),
                lifecycle_task: tokio::sync::Mutex::new(None),
            }),
            engine_info: EngineInfo {
                device_id: "local".into(),
                workspace_scope: WorkspaceScope::Local,
                cursor_sdk_version: None,
                // What a current engine advertises.
                capabilities: zeron_proto::capabilities::CURRENT
                    .iter()
                    .map(|c| c.to_string())
                    .collect(),
            },
            deferred_state: None,
        }
    }

    pub fn client(&self) -> &RpcClient {
        self.inner.client()
    }

    pub fn mode(&self) -> EngineMode {
        self.inner.mode()
    }

    pub async fn media_client(&self) -> Result<RpcClient, RpcError> {
        self.inner.media_client().await
    }

    pub fn engine_info(&self) -> &EngineInfo {
        &self.engine_info
    }

    fn deferred_state(&self) -> Option<tokio::sync::watch::Receiver<DeferredEngineState>> {
        self.deferred_state.clone()
    }

    pub async fn shutdown(&self) {
        self.inner.shutdown().await;
    }

    /// Resolves when the connection to a separate engine is lost.
    pub async fn closed(&self) {
        self.inner.closed().await;
    }
}

/// Query the current protocol first, with a conservative fallback for daemons
/// from before `EngineInfo` existed. Old daemons are always treated as synced.
async fn query_engine_info(client: &RpcClient) -> Result<EngineInfo, RpcError> {
    match client
        .call_as(methods::ENGINE_INFO, serde_json::json!({}))
        .await
    {
        Ok(info) => Ok(info),
        Err(RpcError::UnknownMethod(method)) if method == methods::ENGINE_INFO => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct LocalDevice {
                device_id: String,
            }
            let legacy: LocalDevice = client
                .call_as(methods::LOCAL_DEVICE, serde_json::json!({}))
                .await?;
            Ok(EngineInfo {
                device_id: legacy.device_id,
                workspace_scope: WorkspaceScope::Synced,
                cursor_sdk_version: None,
                capabilities: Vec::new(),
            })
        }
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// Pure state + reducers
// ---------------------------------------------------------------------------

// The frontend-agnostic derivations (sort orders, staleness gating, sidebar
// grouping, the boot gate, relative times) live in `zeron_proto::view`, pure
// and with their own test suite. Re-exported here because every call site in
// this crate reads them as `state::…`.
pub use zeron_proto::view::{
    ChatGroup, ConnectionStatus, GatePhase, Indicator, SESSION_STALE_MS, attention_rank,
    chat_location, display_status, effective_indicator, format_time_ago, gate_phase, group_chats,
    parse_auth_state, project_label, running_subagents, sort_active, sort_chats, sort_spaces, sort_tabs,
};

/// What a session row shows that ticks with the clock rather than with its
/// content: the staleness-checked indicator and running-subagent count. A
/// change in either must repaint even when the row itself is untouched.
fn session_presence(session: &Session, now: DateTime<Utc>) -> (Indicator, u32) {
    (
        effective_indicator(Some(session), now),
        running_subagents(Some(session), now),
    )
}

// ---------------------------------------------------------------------------
// Org gate (pure)
// ---------------------------------------------------------------------------

/// One org membership row (tolerant local mirror of the engine's ListOrgs
/// reply — `{orgs: [{id, organizationId, name}]}`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRow {
    pub organization_id: String,
    pub name: String,
}

/// Parse a ListOrgs reply tolerantly (accepts a bare array too).
pub fn parse_orgs(value: &serde_json::Value) -> Vec<OrgRow> {
    let list = value.get("orgs").unwrap_or(value);
    serde_json::from_value(list.clone()).unwrap_or_default()
}

/// Workspace names must be non-empty (trimmed) and reasonably short.
pub fn org_name_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 64
}

/// Memberships sorted by name (case-insensitive), deduped by organization id.
pub fn sort_memberships(mut orgs: Vec<OrgRow>) -> Vec<OrgRow> {
    orgs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    orgs.dedup_by(|a, b| a.organization_id == b.organization_id);
    orgs
}

// ---------------------------------------------------------------------------
// AppState entity
// ---------------------------------------------------------------------------

/// A composer send whose doc command is queued but not yet executed by the
/// chat's host device — cleared when the host writes the user message back
/// into the transcript (same client-minted id as the [`AppState::echoes`]
/// dedup), or, for a chat that isn't open, when its session row shows the
/// turn that runs it; past [`UNDELIVERED_GRACE_MS`] it surfaces as the
/// explicit failed/retry state instead.
#[derive(Debug, Clone)]
struct PendingSend {
    message_id: String,
    started: DateTime<Utc>,
    /// The chat's session row when the send began; `None` if it had none.
    row_before: Option<Session>,
    /// The send creates the chat, so no earlier turn of it exists.
    new_chat: bool,
}

impl PendingSend {
    /// Whether `session`, the chat's row now, shows its host took this send.
    /// The transcript names the message but is watched for the selected chat
    /// only: a session left before its message landed read "Failed" 120 s
    /// later, mid-run (user report 2026-10-26). A row shows turns, not
    /// messages. A chat with no live run takes a send rather than queueing
    /// it, and its host writes the message, then starts a turn, so the next
    /// turn to start (or to end between two frames) is taken for the send's.
    /// A turn started there first, from another device or a session its host
    /// resumes after a restart, passes for it too.
    ///
    /// No earlier turn's result may count: it would ring the done chime a
    /// pending send keeps quiet (d3141ef0). Hence no stale row, no live row
    /// with a completion the send has not seen, no settled row when a turn
    /// was in flight at send time, no outcome older than the send, and no
    /// interrupted one, which a restart stamps on the turn it cut off. A
    /// restarted host, or a Cloud machine waking, comes back with no
    /// completion at all, which counts.
    fn adopted_by(&self, session: &Session, now: DateTime<Utc>) -> bool {
        use zeron_proto::SessionStatus::{AwaitingInput, Errored, Idle, Working};
        let before = self.row_before.as_ref();
        let settled_before = before.map_or(self.new_chat, |b| matches!(b.status, Idle | Errored));
        match session.status {
            Working | AwaitingInput => {
                let completed_before = before.and_then(|b| b.last_completed_turn.as_ref());
                !zeron_proto::view::session_stale(session, now)
                    && session.started_at.is_some()
                    && session.started_at != before.and_then(|b| b.started_at)
                    && (session.last_completed_turn.as_ref() == completed_before
                        || session.last_completed_turn.is_none() && settled_before)
            }
            Idle | Errored => {
                settled_before
                    && session.last_turn_outcome.is_some_and(|o| {
                        o.status != zeron_proto::DoneStatus::Interrupted
                            && o.finished_at > self.started
                    })
                    && session.last_turn_outcome != before.and_then(|b| b.last_turn_outcome)
            }
        }
    }
}

/// How long an unadopted send reads as Working/Sending (or Queued when the
/// path is degraded) before flipping to the EXPLICIT failed state with a
/// retry affordance. The old 30s overlay silently expired back to Idle with
/// no visible trace of the send at all — the exact hole the 2026-08-19
/// incident fell into.
pub const UNDELIVERED_GRACE_MS: i64 = 120_000;

/// A chat must sync this long before it says so: a quick open never flashes
/// a caption (the engine's degrade grace is the same 4 s).
pub const SYNC_SHOWN_AFTER_MS: i64 = 4_000;
/// A room still down this long into a sync has outlived a whole first dial
/// (20 s connect + 15 s hello deadline): from here a send waits for the
/// connection, it is no longer just a slow open.
pub const SYNC_STALLED_AFTER_MS: i64 = 35_000;

/// How the path from this device to a chat's host reads right now. One
/// derivation for the composer caption, the Queued badges, the pending-send
/// hold and the transcript loader, so they can never tell different stories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatLink {
    pub sync: LinkSync,
    /// The host's presence went dark. Cloud machines that sleep or wake
    /// answer on their own and never count ([`crate::cloud::Presence::warns`]).
    pub host_unreachable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSync {
    /// Delivery is proven, or there is nothing to prove: hosted here, a
    /// local profile, or a dormant chat on a healthy connection.
    Live,
    /// Opening or catching up with its room. `shown` once it has lasted
    /// [`SYNC_SHOWN_AFTER_MS`].
    Syncing {
        shown: bool,
    },
    /// Its room stayed down [`SYNC_STALLED_AFTER_MS`] into the sync.
    Stalled,
    /// The registry is down and this chat has no live room of its own, or
    /// an engine without per-chat sync states reports its room down.
    Reconnecting,
    Offline,
}

impl ChatLink {
    pub const LIVE: Self = Self {
        sync: LinkSync::Live,
        host_unreachable: false,
    };

    /// A send made now would wait on this path: "Queued", not "Sending".
    /// A syncing chat still delivers — pushes go out before catch-up ends.
    pub fn send_would_queue(self) -> bool {
        self.host_unreachable
            || matches!(
                self.sync,
                LinkSync::Stalled | LinkSync::Reconnecting | LinkSync::Offline
            )
    }

    /// Nothing proves a send can be delivered, or its adoption seen, yet:
    /// the pending-send overlay holds instead of expiring.
    pub fn delivery_unconfirmed(self) -> bool {
        self.host_unreachable || self.sync != LinkSync::Live
    }
}

/// Why a selected chat with nothing to show is not known to be empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptLoading {
    /// The engine has not replayed the document yet.
    Opening,
    /// Replayed, but this copy of a remote chat may be behind its room.
    Syncing,
    Reconnecting,
    Offline,
}

/// The engine is still bringing this chat's room up to date. Delivery proven
/// over the room or HTTP ends it; so does any state that is not a sync.
fn chat_syncing(chat: &zeron_proto::ChatConnectivity) -> bool {
    use zeron_proto::ChatSyncState as S;
    !chat.delivery_live && matches!(chat.sync_state, S::Waiting | S::Connecting | S::Offline)
}

/// How long one chat has been syncing, as this view saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncClock {
    /// The caption's hold and the stall both count from here: when the sync
    /// began, or when the chat was opened during it, so a sync already
    /// under way in the background never flashes a caption, or reads as
    /// stalled, on a chat just opened. Nothing else restarts it: a room the
    /// engine moves back to waiting for a slot keeps its clock, so a sync
    /// that never delivers always ends up stalled instead of spinning.
    began: DateTime<Utc>,
}

/// The Cloud setup placeholder's step while the machine comes up; the
/// machine's own first step ("Started a machine") replaces it in place.
const CLOUD_STARTING_STEP: &str = "Starting a machine";
const CLOUD_WAKING_STEP: &str = "Waking the machine";
/// The setup step once Cloud refused to start or wake the machine for lack
/// of credits. The message stays queued: credits added, the machine starts
/// and runs it.
const CLOUD_NO_CREDITS_STEP: &str = "Out of Cloud credits";

/// How often Cloud's status (credits above all) is re-read while attached,
/// and how stale it may be when a window comes forward.
const CLOUD_STATUS_POLL: std::time::Duration = std::time::Duration::from_secs(120);
const CLOUD_STATUS_FOCUS_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);
/// A Cloud read that failed is tried again this soon (unknown credits let
/// sends through).
const CLOUD_STATUS_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// The one-shot timer behind [`AppState::watch_clock_transitions`].
#[derive(Default)]
struct ClockTransitions {
    /// Re-arms the timer after every notify.
    observation: Option<gpui::Subscription>,
    /// The last time observers saw current state (a notify or a wake).
    checked_at: Option<DateTime<Utc>>,
    wake_at: Option<DateTime<Utc>>,
    task: Option<Task<()>>,
}

#[cfg(test)]
thread_local! {
    static TEST_CLOCK: std::cell::Cell<Option<DateTime<Utc>>> = const { std::cell::Cell::new(None) };
}

/// Wall-clock time for clock-driven state derivations. Tests pin it to the
/// simulated executor's timeline with [`TestClock`].
pub(crate) fn clock_now() -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(now) = TEST_CLOCK.with(std::cell::Cell::get) {
        return now;
    }
    Utc::now()
}

/// Pins [`clock_now`] on this test thread until dropped.
#[cfg(test)]
pub(crate) struct TestClock;

#[cfg(test)]
impl TestClock {
    pub(crate) fn start(now: DateTime<Utc>) -> Self {
        let clock = Self;
        clock.set(now);
        clock
    }

    pub(crate) fn set(&self, now: DateTime<Utc>) {
        TEST_CLOCK.with(|clock| clock.set(Some(now)));
    }
}

#[cfg(test)]
impl Drop for TestClock {
    fn drop(&mut self) {
        TEST_CLOCK.with(|clock| clock.set(None));
    }
}

/// A send's attachment-upload leg in flight. `done` is bumped by the upload
/// task per completed chunk (binary bytes); the working label reads it every
/// paint (the spinner already animates each frame), so no notify plumbing is
/// needed. A slow upload renders as "Uploading… N%" instead of a
/// hang-indistinguishable "Sending…" (2026-08-18 user report).
pub struct UploadProgress {
    done: std::sync::Arc<std::sync::atomic::AtomicU64>,
    total: u64,
}

pub(crate) const CANVAS_PANEL_PREFIX: &str = "space-canvas:";

/// Per-space key for new-session-canvas chrome (terminal tabs, panel flags).
pub fn canvas_panel_key(space_id: Option<&str>) -> String {
    format!("{CANVAS_PANEL_PREFIX}{}", space_id.unwrap_or(""))
}

/// The new-session canvas's project/device pick, see [`AppState::canvas_target`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanvasTarget {
    space: Option<String>,
    no_project: bool,
    device: Option<String>,
}

/// Root application state. Reducer methods (`apply_*`, [`Self::session_for`], …)
/// are plain `&mut self` functions so tests construct the struct directly; gpui
/// glue ([`Self::bootstrap`], [`Self::select_chat`]) layers subscriptions on top.
/// How long a requested wake reads as waking before the machine has to
/// answer for itself: a cold start is well under this.
const CLOUD_WAKE_GRACE: std::time::Duration = std::time::Duration::from_secs(180);

/// The least time between session-list re-reads prompted by a surface that
/// couldn't reach its Cloud machine (it retries every couple of seconds).
const CLOUD_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(8);

pub struct AppState {
    pub connection: ConnectionStatus,
    /// Fixed data boundary of the attached engine. Authentication may change
    /// in place, but changing this scope requires assembling a new runtime.
    pub workspace_scope: Option<WorkspaceScope>,
    /// Auth stream value; `None` until the engine reports one (M4).
    pub auth: Option<AuthState>,
    pub devices: Vec<Device>,
    // Last published device presentation. Heartbeats refresh the underlying
    // timestamps without invalidating every view when no displayed value changed.
    device_presentation: Option<Vec<(Device, bool, String)>>,
    // Presence ticks also retire stale remote session indicators. Remember
    // their last published appearance even when the device rows stay online.
    session_presence_presentation: Vec<(Indicator, u32)>,
    /// Live edge posture (WatchConnectivity): drives the connection pill,
    /// composer honesty ("will queue"), and the Queued send badges.
    pub connectivity: zeron_proto::Connectivity,
    /// Whether this runtime's connectivity watch has delivered its first
    /// frame. The default `Disabled` value is only a placeholder and must not
    /// seed notification decisions before the watch is authoritative.
    pub(crate) connectivity_observed: bool,
    /// Each syncing chat's [`SyncClock`] (see [`chat_syncing`]): its
    /// [`LinkSync::Syncing`] caption and [`LinkSync::Stalled`] are timed
    /// here, not from the engine's 4 s room grace.
    sync_clocks: HashMap<String, SyncClock>,
    /// Sorted (see [`sort_spaces`]).
    pub spaces: Vec<Space>,
    /// Sorted (see [`sort_chats`]); includes archived rows — views filter.
    pub chats: Vec<Chat>,
    /// Fork RPC may arrive ahead of its registry row on a remote device.
    pending_side_chat: Option<Chat>,
    /// `pending_side_chat` was started by hand and nothing has minted it:
    /// no registry row, no doc, until its first send.
    unsaved_side_chat: bool,
    pub sessions: Vec<Session>,
    /// Synced user/org sidebar pin state. Local workspaces deliberately ignore
    /// this and continue reading their device-local settings entry.
    pub sidebar_preferences: SidebarPreferencesState,
    session_presentation: Option<Vec<Session>>,
    /// The project the new-session canvas mints into. Healed by
    /// [`Self::apply_spaces`] when the row vanishes; selecting a chat implies
    /// its project.
    pub selected_space: Option<String>,
    /// Deliberate "Don't work in a project" pick: while set, the canvas mints
    /// project-less sessions (cwd `~` on the picked device) and
    /// [`Self::selected_space_row`] reads as `None` — healing must NOT
    /// re-select a project underneath it.
    pub no_project: bool,
    /// The composer's device pick — where project-less sessions run, and the
    /// device whose projects the project picker lists. `None` falls back to
    /// the local device.
    pub selected_device: Option<String>,
    /// The new-session canvas's own target, set aside while a chat is open
    /// (a chat implies ITS project/device) and put back on return, so the
    /// canvas always reopens where the user left it.
    pub(crate) canvas_target: Option<CanvasTarget>,
    pub selected_chat: Option<String>,
    /// Boot auto-select happened (or a manual selection superseded it).
    pub auto_selected: bool,
    /// First chats / spaces watch frame has landed — device-local state that
    /// prunes against the doc (open tabs, the sidebar space filter) must not
    /// judge by the empty pre-sync lists.
    pub chats_synced: bool,
    pub spaces_synced: bool,
    pending_deep_link: Option<crate::links::ConversationDeepLink>,
    deep_link_notice: Option<String>,
    /// Joined transcript of the selected chat (continuations folded engine-side).
    pub transcript: Vec<SessionMessageEntry>,
    /// The selected chat's pending-message queue — what was typed while the
    /// agent was busy, in the order it will be sent. Device-agnostic: the
    /// chat's doc holds them (every device sees the same queue).
    pub queue: Vec<zeron_doc::QueuedMessage>,
    pub context_usage: Option<zeron_proto::ContextUsage>,
    /// The selected chat has a transcript from a `WatchDocMessages` reset
    /// (including a retained reset from an earlier visit). An
    /// empty transcript is otherwise indistinguishable from the pre-replay
    /// gap after selection, where optimistic echoes may already be visible.
    pub transcript_replayed: bool,
    transcript_baselines: HashMap<String, Arc<zeron_doc::TranscriptBaseline>>,
    transcript_cache: std::collections::VecDeque<CachedTranscript>,
    pub(crate) prepared_transcripts: HashMap<String, Arc<crate::transcript::PreparedTranscript>>,
    /// Changes only when transcript/optimistic content changes. Presence,
    /// catalogs and other app-state notifications need no row derivation.
    pub(crate) transcript_revision: u64,
    /// Changes whenever an input of [`Self::file_link_roots`] does — chat
    /// rows, projects, this device's id — so views can memoize the roots.
    pub(crate) link_roots_revision: u64,
    /// Optimistic user echoes per chat id, shown until the doc frame carrying
    /// the same message id arrives (client-minted ids make dedup exact).
    echoes: HashMap<String, Vec<SessionMessageEntry>>,
    /// Send-in-flight overlay per chat id: a queued doc command the host
    /// hasn't executed yet (see [`Self::begin_pending_send`]).
    pending_sends: HashMap<String, PendingSend>,
    /// The in-flight send's attachment upload, when it has one.
    upload_progress: Option<UploadProgress>,
    /// Engine-side queued-attachment transfers by uploadId (`WatchTransfers`
    /// snapshots): real relay-leg progress for the sending thumbnail's
    /// percent ring, present exactly while bytes are moving.
    transfers: HashMap<String, (u64, u64)>,
    /// Written by the changes pane, read by the composer.
    review_comments: HashMap<String, Vec<ReviewComment>>,
    /// File surfaces whose editor-backed comments currently cite a buffer
    /// revision that has not reached disk yet. A chat remains blocked until
    /// every surface waiting on a workspace write has finished or cancelled.
    review_comment_flushes: HashMap<String, HashSet<u64>>,
    /// This engine's device id (best-effort `LocalDevice` probe; `None` until
    /// the engine serves it — views degrade gracefully).
    pub local_device_id: Option<String>,
    /// Device-local agent CLI update lifecycle. Unlike `ListHarnesses`, this
    /// standing stream may be backed by subprocess and network probes.
    pub harness_updates: Vec<zeron_proto::HarnessUpdateStatus>,
    /// Last `CloudStatus` any surface fetched (Settings → Cloud, Devices,
    /// New project). Fetched on open and after actions — never polled — so
    /// device lists can say "Asleep" instead of "Offline" for Cloud.
    pub cloud_status: Option<zeron_proto::CloudStatus>,
    /// Every Cloud session and its machine's state, read on focus, chat
    /// selection and page open — and every few seconds only while one is
    /// being set up, woken or deleted ([`Self::refresh_cloud_sessions`]).
    pub cloud_sessions: Option<zeron_proto::CloudSessions>,
    cloud_sessions_task: Option<Task<()>>,
    /// Sessions (by their chat id) someone asked to wake, and when: the
    /// machine reads as waking until its own heartbeat arrives, rather than
    /// asleep again while the session list catches up.
    cloud_wakes: HashMap<String, std::time::Instant>,
    /// When a surface that couldn't reach a Cloud machine last had the
    /// session list re-read ([`Self::recheck_cloud_machine`]).
    cloud_recheck_at: Option<std::time::Instant>,
    /// The GitHub repositories Cloud reaches (the Zeron GitHub App's
    /// installations): a project whose repository is here can run a session
    /// on Cloud. `None` = Cloud off, GitHub not connected, or not read yet.
    pub cloud_repos: Option<Vec<zeron_proto::GithubRepo>>,
    cloud_repos_task: Option<Task<()>>,
    /// A [`Self::refresh_cloud_repos`] read is in flight (its button spins).
    cloud_repos_refreshing: bool,
    /// Data directory (`ui-settings.json`, `composer-defaults.json`); set at
    /// bootstrap so child views can persist small preference files.
    pub data_dir: Option<PathBuf>,
    /// Notifies at clock-driven transitions (see [`Self::watch_clock_transitions`]).
    clock: ClockTransitions,
    engine: Option<EngineHandle>,
    watch_tasks: Vec<Task<()>>,
    /// How the current engine was reached, kept to reconnect with.
    boot_config: Option<EngineBootConfig>,
    /// Watches the engine connection; a separate engine going away (crash,
    /// restart into an update, sign-out elsewhere) reconnects — starting it
    /// again when nothing serves the port.
    connection_task: Option<Task<()>>,
    /// The next engine exit is ours (sign-out, a workspace switch): the shell
    /// replaces the runtime itself.
    engine_exit_expected: bool,
    /// When Cloud's status was last read, and the reads under way.
    cloud_status_read_at: Option<std::time::Instant>,
    cloud_status_task: Option<Task<()>>,
    cloud_recheck_task: Option<Task<()>>,
    /// When separate engines went away unasked, recently (respawn backoff).
    engine_losses: Vec<std::time::Instant>,
    /// The pending respawn after a loss.
    reconnect_task: Option<Task<()>>,
    /// This window's id to the engine, and whether one of the app's windows
    /// is focused — the engine holds desktop banners while one is.
    viewport_id: String,
    viewport_focused: bool,
    focus_task: Option<Task<()>>,
    transcript_task: Option<Task<()>>,
    change_requests: ChangeRequestClientState,
    change_request_tasks: HashMap<ChangeRequestWatchKey, Task<()>>,
    queue_task: Option<Task<()>>,
    change_requests_visible: bool,
    /// SUBAGENT transcripts keyed by subagent doc id (the right pane's
    /// subagent tabs read these). Independent of `selected_chat`: a tab's
    /// feed must survive chat switches — the tab itself is what scopes it.
    sub_transcripts: HashMap<String, Vec<SessionMessageEntry>>,
    /// One watch task per live subagent doc (single-flight per key).
    /// Dropping a task cancels the engine-side watch and unpins the doc from
    /// the engine LRU — closing a tab MUST go through
    /// [`Self::unwatch_subagent_doc`].
    sub_watch_tasks: HashMap<String, Task<()>>,
}

/// Text/reasoning growth changes the transcript without changing session
/// status, tools, navigation, or composer state. Keep those frames local to
/// the affected transcript instead of rebuilding every app-state observer.
pub(crate) struct TranscriptTextChanged {
    pub doc_id: String,
}

impl gpui::EventEmitter<TranscriptTextChanged> for AppState {}

fn is_text_append(frame: &TranscriptFrame) -> bool {
    matches!(frame, TranscriptFrame::Delta { upsert, append, remove, .. }
        if upsert.is_empty() && remove.is_empty() && !append.is_empty())
}

/// Each session's presence at `now` (the lease-dependent part of a frame).
fn session_presence_at(sessions: &[Session], now: DateTime<Utc>) -> Vec<(Indicator, u32)> {
    sessions
        .iter()
        .map(|session| session_presence(session, now))
        .collect()
}

/// Device rows as rendered at `now`: the heartbeat timestamp only through its
/// online state and last-seen label.
fn device_presentation_at(devices: &[Device], now: DateTime<Utc>) -> Vec<(Device, bool, String)> {
    devices
        .iter()
        .map(|device| {
            let mut metadata = device.clone();
            metadata.last_seen_at = None;
            (
                metadata,
                crate::settings::devices::device_online(device.last_seen_at, now),
                crate::settings::devices::format_last_seen(device.last_seen_at, now),
            )
        })
        .collect()
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connection: ConnectionStatus::Connecting,
            workspace_scope: None,
            auth: None,
            devices: Vec::new(),
            device_presentation: None,
            session_presence_presentation: Vec::new(),
            connectivity: zeron_proto::Connectivity::default(),
            connectivity_observed: false,
            sync_clocks: HashMap::new(),
            spaces: Vec::new(),
            chats: Vec::new(),
            pending_side_chat: None,
            unsaved_side_chat: false,
            sessions: Vec::new(),
            sidebar_preferences: SidebarPreferencesState::default(),
            session_presentation: None,
            selected_space: None,
            no_project: false,
            selected_device: None,
            canvas_target: None,
            selected_chat: None,
            transcript: Vec::new(),
            queue: Vec::new(),
            context_usage: None,
            transcript_replayed: false,
            transcript_baselines: HashMap::new(),
            transcript_cache: Default::default(),
            prepared_transcripts: HashMap::new(),
            transcript_revision: 0,
            link_roots_revision: 0,
            echoes: HashMap::new(),
            pending_sends: HashMap::new(),
            upload_progress: None,
            transfers: HashMap::new(),
            review_comments: HashMap::new(),
            review_comment_flushes: HashMap::new(),
            local_device_id: None,
            harness_updates: Vec::new(),
            cloud_status: None,
            cloud_sessions: None,
            cloud_sessions_task: None,
            cloud_wakes: HashMap::new(),
            cloud_recheck_at: None,
            cloud_repos: None,
            cloud_repos_task: None,
            cloud_repos_refreshing: false,
            data_dir: None,
            clock: ClockTransitions::default(),
            engine: None,
            watch_tasks: Vec::new(),
            boot_config: None,
            connection_task: None,
            engine_exit_expected: false,
            cloud_status_read_at: None,
            cloud_status_task: None,
            cloud_recheck_task: None,
            engine_losses: Vec::new(),
            reconnect_task: None,
            viewport_id: uuid::Uuid::new_v4().to_string(),
            viewport_focused: false,
            focus_task: None,
            transcript_task: None,
            change_requests: ChangeRequestClientState::default(),
            change_request_tasks: HashMap::new(),
            queue_task: None,
            change_requests_visible: true,
            sub_transcripts: HashMap::new(),
            sub_watch_tasks: HashMap::new(),
            auto_selected: false,
            chats_synced: false,
            spaces_synced: false,
            pending_deep_link: None,
            deep_link_notice: None,
        }
    }

    /// The selected chat, or `""` on the new-chat canvas. Identical to the
    /// composer's own attachment/draft key, so a comment written before the
    /// first send survives the chat being minted.
    pub fn composer_key(&self) -> String {
        self.selected_chat.clone().unwrap_or_default()
    }

    /// Per-session chrome key (terminal tabs, panel open flags). Real chats
    /// use the chat id; the new-session canvas is per-space so two projects
    /// don't share one drawer.
    pub fn panel_session_key(&self) -> String {
        match self.selected_chat.as_deref() {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => canvas_panel_key(self.selected_space.as_deref()),
        }
    }

    /// Optional `OpenTerminal` cwd. Existing chats leave this unset so the
    /// engine reads the chat row. The canvas has no row yet — pass the
    /// selected project's folder. When the project id is known but the
    /// WatchSpaces row has not landed, leave cwd unset: `chatId` is
    /// `space-canvas:{spaceId}` and the engine resolves the folder. Only
    /// a deliberate project-less canvas sends `~`.
    pub fn terminal_open_cwd(&self) -> Option<String> {
        self.terminal_open_cwd_for(&self.panel_session_key())
    }

    /// [`Self::terminal_open_cwd`] for a specific panel key (the tab being
    /// opened, which matches the selected session).
    pub fn terminal_open_cwd_for(&self, session_key: &str) -> Option<String> {
        let Some(space_id) = session_key.strip_prefix(CANVAS_PANEL_PREFIX) else {
            return None;
        };
        if space_id.is_empty() || self.no_project {
            return Some("~".to_string());
        }
        self.spaces
            .iter()
            .find(|space| space.id == space_id)
            .map(|space| space.path.clone())
            .filter(|path| !path.trim().is_empty())
    }

    /// Device to address terminal RPCs at, when it isn't this engine.
    /// Canvas keys (`space-canvas:{space}`) resolve through the space row
    /// (or the project-less device pick).
    pub fn terminal_target_device(&self, session_key: &str) -> Option<String> {
        let device = if let Some(space_id) = session_key.strip_prefix(CANVAS_PANEL_PREFIX) {
            if space_id.is_empty() {
                self.selected_device
                    .clone()
                    .or_else(|| self.local_device_id.clone())?
            } else {
                self.spaces
                    .iter()
                    .find(|space| space.id == space_id)?
                    .device_id
                    .clone()
            }
        } else {
            self.chats
                .iter()
                .find(|chat| chat.id == session_key)?
                .device_id
                .clone()
        };
        (self.local_device_id.as_deref() != Some(device.as_str())).then_some(device)
    }

    pub fn review_comments(&self, key: &str) -> &[ReviewComment] {
        self.review_comments
            .get(key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn add_review_comment(&mut self, key: &str, comment: ReviewComment) {
        self.review_comments
            .entry(key.to_string())
            .or_default()
            .push(comment);
    }

    pub fn remove_review_comment(&mut self, key: &str, id: &str) {
        if let Some(list) = self.review_comments.get_mut(key) {
            list.retain(|c| c.id != id);
            if list.is_empty() {
                self.review_comments.remove(key);
                self.review_comment_flushes.remove(key);
            }
        }
    }

    /// Update only a staged comment's body. A stale editor must never recreate
    /// a comment that has already been removed or sent to the agent.
    pub fn update_review_comment_body(&mut self, key: &str, id: &str, body: String) {
        if let Some(comment) = self
            .review_comments
            .get_mut(key)
            .and_then(|comments| comments.iter_mut().find(|comment| comment.id == id))
        {
            comment.body = body;
        }
    }

    pub fn update_review_comment_line(&mut self, key: &str, id: &str, line: u32) {
        if let Some(comment) = self
            .review_comments
            .get_mut(key)
            .and_then(|comments| comments.iter_mut().find(|comment| comment.id == id))
        {
            comment.line = line;
        }
    }

    pub fn rename_review_comment_path(&mut self, key: &str, old_path: &str, new_path: &str) {
        if let Some(comments) = self.review_comments.get_mut(key) {
            for comment in comments
                .iter_mut()
                .filter(|comment| comment.is_file() && comment.path == old_path)
            {
                comment.path = new_path.to_string();
            }
        }
    }

    pub fn take_review_comments(&mut self, key: &str) -> Vec<ReviewComment> {
        self.review_comment_flushes.remove(key);
        self.review_comments.remove(key).unwrap_or_default()
    }

    pub fn purge_review_comments(&mut self, key: &str) {
        self.review_comment_flushes.remove(key);
        self.review_comments.remove(key);
    }

    pub fn begin_review_comment_flush(&mut self, key: &str, source: u64) {
        self.review_comment_flushes
            .entry(key.to_string())
            .or_default()
            .insert(source);
    }

    pub fn finish_review_comment_flush(&mut self, key: &str, source: u64) {
        let remove_key = self
            .review_comment_flushes
            .get_mut(key)
            .is_some_and(|sources| {
                sources.remove(&source);
                sources.is_empty()
            });
        if remove_key {
            self.review_comment_flushes.remove(key);
        }
    }

    pub fn review_comment_flush_pending(&self, key: &str) -> bool {
        self.review_comment_flushes.contains_key(key)
    }

    // ---- reducers (pure) ----

    pub(crate) fn apply_sidebar_preferences(&mut self, value: SidebarPreferencesState) -> bool {
        if value.revision < self.sidebar_preferences.revision || value == self.sidebar_preferences {
            return false;
        }
        self.sidebar_preferences = value;
        true
    }

    /// Returns whether anything observable changed. The engine re-publishes
    /// identical lists (presence and sync churn elsewhere in the doc), and
    /// each notify re-renders the whole shell.
    pub fn apply_chats(&mut self, mut chats: Vec<Chat>) -> bool {
        let mut changed = false;
        if let Some(pending) = &self.pending_side_chat {
            if chats.iter().any(|chat| chat.id == pending.id) {
                self.pending_side_chat = None;
                changed = true;
            } else {
                chats.push(pending.clone());
            }
        }
        sort_chats(&mut chats);
        if !self.chats_synced || self.chats != chats {
            changed = true;
            self.chats = chats;
            self.link_roots_revision = self.link_roots_revision.wrapping_add(1);
            self.chats_synced = true;
            self.transcript_cache
                .retain(|cached| self.chats.iter().any(|c| c.id == cached.chat_id));
        }
        if let Some(selected) = &self.selected_chat
            && !self.chats.iter().any(|c| &c.id == selected)
        {
            changed = true;
            // Selected chat vanished (deleted elsewhere): drop selection + transcript.
            self.transcript_baselines.remove(selected);
            self.prepared_transcripts.remove(selected);
            self.selected_chat = None;
            self.restore_canvas_target();
            self.transcript.clear();
            self.context_usage = None;
            self.transcript_revision = self.transcript_revision.wrapping_add(1);
            self.transcript_replayed = false;
            self.transcript_task = None;
            self.queue.clear();
            self.queue_task = None;
        }
        changed
    }

    pub fn apply_sessions(&mut self, sessions: Vec<Session>) -> bool {
        self.apply_sessions_at(sessions, Utc::now())
    }

    fn apply_sessions_at(&mut self, sessions: Vec<Session>, now: DateTime<Utc>) -> bool {
        let presence = session_presence_at(&sessions, now);
        let presentation: Vec<_> = sessions
            .iter()
            .map(|session| {
                let mut metadata = session.clone();
                // The timestamp is a liveness lease, not visible text. Keep its
                // effective indicator in the key and retain the actual value below.
                metadata.updated_at = DateTime::<Utc>::UNIX_EPOCH;
                metadata
            })
            .collect();
        let changed = self.session_presentation.as_ref() != Some(&presentation)
            || self.session_presence_presentation != presence;
        self.session_presentation = Some(presentation);
        self.session_presence_presentation = presence;
        self.sessions = sessions;
        let adopted = self.adopt_pending_sends(now);
        changed || adopted
    }

    /// Clears the pending send of each chat but the open one whose row shows
    /// the turn that runs it ([`PendingSend::adopted_by`]); the open chat's
    /// transcript names the message, so only it acks there. Returns whether
    /// one was cleared.
    fn adopt_pending_sends(&mut self, now: DateTime<Utc>) -> bool {
        let pending = self.pending_sends.len();
        let selected = self.selected_chat.as_deref();
        let sessions = &self.sessions;
        self.pending_sends.retain(|chat_id, send| {
            selected == Some(chat_id.as_str())
                || !sessions
                    .iter()
                    .find(|s| s.chat_id == *chat_id)
                    .is_some_and(|s| send.adopted_by(s, now))
        });
        self.pending_sends.len() != pending
    }

    /// Returns whether the list or the healed selection changed.
    pub fn apply_spaces(&mut self, mut spaces: Vec<Space>) -> bool {
        sort_spaces(&mut spaces);
        let previous_selection = self.selected_space.clone();
        let mut changed = false;
        if !self.spaces_synced || self.spaces != spaces {
            changed = true;
            self.spaces = spaces;
            self.link_roots_revision = self.link_roots_revision.wrapping_add(1);
            self.spaces_synced = true;
        }
        self.heal_space_selection();
        changed || self.selected_space != previous_selection
    }

    fn heal_space_selection(&mut self) {
        if self.no_project {
            self.selected_space = None;
            return;
        }
        // Heal a vanished selection (project deleted elsewhere): fall back to
        // the first project; its chats died with it, so a matching chat
        // selection is healed by the accompanying chats frame (`apply_chats`).
        // The picker lists projects per-device, so healing prefers one on the
        // picked device — a global fallback would silently re-aim the canvas
        // at another machine.
        if let Some(selected) = &self.selected_space
            && !self.spaces.iter().any(|s| &s.id == selected)
        {
            self.selected_space = self.first_space_on_picked_device();
        }
        // First frame with no selection yet: pick the first project so the
        // canvas never boots project-less by accident — unless the user
        // deliberately opted out.
        if self.selected_space.is_none() && !self.no_project {
            self.selected_space = self.first_space_on_picked_device();
        }
    }

    /// The ordered checkouts a file link from `chat_id` may resolve against:
    /// the chat's own checkout, its parent chat's, then every project root on
    /// the linking chat's device — an agent's paths name its own disk, so a
    /// root on another device never owns them. Project roots have no owning
    /// chat — a link that matches one opens by absolute path through the
    /// linking chat. `local` marks roots whose files sit on this disk; a
    /// remote chat's worktree still resolves links (reads go through its own
    /// device) but cannot offer local file actions.
    pub(crate) fn file_link_roots(
        &self,
        chat_id: &str,
    ) -> Vec<crate::workspace_links::FileLinkRoot> {
        use crate::workspace_links::FileLinkRoot;
        // An unprobed device id means the chats on screen are the local
        // engine's — everything shown counts as local until sync says else.
        let on_this_device = |device: &str| {
            self.local_device_id
                .as_deref()
                .is_none_or(|local| device == local)
        };
        let mut roots: Vec<FileLinkRoot> = Vec::new();
        let mut push = |chat: Option<&Chat>, device: &str, root: Option<&str>| {
            let Some(root) = root.filter(|root| !root.is_empty()) else {
                return;
            };
            if roots.iter().any(|existing| existing.root == root) {
                return;
            }
            roots.push(FileLinkRoot {
                chat: chat.map(|chat| chat.id.clone()),
                local: on_this_device(device),
                root: root.to_owned(),
            });
        };
        let chat_row = |id: &str| self.chats.iter().find(|chat| chat.id == id);
        let linking = chat_row(chat_id);
        // Roots on the linking chat's device; an unknown chat row falls back
        // to this device's.
        let on_link_device = |device: &str| match linking {
            Some(chat) => chat.device_id == device,
            None => on_this_device(device),
        };
        if let Some(chat) = linking {
            push(Some(chat), &chat.device_id, chat.cwd.as_deref());
        }
        if let Some(parent) = linking
            .and_then(|chat| chat.parent_chat_id.as_deref())
            .and_then(chat_row)
            .filter(|parent| on_link_device(&parent.device_id))
        {
            push(Some(parent), &parent.device_id, parent.cwd.as_deref());
        }
        for space in self
            .spaces
            .iter()
            .filter(|space| on_link_device(&space.device_id))
        {
            push(None, &space.device_id, Some(space.path.as_str()));
        }
        roots
    }

    /// Whether `chat_id` lives on this device — an outside file link from it
    /// resolves to this disk, so its menu may offer local file actions. An
    /// unprobed device id (or an unknown chat row) counts as local, matching
    /// `file_link_roots`.
    pub(crate) fn chat_is_local(&self, chat_id: &str) -> bool {
        self.chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_none_or(|chat| {
                self.local_device_id
                    .as_deref()
                    .is_none_or(|local| chat.device_id == local)
            })
    }

    /// Optimistic local echo of a `setChatConfig` mutate: stamp the row now so
    /// the chips update on click; the next chats watch frame carries the same
    /// value once the engine applies the LWW write.
    pub fn apply_chat_config(&mut self, chat_id: &str, config: zeron_proto::ChatConfig) {
        // The pending side chat's copy is what `apply_chats` re-inserts and
        // what an unsaved one is minted from.
        if let Some(chat) = self
            .pending_side_chat
            .as_mut()
            .filter(|chat| chat.id == chat_id)
        {
            chat.config = Some(config.clone());
        }
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.config = Some(config);
        }
    }

    /// Returns whether the posture changed. The first frame after attach always
    /// counts: it ends the bootstrap gap the shell's alert baseline waits on.
    pub fn apply_connectivity(&mut self, connectivity: zeron_proto::Connectivity) -> bool {
        self.apply_connectivity_at(connectivity, clock_now())
    }

    fn apply_connectivity_at(
        &mut self,
        connectivity: zeron_proto::Connectivity,
        now: DateTime<Utc>,
    ) -> bool {
        let changed = !self.connectivity_observed || self.connectivity != connectivity;
        // A new engine's first frame starts every clock afresh: the syncs
        // the last one ran say nothing about this one's.
        if !self.connectivity_observed {
            self.sync_clocks.clear();
        }
        self.sync_clocks.retain(|id, _| {
            connectivity
                .chats
                .iter()
                .any(|chat| chat.chat_id == *id && chat_syncing(chat))
        });
        for chat in connectivity.chats.iter().filter(|chat| chat_syncing(chat)) {
            self.sync_clocks
                .entry(chat.chat_id.clone())
                .or_insert(SyncClock { began: now });
        }
        self.connectivity = connectivity;
        self.connectivity_observed = true;
        changed
    }

    /// The path to `chat_id`'s host at `now` (see [`ChatLink`]). A chat
    /// hosted here never waits: its queued command executes on this device
    /// even fully offline. Opening a remote chat is [`LinkSync::Syncing`] —
    /// an outage is claimed only once its room has stalled.
    pub fn chat_link_at(&self, chat_id: &str, now: DateTime<Utc>) -> ChatLink {
        use zeron_proto::{ChatSyncState as C, ConnectivityState as S};
        let global = match self.connectivity.state {
            S::Disabled => return ChatLink::LIVE,
            S::Offline => Some(LinkSync::Offline),
            S::Reconnecting => Some(LinkSync::Reconnecting),
            S::Connected => None,
        };
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            // Unknown chat (a just-minted canvas send): only the global
            // state can speak.
            return ChatLink {
                sync: global.unwrap_or(LinkSync::Live),
                host_unreachable: false,
            };
        };
        if Some(chat.device_id.as_str()) == self.local_device_id.as_deref() {
            return ChatLink::LIVE;
        }
        let net = self
            .connectivity
            .chats
            .iter()
            .find(|c| c.chat_id == chat_id);
        let sync = match (global, net) {
            (Some(LinkSync::Offline), _) => LinkSync::Offline,
            // The registry can be down while this chat's room or HTTP
            // fallback still delivers.
            (_, Some(net)) if net.delivery_live => LinkSync::Live,
            // A dormant or unknown chat has no such proof.
            (Some(global), _) => global,
            // Dormant by design while the connection is healthy.
            (None, None) => LinkSync::Live,
            (None, Some(net)) => match net.sync_state {
                C::Local | C::Synced | C::StorageError => LinkSync::Live,
                // Engines before per-chat sync states report only the
                // graced room bit.
                C::Unknown if net.connected => LinkSync::Live,
                C::Unknown => LinkSync::Reconnecting,
                C::Waiting | C::Connecting | C::Offline => {
                    let lasted = |ms: i64| {
                        self.sync_clocks
                            .get(chat_id)
                            .is_some_and(|clock| (now - clock.began).num_milliseconds() >= ms)
                    };
                    // A joined room catching up a long backlog never stalls.
                    if !net.connected && lasted(SYNC_STALLED_AFTER_MS) {
                        LinkSync::Stalled
                    } else {
                        LinkSync::Syncing {
                            shown: lasted(SYNC_SHOWN_AFTER_MS),
                        }
                    }
                }
            },
        };
        ChatLink {
            sync,
            host_unreachable: self.device_presence(&chat.device_id, now).warns(),
        }
    }

    /// The path to the device a new chat would run on (the canvas has no
    /// chat row yet): the global posture and that device's presence.
    pub fn device_link_at(&self, device_id: &str, now: DateTime<Utc>) -> ChatLink {
        use zeron_proto::ConnectivityState as S;
        if self.connectivity.state == S::Disabled
            || self.local_device_id.as_deref() == Some(device_id)
        {
            return ChatLink::LIVE;
        }
        ChatLink {
            sync: match self.connectivity.state {
                S::Offline => LinkSync::Offline,
                S::Reconnecting => LinkSync::Reconnecting,
                S::Disabled | S::Connected => LinkSync::Live,
            },
            host_unreachable: self.device_presence(device_id, now).warns(),
        }
    }

    /// A send is queued: in flight AND its path would hold it — the honest
    /// badge is "Queued", not a Working spinner.
    pub fn send_queued(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.send_pending(chat_id, now) && self.chat_link_at(chat_id, now).send_would_queue()
    }

    /// Why the selected chat's blank transcript is still loading, if it is.
    /// A never-synced copy of a remote chat replays as empty, which is not
    /// "no messages": it stays loading until its room proves it caught up.
    /// A chat hosted here, or on a local profile, is authoritative from its
    /// own disk; an unsaved side chat has nothing to load.
    pub fn transcript_loading(&self, now: DateTime<Utc>) -> Option<TranscriptLoading> {
        use zeron_proto::{ChatSyncState as C, ConnectivityState as S};
        let chat_id = self.selected_chat.as_deref()?;
        let hosted_here = self.chats.iter().any(|chat| {
            chat.id == chat_id && Some(chat.device_id.as_str()) == self.local_device_id.as_deref()
        });
        if self.engine.is_none()
            || self.unsaved_side_chat
            || hosted_here
            || (self.connectivity_observed && self.connectivity.state == S::Disabled)
            || !self.transcript.is_empty()
            || self
                .echoes
                .get(chat_id)
                .is_some_and(|echoes| !echoes.is_empty())
        {
            return None;
        }
        if !self.transcript_replayed {
            return Some(TranscriptLoading::Opening);
        }
        let caught_up = self
            .connectivity
            .chats
            .iter()
            .find(|net| net.chat_id == chat_id)
            .is_some_and(|net| {
                net.delivery_live
                    || match net.sync_state {
                        C::Local | C::Synced | C::StorageError => true,
                        C::Unknown => net.connected,
                        C::Waiting | C::Connecting | C::Offline => false,
                    }
            });
        if caught_up {
            return None;
        }
        Some(match self.chat_link_at(chat_id, now).sync {
            LinkSync::Offline => TranscriptLoading::Offline,
            LinkSync::Stalled | LinkSync::Reconnecting => TranscriptLoading::Reconnecting,
            // Not reported yet: the room the replay opened has yet to speak.
            LinkSync::Live | LinkSync::Syncing { .. } => TranscriptLoading::Syncing,
        })
    }

    pub fn apply_devices(&mut self, devices: Vec<Device>) -> bool {
        self.apply_devices_at(devices, Utc::now())
    }

    fn apply_devices_at(&mut self, mut devices: Vec<Device>, now: DateTime<Utc>) -> bool {
        // A local-only workspace has no remote device identity to distinguish.
        // Keep the engine's legacy sentinel out of the UI while preserving real
        // hostnames and user-assigned device names.
        if self.workspace_scope == Some(WorkspaceScope::Local)
            && let Some(local_id) = self.local_device_id.as_deref()
            && let Some(device) = devices.iter_mut().find(|device| device.id == local_id)
            && device.name == "unknown-device"
        {
            device.name = "Local".to_string();
        }
        for device in &devices {
            self.change_requests
                .clear_unsupported_on_version_change(&device.id, device.version.as_deref());
        }
        let presentation = device_presentation_at(&devices, now);
        let session_presence = session_presence_at(&self.sessions, now);
        let changed = self.device_presentation.as_ref() != Some(&presentation)
            || self.session_presence_presentation != session_presence;
        self.device_presentation = Some(presentation);
        self.session_presence_presentation = session_presence;
        // Freshness must advance even when the heartbeat does not redraw:
        // delivery gating and later renders still need the newest timestamp.
        self.devices = devices;
        changed
    }

    /// True when `device_id`'s engine (per its registry device row) is at
    /// least `min`. Unknown devices and unstamped versions are conservatively
    /// false — feature gates fall back to the legacy path rather than speak a
    /// protocol the peer may not understand.
    pub fn device_version_at_least(&self, device_id: &str, min: (u64, u64, u64)) -> bool {
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .and_then(|d| d.version.as_deref())
            .and_then(version_triple)
            .is_some_and(|v| v >= min)
    }

    /// Capability checks use the live EngineInfo for this device (including a
    /// localhost daemon that may not match the UI binary) and synced device
    /// rows for peers. Missing declarations are conservatively unsupported.
    pub fn device_supports(&self, device_id: &str, capability: &str) -> bool {
        if let Some(engine) = self.engine.as_ref()
            && engine.engine_info().device_id == device_id
        {
            return engine.engine_info().supports(capability);
        }
        self.devices
            .iter()
            .find(|device| device.id == device_id)
            .is_some_and(|device| device.supports(capability))
    }

    pub fn chat_host_supports(&self, chat_id: &str, capability: &str) -> bool {
        self.chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| self.device_supports(&chat.device_id, capability))
    }

    /// First project on the composer's picked device (falling back through
    /// the local device, then any project at all — better a cross-device
    /// project than a surprise project-less canvas). Display order.
    fn first_space_on_picked_device(&self) -> Option<String> {
        let device = self
            .selected_device
            .as_deref()
            .or(self.local_device_id.as_deref());
        let sorted = self.spaces_sorted();
        device
            .and_then(|d| sorted.iter().find(|s| s.device_id == d).copied())
            .or_else(|| sorted.first().copied())
            .map(|s| s.id.clone())
    }

    pub fn apply_harness_updates(
        &mut self,
        statuses: Vec<zeron_proto::HarnessUpdateStatus>,
    ) -> bool {
        if self.harness_updates == statuses {
            return false;
        }
        self.harness_updates = statuses;
        true
    }

    pub fn apply_auth(&mut self, auth: AuthState) -> bool {
        if self.auth.as_ref() == Some(&auth) {
            return false;
        }
        self.auth = Some(auth);
        true
    }

    /// Tolerant AuthStatus frame reducer (see [`parse_auth_state`]).
    pub fn apply_auth_value(&mut self, value: serde_json::Value) -> bool {
        match parse_auth_state(&value) {
            Some(auth) => self.apply_auth(auth),
            None => {
                tracing::warn!("dropping unrecognized AuthStatus frame");
                false
            }
        }
    }

    /// Signed in (Cloud is per account; whether this account can use it is
    /// `CloudStatus::available`).
    pub fn signed_in(&self) -> bool {
        matches!(self.auth, Some(AuthState::SignedIn { .. }))
    }

    /// The signed-in user, if the engine reports one.
    pub fn auth_user(&self) -> Option<&zeron_proto::UserProfile> {
        match self.auth.as_ref()? {
            AuthState::SignedIn { user, .. } | AuthState::NeedsOrganization { user } => Some(user),
            AuthState::SignedOut => None,
        }
    }

    pub fn apply_transcript(&mut self, entries: Vec<SessionMessageEntry>) {
        if let Some(id) = &self.selected_chat {
            self.prepared_transcripts.remove(id);
        }
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript = entries;
        self.settle_echoes();
        self.transcript_replayed = true;
        self.ack_pending_send_from_transcript();
    }

    /// Doc frames supersede optimistic echoes carrying the same id. A Cloud
    /// setup placeholder also goes once the doc moved past its message
    /// without the machine's setup entry (the machine was already up).
    fn settle_echoes(&mut self) {
        let Some(chat_id) = self.selected_chat.as_deref() else {
            return;
        };
        let Some(echoes) = self.echoes.get_mut(chat_id) else {
            return;
        };
        let transcript = &self.transcript;
        echoes.retain(|echo| {
            if transcript.iter().any(|e| e.id == echo.id) {
                return false;
            }
            let Some(message_id) = zeron_proto::cloud_setup_message_id(&echo.id) else {
                return true;
            };
            transcript
                .iter()
                .position(|e| e.id == message_id)
                .is_none_or(|ix| ix + 1 == transcript.len())
        });
    }

    /// Apply a `WatchDocMessages` delta frame in place. `Err` = this copy has
    /// diverged; the watch task resubscribes for a fresh reset.
    pub fn apply_transcript_frame(
        &mut self,
        frame: TranscriptFrame,
    ) -> Result<(), TranscriptDesync> {
        if let Some(id) = &self.selected_chat {
            self.prepared_transcripts.remove(id);
        }
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        let is_reset = matches!(&frame, TranscriptFrame::Reset { .. });
        zeron_doc::apply_transcript_frame(&mut self.transcript, frame)?;
        if is_reset {
            self.transcript_replayed = true;
        }
        self.settle_echoes();
        self.ack_pending_send_from_transcript();
        Ok(())
    }

    /// Apply an incoming frame and invalidate only its affected presentation.
    pub fn receive_transcript_frame(
        &mut self,
        frame: TranscriptFrame,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        let text_doc = self
            .selected_chat
            .as_ref()
            .filter(|id| {
                is_text_append(&frame)
                    && !self.pending_sends.contains_key(*id)
                    && self.pending_echoes().is_empty()
            })
            .cloned();
        let result = self.apply_transcript_frame(frame);
        if let Some(doc_id) = text_doc.filter(|_| result.is_ok()) {
            cx.emit(TranscriptTextChanged { doc_id });
        } else {
            cx.notify();
        }
        result
    }

    pub(crate) fn transcript_baseline(
        &self,
        doc_id: &str,
    ) -> Option<&Arc<zeron_doc::TranscriptBaseline>> {
        self.transcript_baselines.get(doc_id)
    }

    pub fn receive_transcript_update(
        &mut self,
        update: zeron_doc::TranscriptUpdate,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        self.receive_transcript_frame(update.frame, cx)?;
        if let (Some(doc_id), Some(baseline)) = (&self.selected_chat, update.replay_baseline) {
            self.transcript_baselines
                .insert(doc_id.clone(), Arc::new(baseline));
        }
        if self.context_usage != update.context_usage {
            self.context_usage = update.context_usage;
            cx.notify();
        }
        Ok(())
    }

    /// The opt-in opening tail is provisional. Never replace a complete view
    /// with it, and don't treat it as a full reset for caching/scroll anchors.
    pub(crate) fn receive_opening_transcript_update(
        &mut self,
        update: zeron_doc::TranscriptUpdate,
        history_pending: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        if history_pending && self.transcript_replayed {
            return Ok(());
        }
        let old_prepared = self
            .selected_chat
            .as_ref()
            .and_then(|id| self.prepared_transcripts.remove(id));
        let old_entries = if matches!(&update.frame, TranscriptFrame::Reset { .. }) {
            std::mem::take(&mut self.transcript)
        } else {
            Vec::new()
        };
        cx.background_executor()
            .spawn(async move {
                drop((old_prepared, old_entries));
            })
            .detach();
        self.receive_transcript_update(update, cx)?;
        if history_pending {
            self.transcript_replayed = false;
        }
        Ok(())
    }

    /// A subagent doc's current transcript copy (empty until its watch's
    /// replay frame lands, or its frozen snapshot is set).
    pub fn sub_transcript(&self, doc_id: &str) -> &[SessionMessageEntry] {
        self.sub_transcripts
            .get(doc_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Watch a SUBAGENT doc (`WatchDocMessages` works for any doc id).
    /// Single-flight per key; a frozen snapshot already in place wins — the
    /// watch would race the (complete) blob with a possibly-purged live doc.
    pub fn watch_subagent_doc(&mut self, doc_id: String, cx: &mut Context<Self>) {
        if self.sub_watch_tasks.contains_key(&doc_id) {
            return;
        }
        let Some(handle) = self.engine.clone() else {
            return;
        };
        self.sub_transcripts.entry(doc_id.clone()).or_default();
        let task = spawn_subagent_watch(cx, handle, doc_id.clone());
        self.sub_watch_tasks.insert(doc_id, task);
    }

    /// Tab closed: drop the watch task (cancels the engine-side watch and
    /// unpins the doc from the engine LRU) and the rows.
    pub fn unwatch_subagent_doc(&mut self, doc_id: &str) {
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(doc_id);
        self.sub_transcripts.remove(doc_id);
        self.prepared_transcripts.remove(doc_id);
        self.transcript_baselines.remove(doc_id);
    }

    /// Frozen-blob path: the finished subagent's uploaded transcript, no
    /// watch needed (and any in-flight watch is superseded).
    pub fn set_subagent_snapshot(&mut self, doc_id: String, entries: Vec<SessionMessageEntry>) {
        self.prepared_transcripts.remove(&doc_id);
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(&doc_id);
        self.transcript_baselines.insert(
            doc_id.clone(),
            Arc::new(zeron_doc::TranscriptBaseline::capture(&entries)),
        );
        self.sub_transcripts.insert(doc_id, entries);
    }

    pub(crate) fn set_prepared_subagent_snapshot(
        &mut self,
        doc_id: String,
        entries: Vec<SessionMessageEntry>,
        prepared: Arc<crate::transcript::PreparedTranscript>,
    ) {
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(&doc_id);
        self.transcript_baselines
            .insert(doc_id.clone(), prepared.navigation_baseline.clone());
        self.prepared_transcripts.insert(doc_id.clone(), prepared);
        self.sub_transcripts.insert(doc_id, entries);
    }

    /// Add an optimistic user echo (composer send path).
    pub fn push_echo(&mut self, chat_id: &str, entry: SessionMessageEntry) {
        let echoes = self.echoes.entry(chat_id.to_string()).or_default();
        if !echoes.iter().any(|e| e.id == entry.id) {
            echoes.push(entry);
            self.transcript_revision = self.transcript_revision.wrapping_add(1);
        }
    }

    /// The send of `message_id` brings `chat_id`'s Cloud machine up (`waking`
    /// a sleeping one): show the first setup step under it right away. The
    /// machine's own setup entry replaces it in place once it boots
    /// (`zeron_proto::cloud_setup_entry_id`), with the steps that follow.
    pub fn push_cloud_setup_echo(
        &mut self,
        chat_id: &str,
        message_id: &str,
        created_at: i64,
        waking: bool,
    ) {
        // Pushed a beat after the send: the machine may already have written
        // its own setup entry, or (already up) moved past the message.
        let entry_id = zeron_proto::cloud_setup_entry_id(message_id);
        if self.selected_chat.as_deref() == Some(chat_id)
            && (self.transcript.iter().any(|e| e.id == entry_id)
                || self
                    .transcript
                    .iter()
                    .position(|e| e.id == message_id)
                    .is_some_and(|ix| ix + 1 < self.transcript.len()))
        {
            return;
        }
        // Worded and shaped like the machine's own first step ("Started a
        // machine"), which replaces it in place: no Cloud-step input, since
        // the doc keeps none on the machine's, so both draw alike.
        let name = if waking {
            CLOUD_WAKING_STEP
        } else {
            CLOUD_STARTING_STEP
        };
        self.push_echo(
            chat_id,
            SessionMessageEntry {
                id: entry_id,
                role: zeron_doc::MessageRole::Assistant,
                parts: vec![zeron_doc::MessagePart::Tool {
                    id: zeron_proto::cloud_setup_part_id(message_id, "machine"),
                    call: zeron_proto::ToolCall::Unknown {
                        name: name.into(),
                        input: None,
                    },
                    is_error: false,
                    resolved: false,
                    output: None,
                    diff: None,
                    output_ref: None,
                    output_bytes: None,
                    diff_ref: None,
                    diff_stats: None,
                    subagent_ref: None,
                    subagent_status: None,
                    subagent_tail: None,
                }],
                created_at: created_at + 1,
                device_id: "local".into(),
                status: Some(zeron_doc::MessageStatus::Streaming),
                continuation_of: None,
                duration_ms: None,
            },
        );
        // The machine may have failed before this placeholder arrived.
        self.settle_failed_cloud_setups();
    }

    /// The selected chat's latest message — an unconfirmed echo first, since
    /// it is newer than anything the transcript holds — with the Cloud setup
    /// entry it brought up, if it had to start or wake a machine. Setup
    /// entry ids derive from their message's, so finding one is enough.
    fn latest_cloud_setup(&self) -> Option<(&SessionMessageEntry, Option<&SessionMessageEntry>)> {
        let user = zeron_doc::MessageRole::User;
        let message = self
            .pending_echoes()
            .iter()
            .rev()
            .find(|entry| entry.role == user)
            .or_else(|| self.transcript.iter().rev().find(|entry| entry.role == user))?;
        let setup_id = zeron_proto::cloud_setup_entry_id(&message.id);
        let setup = self
            .pending_echoes()
            .iter()
            .chain(&self.transcript)
            .find(|entry| entry.id == setup_id);
        Some((message, setup))
    }

    /// The selected chat's current turn is a Cloud setup still under way:
    /// the placeholder, or the machine's setup entry (which carries on as
    /// the turn's answer) still streaming.
    pub fn cloud_setup_streaming(&self) -> bool {
        self.latest_cloud_setup().is_some_and(|(_, setup)| {
            setup.is_some_and(|setup| setup.status == Some(zeron_doc::MessageStatus::Streaming))
        })
    }

    /// When the send that brought the selected chat's Cloud machine up was
    /// made (ms), while that setup belongs to the current turn — the app's
    /// placeholder or the machine's own steps after the latest message.
    /// Setting the machine up is part of the work, so the turn's timer counts
    /// from the send, and keeps that base once the turn itself starts. A
    /// newer message to a machine already up has none.
    pub fn cloud_setup_turn_started(&self) -> Option<i64> {
        let (message, setup) = self.latest_cloud_setup()?;
        setup.map(|_| message.created_at)
    }

    /// The selected chat shows a Cloud setup placeholder (it says what the
    /// send waits on, so the working line under it doesn't repeat it).
    pub fn cloud_setup_echo_pending(&self) -> bool {
        self.pending_echoes()
            .iter()
            .any(|echo| zeron_proto::cloud_setup_message_id(&echo.id).is_some())
    }

    /// Drop an echo (send failed — the prompt returns to the draft).
    pub fn remove_echo(&mut self, chat_id: &str, message_id: &str) {
        if let Some(echoes) = self.echoes.get_mut(chat_id) {
            let previous_len = echoes.len();
            echoes.retain(|e| e.id != message_id);
            if echoes.len() != previous_len {
                self.transcript_revision = self.transcript_revision.wrapping_add(1);
            }
        }
    }

    /// Composer send fired: overlay the chat as Working until the host writes
    /// the user message back into the transcript or its session row shows the
    /// turn that runs it ([`PendingSend::adopted_by`]). A remote send has no
    /// live session row until the host drains the queued command — that gap
    /// read as "no live run" and flashed the Completed dot, and any phantom
    /// Working→Idle edge in it rang the done-chime on send (user report
    /// 2026-08-05).
    pub fn begin_pending_send(&mut self, chat_id: &str, message_id: &str, now: DateTime<Utc>) {
        self.begin_send(chat_id, message_id, false, now);
    }

    /// [`Self::begin_pending_send`] for the send that creates its chat.
    pub fn begin_first_send(&mut self, chat_id: &str, message_id: &str, now: DateTime<Utc>) {
        self.begin_send(chat_id, message_id, true, now);
    }

    fn begin_send(&mut self, chat_id: &str, message_id: &str, new_chat: bool, now: DateTime<Utc>) {
        let row_before = self.session_for(chat_id).cloned();
        self.pending_sends.insert(
            chat_id.to_string(),
            PendingSend {
                message_id: message_id.to_string(),
                started: now,
                row_before,
                new_chat,
            },
        );
    }

    /// Send failed — drop the overlay so the dot tells the truth again. Only
    /// removes the overlay this message started: a quick resend must not lose
    /// its own overlay to the first send's failure cleanup.
    pub fn end_pending_send(&mut self, chat_id: &str, message_id: &str) {
        if self
            .pending_sends
            .get(chat_id)
            .is_some_and(|p| p.message_id == message_id)
        {
            self.pending_sends.remove(chat_id);
        }
    }

    /// Attachment upload starting: expose its progress to the working label.
    pub fn begin_upload_progress(
        &mut self,
        total: u64,
        done: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        self.upload_progress = Some(UploadProgress { done, total });
    }

    /// Upload leg over (success or failure) — the label goes back to plain
    /// send/working wording.
    pub fn end_upload_progress(&mut self) {
        self.upload_progress = None;
    }

    /// Percent of the in-flight attachment upload, clamped to 99 — the last
    /// point belongs to the commit + queue, so "100% but still spinning"
    /// never shows. `None` when no upload is in flight (or it's empty).
    pub fn upload_progress_percent(&self) -> Option<u8> {
        let progress = self.upload_progress.as_ref()?;
        if progress.total == 0 {
            return None;
        }
        let done = progress
            .done
            .load(std::sync::atomic::Ordering::Relaxed)
            .min(progress.total);
        Some(((done * 100) / progress.total).min(99) as u8)
    }

    /// A `WatchTransfers` snapshot: the engine-side relay leg's in-flight
    /// queued-attachment transfers, replacing the whole set each frame.
    pub fn apply_transfers(&mut self, transfers: Vec<zeron_proto::TransferProgress>) -> bool {
        let transfers: HashMap<_, _> = transfers
            .into_iter()
            .map(|t| (t.upload_id, (t.done, t.total)))
            .collect();
        if self.transfers == transfers {
            return false;
        }
        self.transfers = transfers;
        true
    }

    /// Percent of one queued attachment's relay transfer, by the uploadId
    /// its `pending://{uploadId}/…` ref names. Same 99-clamp as
    /// [`Self::upload_progress_percent`]: the last point belongs to the
    /// commit, so "100% but still spinning" never shows. `None` when no
    /// bytes are moving for that upload (staged-but-waiting, retry backoff,
    /// or done) — the thumbnail falls back to its indeterminate spinner.
    pub fn transfer_percent(&self, upload_id: &str) -> Option<u8> {
        let (done, total) = self.transfers.get(upload_id)?;
        if *total == 0 {
            return None;
        }
        Some(((done.min(total) * 100) / total).min(99) as u8)
    }

    /// Is a send still in flight for this chat (unacked)? Inside the grace
    /// window normally; while nothing confirms the chat's path (see
    /// [`ChatLink::delivery_unconfirmed`]) the overlay holds indefinitely —
    /// silently expiring back to Idle left a queued send with no visible
    /// trace at all (the 30s→silence hole, 2026-08-19).
    pub fn send_pending(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.pending_sends.get(chat_id).is_some_and(|p| {
            now.signed_duration_since(p.started).num_milliseconds() <= UNDELIVERED_GRACE_MS
                || self.chat_link_at(chat_id, now).delivery_unconfirmed()
                || self.cloud_machine_booting(chat_id).is_some()
        })
    }

    /// The send has sat unadopted past the grace window: surface the
    /// EXPLICIT failed state ("Not delivered — retry") instead of either
    /// faking progress or silently forgetting the send ever happened. A chat
    /// still syncing has not failed it: the send went out with the room's
    /// first pushes, or leaves when the room joins, and its adoption shows
    /// once the copy catches up.
    pub fn send_undelivered(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.pending_sends.get(chat_id).is_some_and(|p| {
            now.signed_duration_since(p.started).num_milliseconds() > UNDELIVERED_GRACE_MS
        }) && self.cloud_machine_booting(chat_id).is_none()
            && !matches!(
                self.chat_link_at(chat_id, now).sync,
                LinkSync::Syncing { .. }
            )
    }

    /// Retry pressed: restart the grace clock so the overlay returns to its
    /// Sending/Queued phase while the re-kicked delivery runs.
    pub fn retry_pending_send(&mut self, chat_id: &str, now: DateTime<Utc>) {
        if let Some(p) = self.pending_sends.get_mut(chat_id) {
            p.started = now;
        }
    }

    /// When the in-flight send (if any, inside the TTL) was fired — the
    /// elapsed-timer base while the overlay reads as Working. The session
    /// row's `started_at` still belongs to the PREVIOUS turn during this
    /// window, and showing it made a fresh send open at the old turn's
    /// half-hour mark.
    pub fn pending_send_started(&self, chat_id: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.pending_sends
            .get(chat_id)
            .filter(|p| {
                now.signed_duration_since(p.started).num_milliseconds() <= UNDELIVERED_GRACE_MS
                    || self.chat_link_at(chat_id, now).delivery_unconfirmed()
                    || self.cloud_machine_booting(chat_id).is_some()
            })
            .map(|p| p.started)
    }

    /// The host executed the queued command iff the sent message's id showed
    /// up in the transcript (it writes the message before — causally with —
    /// the Working status; sessions.rs dispatch paths).
    fn ack_pending_send_from_transcript(&mut self) {
        if let Some(chat_id) = self.selected_chat.as_deref()
            && let Some(pending) = self.pending_sends.get(chat_id)
            && self.transcript.iter().any(|e| e.id == pending.message_id)
        {
            self.pending_sends.remove(chat_id);
        }
    }

    /// A send the host held for the next turn shows as its queue row, not
    /// as an echo: drop echoes (and the pending-send overlay) for queued ids.
    pub fn apply_queue(&mut self, items: Vec<zeron_doc::QueuedMessage>) {
        if let Some(chat_id) = self.selected_chat.as_deref() {
            if let Some(echoes) = self.echoes.get_mut(chat_id) {
                let before = echoes.len();
                echoes.retain(|echo| !items.iter().any(|q| q.id == echo.id));
                if echoes.len() != before {
                    self.transcript_revision = self.transcript_revision.wrapping_add(1);
                }
            }
            if self
                .pending_sends
                .get(chat_id)
                .is_some_and(|p| items.iter().any(|q| q.id == p.message_id))
            {
                self.pending_sends.remove(chat_id);
            }
        }
        self.queue = items;
    }

    /// Unconfirmed echoes for the selected chat, in send order.
    pub fn pending_echoes(&self) -> &[SessionMessageEntry] {
        self.selected_chat
            .as_deref()
            .and_then(|id| self.echoes.get(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    // ---- queries ----

    /// Non-archived, top-level chats in sidebar order. Chats spawned by
    /// another chat (`parent_chat_id`, the Zeron MCP's orchestration link)
    /// are the parent's workers, not sessions the user started: they stay
    /// reachable by id/deep link but never take a sidebar row or jump slot.
    /// Voice orchestrator chats are hidden the same way.
    pub fn visible_chats(&self) -> impl Iterator<Item = &Chat> {
        self.chats
            .iter()
            .filter(|c| !c.archived && c.is_top_level())
    }

    pub(crate) fn restore_composer_target(
        &mut self,
        defaults: &crate::settings::composer::ComposerDefaults,
    ) {
        if self.selected_chat.is_some() || self.no_project {
            return;
        }
        if self.selected_device.is_none() {
            self.selected_device = defaults.device.clone();
        }
        if self.selected_space.is_none() {
            self.no_project = defaults.no_project;
            self.selected_space = if defaults.no_project {
                None
            } else {
                defaults.project.clone()
            };
        }
    }

    fn current_canvas_target(&self) -> CanvasTarget {
        CanvasTarget {
            space: self.selected_space.clone(),
            no_project: self.no_project,
            device: self.selected_device.clone(),
        }
    }

    /// Put back the canvas target set aside when a chat opened. A project
    /// deleted meanwhile heals the way [`Self::apply_spaces`] would.
    fn restore_canvas_target(&mut self) {
        let Some(target) = self.canvas_target.take() else {
            return;
        };
        self.selected_device = target.device;
        self.no_project = target.no_project;
        self.selected_space = target.space;
        if self.spaces_synced
            && !self.no_project
            && self
                .selected_space
                .as_ref()
                .is_none_or(|id| !self.spaces.iter().any(|s| &s.id == id))
        {
            self.selected_space = self.first_space_on_picked_device();
        }
    }

    pub fn selected_space_row(&self) -> Option<&Space> {
        if self.no_project {
            return None;
        }
        let id = self.selected_space.as_deref()?;
        self.spaces.iter().find(|s| s.id == id)
    }

    /// The device the new-session canvas targets: the picked project's host
    /// when one is selected, else the explicit device pick, else this device.
    pub fn effective_device_id(&self) -> Option<String> {
        if let Some(space) = self.selected_space_row() {
            return Some(space.device_id.clone());
        }
        self.selected_device
            .clone()
            .or_else(|| self.local_device_id.clone())
    }

    /// Pick the composer's target device. Keeps the project pick consistent:
    /// it moves to the project's checkout on the new device, else the first
    /// project there, else "no project".
    pub fn select_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        let moving = self
            .selected_space_row()
            .filter(|s| s.device_id != device_id);
        if let Some(space) = moving {
            let first = self
                .project_members(space)
                .into_iter()
                .find(|s| s.device_id == device_id)
                .or_else(|| {
                    self.spaces_sorted()
                        .into_iter()
                        .find(|s| s.device_id == device_id)
                })
                .map(|s| s.id.clone());
            self.no_project = first.is_none();
            self.selected_space = first;
        }
        self.selected_device = Some(device_id);
        cx.notify();
    }

    pub fn space_row(&self, space_id: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == space_id)
    }

    /// Spaces in display order — case-insensitive alphabetical, the order
    /// both space selectors (sidebar filter, composer picker) list rows in.
    /// Ties break on id so the order is stable across renders.
    pub fn spaces_sorted(&self) -> Vec<&Space> {
        let mut spaces: Vec<&Space> = self.spaces.iter().collect();
        spaces.sort_by_key(|s| (s.display_name().to_lowercase(), s.id.clone()));
        spaces
    }

    pub fn space_for_chat(&self, chat: &Chat) -> Option<&Space> {
        self.space_row(chat.space_id.as_deref()?)
    }

    /// The space whose name and color stand for `space`'s whole project
    /// ([`zeron_proto::view::representative_space`]).
    pub fn representative_space<'a>(&'a self, space: &'a Space) -> &'a Space {
        zeron_proto::view::representative_space(&self.spaces, space)
    }

    /// Every space of `space`'s project ([`zeron_proto::view::project_key`]):
    /// this device's first, then by device name and path.
    pub fn project_members(&self, space: &Space) -> Vec<&Space> {
        let key = zeron_proto::view::project_key(space);
        let local = self.local_device_id.as_deref();
        let mut members: Vec<&Space> = self
            .spaces
            .iter()
            .filter(|s| zeron_proto::view::project_key(s) == key)
            .collect();
        members.sort_by_key(|s| {
            (
                local != Some(s.device_id.as_str()),
                self.device_name(&s.device_id)
                    .unwrap_or_default()
                    .to_lowercase(),
                s.path.clone(),
                s.id.clone(),
            )
        });
        members
    }

    /// Non-archived chats of a space in tab (creation) order. Chats with a
    /// dangling/missing `space_id` are invisible by construction.
    pub fn chats_in_space(&self, space_id: &str) -> Vec<&Chat> {
        let mut chats: Vec<&Chat> = self
            .visible_chats()
            .filter(|c| c.space_id.as_deref() == Some(space_id))
            .collect();
        sort_tabs(&mut chats);
        chats
    }

    pub fn device_name(&self, device_id: &str) -> Option<&str> {
        let device = self.devices.iter().find(|d| d.id == device_id);
        // A session's machine is shown as the Cloud it belongs to.
        if self.is_cloud_session_device(device_id) {
            return Some(
                self.cloud_account_device()
                    .map_or(crate::cloud::CLOUD_LABEL, |d| d.name.as_str()),
            );
        }
        // The logical Cloud device (Cloud projects live there) has no
        // registry row of its own.
        if device.is_none()
            && self
                .cloud_status
                .as_ref()
                .and_then(|status| status.device_id.as_deref())
                == Some(device_id)
        {
            return Some(crate::cloud::CLOUD_LABEL);
        }
        device.map(|d| d.name.as_str())
    }

    /// Devices for lists and pickers: Cloud's per-session machines are
    /// hidden; the logical Cloud device stands for them.
    pub fn listed_devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.iter().filter(|d| crate::cloud::is_listed(d))
    }

    /// Devices the Providers page lists by name. Cloud is one extra entry
    /// there (its accounts live in the vault), never its session machines.
    pub fn provider_devices(&self) -> impl Iterator<Item = &Device> {
        self.listed_devices().filter(|d| !crate::cloud::is_cloud(d))
    }

    /// The device a list shows for `device_id`: a session machine is shown
    /// as the logical Cloud device it belongs to.
    pub fn listed_device_id(&self, device_id: &str) -> String {
        if self.is_cloud_session_device(device_id) {
            let account = self
                .cloud_account_device()
                .map(|d| d.id.clone())
                .or_else(|| self.cloud_status.as_ref().and_then(|s| s.device_id.clone()));
            if let Some(account) = account {
                return account;
            }
        }
        device_id.to_string()
    }

    /// The logical Cloud device's registry row, when Cloud is on.
    pub fn cloud_account_device(&self) -> Option<&Device> {
        self.devices
            .iter()
            .find(|d| crate::cloud::is_cloud_account(d))
    }

    /// A session machine: by its registry capability, or (before its engine
    /// has registered) by a session naming it.
    pub fn is_cloud_session_device(&self, device_id: &str) -> bool {
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .is_some_and(crate::cloud::is_cloud_session_device)
            || self
                .cloud_sessions
                .as_ref()
                .is_some_and(|s| s.sessions.iter().any(|x| x.device_id == device_id))
    }

    /// Host-presence check: is this device's 15s presence heartbeat fresh?
    /// Distinguishes "host offline" (its queued work syncs when it returns)
    /// from slow sync. The local device is trivially online; unknown devices
    /// get the benefit of the doubt (no evidence — don't cry wolf).
    pub fn device_online(&self, device_id: &str, now: DateTime<Utc>) -> bool {
        if self.local_device_id.as_deref() == Some(device_id) {
            return true;
        }
        match self.devices.iter().find(|d| d.id == device_id) {
            Some(d) => crate::settings::devices::device_online(d.last_seen_at, now),
            None => true,
        }
    }

    /// [`Self::device_online`] refined for Cloud. The logical Cloud device
    /// reads from the account (on while Cloud is enabled); a session machine
    /// from its session (a stopped one is "Asleep" — it wakes when its chat
    /// is sent to — not "Offline").
    pub fn device_presence(&self, device_id: &str, now: DateTime<Utc>) -> crate::cloud::Presence {
        use crate::cloud::Presence;
        let device = self.devices.iter().find(|d| d.id == device_id);
        let is_account = device.is_some_and(crate::cloud::is_cloud_account)
            || self
                .cloud_status
                .as_ref()
                .and_then(|s| s.device_id.as_deref())
                == Some(device_id);
        if is_account {
            return crate::cloud::account_presence(self.cloud_status.as_ref());
        }
        let online = self.device_online(device_id, now);
        if self.is_cloud_session_device(device_id) || device.is_some_and(crate::cloud::is_cloud) {
            let session = self
                .cloud_sessions
                .as_ref()
                .and_then(|s| s.sessions.iter().find(|x| x.device_id == device_id));
            let mut presence = crate::cloud::session_presence(device.is_some() && online, session);
            // Machines sleep on their own when idle, and the session list is
            // only re-read while one settles: a "ready" machine whose
            // heartbeat stopped has gone to sleep, not lost its connection.
            if presence == Presence::Online && device.is_some() && !online {
                presence = Presence::Asleep;
            }
            // A wake was asked for and the list hasn't caught up yet.
            if presence == Presence::Asleep
                && session.is_some_and(|session| {
                    self.cloud_wakes
                        .get(&session.chat_id)
                        .is_some_and(|at| at.elapsed() < CLOUD_WAKE_GRACE)
                })
            {
                presence = Presence::Waking;
            }
            return presence;
        }
        if online {
            Presence::Online
        } else {
            Presence::Offline
        }
    }

    /// Publish a fresh `CloudStatus`; observers repaint only on change.
    pub fn set_cloud_status(&mut self, status: zeron_proto::CloudStatus, cx: &mut Context<Self>) {
        self.cloud_status_read_at = Some(std::time::Instant::now());
        let out_of_credits = crate::cloud::out_of_credits(&status);
        if self.cloud_status.as_ref() != Some(&status) {
            self.cloud_status = Some(status);
            cx.notify();
        }
        if out_of_credits && self.block_cloud_setups_for_credits() {
            crate::cloud::show_out_of_credits(
                "You're out of Cloud credits. Your message will send once you add some.",
                cx,
            );
            cx.notify();
        }
    }

    /// Cloud refused (or will refuse) to start or wake a machine for lack of
    /// credits: every send still waiting on one says so instead of "Waking
    /// the machine" forever, and stops reading as working. The message
    /// stays queued for when credits are added. True when any changed.
    fn block_cloud_setups_for_credits(&mut self) -> bool {
        let mut blocked = Vec::new();
        for (chat_id, echoes) in &mut self.echoes {
            for entry in echoes.iter_mut() {
                if zeron_proto::cloud_setup_message_id(&entry.id).is_none() {
                    continue;
                }
                let mut changed = false;
                for part in &mut entry.parts {
                    if let zeron_doc::MessagePart::Tool {
                        call: zeron_proto::ToolCall::Unknown { name, .. },
                        is_error,
                        resolved,
                        ..
                    } = part
                        && (name == CLOUD_WAKING_STEP || name == CLOUD_STARTING_STEP)
                    {
                        *name = CLOUD_NO_CREDITS_STEP.into();
                        *is_error = true;
                        *resolved = true;
                        changed = true;
                    }
                }
                if changed {
                    // Not streaming any more: the turn isn't under way.
                    entry.status = Some(zeron_doc::MessageStatus::Complete);
                    blocked.push(chat_id.clone());
                }
            }
        }
        for chat_id in &blocked {
            self.pending_sends.remove(chat_id);
        }
        if !blocked.is_empty() {
            self.transcript_revision = self.transcript_revision.wrapping_add(1);
        }
        !blocked.is_empty()
    }

    /// Re-read Cloud's status (credits above all), nothing else. A failed
    /// read is retried soon: unknown credits let a Cloud send through.
    pub fn refresh_cloud_status(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if !self.signed_in() {
            return;
        }
        self.cloud_status_task = Some(cx.spawn(async move |this, cx| {
            let status = crate::cloud::fetch_status(engine).await;
            this.update(cx, |state, cx| match status {
                Ok(status) => state.set_cloud_status(status, cx),
                Err(error) => {
                    tracing::debug!(%error, "Cloud status read failed; retrying");
                    state.cloud_status_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(CLOUD_STATUS_RETRY).await;
                        this.update(cx, |state, cx| state.refresh_cloud_status(cx))
                            .ok();
                    }));
                }
            })
            .ok();
        }));
    }

    /// Re-read Cloud's status when the last read is older than `max_age`.
    pub fn refresh_cloud_status_if_stale(
        &mut self,
        max_age: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        if self
            .cloud_status_read_at
            .is_some_and(|at| at.elapsed() < max_age)
        {
            return;
        }
        self.refresh_cloud_status(cx);
    }

    /// A window came forward: credits may have run out (or been added)
    /// while it was away.
    pub fn window_activated(&mut self, cx: &mut Context<Self>) {
        self.refresh_cloud_status_if_stale(CLOUD_STATUS_FOCUS_MAX_AGE, cx);
    }

    /// A send is starting or waking a Cloud machine: read the credits now,
    /// and again once the edge has had its say on the wake.
    pub fn recheck_cloud_credits(&mut self, cx: &mut Context<Self>) {
        const AFTER_WAKE: std::time::Duration = std::time::Duration::from_secs(5);
        self.refresh_cloud_status(cx);
        self.cloud_recheck_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AFTER_WAKE).await;
            this.update(cx, |state, cx| state.refresh_cloud_status(cx))
                .ok();
        }));
    }

    /// Whether Cloud is on for this account (its logical device exists).
    /// Cloud is on for this account — never while signed out, whatever a
    /// stale status says.
    /// Signed in, and the edge offers this account Cloud (`CLOUD_ACCESS`):
    /// Cloud and Billing in Settings, the Cloud checkout, Cloud projects.
    /// False until the status arrives.
    pub fn cloud_offered(&self) -> bool {
        self.signed_in() && self.cloud_status.as_ref().is_some_and(|s| s.available)
    }

    /// [`Self::cloud_offered`], but true while the status is still unknown
    /// (setup keeps its GitHub step until the edge says otherwise).
    pub fn cloud_maybe_offered(&self) -> bool {
        self.signed_in() && self.cloud_status.as_ref().is_none_or(|s| s.available)
    }

    pub fn cloud_enabled(&self) -> bool {
        self.cloud_offered()
            && (self.cloud_status
            .as_ref()
            .is_some_and(crate::cloud::account_enabled)
                || self.cloud_account_device().is_some())
    }

    /// The Cloud session `chat_id` runs in — its own, or for a side chat its
    /// parent's (same machine) — when the chat is hosted on Cloud.
    pub fn chat_cloud_session(&self, chat_id: &str) -> Option<&zeron_proto::CloudSession> {
        let sessions = self.cloud_sessions.as_ref()?;
        let device_id = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .map_or("", |c| c.device_id.as_str());
        crate::cloud::session_for(sessions, chat_id, device_id)
    }

    /// A send to `chat_id` is waiting for its Cloud machine to be set up or
    /// woken: delivered (queued for the machine), not failed — the machine
    /// picks it up as soon as it is up. `None` once the machine runs, or when
    /// it failed (that is a real delivery problem).
    pub fn cloud_machine_booting(&self, chat_id: &str) -> Option<zeron_proto::CloudState> {
        use zeron_proto::CloudState;
        self.chat_machine_down(chat_id)
            .map(|(_, state)| state)
            .filter(|state| {
                matches!(
                    state,
                    CloudState::Provisioning
                        | CloudState::Starting
                        | CloudState::Sleeping
                        | CloudState::Stopping
                )
            })
    }

    /// Whether a send is waiting on a Cloud machine (keeps the session poll
    /// running until the machine is up).
    fn send_waits_on_cloud(&self) -> bool {
        self.pending_sends
            .keys()
            .any(|chat_id| self.cloud_machine_booting(chat_id).is_some())
    }

    /// When `chat_id`'s Cloud machine isn't running: the session to wake
    /// (its chat id) and its state. Files, terminals and changes show a Wake
    /// button then instead of dialing a machine that won't answer.
    pub fn chat_machine_down(&self, chat_id: &str) -> Option<(String, zeron_proto::CloudState)> {
        use zeron_proto::CloudState;
        let chat = self.chats.iter().find(|c| c.id == chat_id)?;
        if !self.is_cloud_session_device(&chat.device_id) {
            return None;
        }
        let known = self.devices.iter().any(|d| d.id == chat.device_id);
        let heartbeat = known && self.device_online(&chat.device_id, Utc::now());
        let waking = |session: &str| {
            self.cloud_wakes
                .get(session)
                .is_some_and(|at| at.elapsed() < CLOUD_WAKE_GRACE)
        };
        let (session, state) = match self.chat_cloud_session(chat_id) {
            // The session list is only re-read while a machine settles, so
            // one that went to sleep on its own still reads "ready": its
            // stopped heartbeat says otherwise.
            Some(session) if session.state == CloudState::Ready => {
                if heartbeat || !known {
                    return None;
                }
                (session.chat_id.clone(), CloudState::Sleeping)
            }
            Some(session) => (session.chat_id.clone(), session.state),
            // No session read yet: trust a fresh heartbeat, else asleep.
            None if heartbeat => return None,
            None => (
                chat.parent_chat_id
                    .clone()
                    .unwrap_or_else(|| chat.id.clone()),
                CloudState::Sleeping,
            ),
        };
        // A wake was asked for and the list hasn't caught up yet.
        let state = match state {
            CloudState::Sleeping | CloudState::Stopping if waking(&session) => CloudState::Starting,
            state => state,
        };
        Some((session, state))
    }

    /// A surface (files, git status) couldn't reach `chat_id`'s host. When
    /// that host is a Cloud machine, re-read the session list: it says at
    /// once that the machine is stopping or asleep, where its heartbeat
    /// takes over a minute to go stale. At most every few seconds.
    pub fn recheck_cloud_machine(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let on_cloud = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .is_some_and(|chat| self.is_cloud_session_device(&chat.device_id));
        if !on_cloud
            || self
                .cloud_recheck_at
                .is_some_and(|at| at.elapsed() < CLOUD_RECHECK_INTERVAL)
        {
            return;
        }
        self.cloud_recheck_at = Some(std::time::Instant::now());
        self.refresh_cloud_sessions(cx);
    }

    /// Wake a sleeping Cloud session (by its chat id). It reads as waking at
    /// once; the session list is followed until the machine is up.
    pub fn wake_cloud_session(&mut self, session_chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if self
            .cloud_wakes
            .get(&session_chat_id)
            .is_some_and(|at| at.elapsed() < CLOUD_WAKE_GRACE)
        {
            return;
        }
        self.cloud_wakes
            .insert(session_chat_id.clone(), std::time::Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<zeron_proto::CloudSession>(
                    zeron_rpc::methods::CLOUD_SESSION_WAKE,
                    serde_json::json!({ "chatId": session_chat_id }),
                )
                .await;
            this.update(cx, |state, cx| match result {
                Ok(session) => {
                    let mut sessions = state.cloud_sessions.clone().unwrap_or_default();
                    sessions.sessions.retain(|s| s.chat_id != session.chat_id);
                    sessions.sessions.push(session);
                    state.set_cloud_sessions(sessions, cx);
                    state.refresh_cloud_sessions(cx);
                }
                Err(error) => {
                    state.cloud_wakes.remove(&session_chat_id);
                    crate::toast::show(
                        cx,
                        crate::toast::ToastKind::Error,
                        format!("Couldn't wake the Cloud agent: {error}"),
                    );
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Publish fresh sessions; observers repaint only on change.
    pub fn set_cloud_sessions(
        &mut self,
        sessions: zeron_proto::CloudSessions,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sessions.as_ref() != Some(&sessions) {
            self.cloud_sessions = Some(sessions);
            self.settle_failed_cloud_setups();
            cx.notify();
        }
    }

    /// A chat whose Cloud machine couldn't be set up or woken: the setup
    /// step shown under its send turns into that failure, with the reason.
    /// Otherwise it would spin forever — only the machine itself, once up,
    /// replaces it. The send keeps its own course: no longer booting, it
    /// reads as not delivered, with Retry.
    fn settle_failed_cloud_setups(&mut self) {
        use zeron_doc::{MessagePart, MessageStatus};
        let failed: Vec<(String, String)> = self
            .echoes
            .keys()
            .filter_map(|chat_id| {
                let session = self.chat_cloud_session(chat_id)?;
                (session.state == zeron_proto::CloudState::Error).then(|| {
                    (
                        chat_id.clone(),
                        session
                            .error
                            .clone()
                            .unwrap_or_else(|| "Something went wrong with Cloud.".into()),
                    )
                })
            })
            .collect();
        for (chat_id, error) in failed {
            let Some(echoes) = self.echoes.get_mut(&chat_id) else {
                continue;
            };
            let mut settled = false;
            for echo in echoes.iter_mut().filter(|echo| {
                zeron_proto::cloud_setup_message_id(&echo.id).is_some()
                    && echo.status == Some(MessageStatus::Streaming)
            }) {
                echo.status = Some(MessageStatus::Complete);
                for part in &mut echo.parts {
                    if let MessagePart::Tool {
                        call: zeron_proto::ToolCall::Unknown { name, .. },
                        is_error,
                        resolved,
                        output,
                        ..
                    } = part
                    {
                        *name = if name == CLOUD_WAKING_STEP {
                            "Couldn't wake the machine"
                        } else {
                            "Couldn't start a machine"
                        }
                        .into();
                        *is_error = true;
                        *resolved = true;
                        *output = Some(error.clone());
                    }
                }
                settled = true;
            }
            if settled {
                self.transcript_revision = self.transcript_revision.wrapping_add(1);
            }
        }
    }

    /// Fold one session (a wake/sleep/delete reply) in, and follow it while
    /// it settles.
    pub fn apply_cloud_session(
        &mut self,
        session: zeron_proto::CloudSession,
        cx: &mut Context<Self>,
    ) {
        let sessions = self
            .cloud_sessions
            .get_or_insert_with(|| zeron_proto::CloudSessions {
                sessions: Vec::new(),
                available: true,
            });
        match sessions
            .sessions
            .iter_mut()
            .find(|s| s.chat_id == session.chat_id)
        {
            Some(existing) => *existing = session,
            None => sessions.sessions.push(session),
        }
        self.settle_failed_cloud_setups();
        cx.notify();
        self.refresh_cloud_sessions(cx);
    }

    /// Read every Cloud session once — then again every
    /// [`crate::cloud::SESSION_POLL`] only while one is being set up, woken
    /// or deleted. Never runs for an account without Cloud.
    pub fn refresh_cloud_sessions(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_enabled() {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.cloud_sessions_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = crate::cloud::fetch_sessions(engine.clone()).await;
                let settling = this
                    .update(cx, |state, cx| {
                        if let Ok(sessions) = result {
                            state.set_cloud_sessions(sessions, cx);
                        }
                        state
                            .cloud_sessions
                            .as_ref()
                            .is_some_and(crate::cloud::any_session_settling)
                            || state.send_waits_on_cloud()
                    })
                    .unwrap_or(false);
                if !settling {
                    break;
                }
                cx.background_executor()
                    .timer(crate::cloud::SESSION_POLL)
                    .await;
            }
        }));
    }

    /// The "@ device" tag for a space — shared by the space pickers' rows,
    /// the sidebar filter trigger, and the composer's space chip. Returns
    /// `(tag, offline)`; staleness renders as a disconnected GLYPH at the
    /// call sites (user request), never words in the tag. A sleeping Cloud
    /// device is not stale — it wakes when a chat opens there.
    pub fn space_device_tag(&self, space: &Space, now: DateTime<Utc>) -> (String, bool) {
        let offline = self.device_presence(&space.device_id, now).warns();
        let device = self
            .device_name(&space.device_id)
            .unwrap_or("Unknown device");
        (format!("@ {device}"), offline)
    }

    /// Does the selected space's folder have git? Drives the branch picker and
    /// the diff sidebar (owner-stamped, synced — no RPC).
    pub fn selected_space_git(&self) -> bool {
        self.selected_space_row().is_some_and(|s| s.git_detected)
    }

    /// Full display status for a chat (tab dots, Active list). A send in
    /// flight ([`Self::begin_pending_send`]) reads as Working — the queued
    /// command is as good as running.
    pub fn display_status_for(&self, chat: &Chat, now: DateTime<Utc>) -> ChatIndicator {
        if self.send_pending(&chat.id, now) {
            return ChatIndicator::Working;
        }
        display_status(chat, self.session_for(&chat.id), now)
    }

    /// The sidebar's Sessions list: every non-archived chat of a LIVE space,
    /// on any device — idle included — in pure recency order (status drives
    /// the dot, never the position; see [`sort_active`]).
    pub fn overview_chats(&self, now: DateTime<Utc>) -> Vec<(ChatIndicator, &Chat)> {
        let mut rows: Vec<(ChatIndicator, &Chat)> = self
            .visible_chats()
            .filter(|c| match c.space_id.as_deref() {
                // Project-less sessions are first-class rows.
                None => true,
                Some(id) => self.space_row(id).is_some(),
            })
            .map(|c| (self.display_status_for(c, now), c))
            .collect();
        sort_active(&mut rows);
        rows
    }

    /// The sidebar's active list exactly as it is drawn: [`Self::overview_chats`]
    /// narrowed to the current project filter. The jump shortcuts and their
    /// hints both count positions here, so neither can drift from the rows on
    /// screen.
    pub fn sidebar_chats(
        &self,
        now: DateTime<Utc>,
        space_filter: Option<&str>,
    ) -> Vec<(ChatIndicator, &Chat)> {
        let in_filter = self.project_filter(space_filter);
        self.overview_chats(now)
            .into_iter()
            .filter(|(_, chat)| in_filter(chat))
            .collect()
    }

    /// The sidebar's project filter matches by repository: it stores one
    /// checkout's space id, and a chat in ANY checkout of that repository
    /// (clones and worktrees on every device, [`zeron_proto::view::project_key`])
    /// matches. A filter naming a space not synced here matches its own id.
    pub fn project_filter<'a>(&'a self, filter: Option<&str>) -> impl Fn(&Chat) -> bool + 'a {
        let filter = filter.map(|id| {
            (
                id.to_owned(),
                self.space_row(id).map(zeron_proto::view::project_key),
            )
        });
        move |chat| {
            let Some((id, key)) = &filter else {
                return true;
            };
            match chat.space_id.as_deref() {
                Some(space_id) if space_id == id => true,
                Some(space_id) => key.as_ref().is_some_and(|key| {
                    self.space_row(space_id)
                        .is_some_and(|space| zeron_proto::view::project_key(space) == *key)
                }),
                None => false,
            }
        }
    }

    /// Every project — one per repository across devices — as its checkouts
    /// in [`Self::project_members`] order (this device's first), ordered by
    /// the project's name.
    pub fn projects(&self) -> Vec<Vec<&Space>> {
        let mut seen = std::collections::HashSet::new();
        let mut projects: Vec<Vec<&Space>> = self
            .spaces
            .iter()
            .filter(|space| seen.insert(zeron_proto::view::project_key(space)))
            .map(|space| self.project_members(space))
            .collect();
        projects.sort_by_key(|members| {
            let representative = self.representative_space(members[0]);
            (
                representative.display_name().to_lowercase(),
                representative.id.clone(),
            )
        });
        projects
    }

    /// A project's host tag: its one device as in [`Self::space_device_tag`],
    /// else the devices holding a checkout. Offline only when every one is.
    pub fn project_device_tag(&self, members: &[&Space], now: DateTime<Utc>) -> (String, bool) {
        let mut devices: Vec<&str> = Vec::new();
        for member in members {
            if !devices.contains(&member.device_id.as_str()) {
                devices.push(member.device_id.as_str());
            }
        }
        match devices.as_slice() {
            [] => (String::new(), false),
            [_] => self.space_device_tag(members[0], now),
            _ => {
                let offline = devices.iter().all(|d| !self.device_online(d, now));
                let tag = if devices.len() == 2 {
                    devices
                        .iter()
                        .map(|d| self.device_name(d).unwrap_or("Unknown device"))
                        .collect::<Vec<_>>()
                        .join(" · ")
                } else {
                    format!("{} devices", devices.len())
                };
                (format!("@ {tag}"), offline)
            }
        }
    }

    pub fn session_for(&self, chat_id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.chat_id == chat_id)
    }

    /// Subagents running under `chat_id` right now (the engine's count on its
    /// session row; 0 once that row goes stale). Every device can ask, opened
    /// chat or not.
    pub fn running_subagents_for(&self, chat_id: &str, now: DateTime<Utc>) -> u32 {
        running_subagents(self.session_for(chat_id), now)
    }

    /// Native run indicator for composer/Stop controls. Sidebar attention is
    /// derived separately by `display_status_for`, including terminal reports.
    pub fn indicator_for(&self, chat_id: &str, now: DateTime<Utc>) -> Indicator {
        if self.send_pending(chat_id, now) {
            return Indicator::Working;
        }
        // A Cloud turn's setup streams into the entry that becomes its answer:
        // working throughout, though the send is acked (the machine wrote the
        // message back) a beat before its session row reads Working.
        if self.selected_chat.as_deref() == Some(chat_id) && self.cloud_setup_streaming() {
            return Indicator::Working;
        }
        zeron_proto::view::native_indicator(self.session_for(chat_id), now)
    }

    /// The earliest moment after `now` at which a clock-derived value flips
    /// with no frame to announce it: a Working/AwaitingInput session going
    /// stale ([`SESSION_STALE_MS`]), a remote device leaving its online window
    /// (offline glyphs, unreachable hosts, Queued badges), a pending send's
    /// grace expiring into "Not delivered", or a syncing chat's caption
    /// showing or its room stalling. Each deadline is the first instant
    /// the predicate reads differently. Relative-time labels are excluded:
    /// they advance on the views' minute heartbeat. Pure.
    pub fn next_clock_transition(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.clock_deadlines()
            .filter(|deadline| *deadline > now)
            .min()
    }

    /// Whether any clock-driven transition fell in `(from, to]`. Heartbeats
    /// that move a deadline later without notifying (session and device
    /// liveness leases) leave an armed timer early; this tells a real
    /// transition from such an obsolete wake. Pure.
    fn clock_transition_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        self.clock_deadlines()
            .any(|deadline| from < deadline && deadline <= to)
    }

    fn clock_deadlines(&self) -> impl Iterator<Item = DateTime<Utc>> + '_ {
        let stale = self
            .sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.activity_status(),
                    zeron_proto::SessionStatus::Working | zeron_proto::SessionStatus::AwaitingInput
                )
            })
            // `age_ms > SESSION_STALE_MS` first holds one millisecond past it.
            .map(|session| {
                session.updated_at + chrono::TimeDelta::milliseconds(SESSION_STALE_MS + 1)
            });
        let offline = self
            .devices
            .iter()
            .filter(|device| self.local_device_id.as_deref() != Some(device.id.as_str()))
            .filter_map(|device| device.last_seen_at)
            // Whole seconds `<= DEVICE_ONLINE_WINDOW_SECS` stay online.
            .map(|at| {
                at + chrono::TimeDelta::seconds(
                    crate::settings::devices::DEVICE_ONLINE_WINDOW_SECS + 1,
                )
            });
        let grace = self.pending_sends.values().map(|pending| {
            pending.started + chrono::TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1)
        });
        // The open chat's caption appears, and a room stalls under it or
        // under a pending send, by clock alone; other chats' clocks change
        // nothing on screen. Only a room that is down can stall.
        let syncing = self
            .sync_clocks
            .iter()
            .filter(|(id, _)| {
                self.selected_chat.as_ref() == Some(*id) || self.pending_sends.contains_key(*id)
            })
            .filter(|(id, _)| {
                self.chats.iter().any(|chat| {
                    chat.id == **id
                        && Some(chat.device_id.as_str()) != self.local_device_id.as_deref()
                })
            })
            .flat_map(|(id, clock)| {
                let down = self
                    .connectivity
                    .chats
                    .iter()
                    .any(|chat| chat.chat_id == *id && !chat.connected);
                let shown = clock.began + chrono::TimeDelta::milliseconds(SYNC_SHOWN_AFTER_MS);
                let stalled = down
                    .then(|| clock.began + chrono::TimeDelta::milliseconds(SYNC_STALLED_AFTER_MS));
                std::iter::once(shown).chain(stalled)
            });
        stale.chain(offline).chain(grace).chain(syncing)
    }

    /// Notify this entity at every clock-driven transition, so each state
    /// observer (composer interrupts, session sounds, views) sees a session
    /// go stale, a device go dark or a send's grace expire on time — no state
    /// frame announces those. Idempotent; every notify re-arms a single
    /// one-shot timer for the next deadline, so idle state never polls.
    pub fn watch_clock_transitions(&mut self, cx: &mut Context<Self>) {
        if self.clock.observation.is_none() {
            self.clock.observation =
                Some(cx.observe_self(|state, cx| state.arm_clock_transition(cx)));
        }
        self.arm_clock_transition(cx);
    }

    /// Re-evaluate the clock-dependent parts of the change-detection caches.
    fn refresh_clock_presentations(&mut self, now: DateTime<Utc>) {
        self.session_presence_presentation = session_presence_at(&self.sessions, now);
        if self.device_presentation.is_some() {
            self.device_presentation = Some(device_presentation_at(&self.devices, now));
        }
    }

    fn arm_clock_transition(&mut self, cx: &mut Context<Self>) {
        let now = clock_now();
        self.clock.checked_at = Some(now);
        let wake = self.next_clock_transition(now);
        if wake == self.clock.wake_at && self.clock.task.is_some() {
            return;
        }
        self.clock.wake_at = wake;
        self.clock.task = wake.map(|wake| {
            let delay = (wake - now).to_std().unwrap_or_default();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |state, cx| {
                    state.clock.wake_at = None;
                    state.clock.task = None;
                    let now = clock_now();
                    let from = state.clock.checked_at.unwrap_or(now);
                    if state.clock_transition_between(from, now) {
                        // Record what this transition changed, so the frame
                        // that later reverts it (a heartbeat reviving the
                        // session or device) compares unequal and notifies.
                        state.refresh_clock_presentations(now);
                        // The self-observer re-arms for the next deadline.
                        cx.notify();
                    } else {
                        state.arm_clock_transition(cx);
                    }
                })
                .ok();
            })
        });
    }

    pub fn selected_chat_row(&self) -> Option<&Chat> {
        let id = self.selected_chat.as_deref()?;
        self.chats.iter().find(|c| c.id == id)
    }

    /// The chat the Archive session shortcut acts on: the selected one, unless
    /// it is already archived. The shortcut archives and never unarchives, so
    /// an archived chat is left alone. Pure.
    pub fn archivable_selected_chat(&self) -> Option<&str> {
        self.selected_chat_row()
            .filter(|chat| !chat.archived)
            .map(|chat| chat.id.as_str())
    }

    /// Latest valid PR for a chat, rechecked against device, checkout, cwd and branch.
    pub fn change_request_for_chat(&self, chat: &Chat) -> Option<&ChangeRequestSummary> {
        self.change_requests
            .change_request_for_chat(chat, &self.spaces)
    }

    pub fn gate(&self) -> GatePhase {
        gate_phase(&self.connection, self.workspace_scope, self.auth.as_ref())
    }

    pub fn engine(&self) -> Option<&EngineHandle> {
        self.engine.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_test_engine(&mut self, handle: EngineHandle) {
        self.engine = Some(handle);
    }

    /// Drop every account-scoped view and subscription after its runtime has
    /// stopped. The next bootstrap must never render rows from the previous
    /// account while the local profile is opening.
    pub fn prepare_runtime_replacement(&mut self, cx: &mut Context<Self>) {
        self.engine = None;
        self.watch_tasks.clear();
        self.transcript_task = None;
        self.change_request_tasks.clear();
        self.change_requests = ChangeRequestClientState::default();
        self.connection = ConnectionStatus::Connecting;
        self.workspace_scope = None;
        self.auth = None;
        self.devices.clear();
        self.device_presentation = None;
        self.session_presence_presentation.clear();
        self.spaces.clear();
        self.chats.clear();
        self.pending_side_chat = None;
        self.unsaved_side_chat = false;
        self.sessions.clear();
        self.sidebar_preferences = SidebarPreferencesState::default();
        self.session_presentation = None;
        self.selected_space = None;
        self.no_project = false;
        self.selected_device = None;
        self.canvas_target = None;
        self.selected_chat = None;
        self.auto_selected = false;
        self.chats_synced = false;
        self.spaces_synced = false;
        self.transcript.clear();
        self.transcript_baselines.clear();
        self.transcript_cache.clear();
        self.prepared_transcripts.clear();
        self.context_usage = None;
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript_replayed = false;
        self.echoes.clear();
        self.pending_sends.clear();
        self.upload_progress = None;
        self.transfers.clear();
        self.sync_clocks.clear();
        self.local_device_id = None;
        self.link_roots_revision = self.link_roots_revision.wrapping_add(1);
        cx.notify();
    }

    /// Independent selection and transcript subscriptions over the same engine.
    /// An `unsaved` chat is a hand-started side chat nothing has written yet:
    /// it opens no doc until its first send mints it (see
    /// [`Self::unsaved_side_chat_create`]).
    pub(crate) fn side_chat_state(
        parent: &Entity<Self>,
        chat: Chat,
        unsaved: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = parent.read(cx);
        let engine = source.engine.clone();
        let mut state = Self::new();
        state.chats = source.chats.clone();
        if !state.chats.iter().any(|c| c.id == chat.id) {
            state.chats.push(chat.clone());
        }
        state.spaces = source.spaces.clone();
        state.devices = source.devices.clone();
        state.data_dir = source.data_dir.clone();
        state.auto_selected = true;
        state.pending_side_chat = Some(chat.clone());
        state.unsaved_side_chat = unsaved;
        if let Some(engine) = engine {
            state.attach_engine(engine, cx);
        }
        state.select_chat(Some(chat.id), cx);
        state.watch_clock_transitions(cx);
        state
    }

    /// This is a side chat's state whose chat its first send has yet to mint.
    pub(crate) fn side_chat_unsaved(&self) -> bool {
        self.unsaved_side_chat
    }

    /// The harness can change until the first send snapshots it.
    /// Keep the harness fixed while createChat is in flight, including uploads.
    pub(crate) fn side_chat_harness_editable(&self) -> bool {
        self.unsaved_side_chat
            && self
                .selected_chat
                .as_ref()
                .is_some_and(|id| !self.pending_sends.contains_key(id))
    }

    /// A turn is streaming in this chat, or parked on a question it is still
    /// owed an answer to. Only the status row decides: a persistent session
    /// parked between turns reports idle.
    pub(crate) fn chat_turn_running(&self, chat_id: &str) -> bool {
        self.sessions.iter().any(|session| {
            session.chat_id == chat_id
                && matches!(
                    session.status,
                    zeron_proto::SessionStatus::Working | zeron_proto::SessionStatus::AwaitingInput
                )
        })
    }

    /// The selected chat may change provider right now: a saved chat between
    /// turns (its transcript is handed to the new provider), or a side-chat
    /// draft before its first send snapshots the harness.
    pub(crate) fn harness_switchable(&self) -> bool {
        if self.side_chat_harness_editable() {
            return true;
        }
        !self.unsaved_side_chat
            && self.selected_chat.as_ref().is_some_and(|id| {
                !self.pending_sends.contains_key(id)
                    && !self.chat_turn_running(id)
                    // An older host would resume the chat's stored session
                    // under the new provider.
                    && self.chat_host_supports(id, zeron_proto::capabilities::PROVIDER_SWITCH_V1)
            })
    }

    fn is_unsaved_side_chat(&self, chat_id: &str) -> bool {
        self.unsaved_side_chat
            && self
                .pending_side_chat
                .as_ref()
                .is_some_and(|chat| chat.id == chat_id)
    }

    /// The `Mutate createChat` params that mint the unsaved side chat
    /// `chat_id` on its first send; `None` once it exists.
    pub(crate) fn unsaved_side_chat_create(&self, chat_id: &str) -> Option<serde_json::Value> {
        if !self.is_unsaved_side_chat(chat_id) {
            return None;
        }
        let chat = self.pending_side_chat.as_ref()?;
        Some(serde_json::json!({
            "op": "createChat",
            "chatId": chat.id,
            "deviceId": chat.device_id,
            "spaceId": chat.space_id,
            "config": chat.config,
            "branch": chat.branch,
            "cwd": chat.cwd,
            "parentChatId": chat.parent_chat_id,
        }))
    }

    /// The unsaved side chat now exists: attach the doc watches it deferred.
    pub(crate) fn side_chat_saved(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if !self.is_unsaved_side_chat(chat_id) {
            return;
        }
        self.unsaved_side_chat = false;
        if self.selected_chat.as_deref() == Some(chat_id) {
            self.focus_chat_sync(chat_id, cx);
            self.start_chat_watches(chat_id.to_owned(), cx);
        }
    }

    fn start_chat_watches(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            return;
        };
        self.transcript_task = Some(spawn_transcript_watch(cx, handle.clone(), chat_id.clone()));
        if handle
            .engine_info()
            .supports(zeron_proto::capabilities::MESSAGE_QUEUE_V1)
        {
            self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
        }
    }

    // ---- gpui glue ----

    /// Kick off (or retry) the engine bootstrap: probe → connect-or-embed on
    /// tokio, then attach subscriptions. Safe to call again after `Failed`.
    pub fn bootstrap(state: Entity<AppState>, config: EngineBootConfig, cx: &mut App) {
        let data_dir = config.data_dir.clone();
        state.update(cx, |s, cx| {
            s.connection = ConnectionStatus::Connecting;
            s.workspace_scope = None;
            s.auth = None;
            s.data_dir = Some(data_dir);
            s.boot_config = Some(config.clone());
            // A deliberate start (launch, Retry, a runtime switch) begins
            // the respawn backoff afresh.
            s.engine_losses.clear();
            s.reconnect_task = None;
            cx.notify();
        });
        Self::connect(state, config, cx);
    }

    /// The next engine exit is the shell's doing (it stops the engine to
    /// replace the runtime): don't reconnect on it.
    pub fn expect_engine_exit(&mut self) {
        self.engine_exit_expected = true;
    }

    /// A separate engine went away unasked: reconnect — the bootstrap probe
    /// starts it again when nothing serves the port. What's on screen stays
    /// while it does; the new engine's watches replace it frame by frame.
    fn engine_connection_lost(&mut self, handle: &EngineHandle, cx: &mut Context<Self>) {
        let current = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.same_connection(handle));
        if !current || self.engine_exit_expected {
            return;
        }
        let Some(config) = self.boot_config.clone() else {
            return;
        };
        // A transport failure (an oversized frame, a reset socket) loses the
        // connection while the engine runs on; say which it was.
        let reason = handle.client().close_reason();
        let respawn = record_engine_loss(&mut self.engine_losses, std::time::Instant::now());
        self.engine = None;
        self.watch_tasks.clear();
        self.transcript_task = None;
        self.queue_task = None;
        self.change_request_tasks.clear();
        self.focus_task = None;
        let Some(delay) = respawn else {
            let log = config.data_dir.join("logs").join("zeron-engine.log");
            tracing::error!(
                losses = self.engine_losses.len(),
                reason = reason.as_deref().unwrap_or("the engine went away"),
                "the engine connection keeps dropping; giving up until Retry"
            );
            self.connection = ConnectionStatus::Failed(match reason {
                Some(reason) => format!(
                    "Zeron keeps losing its connection to the engine: {reason}. \
                     Details are in {}.",
                    log.display()
                ),
                None => format!(
                    "Zeron's engine keeps stopping unexpectedly. Details are in {}.",
                    log.display()
                ),
            });
            cx.notify();
            return;
        };
        tracing::warn!(?delay, ?reason, "lost the engine connection; reconnecting");
        self.reconnect_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = cx.update(|cx| {
                let Some(state) = this.upgrade() else {
                    return;
                };
                // The shell may have replaced the runtime meanwhile.
                let still_down = {
                    let state = state.read(cx);
                    state.engine.is_none() && !state.engine_exit_expected
                };
                if still_down {
                    Self::connect(state, config, cx);
                }
            });
        }));
        cx.notify();
    }

    /// Whether one of the app's windows is focused, for the engine's desktop
    /// banners ("only while Zeron isn't focused"). Repeated while focused, so
    /// a window that dies focused stops counting.
    pub fn set_viewport_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.viewport_focused == focused && self.focus_task.is_some() {
            return;
        }
        self.viewport_focused = focused;
        self.report_focus(cx);
    }

    fn report_focus(&mut self, cx: &mut Context<Self>) {
        const REPEAT: std::time::Duration = std::time::Duration::from_secs(20);
        let Some(engine) = self.engine.clone().filter(|engine| {
            engine
                .engine_info()
                .supports(zeron_proto::capabilities::DESKTOP_NOTIFICATIONS_V1)
        }) else {
            self.focus_task = None;
            return;
        };
        let viewport_id = self.viewport_id.clone();
        let focused = self.viewport_focused;
        self.focus_task = Some(cx.spawn(async move |_, cx| {
            loop {
                let params = serde_json::json!({ "viewportId": viewport_id, "focused": focused });
                if let Err(error) = engine
                    .client()
                    .call(methods::SET_VIEWPORT_FOCUS, params)
                    .await
                {
                    tracing::debug!(%error, "viewport focus report failed");
                }
                if !focused {
                    return;
                }
                cx.background_executor().timer(REPEAT).await;
            }
        }));
    }

    /// Tell the engine this app's windows are going away unfocused (the app
    /// is quitting), so banners aren't held back for a focus report that's
    /// about to go stale.
    pub fn viewport_closing(&self) -> Option<impl std::future::Future<Output = ()> + use<>> {
        let engine = self.engine.clone().filter(|engine| {
            engine
                .engine_info()
                .supports(zeron_proto::capabilities::DESKTOP_NOTIFICATIONS_V1)
        })?;
        let params = serde_json::json!({ "viewportId": self.viewport_id, "focused": false });
        Some(async move {
            let _ = engine
                .client()
                .call(methods::SET_VIEWPORT_FOCUS, params)
                .await;
        })
    }

    /// Whether the engine posts desktop banners itself (the app then posts
    /// none, so nothing doubles up).
    pub fn engine_posts_banners(&self) -> bool {
        self.engine.as_ref().is_some_and(|engine| {
            engine
                .engine_info()
                .supports(zeron_proto::capabilities::DESKTOP_NOTIFICATIONS_V1)
        })
    }

    fn connect(state: Entity<AppState>, config: EngineBootConfig, cx: &mut App) {
        let boot = Tokio::spawn(cx, EngineHandle::bootstrap(config));
        cx.spawn(async move |cx| {
            let outcome = match boot.await {
                Ok(Ok(handle)) => Ok(handle),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join_err) => Err(join_err.to_string()),
            };
            // NB: at the pinned rev `Entity::update(&mut AsyncApp)` returns the
            // closure's value directly (no Result) — AsyncApp implements
            // AppContext like App does.
            state.update(cx, |s, cx| match outcome {
                Ok(handle) => s.attach_engine(handle, cx),
                Err(message) => {
                    tracing::error!(%message, "engine bootstrap failed");
                    s.connection = ConnectionStatus::Failed(message);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Wire the connected engine: mark Ready and start the standing watches.
    /// Methods the engine doesn't serve yet (chats/devices/auth land with the
    /// workspace doc in M4) fail their subscribe and are skipped gracefully.
    fn attach_engine(&mut self, handle: EngineHandle, cx: &mut Context<Self>) {
        // The attachment notification precedes the first connectivity frame.
        // Make that bootstrap gap explicit so the shell resets its alert
        // baseline instead of comparing the new runtime with the old one.
        self.connectivity_observed = false;
        let engine_info = handle.engine_info();
        let supports_harness_updates =
            engine_info.supports(zeron_proto::capabilities::HARNESS_UPDATES_V1);
        self.workspace_scope = Some(engine_info.workspace_scope);
        self.local_device_id = Some(engine_info.device_id.clone());
        self.link_roots_revision = self.link_roots_revision.wrapping_add(1);
        if !supports_harness_updates {
            self.harness_updates.clear();
        }
        self.engine = Some(handle.clone());
        self.engine_exit_expected = false;
        let watched = handle.clone();
        self.connection_task = Some(cx.spawn(async move |this, cx| {
            watched.closed().await;
            this.update(cx, |state, cx| state.engine_connection_lost(&watched, cx))
                .ok();
        }));
        self.report_focus(cx);
        let mut watch_tasks = Vec::with_capacity(10);
        if let Some(task) = spawn_deferred_engine_watch(cx, handle.clone()) {
            watch_tasks.push(task);
        }
        watch_tasks.extend([
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_SESSIONS,
                AppState::apply_sessions,
            ),
            spawn_chats_watch(cx, handle.clone()),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_SIDEBAR_PREFERENCES,
                AppState::apply_sidebar_preferences,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_DEVICES,
                AppState::apply_devices,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_CONNECTIVITY,
                AppState::apply_connectivity,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_TRANSFERS,
                AppState::apply_transfers,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_SPACES,
                AppState::apply_spaces,
            ),
            // Auth frames parse tolerantly — engine and proto tags differ today.
            spawn_watch(
                cx,
                handle.clone(),
                methods::AUTH_STATUS,
                AppState::apply_auth_value,
            ),
            spawn_local_device_probe(cx, handle.clone()),
        ]);
        if supports_harness_updates {
            watch_tasks.push(spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_HARNESS_UPDATES,
                AppState::apply_harness_updates,
            ));
        }
        // Credits change on their own (use, renewals, purchases elsewhere):
        // keep the status the composer's Cloud gate reads current.
        watch_tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOUD_STATUS_POLL).await;
                if this
                    .update(cx, |state, cx| {
                        if state.cloud_enabled() {
                            state.refresh_cloud_status(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
        self.watch_tasks = watch_tasks;
        self.reconcile_change_request_watches(cx);
        // EngineInfo is part of the attachment boundary: views must know which
        // data profile they reached before they are allowed to render Ready.
        self.connection = ConnectionStatus::Ready;
        // Re-subscribe the transcript if a chat was already selected (reconnect path).
        if let Some(chat_id) = self.selected_chat.clone() {
            self.transcript_task =
                Some(spawn_transcript_watch(cx, handle.clone(), chat_id.clone()));
            if handle
                .engine_info()
                .supports(zeron_proto::capabilities::MESSAGE_QUEUE_V1)
            {
                self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
            }
        }
        // Whether this account can run sessions on Cloud, and on which
        // repositories (the checkout picker's Cloud option).
        self.refresh_cloud(cx);
        cx.notify();
    }

    /// Read Cloud's status, then — when it is on — the repositories it
    /// reaches. Best effort: an account without Cloud just has no option.
    pub fn refresh_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.cloud_repos_task = Some(cx.spawn(async move |this, cx| {
            // A failed read (offline, the edge failing for a moment) is tried
            // again: until it answers, Cloud reads as not offered, and
            // nothing else would ask again until a restart.
            let mut delay = std::time::Duration::from_secs(30);
            let mut status = loop {
                match crate::cloud::fetch_status(engine.clone()).await {
                    Ok(status) => break status,
                    Err(_) => {
                        cx.background_executor().timer(delay).await;
                        delay = (delay * 2).min(std::time::Duration::from_secs(300));
                    }
                }
            };
            // Cloud is part of every signed-in account: one not set up yet
            // is set up now. Nothing runs (or bills) until a chat does.
            if status.available
                && status.state == zeron_proto::CloudState::Off
                && let Ok(on) = engine
                    .client()
                    .call_as::<zeron_proto::CloudStatus>(
                        zeron_rpc::methods::CLOUD_ENABLE,
                        serde_json::json!({}),
                    )
                    .await
            {
                status = on;
            }
            let enabled = crate::cloud::account_enabled(&status);
            this.update(cx, |state, cx| {
                state.set_cloud_status(status, cx);
                if enabled {
                    state.refresh_cloud_repos(cx);
                } else if state.cloud_repos.take().is_some() {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Re-read the repositories Cloud reaches (Cloud turned on, GitHub
    /// connected, the GitHub App installed somewhere new).
    pub fn refresh_cloud_repos(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if !self.cloud_enabled() {
            return;
        }
        self.cloud_repos_refreshing = true;
        cx.notify();
        self.cloud_repos_task = Some(cx.spawn(async move |this, cx| {
            #[derive(serde::Deserialize)]
            struct Listing {
                #[serde(default)]
                repos: Vec<zeron_proto::GithubRepo>,
            }
            let repos = engine
                .client()
                .call_as::<Listing>(methods::LIST_GITHUB_REPOS, serde_json::json!({}))
                .await;
            this.update(cx, |state, cx| {
                state.cloud_repos_refreshing = false;
                // A failed read keeps what was known.
                if let Ok(listing) = repos {
                    state.cloud_repos = Some(listing.repos);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn cloud_repos_refreshing(&self) -> bool {
        self.cloud_repos_refreshing
    }

    /// The Cloud repository a project's GitHub origin names, when Cloud
    /// reaches it — the checkout picker then offers Cloud.
    pub fn cloud_repo_for(&self, space: &zeron_proto::Space) -> Option<&zeron_proto::GithubRepo> {
        // Cloud is offered only while it is on.
        if !self.cloud_enabled() {
            return None;
        }
        let name = space.github_repo.as_deref()?;
        self.cloud_repos
            .as_ref()?
            .iter()
            .find(|repo| repo.full_name.eq_ignore_ascii_case(name))
    }

    fn reconcile_change_request_watches(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            self.change_request_tasks.clear();
            return;
        };
        let targets = if self.change_requests_visible {
            desired_watch_targets(&self.chats, &self.spaces, |device| {
                !self.change_requests.is_supported(device)
            })
        } else {
            HashSet::new()
        };

        self.change_request_tasks
            .retain(|target, _| targets.contains(target));
        self.change_requests.retain_targets(&targets);

        let local_device_id = self.local_device_id.clone();
        for target in targets {
            if self.change_request_tasks.contains_key(&target) {
                continue;
            }
            let task = spawn_change_request_watch(
                cx,
                handle.clone(),
                target.clone(),
                local_device_id.clone(),
            );
            self.change_request_tasks.insert(target, task);
        }
    }

    pub fn set_change_requests_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.change_requests_visible != visible {
            self.change_requests_visible = visible;
            self.reconcile_change_request_watches(cx);
        }
    }

    pub fn open_deep_link(&mut self, url: &str, cx: &mut Context<Self>) {
        match crate::links::parse_zeron_conversation_link(url) {
            Ok(link) => {
                self.pending_deep_link = Some(link);
                self.apply_pending_deep_link(cx);
            }
            Err(error) => self.deep_link_notice = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Returns whether a pending link was resolved (selected or rejected).
    fn apply_pending_deep_link(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(link) = self.pending_deep_link.clone() else {
            return false;
        };
        let Some(locator) = crate::links::workspace_locator(
            self.workspace_scope,
            self.auth.as_ref(),
            self.local_device_id.as_deref(),
        ) else {
            return false;
        };
        if locator != link.workspace {
            self.pending_deep_link = None;
            self.deep_link_notice =
                Some("This conversation link belongs to another workspace".into());
            return true;
        }
        if self.chats.iter().any(|chat| chat.id == link.chat_id) {
            self.pending_deep_link = None;
            self.select_chat(Some(link.chat_id), cx);
            true
        } else if self.chats_synced {
            self.pending_deep_link = None;
            self.deep_link_notice = Some("The linked conversation was not found".into());
            true
        } else {
            false
        }
    }

    pub fn take_deep_link_notice(&mut self) -> Option<String> {
        self.deep_link_notice.take()
    }

    /// Select a chat (or clear). Swaps the per-chat doc-transcript subscription:
    /// dropping the old task drops its stream receiver, which cancels the doc
    /// watch server-side. Selecting a chat also lands in its space and marks it
    /// seen (a global-list click must switch the tab strip too).
    pub fn select_chat(&mut self, chat_id: Option<String>, cx: &mut Context<Self>) {
        if let Some(id) = &chat_id
            && !self.is_unsaved_side_chat(id)
        {
            self.focus_chat_sync(id, cx);
        }
        if self.selected_chat == chat_id {
            // Re-selecting still clears a fresh "completed" badge.
            if let Some(id) = chat_id {
                self.mark_chat_seen(&id, cx);
            }
            return;
        }
        if let Some(clock) = chat_id.as_ref().and_then(|id| self.sync_clocks.get_mut(id)) {
            clock.began = clock_now();
        }
        match (&self.selected_chat, &chat_id) {
            (None, Some(_)) => self.canvas_target = Some(self.current_canvas_target()),
            (Some(_), None) => self.restore_canvas_target(),
            _ => {}
        }
        // Take the destination before trimming: switching to the oldest warm
        // transcript must not evict the very entry we are about to display.
        let cached = self
            .transcript_cache
            .iter()
            .position(|cached| Some(&cached.chat_id) == chat_id.as_ref())
            .and_then(|index| self.transcript_cache.remove(index));
        if let Some(previous) = &self.selected_chat {
            let old_baseline = self.transcript_baselines.remove(previous);
            cx.background_executor()
                .spawn(async move {
                    drop(old_baseline);
                })
                .detach();
            let prepared = self.prepared_transcripts.remove(previous);
            if self.transcript_replayed {
                let entries = std::mem::take(&mut self.transcript);
                let bytes = prepared.as_ref().map_or_else(
                    || {
                        entries
                            .iter()
                            .map(|entry| {
                                std::mem::size_of::<SessionMessageEntry>()
                                    + entry.id.len()
                                    + entry
                                        .parts
                                        .iter()
                                        .map(|part| {
                                            std::mem::size_of::<zeron_doc::MessagePart>()
                                                + part.byte_len()
                                        })
                                        .sum::<usize>()
                            })
                            .sum()
                    },
                    |p| p.bytes,
                );
                self.transcript_cache.push_back(CachedTranscript {
                    prepared,
                    chat_id: previous.clone(),
                    entries,
                    context_usage: self.context_usage,
                    bytes,
                });
                while self.transcript_cache.len() > TRANSCRIPT_CACHE_CAP
                    || self
                        .transcript_cache
                        .iter()
                        .map(|cached| cached.bytes)
                        .sum::<usize>()
                        > TRANSCRIPT_CACHE_BYTES
                {
                    let evicted = self.transcript_cache.pop_front();
                    cx.background_executor()
                        .spawn(async move {
                            drop(evicted);
                        })
                        .detach();
                }
            }
        }
        self.selected_chat = chat_id.clone();
        // The chat left no longer acks its send from its transcript, and its
        // row may already show the turn that runs it.
        self.adopt_pending_sends(Utc::now());
        self.auto_selected = true;
        self.transcript.clear();
        self.context_usage = None;
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript_replayed = false;
        if let Some(cached) = cached {
            self.transcript_baselines.insert(
                cached.chat_id.clone(),
                cached
                    .prepared
                    .as_ref()
                    .map(|p| p.navigation_baseline.clone())
                    .unwrap_or_else(|| {
                        Arc::new(zeron_doc::TranscriptBaseline::capture(&cached.entries))
                    }),
            );
            if let Some(prepared) = cached.prepared {
                self.prepared_transcripts.insert(cached.chat_id, prepared);
            }
            self.transcript = cached.entries;
            self.context_usage = cached.context_usage;
            self.transcript_replayed = true;
        }
        self.transcript_task = None;
        self.queue.clear();
        self.queue_task = None;
        if let Some(id) = chat_id.as_deref() {
            // A chat implies its project (or the lack of one); `select_chat(None)`
            // (the new-session canvas) restores the pick set aside above.
            if let Some(chat) = self.chats.iter().find(|c| c.id == id) {
                match chat.space_id.clone() {
                    Some(space_id) => {
                        self.selected_space = Some(space_id);
                        self.no_project = false;
                    }
                    None => {
                        self.selected_space = None;
                        self.no_project = true;
                        self.selected_device = Some(chat.device_id.clone());
                    }
                }
            }
            self.mark_chat_seen(id, cx);
        }
        if let Some(chat_id) = chat_id {
            if self.is_unsaved_side_chat(&chat_id) {
                // Nothing to watch yet, and opening its doc would start one.
                self.transcript_replayed = true;
            } else {
                self.start_chat_watches(chat_id, cx);
            }
        }
        cx.notify();
    }

    /// Replace the selected chat's queue subscription without clearing its
    /// current projection. This is used after an optimistic mutation fails:
    /// the authoritative opening frame repairs the local list even though the
    /// document itself did not change and therefore emitted no new frame.
    pub(crate) fn refresh_selected_queue(&mut self, cx: &mut Context<Self>) {
        self.queue_task = None;
        let (Some(chat_id), Some(handle)) = (self.selected_chat.clone(), self.engine.clone())
        else {
            return;
        };
        if handle
            .engine_info()
            .supports(zeron_proto::capabilities::MESSAGE_QUEUE_V1)
        {
            self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
        }
    }

    /// Select a project; the caller (shell) decides which chat to land on.
    /// `Some` clears a "Don't work in a project" opt-out and re-aims the
    /// device pick at the project's host; `None` IS that opt-out.
    pub fn select_space(&mut self, space_id: Option<String>, cx: &mut Context<Self>) {
        match &space_id {
            Some(id) => {
                self.no_project = false;
                if let Some(device) = self.space_row(id).map(|s| s.device_id.clone()) {
                    self.selected_device = Some(device);
                }
            }
            None => {
                self.selected_device = self.effective_device_id();
                self.no_project = true;
            }
        }
        if self.selected_space == space_id && space_id.is_some() {
            cx.notify();
            return;
        }
        self.selected_space = space_id;
        cx.notify();
    }

    /// Synced seen marker: only fires when the chat is currently unseen
    /// (idempotence — no mutate spam), stamps the local row optimistically so
    /// the LWW round-trip is invisible, and fire-and-forgets the mutate.
    /// Window-focus liveness sweep: ask the engine to probe every open room
    /// (workspace + chat docs). Fire-and-forget; each room ignores the hint
    /// unless it has been broadcast-quiet ≥30s, so spamming is harmless.
    pub fn probe_sync(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            return;
        };
        cx.spawn(async move |_, _| {
            let params = serde_json::json!({});
            if let Err(err) = handle.client().call(methods::PROBE_SYNC, params).await {
                tracing::debug!(error = %err, "probe sync failed");
            }
        })
        .detach();
    }

    pub fn mark_chat_seen(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let unseen = self
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| {
                zeron_proto::view::has_unseen_activity(chat, self.session_for(chat_id))
            });
        let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) else {
            return;
        };
        if !unseen {
            return;
        }
        chat.last_seen_at = Some(Utc::now());
        cx.notify();
        let Some(handle) = self.engine.clone() else {
            return;
        };
        let chat_id = chat_id.to_string();
        cx.spawn(async move |_, _| {
            let params = serde_json::json!({ "op": "markChatSeen", "chatId": chat_id });
            if let Err(err) = handle.client().call(methods::MUTATE, params).await {
                tracing::warn!(chat = %chat_id, error = %err, "markChatSeen failed");
            }
        })
        .detach();
    }

    /// Navigation signal is separate from watch lifetime: resubscribing,
    /// receiving agent output and MCP reads cannot refresh focus priority.
    pub(crate) fn focus_chat_sync(&self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            return;
        };
        let chat_id = chat_id.to_owned();
        cx.spawn(async move |_, _| {
            if let Err(error) = handle
                .client()
                .call(
                    methods::FOCUS_CHAT,
                    serde_json::json!({ "chatId": chat_id }),
                )
                .await
            {
                tracing::debug!(%chat_id, %error, "chat focus sync hint unavailable");
            }
        })
        .detach();
    }

    pub(crate) fn focus_subagent_sync(&self, doc_id: &str, cx: &mut Context<Self>) {
        // Frozen transcripts are static blobs: navigation must not open their
        // old room just to report focus.
        if self.sub_watch_tasks.contains_key(doc_id) {
            self.focus_chat_sync(doc_id, cx);
        }
    }
}

/// Observe assembly after an early attach (cloud onboarding or another viewport
/// reaching the embedded engine over IPC). Data subscriptions wait on the same
/// result, but their individual errors are not authoritative: older engines may
/// legitimately omit a watch method. Only the assembly result may fail the
/// whole connection.
fn spawn_deferred_engine_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
) -> Option<Task<()>> {
    let mut deferred = handle.deferred_state()?;
    Some(cx.spawn(async move |this, cx| {
        let Err(failure) = wait_for_deferred_engine(&mut deferred).await else {
            return;
        };
        tracing::error!(error = %failure, "engine assembly failed after attachment");
        // Embedded handles release their IPC listener before exposing Retry;
        // remote handles stop their completed readiness probe.
        handle.shutdown().await;
        this.update(cx, |state, cx| {
            state.connection = ConnectionStatus::Failed(failure);
            cx.notify();
        })
        .ok();
    }))
}

/// Chats watch. Boot selection is the shell's job (it lands on the first
/// restored open tab, device-local state this entity can't see); this task
/// only pumps frames.
fn spawn_chats_watch(cx: &mut Context<AppState>, handle: EngineHandle) -> Task<()> {
    cx.spawn(async move |this, cx| {
        // Resubscribe loop (same contract as the transcript watch): a daemon
        // restart or RPC drop ends the stream, and a bare return here froze
        // the sidebar until app restart — new chats, renames and archives
        // from every device silently stopped arriving.
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            let mut rx = match handle
                .client()
                .subscribe(methods::WATCH_CHATS, serde_json::json!({}))
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(error = %err, "chats watch unavailable; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue;
                }
            };
            while let Some(value) = rx.recv().await {
                let parsed: Vec<Chat> = match serde_json::from_value(value) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        tracing::warn!(error = %err, "dropping malformed chats frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    // Identical re-published lists skip the re-sort fan-out:
                    // watch reconciliation and a whole-shell re-render.
                    let changed = state.apply_chats(parsed);
                    let linked = state.apply_pending_deep_link(cx);
                    if changed {
                        state.reconcile_change_request_watches(cx);
                    }
                    if changed || linked {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            tracing::debug!("chats stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

pub use zeron_proto::version_triple;

fn spawn_change_request_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    target: ChangeRequestWatchKey,
    local_device_id: Option<String>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        // A host that rejects the checkout (a removed worktree, a folder it
        // no longer knows) keeps rejecting it, so failed subscribes back off.
        const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(60);
        let mut retry_delay = RETRY_DELAY;
        loop {
            let params = watch_params(&target, local_device_id.as_deref());

            let mut subscription = match handle
                .client()
                .subscribe_checked(methods::WATCH_CHECKOUT_CHANGE_REQUEST, params)
                .await
            {
                Ok(subscription) => {
                    retry_delay = RETRY_DELAY;
                    subscription
                }
                Err(RpcError::UnknownMethod(_)) => {
                    tracing::debug!(
                        device = %target.device_id,
                        "checkout change requests unsupported on device"
                    );
                    this.update(cx, |state, cx| {
                        let engine_version = state
                            .devices
                            .iter()
                            .find(|device| device.id == target.device_id)
                            .and_then(|device| device.version.clone());
                        state
                            .change_requests
                            .mark_unsupported(target.device_id.clone(), engine_version);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(err) => {
                    tracing::debug!(
                        device = %target.device_id,
                        cwd = %target.cwd,
                        error = %err,
                        "checkout change request watch unavailable; retrying"
                    );
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                    continue;
                }
            };

            while let Some(value) = subscription.recv().await {
                let snapshot: CheckoutChangeRequestStatus = match serde_json::from_value(value) {
                    Ok(snapshot) => snapshot,
                    Err(err) => {
                        tracing::warn!(
                            device = %target.device_id,
                            cwd = %target.cwd,
                            error = %err,
                            "dropping malformed checkout change request frame"
                        );
                        continue;
                    }
                };
                if this
                    .update(cx, |state, cx| {
                        if state.change_requests.store(target.clone(), snapshot) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }

            // Preserve the latest successful snapshot during a transport gap.
            tracing::debug!(
                device = %target.device_id,
                cwd = %target.cwd,
                "checkout change request stream ended; resubscribing"
            );
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

fn spawn_watch<T: DeserializeOwned + 'static>(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    method: &'static str,
    apply: fn(&mut AppState, T) -> bool,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        // Resubscribe loop: these are the standing Sessions/Devices/Spaces
        // watches — a daemon restart ended the stream and a bare return froze
        // them for the rest of the app's life (remote Working dots staled out
        // to nothing after 45s, and Idle/Completed transitions from other
        // devices never arrived again — "the session never completes").
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            let mut rx = match handle
                .client()
                .subscribe(method, serde_json::json!({}))
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(method, error = %err, "watch unavailable; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue;
                }
            };
            while let Some(value) = rx.recv().await {
                let parsed: T = match serde_json::from_value(value) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        tracing::warn!(method, error = %err, "dropping malformed watch frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    let changed = apply(state, parsed);
                    let linked = state.apply_pending_deep_link(cx);
                    if changed && matches!(method, methods::WATCH_SPACES | methods::WATCH_DEVICES) {
                        state.reconcile_change_request_watches(cx);
                    }
                    if changed || linked {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            tracing::debug!(method, "watch stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// Best-effort `LocalDevice` probe: fills `local_device_id` for the "This
/// device" badge. Engines that don't serve the method leave it `None`.
fn spawn_local_device_probe(cx: &mut Context<AppState>, handle: EngineHandle) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let Ok(value) = handle
            .client()
            .call("LocalDevice", serde_json::json!({}))
            .await
        else {
            tracing::debug!("LocalDevice unavailable; skipping this-device badge");
            return;
        };
        let id = value
            .get("id")
            .or_else(|| value.get("deviceId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(id) = id {
            this.update(cx, |state, cx| {
                state.local_device_id = Some(id);
                state.link_roots_revision = state.link_roots_revision.wrapping_add(1);
                state.apply_pending_deep_link(cx);
                // Watches opened before this probe conservatively route through
                // targetDeviceId. Recreate them now that local routing is known.
                state.change_request_tasks.clear();
                state.reconcile_change_request_watches(cx);
                cx.notify();
            })
            .ok();
        }
    })
}

fn spawn_transcript_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
) -> Task<()> {
    // The scoped subscription cancels even a silent server stream on drop;
    // otherwise a deselected chat would retain its protected sync slot.
    cx.spawn(async move |this, cx| {
        // Outer loop: a delta desync (missed frame) resubscribes immediately
        // and the fresh stream's opening reset heals the copy; a subscribe
        // failure, malformed frame, or stream end retries on a delay. Every
        // path re-enters the loop — a return here freezes the transcript
        // with no banner and no heal short of an app restart (this watch and
        // its engine-side room are the ONLY transcript delivery path). The
        // task itself is dropped by select_chat/apply_chats when the chat is
        // deselected or deleted, so retrying can't outlive relevance.
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": chat_id, "openingTail": true });
            let mut rx = match handle
                .client()
                .subscribe_checked(methods::WATCH_DOC_MESSAGES, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::warn!(%chat_id, error = %err, "transcript watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            let mut preparation = WatchPreparation::new(cx.background_executor().clone());
            while let Some(value) = rx.recv().await {
                let history_pending = value
                    .get("historyPending")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let decoded = cx
                    .background_executor()
                    .spawn(async move {
                        let update: zeron_doc::TranscriptUpdate =
                            serde_json::from_value(value).map_err(|e| e.to_string())?;
                        let prepared = preparation.prepare(&update).map_err(|e| e.to_string())?;
                        Ok::<_, String>((update, prepared, preparation))
                    })
                    .await;
                let (update, prepared, next_preparation) = match decoded {
                    Ok(frame) => frame,
                    Err(err) => {
                        // Schema skew (a newer peer's entry shape arriving
                        // through sync): a skipped frame is a silently stale
                        // copy, so resubscribe for a fresh reset — delayed,
                        // in case the reset itself is what can't parse.
                        tracing::warn!(error = %err, "malformed transcript frame; resubscribing");
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue 'resubscribe;
                    }
                };
                preparation = next_preparation;
                let mut desync = false;
                let alive = this.update(cx, |state, cx| {
                    // Guard against a stale pump racing a newer selection.
                    if state.selected_chat.as_deref() == Some(chat_id.as_str()) {
                        if history_pending && state.transcript_replayed {
                            return;
                        }
                        if let Err(err) =
                            state.receive_opening_transcript_update(update, history_pending, cx)
                        {
                            tracing::warn!(%chat_id, error = %err, "resubscribing transcript");
                            desync = true;
                        } else {
                            state.prepared_transcripts.insert(chat_id.clone(), prepared);
                        }
                    }
                });
                if alive.is_err() {
                    return;
                }
                if desync {
                    continue 'resubscribe;
                }
            }
            // Stream ended: engine restart, RPC drop, or chat purge. Retry;
            // the purge case is cleaned up by apply_chats dropping this task.
            tracing::debug!(%chat_id, "transcript stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// The selected chat's pending-message queue, straight off its doc. Whole-list
/// frames (the queue is a handful of rows at most), retried like the transcript
/// watch — a queue that silently stopped updating would show messages the host
/// has already sent.
fn spawn_queue_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
) -> Task<()> {
    #[derive(serde::Deserialize)]
    struct QueueFrame {
        #[serde(default)]
        items: Vec<zeron_doc::QueuedMessage>,
    }
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": chat_id });
            let mut rx = match handle
                .client()
                .subscribe_checked(methods::WATCH_QUEUE, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(%chat_id, error = %err, "queue watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            while let Some(value) = rx.recv().await {
                let frame: QueueFrame = match serde_json::from_value(value) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(error = %err, "dropping malformed queue frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    // Guard against a stale pump racing a newer selection.
                    if state.selected_chat.as_deref() == Some(chat_id.as_str()) {
                        state.apply_queue(frame.items);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// [`spawn_transcript_watch`]'s shape, writing into `sub_transcripts[doc_id]`
/// instead of the selected chat's transcript. The apply guard is per key so a
/// subagent tab can outlive chat switches.
fn spawn_subagent_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    doc_id: String,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": doc_id });
            let mut rx = match handle
                .client()
                .subscribe_checked(methods::WATCH_DOC_MESSAGES, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::warn!(%doc_id, error = %err, "subagent watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            let mut preparation = WatchPreparation::new(cx.background_executor().clone());
            while let Some(value) = rx.recv().await {
                let decoded = cx
                    .background_executor()
                    .spawn(async move {
                        let update: zeron_doc::TranscriptUpdate =
                            serde_json::from_value(value).map_err(|e| e.to_string())?;
                        let prepared = preparation.prepare(&update).map_err(|e| e.to_string())?;
                        Ok::<_, String>((update, prepared, preparation))
                    })
                    .await;
                let (update, prepared, next_preparation) = match decoded {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(error = %err, "malformed subagent frame; resubscribing");
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue 'resubscribe;
                    }
                };
                preparation = next_preparation;
                let mut desync = false;
                let alive = this.update(cx, |state, cx| {
                    // A stale pump racing a snapshot/unwatch finds no key.
                    if let Some(rows) = state.sub_transcripts.get_mut(&doc_id) {
                        let frame = update.frame;
                        let text_only = is_text_append(&frame);
                        state.transcript_revision = state.transcript_revision.wrapping_add(1);
                        if let Err(err) = zeron_doc::apply_transcript_frame(rows, frame) {
                            tracing::warn!(%doc_id, error = %err, "resubscribing subagent watch");
                            desync = true;
                        }
                        if !desync {
                            state.prepared_transcripts.insert(doc_id.clone(), prepared);
                        }
                        if !desync && let Some(baseline) = update.replay_baseline {
                            state
                                .transcript_baselines
                                .insert(doc_id.clone(), Arc::new(baseline));
                        }
                        if text_only && !desync {
                            cx.emit(TranscriptTextChanged {
                                doc_id: doc_id.clone(),
                            });
                        } else {
                            cx.notify();
                        }
                    }
                });
                if alive.is_err() {
                    return;
                }
                if desync {
                    continue 'resubscribe;
                }
            }
            tracing::debug!(%doc_id, "subagent stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use gpui::AppContext;
    use zeron_engine::{EngineCore, default_registry};
    use zeron_rpc::RpcReply;
    // `SessionStatus` is only needed to build the fixtures below — the module
    // itself derives everything through `zeron_proto::view`.
    use zeron_proto::{DoneStatus, SessionStatus, UserProfile};

    /// A localhost port that was just free (bind :0, read, drop).
    async fn free_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    struct LegacyIdentityRpc;

    #[async_trait]
    impl RpcService for LegacyIdentityRpc {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            match method {
                methods::LOCAL_DEVICE => {
                    RpcReply::value(&serde_json::json!({ "deviceId": "legacy-device" }))
                }
                other => Err(RpcError::UnknownMethod(other.into())),
            }
        }
    }

    struct DeferredIdentityRpc {
        engine_info: EngineInfo,
        state: tokio::sync::watch::Receiver<DeferredEngineState>,
    }

    #[async_trait]
    impl RpcService for DeferredIdentityRpc {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            match method {
                methods::ENGINE_INFO => RpcReply::value(&self.engine_info),
                methods::ENGINE_READY => {
                    let mut state = self.state.clone();
                    wait_for_deferred_engine(&mut state)
                        .await
                        .map_err(RpcError::Failed)?;
                    RpcReply::value(&serde_json::json!({ "ready": true }))
                }
                other => Err(RpcError::UnknownMethod(other.into())),
            }
        }
    }

    #[tokio::test]
    async fn legacy_daemon_identity_falls_back_to_synced_scope() {
        let client = memory_client(Arc::new(LegacyIdentityRpc));

        let info = query_engine_info(&client).await.unwrap();

        assert_eq!(info.device_id, "legacy-device");
        assert_eq!(info.workspace_scope, WorkspaceScope::Synced);
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(info.workspace_scope),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::SignIn
        );
    }

    #[tokio::test]
    async fn remote_viewport_treats_legacy_daemon_as_ready() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        // The viewport reads the daemon's IPC bearer from its data dir.
        let token = zeron_rpc::ipc_auth::issue_token(dir.path()).unwrap();
        let server = tokio::spawn(zeron_rpc::serve_ws_listener(
            listener,
            Arc::new(LegacyIdentityRpc),
            token,
        ));
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .expect("legacy daemon remains attachable");

        let mut deferred = handle
            .deferred_state()
            .expect("remote viewport tracks readiness");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_deferred_engine(&mut deferred),
        )
        .await
        .expect("legacy readiness fallback completes")
        .expect("unknown EngineReady means the old daemon is assembled");

        handle.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn bootstrap_embeds_engine_when_port_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: free_port().await,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None, // offline
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .unwrap();
        assert_eq!(handle.mode(), EngineMode::InProcess);
        assert!(matches!(
            handle
                .deferred_state()
                .expect("embedded lifecycle")
                .borrow()
                .clone(),
            DeferredEngineState::Ready
        ));
        // Same protocol over the in-memory transport: a real engine answers.
        let harnesses = handle
            .client()
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap();
        assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn bootstrap_reports_local_assembly_failure_before_returning_a_handle() {
        let dir = tempfile::tempdir().unwrap();
        zeron_engine::EngineProfile::local(dir.path()).unwrap();
        std::fs::create_dir(dir.path().join("profiles")).unwrap();
        std::fs::write(dir.path().join("profiles/local"), b"not a directory").unwrap();
        let port = free_port().await;

        let error = match EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: Some("client_test".into()),
            default_harness: HarnessId::Mock,
        })
        .await
        {
            Ok(handle) => {
                handle.shutdown().await;
                panic!("a corrupt local store must fail bootstrap")
            }
            Err(error) => error,
        };

        assert!(!format!("{error:#}").is_empty());
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err(),
            "failed bootstrap must release the IPC listener"
        );
    }

    #[tokio::test]
    async fn deferred_engine_failure_remains_observable_after_early_attach() {
        let (state_tx, mut state_rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
        state_tx.send_replace(DeferredEngineState::Failed("store failed".into()));

        assert_eq!(
            wait_for_deferred_engine(&mut state_rx).await,
            Err("store failed".into())
        );
    }

    #[tokio::test]
    async fn remote_viewport_observes_deferred_engine_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (state_tx, state_rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
        let dir = tempfile::tempdir().unwrap();
        let token = zeron_rpc::ipc_auth::issue_token(dir.path()).unwrap();
        let server = tokio::spawn(zeron_rpc::serve_ws_listener(
            listener,
            Arc::new(DeferredIdentityRpc {
                engine_info: EngineInfo {
                    device_id: "owner-device".into(),
                    workspace_scope: WorkspaceScope::Local,
                    cursor_sdk_version: None,
                    capabilities: zeron_proto::capabilities::current(),
                },
                state: state_rx,
            }),
            token,
        ));

        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .expect("second viewport attaches over IPC");
        assert!(matches!(handle.mode(), EngineMode::Remote { .. }));

        let mut deferred = handle
            .deferred_state()
            .expect("remote viewport tracks engine readiness");
        state_tx.send_replace(DeferredEngineState::Failed("store failed".into()));
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                wait_for_deferred_engine(&mut deferred),
            )
            .await
            .expect("remote readiness probe completes"),
            Err("store failed".into())
        );

        handle.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn an_embedded_engine_serves_the_ipc_port_for_other_viewports() {
        // The whole point of embedding-and-serving: a second viewport (the
        // terminal app) can attach to this window's engine with no setup, no
        // separate daemon, and no launch ordering.
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None, // offline
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .unwrap();
        assert_eq!(handle.mode(), EngineMode::InProcess);

        // Attach the way an external viewport would, and speak the same protocol.
        // Without the engine's bearer the port refuses to serve.
        assert!(
            zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{port}"), None)
                .await
                .is_err(),
            "the IPC port must not serve a dial without the engine's token"
        );
        let attached = zeron_rpc::connect_ipc(port, Some(dir.path()))
            .await
            .expect("a second viewport must be able to attach");
        let harnesses = attached
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap();
        assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));

        // Shutting the window down stops accepting, so the next viewport
        // starts its own engine rather than talking to closing stores.
        handle.shutdown().await;
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err(),
            "the port must be released on shutdown"
        );
    }

    #[tokio::test]
    async fn concurrent_bootstraps_elect_one_embedded_engine() {
        // Two viewports of one app booting at once (the Local-switch restart
        // path): both used to probe a closed port, both embedded, and one lost
        // the data-dir lock. The bootstrap gate must elect exactly one owner
        // and turn the other into a plain remote attach.
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        let config = EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None, // offline
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        };
        let (a, b) = tokio::join!(
            EngineHandle::bootstrap(config.clone()),
            EngineHandle::bootstrap(config.clone()),
        );
        let a = a.expect("first viewport boots");
        let b = b.expect("second viewport boots");

        let modes = [a.mode(), b.mode()];
        assert_eq!(
            modes
                .iter()
                .filter(|mode| **mode == EngineMode::InProcess)
                .count(),
            1,
            "exactly one viewport embeds: {modes:?}"
        );
        assert_eq!(
            modes
                .iter()
                .filter(|mode| matches!(mode, EngineMode::Remote { .. }))
                .count(),
            1,
            "the other attaches over IPC: {modes:?}"
        );

        for handle in [&a, &b] {
            let mut deferred = handle.deferred_state().expect("lifecycle tracked");
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                wait_for_deferred_engine(&mut deferred),
            )
            .await
            .expect("readiness resolves")
            .expect("both viewports reach Ready");
        }

        b.shutdown().await;
        a.shutdown().await;
    }

    #[tokio::test]
    async fn the_background_engine_starts_only_on_a_free_real_port() {
        let free = free_port().await;
        assert!(EngineHandle::should_launch_background(true, free).await);
        assert!(!EngineHandle::should_launch_background(false, free).await);
        // Port 0 has no address a background engine could be found at.
        assert!(!EngineHandle::should_launch_background(true, 0).await);
        // A stranger on the port would keep it from ever serving.
        let squatter = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let taken = squatter.local_addr().unwrap().port();
        assert!(!EngineHandle::should_launch_background(true, taken).await);
    }

    #[test]
    fn a_lost_engine_respawns_with_backoff_then_stops_to_show_the_failure() {
        use std::time::Duration;
        let start = std::time::Instant::now();
        let mut losses = Vec::new();
        assert_eq!(
            record_engine_loss(&mut losses, start),
            Some(Duration::from_secs(1))
        );
        // Spread out, losses age away: an update restart now and then
        // always gets the short delay.
        assert_eq!(
            record_engine_loss(
                &mut losses,
                start + ENGINE_LOSS_WINDOW + Duration::from_secs(1)
            ),
            Some(Duration::from_secs(1))
        );
        let burst = start + Duration::from_secs(3600);
        let delays: Vec<_> = (0..5)
            .map(|n| record_engine_loss(&mut losses, burst + Duration::from_secs(n)))
            .collect();
        assert_eq!(
            delays,
            vec![
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(5)),
                Some(Duration::from_secs(15)),
                None,
            ]
        );
    }

    #[tokio::test]
    async fn a_stranger_on_the_ipc_port_does_not_wedge_the_window() {
        // The port probe only proves *something* is listening. A process that
        // accepts TCP and never speaks WebSocket used to hang the dial forever;
        // now it times out and we embed instead, losing only the ability to
        // serve other viewports.
        let squatter = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = squatter.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .expect("a taken port must not fail the boot");
        assert_eq!(handle.mode(), EngineMode::InProcess);
        assert!(
            handle
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .is_ok(),
            "the window still works over its own transport"
        );
        handle.shutdown().await;
        drop(squatter);
    }

    #[tokio::test]
    async fn production_bootstrap_opens_local_data_without_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: free_port().await,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: Some("client_test".into()),
            default_harness: HarnessId::Mock,
        })
        .await
        .unwrap();

        assert_eq!(handle.engine_info().workspace_scope, WorkspaceScope::Local);
        let info: EngineInfo = handle
            .client()
            .call_as(methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(info, *handle.engine_info());

        let mut auth = handle
            .client()
            .subscribe(methods::AUTH_STATUS, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            parse_auth_state(&auth.recv().await.unwrap()),
            Some(AuthState::SignedOut)
        );
        let harnesses = handle
            .client()
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .expect("local data RPC is immediately available");
        assert!(harnesses.as_array().is_some_and(|items| !items.is_empty()));
        assert!(
            !dir.path().join("orgs/dev-org/dev-user").exists(),
            "production boot must not create dev-user data"
        );
        assert!(dir.path().join("profiles/local").is_dir());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn engine_info_is_available_while_cloud_onboarding_is_deferred() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.json"),
            r#"{"refreshToken":"saved","user":{"id":"user_1","email":"u@example.com"}}"#,
        )
        .unwrap();
        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: free_port().await,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: Some("client_test".into()),
            default_harness: HarnessId::Mock,
        })
        .await
        .unwrap();

        assert!(matches!(
            handle
                .deferred_state()
                .expect("embedded lifecycle")
                .borrow()
                .clone(),
            DeferredEngineState::Waiting
        ));

        let info: EngineInfo = handle
            .client()
            .call_as(methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .expect("EngineInfo bypasses deferred cloud stores");
        assert_eq!(info.workspace_scope, WorkspaceScope::Synced);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                handle
                    .client()
                    .call(methods::LIST_HARNESSES, serde_json::json!({})),
            )
            .await
            .is_err(),
            "cloud data waits for organization onboarding"
        );
        assert!(!dir.path().join("orgs").exists());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn bootstrap_connects_when_daemon_is_listening() {
        // Stand in for `zeron headless`: an engine served over the WS IPC port.
        let daemon_dir = tempfile::tempdir().unwrap();
        let core = EngineCore::assemble(
            daemon_dir.path(),
            Arc::new(default_registry()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let ui_dir = tempfile::tempdir().unwrap();
        // The daemon publishes its bearer; this viewport shares the data dir
        // it reads the token from.
        let token = zeron_rpc::ipc_auth::issue_token(ui_dir.path()).unwrap();
        tokio::spawn(zeron_rpc::serve_ws_listener(
            listener,
            core.rpc_service(),
            token,
        ));

        let handle = EngineHandle::bootstrap(EngineBootConfig {
            data_dir: ui_dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::Mock,
        })
        .await
        .unwrap();
        assert_eq!(
            handle.mode(),
            EngineMode::Remote {
                url: format!("ws://127.0.0.1:{port}")
            }
        );
        assert_eq!(
            handle.engine_info().workspace_scope,
            WorkspaceScope::Development
        );
        let harnesses = handle
            .client()
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap();
        assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));
        assert!(matches!(
            handle
                .client()
                .call(methods::STOP_ENGINE, serde_json::json!({}))
                .await,
            Err(RpcError::UnknownMethod(method)) if method == methods::STOP_ENGINE
        ));
    }

    fn chat(id: &str, created_min: i64, last_msg_min: Option<i64>) -> Chat {
        let base = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .unwrap()
            .to_utc();
        Chat {
            id: id.into(),
            device_id: "dev".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: last_msg_min.map(|m| base + TimeDelta::minutes(m)),
            created_at: base + TimeDelta::minutes(created_min),
            harness_session_id: None,
            harness_session_cwd: None,
            harness_session_harness: None,
            parent_chat_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    fn space(id: &str, device_id: &str, path: &str, created_min: i64) -> Space {
        let base = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .unwrap()
            .to_utc();
        Space {
            id: id.into(),
            device_id: device_id.into(),
            path: path.into(),
            name: None,
            git_detected: false,
            git_checked_at: None,
            checkout_id: None,
            repository_id: None,
            github_repo: None,
            created_at: base + TimeDelta::minutes(created_min),
        }
    }

    fn session(
        chat_id: &str,
        status: SessionStatus,
        updated_secs_ago: i64,
        now: DateTime<Utc>,
    ) -> Session {
        Session {
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: Vec::new(),
            chat_id: chat_id.into(),
            device_id: "dev".into(),
            status,
            started_at: None,
            updated_at: now - TimeDelta::seconds(updated_secs_ago),
        }
    }

    fn user_entry(id: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: zeron_doc::MessageRole::User,
            parts: Vec::new(),
            created_at: 0,
            device_id: "dev".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[gpui::test]
    fn opening_tail_is_visible_but_never_replaces_or_caches_complete_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            let update = |id: &str| zeron_doc::TranscriptUpdate {
                frame: TranscriptFrame::reset(&[user_entry(id)]),
                context_usage: None,
                replay_baseline: None,
            };
            state.select_chat(Some("whale".into()), cx);
            state
                .receive_opening_transcript_update(update("tail"), true, cx)
                .unwrap();
            assert_eq!(state.transcript[0].id, "tail");
            assert!(
                !state.transcript_replayed,
                "scroll-anchor fallback must await full history"
            );
            state.select_chat(None, cx);
            assert!(
                state.transcript_cache.is_empty(),
                "a preview is not a complete cached copy"
            );
            state.select_chat(Some("whale".into()), cx);
            state
                .receive_opening_transcript_update(update("full"), false, cx)
                .unwrap();
            state
                .receive_opening_transcript_update(update("tail"), true, cx)
                .unwrap();
            assert_eq!(
                state.transcript[0].id, "full",
                "reconnect must preserve the full view"
            );
            state.select_chat(None, cx);
            state.select_chat(Some("whale".into()), cx);
            state
                .receive_opening_transcript_update(update("tail"), true, cx)
                .unwrap();
            assert_eq!(
                state.transcript[0].id, "full",
                "revisit must preserve the cache"
            );
        });
    }

    #[gpui::test]
    fn whale_transcript_revisit_is_synchronous_and_fresh_reset_wins(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.select_chat(Some("whale".into()), cx);
            let entries: Vec<_> = (0..2000)
                .map(|i| {
                    let mut entry = user_entry(&format!("message-{i}"));
                    entry.parts.push(zeron_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    });
                    entry
                })
                .collect();
            state.apply_transcript(entries);
            let allocation = state.transcript.as_ptr();
            for _ in 0..10 {
                state.select_chat(Some("other".into()), cx);
                assert!(
                    state.transcript.is_empty(),
                    "never show another chat's rows"
                );
                state.select_chat(Some("whale".into()), cx);
                assert_eq!(
                    state.transcript.len(),
                    2000,
                    "revisit must render before any RPC frame"
                );
                assert_eq!(
                    state.transcript.as_ptr(),
                    allocation,
                    "move whale payloads, don't copy them"
                );
                assert!(state.transcript_replayed);
                assert!(
                    state
                        .transcript_baseline("whale")
                        .unwrap()
                        .covers(&state.transcript[0])
                );
            }
            state
                .apply_transcript_frame(TranscriptFrame::reset(&[user_entry(
                    "new-authoritative-row",
                )]))
                .unwrap();
            assert_eq!(state.transcript.len(), 1);
            assert_eq!(state.transcript[0].id, "new-authoritative-row");
            state.select_chat(None, cx);
            state.select_chat(Some("whale".into()), cx);
            assert_eq!(state.transcript[0].id, "new-authoritative-row");
            // An authoritative empty reset must still be honored.
            state
                .apply_transcript_frame(TranscriptFrame::reset(&[]))
                .unwrap();
            assert!(state.transcript.is_empty());
        });
    }

    #[gpui::test]
    fn transcript_revisit_cache_is_bounded_and_account_scoped(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            for i in 0..20 {
                state.select_chat(Some(format!("chat-{i}")), cx);
                state.apply_transcript(vec![user_entry("row")]);
            }
            state.select_chat(None, cx);
            assert_eq!(state.transcript_cache.len(), TRANSCRIPT_CACHE_CAP);
            state.select_chat(Some("chat-0".into()), cx);
            assert!(state.transcript.is_empty(), "oldest entry was evicted");
            state.apply_chats(vec![chat("chat-19", 0, None)]);
            assert_eq!(
                state.transcript_cache.len(),
                1,
                "deleted chats leave no cached rows"
            );
            state.prepare_runtime_replacement(cx);
            state.select_chat(Some("chat-19".into()), cx);
            assert!(
                state.transcript.is_empty(),
                "account replacement must clear cached content"
            );
            assert!(state.transcript_cache.is_empty());
        });
    }

    #[gpui::test]
    fn transcript_cache_rejects_oversize_and_unloaded_entries(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.select_chat(Some("loading".into()), cx);
            state.select_chat(Some("oversize".into()), cx);
            assert!(state.transcript_cache.is_empty());
            let mut entry = user_entry("large");
            entry.parts.push(zeron_doc::MessagePart::Text {
                id: "text".into(),
                text: "x".repeat(TRANSCRIPT_CACHE_BYTES + 1),
            });
            state.apply_transcript(vec![entry]);
            state.select_chat(None, cx);
            assert!(
                state.transcript_cache.is_empty(),
                "byte budget applies even to one whale"
            );
        });
    }

    #[test]
    fn transcript_revision_tracks_replay_echoes_and_subagent_content() {
        let mut state = AppState::new();
        state.selected_chat = Some("c".into());
        let initial = state.transcript_revision;
        state.apply_transfers(Vec::new());
        state.apply_auth(AuthState::SignedOut);
        assert_eq!(
            state.transcript_revision, initial,
            "unrelated notifications are inert"
        );

        state.push_echo("c", user_entry("m1"));
        let echo = state.transcript_revision;
        assert_ne!(echo, initial);
        state.push_echo("c", user_entry("m1"));
        state.remove_echo("c", "absent");
        assert_eq!(
            state.transcript_revision, echo,
            "unchanged echoes do not invalidate"
        );
        state.apply_transcript(vec![user_entry("m1")]);
        assert!(state.pending_echoes().is_empty());
        assert_ne!(
            state.transcript_revision, echo,
            "echo-to-replay handoff invalidates"
        );

        let before_reset = state.transcript_revision;
        state
            .apply_transcript_frame(TranscriptFrame::Reset { reset: Vec::new() })
            .unwrap();
        assert!(state.transcript_replayed);
        assert_ne!(
            state.transcript_revision, before_reset,
            "empty replay is authoritative"
        );

        let before_subagent = state.transcript_revision;
        state.set_subagent_snapshot("sub".into(), vec![user_entry("nested")]);
        assert_ne!(state.transcript_revision, before_subagent);
        let before_close = state.transcript_revision;
        state.unwatch_subagent_doc("sub");
        assert!(state.sub_transcript("sub").is_empty());
        assert_ne!(state.transcript_revision, before_close);
    }

    fn device(id: &str, name: &str) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            platform: "macos".into(),
            last_seen_at: None,
            created_at: None,
            version: None,
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        }
    }

    #[test]
    fn a_ready_cloud_machine_that_stopped_answering_is_asleep() {
        use crate::cloud::Presence;
        let mut state = AppState::new();
        let now = Utc::now();
        let mut machine = device("cloud-s", "Cloud");
        machine.capabilities = vec![zeron_proto::CLOUD_SESSION_CAPABILITY.into()];
        machine.last_seen_at = Some(now);
        state.devices = vec![machine];
        state.cloud_sessions = Some(
            serde_json::from_value(serde_json::json!({
                "available": true,
                "sessions": [{ "chatId": "chat", "deviceId": "cloud-s", "spaceId": "space", "state": "ready", "createdAt": 0 }]
            }))
            .unwrap(),
        );
        assert_eq!(state.device_presence("cloud-s", now), Presence::Online);
        // Idle machines sleep on their own; the cached list still says ready.
        let later = now + TimeDelta::minutes(10);
        assert_eq!(state.device_presence("cloud-s", later), Presence::Asleep);
        assert!(!state.device_presence("cloud-s", later).warns());
    }

    #[test]
    fn session_heartbeats_preserve_freshness_without_redrawing_unchanged_status() {
        let mut state = AppState::new();
        let now = Utc::now();
        let mut row = Session {
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: Vec::new(),
            chat_id: "chat".into(),
            device_id: "host".into(),
            status: SessionStatus::Working,
            started_at: Some(now),
            updated_at: now,
        };
        assert!(state.apply_sessions_at(vec![row.clone()], now));
        row.updated_at = now + TimeDelta::seconds(30);
        assert!(!state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(30)));
        assert_eq!(state.sessions[0].updated_at, row.updated_at);
        assert_eq!(
            state.indicator_for("chat", now + TimeDelta::seconds(60)),
            Indicator::Working
        );
        assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(76)));
        row.updated_at = now + TimeDelta::seconds(80);
        assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(80)));
        row.status = SessionStatus::AwaitingInput;
        assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(80)));
        row.status = SessionStatus::Idle;
        row.started_at = None;
        assert!(state.apply_sessions_at(vec![row], now + TimeDelta::seconds(80)));
        assert!(state.apply_sessions_at(vec![], now + TimeDelta::seconds(80)));
    }

    #[test]
    fn unchanged_device_heartbeat_still_retires_stale_session_indicators() {
        let mut state = AppState::new();
        let now = Utc::now();
        state.sessions = vec![Session {
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: Vec::new(),
            chat_id: "chat".into(),
            device_id: "host".into(),
            status: SessionStatus::Working,
            started_at: Some(now),
            updated_at: now,
        }];
        let mut row = device("host", "Host");
        row.last_seen_at = Some(now);
        assert!(state.apply_devices_at(vec![row.clone()], now));
        row.last_seen_at = Some(now + TimeDelta::seconds(30));
        assert!(!state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(30)));
        row.last_seen_at = Some(now + TimeDelta::seconds(46));
        assert!(state.apply_devices_at(vec![row], now + TimeDelta::seconds(46)));
        assert_eq!(
            state.indicator_for("chat", now + TimeDelta::seconds(46)),
            Indicator::None
        );
        let mut recovered = state.sessions.clone();
        recovered[0].updated_at = now + TimeDelta::seconds(47);
        assert!(
            state.apply_sessions_at(recovered, now + TimeDelta::seconds(47)),
            "a heartbeat must immediately restore an indicator retired by a device tick"
        );
    }

    #[test]
    fn device_heartbeats_keep_freshness_without_repainting_unchanged_presentation() {
        let mut state = AppState::new();
        let now = Utc::now();
        let mut row = device("host", "Host");
        row.last_seen_at = Some(now);
        assert!(state.apply_devices_at(vec![row.clone()], now));
        row.last_seen_at = Some(now + TimeDelta::seconds(15));
        assert!(!state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(15)));
        assert_eq!(state.devices[0].last_seen_at, row.last_seen_at);
        // The settings label still advances even before the presence dot expires.
        assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(75)));
        // An identical frame must redraw when presence crosses the expiry boundary.
        assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(86)));
        assert!(!state.device_online("host", now + TimeDelta::seconds(86)));
        row.last_seen_at = Some(now + TimeDelta::seconds(90));
        assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(90)));
        assert!(state.device_online("host", now + TimeDelta::seconds(90)));
        row.name = "Renamed".into();
        assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(90)));
        row.version = Some("9.0.0".into());
        assert!(state.apply_devices_at(vec![row], now + TimeDelta::seconds(90)));
        assert!(state.apply_devices_at(vec![], now + TimeDelta::seconds(90)));
        assert!(!state.apply_devices_at(vec![], now + TimeDelta::seconds(90)));
    }

    #[test]
    fn local_workspace_hides_the_unknown_device_sentinel() {
        let mut state = AppState::new();
        state.workspace_scope = Some(WorkspaceScope::Local);
        state.local_device_id = Some("local".into());

        state.apply_devices(vec![
            device("local", "unknown-device"),
            device("remote", "unknown-device"),
        ]);

        assert_eq!(state.device_name("local"), Some("Local"));
        assert_eq!(state.device_name("remote"), Some("unknown-device"));

        state.apply_devices(vec![device("local", "José's MacBook Pro")]);
        assert_eq!(state.device_name("local"), Some("José's MacBook Pro"));
    }

    #[test]
    fn device_version_change_reenables_change_request_capability() {
        let mut state = AppState::new();
        state
            .change_requests
            .mark_unsupported("remote".into(), Some("0.2.2".into()));
        let mut old = device("remote", "Remote");
        old.version = Some("0.2.2".into());
        state.apply_devices(vec![old]);
        assert!(!state.change_requests.is_supported("remote"));

        let mut upgraded = device("remote", "Remote");
        upgraded.version = Some("0.2.3".into());
        state.apply_devices(vec![upgraded]);
        assert!(state.change_requests.is_supported("remote"));
    }

    #[test]
    fn transfer_percent_tracks_snapshots_by_upload_id() {
        let mut s = AppState::new();
        assert_eq!(s.transfer_percent("u1"), None);

        let frame = |id: &str, done, total| zeron_proto::TransferProgress {
            upload_id: id.into(),
            file_name: "a.png".into(),
            done,
            total,
        };
        s.apply_transfers(vec![frame("u1", 430, 1_000), frame("u2", 0, 400)]);
        assert_eq!(s.transfer_percent("u1"), Some(43));
        assert_eq!(s.transfer_percent("u2"), Some(0));
        assert_eq!(s.transfer_percent("other"), None);

        // The last point belongs to the commit — never a stuck 100%; b64
        // padding overshoot stays clamped too.
        s.apply_transfers(vec![frame("u1", 1_000, 1_000), frame("u3", 12, 0)]);
        assert_eq!(s.transfer_percent("u1"), Some(99));
        // Zero-total renders indeterminate, not a division blowup.
        assert_eq!(s.transfer_percent("u3"), None);
        // Snapshots REPLACE: u2's transfer retired with its entry.
        assert_eq!(s.transfer_percent("u2"), None);

        s.apply_transfers(Vec::new());
        assert_eq!(s.transfer_percent("u1"), None);
    }

    #[test]
    fn upload_progress_percent_clamps_and_clears() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};
        let mut s = AppState::new();
        assert_eq!(s.upload_progress_percent(), None);

        let done = Arc::new(AtomicU64::new(0));
        s.begin_upload_progress(1_000, done.clone());
        assert_eq!(s.upload_progress_percent(), Some(0));
        done.store(430, Ordering::Relaxed);
        assert_eq!(s.upload_progress_percent(), Some(43));
        // The last point belongs to commit+queue — never show a stuck 100%.
        done.store(1_000, Ordering::Relaxed);
        assert_eq!(s.upload_progress_percent(), Some(99));
        // Overshoot (b64 padding rounding) stays clamped.
        done.store(1_002, Ordering::Relaxed);
        assert_eq!(s.upload_progress_percent(), Some(99));

        s.end_upload_progress();
        assert_eq!(s.upload_progress_percent(), None);

        // A zero-byte total renders as plain "Sending…", not a percent.
        s.begin_upload_progress(0, Arc::new(AtomicU64::new(0)));
        assert_eq!(s.upload_progress_percent(), None);
    }

    #[test]
    fn send_pending_overlays_working_until_the_grace_window() {
        let now = Utc::now();
        let s_chat = chat("c", 0, Some(10)); // unseen, no session row
        let mut s = AppState::new();
        assert_eq!(s.display_status_for(&s_chat, now), ChatIndicator::Completed);
        assert_eq!(s.indicator_for("c", now), Indicator::None);
        s.begin_pending_send("c", "m1", now);
        assert_eq!(s.display_status_for(&s_chat, now), ChatIndicator::Working);
        assert_eq!(s.indicator_for("c", now), Indicator::Working);
        // Time-bounded: an offline host must not leave an eternal spinner —
        // past the grace the overlay yields (and `send_undelivered` takes
        // over with the explicit failed state).
        let later = now + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        assert_eq!(
            s.display_status_for(&s_chat, later),
            ChatIndicator::Completed
        );
        assert_eq!(s.indicator_for("c", later), Indicator::None);
        assert!(s.send_undelivered("c", later));
    }

    /// A row of `chat_id` in `status`, its turn started at `started_at` and
    /// heartbeating at `at`.
    fn turn(
        chat_id: &str,
        status: SessionStatus,
        started_at: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Session {
        Session {
            started_at,
            ..session(chat_id, status, 0, at)
        }
    }

    fn outcome(status: DoneStatus, finished_at: DateTime<Utc>) -> zeron_proto::NativeTurnOutcome {
        zeron_proto::NativeTurnOutcome {
            status,
            finished_at,
        }
    }

    /// The row of a turn that ended at `at` with `status`, settled the way
    /// the engine settles it; `completed` is the chat's completion marker.
    fn ended(
        chat_id: &str,
        status: DoneStatus,
        completed: Option<&str>,
        at: DateTime<Utc>,
    ) -> Session {
        let settled = match status {
            DoneStatus::Errored => SessionStatus::Errored,
            _ => SessionStatus::Idle,
        };
        Session {
            last_completed_turn: completed.map(Into::into),
            last_turn_outcome: Some(outcome(status, at)),
            ..turn(chat_id, settled, None, at)
        }
    }

    /// The shell's chime for `row`, the frame after `prev`.
    fn chime(
        s: &AppState,
        prev: &Session,
        row: &Session,
        now: DateTime<Utc>,
    ) -> Option<crate::sound::Ping> {
        crate::sound::SessionNotificationState::new(row, now).ping_since(
            &crate::sound::SessionNotificationState::new(prev, prev.updated_at),
            s.send_pending(&row.chat_id, now),
        )
    }

    #[test]
    fn a_send_left_before_it_landed_is_adopted_by_the_turn_its_host_starts() {
        // User report 2026-10-26: sessions launched back to back, each left
        // for the next before its host wrote the message (a fresh worktree is
        // checked out first). Only the selected chat's transcript acked a
        // send, so 120 s later the sidebar painted live runs "Failed".
        let sent = Utc::now();
        let t = |secs: i64| sent + TimeDelta::seconds(secs);
        let mut s = AppState::new();
        s.selected_chat = Some("new".into());
        s.begin_first_send("new", "m1", sent);
        s.selected_chat = None; // Cmd+N: the next session's canvas
        let new = chat("new", 0, Some(1));

        let running = turn("new", SessionStatus::Working, Some(t(2)), t(2));
        assert!(s.apply_sessions_at(vec![running.clone()], t(2)));
        assert!(!s.send_pending("new", t(2)));
        assert_eq!(s.display_status_for(&new, t(2)), ChatIndicator::Working);

        // It finishes inside the grace window: Done, with its chime.
        let done = ended("new", DoneStatus::Completed, Some("t1"), t(40));
        s.apply_sessions_at(vec![done.clone()], t(40));
        assert_eq!(s.display_status_for(&new, t(40)), ChatIndicator::Completed);
        assert_eq!(
            chime(&s, &running, &done, t(40)),
            Some(crate::sound::Ping::Done)
        );
        assert!(!s.send_undelivered("new", t(121)));
    }

    #[test]
    fn a_send_whose_turn_asks_a_question_reads_as_input() {
        let sent = Utc::now();
        let at = sent + TimeDelta::seconds(3);
        let mut s = AppState::new();
        s.begin_first_send("q", "m1", sent);
        // Started and parked on its question between two frames.
        let asking = turn("q", SessionStatus::AwaitingInput, Some(at), at);
        s.apply_sessions_at(vec![asking], at);
        let q = chat("q", 0, None);
        assert_eq!(s.display_status_for(&q, at), ChatIndicator::AwaitingInput);
        let later = sent + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        assert!(!s.send_undelivered("q", later));
    }

    #[test]
    fn only_a_fresh_turn_started_after_the_send_adopts_it() {
        let sent = Utc::now();
        let t = |secs: i64| sent + TimeDelta::seconds(secs);
        // The previous turn's row has gone stale, so the chat took a send.
        let previous = Session {
            last_completed_turn: Some("t0".into()),
            ..turn("a", SessionStatus::Working, Some(t(-600)), t(-60))
        };
        let mut s = AppState::new();
        s.apply_sessions_at(vec![previous.clone()], sent);
        s.begin_pending_send("a", "m1", sent);
        let pending_after = |s: &mut AppState, row: Session, now| {
            s.apply_sessions_at(vec![row], now);
            s.send_pending("a", now)
        };

        // That turn's heartbeat resumes.
        let heartbeat = Session {
            updated_at: t(5),
            ..previous.clone()
        };
        assert!(pending_after(&mut s, heartbeat, t(5)));
        // A turn on a row nobody has refreshed since.
        let stale = Session {
            started_at: Some(t(6)),
            updated_at: t(6),
            ..previous.clone()
        };
        assert!(pending_after(&mut s, stale, t(60)));
        // A live row with no turn start at all (a peer that sends none).
        let unstamped = Session {
            started_at: None,
            updated_at: t(60),
            ..previous.clone()
        };
        assert!(pending_after(&mut s, unstamped, t(60)));
        // That turn's end: no new turn to show.
        let stopped = ended("a", DoneStatus::Interrupted, Some("t0"), t(61));
        assert!(pending_after(&mut s, stopped, t(61)));
        // Another chat's turn.
        let other = turn("b", SessionStatus::Working, Some(t(62)), t(62));
        assert!(pending_after(&mut s, other, t(62)));

        let adopted = Session {
            started_at: Some(t(63)),
            updated_at: t(63),
            ..previous
        };
        assert!(!pending_after(&mut s, adopted, t(63)));
    }

    #[test]
    fn a_late_result_of_an_earlier_turn_never_adopts_a_send() {
        // d3141ef0: completions stay quiet while a send is pending, and the
        // result of a turn that ran before the send proves nothing about it.
        let sent = Utc::now();
        let t = |secs: i64| sent + TimeDelta::seconds(secs);
        // A turn in flight at send time (its row had gone stale) ends, alone
        // or with the next turn's start in one coalesced frame.
        let previous = Session {
            last_completed_turn: Some("t0".into()),
            ..turn("a", SessionStatus::Working, Some(t(-600)), t(-60))
        };
        let next = Session {
            last_completed_turn: Some("t1".into()),
            ..turn("a", SessionStatus::Working, Some(t(3)), t(3))
        };
        // A settled row that lagged: a turn it never showed had already ended.
        let idle = ended("a", DoneStatus::Completed, Some("t0"), t(-3600));
        let caught_up = ended("a", DoneStatus::Completed, Some("t1"), t(-5));
        for (before, row) in [
            (
                &previous,
                ended("a", DoneStatus::Completed, Some("t1"), t(2)),
            ),
            (&previous, next),
            (&idle, caught_up),
        ] {
            let mut s = AppState::new();
            s.apply_sessions_at(vec![before.clone()], sent);
            s.begin_pending_send("a", "m1", sent);
            s.apply_sessions_at(vec![row.clone()], t(3));
            assert!(s.send_pending("a", t(3)), "{row:?}");
            assert_eq!(chime(&s, before, &row, t(3)), None, "{row:?}");
        }
    }

    #[test]
    fn a_turn_that_ends_between_two_frames_adopts_a_send_it_could_only_be() {
        let sent = Utc::now();
        let at = sent + TimeDelta::seconds(2);
        let later = sent + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        // A new chat's first row already has its turn over.
        let new = chat("new", 0, Some(1));
        for (status, completed, shown) in [
            (DoneStatus::Completed, Some("t1"), ChatIndicator::Completed),
            (DoneStatus::Errored, None, ChatIndicator::Errored),
        ] {
            let mut s = AppState::new();
            s.begin_first_send("new", "m1", sent);
            s.apply_sessions_at(vec![ended("new", status, completed, at)], at);
            assert!(!s.send_pending("new", at), "{status:?}");
            assert_eq!(s.display_status_for(&new, at), shown);
            assert!(!s.send_undelivered("new", later), "{status:?}");
        }

        // A settled chat's next turn: after a completed turn, a failed one, or
        // one stamped by a host whose clock runs ahead of this device's.
        let hour_ago = sent - TimeDelta::hours(1);
        let finished = ended("a", DoneStatus::Completed, Some("t0"), hour_ago);
        let failed = ended("a", DoneStatus::Errored, Some("t0"), hour_ago);
        let second_on = sent + TimeDelta::seconds(1);
        let ahead = ended("a", DoneStatus::Completed, Some("t0"), second_on);
        for before in [finished, failed, ahead] {
            let mut s = AppState::new();
            s.apply_sessions_at(vec![before.clone()], sent);
            s.begin_pending_send("a", "m1", sent);
            // The same row again, a restarted host's empty one, or a Stop,
            // ends no turn of the send's.
            let empty = Session {
                last_completed_turn: None,
                last_turn_outcome: None,
                ..before.clone()
            };
            let stopped = ended("a", DoneStatus::Interrupted, Some("t0"), at);
            for row in [before.clone(), empty, stopped] {
                s.apply_sessions_at(vec![row.clone()], at);
                assert!(s.send_pending("a", at), "{row:?}");
            }
            s.apply_sessions_at(vec![ended("a", DoneStatus::Completed, Some("t1"), at)], at);
            assert!(!s.send_pending("a", at), "{before:?}");
        }
    }

    #[test]
    fn a_restarted_host_adopts_with_no_completion_but_not_on_its_recovery() {
        // A host engine restart (a Cloud machine waking, too) starts its rows
        // over: the send's turn comes back with no completion marker at all,
        // after a finished turn or a stopped one.
        let sent = Utc::now();
        let t = |secs: i64| sent + TimeDelta::seconds(secs);
        let first = turn("a", SessionStatus::Working, Some(t(5)), t(5));
        for before in [
            ended("a", DoneStatus::Completed, Some("t0"), t(-3600)),
            ended("a", DoneStatus::Interrupted, Some("t0"), t(-3600)),
        ] {
            let mut s = AppState::new();
            s.apply_sessions_at(vec![before.clone()], sent);
            s.begin_pending_send("a", "m1", sent);
            // Boot recovery settles a turn the restart cut off, which ran
            // nothing of the send's.
            s.apply_sessions_at(vec![ended("a", DoneStatus::Interrupted, None, t(4))], t(4));
            assert!(s.send_pending("a", t(4)), "{before:?}");
            s.apply_sessions_at(vec![first.clone()], t(5));
            assert!(!s.send_pending("a", t(5)), "{before:?}");
        }

        // A turn in flight at send time (its row had gone stale), which the
        // restart cut off, resumes first.
        let previous = Session {
            last_completed_turn: Some("t0".into()),
            ..turn("a", SessionStatus::Working, Some(t(-600)), t(-60))
        };
        let mut s = AppState::new();
        s.apply_sessions_at(vec![previous], sent);
        s.begin_pending_send("a", "m1", sent);
        s.apply_sessions_at(vec![first], t(5));
        assert!(s.send_pending("a", t(5)));
    }

    #[test]
    fn the_open_chat_keeps_its_exact_ack_and_leaving_hands_it_to_the_row() {
        let sent = Utc::now();
        let t = |secs: i64| sent + TimeDelta::seconds(secs);
        let mut s = AppState::new();
        s.selected_chat = Some("a".into());
        s.begin_first_send("a", "m1", sent);
        // Its transcript names the message; the row only shows a turn.
        let asking = turn("a", SessionStatus::AwaitingInput, Some(t(2)), t(2));
        s.apply_sessions_at(vec![asking.clone()], t(2));
        assert!(s.send_pending("a", t(2)));
        // Left: the next frame, though it looks the same, adopts and repaints.
        s.selected_chat = Some("b".into());
        let heartbeat = Session {
            updated_at: t(12),
            ..asking
        };
        assert!(s.apply_sessions_at(vec![heartbeat], t(12)));
        assert!(!s.send_pending("a", t(12)));
    }

    #[gpui::test]
    fn leaving_a_chat_hands_its_send_to_the_row_it_already_has(cx: &mut gpui::TestAppContext) {
        // Its turn ended while it was open, before the transcript named the
        // message, and nothing else is live: no frame follows the user out.
        let state = cx.new(|_| AppState::new());
        state.update(cx, |s, cx| {
            let sent = Utc::now() - TimeDelta::seconds(10);
            let at = sent + TimeDelta::seconds(5);
            s.select_chat(Some("a".into()), cx);
            s.begin_first_send("a", "m1", sent);
            s.apply_sessions_at(vec![ended("a", DoneStatus::Completed, Some("t1"), at)], at);
            assert!(s.send_pending("a", at));
            s.select_chat(Some("b".into()), cx);
            assert!(!s.send_pending("a", Utc::now()));
        });
    }

    #[test]
    fn a_known_chat_whose_row_arrives_after_the_send_proves_nothing() {
        // Its first row may end a turn that was running as the user sent.
        let sent = Utc::now();
        let at = sent + TimeDelta::seconds(2);
        let mut s = AppState::new();
        s.begin_pending_send("a", "m1", sent);
        let old = ended("a", DoneStatus::Completed, Some("t0"), at);
        s.apply_sessions_at(vec![old], at);
        assert!(s.send_pending("a", at));
    }

    #[test]
    fn a_send_no_host_takes_still_fails_past_the_grace() {
        let sent = Utc::now();
        let later = sent + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        let mut s = AppState::new();
        s.begin_first_send("a", "m1", sent);
        let other = turn("b", SessionStatus::Working, Some(later), later);
        s.apply_sessions_at(vec![other], later);
        assert!(s.send_undelivered("a", later));
    }

    #[gpui::test]
    fn an_unreachable_cloud_host_rereads_sessions_at_most_every_few_seconds(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut s = AppState::new();
            let mut cloud = chat("c", 0, Some(10));
            cloud.device_id = "cloud-s1".into();
            let mut local = chat("l", 0, Some(10));
            local.device_id = "mac".into();
            s.chats = vec![cloud, local];
            let mut machine = device("cloud-s1", "Cloud");
            machine.capabilities = vec![zeron_proto::CLOUD_SESSION_CAPABILITY.into()];
            s.devices = vec![machine];
            s
        });
        state.update(cx, |s, cx| {
            s.recheck_cloud_machine("l", cx);
            assert!(s.cloud_recheck_at.is_none(), "not a Cloud machine");
            s.recheck_cloud_machine("c", cx);
            let first = s.cloud_recheck_at.expect("re-read");
            s.recheck_cloud_machine("c", cx);
            assert_eq!(s.cloud_recheck_at, Some(first), "throttled");
        });
    }

    #[test]
    fn a_cloud_machine_that_went_quiet_is_down_and_a_wake_reads_as_waking() {
        use zeron_proto::{CloudSessions, CloudState};
        let now = Utc::now();
        let mut s = AppState::new();
        let mut c = chat("c", 0, Some(10));
        c.device_id = "cloud-s1".into();
        s.chats = vec![c];
        let mut machine = device("cloud-s1", "Cloud");
        machine.capabilities = vec![zeron_proto::CLOUD_SESSION_CAPABILITY.into()];
        machine.last_seen_at = Some(now);
        s.devices = vec![machine];
        s.cloud_sessions = Some(CloudSessions {
            sessions: vec![
                serde_json::from_value(serde_json::json!({
                    "chatId": "c", "deviceId": "cloud-s1", "spaceId": "sp",
                    "state": "ready", "createdAt": 0
                }))
                .unwrap(),
            ],
            available: true,
        });
        assert_eq!(s.chat_machine_down("c"), None, "ready and answering");
        // It slept on its own: the list still says ready, the heartbeat stopped.
        s.devices[0].last_seen_at = Some(now - TimeDelta::minutes(10));
        assert_eq!(
            s.chat_machine_down("c"),
            Some(("c".into(), CloudState::Sleeping))
        );
        // Wake asked for: waking until the machine answers.
        s.cloud_wakes.insert("c".into(), std::time::Instant::now());
        assert_eq!(
            s.chat_machine_down("c"),
            Some(("c".into(), CloudState::Starting))
        );
        assert_eq!(
            s.device_presence("cloud-s1", Utc::now()),
            crate::cloud::Presence::Waking
        );
        s.devices[0].last_seen_at = Some(Utc::now());
        assert_eq!(s.chat_machine_down("c"), None, "up again");
    }

    #[test]
    fn a_send_waiting_on_a_booting_cloud_machine_is_pending_not_undelivered() {
        use zeron_proto::{CloudSession, CloudSessions, CloudState};
        let now = Utc::now();
        let later = now + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        let mut s = AppState::new();
        let mut c = chat("c", 0, Some(10));
        c.device_id = "cloud-s1".into();
        s.chats = vec![c];
        let session = |state| CloudSession {
            chat_id: "c".into(),
            device_id: "cloud-s1".into(),
            space_id: "sp".into(),
            repo: Some("acme/app".into()),
            path: Some("/home/user/app".into()),
            state,
            error: None,
            failed_action: None,
            last_active_at: None,
            created_at: 0,
        };
        s.begin_pending_send("c", "m1", now);
        for state in [
            CloudState::Provisioning,
            CloudState::Starting,
            CloudState::Sleeping,
        ] {
            s.cloud_sessions = Some(CloudSessions {
                sessions: vec![session(state)],
                available: true,
            });
            assert_eq!(s.cloud_machine_booting("c"), Some(state));
            assert!(s.send_pending("c", later), "{state:?}");
            assert!(!s.send_undelivered("c", later), "{state:?}");
        }
        // A machine that failed is a real delivery problem.
        s.cloud_sessions = Some(CloudSessions {
            sessions: vec![session(CloudState::Error)],
            available: true,
        });
        assert_eq!(s.cloud_machine_booting("c"), None);
        assert!(s.send_undelivered("c", later));
    }

    #[test]
    fn a_cloud_turn_stays_working_while_its_setup_streams() {
        let entry = |id: &str, role, status| SessionMessageEntry {
            id: id.into(),
            role,
            parts: Vec::new(),
            created_at: 1,
            device_id: "d".into(),
            status,
            continuation_of: None,
            duration_ms: None,
        };
        let user = zeron_doc::MessageRole::User;
        let assistant = zeron_doc::MessageRole::Assistant;
        let now = Utc::now();
        let mut s = AppState::new();
        s.selected_chat = Some("c".into());
        // The machine wrote the message back and is on its setup; no
        // session row says Working yet.
        s.apply_transcript(vec![
            entry("m1", user, None),
            entry("m1.cloud-setup", assistant, Some(zeron_doc::MessageStatus::Streaming)),
        ]);
        assert_eq!(s.indicator_for("c", now), Indicator::Working);
        // Finished: the session row speaks for itself again.
        s.apply_transcript(vec![
            entry("m1", user, None),
            entry("m1.cloud-setup", assistant, Some(zeron_doc::MessageStatus::Complete)),
        ]);
        assert_ne!(s.indicator_for("c", now), Indicator::Working);
    }

    #[test]
    fn a_cloud_turn_is_timed_from_the_send_that_started_its_machine() {
        let entry = |id: &str, role, created_at| SessionMessageEntry {
            id: id.into(),
            role,
            parts: Vec::new(),
            created_at,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let user = zeron_doc::MessageRole::User;
        let assistant = zeron_doc::MessageRole::Assistant;
        let mut s = AppState::new();
        s.selected_chat = Some("c".into());
        // An ordinary turn: no Cloud setup, no base.
        s.apply_transcript(vec![entry("m0", user, 10)]);
        assert_eq!(s.cloud_setup_turn_started(), None);
        // Sent at 1000 to a machine that has to start: the placeholder.
        s.push_echo("c", entry("m1", user, 1_000));
        s.push_cloud_setup_echo("c", "m1", 1_000, false);
        assert_eq!(s.cloud_setup_turn_started(), Some(1_000));
        // The machine's own steps, then its turn: still the send.
        s.apply_transcript(vec![
            entry("m0", user, 10),
            entry("m1", user, 1_000),
            entry("m1.cloud-setup", assistant, 9_000),
            entry("a1", assistant, 12_000),
        ]);
        assert_eq!(s.cloud_setup_turn_started(), Some(1_000));
        // The next message, still only an echo, goes to the machine already
        // up: no base, not the earlier turn's send.
        s.push_echo("c", entry("m2", user, 20_000));
        assert_eq!(s.cloud_setup_turn_started(), None);
        // And once the host wrote it back.
        s.apply_transcript(vec![
            entry("m1", user, 1_000),
            entry("m1.cloud-setup", assistant, 9_000),
            entry("m2", user, 20_000),
        ]);
        assert_eq!(s.cloud_setup_turn_started(), None);
    }

    #[test]
    fn a_cloud_machine_that_failed_settles_its_setup_step() {
        use zeron_proto::{CloudSession, CloudSessions, CloudState};
        let mut s = AppState::new();
        let mut c = chat("c", 0, Some(10));
        c.device_id = "cloud-s1".into();
        s.chats = vec![c];
        s.selected_chat = Some("c".into());
        let session = |state, error: Option<&str>| CloudSessions {
            sessions: vec![CloudSession {
                chat_id: "c".into(),
                device_id: "cloud-s1".into(),
                space_id: "sp".into(),
                repo: Some("acme/app".into()),
                path: Some("/home/user/app".into()),
                state,
                error: error.map(String::from),
                failed_action: None,
                last_active_at: None,
                created_at: 0,
            }],
            available: true,
        };
        s.cloud_sessions = Some(session(CloudState::Provisioning, None));
        s.push_cloud_setup_echo("c", "m1", 0, false);
        let step = |s: &AppState| match &s.pending_echoes()[0].parts[0] {
            zeron_doc::MessagePart::Tool {
                call: zeron_proto::ToolCall::Unknown { name, .. },
                is_error,
                output,
                ..
            } => (name.clone(), *is_error, output.clone()),
            other => panic!("{other:?}"),
        };
        // Still being set up: the step spins.
        assert_eq!(step(&s), ("Starting a machine".into(), false, None));
        // Failed: the step says so, with the reason.
        s.cloud_sessions = Some(session(CloudState::Error, Some("No room")));
        s.settle_failed_cloud_setups();
        assert_eq!(
            step(&s),
            ("Couldn't start a machine".into(), true, Some("No room".into()))
        );
        assert_eq!(
            s.pending_echoes()[0].status,
            Some(zeron_doc::MessageStatus::Complete)
        );
    }

    #[test]
    fn send_pending_acked_when_the_host_writes_the_message_back() {
        let now = Utc::now();
        let mut s = AppState::new();
        s.selected_chat = Some("c".into());
        s.begin_pending_send("c", "m1", now);
        // A frame without the message keeps the overlay.
        s.apply_transcript(vec![user_entry("other")]);
        assert!(s.send_pending("c", now));
        // The host executed the command: our id comes back in the doc.
        s.apply_transcript(vec![user_entry("other"), user_entry("m1")]);
        assert!(!s.send_pending("c", now));
    }

    #[test]
    fn send_failure_cleanup_only_ends_its_own_overlay() {
        let now = Utc::now();
        let mut s = AppState::new();
        s.begin_pending_send("c", "m1", now);
        s.begin_pending_send("c", "m2", now); // quick resend superseded m1
        s.end_pending_send("c", "m1"); // m1's failure cleanup arrives late
        assert!(s.send_pending("c", now), "m2's overlay must survive");
        s.end_pending_send("c", "m2");
        assert!(!s.send_pending("c", now));
    }

    #[test]
    fn chats_sort_by_last_message_desc_with_created_fallback() {
        let mut chats = vec![
            chat("a", 0, Some(10)),
            chat("b", 5, None), // no messages → keys on created_at (+5min)
            chat("c", 1, Some(30)),
            chat("d", 40, None), // created after every message
        ];
        sort_chats(&mut chats);
        let order: Vec<&str> = chats.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(order, ["d", "c", "a", "b"]);
    }

    #[test]
    fn chat_sort_ties_are_deterministic() {
        let mut chats = vec![chat("z", 0, Some(10)), chat("a", 0, Some(10))];
        sort_chats(&mut chats);
        assert_eq!(chats[0].id, "a");
    }

    #[test]
    fn working_indicator_staleness() {
        let now = Utc::now();
        // Fresh working session shows.
        let fresh = session("c", SessionStatus::Working, 10, now);
        assert_eq!(effective_indicator(Some(&fresh), now), Indicator::Working);
        // Stale working session is suppressed — crashed backend, not eternal spinner.
        let stale = session("c", SessionStatus::Working, 46, now);
        assert_eq!(effective_indicator(Some(&stale), now), Indicator::None);
        // Exactly at the boundary still shows (strictly-older-than semantics).
        let edge = session("c", SessionStatus::Working, 45, now);
        assert_eq!(effective_indicator(Some(&edge), now), Indicator::Working);
        // Future timestamps (clock skew) count as fresh.
        let skewed = session("c", SessionStatus::Working, -30, now);
        assert_eq!(effective_indicator(Some(&skewed), now), Indicator::Working);
    }

    #[test]
    fn indicator_kinds() {
        let now = Utc::now();
        assert_eq!(effective_indicator(None, now), Indicator::None);
        let idle = session("c", SessionStatus::Idle, 0, now);
        assert_eq!(effective_indicator(Some(&idle), now), Indicator::None);
        // Errored is not staleness-gated: the error stays visible.
        let errored = session("c", SessionStatus::Errored, 600, now);
        assert_eq!(effective_indicator(Some(&errored), now), Indicator::Errored);
        let awaiting = session("c", SessionStatus::AwaitingInput, 5, now);
        assert_eq!(
            effective_indicator(Some(&awaiting), now),
            Indicator::AwaitingInput
        );
        let awaiting_stale = session("c", SessionStatus::AwaitingInput, 300, now);
        assert_eq!(
            effective_indicator(Some(&awaiting_stale), now),
            Indicator::None
        );
    }

    #[test]
    fn display_status_derivation() {
        let now = Utc::now();
        let mut c = chat("c", 0, Some(10));
        // Live states win regardless of seen.
        let working = session("c", SessionStatus::Working, 5, now);
        assert_eq!(
            display_status(&c, Some(&working), now),
            ChatIndicator::Working
        );
        let awaiting = session("c", SessionStatus::AwaitingInput, 5, now);
        assert_eq!(
            display_status(&c, Some(&awaiting), now),
            ChatIndicator::AwaitingInput
        );
        // Finished + unseen = Completed (no session row at all).
        assert_eq!(display_status(&c, None, now), ChatIndicator::Completed);
        // Idle session + unseen = Completed.
        let idle = session("c", SessionStatus::Idle, 5, now);
        assert_eq!(
            display_status(&c, Some(&idle), now),
            ChatIndicator::Completed
        );
        // A lost host is not a successful completion, even with unread text.
        let stale = session("c", SessionStatus::Working, 300, now);
        assert_eq!(display_status(&c, Some(&stale), now), ChatIndicator::Idle);
        // Seen after the last message = Idle.
        c.last_seen_at = c.last_message_at.map(|t| t + TimeDelta::minutes(1));
        assert_eq!(display_status(&c, Some(&idle), now), ChatIndicator::Idle);
        // Errored + unseen = Errored; seen clears it to Idle.
        let errored = session("c", SessionStatus::Errored, 600, now);
        assert_eq!(display_status(&c, Some(&errored), now), ChatIndicator::Idle);
        c.last_seen_at = None;
        assert_eq!(
            display_status(&c, Some(&errored), now),
            ChatIndicator::Errored
        );
        // No messages at all: nothing to see — Idle.
        let fresh = chat("f", 0, None);
        assert_eq!(display_status(&fresh, None, now), ChatIndicator::Idle);
    }

    #[gpui::test]
    fn terminal_result_is_seen_without_an_assistant_message(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            let now = Utc::now() - TimeDelta::seconds(1);
            state.chats = vec![chat("c", 0, None)];
            let mut row = session("c", SessionStatus::Idle, 0, now);
            row.terminal_activity = Some(zeron_proto::TerminalActivity {
                terminal_id: "pty".into(),
                updated_at: now,
                report: zeron_proto::ProgramStatus {
                    id: String::new(),
                    state: zeron_proto::ProgramState::Done,
                    app: None,
                    kind: None,
                    progress: None,
                    title: None,
                    message: None,
                },
            });
            state.apply_sessions(vec![row]);
            assert!(
                !state.chats[0].unseen(),
                "there is no new assistant message"
            );
            assert_eq!(
                state.display_status_for(&state.chats[0], now),
                ChatIndicator::Completed
            );
            state.mark_chat_seen("c", cx);
            assert_eq!(
                state.display_status_for(&state.chats[0], Utc::now()),
                ChatIndicator::Idle
            );
        });
    }

    #[test]
    fn active_list_sorts_by_recency_only_status_never_moves_rows() {
        let a = chat("a", 0, Some(10)); // Completed (older)
        let b = chat("b", 0, Some(20)); // Completed (newer)
        let c = chat("c", 0, Some(5)); // AwaitingInput
        let d = chat("d", 0, Some(1)); // Working
        let mut rows = vec![
            (ChatIndicator::Completed, &a),
            (ChatIndicator::Completed, &b),
            (ChatIndicator::AwaitingInput, &c),
            (ChatIndicator::Working, &d),
        ];
        sort_active(&mut rows);
        let order: Vec<&str> = rows.iter().map(|(_, c)| c.id.as_str()).collect();
        assert_eq!(order, ["b", "a", "c", "d"], "recency desc, status ignored");

        // Opening a completed session (completed → seen → idle) must NOT
        // change its position (user report: rows jumped under the pointer).
        let mut seen = vec![
            (ChatIndicator::Idle, &a),
            (ChatIndicator::Completed, &b),
            (ChatIndicator::AwaitingInput, &c),
            (ChatIndicator::Working, &d),
        ];
        sort_active(&mut seen);
        let order_after: Vec<&str> = seen.iter().map(|(_, c)| c.id.as_str()).collect();
        assert_eq!(order, order_after);
    }

    #[test]
    fn tabs_order_by_creation_not_activity() {
        let a = chat("a", 5, Some(100)); // created later, very active
        let b = chat("b", 1, Some(2));
        let mut tabs = vec![&a, &b];
        sort_tabs(&mut tabs);
        let order: Vec<&str> = tabs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(order, ["b", "a"]);
    }

    #[test]
    fn apply_spaces_sorts_and_heals_selection() {
        let mut state = AppState::new();
        state.apply_spaces(vec![
            space("s2", "dev", "/b", 2),
            space("s1", "dev", "/a", 1),
        ]);
        let ids: Vec<&str> = state.spaces.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s1", "s2"]);
        // First frame auto-selects the first space.
        assert_eq!(state.selected_space.as_deref(), Some("s1"));
        state.selected_space = Some("s2".into());
        // Vanished selection heals to the first space.
        state.apply_spaces(vec![space("s1", "dev", "/a", 1)]);
        assert_eq!(state.selected_space.as_deref(), Some("s1"));
        // No spaces at all: selection clears.
        state.apply_spaces(vec![]);
        assert_eq!(state.selected_space, None);
    }

    #[test]
    fn file_link_roots_order_chat_parent_then_projects() {
        use crate::workspace_links::FileLinkRoot;
        let mut state = AppState::new();
        state.local_device_id = Some("dev".into());
        let mut parent = chat("parent", 0, None);
        parent.cwd = Some("/repo".into());
        let mut fork = chat("fork", 1, None);
        fork.cwd = Some("/fork/fork".into());
        fork.parent_chat_id = Some("parent".into());
        // A chat hosted on another device still contributes its checkout —
        // reads go through it — but is flagged remote so system-level file
        // actions stay hidden.
        let mut remote = chat("remote", 2, None);
        remote.device_id = "other".into();
        remote.cwd = Some("/far/worktree".into());
        remote.parent_chat_id = Some("fork".into());
        state.apply_chats(vec![parent, fork, remote]);
        state.apply_spaces(vec![
            space("local", "dev", "/projects/zeron", 0),
            space("remote", "other", "/remote/only", 1),
        ]);
        let root = |chat: Option<&str>, root: &str, local: bool| FileLinkRoot {
            chat: chat.map(str::to_owned),
            root: root.into(),
            local,
        };

        let roots = state.file_link_roots("fork");
        assert_eq!(
            roots,
            vec![
                root(Some("fork"), "/fork/fork", true),
                root(Some("parent"), "/repo", true),
                root(None, "/projects/zeron", true),
            ],
            "own checkout, parent, then this device's projects"
        );
        assert!(state.chat_is_local("fork"));
        assert!(!state.chat_is_local("remote"));
        // A remote chat's paths name its own device: this device's projects
        // and a parent hosted here never own them.
        assert_eq!(
            state.file_link_roots("remote"),
            vec![
                root(Some("remote"), "/far/worktree", false),
                root(None, "/remote/only", false),
            ]
        );

        // An absolute path outside every known root is still a file: it
        // resolves as an outside link owned by the linking chat.
        let root_refs: Vec<&str> = roots.iter().map(|root| root.root.as_str()).collect();
        assert!(matches!(
            crate::workspace_links::first_root_owning("/elsewhere/x.md", root_refs.clone()),
            Some(crate::workspace_links::FileLinkResolution::Outside(link))
                if link.path == "/elsewhere/x.md" && link.outside
        ));
        // A path under the parent's checkout resolves to that root.
        assert_eq!(
            match crate::workspace_links::first_root_owning("/repo/src/lib.rs", root_refs) {
                Some(crate::workspace_links::FileLinkResolution::Owned { root, link }) =>
                    Some((roots[root].chat.clone(), link.path)),
                _ => None,
            },
            Some((Some("parent".into()), "src/lib.rs".into()))
        );
    }

    #[test]
    fn canvas_terminal_uses_the_selected_project_folder() {
        let mut state = AppState::new();
        state.local_device_id = Some("local".into());
        state.apply_spaces(vec![
            space("s1", "local", "/Users/me/proj", 1),
            space("remote-space", "remote", "/remote/proj", 2),
        ]);
        state.selected_chat = None;
        state.no_project = false;
        state.selected_space = Some("s1".into());

        assert_eq!(state.panel_session_key(), "space-canvas:s1");
        assert_eq!(state.terminal_open_cwd().as_deref(), Some("/Users/me/proj"));
        assert_eq!(
            state.terminal_open_cwd_for("space-canvas:s1").as_deref(),
            Some("/Users/me/proj")
        );
        // Project id is known, row not in the list yet — do not send `~`.
        state.spaces.clear();
        assert_eq!(state.terminal_open_cwd(), None);
        state.apply_spaces(vec![
            space("s1", "local", "/Users/me/proj", 1),
            space("remote-space", "remote", "/remote/proj", 2),
        ]);
        assert_eq!(state.terminal_open_cwd().as_deref(), Some("/Users/me/proj"));
        assert_eq!(
            state.terminal_target_device(&state.panel_session_key()),
            None,
            "local project stays on this engine"
        );

        state.selected_space = Some("remote-space".into());
        assert_eq!(
            state
                .terminal_target_device("space-canvas:remote-space")
                .as_deref(),
            Some("remote")
        );

        state.no_project = true;
        state.selected_space = None;
        state.selected_device = Some("remote".into());
        assert_eq!(state.panel_session_key(), "space-canvas:");
        assert_eq!(state.terminal_open_cwd().as_deref(), Some("~"));
        assert_eq!(
            state.terminal_target_device("space-canvas:").as_deref(),
            Some("remote")
        );

        let mut row = chat("chat-1", 0, None);
        row.device_id = "remote".into();
        state.chats = vec![row];
        state.selected_chat = Some("chat-1".into());
        assert_eq!(state.panel_session_key(), "chat-1");
        assert_eq!(state.terminal_open_cwd(), None);
        assert_eq!(
            state.terminal_target_device("chat-1").as_deref(),
            Some("remote")
        );
    }

    #[test]
    fn projectless_preference_survives_restart_and_space_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = crate::settings::composer::ComposerDefaults {
            device: Some("remote".into()),
            // Older versions kept a stale project alongside the opt-out.
            project: Some("old-project".into()),
            no_project: true,
            ..Default::default()
        };
        defaults.save(dir.path()).unwrap();
        let mut state = AppState::new();
        state.restore_composer_target(&crate::settings::composer::ComposerDefaults::load(
            dir.path(),
        ));
        for spaces in [
            vec![],
            vec![space("s1", "remote", "/a", 1)],
            vec![],
            vec![space("s2", "other", "/b", 2)],
        ] {
            state.apply_spaces(spaces);
            assert!(state.no_project);
            assert!(state.selected_space.is_none());
            assert!(state.selected_space_row().is_none());
            assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
        }
    }

    #[gpui::test]
    fn projectless_selection_clears_project_and_survives_device_switch(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.apply_spaces(vec![
                space("s1", "dev", "/a", 1),
                space("s2", "remote", "/b", 2),
            ]);
            state.select_space(Some("s1".into()), cx);
            state.select_space(None, cx);
            assert!(state.selected_space.is_none());
            state.select_device("remote".into(), cx);
            assert!(state.no_project);
            assert!(state.selected_space.is_none());
            assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
            state.select_space(Some("s2".into()), cx);
            assert!(!state.no_project);
            assert_eq!(state.selected_space_row().unwrap().id, "s2");
            state.selected_device = Some("dev".into());
            state.select_space(None, cx);
            assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
            state.select_space(Some("s2".into()), cx);
            state.select_device("empty-device".into(), cx);
            assert!(state.selected_space.is_none());
            assert_eq!(state.effective_device_id().as_deref(), Some("empty-device"));
        });
    }

    #[gpui::test]
    fn canvas_reopens_on_its_own_target_after_visiting_chats(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.apply_spaces(vec![
                space("s1", "dev", "/a", 1),
                space("s2", "remote", "/b", 2),
            ]);
            let mut remote = chat("remote-chat", 1, None);
            remote.device_id = "remote".into();
            remote.space_id = Some("s2".into());
            let mut loose = chat("loose-chat", 2, None);
            loose.device_id = "remote".into();
            state.apply_chats(vec![remote, loose.clone()]);
            state.select_space(Some("s1".into()), cx);

            // Chats imply their own target while open…
            state.select_chat(Some("remote-chat".into()), cx);
            assert_eq!(state.selected_space.as_deref(), Some("s2"));
            state.select_chat(Some("loose-chat".into()), cx);
            assert!(state.no_project);
            // …and the canvas returns to where it was left.
            state.select_chat(None, cx);
            assert!(!state.no_project);
            assert_eq!(state.selected_space.as_deref(), Some("s1"));
            assert_eq!(state.effective_device_id().as_deref(), Some("dev"));

            // A projectless canvas survives a project chat, and a chat deleted
            // elsewhere lands back on it too.
            state.select_space(None, cx);
            state.select_device("remote".into(), cx);
            state.select_chat(Some("remote-chat".into()), cx);
            state.apply_chats(vec![loose]);
            assert!(state.selected_chat.is_none());
            assert!(state.no_project);
            assert!(state.selected_space.is_none());
            assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
        });
    }

    #[test]
    fn chats_in_space_filters_and_orders() {
        let mut state = AppState::new();
        state.apply_spaces(vec![space("s1", "dev", "/a", 1)]);
        let mut in_space_new = chat("new", 5, None);
        in_space_new.space_id = Some("s1".into());
        let mut in_space_old = chat("old", 1, Some(50)); // active but created first
        in_space_old.space_id = Some("s1".into());
        let mut other = chat("other", 2, None);
        other.space_id = Some("s2".into());
        let mut archived = chat("gone", 0, None);
        archived.space_id = Some("s1".into());
        archived.archived = true;
        let dangling = chat("dangling", 3, None); // no space id
        state.apply_chats(vec![in_space_new, in_space_old, other, archived, dangling]);
        let ids: Vec<&str> = state
            .chats_in_space("s1")
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["old", "new"]);
        // The overview shows every live-space chat (idle included) PLUS
        // project-less chats (first-class since the project selectors);
        // chats of unknown spaces stay hidden. Completed ("old") outranks
        // idle ("new"/"dangling").
        let now = Utc::now();
        let overview: Vec<&str> = state
            .overview_chats(now)
            .iter()
            .map(|(_, c)| c.id.as_str())
            .collect();
        assert_eq!(overview, ["old", "new", "dangling"]);
    }

    #[test]
    fn project_filter_and_projects_span_a_repository_across_devices() {
        let mut state = AppState::new();
        state.local_device_id = Some("laptop".into());
        let mut remote = space("remote", "metal", "/w/anara", 1);
        remote.repository_id = Some("commit:abc".into());
        let mut local = space("local", "laptop", "/h/anara", 2);
        local.repository_id = Some("commit:abc".into());
        let other = space("other", "laptop", "/h/notes", 3);
        state.apply_spaces(vec![remote, local, other]);
        let in_space = |id: &str, space: &str| {
            let mut chat = chat(id, 0, None);
            chat.space_id = Some(space.into());
            chat
        };
        state.apply_chats(vec![
            in_space("on-remote", "remote"),
            in_space("on-local", "local"),
            in_space("on-other", "other"),
        ]);

        // One project per repository, its checkouts local-first.
        let projects: Vec<Vec<&str>> = state
            .projects()
            .iter()
            .map(|members| members.iter().map(|s| s.id.as_str()).collect())
            .collect();
        assert_eq!(projects, vec![vec!["local", "remote"], vec!["other"]]);

        // Filtering on either checkout shows the whole repository's chats.
        for filter in ["remote", "local"] {
            let mut ids: Vec<&str> = state
                .sidebar_chats(Utc::now(), Some(filter))
                .iter()
                .map(|(_, c)| c.id.as_str())
                .collect();
            ids.sort();
            assert_eq!(ids, ["on-local", "on-remote"], "filter {filter}");
        }
        let other_only: Vec<&str> = state
            .sidebar_chats(Utc::now(), Some("other"))
            .iter()
            .map(|(_, c)| c.id.as_str())
            .collect();
        assert_eq!(other_only, ["on-other"]);

        // A two-device project tags both hosts; a one-device one keeps the
        // plain host tag.
        let anara = state.projects().into_iter().next().unwrap();
        let (tag, _) = state.project_device_tag(&anara, Utc::now());
        assert!(tag.starts_with("@ ") && tag.contains(" · "), "{tag}");
        let notes = state.projects().into_iter().nth(1).unwrap();
        assert_eq!(
            state.project_device_tag(&notes, Utc::now()).0,
            state.space_device_tag(notes[0], Utc::now()).0
        );
    }

    #[test]
    fn apply_chats_drops_vanished_selection() {
        let mut state = AppState::new();
        state.apply_chats(vec![chat("a", 0, None), chat("b", 1, None)]);
        state.selected_chat = Some("a".into());
        state.transcript = vec![];
        state.apply_chats(vec![chat("b", 1, None)]);
        assert_eq!(state.selected_chat, None);
        // Still-present selection survives.
        state.selected_chat = Some("b".into());
        state.apply_chats(vec![chat("b", 1, None), chat("c", 2, None)]);
        assert_eq!(state.selected_chat.as_deref(), Some("b"));
    }

    fn clock_epoch() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .unwrap()
            .to_utc()
    }

    /// Builds a clock-watching state at the pinned `now` and counts its
    /// notifications.
    fn clock_state(
        cx: &mut gpui::TestAppContext,
        setup: impl FnOnce(&mut AppState),
    ) -> (
        Entity<AppState>,
        std::rc::Rc<std::cell::Cell<usize>>,
        gpui::Subscription,
    ) {
        let state = cx.new(|cx| {
            let mut state = AppState::new();
            setup(&mut state);
            state.watch_clock_transitions(cx);
            state
        });
        let notifies = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = notifies.clone();
        let subscription =
            cx.update(|cx| cx.observe(&state, move |_, _| counter.set(counter.get() + 1)));
        (state, notifies, subscription)
    }

    /// Steps the simulated executor and the pinned wall clock together.
    fn advance_to(
        cx: &mut gpui::TestAppContext,
        clock: &TestClock,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) {
        clock.set(to);
        cx.executor().advance_clock((to - from).to_std().unwrap());
    }

    /// Proves the state notifies exactly once, exactly at `deadline`.
    fn assert_state_notifies_at(
        cx: &mut gpui::TestAppContext,
        now: DateTime<Utc>,
        deadline: DateTime<Utc>,
        setup: impl FnOnce(&mut AppState),
    ) {
        let clock = TestClock::start(now);
        let (_state, notifies, _subscription) = clock_state(cx, setup);
        let before = deadline - TimeDelta::milliseconds(1);
        advance_to(cx, &clock, now, before);
        assert_eq!(notifies.get(), 0, "no notify before the deadline");
        advance_to(cx, &clock, before, deadline);
        assert_eq!(notifies.get(), 1, "one notify at the deadline");
        // Nothing else is due: the next minute passes silently.
        advance_to(cx, &clock, deadline, deadline + TimeDelta::minutes(1));
        assert_eq!(notifies.get(), 1);
    }

    #[gpui::test]
    fn stale_session_notifies_state_at_its_cutoff(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let row = session("row", SessionStatus::Working, 10, now);
        let cutoff = row.updated_at + TimeDelta::milliseconds(SESSION_STALE_MS + 1);
        let mut probe = AppState::new();
        probe.sessions = vec![row.clone()];
        assert_eq!(
            probe.indicator_for("row", cutoff - TimeDelta::milliseconds(1)),
            Indicator::Working
        );
        assert_eq!(probe.indicator_for("row", cutoff), Indicator::None);
        assert_state_notifies_at(cx, now, cutoff, |state| state.sessions = vec![row]);
    }

    #[gpui::test]
    fn remote_device_notifies_state_when_it_goes_offline(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let seen = now - TimeDelta::seconds(40);
        let mut remote = device("remote", "Remote");
        remote.last_seen_at = Some(seen);
        // The local device is always online; its stale heartbeat is no deadline.
        let mut local = device("local", "Local");
        local.last_seen_at = Some(now - TimeDelta::seconds(65));
        let offline =
            seen + TimeDelta::seconds(crate::settings::devices::DEVICE_ONLINE_WINDOW_SECS + 1);
        let mut probe = AppState::new();
        probe.devices = vec![remote.clone()];
        assert!(probe.device_online("remote", offline - TimeDelta::milliseconds(1)));
        assert!(!probe.device_online("remote", offline));
        assert_state_notifies_at(cx, now, offline, |state| {
            state.local_device_id = Some("local".into());
            state.devices = vec![local, remote];
        });
    }

    #[gpui::test]
    fn pending_send_notifies_state_when_its_grace_expires(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let started = now - TimeDelta::seconds(100);
        let expiry = started + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
        let mut probe = AppState::new();
        probe.begin_pending_send("chat", "message", started);
        assert!(!probe.send_undelivered("chat", expiry - TimeDelta::milliseconds(1)));
        assert!(probe.send_undelivered("chat", expiry));
        assert_state_notifies_at(cx, now, expiry, |state| {
            state.begin_pending_send("chat", "message", started)
        });
    }

    #[gpui::test]
    fn settled_state_arms_no_clock(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let clock = TestClock::start(now);
        let (state, notifies, _subscription) = clock_state(cx, |state| {
            state.sessions = vec![
                session("failed", SessionStatus::Errored, 0, now),
                session("idle", SessionStatus::Idle, 0, now),
            ];
        });
        assert!(state.read_with(cx, |state, _| state.clock.task.is_none()));
        advance_to(cx, &clock, now, now + TimeDelta::hours(1));
        assert_eq!(notifies.get(), 0);
    }

    #[gpui::test]
    fn silent_heartbeat_defers_the_stale_notify(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let clock = TestClock::start(now);
        let row = session("row", SessionStatus::Working, 10, now);
        let first_cutoff = row.updated_at + TimeDelta::milliseconds(SESSION_STALE_MS + 1);
        let (state, notifies, _subscription) = clock_state(cx, |state| {
            state.apply_sessions_at(vec![row], now);
        });
        // A heartbeat only refreshes the lease: the reducer stores it without
        // notifying, so the armed timer is now early.
        let heartbeat = now + TimeDelta::seconds(20);
        advance_to(cx, &clock, now, heartbeat);
        state.update(cx, |state, _| {
            let changed = state.apply_sessions_at(
                vec![session("row", SessionStatus::Working, 0, heartbeat)],
                heartbeat,
            );
            assert!(!changed);
        });
        advance_to(cx, &clock, heartbeat, first_cutoff);
        assert_eq!(notifies.get(), 0, "an obsolete deadline re-arms silently");
        let cutoff = heartbeat + TimeDelta::milliseconds(SESSION_STALE_MS + 1);
        advance_to(
            cx,
            &clock,
            first_cutoff,
            cutoff - TimeDelta::milliseconds(1),
        );
        assert_eq!(notifies.get(), 0);
        advance_to(cx, &clock, cutoff - TimeDelta::milliseconds(1), cutoff);
        assert_eq!(notifies.get(), 1);
    }

    #[gpui::test]
    fn revival_after_a_clock_transition_notifies_and_rearms(cx: &mut gpui::TestAppContext) {
        let now = clock_epoch();
        let clock = TestClock::start(now);
        let row = session("row", SessionStatus::Working, 0, now);
        let stale_at = now + TimeDelta::milliseconds(SESSION_STALE_MS + 1);
        let (state, notifies, _subscription) = clock_state(cx, |state| {
            state.apply_sessions_at(vec![row], now);
        });
        advance_to(cx, &clock, now, stale_at);
        assert_eq!(notifies.get(), 1, "went stale by clock alone");
        // A heartbeat revives it: that is a visible change again, and the
        // revived lease arms its own cutoff.
        let revived = stale_at + TimeDelta::seconds(1);
        advance_to(cx, &clock, stale_at, revived);
        state.update(cx, |state, cx| {
            let changed = state.apply_sessions_at(
                vec![session("row", SessionStatus::Working, 0, revived)],
                revived,
            );
            assert!(changed, "revival is a change");
            cx.notify();
        });
        assert_eq!(notifies.get(), 2);
        let next_cutoff = revived + TimeDelta::milliseconds(SESSION_STALE_MS + 1);
        advance_to(cx, &clock, revived, next_cutoff);
        assert_eq!(notifies.get(), 3, "the revived lease's cutoff fires");
    }

    #[test]
    fn identical_chats_frames_report_no_change() {
        let mut state = AppState::new();
        // The first frame flips `chats_synced` even when empty.
        assert!(state.apply_chats(vec![]));
        assert!(!state.apply_chats(vec![]));
        assert!(state.apply_chats(vec![chat("b", 1, None), chat("a", 0, None)]));
        let revision = state.link_roots_revision;
        // Same rows in a different wire order sort to the same list.
        assert!(!state.apply_chats(vec![chat("a", 0, None), chat("b", 1, None)]));
        assert_eq!(state.link_roots_revision, revision);
        let mut renamed = chat("a", 0, None);
        renamed.title = Some("Renamed".into());
        assert!(state.apply_chats(vec![renamed.clone(), chat("b", 1, None)]));
        assert_ne!(state.link_roots_revision, revision);
        // A selection pointing at no row still heals on an unchanged frame.
        state.selected_chat = Some("gone".into());
        assert!(state.apply_chats(vec![renamed, chat("b", 1, None)]));
        assert_eq!(state.selected_chat, None);
    }

    #[test]
    fn identical_spaces_frames_report_no_change() {
        let mut state = AppState::new();
        assert!(state.apply_spaces(vec![]));
        assert!(!state.apply_spaces(vec![]));
        let frame = vec![space("s2", "dev", "/b", 2), space("s1", "dev", "/a", 1)];
        assert!(state.apply_spaces(frame.clone()));
        assert_eq!(state.selected_space.as_deref(), Some("s1"));
        assert!(!state.apply_spaces(frame.clone()));
        // A vanished selection still heals (and reports it) on an unchanged list.
        state.selected_space = Some("gone".into());
        assert!(state.apply_spaces(frame));
        assert_eq!(state.selected_space.as_deref(), Some("s1"));
    }

    #[test]
    fn identical_connectivity_frames_report_no_change() {
        use zeron_proto::{Connectivity, ConnectivityState};
        let mut state = AppState::new();
        // The first frame ends the bootstrap gap even when it equals the default.
        assert!(state.apply_connectivity(Connectivity::default()));
        assert!(state.connectivity_observed);
        assert!(!state.apply_connectivity(Connectivity::default()));
        let reconnecting = Connectivity {
            state: ConnectivityState::Reconnecting,
            retry_at_ms: 5_000,
            ..Default::default()
        };
        assert!(state.apply_connectivity(reconnecting.clone()));
        assert!(!state.apply_connectivity(reconnecting.clone()));
        assert!(state.apply_connectivity(Connectivity {
            retry_at_ms: 9_000,
            ..reconnecting
        }));
        // Re-attach reopens the gap: the next frame counts again.
        state.connectivity_observed = false;
        assert!(state.apply_connectivity(state.connectivity.clone()));
    }

    #[test]
    fn identical_transfer_auth_and_harness_frames_report_no_change() {
        use zeron_proto::{HarnessUpdatePhase, HarnessUpdateStatus, TransferProgress};
        let mut state = AppState::new();
        assert!(!state.apply_transfers(vec![]));
        let progress = |done| TransferProgress {
            upload_id: "u1".into(),
            file_name: "a.png".into(),
            done,
            total: 100,
        };
        assert!(state.apply_transfers(vec![progress(10)]));
        assert!(!state.apply_transfers(vec![progress(10)]));
        assert!(state.apply_transfers(vec![progress(20)]));
        assert_eq!(state.transfer_percent("u1"), Some(20));
        assert!(state.apply_transfers(vec![]));

        assert!(state.apply_auth(AuthState::SignedOut));
        assert!(!state.apply_auth_value(serde_json::json!({ "state": "signedOut" })));
        let user = UserProfile {
            id: "u".into(),
            email: "w@example.com".into(),
            name: None,
        };
        assert!(state.apply_auth(AuthState::NeedsOrganization { user: user.clone() }));
        assert!(!state.apply_auth(AuthState::NeedsOrganization { user }));
        assert!(!state.apply_auth_value(serde_json::json!({ "bogus": true })));

        let status = |phase| HarnessUpdateStatus {
            harness: HarnessId::Codex,
            installed_version: Some("1.0".into()),
            latest_version: Some("2.0".into()),
            channel: None,
            source: Default::default(),
            policy: Default::default(),
            phase,
            progress: None,
            checked_at: None,
            error: None,
            can_apply: true,
            manual_command: None,
            compatibility: None,
            held_back: None,
        };
        assert!(!state.apply_harness_updates(vec![]));
        assert!(state.apply_harness_updates(vec![status(HarnessUpdatePhase::Available)]));
        assert!(!state.apply_harness_updates(vec![status(HarnessUpdatePhase::Available)]));
        assert!(state.apply_harness_updates(vec![status(HarnessUpdatePhase::Current)]));
    }

    #[test]
    fn apply_chat_config_stamps_the_row() {
        let mut state = AppState::new();
        state.apply_chats(vec![chat("a", 0, None), chat("b", 1, None)]);
        let config = zeron_proto::ChatConfig {
            harness: HarnessId::ClaudeCode,
            model: Some("claude-fable-5".into()),
            reasoning: Some(zeron_proto::ReasoningLevel::XHigh),
            model_options: serde_json::Map::new(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            access: Default::default(),
            plan: false,
        };
        state.apply_chat_config("a", config.clone());
        assert_eq!(
            state.chats.iter().find(|c| c.id == "a").unwrap().config,
            Some(config)
        );
        assert!(
            state
                .chats
                .iter()
                .find(|c| c.id == "b")
                .unwrap()
                .config
                .is_none()
        );
        // Unknown chat: no-op, no panic.
        state.apply_chat_config(
            "missing",
            zeron_proto::ChatConfig {
                harness: HarnessId::ClaudeCode,
                model: None,
                reasoning: None,
                model_options: serde_json::Map::new(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                access: Default::default(),
                plan: false,
            },
        );
    }

    #[test]
    fn visible_chats_filters_archived() {
        let mut state = AppState::new();
        let mut archived = chat("a", 0, Some(99));
        archived.archived = true;
        state.apply_chats(vec![archived, chat("b", 1, None)]);
        let visible: Vec<&str> = state.visible_chats().map(|c| c.id.as_str()).collect();
        assert_eq!(visible, ["b"]);
    }

    #[test]
    fn visible_chats_hide_spawned_children() {
        let now = Utc::now();
        let mut state = AppState::new();
        let mut child = chat("child", 0, Some(1));
        child.parent_chat_id = Some("parent".into());
        state.apply_chats(vec![child, chat("parent", 1, Some(2))]);
        let visible: Vec<&str> = state.visible_chats().map(|c| c.id.as_str()).collect();
        assert_eq!(visible, ["parent"]);
        // The sidebar list and jump slots follow the same rule.
        let rows: Vec<&str> = state
            .sidebar_chats(now, None)
            .into_iter()
            .map(|(_, c)| c.id.as_str())
            .collect();
        assert_eq!(rows, ["parent"]);
        // The child row itself is still addressable (deep links, tabs).
        assert!(state.chats.iter().any(|c| c.id == "child"));
    }

    #[test]
    fn side_chats_remain_addressable_but_do_not_appear_in_sidebar() {
        let mut state = AppState::new();
        let main = chat("main", 0, None);
        let mut side = chat("side", 1, Some(2));
        side.parent_chat_id = Some(main.id.clone());
        state.apply_chats(vec![main, side]);
        assert_eq!(
            state
                .visible_chats()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["main"]
        );
        state.selected_chat = Some("side".into());
        assert_eq!(
            state.selected_chat_row().unwrap().parent_chat_id.as_deref(),
            Some("main")
        );
    }

    #[test]
    fn remote_fork_waits_for_registry_then_observes_deletion() {
        let mut state = AppState::new();
        let mut side = chat("side", 1, None);
        side.parent_chat_id = Some("main".into());
        state.selected_chat = Some(side.id.clone());
        state.pending_side_chat = Some(side.clone());
        state.apply_chats(vec![]);
        assert_eq!(state.selected_chat.as_deref(), Some("side"));
        state.apply_chats(vec![side]);
        assert!(state.pending_side_chat.is_none());
        state.apply_chats(vec![]);
        assert_eq!(state.selected_chat, None);
    }

    #[test]
    fn jump_slots_count_the_rows_the_sidebar_draws() {
        let now = Utc::now();
        let mut state = AppState::new();
        let mut in_space = chat("a", 0, Some(3));
        in_space.space_id = Some("s1".into());
        let mut other_space = chat("b", 1, Some(2));
        other_space.space_id = Some("s2".into());
        let mut archived = chat("gone", 2, Some(1));
        archived.space_id = Some("s1".into());
        archived.archived = true;
        state.apply_spaces(vec![
            space("s1", "d1", "/tmp/s1", 0),
            space("s2", "d1", "/tmp/s2", 1),
        ]);
        state.apply_chats(vec![in_space, other_space, archived]);

        // The archived row is not in the active list, so no slot reaches it.
        let order: Vec<&str> = state
            .sidebar_chats(now, None)
            .iter()
            .map(|(_, c)| c.id.as_str())
            .collect();
        assert_eq!(order.len(), 2);
        assert!(!order.contains(&"gone"));

        // A project filter renumbers: the visible rows only.
        let filtered: Vec<&str> = state
            .sidebar_chats(now, Some("s2"))
            .iter()
            .map(|(_, c)| c.id.as_str())
            .collect();
        assert_eq!(filtered, ["b"]);
    }

    #[test]
    fn archive_shortcut_only_targets_an_open_active_chat() {
        let mut state = AppState::new();
        let mut archived = chat("a", 0, None);
        archived.archived = true;
        state.apply_chats(vec![archived, chat("b", 1, None)]);
        // No chat open: nothing to archive.
        assert_eq!(state.archivable_selected_chat(), None);
        // The open active chat is the target.
        state.selected_chat = Some("b".into());
        assert_eq!(state.archivable_selected_chat(), Some("b"));
        // An already archived chat stays put — the shortcut never unarchives.
        state.selected_chat = Some("a".into());
        assert_eq!(state.archivable_selected_chat(), None);
    }

    #[test]
    fn a_send_held_in_the_queue_drops_its_echo_and_pending_overlay() {
        let mut state = AppState::new();
        state.selected_chat = Some("c1".into());
        let echo = |id: &str| SessionMessageEntry {
            id: id.into(),
            role: zeron_doc::MessageRole::User,
            parts: vec![],
            created_at: 0,
            device_id: "local".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let row = |id: &str| zeron_doc::QueuedMessage {
            id: id.into(),
            text: "held".into(),
            attachments: vec![],
            hold_for_turn_end: false,
            issued_by: "local".into(),
            issued_at: 0,
            edited_at: None,
            delivery_gate: None,
        };
        state.push_echo("c1", echo("held"));
        state.push_echo("c1", echo("sent"));
        state.begin_pending_send("c1", "held", Utc::now());
        state.apply_queue(vec![row("held")]);
        let ids: Vec<_> = state
            .pending_echoes()
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, vec!["sent"]);
        assert!(!state.pending_sends.contains_key("c1"));
        assert_eq!(state.queue.len(), 1);
    }

    #[test]
    fn echoes_show_until_doc_frame_confirms() {
        let mut state = AppState::new();
        state.selected_chat = Some("c1".into());
        let echo = SessionMessageEntry {
            id: "m1".into(),
            role: zeron_doc::MessageRole::User,
            parts: vec![],
            created_at: 0,
            device_id: "local".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        state.push_echo("c1", echo.clone());
        // Duplicate pushes dedupe.
        state.push_echo("c1", echo.clone());
        assert_eq!(state.pending_echoes().len(), 1);
        // Frames without the id keep the echo.
        state.apply_transcript(vec![]);
        assert_eq!(state.pending_echoes().len(), 1);
        // The confirming frame prunes it.
        state.apply_transcript(vec![SessionMessageEntry {
            id: "m1".into(),
            ..echo.clone()
        }]);
        assert!(state.pending_echoes().is_empty());
        // Failure path: explicit removal.
        state.push_echo(
            "c1",
            SessionMessageEntry {
                id: "m2".into(),
                ..echo.clone()
            },
        );
        state.remove_echo("c1", "m2");
        assert!(state.pending_echoes().is_empty());
        // Echoes are per chat.
        state.push_echo(
            "other",
            SessionMessageEntry {
                id: "m3".into(),
                ..echo
            },
        );
        assert!(state.pending_echoes().is_empty());
    }

    #[test]
    fn a_cloud_setup_placeholder_hands_off_to_the_machine_entry_or_goes() {
        let entry = |id: &str, role| SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![],
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let user = zeron_doc::MessageRole::User;
        let assistant = zeron_doc::MessageRole::Assistant;
        let mut state = AppState::new();
        state.selected_chat = Some("c1".into());
        state.push_echo("c1", entry("m1", user));
        state.push_cloud_setup_echo("c1", "m1", 0, false);
        assert!(state.cloud_setup_echo_pending());
        let placeholder = &state.pending_echoes()[1];
        assert_eq!(placeholder.id, "m1.cloud-setup");
        let zeron_doc::MessagePart::Tool {
            id, call, resolved, ..
        } = &placeholder.parts[0]
        else {
            panic!("{:?}", placeholder.parts)
        };
        assert_eq!(id, "m1.cloud-machine");
        // Shaped like the machine's own step, which the doc keeps no input on.
        assert_eq!(
            call,
            &zeron_proto::ToolCall::Unknown {
                name: "Starting a machine".into(),
                input: None,
            }
        );
        assert!(!resolved);
        // The host wrote the message: the machine is still coming up.
        state.apply_transcript(vec![entry("m1", user)]);
        assert!(state.cloud_setup_echo_pending());
        // Its setup entry takes over (same ids).
        state.apply_transcript(vec![entry("m1", user), entry("m1.cloud-setup", assistant)]);
        assert!(!state.cloud_setup_echo_pending());
        // A machine that was already up answers without one: the
        // placeholder still goes.
        state.push_echo("c1", entry("m2", user));
        state.push_cloud_setup_echo("c1", "m2", 0, true);
        state.apply_transcript(vec![entry("m2", user)]);
        assert!(state.cloud_setup_echo_pending());
        state.apply_transcript(vec![entry("m2", user), entry("a2", assistant)]);
        assert!(!state.cloud_setup_echo_pending());
        // The placeholder lands a beat after the send: never once the machine
        // has written its own setup entry or already answered.
        state.apply_transcript(vec![
            entry("m1", user),
            entry("m1.cloud-setup", assistant),
            entry("m2", user),
            entry("a2", assistant),
        ]);
        state.push_cloud_setup_echo("c1", "m1", 0, false);
        state.push_cloud_setup_echo("c1", "m2", 0, true);
        assert!(!state.cloud_setup_echo_pending());
    }

    #[test]
    fn a_wake_refused_for_credits_says_so_and_stops_working() {
        let mut state = AppState::new();
        state.selected_chat = Some("c1".into());
        state.push_echo(
            "c1",
            SessionMessageEntry {
                id: "m1".into(),
                role: zeron_doc::MessageRole::User,
                parts: vec![],
                created_at: 0,
                device_id: "d".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            },
        );
        state.push_cloud_setup_echo("c1", "m1", 0, true);
        state.begin_pending_send("c1", "m1", chrono::Utc::now());
        assert!(state.send_pending("c1", chrono::Utc::now()));
        assert!(state.cloud_setup_streaming());
        assert!(state.block_cloud_setups_for_credits());
        // Neither the overlay nor the placeholder keeps the chat Working.
        assert_eq!(
            state.indicator_for("c1", chrono::Utc::now()),
            Indicator::None
        );
        let step = state.echoes["c1"]
            .iter()
            .find(|entry| entry.id == zeron_proto::cloud_setup_entry_id("m1"))
            .expect("placeholder");
        let zeron_doc::MessagePart::Tool {
            call,
            is_error,
            resolved,
            ..
        } = &step.parts[0]
        else {
            panic!("{:?}", step.parts)
        };
        assert_eq!(
            call,
            &zeron_proto::ToolCall::Unknown {
                name: CLOUD_NO_CREDITS_STEP.into(),
                input: None,
            }
        );
        assert!(*is_error && *resolved);
        // No more "working" for a send that can't run yet.
        assert!(!state.send_pending("c1", chrono::Utc::now()));
        // Once said, it isn't said again (one toast per send).
        assert!(!state.block_cloud_setups_for_credits());
    }

    #[test]
    fn transcript_replay_barrier_requires_the_opening_reset() {
        let mut state = AppState::new();
        assert!(!state.transcript_replayed);

        state
            .apply_transcript_frame(TranscriptFrame::Delta {
                upsert: Vec::new(),
                append: Vec::new(),
                remove: Vec::new(),
                count: 0,
            })
            .expect("empty delta");
        assert!(!state.transcript_replayed);

        state
            .apply_transcript_frame(TranscriptFrame::Reset { reset: Vec::new() })
            .expect("empty reset");
        assert!(state.transcript_replayed);
    }

    #[test]
    fn gate_phases() {
        let user = UserProfile {
            id: "u".into(),
            email: "w@example.com".into(),
            name: None,
        };
        assert_eq!(
            gate_phase(&ConnectionStatus::Connecting, None, None),
            GatePhase::Loading
        );
        assert_eq!(
            gate_phase(&ConnectionStatus::Failed("boom".into()), None, None),
            GatePhase::Failed("boom".into())
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Local),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::Ready
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::SignIn
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedIn {
                    user: user.clone(),
                    org_id: None
                })
            ),
            GatePhase::Ready
        );
        // No org yet → org gate.
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::NeedsOrganization { user })
            ),
            GatePhase::OrgGate
        );
    }

    #[test]
    fn auth_changes_do_not_change_a_local_runtime_scope_or_watches() {
        let mut state = AppState::new();
        state.workspace_scope = Some(WorkspaceScope::Local);
        state.watch_tasks.push(Task::ready(()));

        state.apply_auth(AuthState::NeedsOrganization {
            user: UserProfile {
                id: "u".into(),
                email: "w@example.com".into(),
                name: None,
            },
        });
        assert_eq!(state.workspace_scope, Some(WorkspaceScope::Local));
        assert_eq!(state.watch_tasks.len(), 1);

        state.apply_auth(AuthState::SignedIn {
            user: UserProfile {
                id: "u".into(),
                email: "w@example.com".into(),
                name: None,
            },
            org_id: Some("org-1".into()),
        });
        assert_eq!(state.workspace_scope, Some(WorkspaceScope::Local));
        assert_eq!(state.watch_tasks.len(), 1);
    }

    #[test]
    fn auth_frames_parse_both_wire_shapes() {
        // Proto shape.
        let proto = serde_json::json!({ "state": "signedOut" });
        assert_eq!(parse_auth_state(&proto), Some(AuthState::SignedOut));
        // Engine shape (`_tag`, PascalCase, orgId).
        let engine = serde_json::json!({
            "_tag": "SignedIn",
            "user": { "id": "u1", "email": "w@example.com" },
            "orgId": "org-1",
        });
        let Some(AuthState::SignedIn { user, org_id }) = parse_auth_state(&engine) else {
            panic!("expected SignedIn");
        };
        assert_eq!(user.email, "w@example.com");
        assert_eq!(org_id.as_deref(), Some("org-1"));
        let needs = serde_json::json!({
            "_tag": "NeedsOrganization",
            "user": { "id": "u1", "email": "w@example.com", "name": "W" },
        });
        assert!(matches!(
            parse_auth_state(&needs),
            Some(AuthState::NeedsOrganization { .. })
        ));
        // Garbage → None (frame dropped, not a crash).
        assert_eq!(
            parse_auth_state(&serde_json::json!({ "_tag": "Wat" })),
            None
        );
        assert_eq!(parse_auth_state(&serde_json::json!(42)), None);
    }

    fn chat_with_cwd(id: &str, created_min: i64, cwd: Option<&str>) -> Chat {
        let mut c = chat(id, created_min, None);
        c.cwd = cwd.map(str::to_string);
        c
    }

    #[test]
    fn project_labels_from_cwd() {
        assert_eq!(project_label(Some("/home/w/dev/zeron")), "zeron");
        assert_eq!(project_label(Some("/home/w/dev/zeron/")), "zeron");
        assert_eq!(project_label(None), "No project");
        assert_eq!(project_label(Some("~")), "No project");
        assert_eq!(project_label(Some("~/")), "No project");
        assert_eq!(
            project_label(Some("~/.zeron/scratch/2026-10-11-3e4f5a6b")),
            "No project"
        );
        assert_eq!(project_label(Some("   ")), "No project");
        assert_eq!(project_label(Some("/")), "/");
    }

    #[test]
    fn grouped_sidebar_preserves_recency_order() {
        // Input is sidebar-sorted (most recent first).
        let chats = [
            chat_with_cwd("a", 9, Some("/dev/zeron")),
            chat_with_cwd("b", 8, Some("/dev/zed")),
            chat_with_cwd("c", 7, Some("/dev/zeron")),
            chat_with_cwd("d", 6, None),
        ];
        let groups = group_chats(chats.iter());
        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
        // Groups ordered by their most recent chat; rows keep order.
        assert_eq!(labels, ["zeron", "zed", "No project"]);
        let zeron_ids: Vec<&str> = groups[0].chats.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(zeron_ids, ["a", "c"]);
        assert!(group_chats(std::iter::empty()).is_empty());
    }

    #[test]
    fn relative_times_match_zeron_format() {
        let now = Utc::now();
        let ago = |secs: i64| now - chrono::Duration::seconds(secs);
        assert_eq!(format_time_ago(ago(0), now), "now");
        assert_eq!(format_time_ago(ago(59), now), "now");
        assert_eq!(format_time_ago(ago(60), now), "1m");
        assert_eq!(format_time_ago(ago(59 * 60), now), "59m");
        assert_eq!(format_time_ago(ago(60 * 60), now), "1h");
        assert_eq!(format_time_ago(ago(23 * 3600 + 3599), now), "23h");
        assert_eq!(format_time_ago(ago(24 * 3600), now), "1d");
        assert_eq!(format_time_ago(ago(6 * 86400), now), "6d");
        assert_eq!(format_time_ago(ago(7 * 86400), now), "1w");
        assert_eq!(format_time_ago(ago(30 * 86400), now), "4w");
        assert_eq!(format_time_ago(ago(35 * 86400), now), "1mo");
        assert_eq!(format_time_ago(ago(400 * 86400), now), "1y");
        // Clock skew (future timestamps) clamps to "now".
        assert_eq!(
            format_time_ago(now + chrono::Duration::hours(2), now),
            "now"
        );
    }

    #[test]
    fn chat_location_joins_project_and_branch() {
        let mut c = chat_with_cwd("x", 1, Some("/home/w/dev/soccertcg"));
        c.branch = Some("zeron/rebalance".into());
        assert_eq!(
            chat_location(&c).as_deref(),
            Some("soccertcg · zeron/rebalance")
        );
        c.branch = None;
        assert_eq!(chat_location(&c).as_deref(), Some("soccertcg"));
        c.cwd = None;
        c.branch = Some("main".into());
        assert_eq!(chat_location(&c).as_deref(), Some("main"));
        c.branch = Some("   ".into());
        assert_eq!(chat_location(&c), None);
        c.branch = None;
        assert_eq!(chat_location(&c), None);
        // Projectless: no project name, no folder.
        for cwd in ["~", "~/.zeron/scratch/2026-10-11-3e4f5a6b"] {
            c.cwd = Some(cwd.into());
            assert_eq!(chat_location(&c), None);
        }
    }

    #[test]
    fn org_gate_reducers() {
        assert!(org_name_valid("Acme"));
        assert!(org_name_valid("  padded  "));
        assert!(!org_name_valid(""));
        assert!(!org_name_valid("   "));
        assert!(!org_name_valid(&"x".repeat(65)));

        let rows = parse_orgs(&serde_json::json!({ "orgs": [
            { "id": "m2", "organizationId": "o2", "name": "beta" },
            { "id": "m1", "organizationId": "o1", "name": "Alpha" },
            { "id": "m3", "organizationId": "o1", "name": "Alpha" },
        ]}));
        assert_eq!(rows.len(), 3);
        let sorted = sort_memberships(rows);
        let names: Vec<&str> = sorted.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            ["Alpha", "beta"],
            "case-insensitive sort + dedupe by org id"
        );
        // Bare-array replies parse too; garbage yields empty.
        assert_eq!(
            parse_orgs(&serde_json::json!([{ "id": "m", "organizationId": "o", "name": "n" }]))
                .len(),
            1
        );
        assert!(parse_orgs(&serde_json::json!("nope")).is_empty());
    }

    #[test]
    fn review_comment_body_updates_are_scoped_and_keep_current_metadata() {
        let mut state = AppState::new();
        let original = ReviewComment::file("a.rs", 2, "Original");
        state.add_review_comment("chat-1", original.clone());
        state.add_review_comment("chat-2", original.clone());
        state.begin_review_comment_flush("chat-1", 1);
        state.update_review_comment_line("chat-1", &original.id, 9);
        state.rename_review_comment_path("chat-1", "a.rs", "renamed.rs");
        state.update_review_comment_body("chat-1", &original.id, "Revised".into());
        let mut expected = original.clone();
        expected.path = "renamed.rs".into();
        expected.line = 9;
        expected.body = "Revised".into();
        assert_eq!(state.review_comments("chat-1"), &[expected]);
        assert_eq!(state.review_comments("chat-2"), &[original.clone()]);
        assert!(state.review_comment_flush_pending("chat-1"));
        state.remove_review_comment("chat-1", &original.id);
        state.update_review_comment_body("chat-1", &original.id, "Stale".into());
        state.update_review_comment_body("missing-chat", &original.id, "Stale".into());
        assert!(state.review_comments("chat-1").is_empty());
        assert!(state.review_comments("missing-chat").is_empty());
    }

    #[test]
    fn review_comment_flush_waits_for_every_file_surface() {
        let mut state = AppState::new();

        state.begin_review_comment_flush("chat-1", 1);
        state.begin_review_comment_flush("chat-1", 2);
        assert!(state.review_comment_flush_pending("chat-1"));

        state.finish_review_comment_flush("chat-1", 1);
        assert!(state.review_comment_flush_pending("chat-1"));

        state.finish_review_comment_flush("chat-1", 2);
        assert!(!state.review_comment_flush_pending("chat-1"));
    }

    #[test]
    fn version_triple_parses_and_gates_device_features() {
        assert_eq!(version_triple("0.2.12"), Some((0, 2, 12)));
        assert_eq!(version_triple("0.2.12-beta.1"), Some((0, 2, 12)));
        assert_eq!(version_triple("1.0.0+build7"), Some((1, 0, 0)));
        assert_eq!(version_triple("0.2"), None);
        assert_eq!(version_triple("garbage"), None);

        let mut s = AppState::default();
        assert!(
            !s.device_version_at_least("d1", (0, 2, 12)),
            "unknown device conservatively fails the gate"
        );
        s.devices = vec![Device {
            id: "d1".into(),
            name: "laptop".into(),
            platform: "macos".into(),
            last_seen_at: None,
            created_at: None,
            version: Some("0.2.12".into()),
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        }];
        assert!(s.device_version_at_least("d1", (0, 2, 12)));
        assert!(!s.device_version_at_least("d1", (0, 2, 13)));
        s.devices[0].version = None;
        assert!(
            !s.device_version_at_least("d1", (0, 2, 12)),
            "unstamped version conservatively fails the gate"
        );
    }

    #[test]
    fn explicit_capabilities_distinguish_same_version_builds() {
        let mut state = AppState::default();
        state.devices = vec![
            Device {
                id: "personal".into(),
                name: "personal".into(),
                platform: "macos".into(),
                last_seen_at: None,
                created_at: None,
                version: Some("0.2.31".into()),
                cursor_sdk_version: None,
                capabilities: vec![zeron_proto::capabilities::MESSAGE_QUEUE_V1.into()],
            },
            Device {
                id: "upstream".into(),
                name: "upstream".into(),
                platform: "macos".into(),
                last_seen_at: None,
                created_at: None,
                version: Some("0.2.31".into()),
                cursor_sdk_version: None,
                capabilities: Vec::new(),
            },
        ];

        assert!(state.device_supports("personal", zeron_proto::capabilities::MESSAGE_QUEUE_V1));
        assert!(!state.device_supports("upstream", zeron_proto::capabilities::MESSAGE_QUEUE_V1));
    }

    #[test]
    fn delivery_degradation_and_queued_sends_tell_the_truth() {
        use zeron_proto::{ChatConnectivity, ConnectivityState};
        let now = Utc::now();
        let mut s = AppState::default();
        s.local_device_id = Some("local".into());
        let mut remote = chat("c-remote", 0, None);
        remote.device_id = "remote".into();
        let mut local = chat("c-local", 0, None);
        local.device_id = "local".into();
        s.chats = vec![remote, local];
        s.devices = vec![Device {
            id: "remote".into(),
            name: "vps".into(),
            platform: "linux".into(),
            last_seen_at: Some(now),
            created_at: None,
            version: None,
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        }];
        s.connectivity.state = ConnectivityState::Connected;
        s.connectivity.chats = vec![ChatConnectivity {
            sync_state: zeron_proto::ChatSyncState::Unknown,
            chat_id: "c-remote".into(),
            connected: true,
            delivery_live: false,
            pending_pushes: 0,
        }];

        let queues = |s: &AppState, id: &str| s.chat_link_at(id, now).send_would_queue();

        // Healthy: nothing would queue. An engine without per-chat sync
        // states proves its room with the graced bit alone.
        assert_eq!(s.chat_link_at("c-remote", now), ChatLink::LIVE);
        assert_eq!(s.chat_link_at("c-local", now), ChatLink::LIVE);

        // Idle chats can remain without a socket for longer than the engine's
        // grace. A stale down sample during focus must not flash a warning.
        s.connectivity.chats[0].sync_state = zeron_proto::ChatSyncState::Local;
        s.connectivity.chats[0].connected = false;
        assert!(!queues(&s, "c-remote"));

        // A requested reconnect, or a room still down past the engine's
        // grace, is SYNCING — opening a chat on a slow link is no outage.
        for sync_state in [
            zeron_proto::ChatSyncState::Waiting,
            zeron_proto::ChatSyncState::Connecting,
        ] {
            for connected in [true, false] {
                s.connectivity.chats[0].sync_state = sync_state;
                s.connectivity.chats[0].connected = connected;
                assert_eq!(
                    s.chat_link_at("c-remote", now).sync,
                    LinkSync::Syncing { shown: false }
                );
                assert!(!queues(&s, "c-remote"));
            }
        }
        s.connectivity.chats[0].connected = true;

        // Global registry failure still warns for a dormant chat.
        s.connectivity.chats[0].sync_state = zeron_proto::ChatSyncState::Local;
        s.connectivity.state = ConnectivityState::Reconnecting;
        assert_eq!(s.chat_link_at("c-remote", now).sync, LinkSync::Reconnecting);
        assert!(queues(&s, "c-remote"));
        s.connectivity.chats[0].sync_state = zeron_proto::ChatSyncState::Waiting;
        assert!(queues(&s, "c-remote"));

        // A live chat room can deliver while the registry reconnects. The
        // graced `connected` bit alone is not proof, but delivery_live is.
        s.connectivity.chats[0].sync_state = zeron_proto::ChatSyncState::Synced;
        assert!(queues(&s, "c-remote"));
        s.connectivity.chats[0].delivery_live = true;
        assert!(!queues(&s, "c-remote"));
        s.begin_pending_send("c-remote", "m-live", now);
        assert!(!s.send_queued("c-remote", now));
        s.connectivity.chats[0].sync_state = zeron_proto::ChatSyncState::Connecting;
        s.connectivity.chats[0].connected = false; // HTTP delivery can outlive the socket.
        assert!(!queues(&s, "c-remote"));
        s.connectivity.chats[0].delivery_live = false;
        assert!(queues(&s, "c-remote"));
        assert!(s.send_queued("c-remote", now));
        s.connectivity.chats[0].connected = true;
        s.connectivity.state = ConnectivityState::Connected;

        // Host gone presence-dark → unreachable (a send would queue at best).
        s.connectivity.chats[0].delivery_live = true;
        s.devices[0].last_seen_at = Some(now - TimeDelta::minutes(10));
        assert_eq!(
            s.chat_link_at("c-remote", now),
            ChatLink {
                sync: LinkSync::Live,
                host_unreachable: true
            }
        );
        assert!(queues(&s, "c-remote"));
        s.devices[0].last_seen_at = Some(now);

        // OS offline: remote queues; a locally-hosted chat NEVER does (the
        // queued command executes on this device even fully offline).
        s.connectivity.state = ConnectivityState::Offline;
        assert_eq!(s.chat_link_at("c-remote", now).sync, LinkSync::Offline);
        assert_eq!(s.chat_link_at("c-local", now), ChatLink::LIVE);
        s.connectivity.chats[0].delivery_live = false;

        // Local profile (Disabled): nothing queues.
        s.connectivity.state = ConnectivityState::Disabled;
        assert_eq!(s.chat_link_at("c-remote", now), ChatLink::LIVE);

        // Queued = pending send + a path that would hold it — and an
        // unconfirmed path HOLDS the overlay past the grace window instead of
        // silently expiring (the no-trace hole).
        s.connectivity.state = ConnectivityState::Offline;
        s.begin_pending_send("c-remote", "m1", now - TimeDelta::seconds(200));
        assert!(s.send_pending("c-remote", now));
        assert!(s.send_queued("c-remote", now));
        // Past the grace: the explicit failed state surfaces alongside.
        assert!(s.send_undelivered("c-remote", now));
        // Retry restarts the clock: back to Queued, no longer failed.
        s.retry_pending_send("c-remote", now);
        assert!(!s.send_undelivered("c-remote", now));
        assert!(s.send_queued("c-remote", now));
        // A syncing chat delivers but cannot show the send adopted yet: the
        // overlay holds without claiming Queued.
        s.retry_pending_send("c-remote", now - TimeDelta::seconds(200));
        s.connectivity.state = ConnectivityState::Connected;
        assert!(s.send_pending("c-remote", now));
        assert!(!s.send_queued("c-remote", now));
        // Path proven live with a stale unacked send: the overlay expires
        // after the grace rather than holding forever.
        s.connectivity.chats[0].delivery_live = true;
        assert!(!s.send_pending("c-remote", now));
        assert!(!s.send_queued("c-remote", now));
        // …but the explicit undelivered flag still tells the truth.
        assert!(s.send_undelivered("c-remote", now));
    }

    fn remote_chat_state(now: DateTime<Utc>) -> AppState {
        let mut s = AppState::default();
        s.local_device_id = Some("local".into());
        let mut remote = chat("c-remote", 0, None);
        remote.device_id = "remote".into();
        let mut local = chat("c-local", 0, None);
        local.device_id = "local".into();
        s.chats = vec![remote, local];
        let mut host = device("remote", "omarchy");
        host.last_seen_at = Some(now);
        s.devices = vec![host];
        s
    }

    fn connectivity(
        sync_state: zeron_proto::ChatSyncState,
        connected: bool,
        delivery_live: bool,
    ) -> zeron_proto::Connectivity {
        zeron_proto::Connectivity {
            state: zeron_proto::ConnectivityState::Connected,
            chats: vec![zeron_proto::ChatConnectivity {
                chat_id: "c-remote".into(),
                sync_state,
                connected,
                delivery_live,
                pending_pushes: 0,
            }],
            ..Default::default()
        }
    }

    /// The reported bug: opening a chat on a distant host over a slow link
    /// read as an outage 4 s in ("Messages will send once the connection
    /// recovers"), before anything was sent. It is syncing until its room
    /// has outlived a whole first dial.
    #[test]
    fn opening_a_remote_chat_syncs_before_it_queues() {
        use zeron_proto::ChatSyncState as C;
        let t0 = clock_epoch();
        let at = |ms: i64| t0 + TimeDelta::milliseconds(ms);
        let mut s = remote_chat_state(t0);
        // The dial starts at once, inside the engine's room grace.
        s.apply_connectivity_at(connectivity(C::Connecting, true, false), t0);
        let link = |s: &AppState, ms| s.chat_link_at("c-remote", at(ms)).sync;
        assert_eq!(link(&s, 0), LinkSync::Syncing { shown: false });
        // The engine's room grace lapses (connected → false) 4 s in, while
        // the dial is still inside its 20 s connect + 15 s hello budget.
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(4_000));
        assert_eq!(
            link(&s, SYNC_SHOWN_AFTER_MS - 1),
            LinkSync::Syncing { shown: false }
        );
        assert_eq!(
            link(&s, SYNC_SHOWN_AFTER_MS),
            LinkSync::Syncing { shown: true }
        );
        assert!(!s.chat_link_at("c-remote", at(34_999)).send_would_queue());
        assert_eq!(
            link(&s, SYNC_STALLED_AFTER_MS - 1),
            LinkSync::Syncing { shown: true }
        );
        assert_eq!(link(&s, SYNC_STALLED_AFTER_MS), LinkSync::Stalled);
        assert!(
            s.chat_link_at("c-remote", at(SYNC_STALLED_AFTER_MS))
                .send_would_queue()
        );
        // A joined room catching up a long backlog never stalls.
        s.apply_connectivity_at(connectivity(C::Connecting, true, false), at(40_000));
        assert_eq!(link(&s, 600_000), LinkSync::Syncing { shown: true });
        // Delivery proven: live, and the next sync starts its own clock.
        s.apply_connectivity_at(connectivity(C::Synced, true, true), at(600_000));
        assert_eq!(link(&s, 600_000), LinkSync::Live);
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(700_000));
        assert_eq!(link(&s, 700_000), LinkSync::Syncing { shown: false });
        assert_eq!(link(&s, 700_000 + SYNC_STALLED_AFTER_MS), LinkSync::Stalled);
        // Unchanged frames keep the clock; a vanished chat drops it.
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(720_000));
        assert_eq!(link(&s, 700_000 + SYNC_STALLED_AFTER_MS), LinkSync::Stalled);
        s.apply_connectivity_at(zeron_proto::Connectivity::default(), at(730_000));
        assert!(s.sync_clocks.is_empty());
    }

    /// A chat queued behind busy rooms reads as syncing when opened, with a
    /// fresh hold and a fresh stall budget. But a sync that never delivers
    /// always ends up stalled: the engine moving its room back to waiting
    /// for a slot does not restart the clock and spin forever.
    #[gpui::test]
    fn a_sync_that_never_delivers_stalls_even_back_in_the_queue(cx: &mut gpui::TestAppContext) {
        use zeron_proto::ChatSyncState as C;
        let t0 = clock_epoch();
        let clock = TestClock::start(t0);
        let at = |ms: i64| t0 + TimeDelta::milliseconds(ms);
        let state = cx.new(|_| {
            let mut s = remote_chat_state(t0);
            s.devices.clear();
            s.apply_connectivity_at(connectivity(C::Waiting, false, false), t0);
            s
        });
        let link = |cx: &mut gpui::TestAppContext, ms| {
            state.read_with(cx, |s, _| s.chat_link_at("c-remote", at(ms)).sync)
        };
        assert_eq!(link(cx, 60_000), LinkSync::Stalled);
        // Opened a minute in: the caption's hold and the stall start now, so
        // a sync that ends quickly never flashes either.
        clock.set(at(60_000));
        state.update(cx, |s, cx| s.select_chat(Some("c-remote".into()), cx));
        assert_eq!(link(cx, 60_000), LinkSync::Syncing { shown: false });
        assert_eq!(
            link(cx, 60_000 + SYNC_SHOWN_AFTER_MS),
            LinkSync::Syncing { shown: true }
        );
        // Admitted, then moved back to waiting for a slot before its room
        // ever joined: the clock runs on through both.
        state.update(cx, |s, _| {
            s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(61_000))
        });
        state.update(cx, |s, _| {
            s.apply_connectivity_at(connectivity(C::Waiting, false, false), at(80_000))
        });
        assert_eq!(
            link(cx, 60_000 + SYNC_STALLED_AFTER_MS - 1),
            LinkSync::Syncing { shown: true }
        );
        assert_eq!(link(cx, 60_000 + SYNC_STALLED_AFTER_MS), LinkSync::Stalled);
        state.update(cx, |s, _| {
            s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(100_000))
        });
        assert_eq!(link(cx, 200_000), LinkSync::Stalled);
        // Only delivery ends it.
        state.update(cx, |s, _| {
            s.apply_connectivity_at(connectivity(C::Synced, true, true), at(200_000))
        });
        assert_eq!(link(cx, 200_000), LinkSync::Live);
    }

    /// A new engine's first frame restarts every sync clock: a chat the last
    /// engine was dialing does not read as stalled 4 s into a fresh attempt.
    #[test]
    fn a_new_engine_restarts_sync_clocks() {
        use zeron_proto::ChatSyncState as C;
        let t0 = clock_epoch();
        let at = |ms: i64| t0 + TimeDelta::milliseconds(ms);
        let mut s = remote_chat_state(t0);
        s.devices.clear();
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), t0);
        assert_eq!(
            s.chat_link_at("c-remote", at(60_000)).sync,
            LinkSync::Stalled
        );
        // `attach_engine`: the bootstrap gap before the new engine's frame.
        s.connectivity_observed = false;
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), at(60_000));
        assert_eq!(
            s.chat_link_at("c-remote", at(64_000)).sync,
            LinkSync::Syncing { shown: true }
        );
        assert_eq!(
            s.chat_link_at("c-remote", at(60_000 + SYNC_STALLED_AFTER_MS))
                .sync,
            LinkSync::Stalled
        );
    }

    /// Past the grace, a send on a chat still syncing is in flight, not
    /// failed: its pushes left before catch-up and its adoption shows once
    /// the copy catches up. A stalled room fails it as before.
    #[test]
    fn a_send_on_a_syncing_chat_is_not_undelivered() {
        use zeron_proto::ChatSyncState as C;
        let t0 = clock_epoch();
        let at = |ms: i64| t0 + TimeDelta::milliseconds(ms);
        let mut s = remote_chat_state(t0);
        s.devices.clear();
        s.begin_pending_send("c-remote", "m", t0);
        // A long catch-up on a joined room.
        s.apply_connectivity_at(connectivity(C::Connecting, true, false), t0);
        let late = at(UNDELIVERED_GRACE_MS + 60_000);
        assert!(s.send_pending("c-remote", late));
        assert!(!s.send_queued("c-remote", late));
        assert!(!s.send_undelivered("c-remote", late));
        // Its room drops: reconnecting at once, so the send waits and the
        // explicit failed state surfaces.
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), late);
        assert!(s.send_queued("c-remote", late));
        assert!(s.send_undelivered("c-remote", late));

        // A send already past its grace when a fresh dial starts waits out
        // that dial's budget before it fails.
        let mut fresh = remote_chat_state(t0);
        fresh.devices.clear();
        fresh.begin_pending_send("c-remote", "m", t0 - TimeDelta::minutes(5));
        fresh.apply_connectivity_at(connectivity(C::Connecting, false, false), t0);
        assert!(!fresh.send_undelivered("c-remote", at(SYNC_STALLED_AFTER_MS - 1)));
        assert!(fresh.send_undelivered("c-remote", at(SYNC_STALLED_AFTER_MS)));
    }

    /// The new-chat canvas has no chat row: its link is the global posture
    /// and the target device's presence.
    #[test]
    fn canvas_link_reads_the_global_state_and_the_device() {
        use zeron_proto::ConnectivityState as S;
        let t0 = clock_epoch();
        let mut s = remote_chat_state(t0);
        s.connectivity.state = S::Connected;
        assert_eq!(s.device_link_at("remote", t0), ChatLink::LIVE);
        s.devices[0].last_seen_at = Some(t0 - TimeDelta::minutes(10));
        assert_eq!(
            s.device_link_at("remote", t0),
            ChatLink {
                sync: LinkSync::Live,
                host_unreachable: true
            }
        );
        s.connectivity.state = S::Reconnecting;
        assert_eq!(s.device_link_at("remote", t0).sync, LinkSync::Reconnecting);
        s.connectivity.state = S::Offline;
        assert_eq!(s.device_link_at("remote", t0).sync, LinkSync::Offline);
        // This device, or a local profile, never waits.
        assert_eq!(s.device_link_at("local", t0), ChatLink::LIVE);
        s.connectivity.state = S::Disabled;
        assert_eq!(s.device_link_at("remote", t0), ChatLink::LIVE);
    }

    /// HTTP fallback can deliver while the socket never joins (networks
    /// that refuse the WS upgrade): proof of delivery beats the room bit.
    #[test]
    fn http_fallback_is_live_while_the_registry_is_connected() {
        use zeron_proto::ChatSyncState as C;
        let t0 = clock_epoch();
        let mut s = remote_chat_state(t0);
        s.devices.clear(); // no presence evidence: reachable
        s.apply_connectivity_at(connectivity(C::Connecting, false, true), t0);
        let later = t0 + TimeDelta::minutes(5);
        assert_eq!(s.chat_link_at("c-remote", later), ChatLink::LIVE);
        assert!(s.sync_clocks.is_empty());
    }

    /// A sleeping or waking Cloud machine answers the send itself: never
    /// "Queued", never "can't reach".
    #[test]
    fn sleeping_cloud_machine_is_not_queued() {
        let t0 = clock_epoch();
        let mut s = remote_chat_state(t0);
        s.apply_connectivity_at(
            connectivity(zeron_proto::ChatSyncState::Synced, true, true),
            t0,
        );
        // A session machine whose heartbeat stopped has gone to sleep.
        s.devices[0].capabilities = vec![zeron_proto::CLOUD_SESSION_CAPABILITY.into()];
        s.devices[0].last_seen_at = Some(t0 - TimeDelta::minutes(30));
        assert!(matches!(
            s.device_presence("remote", t0),
            crate::cloud::Presence::Asleep | crate::cloud::Presence::Waking
        ));
        assert_eq!(s.chat_link_at("c-remote", t0), ChatLink::LIVE);
        s.begin_pending_send("c-remote", "m", t0);
        assert!(!s.send_queued("c-remote", t0));
        // A plain device that went dark is unreachable.
        s.devices[0].capabilities.clear();
        assert!(s.chat_link_at("c-remote", t0).host_unreachable);
        assert!(s.send_queued("c-remote", t0));
    }

    #[gpui::test]
    fn chat_sync_caption_and_stall_are_clock_transitions(cx: &mut gpui::TestAppContext) {
        use zeron_proto::ChatSyncState as C;
        let now = clock_epoch();
        let clock = TestClock::start(now);
        let (state, notifies, _subscription) = clock_state(cx, |state| {
            *state = remote_chat_state(now);
            // Only the sync's own deadlines: no heartbeat to lapse.
            state.devices.clear();
            state.selected_chat = Some("c-remote".into());
            state.apply_connectivity_at(connectivity(C::Connecting, false, false), now);
        });
        let shown = now + TimeDelta::milliseconds(SYNC_SHOWN_AFTER_MS);
        let stalled = now + TimeDelta::milliseconds(SYNC_STALLED_AFTER_MS);
        let before = |at: DateTime<Utc>| at - TimeDelta::milliseconds(1);
        advance_to(cx, &clock, now, before(shown));
        assert_eq!(notifies.get(), 0);
        advance_to(cx, &clock, before(shown), shown);
        assert_eq!(notifies.get(), 1, "the caption shows on time");
        advance_to(cx, &clock, shown, before(stalled));
        assert_eq!(notifies.get(), 1);
        advance_to(cx, &clock, before(stalled), stalled);
        assert_eq!(notifies.get(), 2, "the stall reads on time");
        advance_to(cx, &clock, stalled, stalled + TimeDelta::minutes(1));
        assert_eq!(notifies.get(), 2);
        assert_eq!(
            state.read_with(cx, |state, _| state
                .chat_link_at("c-remote", clock_now())
                .sync),
            LinkSync::Stalled
        );
        // A joined room never stalls: no deadline past the caption.
        let joined = stalled + TimeDelta::minutes(2);
        advance_to(cx, &clock, stalled + TimeDelta::minutes(1), joined);
        state.update(cx, |state, _| {
            state.apply_connectivity_at(zeron_proto::Connectivity::default(), joined);
            state.apply_connectivity_at(connectivity(C::Connecting, true, false), joined);
        });
        let deadlines: Vec<_> = state.read_with(cx, |state, _| state.clock_deadlines().collect());
        assert_eq!(
            deadlines,
            [joined + TimeDelta::milliseconds(SYNC_SHOWN_AFTER_MS)]
        );
        // A chat syncing in the background, with nothing sent, shows nothing
        // that changes by the clock; a pending send needs its stall on time.
        state.update(cx, |state, _| {
            state.selected_chat = None;
            state.apply_connectivity_at(connectivity(C::Connecting, false, false), joined);
        });
        assert_eq!(
            state.read_with(cx, |state, _| state.clock_deadlines().count()),
            0
        );
        state.update(cx, |state, _| {
            state.begin_pending_send("c-remote", "m", joined)
        });
        let deadlines: Vec<_> = state.read_with(cx, |state, _| state.clock_deadlines().collect());
        assert!(deadlines.contains(&(joined + TimeDelta::milliseconds(SYNC_STALLED_AFTER_MS))));
        state.update(cx, |state, _| {
            state.pending_sends.clear();
            state.selected_chat = Some("c-remote".into());
        });
        // A chat hosted here never arms one.
        state.update(cx, |state, _| {
            state.chats.retain(|chat| chat.id != "c-remote");
            let mut here = chat("c-remote", 0, None);
            here.device_id = "local".into();
            state.chats.push(here);
        });
        assert_eq!(
            state.read_with(cx, |state, _| state.clock_deadlines().count()),
            0
        );
    }

    /// A never-synced copy of a remote chat replays as empty: it must read
    /// as loading — opening, then syncing — not as a chat with no messages.
    #[test]
    fn cold_remote_chat_reads_as_loading_not_empty() {
        use zeron_proto::{ChatSyncState as C, ConnectivityState};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, _requests) = tokio::sync::mpsc::channel::<String>(8);
        let (_replies, inbound) = tokio::sync::mpsc::channel::<String>(8);
        let t0 = clock_epoch();
        let mut s = remote_chat_state(t0);
        s.selected_chat = Some("c-remote".into());
        // No engine: nothing can be loading.
        assert_eq!(s.transcript_loading(t0), None);
        s.set_test_engine(EngineHandle::from_test_client(zeron_rpc::RpcClient::new(
            out, inbound,
        )));
        // Selected, not yet replayed, before any connectivity frame.
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Opening));
        // Replayed empty while the room the replay opened has yet to speak.
        s.transcript_replayed = true;
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Syncing));
        s.apply_connectivity_at(connectivity(C::Waiting, true, false), t0);
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Syncing));
        let stalled = t0 + TimeDelta::milliseconds(SYNC_STALLED_AFTER_MS);
        s.apply_connectivity_at(connectivity(C::Connecting, false, false), t0);
        assert_eq!(
            s.transcript_loading(stalled),
            Some(TranscriptLoading::Reconnecting)
        );
        let mut offline = connectivity(C::Offline, false, false);
        offline.state = ConnectivityState::Offline;
        s.apply_connectivity_at(offline, t0);
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Offline));
        // Caught up: an empty chat really is empty.
        s.apply_connectivity_at(connectivity(C::Synced, true, true), t0);
        assert_eq!(s.transcript_loading(t0), None);
        // Neither a storage failure nor a dormant chat keeps it loading.
        for sync_state in [C::StorageError, C::Local] {
            s.apply_connectivity_at(connectivity(sync_state, false, false), t0);
            assert_eq!(s.transcript_loading(t0), None, "{sync_state:?}");
        }
        s.apply_connectivity_at(connectivity(C::Connecting, true, false), t0);
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Syncing));
        // An optimistic echo or any entry is something to show.
        s.echoes
            .entry("c-remote".into())
            .or_default()
            .push(user_entry("m"));
        assert_eq!(s.transcript_loading(t0), None);
        s.echoes.clear();
        s.transcript = vec![user_entry("m")];
        assert_eq!(s.transcript_loading(t0), None);
        s.transcript.clear();
        // A chat hosted here is authoritative from its own disk.
        s.selected_chat = Some("c-local".into());
        s.transcript_replayed = false;
        assert_eq!(s.transcript_loading(t0), None);
        // So is every chat on a local profile, once that is known.
        s.selected_chat = Some("c-remote".into());
        s.apply_connectivity_at(zeron_proto::Connectivity::default(), t0);
        assert_eq!(s.transcript_loading(t0), None);
        // An unsaved side chat has nothing to load.
        s.connectivity_observed = false;
        assert_eq!(s.transcript_loading(t0), Some(TranscriptLoading::Opening));
        s.unsaved_side_chat = true;
        assert_eq!(s.transcript_loading(t0), None);
    }
}

#[cfg(any(
    feature = "appshots-fixture",
    feature = "pull-request-fixture",
    feature = "project-path-fixture"
))]
impl AppState {
    /// Keep fixture documents deterministic while using the real attachment RPC.
    pub fn fixture_attachment_engine(&mut self, engine: EngineHandle) {
        self.engine = Some(engine);
    }
}

#[cfg(any(test, feature = "project-palette-fixture"))]
impl AppState {
    /// Seed provider metadata for the isolated native sidebar review fixture
    /// and for tests that click a sidebar badge.
    pub fn fixture_sidebar_change_request(
        &mut self,
        snapshot: zeron_proto::CheckoutChangeRequestStatus,
    ) {
        let key = crate::change_requests::ChangeRequestWatchKey {
            device_id: snapshot.device_id.clone(),
            cwd: snapshot.cwd.clone(),
            branch: snapshot.branch.clone(),
            checkout_id: Some(snapshot.checkout_id.clone()),
        };
        self.change_requests.store(key, snapshot);
    }
}
