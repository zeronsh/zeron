//! [`Client`] — the account-scoped root object.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio_util::sync::CancellationToken;
use zeron_doc::RegistryDoc;
use zeron_proto::{Chat, ChatConfig, SidebarPinChange, SidebarSectionChange};

use crate::attachments::{self, AttachmentCache};
use crate::auth::TokenProvider;
use crate::catalog::{self, HarnessInfo, ModelInfo};
use crate::config::{AuthTokens, ClientConfig, Credentials};
use crate::connectivity::{
    Connectivity, ConnectivityState, ConnectivityTracker, RawConnectivity, SendState,
};
use crate::demo::DemoHost;
use crate::error::{ClientError, Result};
use crate::events::{ClientEvent, ClientListener, EventPump};
use crate::live::LiveBackend;
use crate::rpc::{FolderListing, ProgressFn, QUEUED_ATTACHMENTS_MIN, RepoRef, capability};
use crate::session::{HostCapabilities, OutgoingAttachment, RoomState, SessionCore, SessionHandle};
use crate::workspace::{SearchHit, WorkspaceSnapshot, WorkspaceStore};
use crate::{lock, now_ms, read, write};

/// Attachment read cache budget.
const ATTACHMENT_CACHE_BYTES: usize = 48 * 1024 * 1024;
/// Time-driven re-derivation cadence (staleness, presence, send grace).
const TICK: Duration = Duration::from_secs(1);
/// Detached sessions kept warm (doc + room) before the least recently used
/// is evicted. On-screen sessions, streaming ones and ones with unadopted
/// sends are never evicted.
pub const WARM_SESSION_CAP: usize = 6;
/// Sessions `preload_sessions` warms (front page order).
pub const PRELOAD_CAP: usize = 4;

/// Where a new session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// In a project: host = the project's owning device, cwd = its folder
    /// (or `cwd` override, e.g. a picked worktree).
    Project { space_id: String },
    /// No project: runs in the chosen host's home folder (`~`).
    Projectless { device_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSession {
    pub target: SessionTarget,
    pub config: Option<ChatConfig>,
    /// Branch to stamp from the first frame (picked ref).
    pub branch: Option<String>,
    /// Checkout override (an existing worktree's path).
    pub cwd: Option<String>,
    pub title: Option<String>,
}

pub(crate) enum Backend {
    Demo(Arc<DemoHost>),
    Live(Box<LiveBackend>),
}

pub(crate) struct ClientInner {
    pub(crate) config: ClientConfig,
    pub(crate) credentials: Credentials,
    pub(crate) events: Arc<EventPump>,
    pub(crate) workspace: WorkspaceStore,
    pub(crate) tokens: TokenProvider,
    pub(crate) attachment_cache: AttachmentCache,
    backend: OnceLock<Backend>,
    sessions: Mutex<HashMap<String, Arc<SessionCore>>>,
    pub(crate) cancel: CancellationToken,
    connectivity_tracker: Mutex<ConnectivityTracker>,
    connectivity: RwLock<Connectivity>,
    path_online: AtomicBool,
    foreground: AtomicBool,
    synced: AtomicBool,
    harness_catalogs: Mutex<HashMap<String, Vec<HarnessInfo>>>,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        // Released without `shutdown()`: stop every background task.
        self.cancel.cancel();
    }
}

impl ClientInner {
    pub(crate) fn backend(&self) -> &Backend {
        self.backend.get().expect("backend initialized")
    }

    pub(crate) fn demo(&self) -> Option<&Arc<DemoHost>> {
        match self.backend() {
            Backend::Demo(demo) => Some(demo),
            Backend::Live(_) => None,
        }
    }

    pub(crate) fn is_demo(&self) -> bool {
        self.demo().is_some()
    }

    pub(crate) fn live(&self) -> Option<&LiveBackend> {
        match self.backend.get()? {
            Backend::Live(live) => Some(live),
            Backend::Demo(_) => None,
        }
    }

    pub(crate) fn session_core(&self, chat_id: &str) -> Option<Arc<SessionCore>> {
        lock(&self.sessions).get(chat_id).cloned()
    }

    pub(crate) fn cores(&self) -> Vec<Arc<SessionCore>> {
        lock(&self.sessions).values().cloned().collect()
    }

    pub(crate) fn network_online(&self) -> bool {
        self.path_online.load(Ordering::Acquire)
    }

    /// Re-derive the workspace snapshot; on change, notify and refresh every
    /// open composer (host presence / live status feed them).
    pub(crate) fn recompute_workspace(self: &Arc<Self>) {
        let send_states: HashMap<String, SendState> = self
            .cores()
            .iter()
            .filter_map(|core| core.send_state().map(|s| (core.chat_id.clone(), s)))
            .collect();
        let synced = self.synced.load(Ordering::Acquire);
        if let Some(revision) =
            self.workspace
                .recompute(&self.config.device_id, &send_states, synced)
        {
            self.events.workspace(revision);
            // Host presence / live status feed every open session's snapshot
            // (working flag) and composer; `refresh` is O(1) when no doc
            // event is pending.
            for core in self.cores() {
                core.refresh();
            }
        }
    }

    /// After any local registry write: settle (demo) / push (live), re-derive.
    pub(crate) fn after_registry_write(self: &Arc<Self>) {
        match self.backend() {
            Backend::Demo(demo) => demo.settle_registry(&self.workspace),
            Backend::Live(live) => live.registry_written(),
        }
        self.recompute_workspace();
    }

    /// Live: registry rows/acks applied (or (re)joined).
    pub(crate) fn registry_changed(self: &Arc<Self>) {
        let Some(live) = self.live() else { return };
        let posture = live.registry_posture();
        if posture.synced {
            self.synced.store(true, Ordering::Release);
            // Initialize prefs / prune pins of deleted chats (authoritative
            // only once a server state applied — never on a cold replica).
            match self
                .workspace
                .mutate(|doc| doc.reconcile_sidebar_pins(true))
            {
                Ok(true) => live.registry_written(),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "sidebar pin reconcile failed"),
            }
        }
        live.mark_registry_dirty();
        if let Some(presence) = live.presence() {
            self.workspace.replace_presence(presence);
        }
        self.recompute_connectivity();
        self.recompute_workspace();
        self.reconcile_change_request_watches();
        // New rows may flip a chat's roomGen / host: attach rooms.
        for core in self.cores() {
            core.ensure_room(self);
        }
    }

    /// Live: a presence beat arrived (or the local beat ticked).
    pub(crate) fn presence_changed(self: &Arc<Self>) {
        let Some(live) = self.live() else { return };
        if let Some(presence) = live.presence() {
            let before = self.workspace.presence();
            let now = now_ms();
            for (device, at) in &presence {
                let was_fresh = before
                    .get(device)
                    .is_some_and(|b| now - b < crate::connectivity::PRESENCE_FRESH_MS);
                if !was_fresh && now - at < crate::connectivity::PRESENCE_FRESH_MS {
                    live.relay.peer_alive(device);
                }
            }
            self.workspace.replace_presence(presence);
        }
        self.recompute_workspace();
    }

    pub(crate) fn connectivity_changed(self: &Arc<Self>) {
        self.recompute_connectivity();
    }

    /// PR-status streams for every active chat with a branch (per
    /// `(device, repo root, branch)`), only while foregrounded.
    pub(crate) fn reconcile_change_request_watches(self: &Arc<Self>) {
        let Some(live) = self.live() else { return };
        if !self.foreground.load(Ordering::Acquire) {
            live.relay.stop_watches();
            return;
        }
        let (state, _) = self.workspace.state();
        let targets = state
            .chats
            .iter()
            .filter(|c| !c.archived && c.parent_chat_id.is_none())
            .filter_map(|c| {
                let source = c.source_context.as_ref()?;
                let branch = source.branch.trim();
                (!branch.is_empty() && !source.repo_root.is_empty()).then(|| {
                    crate::live::relay::WatchKey {
                        device_id: c.device_id.clone(),
                        cwd: source.repo_root.clone(),
                        branch: branch.to_owned(),
                    }
                })
            })
            .collect();
        live.relay.reconcile_watches(self, targets);
    }

    /// Best-effort durable wake for the chat's host (`POST /device/{id}/nudge`).
    /// Waits (briefly) for our own pending registry writes — e.g. a fresh
    /// chat's CreateChat row — to reach the edge first, so the host never
    /// wakes for a chat it can't see yet.
    pub(crate) fn nudge_host(self: &Arc<Self>, host: &str, chat_id: &str) {
        let Some(live) = self.live() else { return };
        let url = crate::live::urls::nudge(&live.edge, host);
        let inner = Arc::downgrade(self);
        let chat_id = chat_id.to_owned();
        crate::runtime::shared().spawn(async move {
            for _ in 0..50 {
                let Some(inner) = inner.upgrade() else { return };
                if inner.workspace.mutate(|doc| doc.pending_len()) == 0 {
                    break;
                }
                drop(inner);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let Some(inner) = inner.upgrade() else { return };
            let Ok(token) = inner.tokens.bearer().await else {
                return;
            };
            drop(inner);
            for attempt in 0..3u64 {
                let sent = crate::auth::http()
                    .post(&url)
                    .bearer_auth(&token)
                    .json(&serde_json::json!({ "chatId": chat_id }))
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await;
                match sent {
                    // 503 nudge_queue_full is retryable.
                    Ok(response) if response.status().as_u16() == 503 => {
                        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                    }
                    _ => return,
                }
            }
        });
    }

    pub(crate) fn registry_write<R>(
        self: &Arc<Self>,
        f: impl FnOnce(&mut RegistryDoc) -> std::result::Result<R, zeron_doc::DocError>,
    ) -> Result<R> {
        let result = self.workspace.mutate(f)?;
        self.after_registry_write();
        Ok(result)
    }

    pub(crate) fn mark_seen(self: &Arc<Self>, chat_id: &str) {
        let _ = self.registry_write(|doc| doc.set_chat_seen(chat_id, Utc::now()));
    }

    pub(crate) fn recompute_connectivity(self: &Arc<Self>) {
        let raw = match self.live() {
            None => RawConnectivity {
                path_offline: !self.network_online(),
                registry_connected: true,
                registry_retry_at_ms: None,
                last_failure: None,
                chat_rooms: Vec::new(),
            },
            Some(live) => {
                let posture = live.registry_posture();
                RawConnectivity {
                    path_offline: !self.network_online(),
                    registry_connected: posture.connected,
                    registry_retry_at_ms: posture.retry_at_ms,
                    last_failure: posture.last_failure,
                    chat_rooms: self
                        .cores()
                        .iter()
                        .filter_map(|core| {
                            let room = core.room()?;
                            Some((core.chat_id.clone(), room.connected(), room.retry_at_ms()))
                        })
                        .collect(),
                }
            }
        };
        let next = lock(&self.connectivity_tracker).compute(&raw, now_ms());
        let changed = {
            let mut current = write(&self.connectivity);
            if *current == next {
                false
            } else {
                *current = next.clone();
                true
            }
        };
        if changed {
            self.events.ordered(ClientEvent::ConnectivityChanged(next));
            for core in self.cores() {
                core.refresh();
            }
            if let Some(demo) = self.demo()
                && self.network_online()
            {
                demo.network_recovered();
            }
        }
    }

    pub(crate) fn connectivity(&self) -> Connectivity {
        read(&self.connectivity).clone()
    }

    /// Whether a send to this chat would queue rather than deliver promptly
    /// (legacy `chatDeliveryDegraded`): OS offline, the chat's room degraded,
    /// the registry down with no room, or the host presence-dark.
    pub(crate) fn chat_delivery_degraded(&self, chat_id: &str) -> bool {
        let connectivity = self.connectivity();
        if connectivity.state == ConnectivityState::Offline {
            return true;
        }
        if connectivity.degraded_chats.iter().any(|c| c == chat_id) {
            return true;
        }
        if !matches!(
            connectivity.state,
            ConnectivityState::Connected | ConnectivityState::Disabled
        ) {
            return true;
        }
        self.workspace
            .snapshot()
            .session(chat_id)
            .is_some_and(|row| !row.device_online)
    }

    pub(crate) fn room_state(&self, chat_id: &str) -> RoomState {
        let connectivity = self.connectivity();
        let degraded = connectivity.degraded_chats.iter().any(|c| c == chat_id);
        match self.session_core(chat_id).and_then(|core| core.room()) {
            Some(room) => RoomState {
                connected: room.connected(),
                retry_at_ms: room.retry_at_ms().or(connectivity.retry_at_ms),
                degraded,
            },
            None => RoomState {
                connected: connectivity.state != ConnectivityState::Offline,
                retry_at_ms: connectivity.retry_at_ms,
                degraded,
            },
        }
    }

    pub(crate) fn host_capabilities(
        &self,
        device_id: &str,
        harness: Option<&str>,
    ) -> HostCapabilities {
        let workspace = self.workspace.snapshot();
        let Some(device) = workspace.device(device_id) else {
            return HostCapabilities::default();
        };
        let has = |cap: &str| device.capabilities.iter().any(|c| c == cap);
        let mid_turn_steering = harness.and_then(|harness| {
            lock(&self.harness_catalogs)
                .get(device_id)
                .and_then(|list| list.iter().find(|h| h.id == harness))
                .and_then(HarnessInfo::mid_turn_steering)
        });
        HostCapabilities {
            message_queue: has(capability::MESSAGE_QUEUE_V1),
            queue_actions: has(capability::MESSAGE_QUEUE_ACTIONS_V1),
            queue_attachments: has(capability::MESSAGE_QUEUE_ATTACHMENTS_V1),
            clean_attachment_text: has(capability::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1),
            queue_edit_lease: has(capability::MESSAGE_QUEUE_EDIT_LEASE_V1),
            queued_attachments: device
                .version
                .as_deref()
                .and_then(zeron_proto::version_triple)
                .is_some_and(|v| v >= QUEUED_ATTACHMENTS_MIN),
            mid_turn_steering,
        }
    }

    /// Mint `pending://` refs for picked images and keep their bytes local
    /// (the echo renders them immediately; the escort pushes them).
    pub(crate) fn stage_attachments(
        &self,
        chat_id: &str,
        device_id: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<Vec<String>> {
        if let Some(too_big) = attachments
            .iter()
            .find(|a| a.data.len() > attachments::MAX_ATTACHMENT_BYTES)
        {
            return Err(ClientError::InvalidArgument(format!(
                "{} is larger than 24 MB",
                too_big.name
            )));
        }
        attachments
            .iter()
            .map(|attachment| {
                let upload_id = crate::new_id();
                let name = attachments::upload_file_name(&attachment.name);
                let reference = attachments::pending_ref(&upload_id, &name);
                if let Some(live) = self.live() {
                    let meta = crate::live::escort::StashMeta {
                        upload_id: upload_id.clone(),
                        chat_id: chat_id.to_owned(),
                        host_device_id: device_id.to_owned(),
                        name: name.clone(),
                        created_at_ms: now_ms(),
                    };
                    live.escorts
                        .stash(&meta, &attachment.data)
                        .map_err(|e| ClientError::Storage(e.to_string()))?;
                }
                self.attachment_cache
                    .put(device_id, &reference, Arc::new(attachment.data.clone()));
                Ok(reference)
            })
            .collect()
    }

    /// A command/queue row was written: wake the host (and escort any
    /// staged attachment bytes).
    pub(crate) fn after_command(self: &Arc<Self>, core: &Arc<SessionCore>, has_attachments: bool) {
        match self.backend() {
            Backend::Demo(demo) => demo.on_command(&core.chat_id),
            Backend::Live(live) => {
                if has_attachments {
                    live.escorts.respawn_chat(self, &core.chat_id);
                }
                if let Some(host) = self.workspace.chat(&core.chat_id).map(|c| c.device_id) {
                    self.nudge_host(&host, &core.chat_id);
                }
            }
        }
    }

    pub(crate) async fn host_rpc(
        self: &Arc<Self>,
        device_id: &str,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match self.backend() {
            Backend::Demo(demo) => demo.host_rpc(device_id, method, params).await,
            Backend::Live(live) => live.relay.call(device_id, method, params).await,
        }
    }

    /// Fresh socket for the chat's room (retry delivery).
    pub(crate) fn kick_room(&self, chat_id: &str) {
        if let Some(room) = self.session_core(chat_id).and_then(|c| c.room()) {
            room.redial();
        }
        if let Some(live) = self.live() {
            live.kick();
        }
    }

    /// Drop the least recently used detached, quiet sessions past the cap.
    pub(crate) fn evict_sessions(&self) {
        let mut sessions = lock(&self.sessions);
        let mut idle: Vec<(i64, String)> = sessions
            .values()
            .filter(|core| {
                !core.view_attached() && !core.has_pending_sends() && !core.snapshot().streaming
            })
            .map(|core| (core.touched_ms(), core.chat_id.clone()))
            .collect();
        if idle.len() <= WARM_SESSION_CAP {
            return;
        }
        idle.sort();
        let excess = idle.len() - WARM_SESSION_CAP;
        for (_, chat_id) in idle.into_iter().take(excess) {
            tracing::debug!(chat = %chat_id, "evicting warm session");
            sessions.remove(&chat_id);
        }
    }

    fn tick(self: &Arc<Self>) {
        if let Some(live) = self.live() {
            if let Some(presence) = live.presence() {
                self.workspace.replace_presence(presence);
            }
            let peers_known = self
                .workspace
                .snapshot()
                .devices
                .iter()
                .any(|d| d.is_execution_host);
            live.check_registry_liveness(peers_known);
        }
        self.recompute_connectivity();
        self.recompute_workspace();
        for core in self.cores() {
            if core.has_pending_sends() {
                core.refresh();
            }
        }
    }
}

/// One signed-in account (or Demo mode). Cheap to clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("device_id", &self.inner.config.device_id)
            .field("demo", &self.inner.is_demo())
            .finish()
    }
}

impl Client {
    /// Build and start. Never blocks on the network: live mode hydrates from
    /// `data_dir` and connects in the background; Demo seeds its dataset.
    pub fn new(
        config: ClientConfig,
        credentials: Credentials,
        listener: Arc<dyn ClientListener>,
    ) -> Result<Self> {
        if config.device_id.trim().is_empty() {
            return Err(ClientError::InvalidArgument("device_id is required".into()));
        }
        let events = EventPump::new(listener);
        let tokens = TokenProvider::new(&credentials, config.edge_base(), events.clone());
        // Live: restore the registry replica + open the docs store first, so
        // the very first snapshot renders the cached workspace (instant).
        let (registry, store) = if credentials.is_demo() {
            (RegistryDoc::new(config.device_id.clone()), None)
        } else {
            let (store, registry) = LiveBackend::open(&config.data_dir, &config.device_id)?;
            (registry, Some(store))
        };
        let inner = Arc::new(ClientInner {
            workspace: WorkspaceStore::new(registry),
            events: events.clone(),
            tokens,
            attachment_cache: AttachmentCache::new(ATTACHMENT_CACHE_BYTES),
            backend: OnceLock::new(),
            sessions: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
            connectivity_tracker: Mutex::new(ConnectivityTracker::default()),
            connectivity: RwLock::new(Connectivity {
                state: if credentials.is_demo() {
                    ConnectivityState::Connected
                } else {
                    ConnectivityState::Reconnecting
                },
                ..Default::default()
            }),
            path_online: AtomicBool::new(true),
            foreground: AtomicBool::new(true),
            synced: AtomicBool::new(false),
            harness_catalogs: Mutex::new(HashMap::new()),
            credentials: credentials.clone(),
            config,
        });
        let backend = match &credentials {
            Credentials::Demo(options) => {
                let demo = DemoHost::new(&inner, options.clone());
                demo.seed(&inner)?;
                inner.synced.store(true, Ordering::Release);
                Backend::Demo(demo)
            }
            _ => Backend::Live(Box::new(LiveBackend::new(
                &inner,
                store.ok_or_else(|| ClientError::Internal("docs store missing".into()))?,
            ))),
        };
        let _ = inner.backend.set(backend);
        events.start(inner.cancel.clone());
        inner.recompute_workspace();
        match inner.backend() {
            Backend::Demo(demo) => demo.start(&inner, inner.cancel.clone()),
            Backend::Live(live) => {
                live.start(&inner);
                inner.recompute_connectivity();
            }
        }
        let ticker = Arc::downgrade(&inner);
        let cancel = inner.cancel.clone();
        crate::runtime::shared().spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = interval.tick() => {}
                }
                let Some(inner) = ticker.upgrade() else {
                    return;
                };
                if inner.foreground.load(Ordering::Acquire) {
                    inner.tick();
                }
            }
        });
        Ok(Self { inner })
    }

    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    pub fn is_demo(&self) -> bool {
        self.inner.is_demo()
    }

    pub fn device_id(&self) -> &str {
        &self.inner.config.device_id
    }

    pub fn user_id(&self) -> &str {
        self.inner.credentials.user_id()
    }

    pub fn org_id(&self) -> &str {
        self.inner.credentials.org_id()
    }

    /// Stop every background task (sign-out). Snapshots stay readable.
    pub fn shutdown(&self) {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.stop(),
            Backend::Live(live) => {
                live.flush_registry(&self.inner);
                live.stop();
            }
        }
        self.inner.cancel.cancel();
        // Dropping the cores stops their rooms (each flushes its snapshot).
        lock(&self.inner.sessions).clear();
        self.inner.attachment_cache.clear();
    }

    /// The platform restored/re-signed a newer WorkOS pair.
    pub fn update_tokens(&self, tokens: AuthTokens) {
        self.inner.tokens.update_tokens(tokens);
    }

    // ── workspace reads ────────────────────────────────────────────────────

    /// O(1): the current derived workspace.
    pub fn workspace(&self) -> Arc<WorkspaceSnapshot> {
        self.inner.workspace.snapshot()
    }

    pub fn connectivity(&self) -> Connectivity {
        self.inner.connectivity()
    }

    /// The chat's run configuration (harness/model/effort/options/sandbox).
    pub fn session_config(&self, chat_id: &str) -> Option<ChatConfig> {
        self.inner.workspace.chat(chat_id).and_then(|c| c.config)
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.workspace().search(query, limit)
    }

    // ── workspace writes ───────────────────────────────────────────────────

    /// Mint a chat row (born on chat2: `roomGen: 2`). The row is written
    /// before any session doc opens, so the host never reads it as legacy.
    pub fn create_session(&self, new: NewSession) -> Result<String> {
        let (state, _) = self.inner.workspace.state();
        let (device_id, space_id, cwd) = match &new.target {
            SessionTarget::Project { space_id } => {
                let space = state
                    .spaces
                    .iter()
                    .find(|s| &s.id == space_id)
                    .ok_or_else(|| ClientError::NotFound(space_id.clone()))?;
                (
                    space.device_id.clone(),
                    Some(space.id.clone()),
                    new.cwd.clone().unwrap_or_else(|| space.path.clone()),
                )
            }
            SessionTarget::Projectless { device_id } => {
                let host = state
                    .devices
                    .iter()
                    .find(|d| &d.id == device_id)
                    .ok_or_else(|| ClientError::NotFound(device_id.clone()))?;
                if matches!(host.platform.as_str(), "ios" | "android" | "ipados") {
                    return Err(ClientError::InvalidArgument(
                        "that device can't host sessions".into(),
                    ));
                }
                (device_id.clone(), None, "~".to_owned())
            }
        };
        let now = Utc::now();
        let chat = Chat {
            id: crate::new_id(),
            device_id,
            title: new.title.filter(|t| !t.trim().is_empty()),
            archived: false,
            cwd: Some(cwd),
            branch: new.branch,
            checkout_id: None,
            source_context: None,
            config: new.config,
            last_message_preview: None,
            last_message_at: None,
            created_at: now,
            harness_session_id: None,
            harness_session_cwd: None,
            space_id,
            last_seen_at: None,
            room_gen: Some(2),
            parent_chat_id: None,
        };
        let id = chat.id.clone();
        self.inner.registry_write(|doc| doc.upsert_chat(&chat))?;
        Ok(id)
    }

    fn chat_write(
        &self,
        chat_id: &str,
        f: impl FnOnce(&mut RegistryDoc) -> std::result::Result<bool, zeron_doc::DocError>,
    ) -> Result<()> {
        if self.inner.registry_write(f)? {
            Ok(())
        } else {
            Err(ClientError::NotFound(chat_id.to_owned()))
        }
    }

    pub fn archive_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.set_chat_archived(chat_id, true))
    }

    pub fn unarchive_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.set_chat_archived(chat_id, false))
    }

    pub fn rename_session(&self, chat_id: &str, title: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.rename_chat(chat_id, title.trim()))
    }

    pub fn mark_seen(&self, chat_id: &str) {
        self.inner.mark_seen(chat_id);
    }

    pub fn set_session_config(&self, chat_id: &str, config: &ChatConfig) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.set_chat_config(chat_id, config))
    }

    /// Remove the chat row (the doc itself stays on the edge).
    pub fn delete_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.delete_chat(chat_id))?;
        let core = lock(&self.inner.sessions).remove(chat_id);
        drop(core);
        if let Some(live) = self.inner.live()
            && let Err(err) = live.store.delete_snapshot(chat_id)
        {
            tracing::debug!(error = %err, "local chat snapshot delete failed");
        }
        Ok(())
    }

    /// Mid-session ref switch: retarget onto the ref's existing worktree (row
    /// writes only), else `git checkout` in the session's own folder.
    pub async fn switch_session_ref(&self, chat_id: &str, reference: RepoRef) -> Result<()> {
        let chat = self
            .inner
            .workspace
            .chat(chat_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.to_owned()))?;
        let cwd = chat
            .cwd
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("session has no working folder".into()))?;
        if let Some(worktree) = reference.worktree_path.clone() {
            if worktree != cwd {
                self.inner.registry_write(|doc| {
                    doc.set_chat_cwd(chat_id, &worktree)?;
                    doc.set_chat_branch(chat_id, &reference.name)
                })?;
            }
            return Ok(());
        }
        self.switch_ref(&chat.device_id, &cwd, &reference.name)
            .await?;
        self.inner
            .registry_write(|doc| doc.set_chat_branch(chat_id, &reference.name))?;
        Ok(())
    }

    fn pin_change(&self, change: SidebarPinChange) -> Result<()> {
        if !self.workspace().pins_ready {
            return Err(ClientError::Unsupported("pins are not synced yet".into()));
        }
        self.inner
            .registry_write(|doc| doc.change_sidebar_pin(&change))
    }

    pub fn pin_session(&self, chat_id: &str) -> Result<()> {
        let after = self
            .inner
            .workspace
            .state()
            .1
            .and_then(|p| p.pinned_session_ids.last().cloned())
            .filter(|last| last != chat_id);
        self.pin_change(SidebarPinChange::Pin {
            session_id: chat_id.to_owned(),
            after,
            before: None,
        })
    }

    pub fn unpin_session(&self, chat_id: &str) -> Result<()> {
        self.pin_change(SidebarPinChange::Unpin {
            session_id: chat_id.to_owned(),
        })
    }

    /// Reorder a pin between neighbours (either may be `None` at the ends).
    pub fn move_pin(
        &self,
        chat_id: &str,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.pin_change(SidebarPinChange::Move {
            session_id: chat_id.to_owned(),
            after,
            before,
        })
    }

    fn section_change(&self, change: SidebarSectionChange) -> Result<()> {
        self.pin_change(SidebarPinChange::Section { change })
    }

    pub fn create_section(&self, name: &str) -> Result<String> {
        let id = crate::new_id();
        self.section_change(SidebarSectionChange::Create {
            id: id.clone(),
            name: name.trim().to_owned(),
        })?;
        Ok(id)
    }

    pub fn rename_section(&self, section_id: &str, name: &str) -> Result<()> {
        self.section_change(SidebarSectionChange::Rename {
            id: section_id.to_owned(),
            name: name.trim().to_owned(),
        })
    }

    /// Delete a section (its sessions return to the recent list).
    pub fn delete_section(&self, section_id: &str) -> Result<()> {
        self.section_change(SidebarSectionChange::Delete {
            id: section_id.to_owned(),
        })
    }

    pub fn set_section_collapsed(&self, section_id: &str, collapsed: bool) -> Result<()> {
        self.section_change(SidebarSectionChange::Collapse {
            id: section_id.to_owned(),
            collapsed,
        })
    }

    /// Move a session into a section (`None` = back to the recent list).
    pub fn assign_section(&self, chat_id: &str, section_id: Option<String>) -> Result<()> {
        self.section_change(SidebarSectionChange::Assign {
            session_id: chat_id.to_owned(),
            section_id,
        })
    }

    /// Create a project on `device_id` (deduped on device + path). Live
    /// mode asks the owning host (`Mutate {createSpace}`) and falls back to
    /// a local row write when it is unreachable.
    pub async fn create_project(
        &self,
        device_id: &str,
        path: &str,
        git_detected: bool,
    ) -> Result<String> {
        let (state, _) = self.inner.workspace.state();
        if let Some(existing) = state
            .spaces
            .iter()
            .find(|s| s.device_id == device_id && s.path == path)
        {
            return Ok(existing.id.clone());
        }
        let space = zeron_proto::Space {
            id: crate::new_id(),
            device_id: device_id.to_owned(),
            path: path.to_owned(),
            name: None,
            git_detected,
            git_checked_at: None,
            checkout_id: None,
            created_at: Utc::now(),
        };
        let id = space.id.clone();
        // The owning host writes the row itself (it knows git state and runs
        // the project's setup); an unreachable host gets a local row write
        // it will adopt later.
        if let Some(live) = self.inner.live() {
            let asked = live
                .relay
                .call(
                    device_id,
                    zeron_rpc::methods::MUTATE,
                    serde_json::json!({
                        "op": "createSpace",
                        "spaceId": id,
                        "deviceId": device_id,
                        "path": path,
                        "gitDetected": git_detected,
                    }),
                )
                .await;
            match asked {
                Ok(_) => {
                    // Wait (briefly) for the host's row so the caller can
                    // start a session in it right away.
                    for _ in 0..50 {
                        if self
                            .inner
                            .workspace
                            .state()
                            .0
                            .spaces
                            .iter()
                            .any(|s| s.id == id)
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    return Ok(id);
                }
                Err(err) => {
                    tracing::info!(error = %err, "createSpace via host failed; writing locally")
                }
            }
        }
        self.inner.registry_write(|doc| doc.upsert_space(&space))?;
        Ok(id)
    }

    pub fn rename_project(&self, space_id: &str, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if self
            .inner
            .registry_write(|doc| doc.rename_space(space_id, name))?
        {
            Ok(())
        } else {
            Err(ClientError::NotFound(space_id.to_owned()))
        }
    }

    /// Delete a project and cascade its chats' rows.
    pub fn delete_project(&self, space_id: &str) -> Result<()> {
        let deleted = self
            .inner
            .registry_write(|doc| doc.delete_space(space_id))?;
        let mut sessions = lock(&self.inner.sessions);
        for chat_id in deleted.chat_ids {
            sessions.remove(&chat_id);
        }
        Ok(())
    }

    // ── sessions ───────────────────────────────────────────────────────────

    /// Open (or return the already-open) session. Instant: the local
    /// snapshot renders first; sync follows in the background.
    pub fn open_session(&self, chat_id: &str) -> Result<SessionHandle> {
        if let Some(core) = self.inner.session_core(chat_id) {
            return Ok(SessionHandle { core });
        }
        if self.inner.workspace.chat(chat_id).is_none() {
            return Err(ClientError::NotFound(chat_id.to_owned()));
        }
        let (doc, cursor, hydrated) = match self.inner.backend() {
            Backend::Demo(demo) => (demo.session_doc(chat_id)?, 0, true),
            Backend::Live(live) => {
                let local = crate::live::room::load_local(&live.store, chat_id);
                (local.doc, local.cursor, local.had_content)
            }
        };
        let core = SessionCore::new(chat_id, &self.inner, doc, cursor);
        let core = {
            let mut sessions = lock(&self.inner.sessions);
            sessions
                .entry(chat_id.to_owned())
                .or_insert_with(|| core.clone())
                .clone()
        };
        if hydrated {
            core.set_hydrated();
        }
        core.ensure_room(&self.inner);
        core.touch();
        core.refresh();
        if let Some(demo) = self.inner.demo() {
            demo.session_opened(&core);
        }
        self.inner.evict_sessions();
        Ok(SessionHandle { core })
    }

    /// An already-open session, if any.
    pub fn session(&self, chat_id: &str) -> Option<SessionHandle> {
        self.inner
            .session_core(chat_id)
            .map(|core| SessionHandle { core })
    }

    /// The view closed. The session stays warm (streaming continues) until
    /// the eviction policy reclaims it.
    pub fn close_session(&self, chat_id: &str) {
        if let Some(handle) = self.session(chat_id) {
            handle.set_view_attached(false);
        }
        self.inner.evict_sessions();
    }

    // ── host RPC ───────────────────────────────────────────────────────────

    /// Harness catalog of an execution device: live (cached to disk), else
    /// the last cached list, else the static fallback. Cached in memory for
    /// capability gating (mid-turn steering).
    pub async fn list_harnesses(&self, device_id: &str) -> Vec<HarnessInfo> {
        let list = match self.inner.backend() {
            Backend::Demo(demo) => demo.list_harnesses(device_id).await,
            Backend::Live(live) => {
                let cache = catalog::DiskCatalog::new(&self.inner.config.data_dir);
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::LIST_HARNESSES,
                        serde_json::json!({}),
                    )
                    .await
                    .and_then(|v| {
                        serde_json::from_value::<Vec<HarnessInfo>>(v)
                            .map_err(|e| ClientError::HostError(e.to_string()))
                    });
                match reply {
                    Ok(list) => {
                        let list: Vec<HarnessInfo> =
                            list.into_iter().filter(|h| h.id != "mock").collect();
                        cache.put_harnesses(device_id, &list);
                        list
                    }
                    Err(err) => {
                        tracing::debug!(device = %device_id, error = %err, "ListHarnesses failed; using cache");
                        cache
                            .harnesses(device_id)
                            .unwrap_or_else(catalog::fallback_harnesses)
                    }
                }
            }
        };
        lock(&self.inner.harness_catalogs).insert(device_id.to_owned(), list.clone());
        self.inner.recompute_workspace();
        for core in self.inner.cores() {
            core.recompute_composer(&self.inner);
        }
        list
    }

    /// Model catalog for `harness` on `device_id` (normalized live reply,
    /// else the cached one, else the curated static list).
    pub async fn list_models(&self, device_id: &str, harness: &str) -> Vec<ModelInfo> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_models(harness).await,
            Backend::Live(live) => {
                let cache = catalog::DiskCatalog::new(&self.inner.config.data_dir);
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::LIST_MODELS,
                        serde_json::json!({ "harness": harness }),
                    )
                    .await
                    .and_then(|v| {
                        serde_json::from_value::<Vec<ModelInfo>>(v)
                            .map_err(|e| ClientError::HostError(e.to_string()))
                    });
                match reply {
                    Ok(list) if !list.is_empty() => {
                        let list = catalog::normalize_models(harness, list);
                        cache.put_models(device_id, harness, &list);
                        list
                    }
                    _ => cache
                        .models(device_id, harness)
                        .unwrap_or_else(|| catalog::fallback_models(harness)),
                }
            }
        }
    }

    pub async fn list_refs(&self, device_id: &str, repo_path: &str) -> Result<Vec<RepoRef>> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_refs(repo_path).await,
            Backend::Live(live) => {
                let value = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::LIST_REFS,
                        serde_json::json!({ "repoPath": repo_path }),
                    )
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// Workspace file search for composer `@` mentions (host `SearchFiles`,
    /// gitignore-aware; an empty query returns the chat's featured files).
    pub async fn search_files(
        &self,
        device_id: &str,
        chat_id: Option<String>,
        space_id: Option<String>,
        query: &str,
    ) -> Result<Vec<zeron_proto::FileSearchMatch>> {
        match self.inner.backend() {
            Backend::Demo(_) => {
                const FILES: &[&str] = &[
                    "crates/mobile/src/layout/mod.rs",
                    "crates/mobile/src/layout/markdown.rs",
                    "crates/mobile/src/layout/rows.rs",
                    "crates/text/src/layout.rs",
                    "crates/text/src/prepare.rs",
                    "crates/markdown/src/parser.rs",
                    "apps/ios/Zeron/Transcript/TranscriptListView.swift",
                    "apps/ios/Zeron/Composer/ComposerBar.swift",
                    "docs/mobile-rewrite.md",
                    "README.md",
                ];
                let q = query.to_lowercase();
                Ok(FILES
                    .iter()
                    .filter(|p| q.is_empty() || p.to_lowercase().contains(&q))
                    .map(|p| zeron_proto::FileSearchMatch {
                        path: (*p).to_owned(),
                        is_dir: false,
                    })
                    .collect())
            }
            Backend::Live(live) => {
                let value = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::SEARCH_FILES,
                        serde_json::json!({ "query": query, "chatId": chat_id, "spaceId": space_id }),
                    )
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// Browse folders on a device (`None` = its home folder).
    pub async fn list_folders(
        &self,
        device_id: &str,
        path: Option<String>,
    ) -> Result<FolderListing> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_folders(device_id, path).await,
            Backend::Live(live) => {
                let params = match path.filter(|p| !p.is_empty()) {
                    Some(path) => serde_json::json!({ "path": path }),
                    None => serde_json::json!({}),
                };
                let value = live
                    .relay
                    .call(device_id, zeron_rpc::methods::LIST_FOLDERS, params)
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// `git checkout <ref>` in `repo_path` on the device.
    pub async fn switch_ref(&self, device_id: &str, repo_path: &str, ref_name: &str) -> Result<()> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.switch_ref(repo_path, ref_name).await,
            Backend::Live(live) => live
                .relay
                .call(
                    device_id,
                    zeron_rpc::methods::SWITCH_REF,
                    serde_json::json!({ "repoPath": repo_path, "refName": ref_name }),
                )
                .await
                .map(|_| ()),
        }
    }

    /// Create a worktree for `branch` (an existing ref); returns its path.
    pub async fn create_worktree(
        &self,
        device_id: &str,
        space_id: &str,
        repo_path: &str,
        branch: &str,
    ) -> Result<String> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.create_worktree(repo_path, branch).await,
            Backend::Live(live) => {
                let mut params = serde_json::json!({ "repoPath": repo_path, "branch": branch });
                if !space_id.is_empty() {
                    params["spaceId"] = serde_json::Value::String(space_id.to_owned());
                }
                let value = live
                    .relay
                    .call(device_id, zeron_rpc::methods::CREATE_WORKTREE, params)
                    .await?;
                value
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| ClientError::HostError("CreateWorktree returned no path".into()))
            }
        }
    }

    /// Chunked upload of one file to the host; returns its durable path there.
    pub async fn upload_attachment(
        &self,
        device_id: &str,
        name: &str,
        data: Vec<u8>,
        progress: Option<ProgressFn>,
    ) -> Result<String> {
        if data.len() > attachments::MAX_ATTACHMENT_BYTES {
            return Err(ClientError::InvalidArgument(format!(
                "{name} is larger than 24 MB"
            )));
        }
        match self.inner.backend() {
            Backend::Demo(demo) => {
                demo.upload(&self.inner, device_id, name, data, progress)
                    .await
            }
            Backend::Live(live) => {
                let upload_id = crate::new_id();
                let file_name = attachments::upload_file_name(name);
                let path = live
                    .relay
                    .upload(device_id, &upload_id, &file_name, &data, progress)
                    .await?;
                self.inner
                    .attachment_cache
                    .put(device_id, &path, Arc::new(data));
                Ok(path)
            }
        }
    }

    /// Attachment bytes (LRU-cached). Accepts host paths and this device's
    /// own `pending://` refs.
    pub async fn read_attachment(&self, device_id: &str, path: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.inner.attachment_cache.get(device_id, path) {
            return Ok(hit);
        }
        let bytes = match self.inner.backend() {
            Backend::Demo(demo) => demo.read_attachment(path)?,
            Backend::Live(live) => Arc::new(live.relay.read_attachment(device_id, path).await?),
        };
        self.inner
            .attachment_cache
            .put(device_id, path, bytes.clone());
        Ok(bytes)
    }

    // ── lifecycle ──────────────────────────────────────────────────────────

    /// OS network path (NWPathMonitor): `false` only for a definitive
    /// "unsatisfied" — ambiguity stays online.
    pub fn set_network_online(&self, online: bool) {
        let was = self.inner.path_online.swap(online, Ordering::AcqRel);
        if !self.inner.is_demo() {
            // Parks/un-parks every sync backoff in the process (zeron-sync).
            zeron_sync::wake::set_path_online(online);
        }
        if was != online {
            self.inner.recompute_connectivity();
            if online && let Some(live) = self.inner.live() {
                live.kick();
                self.kick_rooms();
            }
        }
    }

    /// App returned to the foreground: resume ticking, probe the registry
    /// and every open room (liveness is judged by protocol frames only),
    /// restart PR watches.
    pub fn on_foreground(&self) {
        self.inner.foreground.store(true, Ordering::Release);
        if let Some(live) = self.inner.live() {
            live.kick();
            live.probe_health();
            self.kick_rooms();
            live.relay.clear_unsupported();
            self.inner.reconcile_change_request_watches();
        }
        self.inner.tick();
    }

    /// App is backgrounding: persist registry + docs now; pause time-driven
    /// work and PR streams.
    pub fn on_background(&self) {
        self.inner.foreground.store(false, Ordering::Release);
        if let Some(live) = self.inner.live() {
            live.relay.stop_watches();
            live.flush_registry(&self.inner);
            for core in self.inner.cores() {
                core.flush();
            }
        }
    }

    fn kick_rooms(&self) {
        for core in self.inner.cores() {
            if let Some(room) = core.room() {
                room.kick();
            }
        }
    }

    /// Warm the most relevant sessions (front page order: pinned, sections,
    /// recent), up to [`PRELOAD_CAP`]. Opening is instant (local snapshot);
    /// live rooms dial behind the client's dial cap.
    pub fn preload_sessions(&self) {
        let workspace = self.workspace();
        let front = &workspace.front;
        let candidates = front
            .pinned
            .iter()
            .chain(front.sections.iter().flat_map(|s| s.sessions.iter()))
            .chain(front.recent.iter())
            .filter(|row| row.room_gen >= 2);
        for row in candidates.take(PRELOAD_CAP) {
            if self.inner.session_core(&row.id).is_none() {
                let _ = self.open_session(&row.id);
            }
        }
    }

    /// Ids of the sessions currently open (warm), most recently used first.
    pub fn open_session_ids(&self) -> Vec<String> {
        let mut cores = self.inner.cores();
        cores.sort_by_key(|c| std::cmp::Reverse(c.touched_ms()));
        cores.iter().map(|c| c.chat_id.clone()).collect()
    }
}

/// Seed helpers shared with the demo host.
pub(crate) fn ms(at: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(at)
        .single()
        .unwrap_or_else(Utc::now)
}
