//! [`Client`] — the account-scoped root object.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio_util::sync::CancellationToken;
use zeron_doc::{RegistryDoc, SessionDoc};
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
use crate::rpc::{FolderListing, ProgressFn, QUEUED_ATTACHMENTS_MIN, RepoRef, capability};
use crate::session::{
    HostCapabilities, OutgoingAttachment, RoomState, SessionCore, SessionHandle,
};
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

/// Live-mode transport. Phase 1 wires auth + nudges; registry/chat2/relay
/// land in phase 2.
pub(crate) struct LiveBackend {
    edge_url: String,
}

pub(crate) enum Backend {
    Demo(Arc<DemoHost>),
    Live(LiveBackend),
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
    cancel: CancellationToken,
    connectivity_tracker: Mutex<ConnectivityTracker>,
    connectivity: RwLock<Connectivity>,
    path_online: AtomicBool,
    foreground: AtomicBool,
    synced: AtomicBool,
    harness_catalogs: Mutex<HashMap<String, Vec<HarnessInfo>>>,
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

    pub(crate) fn session_core(&self, chat_id: &str) -> Option<Arc<SessionCore>> {
        lock(&self.sessions).get(chat_id).cloned()
    }

    fn cores(&self) -> Vec<Arc<SessionCore>> {
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
        if let Some(demo) = self.demo() {
            demo.settle_registry(&self.workspace);
        }
        self.recompute_workspace();
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

    fn recompute_connectivity(self: &Arc<Self>) {
        let raw = RawConnectivity {
            path_offline: !self.network_online(),
            // Phase 2: the registry client's reconnect state.
            registry_connected: true,
            registry_retry_at_ms: None,
            last_failure: None,
            chat_rooms: Vec::new(),
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
        RoomState {
            connected: connectivity.state != ConnectivityState::Offline,
            retry_at_ms: connectivity.retry_at_ms,
            degraded: connectivity.degraded_chats.iter().any(|c| c == chat_id),
        }
    }

    pub(crate) fn host_capabilities(&self, device_id: &str, harness: Option<&str>) -> HostCapabilities {
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
        device_id: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<Vec<String>> {
        attachments
            .iter()
            .map(|attachment| {
                if attachment.data.len() > attachments::MAX_ATTACHMENT_BYTES {
                    return Err(ClientError::InvalidArgument(format!(
                        "{} is larger than 24 MB",
                        attachment.name
                    )));
                }
                let reference = attachments::pending_ref(&crate::new_id(), &attachment.name);
                self.attachment_cache
                    .put(device_id, &reference, Arc::new(attachment.data.clone()));
                Ok(reference)
            })
            .collect()
    }

    /// A command/queue row was written: wake the host.
    pub(crate) fn after_command(self: &Arc<Self>, core: &Arc<SessionCore>, _has_attachments: bool) {
        match self.backend() {
            Backend::Demo(demo) => demo.on_command(&core.chat_id),
            Backend::Live(live) => {
                let Some(host) = self.workspace.chat(&core.chat_id).map(|c| c.device_id) else {
                    return;
                };
                let inner = self.clone();
                let chat_id = core.chat_id.clone();
                let url = format!("{}/device/{host}/nudge", live.edge_url);
                crate::runtime::shared().spawn(async move {
                    let Ok(token) = inner.tokens.bearer().await else { return };
                    let _ = crate::auth::http()
                        .post(url)
                        .bearer_auth(token)
                        .json(&serde_json::json!({ "chatId": chat_id }))
                        .send()
                        .await;
                });
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
            Backend::Live(_) => Err(ClientError::NotImplemented(format!("relay {method}"))),
        }
    }

    pub(crate) fn kick_room(&self, _chat_id: &str) {}

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
        let registry = RegistryDoc::new(config.device_id.clone());
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
            _ => Backend::Live(LiveBackend {
                edge_url: inner.config.edge_base().to_owned(),
            }),
        };
        let _ = inner.backend.set(backend);
        events.start(inner.cancel.clone());
        inner.recompute_workspace();
        if let Some(demo) = inner.demo() {
            demo.start(&inner, inner.cancel.clone());
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
                let Some(inner) = ticker.upgrade() else { return };
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
        self.inner.cancel.cancel();
        if let Some(demo) = self.inner.demo() {
            demo.stop();
        }
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
        lock(&self.inner.sessions).remove(chat_id);
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
        self.switch_ref(&chat.device_id, &cwd, &reference.name).await?;
        self.inner
            .registry_write(|doc| doc.set_chat_branch(chat_id, &reference.name))?;
        Ok(())
    }

    fn pin_change(&self, change: SidebarPinChange) -> Result<()> {
        if !self.workspace().pins_ready {
            return Err(ClientError::Unsupported(
                "pins are not synced yet".into(),
            ));
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
    pub fn move_pin(&self, chat_id: &str, after: Option<String>, before: Option<String>) -> Result<()> {
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
    pub async fn create_project(&self, device_id: &str, path: &str, git_detected: bool) -> Result<String> {
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
        self.inner.registry_write(|doc| doc.upsert_space(&space))?;
        Ok(id)
    }

    pub fn rename_project(&self, space_id: &str, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if self.inner.registry_write(|doc| doc.rename_space(space_id, name))? {
            Ok(())
        } else {
            Err(ClientError::NotFound(space_id.to_owned()))
        }
    }

    /// Delete a project and cascade its chats' rows.
    pub fn delete_project(&self, space_id: &str) -> Result<()> {
        let deleted = self.inner.registry_write(|doc| doc.delete_space(space_id))?;
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
        let doc = match self.inner.demo() {
            Some(demo) => demo.session_doc(chat_id)?,
            None => SessionDoc::from_doc(loro::LoroDoc::new()),
        };
        let core = SessionCore::new(chat_id, &self.inner, doc);
        let core = {
            let mut sessions = lock(&self.inner.sessions);
            sessions
                .entry(chat_id.to_owned())
                .or_insert_with(|| core.clone())
                .clone()
        };
        if self.inner.is_demo() {
            core.set_hydrated();
        }
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

    /// Harness catalog of an execution device (static fallback when the
    /// device is unreachable). Cached for capability gating.
    pub async fn list_harnesses(&self, device_id: &str) -> Vec<HarnessInfo> {
        let list = match self.inner.demo() {
            Some(demo) => demo.list_harnesses(device_id).await,
            None => catalog::fallback_harnesses(),
        };
        lock(&self.inner.harness_catalogs).insert(device_id.to_owned(), list.clone());
        list
    }

    /// Model catalog for `harness` on `device_id` (normalized live reply,
    /// else the curated static list).
    pub async fn list_models(&self, device_id: &str, harness: &str) -> Vec<ModelInfo> {
        match self.inner.demo() {
            Some(demo) => demo.list_models(harness).await,
            None => {
                let _ = device_id;
                catalog::fallback_models(harness)
            }
        }
    }

    pub async fn list_refs(&self, device_id: &str, repo_path: &str) -> Result<Vec<RepoRef>> {
        match self.inner.demo() {
            Some(demo) => demo.list_refs(repo_path).await,
            None => Err(ClientError::NotImplemented(format!("ListRefs on {device_id}"))),
        }
    }

    /// Browse folders on a device (`None` = its home folder).
    pub async fn list_folders(&self, device_id: &str, path: Option<String>) -> Result<FolderListing> {
        match self.inner.demo() {
            Some(demo) => demo.list_folders(device_id, path).await,
            None => Err(ClientError::NotImplemented(format!("ListFolders on {device_id}"))),
        }
    }

    /// `git checkout <ref>` in `repo_path` on the device.
    pub async fn switch_ref(&self, device_id: &str, repo_path: &str, ref_name: &str) -> Result<()> {
        match self.inner.demo() {
            Some(demo) => demo.switch_ref(repo_path, ref_name).await,
            None => Err(ClientError::NotImplemented(format!("SwitchRef on {device_id}"))),
        }
    }

    /// Create a worktree off `base`; returns its path.
    pub async fn create_worktree(
        &self,
        device_id: &str,
        space_id: &str,
        repo_path: &str,
        base: &str,
    ) -> Result<String> {
        let _ = space_id;
        match self.inner.demo() {
            Some(demo) => demo.create_worktree(repo_path, base).await,
            None => Err(ClientError::NotImplemented(format!("CreateWorktree on {device_id}"))),
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
            return Err(ClientError::InvalidArgument(format!("{name} is larger than 24 MB")));
        }
        match self.inner.demo() {
            Some(demo) => demo.upload(&self.inner, device_id, name, data, progress).await,
            None => Err(ClientError::NotImplemented(format!("UploadChunk to {device_id}"))),
        }
    }

    /// Attachment bytes (LRU-cached). Accepts host paths and this device's
    /// own `pending://` refs.
    pub async fn read_attachment(&self, device_id: &str, path: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.inner.attachment_cache.get(device_id, path) {
            return Ok(hit);
        }
        match self.inner.demo() {
            Some(demo) => {
                let bytes = demo.read_attachment(path)?;
                self.inner.attachment_cache.put(device_id, path, bytes.clone());
                Ok(bytes)
            }
            None => Err(ClientError::NotImplemented(format!("ReadAttachmentChunk from {device_id}"))),
        }
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
        }
    }

    /// App returned to the foreground: resume ticking, redial/probe rooms.
    pub fn on_foreground(&self) {
        self.inner.foreground.store(true, Ordering::Release);
        self.inner.tick();
    }

    /// App is backgrounding: persist docs now; pause time-driven work.
    pub fn on_background(&self) {
        self.inner.foreground.store(false, Ordering::Release);
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
    Utc.timestamp_millis_opt(at).single().unwrap_or_else(Utc::now)
}
