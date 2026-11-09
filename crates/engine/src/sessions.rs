//! SessionsEngine — per-chat agent runs: dispatch, steering, interrupts, input bridging,
//! journal + broadcast fan-out, and 120ms coalesced doc streaming.
//!
//! Pragmatic port of zeron's `sessions.ts` (spec: feature-inventory §3.2):
//! - every `AgentEvent` is (a) appended to the on-disk run journal, (b) broadcast to
//!   in-process subscribers, (c) folded via `fold_event_into_parts` and diffed into the
//!   chat's `SessionDoc` through `SegmentWriter` on a coalesced `STREAM_COMMIT_MS` timer;
//! - the user message entry is pushed to the doc immediately on dispatch (id = the
//!   command's client-minted message id, so optimistic echoes never flicker);
//! - a `Steered` event splits the assistant entry at the exact boundary;
//! - recovery (interrupt or a stale journal at boot) stamps the streaming entry `aborted`.
//!
//! Scope notes: sessions are keyed by chat id (one live run per chat). Zeron's pulse
//! loop is ported as the 15s liveness heartbeat in `drive_run`; its stall watchdog is
//! deliberately NOT ported (rejected in review — agents may legitimately wait on
//! something for far longer than any timeout, and a live child IS the working signal).
//! Every dying path must instead carry its own visible error (child crash with stderr,
//! spawn failure, stream error, engine-restart recovery).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use chrono::Utc;
use futures::StreamExt;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use zeron_doc::{
    DocError, MessagePart, MessageRole, MessageStatus, STREAM_COMMIT_MS, SegmentWriter, SessionDoc,
    SessionMessageEntry, fold_event_into_parts, sanitize_tool_call,
};
use zeron_harness::{CancellationToken, Harness, RunControls, SteerMessage, TurnControl};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, Session, SessionStatus, UserInputAnswer,
    UserInputQuestion,
};

use crate::boat_lease::{BoatLease, BoatRun};
use crate::doc_host::{ChatDocHandle, DocHost};

#[path = "session_activity.rs"]
mod activity;
use crate::registry::{HarnessDescriptor, HarnessRegistry};
use crate::run_journal::RunJournal;
use crate::{EngineError, new_id, now_ms};

/// One journaled event: the durable seq plus the event, as broadcast to subscribers.
#[derive(Debug, Clone)]
pub struct JournaledEvent {
    pub seq: u64,
    pub event: AgentEvent,
}

/// Outcome of a steer attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerOutcome {
    /// Delivered into the live run's steering mailbox.
    Accepted,
    /// A live run owns the turn, but an update prevents accepting another prompt.
    DeferredByUpdate,
    /// No live steerable run — the caller should dispatch the prompt as a new turn.
    NotSteerable,
}

type PendingInputs = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<UserInputAnswer>>>>>;

/// A harness-native session id plus the cwd it was created under. Harness
/// session stores are cwd-scoped (claude keys conversations by project
/// directory — zeron sessions.ts:563 "harness session stores are keyed by
/// cwd"), so resume is only injected for runs launched from the same cwd.
#[derive(Debug, Clone)]
struct HarnessSessionRef {
    session_id: String,
    cwd: String,
    /// The provider that minted this id. Older durable rows may not yet have
    /// an owner tag, but a known owner must match before an id is resumed.
    harness: Option<HarnessId>,
}

/// The configuration a live harness runtime launched with. The steering
/// mailbox carries only the prompt (and its images), so a request whose
/// launch settings differ would silently run with the old process's — it
/// must replace the runtime and resume its harness-native session instead.
/// The driver decides what "differ" means ([`Harness::same_runtime`]): a
/// replacement kills the runtime's background work, so a setting the
/// launch never reads, or the same value spelled differently by another
/// client, must never force one.
#[derive(Clone)]
struct RuntimeConfig {
    harness_id: HarnessId,
    request: RunRequest,
    harness: Arc<dyn Harness>,
}

impl RuntimeConfig {
    fn new(harness_id: HarnessId, request: &RunRequest, harness: Arc<dyn Harness>) -> Self {
        Self {
            harness_id,
            request: request.clone(),
            harness,
        }
    }

    fn can_route(&self, harness_id: HarnessId, request: &RunRequest) -> bool {
        self.harness_id == harness_id && self.harness.same_runtime(&self.request, request)
    }

    /// How `request` reaches this runtime: `Some(false)` as is, `Some(true)`
    /// with its configuration adopted in place first
    /// ([`Harness::reconfigures_in_place`]), `None` only by replacing it.
    fn route(&self, harness_id: HarnessId, request: &RunRequest) -> Option<bool> {
        if self.can_route(harness_id, request) {
            Some(false)
        } else if self.harness_id == harness_id
            && self.harness.reconfigures_in_place(&self.request, request)
        {
            Some(true)
        } else {
            None
        }
    }
}

/// A run's live voice calls. They hold its idle reaper off, and each hang-up
/// wakes the reaper to restart its window from there.
#[derive(Default)]
pub(crate) struct VoiceActivity {
    calls: std::sync::atomic::AtomicUsize,
    ended: tokio::sync::Notify,
}
impl VoiceActivity {
    pub(crate) fn live(&self) -> bool {
        self.calls.load(std::sync::atomic::Ordering::Acquire) > 0
    }
    /// A live call until the returned guard drops.
    pub(crate) fn call(self: &Arc<Self>) -> VoiceCall {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        VoiceCall(self.clone())
    }
}
pub(crate) struct VoiceCall(Arc<VoiceActivity>);
impl Drop for VoiceCall {
    fn drop(&mut self) {
        self.0
            .calls
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        // A stored permit: the reaper hears it even while busy elsewhere.
        self.0.ended.notify_one();
    }
}

struct RunHandle {
    inference: Option<zeron_harness::InferenceRoute>,
    inference_requests: usize,
    inference_changed: Option<Arc<tokio::sync::Notify>>,
    voice: Option<zeron_harness::codex::realtime::RealtimeHandle>,
    voice_active: Arc<VoiceActivity>,
    run_id: String,
    supervision_token: String,
    supervision: watch::Sender<usize>,
    steerable: bool,
    runtime_config: RuntimeConfig,
    steer_tx: mpsc::Sender<SteerMessage>,
    /// Harness-level cancellation (protocol interrupt + child teardown).
    interrupt_token: CancellationToken,
    /// Turn-level stop + the runtime's background-work report. Used for
    /// stopping a turn when the driver [`Harness::stops_turn_in_place`].
    turn: TurnControl,
    stops_turn_in_place: bool,
    /// The harness stream is open. Before that there is no runtime to keep:
    /// a stop tears the pending start down instead.
    started: Arc<std::sync::atomic::AtomicBool>,
    /// Engine-level cancel: arms the run task's grace deadline so a harness that
    /// ignores its token can never strand the run.
    cancel: watch::Sender<bool>,
    engine_tx: mpsc::UnboundedSender<AgentEvent>,
    pending_inputs: PendingInputs,
    /// Steers accepted into the mailbox but not yet confirmed by a `Steered`
    /// event — the at-least-once ledger. A run can die with accepted steers
    /// still in its mailbox (idle reaper vs. a routed send; a mid-turn error
    /// discarding queued boundary steers): the run task drains this at exit
    /// and re-dispatches each entry as a fresh turn, so an accepted message
    /// can never silently evaporate from a transcript that shows it as sent.
    routed_steers: Arc<Mutex<std::collections::VecDeque<RoutedSteer>>>,
    /// This runtime's provider session holds the fork's copied history (a
    /// side chat's bootstrap went out on its run or on one of its steers).
    fork_history_sent: Arc<std::sync::atomic::AtomicBool>,
    /// The user stopped this runtime: it is being torn down and takes no
    /// more prompts. Flipped under the ledger lock, so every accepted steer
    /// is unambiguously before the stop (cancelled with it) or after it
    /// (re-dispatched to a fresh runtime — never lost).
    retiring: Arc<std::sync::atomic::AtomicBool>,
    /// Set while graceful shutdown owns an in-flight turn. A later user Stop
    /// clears it so shutdown recovery never revives explicitly stopped work.
    shutdown_cut: std::sync::atomic::AtomicBool,
    /// Credential identity used when this provider process started.
    credential: Option<String>,
    /// CLI executable identity when this provider process started.
    binary: Option<String>,
    /// Whether this run was launched with provider-native subagents disabled.
    side_chats_only: bool,
}

/// The CLI binary a run launches now: real path, modification time and size.
fn binary_identity(harness: &dyn Harness) -> Option<String> {
    let path = std::fs::canonicalize(harness.executable_path()?).ok()?;
    let metadata = path.metadata().ok()?;
    Some(format!(
        "{}:{:?}:{}",
        path.display(),
        metadata.modified().ok(),
        metadata.len()
    ))
}

/// Connection-owned: a stale subscription releases only its original run.
pub(crate) struct SupervisionLease {
    pub run_id: String,
    pub interrupt: CancellationToken,
    holders: watch::Sender<usize>,
}

impl Drop for SupervisionLease {
    fn drop(&mut self) {
        self.holders.send_modify(|count| *count -= 1);
    }
}

struct RunIdentity {
    chat_id: String,
    run_id: String,
}

/// Holds provider cleanup through startup failures and the driver's actual exit.
struct ProviderRun {
    inner: Arc<Inner>,
    run_id: String,
    _boat: Option<BoatRun>,
}

impl Drop for ProviderRun {
    fn drop(&mut self) {
        self.inner.namespace_tasks.finish_turn(&self.run_id);
    }
}

/// Kept by the relay until its response stream ends or the request is dropped.
pub(crate) struct InferenceActivity {
    inner: std::sync::Weak<Inner>,
    identity: Arc<RunIdentity>,
}

impl Drop for InferenceActivity {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else { return; };
        let mut runs = lock(&inner.runs);
        let Some(handle) = runs.get_mut(&self.identity.chat_id)
            .filter(|h| h.run_id == self.identity.run_id) else { return; };
        handle.inference_requests -= 1;
        if let Some(changed) = &handle.inference_changed { changed.notify_one(); }
        let pending = lock(&handle.routed_steers);
        let idle = lock(&inner.statuses).get(&self.identity.chat_id)
            .is_some_and(|s| matches!(s.status, SessionStatus::Idle | SessionStatus::Errored));
        if handle.inference_requests == 0 && pending.is_empty() && idle {
            inner.finish_turn(&self.identity.run_id);
        }
    }
}

/// One accepted-but-unconfirmed steer: enough to re-dispatch it verbatim.
#[derive(Debug, Clone)]
struct RoutedSteer {
    prompt: String,
    message_id: String,
    /// The delivered text carried the fork's copied history: its delivery
    /// is recorded only when the runtime confirms consuming it (`Steered`).
    /// An orphan re-dispatches the bare prompt, still owing the history.
    fork_history: bool,
    /// Accepted after the user's Stop began tearing the runtime down: the
    /// Stop did not cancel it, so the message still runs.
    after_stop: bool,
    /// The run configuration it was sent with, images included: an orphan
    /// re-dispatches exactly this, not whatever the chat's latest request
    /// became. `None` when the chat had no recorded request.
    request: Option<Box<RunRequest>>,
}

#[derive(Debug, Clone)]
struct Revival {
    message_id: String,
    continuation: Option<String>,
}

struct Inner {
    boat_lease: BoatLease,
    namespace_tasks: crate::namespace_tasks::NamespaceTasks,
    voice: crate::voice::VoiceManager,
    device_id: String,
    /// The engine is shutting down: dying runs re-dispatch nothing.
    shutting_down: std::sync::atomic::AtomicBool,
    revive_after_shutdown: std::sync::atomic::AtomicBool,
    revivals: Mutex<HashMap<String, Revival>>,
    /// Per chat: messages a Stop cancelled while they were still on their
    /// way (see [`SessionsEngine::cancel_sent_messages`]).
    cancelled_sends: Mutex<HashMap<String, std::collections::HashSet<String>>>,
    user_stops: Mutex<HashMap<String, u64>>,
    /// Loopback IPC port this engine serves, once known (0 = not serving):
    /// what the injected `zeron mcp` server dials back into.
    ipc_port: std::sync::atomic::AtomicU16,
    /// Profile-local bearer path for authenticated IPC. Agents receive the
    /// path and read the rotating token when they connect.
    ipc_token_file: Mutex<Option<std::path::PathBuf>>,
    journal: Arc<RunJournal>,
    registry: Arc<HarnessRegistry>,
    /// Set-once (first wins), cleared on runtime retirement: sessions and
    /// doc-host reference each other through Arcs, so this back-edge must be
    /// severable for a replaced engine graph to drop.
    doc_host: Mutex<Option<DocHost>>,
    /// chat_id → live run.
    runs: Mutex<HashMap<String, RunHandle>>,
    /// chat_id → broadcast hub (retained across runs so subscribers survive turns).
    hubs: Mutex<HashMap<String, broadcast::Sender<JournaledEvent>>>,
    statuses: Mutex<HashMap<String, Session>>,
    sessions_tx: watch::Sender<Vec<Session>>,
    /// View-only projection that merges terminal-reported activity into the
    /// native session rows without changing native control state.
    activity_tx: OnceLock<watch::Sender<Vec<Session>>>,
    activity_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Last dispatched request per chat — the steer→new-turn fallback re-derives its
    /// run config from this (chat config rows land with the workspace doc in M4).
    last_requests: Mutex<HashMap<String, RunRequest>>,
    /// Harness-native session ids per chat (resume continuity across turns) —
    /// the live-process cache over the durable copy on the workspace chat row
    /// (zeron kept the same pair on `chats.harness_session_id`). An empty
    /// session id is the "do not resume" tombstone after a rejected resume.
    harness_sessions: Mutex<HashMap<String, HarnessSessionRef>>,
    /// Auto-titler for untitled chats (wired at engine assembly; absent in bare tests).
    titles: OnceLock<crate::titles::TitleGenerator>,
    generated_images: OnceLock<(crate::uploads::Uploads, std::path::PathBuf)>,
    crew: OnceLock<crate::crew::CrewServices>,
    imported_native_histories: Mutex<std::collections::BTreeMap<String, std::path::PathBuf>>,
    imported_recovery: OnceLock<Vec<zeron_sync::crew_import::ImportedRecoveryIntent>>,
    original_epoch_histories: OnceLock<zeron_sync::crew_import::OriginalEpochHistories>,
    original_epoch_archives: OnceLock<Vec<std::path::PathBuf>>,
    original_epoch_binding: Mutex<()>,
    native_branches: OnceLock<std::path::PathBuf>,
    /// [`crate::Repos::scratch_root`]; bare tests fall back to
    /// `~/.zeron/scratch`.
    scratch_root: OnceLock<std::path::PathBuf>,
    /// Fired with `(chat_id, cwd)` when a user prompt starts a turn (fresh
    /// dispatch or accepted steer) — the diff sync snapshots the checkout tree
    /// for the Changes pane's "Latest turn" scope. Absent in bare tests.
    turn_listener: OnceLock<TurnListener>,
}

/// Turn-start hook: called with `(chat_id, cwd)`.
pub type TurnListener = Arc<dyn Fn(&str, &str) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn admit_imported_recovery(
    host: &DocHost,
    intent: &zeron_sync::crew_import::ImportedRecoveryIntent,
    device: &str,
    owner: &str,
    scope: &zeron_proto::CollaborationScope,
) -> Result<bool, EngineError> {
    let denied = || EngineError::Other("Imported recovery authority or exact source identity changed".into());
    if intent.destination_device_id != device || intent.owner_subject != owner
        || intent.project_id != scope.project_id || intent.deployment_id != scope.deployment_id
        || scope.session_id.is_some() || intent.source_device_id.is_empty()
        || intent.source_chat_id.is_empty() || intent.command_id.is_empty()
        || intent.harness != HarnessId::Omp
        || intent.request.harness != Some(intent.harness)
        || intent.request.resume.as_deref() != Some(intent.native_session_id.as_str())
        || intent.request.cwd.is_empty()
        || intent.request.model_options.get("crewAccountId").and_then(serde_json::Value::as_str)
            != intent.agent_account_id.as_deref()
    { return Err(denied()); }
    let workspace = host.workspace().ok_or_else(denied)?;
    let chat = workspace.chat(&intent.chat_id)?.ok_or_else(denied)?;
    if chat.crew.as_ref().is_some_and(|metadata| metadata.original_epoch_history.is_some()) { return Ok(false); }
    if chat.device_id != device { return Err(denied()); }
    match &chat.crew {
        Some(metadata) if !zeron_proto::is_local_history_chat(&chat.id)
            && metadata.owner_subject == owner && metadata.scope.project_id == scope.project_id
            && metadata.scope.deployment_id == scope.deployment_id
            && metadata.session_id == chat.id && metadata.scope.session_id.as_deref() == Some(chat.id.as_str()) => {}
        None if zeron_proto::is_local_history_chat(&chat.id) => {}
        _ => return Err(denied()),
    }
    let handle = host.open(&intent.chat_id)?;
    let payload = zeron_doc::SessionCommandPayload::Run {
        request: intent.request.clone(), message_id: intent.user_message_id.clone(),
    };
    if let Some(existing) = handle.doc().read_commands()?.into_iter().find(|entry| entry.id == intent.command_id) {
        if existing.payload != payload || existing.issued_by != device { return Err(denied()); }
        // The existing command owns delivery and its durable outcome, including unknown.
        return Ok(false);
    }
    if host.crew_store().is_processed(&intent.command_id)? {
        // The claim survived but its document mirror did not. Never re-admit it.
        return Ok(false);
    }
    if chat.harness_session_id.as_deref() != Some(intent.native_session_id.as_str())
        || chat.harness_session_cwd.as_deref() != Some(intent.request.cwd.as_str())
        || chat.cwd.as_deref() != Some(intent.request.cwd.as_str())
        || !chat.config.as_ref().is_some_and(|config|
            config.harness == intent.harness && config.model == intent.request.model)
        || handle.doc().read_entries()?.iter().rev().find(|entry| entry.role == MessageRole::User)
            .is_none_or(|entry| entry.id != intent.user_message_id)
    { return Err(denied()); }
    host.queue_command_named(&intent.chat_id, payload, Vec::new(), Some(&intent.command_id), None)?;
    Ok(true)
}

#[derive(Clone)]
pub struct SessionsEngine {
    inner: Arc<Inner>,
}

impl SessionsEngine {
    pub fn new(
        device_id: String,
        journal: Arc<RunJournal>,
        registry: Arc<HarnessRegistry>,
    ) -> Self {
        let (sessions_tx, _) = watch::channel(Vec::new());
        Self {
            inner: Arc::new(Inner {
                boat_lease: BoatLease::detect(),
                namespace_tasks: crate::namespace_tasks::NamespaceTasks::detect(),
                voice: crate::voice::VoiceManager::default(),
                device_id,
                shutting_down: std::sync::atomic::AtomicBool::new(false),
                revive_after_shutdown: std::sync::atomic::AtomicBool::new(true),
                revivals: Mutex::new(HashMap::new()),
                cancelled_sends: Mutex::new(HashMap::new()),
                user_stops: Mutex::new(HashMap::new()),
                ipc_port: std::sync::atomic::AtomicU16::new(0),
                ipc_token_file: Mutex::new(None),
                journal,
                registry,
                doc_host: Mutex::new(None),
                runs: Mutex::new(HashMap::new()),
                hubs: Mutex::new(HashMap::new()),
                statuses: Mutex::new(HashMap::new()),
                sessions_tx,
                activity_tx: OnceLock::new(),
                activity_task: Mutex::new(None),
                last_requests: Mutex::new(HashMap::new()),
                harness_sessions: Mutex::new(HashMap::new()),
                titles: OnceLock::new(),
                generated_images: OnceLock::new(),
                crew: OnceLock::new(),
                imported_native_histories: Mutex::new(Default::default()),
                imported_recovery: OnceLock::new(),
                original_epoch_histories: OnceLock::new(),
                original_epoch_archives: OnceLock::new(),
                original_epoch_binding: Mutex::new(()),
                native_branches: OnceLock::new(),
                scratch_root: OnceLock::new(),
                turn_listener: OnceLock::new(),
            }),
        }
    }

    pub(crate) fn set_crew(&self, crew: crate::crew::CrewServices) {
        if let Some(titles) = self.inner.titles.get() {
            titles.set_crew(crew.clone());
        }
        let _ = self.inner.crew.set(crew);
    }

    pub(crate) fn load_crew_import(&self, profile: &std::path::Path) -> Result<(), EngineError> {
        let original_epochs = zeron_sync::crew_import::imported_original_epoch_histories(profile)
            .map_err(|error| EngineError::Other(error.to_string()))?;
        let _ = self.inner.original_epoch_histories.set(original_epochs);
        let branches = profile.join("native-branches");
        std::fs::create_dir_all(&branches)?;
        let _ = self.inner.native_branches.set(branches.canonicalize()?);
        let mut histories = zeron_sync::crew_import::imported_native_histories(profile)
            .map_err(|error| EngineError::Other(error.to_string()))?;
        let mut original_archives = Vec::new();
        for (native, archive) in &histories {
            if self.inner.original_epoch_histories.get().is_some_and(|histories|
                histories.histories.values().any(|history| history.native_session_id.as_deref() == Some(native.as_str()))) {
                original_archives.push(archive.canonicalize()?);
            }
        }
        let _ = self.inner.original_epoch_archives.set(original_archives);
        if !histories.is_empty() {
            let launches = profile.join("native-resumes");
            std::fs::create_dir_all(&launches)?;
            let launches = launches.canonicalize()?;
            for (id, archive) in &mut histories {
                if std::path::Path::new(id).file_name() != Some(std::ffi::OsStr::new(id)) {
                    return Err(EngineError::Other("Invalid imported native session ID".into()));
                }
                let target = launches.join(format!("{id}.jsonl"));
                crate::omp_session_artifact::prepare_imported_native_session(archive, &target, id)?;
                *archive = target;
            }
        }
        *lock(&self.inner.imported_native_histories) = histories;
        let recovery = zeron_sync::crew_import::imported_recovery_intents(profile)
            .map_err(|error| EngineError::Other(error.to_string()))?;
        let _ = self.inner.imported_recovery.set(recovery);
        Ok(())
    }
    pub(crate) fn original_epoch_history(&self, chat_id: &str) -> Option<&zeron_proto::OriginalEpochHistory> {
        self.inner.original_epoch_histories.get()?.histories.get(chat_id)
    }

    /// Historical processed claims are provenance, not an interrupted local execution.
    pub(crate) fn is_original_epoch_command(&self, command_id: &str) -> bool {
        self.inner.original_epoch_histories.get()
            .is_some_and(|history| history.command_ids.contains(command_id))
    }

    pub(crate) fn reset_original_epoch_native_association(&self, chat_id: &str) {
        lock(&self.inner.harness_sessions).remove(chat_id);
        lock(&self.inner.last_requests).remove(chat_id);
    }

    /// Only the importer's immutable proof and an explicit, verified Start receipt
    /// can release a logical session. Editable index metadata cannot release it.
    pub(crate) fn ensure_execution_allowed(&self, chat_id: &str, resume: Option<&str>) -> Result<(), EngineError> {
        let denied = || EngineError::Other("Original execution epoch is read-only history; explicitly start a verified new epoch before running. Its original native identity cannot be resumed or forked.".into());
        if let Some(selector) = resume
            && let Some(histories) = self.inner.original_epoch_histories.get().filter(|loaded| !loaded.histories.is_empty())
        {
            let selected = std::path::Path::new(selector);
            let resolved = selected.is_absolute().then(|| selected.canonicalize().ok()).flatten();
            if self.inner.original_epoch_archives.get().is_some_and(|archives|
                archives.iter().any(|archive| archive == selected || resolved.as_ref() == Some(archive))) {
                return Err(denied());
            }
            // ponytail: scan the bounded import map; index selectors if profiles grow large.
            for native in histories.histories.values().filter_map(|history| history.native_session_id.as_deref()) {
                if selector == native { return Err(denied()); }
                if selected.is_absolute() {
                    let launch = self.native_launch_selector(native);
                    let launch = std::path::Path::new(&launch);
                    if selected == launch || resolved.as_deref() == Some(launch) { return Err(denied()); }
                }
            }
        }
        let host = self.inner.doc_host();
        let chat = host.as_ref().and_then(|host| host.workspace()).map(|workspace| workspace.chat(chat_id)).transpose()?.flatten();
        let Some(history) = self.original_epoch_history(chat_id) else {
            if chat.as_ref().and_then(|chat| chat.crew.as_ref()).is_some_and(|metadata| metadata.original_epoch_history.is_some()) {
                return Err(denied());
            }
            return Ok(());
        };
        let host = host.ok_or_else(denied)?;
        let _binding = lock(&self.inner.original_epoch_binding);
        let workspace = host.workspace().ok_or_else(denied)?;
        let mut chat = workspace.chat(chat_id)?.ok_or_else(denied)?;
        if let Some(crew) = self.inner.crew.get()
            && let Some(authority) = crew.device_grant_authority()
        {
            if chat.crew.is_none() { chat = crew.adopt_chat(workspace, chat_id)?; }
            let metadata = chat.crew.as_ref().ok_or_else(denied)?;
            let marker_missing = metadata.original_epoch_history.is_none();
            if authority.grant_id.is_empty() || authority.expires_at <= now_ms()
                || authority.device_id != self.inner.device_id
                || authority.device_id != crate::scaffold::device_id_for(&authority.sandbox_id, authority.lifecycle_epoch)
                || authority.sandbox_id != history.sandbox_id || authority.lifecycle_epoch <= history.coordinator_lifecycle_epoch
                || authority.principal_subject != metadata.owner_subject || authority.scope != metadata.scope
                || authority.scope.session_id.as_deref() != Some(chat_id)
                || metadata.original_epoch_history.as_ref().is_some_and(|marker| marker != history)
            { return Err(denied()); }
            if marker_missing { chat.crew.as_mut().ok_or_else(denied)?.original_epoch_history = Some(history.clone()); }
            // The server mints this exact new-epoch grant only after its explicit
            // Start intent. Bootstrap must first restore the verified import proof.
            let key = format!("original-epoch-start:{chat_id}:{}:{}", history.lifecycle_epoch, authority.lifecycle_epoch);
            let store = host.crew_store();
            let request = serde_json::to_vec(history).map_err(|_| denied())?;
            let outcome = serde_json::to_vec(&serde_json::json!({
                "sandboxId":authority.sandbox_id, "lifecycleEpoch":authority.lifecycle_epoch, "deviceId":authority.device_id,
            })).map_err(|_| denied())?;
            store.claim_crew_command(&key, &request)?;
            let (saved_request, saved) = store.crew_command(&key)?.ok_or_else(denied)?;
            if saved_request != request { return Err(denied()); }
            if let Some(saved) = saved {
                if saved != outcome { return Err(denied()); }
                if marker_missing { workspace.publish_crew_chat(&chat, None)?; }
            } else {
                let metadata = chat.crew.as_mut().ok_or_else(denied)?;
                if let Some(environment) = metadata.environment.as_mut() {
                    let zeron_proto::SessionEnvironmentSource::Scaffold { sandbox_id, lifecycle_epoch, .. } = &mut environment.source else {
                        return Err(denied());
                    };
                    if sandbox_id != &authority.sandbox_id { return Err(denied()); }
                    *lifecycle_epoch = Some(authority.lifecycle_epoch);
                }
                chat.device_id = authority.device_id.clone();
                chat.harness_session_id = Some(String::new());
                chat.harness_session_cwd = None;
                workspace.publish_crew_chat(&chat, None)?;
                self.reset_original_epoch_native_association(chat_id);
                store.finish_crew_command(&key, &outcome)?;
            }
            if chat.device_id != authority.device_id
                || chat.harness_session_id.as_deref().filter(|id| !id.is_empty())
                    .is_some_and(|id| history.native_session_id.as_deref() == Some(id))
            { return Err(denied()); }
            return Ok(());
        }
        let metadata = chat.crew.as_ref().ok_or_else(denied)?;
        let environment = metadata.environment.as_ref().ok_or_else(denied)?;
        let zeron_proto::SessionEnvironmentSource::Scaffold { lifecycle_epoch: Some(epoch), .. } = &environment.source else {
            return Err(denied());
        };
        let (request, outcome) = host.crew_store().crew_command(&format!("original-epoch-start:{chat_id}:{}:{epoch}", history.lifecycle_epoch))?
            .ok_or_else(denied)?;
        if serde_json::from_slice::<zeron_proto::OriginalEpochHistory>(&request).ok().as_ref() != Some(history) {
            return Err(denied());
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Binding { sandbox_id: String, lifecycle_epoch: u64, device_id: String }
        let binding: Binding = serde_json::from_slice(outcome.as_deref().ok_or_else(denied)?).map_err(|_| denied())?;
        if metadata.original_epoch_history.as_ref() != Some(history)
            || binding.lifecycle_epoch <= history.coordinator_lifecycle_epoch
            || binding.sandbox_id != history.sandbox_id
            || binding.device_id != crate::scaffold::device_id_for(&binding.sandbox_id, binding.lifecycle_epoch)
            || chat.device_id != binding.device_id
            || !matches!(&environment.source, zeron_proto::SessionEnvironmentSource::Scaffold { sandbox_id, lifecycle_epoch: Some(epoch), .. }
                if sandbox_id == &binding.sandbox_id && *epoch == binding.lifecycle_epoch)
            || chat.harness_session_id.as_deref().filter(|id| !id.is_empty()).is_some_and(|id| history.native_session_id.as_deref() == Some(id))
        { return Err(denied()); }
        Ok(())
    }


    /// Rewrite only the host launch selector; the synced native ID stays portable.
    pub(crate) fn native_launch_selector(&self, id: &str) -> String {
        if let Some(path) = lock(&self.inner.imported_native_histories).get(id) {
            return path.to_string_lossy().into_owned();
        }
        if let Ok(path) = self.native_branch_path(id) {
            if path.is_file() { return path.to_string_lossy().into_owned(); }
        }
        id.to_owned()
    }

    pub(crate) fn native_branch_path(&self, id: &str) -> Result<std::path::PathBuf, EngineError> {
        uuid::Uuid::parse_str(id).map_err(|_| EngineError::Other("Invalid native branch ID".into()))?;
        Ok(self.inner.native_branches.get()
            .ok_or_else(|| EngineError::Other("Native branch store unavailable".into()))?
            .join(format!("{id}.jsonl")))
    }

    fn native_fork_source(&self, chat_id: &str, harness: HarnessId, resume: Option<&str>) -> Result<Option<String>, EngineError> {
        let Some(host) = self.inner.doc_host() else { return Ok(None) };
        let Some((bytes, _)) = host.crew_store().crew_command(&format!("native-fork-source:{chat_id}"))?
            else { return Ok(None) };
        #[derive(serde::Deserialize)]
        struct Source { source: String, harness: HarnessId }
        let source: Source = serde_json::from_slice(&bytes).map_err(|error| EngineError::Other(error.to_string()))?;
        if resume != Some(source.source.as_str()) { return Ok(None); }
        if harness != source.harness || !matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex) {
            return Err(EngineError::Other("Native fork must launch with its original harness".into()));
        }
        Ok(Some(source.source))
    }

    fn managed_services(&self) -> Result<Option<&crate::crew::CrewServices>, EngineError> {
        if !self.inner.registry.managed_inference() {
            return Ok(None);
        }
        let crew = self.inner.crew.get().ok_or_else(|| EngineError::Other(
            "Crew-managed execution requires a verified signed-in authority".into(),
        ))?;
        crew.ensure_identity()?;
        Ok(Some(crew))
    }

    pub(crate) fn retain_supervision(&self, chat_id: &str, token: &str) -> Result<SupervisionLease, EngineError> {
        let runs = lock(&self.inner.runs);
        if self.inner.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
            return Err(EngineError::Other("engine is shutting down".into()));
        }
        let handle = runs.get(chat_id).filter(|handle| {
            handle.supervision_token == token && handle.steerable
                && !handle.interrupt_token.is_cancelled()
                && !handle.retiring.load(std::sync::atomic::Ordering::Acquire)
        }).ok_or_else(|| EngineError::Other("no authorized live supervision run".into()))?;
        if let Some(crew) = self.managed_services()? {
            // Reuse dispatch's ownership gate, including host-local imported histories.
            let workspace = self.inner.workspace()
                .ok_or_else(|| EngineError::Other("Crew workspace unavailable".into()))?;
            crew.adopt_chat(&workspace, chat_id)?;
        }
        handle.supervision.send_modify(|count| *count += 1);
        Ok(SupervisionLease {
            run_id: handle.run_id.clone(),
            interrupt: handle.interrupt_token.clone(),
            holders: handle.supervision.clone(),
        })
    }

    pub(crate) fn voice_manager(&self) -> crate::voice::VoiceManager {
        self.inner.voice.clone()
    }

    /// Record the loopback IPC port this engine serves. Runs started after
    /// this carry Zeron's MCP server (see [`Inner::zeron_mcp`]); until then —
    /// or with 0 — agents get no Zeron tools rather than a dead server.
    pub fn set_ipc_port(&self, port: u16) {
        self.inner
            .ipc_port
            .store(port, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_ipc_token_file(&self, path: std::path::PathBuf) {
        *lock(&self.inner.ipc_token_file) = Some(path);
    }

    /// Wire the doc host (called once at engine assembly; the two services are mutually
    /// referential by design — sessions stream into docs, docs execute commands here).
    pub fn set_doc_host(&self, host: DocHost) {
        // First set wins (the OnceLock contract this slot replaced).
        let mut slot = lock(&self.inner.doc_host);
        if slot.is_none() {
            *slot = Some(host);
        }
    }

    /// Sever the doc-host back-edge (runtime retirement; the doc host's
    /// `shutdown_workers` clears its own sessions edge). Every access site
    /// already treats a missing doc host as "not wired".
    pub fn clear_doc_host(&self) {
        lock(&self.inner.doc_host).take();
    }

    /// Bind generated-image intake to the same profile store used by attachment RPCs.
    /// Where projectless chats' scratch folders resolve (set once at boot).
    pub(crate) fn set_scratch_root(&self, root: std::path::PathBuf) {
        let _ = self.inner.scratch_root.set(root);
    }

    pub fn set_generated_images(
        &self,
        uploads: crate::uploads::Uploads,
        allowed_root: std::path::PathBuf,
    ) {
        let _ = self.inner.generated_images.set((uploads, allowed_root));
    }

    /// Wire the chat auto-titler (called once at engine assembly). After each
    /// completed exchange the run task fires it for still-untitled chats.
    pub fn set_titles(&self, titles: crate::titles::TitleGenerator) {
        if let Some(crew) = self.inner.crew.get() {
            titles.set_crew(crew.clone());
        }
        let _ = self.inner.titles.set(titles);
    }

    /// Wire the turn-start listener (called once at engine assembly).
    pub fn set_turn_listener(&self, listener: TurnListener) {
        let _ = self.inner.turn_listener.set(listener);
    }

    fn note_turn_start(&self, chat_id: &str, cwd: &str) {
        if let Some(listener) = self.inner.turn_listener.get() {
            listener(chat_id, cwd);
        }
    }

    fn doc_handle(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        let host = self
            .inner
            .doc_host()
            .ok_or_else(|| EngineError::Other("doc host not wired into sessions engine".into()))?;
        host.open(chat_id)
    }

    /// Status watch: the full session list, re-sent on every transition.
    pub fn watch_sessions(&self) -> watch::Receiver<Vec<Session>> {
        self.inner
            .activity_tx
            .get()
            .unwrap_or(&self.inner.sessions_tx)
            .subscribe()
    }

    pub fn session_status(&self, chat_id: &str) -> Option<Session> {
        lock(&self.inner.statuses).get(chat_id).cloned()
    }

    /// Whether this harness takes a prompt *during* a turn, rather than only at
    /// the boundary between turns. The queue drain's central question: a
    /// mid-turn agent gets the message now, everything else holds it until the
    /// turn ends. Catalog lookup only — never spawns.
    pub fn steers_mid_turn(&self, harness: HarnessId) -> bool {
        self.inner
            .registry
            .descriptors()
            .iter()
            .find(|d| d.id == harness)
            .is_some_and(HarnessDescriptor::steers_mid_turn)
    }

    /// Whether this chat has a turn in flight — streaming, or parked on a
    /// question it is still owed an answer to (see [`is_active`]). A persistent
    /// session parked BETWEEN turns is not: it holds a warm child with nothing
    /// outstanding, and takes the next prompt straight through its mailbox.
    pub fn turn_in_flight(&self, chat_id: &str) -> bool {
        lock(&self.inner.statuses)
            .get(chat_id)
            .is_some_and(is_active)
    }

    /// Whether the chat's live runtime reports background work (subagents,
    /// background shells) still running — parked or not.
    pub fn holds_background_work(&self, chat_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(chat_id)
            .is_some_and(|h| h.turn.background_live())
    }

    /// Any run currently working or blocked on input — the auto-updater's
    /// "don't restart from under a session" gate.
    pub fn any_active(&self) -> bool {
        lock(&self.inner.statuses).values().any(is_active)
            || lock(&self.inner.runs)
                .values()
                .any(|r| r.voice_active.live() || r.inference_requests > 0)
    }

    /// A text prompt for `chat_id` would land in the mailbox of a live
    /// turn-boundary agent mid-turn. The agent reads it only after the turn,
    /// but a mailbox delivery writes the user message now — above the reply
    /// still streaming for the message before it. Such prompts belong in the
    /// visible queue. `request` = a Run that may differ from the live config
    /// (one the runtime adopts in place still lands in its mailbox; any other
    /// config restarts the runtime instead, which is no hold).
    pub fn defers_to_turn_end(
        &self,
        chat_id: &str,
        request: Option<(HarnessId, &RunRequest)>,
    ) -> bool {
        if !self.turn_in_flight(chat_id) {
            return false;
        }
        let live = lock(&self.inner.runs).get(chat_id).map(|h| {
            (
                h.runtime_config.harness_id,
                h.steerable
                    && request.is_none_or(|(id, r)| h.runtime_config.route(id, r).is_some()),
            )
        });
        live.is_some_and(|(harness, routable)| routable && !self.steers_mid_turn(harness))
    }

    /// The harness of the chat's live run, while one is in flight.
    pub fn live_harness(&self, chat_id: &str) -> Option<HarnessId> {
        if !self.turn_in_flight(chat_id) {
            return None;
        }
        lock(&self.inner.runs)
            .get(chat_id)
            .map(|h| h.runtime_config.harness_id)
    }

    pub fn note_provider_switch(&self, chat_id: &str, harness_id: HarnessId, doc: &ChatDocHandle) {
        self.inner.note_provider_switch(chat_id, harness_id, doc);
    }

    /// Whether an accepted agent update gates the chat's live run, so a new
    /// prompt waits in the queue rather than joining that run's transcript.
    pub fn live_run_update_pending(&self, chat_id: &str) -> bool {
        let harness = lock(&self.inner.runs)
            .get(chat_id)
            .map(|h| h.runtime_config.harness_id);
        harness.is_some_and(|harness| self.inner.registry.update_pending(harness))
    }

    /// The chat's live run accepts steering into its mailbox.
    pub fn live_run_steerable(&self, chat_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(chat_id)
            .is_some_and(|h| h.steerable && !h.retiring.load(std::sync::atomic::Ordering::Acquire))
    }

    /// The last request dispatched for a chat (steer→new-turn fallback).
    pub fn last_request(&self, chat_id: &str) -> Option<RunRequest> {
        lock(&self.inner.last_requests).get(chat_id).cloned()
    }

    /// Subscribe to a chat's live event stream: returns the journal replay after
    /// `after_seq` plus a live receiver. Subscribe-then-replay ordering means overlap
    /// (dedupe by seq) rather than gaps.
    pub fn subscribe(
        &self,
        chat_id: &str,
        after_seq: u64,
    ) -> Result<(Vec<JournaledEvent>, broadcast::Receiver<JournaledEvent>), EngineError> {
        let rx = {
            let mut hubs = lock(&self.inner.hubs);
            hubs.entry(chat_id.to_string())
                .or_insert_with(|| broadcast::channel(1024).0)
                .subscribe()
        };
        let replay = self
            .inner
            .journal
            .replay(chat_id, after_seq)?
            .into_iter()
            .map(|(seq, event)| JournaledEvent { seq, event })
            .collect();
        Ok((replay, rx))
    }

    /// Local experimental bootstrap. No Submit, empty user entry or title request.
    pub async fn start_idle(
        &self,
        chat_id: &str,
        request: RunRequest,
    ) -> Result<String, EngineError> {
        self.ensure_execution_allowed(chat_id, request.resume.as_deref())?;
        // A fresh explicit request may renew an identity-invalidated idle
        // process. Never interrupt a native delegated turn to recover voice.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let stale = lock(&self.inner.runs).get(chat_id).map(|run| {
                (
                    run.voice.as_ref().is_some_and(|v| v.invalidated()),
                    run.voice_active.live(),
                )
            });
            match stale {
                Some((true, false)) => {
                    if self.turn_in_flight(chat_id) {
                        return Err(EngineError::Other(
                            "identity changed during delegated work".into(),
                        ));
                    }
                    self.interrupt(chat_id).await?;
                    break;
                }
                Some((true, _)) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(EngineError::Other("voice cleanup in progress".into()));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                _ => break,
            }
        }
        self.dispatch_inner(chat_id, HarnessId::Codex, request, None, false, None, true)
            .await
    }

    pub(crate) fn take_voice_bridge(
        &self,
        chat_id: &str,
    ) -> Option<(
        zeron_harness::codex::realtime::RealtimeHandle,
        tokio::sync::broadcast::Receiver<zeron_proto::voice::VoiceEvent>,
        Arc<VoiceActivity>,
    )> {
        let mut runs = lock(&self.inner.runs);
        let run = runs.get_mut(chat_id)?;
        Some((
            run.voice.clone()?,
            run.voice.as_ref()?.events.subscribe(),
            run.voice_active.clone(),
        ))
    }

    /// Start (or route) a run for `chat_id`.
    ///
    /// - The user message entry is written to the doc immediately (id = `message_id`).
    /// - A live steerable run receives the prompt as its next turn via the mailbox
    ///   (zeron's persistent-session routing); otherwise any live run is interrupted
    ///   first — never two runtimes driving one chat.
    pub async fn dispatch(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        self.ensure_execution_allowed(chat_id, request.resume.as_deref())?;
        let result = self.dispatch_with(chat_id, harness_id, request, message_id, false, None)
            .await;
        // Admission can fail before drive_run exists (for example Agent Auth).
        // The saved user entry is not proof of execution: retain the reason in
        // the same durable error-part format as a harness startup failure.
        if let Err(error) = &result
            && !lock(&self.inner.runs).contains_key(chat_id)
        {
            let written = self.doc_handle(chat_id).and_then(|handle| {
                finish_segment(
                    &handle.writer(), None, &new_id(), &self.inner.device_id, now_ms(),
                    &[MessagePart::Error { id: "e0".into(), message: error.to_string() }],
                    MessageStatus::Complete,
                )
                .map_err(EngineError::from)
            });
            if let Err(error) = written {
                tracing::warn!(chat = %chat_id, %error, "admission failure entry failed");
            }
            self.set_status(chat_id, SessionStatus::Errored, false);
        }
        result
    }

    /// [`Self::dispatch`] with the startup-crash retry marker: the retry
    /// re-dispatches with `startup_retry = true`, which makes that attempt
    /// final (its own startup death surfaces instead of retrying again).
    /// Boxed future: `drive_run` re-enters this for that retry, and the
    /// erasure breaks the opaque-type cycle the recursion would otherwise form.
    fn dispatch_with<'a>(
        &'a self,
        chat_id: &'a str,
        harness_id: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
        startup_retry: bool,
        stops_seen: Option<u64>,
    ) -> futures::future::BoxFuture<'a, Result<String, EngineError>> {
        Box::pin(self.dispatch_inner(
            chat_id,
            harness_id,
            request,
            message_id,
            startup_retry,
            stops_seen,
            false,
        ))
    }

    async fn dispatch_inner(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        mut request: RunRequest,
        mut message_id: Option<String>,
        startup_retry: bool,
        stops_seen: Option<u64>,
        idle: bool,
    ) -> Result<String, EngineError> {
        self.ensure_execution_allowed(chat_id, request.resume.as_deref())?;
        if self.inner.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
            return Err(EngineError::Other("Crew is shutting down".into()));
        }
        let managed = self.managed_services()?.is_some();
        if let Some(crew) = self.inner.crew.get() {
            let workspace = self.inner.workspace().ok_or_else(|| EngineError::Other("Crew workspace unavailable".into()))?;
            crew.adopt_chat(&workspace, chat_id)?;
        }
        // Project-less chats store a home-relative cwd (the creating device
        // can't know the host's home); expand it here, on the host, where the
        // run spawns, creating a new chat's scratch folder.
        let scratch_root = self
            .inner
            .scratch_root
            .get()
            .cloned()
            .unwrap_or_else(|| crate::repos::home_dir().join(".zeron").join("scratch"));
        request.cwd = crate::repos::resolve_chat_cwd(&request.cwd, &scratch_root)
            .map_err(EngineError::Other)?;
        // Native-only catalog entries have no portable file fallback. Reject
        // cross-harness delivery before recording or routing the user turn.
        zeron_proto::invocation::validate_harness_invocations(&request.prompt, harness_id)
            .map_err(EngineError::Other)?;
        if !idle {
            self.inner.boat_lease.protect().await.map_err(EngineError::Other)?;
        }
        if let Some(id) = &message_id
            && self.inner.reviving(chat_id, id)
            && lock(&self.inner.runs).contains_key(chat_id)
        {
            self.inner.drop_revival(chat_id, id);
            tracing::info!(chat = %chat_id, "auto-resume skipped: the chat moved on");
            return Ok(String::new());
        }
        let routed = lock(&self.inner.runs).get(chat_id).map(|h| {
            (
                h.run_id.clone(),
                h.steerable && !h.retiring.load(std::sync::atomic::Ordering::Acquire),
                if managed && (h.runtime_config.request.model != request.model
                    || h.runtime_config.request.model_options.get("crewAccountId") != request.model_options.get("crewAccountId"))
                {
                    None
                } else {
                    h.runtime_config.route(harness_id, &request)
                },
                h.steer_tx.clone(),
                h.routed_steers.clone(),
                h.fork_history_sent.clone(),
                h.retiring.clone(),
                h.credential.clone(),
                h.binary.clone(),
                h.runtime_config.harness.clone(),
                h.side_chats_only,
            )
        });
        if let Some((
            run_id,
            steerable,
            route,
            steer_tx,
            ledger,
            history_sent,
            retiring,
            credential,
            binary,
            live_harness,
            side_chats_only,
        )) = routed
        {
            if idle {
                // No prompt rides an idle start, so nothing could carry an
                // in-place reconfiguration: only the runtime as is will do.
                return if route == Some(false) && harness_id == HarnessId::Codex {
                    Ok(run_id)
                } else {
                    Err(EngineError::Other("incompatible live runtime".into()))
                };
            }
            let route = if self
                .credential_changed(chat_id, harness_id, credential.as_deref())
                .await
                || self
                    .binary_changed(chat_id, harness_id, live_harness, binary.as_deref())
                    .await
                || self.native_subagents_changed(chat_id, harness_id, side_chats_only)
            {
                None
            } else {
                route
            };
            let user_id = message_id.clone().unwrap_or_else(new_id);
            let mut bootstrap = None;
            let same_runtime = route.is_some();
            let reconfigure = route == Some(true);
            let accepted = if steerable && same_runtime {
                bootstrap =
                    self.warm_fork_history(chat_id, harness_id, &request.prompt, &history_sent);
                let delivered = bootstrap.as_deref().unwrap_or(&request.prompt);
                // Warm dispatch uses the same mailbox as explicit steering.
                // Register acceptance before a fast boundary can retire it.
                let message = SteerMessage {
                    // OpenCode must see the canonical selection before it
                    // decodes the provider command: a project-scoped command
                    // can disappear between composer discovery and delivery.
                    prompt: if harness_id == HarnessId::Opencode {
                        delivered.to_owned()
                    } else {
                        zeron_proto::invocation::harness_prompt(delivered, harness_id)
                    },
                    message_id: Some(user_id.clone()),
                    // Images ride the mailbox too: replacing the runtime just
                    // to inline them killed its background work.
                    attachments: request.attachments.clone(),
                    // A changed model/effort/option the live process adopts
                    // in place: replacing it killed its background work.
                    config: reconfigure.then(|| Box::new(request.clone())),
                };
                if let Ok(permit) = steer_tx.reserve().await {
                    // Commit the reserved slot atomically with the update
                    // marker. An accepted update releases the slot instead, and
                    // the prompt takes the fresh-run path behind the update.
                    self.inner
                        .registry
                        .while_update_clear(harness_id, || -> Result<bool, EngineError> {
                            let runs = lock(&self.inner.runs);
                            let Some(handle) = runs.get(chat_id).filter(|h| h.run_id == run_id
                                && !h.retiring.load(std::sync::atomic::Ordering::Acquire)) else {
                                return Ok(false);
                            };
                            let mut pending = lock(&ledger);
                            self.inner.start_turn(chat_id, handle)?;
                            pending.push_back(RoutedSteer {
                                prompt: request.prompt.clone(),
                                message_id: user_id.clone(),
                                fork_history: bootstrap.is_some(),
                                after_stop: retiring.load(std::sync::atomic::Ordering::Acquire),
                                request: Some(Box::new(request.clone())),
                            });
                            permit.send(message);
                            Ok(true)
                        })
                        .transpose()?.unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };
            if accepted {
                if reconfigure {
                    // The runtime runs this configuration from now on: later
                    // sends compare against it, and a re-dispatch of an
                    // orphaned steer starts from it.
                    if let Some(h) = lock(&self.inner.runs)
                        .get_mut(chat_id)
                        .filter(|h| h.run_id == run_id)
                    {
                        h.runtime_config.request = request.clone();
                    }
                    lock(&self.inner.last_requests).insert(chat_id.to_string(), request.clone());
                }
                let handle = self.doc_handle(chat_id)?;
                handle.write_user_message(&user_id, &request.prompt, now_ms())?;
                self.note_turn_start(chat_id, &request.cwd);
                if self.is_live(chat_id, &run_id) {
                    // A parked runtime's journal still ends in its last Done.
                    self.inner.journal.note_pending_start(chat_id);
                    if bootstrap.is_some() {
                        history_sent.store(true, std::sync::atomic::Ordering::Release);
                    }
                    // Working BEFORE the lastMessageAt bump: both ride the
                    // workspace doc from this one peer, so causal order makes it
                    // impossible for an observer to hold [new message, old status]
                    // — that gap read as unseen-with-no-live-run = a phantom
                    // "completed" flash on every remote send (2026-07-31).
                    self.inner.set_run_status(chat_id, &run_id, SessionStatus::Working, false);
                    self.inner.note_message(chat_id, &request.prompt);
                    return Ok(run_id);
                }
                // The run died around the send. If its exit drain already
                // claimed the entry, that re-dispatch owns the message —
                // otherwise reclaim it and fall through to a fresh run.
                let reclaimed = {
                    let mut ledger = lock(&ledger);
                    let before = ledger.len();
                    ledger.retain(|s| s.message_id != user_id);
                    ledger.len() != before
                };
                if !reclaimed {
                    self.inner.note_message(chat_id, &request.prompt);
                    return Ok(run_id);
                }
                // Keep the already-written doc entry's id for the fresh run
                // below (write_user_message dedupes by id).
                message_id = Some(user_id);
            }
            if !same_runtime {
                tracing::info!(
                    chat = %chat_id,
                    "restarting live harness to apply changed run configuration"
                );
            }
            // Mailbox closed (runtime mid-teardown / non-steering harness) or
            // the routed run died with the message reclaimed, or the launch
            // configuration changed: replace the runtime.
            self.terminate(chat_id).await?;
        }

        // A Stop sent after this message cancelled it while it was on its
        // way: never start the stopped work again in a fresh runtime.
        if let Some(id) = &message_id
            && lock(&self.inner.cancelled_sends)
                .get_mut(chat_id)
                .is_some_and(|ids| ids.remove(id))
        {
            tracing::info!(chat = %chat_id, "message cancelled by a later Stop: not dispatched");
            return Ok(String::new());
        }
        let harness = self.inner.registry.resolve(harness_id)?;
        let handle = self.doc_handle(chat_id)?;
        let user_id = message_id.unwrap_or_else(new_id);
        let revival = {
            let mut revivals = lock(&self.inner.revivals);
            match revivals.get(chat_id) {
                Some(revival) if revival.message_id == user_id => revivals.remove(chat_id),
                _ => None,
            }
        };
        if !idle {
            self.inner
                .note_provider_switch(chat_id, harness_id, &handle);
            handle.write_user_message(&user_id, &request.prompt, now_ms())?;
        }

        // Engine-owned resume (zeron sessions.ts:736 — every dispatch read the
        // chat's stored harness session): callers always send `resume: None`;
        // the engine threads the chat's prior harness session back in so a new
        // process (app restart) continues the same harness conversation. The
        // startup-crash retry injects too — a stale id is the harness's
        // problem now (`session/load` falls back to `session/new` internally),
        // and starting the retry fresh silently dropped a good conversation.
        let mut resume_injected = false;
        if request.resume.is_none() {
            request.resume = self
                .inner
                .resume_for(chat_id, &request.cwd, harness_id)
                .or_else(|| {
                    Inner::own_provider_session(handle.doc(), harness_id, &request.cwd)
                });
            resume_injected = request.resume.is_some();
        }
        let revival = revival.map(|mut revival| {
            if request.resume.is_none() {
                revival.continuation = None;
            }
            revival
        });
        self.ensure_execution_allowed(chat_id, request.resume.as_deref())?;
        lock(&self.inner.last_requests).insert(chat_id.to_string(), request.clone());
        let fork_from = self.native_fork_source(chat_id, harness_id, request.resume.as_deref())?;
        if let Some(source) = &fork_from { self.ensure_execution_allowed(chat_id, Some(source))?; }
        if harness_id == HarnessId::Omp {
            request.resume = request.resume.as_deref().map(|id| self.native_launch_selector(id));
        }
        let run_id = new_id();
        let (steer_tx, steer_rx) = mpsc::channel::<SteerMessage>(32);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (engine_tx, engine_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let interrupt_token = CancellationToken::new();
        let inference_changed = self.inner.protection_required().then(|| Arc::new(tokio::sync::Notify::new()));
        let provider_run = ProviderRun {
            inner: self.inner.clone(),
            run_id: run_id.clone(),
            _boat: self.inner.boat_lease.run_guard(&run_id, &interrupt_token),
        };
        if !idle {
            self.inner.boat_lease.start_turn(&run_id, &interrupt_token, &cancel_tx, &engine_tx)
                .map_err(EngineError::Other)?;
            self.inner.namespace_tasks.start_turn(chat_id, &run_id)
                .map_err(|error| EngineError::Other(error.to_string()))?;
        }
        let inference = if let Some(crew) = self.managed_services()? {
            Some(tokio::select! {
                biased;
                _ = interrupt_token.cancelled() => return Err(EngineError::Other(crate::boat_lease::FAILURE.into())),
                route = crew.prepare_inference(
                    chat_id,
                    harness_id,
                    request.model.as_deref(),
                    request.model_options.get("crewAccountId").and_then(serde_json::Value::as_str),
                    self.inner.inference_admission(chat_id, &run_id, &interrupt_token),
                ) => route?,
            })
        } else {
            None
        };

        let supervision_token = new_id();
        let (supervision, supervision_rx) = watch::channel(0);
        let pending_inputs: PendingInputs = Arc::new(Mutex::new(HashMap::new()));

        // Input bridge: the harness asks questions; we mint the request id, park the
        // resolver for `respond_input`, and surface the event through the run pipeline.
        let request_input = {
            let pending = pending_inputs.clone();
            let engine_tx = engine_tx.clone();
            Box::new(move |questions: Vec<UserInputQuestion>| {
                let (mut tx, rx) = oneshot::channel();
                let (answer_tx, answer_rx) = oneshot::channel();
                let request_id = new_id();
                lock(&pending).insert(request_id.clone(), answer_tx);
                let _ = engine_tx.send(AgentEvent::InputRequested {
                    request_id: request_id.clone(),
                    questions,
                });
                let pending = pending.clone();
                let engine_tx = engine_tx.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        // A dropped resolver stays an error for the harness, as before.
                        answer = answer_rx => if let Ok(answer) = answer { let _ = tx.send(answer); },
                        _ = tx.closed() => {
                            lock(&pending).remove(&request_id);
                            let _ = engine_tx.send(AgentEvent::InputResolved {
                                request_id,
                                answers: Some(Vec::new()),
                            });
                        }
                    }
                });
                rx
            })
        };
        let turn = TurnControl::default();
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fork_history_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (voice_handle, realtime, _voice_events) = zeron_harness::codex::realtime::channel();
        let voice_active = Arc::new(VoiceActivity::default());
        let credential = self.inner.registry.run_credential(harness_id).await;
        let binary = {
            let harness = harness.clone();
            tokio::task::spawn_blocking(move || binary_identity(harness.as_ref()))
                .await
                .ok()
                .flatten()
        };
        let side_chats_only = self.inner.side_chats_only(chat_id, harness_id);
        let controls = RunControls {
            context: Some(zeron_harness::RunContext {
                session_id: chat_id.to_owned(),
                ipc_port: self.inner.ipc_port.load(std::sync::atomic::Ordering::Acquire),
                supervision_token: Some(supervision_token.clone()),
                inference: inference.clone(),
                fork_from,
            }),
            realtime: (harness_id == HarnessId::Codex && !managed).then_some(realtime),
            execution_lease: None,
            request_input,
            steering: steer_rx,
            interrupt: interrupt_token.clone(),
            turn: turn.clone(),
        };

        let superseded = revival.is_some()
            && handle
                .doc()
                .read_entries()
                .unwrap_or_default()
                .iter()
                .rev()
                .find(|entry| entry.role == MessageRole::User)
                .is_some_and(|entry| entry.id != user_id);
        let mut runs = lock(&self.inner.runs);
        let stops = self.inner.user_stop_count(chat_id);
        let moved_on = superseded || (revival.is_some() && runs.contains_key(chat_id));
        let stopped = stops_seen.is_some_and(|seen| seen != stops);
        if moved_on || stopped {
            drop(runs);
            if moved_on {
                tracing::info!(chat = %chat_id, "auto-resume skipped: the chat moved on");
            } else {
                tracing::info!(chat = %chat_id, "startup retry abandoned after Stop");
                self.inner.abandon_retry(chat_id, false);
            }
            return Ok(String::new());
        }
        if self.inner.shutting_down.load(std::sync::atomic::Ordering::Acquire) || interrupt_token.is_cancelled() {
            drop(runs);
            if stops_seen.is_some() {
                self.inner.abandon_retry(chat_id, true);
                return Ok(String::new());
            }
            if let (Some(crew), Some(route)) = (self.inner.crew.get(), inference.as_ref()) {
                crew.retire_inference(route);
            }
            return Err(EngineError::Other("Crew run interrupted before admission".into()));
        }
        if !idle {
            self.inner.journal.clear_user_stop(chat_id);
        }
        runs.insert(
            chat_id.to_string(),
            RunHandle {
                inference,
                inference_requests: 0,
                inference_changed: inference_changed.clone(),
                voice: (harness_id == HarnessId::Codex && !managed).then_some(voice_handle),
                voice_active: voice_active.clone(),
                run_id: run_id.clone(),
                supervision_token,
                supervision,
                steerable: harness.supports_steering(),
                runtime_config: RuntimeConfig::new(harness_id, &request, harness.clone()),
                steer_tx,
                interrupt_token,
                stops_turn_in_place: harness.supports_steering() && harness.stops_turn_in_place(),
                turn: turn.clone(),
                started: started.clone(),
                cancel: cancel_tx,
                engine_tx,
                pending_inputs,
                routed_steers: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                fork_history_sent: fork_history_sent.clone(),
                retiring: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                shutdown_cut: std::sync::atomic::AtomicBool::new(false),
                credential,
                binary,
                side_chats_only,
            },
        );
        drop(runs);
        if !idle {
            // Until the run journals its first event (it may first wait for an
            // agent update or a lease), a dying engine would leave this send
            // looking answered. Boot recovery re-dispatches a marked turn.
            self.inner.journal.note_pending_start(chat_id);
        }
        self.inner.set_run_status(
            chat_id, &run_id,
            if idle {
                SessionStatus::Idle
            } else {
                SessionStatus::Working
            },
            true,
        );
        // AFTER Working (same causal-order guarantee as the steer path): the
        // lastMessageAt bump must never be observable ahead of the live run.
        if !idle {
            self.inner.note_message(chat_id, &request.prompt);
        }

        // Name the chat NOW, off the first prompt — not after the first
        // exchange completes ("called New session for a long time for no
        // reason"; the titler only needs the prompt and skips titled chats;
        // the Done-time call below stays as the retry for a failed
        // generation).
        if !idle && let Some(titles) = self.inner.titles.get() {
            titles.maybe_generate(chat_id, harness_id, &request.prompt, &request.cwd);
        }

        tokio::spawn(drive_run(
            self.inner.clone(),
            chat_id.to_string(),
            run_id.clone(),
            harness,
            request,
            handle.writer(),
            controls,
            engine_rx,
            cancel_rx,
            supervision_rx,
            RunResumeState {
                idle,
                voice_active,
                user_message_id: user_id,
                resume_injected,
                startup_retry,
                revival,
                stops_seen: stops,
                fork_history_sent,
                started,
            },
            turn,
            provider_run,
            inference_changed,
            side_chats_only,
        ));
        Ok(run_id)
    }

    async fn credential_changed(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        started_with: Option<&str>,
    ) -> bool {
        if self.turn_in_flight(chat_id) || self.holds_background_work(chat_id) {
            return false;
        }
        match self.inner.registry.run_credential(harness_id).await {
            Some(now) if Some(now.as_str()) != started_with => {
                tracing::info!(chat = %chat_id, harness = ?harness_id,
                    "credential changed; replacing the parked session");
                true
            }
            _ => false,
        }
    }

    async fn binary_changed(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        harness: Arc<dyn Harness>,
        started_with: Option<&str>,
    ) -> bool {
        let Some(started_with) = started_with else {
            return false;
        };
        if self.turn_in_flight(chat_id) || self.holds_background_work(chat_id) {
            return false;
        }
        let now = tokio::task::spawn_blocking(move || binary_identity(harness.as_ref()))
            .await
            .ok()
            .flatten();
        match now {
            Some(now) if now != started_with => {
                tracing::info!(chat = %chat_id, harness = ?harness_id,
                    "agent CLI changed on disk; replacing the parked session");
                true
            }
            _ => false,
        }
    }

    fn native_subagents_changed(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        started_side_chats_only: bool,
    ) -> bool {
        let side_chats_only = self.inner.side_chats_only(chat_id, harness_id);
        if started_side_chats_only == side_chats_only
            || self.turn_in_flight(chat_id)
            || self.holds_background_work(chat_id)
        {
            return false;
        }
        tracing::info!(chat = %chat_id, harness = ?harness_id,
            "native subagents switched; replacing the parked session");
        true
    }

    /// A warm send's prompt with the fork's copied history in front, when the
    /// live provider session never received it (the fork's first turns were
    /// native commands) and no earlier send in this runtime carries it
    /// (`sent`). `None` = send the prompt as is.
    fn warm_fork_history(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        prompt: &str,
        sent: &std::sync::atomic::AtomicBool,
    ) -> Option<String> {
        if sent.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let session = lock(&self.inner.harness_sessions)
            .get(chat_id)
            .map(|known| known.session_id.clone())
            .filter(|id| !id.is_empty());
        let handle = self.doc_handle(chat_id).ok()?;
        self.inner.fork_history_prompt(
            chat_id,
            handle.doc(),
            harness_id,
            prompt,
            None,
            ProviderSession::Continued(session.as_deref()),
        )
    }

    /// Push a steer prompt into the live run's mailbox. `NotSteerable` when no live
    /// steerable run exists — the caller (command executor) dispatches a new turn.
    pub async fn steer(
        &self,
        chat_id: &str,
        prompt: &str,
        message_id: Option<String>,
    ) -> Result<SteerOutcome, EngineError> {
        self.steer_at(chat_id, prompt, message_id, now_ms()).await
    }

    pub(crate) async fn steer_at(
        &self,
        chat_id: &str,
        prompt: &str,
        message_id: Option<String>,
        issued_at: i64,
    ) -> Result<SteerOutcome, EngineError> {
        self.ensure_execution_allowed(chat_id, None)?;
        self.managed_services()?;
        let target = lock(&self.inner.runs)
            .get(chat_id)
            .filter(|h| h.steerable && !h.retiring.load(std::sync::atomic::Ordering::Acquire))
            .map(|h| {
                (
                    h.run_id.clone(),
                    h.runtime_config.harness_id,
                    h.steer_tx.clone(),
                    h.routed_steers.clone(),
                    h.fork_history_sent.clone(),
                    h.retiring.clone(),
                )
            });
        let Some((run_id, harness_id, steer_tx, ledger, history_sent, retiring)) = target else {
            return Ok(SteerOutcome::NotSteerable);
        };
        zeron_proto::invocation::validate_harness_invocations(prompt, harness_id)
            .map_err(EngineError::Other)?;
        self.inner.boat_lease.protect().await.map_err(EngineError::Other)?;
        let user_id = message_id.unwrap_or_else(new_id);
        let bootstrap = self.warm_fork_history(chat_id, harness_id, prompt, &history_sent);
        let delivered = bootstrap.as_deref().unwrap_or(prompt);
        // A steer runs on the configuration live now; record it with the
        // message so a re-dispatch keeps it.
        let sent_with = self.last_request(chat_id).map(|mut request| {
            request.prompt = prompt.to_string();
            request.attachments = Vec::new();
            Box::new(request)
        });
        let message = SteerMessage {
            prompt: if harness_id == HarnessId::Opencode {
                delivered.to_owned()
            } else {
                zeron_proto::invocation::harness_prompt(delivered, harness_id)
            },
            message_id: Some(user_id.clone()),
            attachments: Vec::new(),
            config: None,
        };
        // Saturation is backpressure, not a dead runtime. Waiting for room
        // preserves the live process and every accepted message in a burst.
        let Ok(permit) = steer_tx.reserve().await else {
            return Ok(SteerOutcome::NotSteerable);
        };
        let accepted = self.inner.registry.while_update_clear(harness_id, || -> Result<bool, EngineError> {
            let runs = lock(&self.inner.runs);
            let Some(handle) = runs.get(chat_id).filter(|h| h.run_id == run_id
                && !h.retiring.load(std::sync::atomic::Ordering::Acquire)) else {
                return Ok(false);
            };
            // Serialize mailbox acceptance with confirmation and Done-time
            // inspection: a fast consumer must never outrun its ledger entry.
            // The update marker is checked in the same critical section, so an
            // accepted update releases the reserved slot instead.
            let mut pending = lock(&ledger);
            self.inner.start_turn(chat_id, handle)?;
            pending.push_back(RoutedSteer {
                prompt: prompt.to_string(),
                message_id: user_id.clone(),
                fork_history: bootstrap.is_some(),
                after_stop: retiring.load(std::sync::atomic::Ordering::Acquire),
                request: sent_with,
            });
            permit.send(message);
            Ok(true)
        });
        match accepted.transpose()? {
            None => return Ok(SteerOutcome::DeferredByUpdate),
            Some(false) => return Ok(SteerOutcome::NotSteerable),
            Some(true) => {}
        }
        let handle = self.doc_handle(chat_id)?;
        handle.write_user_message(&user_id, prompt, issued_at.min(now_ms()))?;
        // A routed steer is a turn too. Fired here (not only on the confirmed
        // path) — a reclaim falls back to dispatch, which just re-snapshots.
        if let Some(request) = self.last_request(chat_id) {
            self.note_turn_start(chat_id, &request.cwd);
        }
        if self.is_live(chat_id, &run_id) {
            if bootstrap.is_some() {
                history_sent.store(true, std::sync::atomic::Ordering::Release);
            }
            self.inner.set_run_status(chat_id, &run_id, SessionStatus::Working, false);
            self.inner.note_message(chat_id, prompt);
            return Ok(SteerOutcome::Accepted);
        }
        // The run died around the send. Exit drain claimed the entry → its
        // re-dispatch owns the message; still ours → reclaim and report
        // NotSteerable so the executor falls back to a fresh dispatch
        // (same message id — the doc entry dedupes).
        let reclaimed = {
            let mut ledger = lock(&ledger);
            let before = ledger.len();
            ledger.retain(|s| s.message_id != user_id);
            ledger.len() != before
        };
        if reclaimed {
            return Ok(SteerOutcome::NotSteerable);
        }
        self.inner.note_message(chat_id, prompt);
        Ok(SteerOutcome::Accepted)
    }

    /// A Stop acting now cancels these messages, sent before it but still
    /// on their way: one that reaches dispatch after the runtime went down
    /// is dropped rather than run in a fresh runtime.
    pub fn cancel_sent_messages(&self, chat_id: &str, message_ids: Vec<String>) {
        if message_ids.is_empty() {
            return;
        }
        lock(&self.inner.cancelled_sends)
            .entry(chat_id.to_string())
            .or_default()
            .extend(message_ids);
    }

    /// The user's Stop: a hard boundary. The in-flight turn ends `aborted`
    /// and the runtime goes with it — background subagents, background
    /// shells, everything the agent runs. The next message resumes the
    /// conversation in a fresh process. (Sending, steering and switching
    /// models never come here: they keep the runtime and its work.)
    pub async fn interrupt(&self, chat_id: &str) -> Result<bool, EngineError> {
        self.note_user_stop(chat_id);
        self.terminate(chat_id).await
    }

    fn note_user_stop(&self, chat_id: &str) {
        *lock(&self.inner.user_stops)
            .entry(chat_id.to_string())
            .or_default() += 1;
        let live = if let Some(run) = lock(&self.inner.runs).get(chat_id) {
            run.shutdown_cut
                .store(false, std::sync::atomic::Ordering::Release);
            true
        } else {
            false
        };
        if live && self.turn_in_flight(chat_id) {
            self.inner.journal.note_user_stop(chat_id);
        }
    }

    /// End the in-flight turn so another prompt can take its place ("send
    /// now"), keeping the runtime and its background work. Its streaming
    /// entry is stamped `aborted`; this waits (bounded) for that settlement
    /// so callers observe a consistent doc. Returns whether there was a turn
    /// (or runtime) to stop.
    ///
    /// A driver that [`Harness::stops_turn_in_place`] ends only the turn: the
    /// runtime parks with its background work — subagents, background shells
    /// — still running, and the next prompt routes into it. Drivers without
    /// that, runs whose harness has not started yet, and stops that do not
    /// settle fall back to [`Self::terminate`].
    pub async fn stop_turn(&self, chat_id: &str) -> Result<bool, EngineError> {
        const STOP_SETTLE: std::time::Duration = std::time::Duration::from_secs(15);
        self.note_user_stop(chat_id);
        let target = lock(&self.inner.runs)
            .get(chat_id)
            .filter(|h| {
                h.stops_turn_in_place && h.started.load(std::sync::atomic::Ordering::Acquire)
            })
            .map(|h| (h.run_id.clone(), h.turn.clone(), h.pending_inputs.clone()));
        let Some((run_id, turn, pending)) = target else {
            return self.terminate(chat_id).await;
        };
        if !self.turn_in_flight(chat_id) {
            return Ok(false); // parked between turns: nothing to stop
        }
        // Unpark questions first: the stopped turn can never be answered.
        let parked: Vec<_> = lock(&pending).drain().map(|(_, tx)| tx).collect();
        for tx in parked {
            let _ = tx.send(Vec::new());
        }
        turn.stop_turn();
        let deadline = tokio::time::Instant::now() + STOP_SETTLE;
        while tokio::time::Instant::now() < deadline {
            if !self.is_live(chat_id, &run_id) || !self.turn_in_flight(chat_id) {
                return Ok(true);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tracing::warn!(chat = %chat_id, "turn stop did not settle; tearing the runtime down");
        self.terminate(chat_id).await
    }

    /// Tear the chat's live runtime down (protocol interrupt, then the
    /// child). The run settles with a synthetic `Done{interrupted}` and its
    /// streaming entry stamped `aborted`; this waits (bounded) for that
    /// settlement. Background work the runtime held dies with it.
    pub async fn terminate(&self, chat_id: &str) -> Result<bool, EngineError> {
        let target = lock(&self.inner.runs).get(chat_id).map(|h| {
            (
                h.run_id.clone(),
                h.interrupt_token.clone(),
                h.cancel.clone(),
                h.pending_inputs.clone(),
                h.routed_steers.clone(),
                h.retiring.clone(),
            )
        });
        let Some((run_id, token, cancel, pending, ledger, retiring)) = target else {
            return Ok(false);
        };
        // From here the runtime takes no prompt: a send now starts a fresh
        // runtime, and one that raced into this mailbox is re-dispatched.
        {
            let _order = lock(&ledger);
            retiring.store(true, std::sync::atomic::Ordering::Release);
        }
        // Publish cancellation BEFORE waking the harness. Otherwise a fast
        // EOF after token.cancel() can beat this watch notification and be
        // classified as an error instead of an interrupted turn.
        let _ = cancel.send(true);
        // Unpark questions before harness teardown, which can await them.
        let parked: Vec<_> = lock(&pending).drain().map(|(_, tx)| tx).collect();
        for tx in parked {
            let _ = tx.send(Vec::new());
        }
        // Harness-level interrupt (protocol + child teardown) …
        token.cancel();
        // Bounded settle wait (the run task appends Done + stamps `aborted`).
        for _ in 0..500 {
            if !self.is_live(chat_id, &run_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok(true)
    }

    /// Fold an engine-made event (a `show_visual` page) into the chat's live
    /// run, after whatever the harness streamed so far. Returns `false` when
    /// the chat has no run on this device.
    /// Whether the chat has a run on this device (one `inject_event` can reach).
    pub fn has_run(&self, chat_id: &str) -> bool {
        lock(&self.inner.runs).contains_key(chat_id)
    }

    pub fn inject_event(&self, chat_id: &str, event: AgentEvent) -> bool {
        lock(&self.inner.runs)
            .get(chat_id)
            .is_some_and(|h| h.engine_tx.send(event).is_ok())
    }

    /// Resolve a pending `request_input` question set. Returns `false` when no such
    /// request is pending (unknown id, or the run already settled).
    pub fn respond_input(
        &self,
        chat_id: &str,
        request_id: &str,
        answers: Vec<UserInputAnswer>,
    ) -> Result<bool, EngineError> {
        let target = lock(&self.inner.runs)
            .get(chat_id)
            .map(|h| (h.pending_inputs.clone(), h.engine_tx.clone()));
        let Some((pending, engine_tx)) = target else {
            return Ok(false);
        };
        let Some(resolver) = lock(&pending).remove(request_id) else {
            return Ok(false);
        };
        let _ = engine_tx.send(AgentEvent::InputResolved {
            request_id: request_id.to_string(),
            answers: Some(answers.clone()),
        });
        let _ = resolver.send(answers);
        Ok(true)
    }

    fn recover_imported(&self) -> Result<usize, EngineError> {
        let Some(intents) = self.inner.imported_recovery.get().filter(|intents| !intents.is_empty()) else {
            return Ok(0);
        };
        let crew = self.inner.crew.get()
            .ok_or_else(|| EngineError::Other("Imported recovery requires verified Crew sign-in".into()))?;
        let (owner, scope) = crew.controller().map_err(|error| EngineError::Other(error.to_string()))?;
        let host = self.inner.doc_host()
            .ok_or_else(|| EngineError::Other("Imported recovery requires its document host".into()))?;
        let mut recovered = 0;
        for intent in intents {
            if self.original_epoch_history(&intent.chat_id).is_some() { continue; }
            if lock(&self.inner.runs).contains_key(&intent.chat_id) { continue; }
            if !lock(&self.inner.imported_native_histories).contains_key(&intent.native_session_id) {
                return Err(EngineError::Other("Imported recovery has no verified native history".into()));
            }
            recovered += usize::from(admit_imported_recovery(&host, intent, &self.inner.device_id, &owner, &scope)?);
        }
        Ok(recovered)
    }

    /// Boot recovery: for every journal whose last event is not `Done` (a run died
    /// mid-stream), stamp this device's abandoned `streaming` doc entries `aborted`
    /// with a VISIBLE "Run interrupted by engine restart" error part, close the
    /// journal with a synthetic `Done{interrupted}` — and then PICK THE RUN BACK
    /// UP: a fresh crashed turn with revival budget left is re-dispatched against
    /// the remembered harness session (zeron: "not just eulogized";
    /// `MAX_AUTO_RESUME` = 3 consecutive revivals, fresh = crashed < 12h ago).
    pub fn recover_stale(&self) -> Result<usize, EngineError> {
        const MAX_AUTO_RESUME: u32 = 3;
        const RESUME_FRESH_MS: i64 = 12 * 60 * 60 * 1000;

        let mut cut_off: std::collections::BTreeMap<String, Option<i64>> = self
            .inner
            .journal
            .stale_sessions()?
            .into_iter()
            .map(|chat_id| (chat_id, None))
            .collect();
        // Sends admitted but never journaled: the engine died while the run
        // waited to start, behind a Done that reads as idle. A stale journal
        // (the run did journal before dying) speaks for itself instead.
        let mut unstarted = std::collections::BTreeSet::new();
        for chat_id in self.inner.journal.pending_starts()? {
            self.inner.journal.clear_pending_start(&chat_id);
            if !cut_off.contains_key(&chat_id) {
                cut_off.insert(chat_id.clone(), None);
                unstarted.insert(chat_id);
            }
        }
        for (chat_id, at) in self.inner.journal.shutdown_interrupts()? {
            if cut_off.contains_key(&chat_id) || self.inner.ended_by_shutdown(&chat_id) {
                cut_off.entry(chat_id).or_insert(Some(at));
            } else {
                self.inner.journal.clear_shutdown_interrupt(&chat_id);
            }
        }
        let mut recovered = self.recover_imported()?;
        for (chat_id, shutdown_at) in cut_off {
            self.inner.journal.clear_shutdown_interrupt(&chat_id);
            // Only an actual new native session associated after trusted Start
            // can recover. Original/unknown journals never seed a new epoch.
            if let Some(history) = self.original_epoch_history(&chat_id) {
                let Some((native, _, _)) = self.inner.journal_harness_session(&chat_id) else { continue; };
                if history.native_session_id.as_deref() == Some(native.as_str())
                    || self.ensure_execution_allowed(&chat_id, Some(&native)).is_err()
                    || self.inner.workspace().and_then(|workspace| workspace.chat_harness_session(&chat_id))
                        .is_none_or(|stored| stored.session_id != native)
                { continue; }
            }
            // Imported execution has an exact durable request and native command claim.
            // A missing outcome is unknown, never permission to synthesize/replay a prompt.
            if self.inner.imported_recovery.get().is_some_and(|intents|
                intents.iter().any(|intent| intent.chat_id == chat_id)) { continue; }
            if lock(&self.inner.runs).contains_key(&chat_id) {
                continue; // a live run owns this journal
            }
            let stopped = self.inner.journal.user_stopped(&chat_id);
            self.inner.journal.clear_user_stop(&chat_id);
            let handle = self.doc_handle(&chat_id)?;
            // Harness continuity first: the crashed run's session id may only
            // exist in the journal (the debounced workspace-row write may
            // never have landed) — remember it so the revived run resumes the
            // same harness conversation (zeron recoverDraft, sessions.ts:538).
            if let Some((session_id, cwd, Some(harness))) =
                self.inner.journal_harness_session(&chat_id)
            {
                self.inner
                    .remember_harness_session(&chat_id, &session_id, &cwd, harness);
            }
            // The revival prompt: the last user message (idempotent re-dispatch
            // under the SAME id — `write_user_message` dedupes by id, so the
            // transcript never shows a duplicate).
            let entries = handle.doc().read_entries().unwrap_or_default();
            let prompt = entries
                .iter()
                .rev()
                .find(|e| e.role == MessageRole::User)
                .and_then(|e| {
                    e.parts.iter().find_map(|p| match p {
                        MessagePart::Text { text, .. } => Some((e.id.clone(), text.clone())),
                        _ => None,
                    })
                });
            let now = now_ms();
            let attempts = self
                .inner
                .journal
                .resume_attempts(&chat_id, now - RESUME_FRESH_MS);
            // Fresh: the crashed reply's streaming entry is recent — or no
            // reply entry reached disk at all (the doc saves on a debounce,
            // and a kill moments into the turn leaves the user's message as
            // the transcript's last word), and that message is recent.
            let fresh = shutdown_at.is_some_and(|at| now - at < RESUME_FRESH_MS)
                || shutdown_at.is_none()
                    && entries
                        .iter()
                        .rev()
                        .find(|e| e.status == Some(MessageStatus::Streaming))
                        .or_else(|| entries.last().filter(|e| e.role == MessageRole::User))
                        .is_some_and(|e| now - e.created_at < RESUME_FRESH_MS);
            let crashed = shutdown_at.is_none();
            // An unstarted send owns none of the journal: its last Done is the
            // previous turn's, and nothing reached the provider.
            let tail = if unstarted.contains(&chat_id) {
                TurnTail::default()
            } else {
                self.inner
                    .journal
                    .events_rev(&chat_id)
                    .map(|events| turn_tail(events.map_while(Result::ok).map(|(_, event)| event), !crashed))
                    .unwrap_or_default()
            };
            let request = prompt
                .as_ref()
                .and_then(|(_, text)| self.revival_request(&chat_id, text));
            let pickup = match &request {
                Some((_, harness_id))
                    if fresh
                        && !stopped
                        && !(crashed && tail.finished)
                        && prompt
                            .as_ref()
                            .is_some_and(|(_, text)| !native_command(text, *harness_id)) =>
                {
                    if tail.awaiting_input {
                        Pickup::AwaitingAnswer
                    } else if attempts >= MAX_AUTO_RESUME {
                        Pickup::Exhausted
                    } else {
                        Pickup::Resume
                    }
                }
                _ => Pickup::Skip,
            };

            let note = match pickup {
                Pickup::Resume => "Run interrupted by engine restart — resuming".to_owned(),
                Pickup::AwaitingAnswer => "Run interrupted by engine restart while it waited for your answer — send a message to continue".to_owned(),
                Pickup::Exhausted => format!(
                    "Run interrupted by engine restart — not resumed automatically after {MAX_AUTO_RESUME} tries in a row; send a message to continue"
                ),
                Pickup::Skip => "Run interrupted by engine restart".to_owned(),
            };
            if crashed {
                self.inner.publish(
                    &chat_id,
                    &AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: Some(note.clone()),
                        session_id: None,
                    },
                );
            }
            let stamped = handle.mark_abandoned_streams(&note)?.len();
            self.inner.set_status_with_completion(
                &chat_id,
                None,
                SessionStatus::Idle,
                false,
                None,
                Some(DoneStatus::Interrupted),
            );
            tracing::info!(chat = %chat_id, stamped, ?pickup, attempts, "recovered interrupted session");
            recovered += 1;

            let (Pickup::Resume, Some((user_id, prompt_text)), Some((mut request, harness_id))) =
                (pickup, prompt, request)
            else { continue; };
            let attempt = self.inner.journal.note_resume_attempt(
                &chat_id,
                now,
                now - RESUME_FRESH_MS,
            );
            let continuation = (tail.reached_provider && answered_in(&entries, &user_id))
                .then(|| revival_prompt(&prompt_text));
            lock(&self.inner.revivals).insert(
                chat_id.clone(),
                Revival {
                    message_id: user_id.clone(),
                    continuation,
                },
            );
            request.resume = None;
            request.attachments = Vec::new();
            let sessions = self.clone();
            tokio::spawn(async move {
                let Some(host) = sessions.inner.doc_host() else {
                    return;
                };
                if sessions.has_run(&chat_id) {
                    sessions.inner.drop_revival(&chat_id, &user_id);
                    return;
                }
                match host
                    .dispatch_with_source_context(
                        &sessions,
                        &chat_id,
                        harness_id,
                        request,
                        Some(user_id.clone()),
                    )
                    .await
                {
                    Ok(_) => {
                        tracing::info!(chat = %chat_id, attempt, "auto-resumed crashed run")
                    }
                    Err(err) => {
                        sessions.inner.drop_revival(&chat_id, &user_id);
                        tracing::warn!(chat = %chat_id, error = %err, "auto-resume dispatch failed")
                    }
                }
            });
        }
        Ok(recovered)
    }

    fn revival_request(&self, chat_id: &str, prompt: &str) -> Option<(RunRequest, HarnessId)> {
        let host = self.inner.doc_host()?;
        let mut request = self
            .last_request(chat_id)
            .or_else(|| host.request_from_chat_row(chat_id, prompt))
            .or_else(|| {
                let (_, cwd, _) = self.inner.journal_harness_session(chat_id)?;
                Some(RunRequest {
                    mcp: None,
                    prompt: String::new(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd,
                    sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                    auto_approve: false,
                    access: Default::default(),
                    plan: false,
                    attachments: Vec::new(),
                    resume: None,
                    worktree: None,
                    env: Default::default(),
                })
            })?;
        request.prompt = prompt.to_owned();
        let harness_id = host.harness_for_request(chat_id, &request);
        Some((request, harness_id))
    }

    /// Graceful shutdown: interrupt every live run so streaming entries settle.
    pub async fn shutdown(&self) {
        self.inner
            .shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.boat_lease.shutdown_now();
        self.inner.voice.retire();
        if self
            .inner
            .revive_after_shutdown
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let working: Vec<String> = lock(&self.inner.statuses)
                .values()
                .filter(|session| session.status == SessionStatus::Working)
                .map(|session| session.chat_id.clone())
                .collect();
            let runs = lock(&self.inner.runs);
            for chat_id in working {
                if let Some(run) = runs
                    .get(&chat_id)
                    .filter(|run| !run.retiring.load(std::sync::atomic::Ordering::Acquire))
                {
                    run.shutdown_cut
                        .store(true, std::sync::atomic::Ordering::Release);
                }
            }
        } else {
            let live: Vec<String> = lock(&self.inner.runs).keys().cloned().collect();
            for chat_id in live {
                if self.turn_in_flight(&chat_id) {
                    self.inner.journal.note_user_stop(&chat_id);
                }
            }
        }
        if let Some(crew) = self.inner.crew.get() {
            for run in lock(&self.inner.runs).values() {
                if let Some(route) = &run.inference {
                    crew.retire_inference(route);
                }
            }
        }
        let chats: Vec<String> = lock(&self.inner.runs).keys().cloned().collect();
        for chat_id in chats {
            if let Err(err) = self.terminate(&chat_id).await {
                tracing::warn!(chat = %chat_id, error = %err, "shutdown interrupt failed");
            }
        }
        self.inner.boat_lease.shutdown().await;
        self.inner.namespace_tasks.shutdown();
    }

    pub fn forgo_revival(&self) {
        self.inner
            .revive_after_shutdown
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn is_live(&self, chat_id: &str, run_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(chat_id)
            .is_some_and(|h| h.run_id == run_id)
    }

    fn set_status(&self, chat_id: &str, status: SessionStatus, fresh_start: bool) {
        self.inner.set_status(chat_id, status, fresh_start);
    }
}

impl Inner {
    fn mark_shutdown_cut(&self, chat_id: &str, run_id: &str) -> bool {
        let cut = lock(&self.runs)
            .get(chat_id)
            .filter(|run| run.run_id == run_id)
            .is_some_and(|run| {
                run.shutdown_cut
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
            });
        if cut
            && self
                .revive_after_shutdown
                .load(std::sync::atomic::Ordering::Acquire)
        {
            self.journal.note_shutdown_interrupt(chat_id, now_ms());
            true
        } else {
            false
        }
    }

    fn ended_by_shutdown(&self, chat_id: &str) -> bool {
        self.journal
            .events_rev(chat_id)
            .ok()
            .and_then(|mut events| events.next())
            .and_then(Result::ok)
            .is_some_and(|(_, event)| {
                matches!(
                    event,
                    AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        ..
                    }
                )
            })
    }

    fn reviving(&self, chat_id: &str, message_id: &str) -> bool {
        lock(&self.revivals)
            .get(chat_id)
            .is_some_and(|revival| revival.message_id == message_id)
    }

    fn user_stop_count(&self, chat_id: &str) -> u64 {
        lock(&self.user_stops).get(chat_id).copied().unwrap_or(0)
    }

    fn abandon_retry(&self, chat_id: &str, cut_by_shutdown: bool) {
        // A retry has no live handle of its own. Serialize every durable
        // abandonment side effect with run insertion so a newer replacement
        // can never inherit this retry's Done or shutdown marker.
        let runs = lock(&self.runs);
        if runs.contains_key(chat_id) {
            return;
        }
        if cut_by_shutdown
            && self
                .revive_after_shutdown
                .load(std::sync::atomic::Ordering::Acquire)
        {
            self.journal.note_shutdown_interrupt(chat_id, now_ms());
        }
        self.publish(
            chat_id,
            &AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: cut_by_shutdown.then(|| SHUTDOWN_NOTE.into()),
                session_id: None,
            },
        );
        drop(runs);
        // `None` rechecks ownership after releasing the lock above; if a run
        // arrived meanwhile, its status remains untouched.
        self.set_status_with_completion(
            chat_id,
            None,
            SessionStatus::Idle,
            false,
            None,
            Some(DoneStatus::Interrupted),
        );
    }

    fn drop_revival(&self, chat_id: &str, message_id: &str) {
        let mut revivals = lock(&self.revivals);
        if revivals
            .get(chat_id)
            .is_some_and(|revival| revival.message_id == message_id)
        {
            revivals.remove(chat_id);
        }
    }

    fn protection_required(&self) -> bool {
        self.boat_lease.required() || self.namespace_tasks.required()
    }

    fn start_turn(&self, chat_id: &str, handle: &RunHandle) -> Result<(), EngineError> {
        self.boat_lease.start_turn(
            &handle.run_id, &handle.interrupt_token, &handle.cancel, &handle.engine_tx,
        ).map_err(EngineError::Other)?;
        if let Err(error) = self.namespace_tasks.start_turn(chat_id, &handle.run_id) {
            self.boat_lease.finish_turn(&handle.run_id);
            return Err(EngineError::Other(error.to_string()));
        }
        Ok(())
    }

    fn finish_turn(&self, run_id: &str) {
        self.boat_lease.finish_turn(run_id);
        self.namespace_tasks.finish_turn(run_id);
    }

    fn inference_admission(
        self: &Arc<Self>, chat_id: &str, run_id: &str, interrupt: &CancellationToken,
    ) -> Option<crate::inference_relay::RunInferenceAdmission> {
        if !self.protection_required() { return None; }
        let inner = Arc::downgrade(self);
        let identity = Arc::new(RunIdentity { chat_id: chat_id.to_owned(), run_id: run_id.to_owned() });
        let cancellation = interrupt.clone();
        Some(crate::inference_relay::RunInferenceAdmission {
            cancellation: cancellation.clone(),
            check: Arc::new(move || {
                let inner = inner.clone();
                let identity = identity.clone();
                let cancellation = cancellation.clone();
                Box::pin(async move {
                    let inner = inner.upgrade().ok_or_else(|| EngineError::Other("Crew runtime retired".into()))?;
                    let result = async {
                        if let Some(activity) = inner.begin_inference(&identity, false)? { return Ok(activity); }
                        tokio::select! {
                            biased;
                            _ = cancellation.cancelled() => return Err(EngineError::Other("Crew inference run interrupted".into())),
                            result = inner.boat_lease.protect() => result.map_err(EngineError::Other)?,
                        }
                        inner.begin_inference(&identity, true)?
                            .ok_or_else(|| EngineError::Other("Crew inference admission disappeared".into()))
                    }.await;
                    if let Err(error) = &result {
                        let runs = lock(&inner.runs);
                        if let Some(handle) = runs.get(&identity.chat_id).filter(|h| h.run_id == identity.run_id
                            && !h.interrupt_token.is_cancelled()) {
                            let _ = handle.engine_tx.send(AgentEvent::Done {
                                status: DoneStatus::Errored, result: None, error: Some(error.to_string()), session_id: None,
                            });
                            let _ = handle.cancel.send(true);
                            handle.interrupt_token.cancel();
                        }
                    }
                    result
                })
            }),
        })
    }

    fn begin_inference(self: &Arc<Self>, identity: &Arc<RunIdentity>, confirmed: bool)
        -> Result<Option<InferenceActivity>, EngineError>
    {
        let mut runs = lock(&self.runs);
        let handle = runs.get_mut(&identity.chat_id).filter(|h| h.run_id == identity.run_id
            && !h.retiring.load(std::sync::atomic::Ordering::Acquire)
            && !h.interrupt_token.is_cancelled())
            .ok_or_else(|| EngineError::Other("Stale or interrupted Crew inference run".into()))?;
        if self.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
            return Err(EngineError::Other("Crew is shutting down".into()));
        }
        if !confirmed && self.boat_lease.required() && !self.boat_lease.active(&identity.run_id) { return Ok(None); }
        self.start_turn(&identity.chat_id, handle)?;
        handle.inference_requests += 1;
        let changed = handle.inference_changed.clone();
        // OMP also uses this route for background skill-description compression.
        // Only its native turn events own foreground status; every request still
        // owns provider protection until its response is consumed or dropped.
        let inference_drives_turn = handle.runtime_config.harness_id != HarnessId::Omp;
        drop(runs);
        if inference_drives_turn {
            self.set_run_status(&identity.chat_id, &identity.run_id, SessionStatus::Working, false);
        }
        if let Some(changed) = changed { changed.notify_one(); }
        Ok(Some(InferenceActivity { inner: Arc::downgrade(self), identity: identity.clone() }))
    }

    fn inference_in_flight(&self, chat_id: &str, run_id: &str) -> bool {
        lock(&self.runs).get(chat_id).is_some_and(|h| h.run_id == run_id && h.inference_requests > 0)
    }

    /// Journal + broadcast one event (the two unconditional legs of the pipeline).
    /// Sanitize before ANY journal/publish path, including nested subagent events.
    async fn prepare_generated_image(
        &self,
        chat_id: &str,
        event: AgentEvent,
        seen: &mut std::collections::HashSet<String>,
    ) -> Vec<AgentEvent> {
        let mut parents = Vec::new();
        let mut leaf = event;
        while let AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } = leaf
        {
            parents.push(parent_tool_use_id);
            leaf = *event;
        }
        let events = if let AgentEvent::GeneratedImage { id, path, .. } = leaf {
            let key = std::iter::once(chat_id)
                .chain(parents.iter().map(String::as_str))
                .chain(std::iter::once(id.as_str()))
                .collect::<Vec<_>>()
                .join("\0");
            if seen.contains(&key) {
                return vec![];
            }
            let result = if let Some((uploads, allowed_root)) = self.generated_images.get() {
                let uploads = uploads.clone();
                let root = allowed_root.clone();
                let stable_key = key.clone();
                tokio::task::spawn_blocking(move || {
                    uploads.import_generated_image(std::path::Path::new(&path), &root, &stable_key)
                })
                .await
                .unwrap_or_else(|err| Err(EngineError::Other(err.to_string())))
            } else {
                Err(EngineError::Other(
                    "Generated image intake is not configured".into(),
                ))
            };
            match result {
                Ok(image) => {
                    seen.insert(key);
                    vec![AgentEvent::GeneratedImage {
                        id,
                        path: image.path,
                        name: image.name,
                        mime_type: image.mime_type,
                    }]
                }
                Err(err) => {
                    tracing::warn!(chat = %chat_id, error = %err, "generated image import failed");
                    vec![
                        AgentEvent::ToolResult {
                            id: id.strip_suffix(":image").unwrap_or(&id).into(),
                            is_error: true,
                            output: None,
                            diff: None,
                        },
                        AgentEvent::Error {
                            message: "Generated image unavailable".into(),
                        },
                    ]
                }
            }
        } else {
            vec![leaf]
        };
        events
            .into_iter()
            .map(|mut event| {
                for parent in parents.iter().rev() {
                    event = AgentEvent::Subagent {
                        parent_tool_use_id: parent.clone(),
                        event: Box::new(event),
                    };
                }
                event
            })
            .collect()
    }

    fn publish(&self, chat_id: &str, event: &AgentEvent) -> u64 {
        let seq = match self.journal.append(chat_id, event) {
            Ok(seq) => seq,
            Err(err) => {
                tracing::error!(chat = %chat_id, error = %err, "journal append failed");
                0
            }
        };
        if let Some(hub) = lock(&self.hubs).get(chat_id) {
            let _ = hub.send(JournaledEvent {
                seq,
                event: event.clone(),
            });
        }
        seq
    }

    /// Bump the session's freshness on stream activity WITHOUT a status
    /// transition. Long silent-LOOKING stretches (thinking heartbeats, a big
    /// tool input being generated) still carry events — the UI's 45s
    /// staleness gate must not flip "Working" off mid-run. Throttled: a
    /// workspace-doc mirror per delta would be far too chatty.
    fn touch_session(&self, chat_id: &str, run_id: &str) {
        let runs = lock(&self.runs);
        if !runs.get(chat_id).is_some_and(|h| h.run_id == run_id) { return; }
        const TOUCH_THROTTLE_MS: i64 = 10_000;
        let now = Utc::now();
        let session = {
            let mut statuses = lock(&self.statuses);
            let Some(entry) = statuses.get_mut(chat_id) else {
                return;
            };
            let age = now
                .signed_duration_since(entry.updated_at)
                .num_milliseconds();
            if age < TOUCH_THROTTLE_MS {
                return;
            }
            entry.updated_at = now;
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            session
        };
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
    }

    fn set_status(&self, chat_id: &str, status: SessionStatus, fresh_start: bool) {
        self.set_status_with_completion(chat_id, None, status, fresh_start, None, None);
    }

    fn set_run_status(&self, chat_id: &str, run_id: &str, status: SessionStatus, fresh_start: bool) {
        self.set_status_with_completion(chat_id, Some(run_id), status, fresh_start, None, None);
    }

    /// Publish how many subagents of `chat_id` are streaming. A change is
    /// mirrored like any status transition (it doubles as a heartbeat); an
    /// unchanged count is a no-op, and a chat without a status row has
    /// nothing to badge yet.
    fn set_running_subagents(&self, chat_id: &str, run_id: &str, running: usize) {
        let runs = lock(&self.runs);
        if !runs.get(chat_id).is_some_and(|h| h.run_id == run_id) { return; }
        let running = u32::try_from(running).unwrap_or(u32::MAX);
        let session = {
            let mut statuses = lock(&self.statuses);
            let Some(entry) = statuses.get_mut(chat_id) else {
                return;
            };
            if entry.running_subagents == running {
                return;
            }
            entry.running_subagents = running;
            entry.updated_at = Utc::now();
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            session
        };
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
    }

    fn has_pending_steers(&self, chat_id: &str, run_id: &str) -> bool {
        lock(&self.runs)
            .get(chat_id)
            .filter(|h| h.run_id == run_id)
            .is_some_and(|h| !lock(&h.routed_steers).is_empty())
    }

    /// Publish the completion marker atomically with the settled status. Keep
    /// it on subsequent Working/heartbeat rows for coalesced and remote watches.
    fn set_status_with_completion(
        &self,
        chat_id: &str,
        run_id: Option<&str>,
        status: SessionStatus,
        fresh_start: bool,
        completed_turn: Option<String>,
        outcome: Option<DoneStatus>,
    ) {
        // Hold ownership through status publication: a late retired task must
        // never settle or release whichever replacement now owns the chat.
        let runs = lock(&self.runs);
        self.set_status_with_completion_locked(&runs, chat_id, run_id, status, fresh_start, completed_turn, outcome);
    }

    fn set_status_with_completion_locked(
        &self,
        runs: &MutexGuard<'_, HashMap<String, RunHandle>>,
        chat_id: &str,
        run_id: Option<&str>,
        status: SessionStatus,
        fresh_start: bool,
        completed_turn: Option<String>,
        outcome: Option<DoneStatus>,
    ) {
        let handle = runs.get(chat_id);
        match run_id {
            Some(run) if !handle.is_some_and(|h| h.run_id == run) => return,
            None if handle.is_some() => return,
            _ => {}
        }
        if matches!(status, SessionStatus::Idle | SessionStatus::Errored)
            && let Some(handle) = handle
        {
            let pending = lock(&handle.routed_steers);
            if pending.is_empty() && handle.inference_requests == 0 {
                self.finish_turn(&handle.run_id);
            }
        }
        let now = Utc::now();
        let session = {
            let mut statuses = lock(&self.statuses);
            let entry = statuses
                .entry(chat_id.to_string())
                .or_insert_with(|| Session {
                    last_completed_turn: None,
                    chat_id: chat_id.to_string(),
                    device_id: self.device_id.clone(),
                    status,
                    started_at: None,
                    updated_at: now,
                    last_turn_outcome: None,
                    terminal_activity: None,
                    running_subagents: 0,
                });
            // `started_at` is the elapsed-timer base and must only ever mean
            // "this turn". Entering Working from a settled state always
            // restamps (a steer into a parked session is a NEW turn — reusing
            // the old base showed the previous turn's 30min on send), and a
            // settled session drops its base entirely so no later reader can
            // resurrect a stale elapsed.
            let was_active = matches!(
                entry.status,
                SessionStatus::Working | SessionStatus::AwaitingInput
            );
            if let Some(turn) = completed_turn {
                entry.last_completed_turn = Some(turn);
            }
            entry.status = status;
            if let Some(outcome) = outcome {
                entry.last_turn_outcome = Some(zeron_proto::NativeTurnOutcome {
                    status: outcome,
                    finished_at: now,
                });
            } else if status == SessionStatus::Errored {
                entry.last_turn_outcome = Some(zeron_proto::NativeTurnOutcome {
                    status: DoneStatus::Errored,
                    finished_at: now,
                });
            } else if status == SessionStatus::Working && (fresh_start || !was_active) {
                entry.last_turn_outcome = None;
            }
            entry.updated_at = now;
            match status {
                SessionStatus::Working if fresh_start || !was_active => {
                    entry.started_at = Some(now);
                }
                SessionStatus::Working | SessionStatus::AwaitingInput => {}
                SessionStatus::Idle | SessionStatus::Errored => {
                    entry.started_at = None;
                }
            }
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            // send_replace: keep the current value fresh even with no receivers,
            // so late WatchSessions subscribers see the last transition.
            self.sessions_tx.send_replace(list);
            session
        };
        // Mirror the transition into the workspace doc's session-status row so
        // remote devices' sidebars show this run (staleness-checked client-side).
        if self.activity_tx.get().is_none()
            && let Some(ws) = self.workspace()
        {
            ws.record_session(&session);
        }
    }

    /// The doc host, once wired. `None` before assembly or after retirement.
    fn doc_host(&self) -> Option<DocHost> {
        lock(&self.doc_host).clone()
    }

    /// A message the user already sent follows the turn that is ending
    /// (see [`crate::doc_host::ChatDocHandle::continues_with_sent_message`]).
    fn continues_with_sent_message(&self, chat_id: &str) -> bool {
        self.doc_host()
            .and_then(|host| host.open(chat_id).ok())
            .is_some_and(|handle| handle.continues_with_sent_message())
    }

    /// Whether a run of `chat_id` launches without its provider's own
    /// subagents (Settings → Providers), delegating through side chats
    /// instead. Never for a chat that has a parent: delegation is one level
    /// deep, so a side chat can't create chats and keeps its provider's own.
    /// Nor while its parent is unknown here (its run outran its row).
    fn side_chats_only(&self, chat_id: &str, harness_id: HarnessId) -> bool {
        !self.registry.native_subagents(harness_id)
            && self.workspace().is_none_or(|ws| {
                !ws.parent_unknown(chat_id)
                    && !ws
                        .chat(chat_id)
                        .ok()
                        .flatten()
                        .is_some_and(|chat| chat.parent_chat_id.is_some())
            })
    }

    /// Zeron's own MCP server for a run of `chat_id`: this binary's `zeron
    /// mcp` subcommand, dialing the engine's IPC port and stamped with the
    /// originating chat + device so the agent's side chats link back here.
    /// None when the engine serves no port or its executable is unknown.
    fn zeron_mcp(&self, chat_id: &str, side_chats_only: bool) -> Option<zeron_proto::McpServer> {
        let port = self.ipc_port.load(std::sync::atomic::Ordering::Relaxed);
        if port == 0 {
            return None;
        }
        let command = std::env::current_exe().ok()?.to_str()?.to_owned();
        let mut env: std::collections::BTreeMap<String, String> = [
            ("ZERON_IPC_PORT".to_owned(), port.to_string()),
            ("ZERON_CHAT_ID".to_owned(), chat_id.to_owned()),
            ("ZERON_DEVICE_ID".to_owned(), self.device_id.clone()),
        ]
        .into_iter()
        .collect();
        if side_chats_only {
            env.insert(
                zeron_proto::McpServer::SIDE_CHATS_ONLY_ENV.to_owned(),
                "1".to_owned(),
            );
        }
        if let Some(path) = lock(&self.ipc_token_file).as_ref() {
            env.insert(
                "ZERON_IPC_TOKEN_FILE".into(),
                path.to_string_lossy().into_owned(),
            );
        }
        Some(zeron_proto::McpServer {
            name: "zeron".into(),
            command,
            args: vec!["mcp".into()],
            env,
        })
    }

    fn workspace(&self) -> Option<crate::workspace_host::WorkspaceHost> {
        self.doc_host().and_then(|host| host.workspace().cloned())
    }

    /// Sidebar freshness: push a message-persist preview into the chat's workspace row.
    fn note_message(&self, chat_id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(ws) = self.workspace() {
            ws.note_message(chat_id, text);
        }
    }

    /// Record the chat's harness-native session id (and its cwd): live-process
    /// cache plus the durable workspace chat row — the row is what survives an
    /// engine restart (zeron sessions.ts:1039).
    fn remember_harness_session(
        &self,
        chat_id: &str,
        session_id: &str,
        cwd: &str,
        harness: HarnessId,
    ) {
        if session_id.is_empty() {
            return;
        }
        lock(&self.harness_sessions).insert(
            chat_id.to_string(),
            HarnessSessionRef {
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
                harness: Some(harness),
            },
        );
        if let Some(ws) = self.workspace() {
            ws.set_chat_harness_session(chat_id, session_id, cwd, Some(harness));
        }
    }

    // NB: there is deliberately no `forget_harness_session` anymore. The old
    // tombstone fired on "run died before SessionStarted", which — since the
    // ACP conversion made stale ids a harness-internal fallback — only ever
    // meant a child STARTUP failure, and permanently severed good
    // conversations (user incident 2026-08-13). A truly stale id simply
    // yields a fresh session whose SessionStarted overwrites the row.

    /// The session id to resume for a run in `chat_id` launching from `cwd`
    /// (zeron sessions.ts:736, looked up on every dispatch):
    /// live-process cache → workspace chat row → journal scan (the crash path
    /// where the debounced row write never landed — SessionStarted/Done events
    /// are journaled per event, flushed immediately). Cwd-gated throughout:
    /// harness session stores are keyed by cwd, so a session created elsewhere
    /// never rides `--resume`. An empty stored id is the explicit tombstone —
    /// no resume, no falling through to staler sources.
    fn resume_for(&self, chat_id: &str, cwd: &str, harness: HarnessId) -> Option<String> {
        // The same directory, however it is spelled: an agent reports where
        // it runs with symlinks resolved (`/private/var/…` for `/var/…` on
        // macOS), and until a turn's Done restores the request's spelling a
        // mid-turn crash leaves that one stored — a string compare then
        // started the resumed run in a fresh session, its history gone.
        let cwd_ok = |session_cwd: &str| {
            session_cwd.is_empty()
                || session_cwd == cwd
                || std::fs::canonicalize(session_cwd)
                    .is_ok_and(|a| std::fs::canonicalize(cwd).is_ok_and(|b| a == b))
        };
        if let Some(known) = lock(&self.harness_sessions).get(chat_id).cloned()
            && let Some(owner) = known.harness
        {
            return (!known.session_id.is_empty() && cwd_ok(&known.cwd) && owner == harness)
                .then_some(known.session_id);
        }
        if let Some(ws) = self.workspace()
            && let Some(stored) = ws.chat_harness_session(chat_id)
        {
            if stored.session_id.is_empty() {
                return None;
            }
            let owner = stored.harness.or_else(|| {
                let (id, session_cwd, owner) = self.journal_harness_session(chat_id)?;
                let owner = owner?;
                self.remember_harness_session(chat_id, &id, &session_cwd, owner);
                Some(owner)
            });
            let cwd_matches = cwd_ok(stored.cwd.as_deref().unwrap_or(""));
            if let Some(owner) = owner {
                lock(&self.harness_sessions).insert(
                    chat_id.to_string(),
                    HarnessSessionRef {
                        session_id: stored.session_id.clone(),
                        cwd: stored.cwd.clone().unwrap_or_default(),
                        harness: Some(owner),
                    },
                );
            }
            return (cwd_matches && owner.is_none_or(|owner| owner == harness))
                .then_some(stored.session_id);
        }
        let (session_id, session_cwd, owner) = self.journal_harness_session(chat_id)?;
        // Cache the journal hit (memory + row) so later dispatches skip the scan.
        if let Some(owner) = owner {
            self.remember_harness_session(chat_id, &session_id, &session_cwd, owner);
        }
        (cwd_ok(&session_cwd) && owner.is_none_or(|owner| owner == harness)).then_some(session_id)
    }

    fn owned_session(&self, chat_id: &str) -> Option<HarnessSessionRef> {
        if let Some(known) = lock(&self.harness_sessions).get(chat_id).cloned()
            && known.harness.is_some()
        {
            return Some(known);
        }
        if let Some(ws) = self.workspace()
            && let Some(stored) = ws.chat_harness_session(chat_id)
        {
            if stored.session_id.is_empty() {
                return None;
            }
            let harness = stored
                .harness
                .or_else(|| self.journal_harness_session(chat_id)?.2)?;
            return Some(HarnessSessionRef {
                session_id: stored.session_id,
                cwd: stored.cwd.unwrap_or_default(),
                harness: Some(harness),
            });
        }
        let (session_id, cwd, harness) = self.journal_harness_session(chat_id)?;
        Some(HarnessSessionRef {
            session_id,
            cwd,
            harness: Some(harness?),
        })
    }

    fn own_provider_session(doc: &SessionDoc, harness: HarnessId, cwd: &str) -> Option<String> {
        if !doc.provider_switched() {
            return None;
        }
        doc.provider_session(harness.as_str())
            .filter(|mark| !mark.session_id.is_empty() && session_cwd_matches(&mark.cwd, cwd))
            .map(|mark| mark.session_id)
    }

    fn note_provider_session(
        doc: &SessionDoc,
        harness: HarnessId,
        session_id: &str,
        cwd: &str,
        through: Option<&str>,
    ) {
        if session_id.is_empty() || !doc.provider_switched() {
            return;
        }
        let entries = doc.read_entries().unwrap_or_default();
        let position = |id: &str| entries.iter().position(|entry| entry.id == id);
        let seen = match through {
            Some(id) => position(id).map(|_| id.to_owned()),
            None => entries.last().map(|entry| entry.id.clone()),
        };
        let previous = doc
            .provider_session(harness.as_str())
            .filter(|mark| mark.session_id == session_id);
        let seen = match (previous.and_then(|mark| mark.seen), seen) {
            (Some(old), Some(new)) if position(&old) > position(&new) => Some(old),
            (old, new) => new.or(old),
        };
        let mark = zeron_doc::ProviderSessionMark {
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
            seen,
        };
        if let Err(err) = doc.set_provider_session(harness.as_str(), &mark) {
            tracing::warn!(error = %err, "provider session record write failed");
        }
    }

    /// The last harness session id named anywhere in the chat's journal, with
    /// the cwd of the `SessionStarted` that governs it. `Done.session_id`
    /// inherits the cwd of the most recent `SessionStarted` (same run).
    ///
    /// Walks the journal newest-first and stops at the answer, so boot recovery
    /// doesn't load whole multi-MB journals: the newest non-empty id wins, and a
    /// `Done` id takes the cwd of the nearest earlier `SessionStarted` (any id).
    fn journal_harness_session(
        &self,
        chat_id: &str,
    ) -> Option<(String, String, Option<HarnessId>)> {
        let warn = |err: crate::run_journal::JournalError| {
            tracing::warn!(chat = %chat_id, error = %err, "journal scan for harness session failed");
        };
        let events = match self.journal.events_rev(chat_id) {
            Ok(events) => events,
            Err(err) => {
                warn(err);
                return None;
            }
        };
        // A `Done` id seen first, still waiting for its run's `SessionStarted` cwd.
        let mut done_id: Option<String> = None;
        for item in events {
            let (seq, event) = match item {
                Ok(item) => item,
                Err(err) => {
                    warn(err);
                    return None;
                }
            };
            if seq == 0 {
                continue; // `replay(_, 0)` (the old full scan) never yielded seq 0
            }
            match event {
                AgentEvent::SessionStarted {
                    session_id,
                    cwd,
                    harness,
                    ..
                } => {
                    if let Some(id) = done_id {
                        return Some((id, cwd, Some(harness)));
                    }
                    if !session_id.is_empty() {
                        return Some((session_id, cwd, Some(harness)));
                    }
                }
                AgentEvent::Done {
                    session_id: Some(session_id),
                    ..
                } if done_id.is_none() && !session_id.is_empty() => {
                    done_id = Some(session_id);
                }
                _ => {}
            }
        }
        done_id.map(|id| (id, String::new(), None))
    }

    /// Actual driver exit only: close admission and claim every accepted steer
    /// under the same runs → ledger order used by mailbox admission.
    fn retire_run(&self, chat_id: &str, run_id: &str) -> Vec<RoutedSteer> {
        let runs = lock(&self.runs);
        let Some(handle) = runs.get(chat_id).filter(|h| h.run_id == run_id) else {
            return Vec::new();
        };
        let mut pending = lock(&handle.routed_steers);
        handle.retiring.store(true, std::sync::atomic::Ordering::Release);
        std::mem::take(&mut *pending).into()
    }

    fn remove_run(&self, chat_id: &str, run_id: &str) {
        let removed = {
            let mut runs = lock(&self.runs);
            if runs.get(chat_id).is_some_and(|h| h.run_id == run_id) {
                runs.remove(chat_id)
            } else {
                None
            }
        };
        self.finish_turn(run_id);
        if let Some(run) = &removed {
            // Stream termination/start failure must also end silent retain subscriptions.
            run.interrupt_token.cancel();
        }
        if let Some(route) = removed.and_then(|run| run.inference)
            && let Some(crew) = self.crew.get()
        {
            crew.retire_inference(&route);
        }
    }
}

// ── run task ────────────────────────────────────────────────────────────────

/// Which provider session a side chat's prompt goes to.
#[derive(Clone, Copy)]
enum ProviderSession<'a> {
    /// A new provider session: it holds none of the transcript.
    Fresh,
    /// An existing one (id when known): it holds the fork's own turns.
    Continued(Option<&'a str>),
}

/// A run's provider session now holds the fork history its own prompt
/// carried (steers record theirs when confirmed, not here).
fn note_fork_history_session(doc: &SessionDoc, carried: bool, session_id: &str) {
    if carried && !session_id.is_empty() {
        let _ = doc.set_fork_history_session(session_id);
    }
}

fn session_cwd_matches(session_cwd: &str, cwd: &str) -> bool {
    session_cwd.is_empty()
        || session_cwd == cwd
        || std::fs::canonicalize(session_cwd)
            .is_ok_and(|a| std::fs::canonicalize(cwd).is_ok_and(|b| a == b))
}

/// Ceiling on the prior conversation replayed into a provider session that
/// lacks it. Characters of serialized history, not tokens: roughly 20k
/// tokens, small enough to leave every supported model room to work.
const HISTORY_REPLAY_CHARS: usize = 80_000;
/// One message's replayed text is cut past this, keeping its head.
const HISTORY_MESSAGE_CHARS: usize = 8_000;
/// A replayed tool call keeps only this much of its output.
const HISTORY_TOOL_OUTPUT_CHARS: usize = 600;

/// `text` cut to `max` characters on a char boundary, with a marker when cut.
fn clip_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}… [truncated]", &text[..cut]),
        None => text.to_owned(),
    }
}

impl Inner {
    /// Record the seam when a chat moves to another provider and preserve the
    /// outgoing provider's resumable session for a later switch back.
    fn note_provider_switch(&self, chat_id: &str, harness_id: HarnessId, doc: &ChatDocHandle) {
        let entries = doc.doc().read_entries().unwrap_or_default();
        if entries.is_empty() {
            return;
        }
        let latest = 'entries: {
            for entry in entries.iter().rev() {
                for part in &entry.parts {
                    match part {
                        MessagePart::Switch { to_harness, .. } => {
                            break 'entries Some(
                                to_harness.as_deref().and_then(HarnessId::from_slug),
                            );
                        }
                        MessagePart::Fork { .. } => break 'entries None,
                        _ => {}
                    }
                }
            }
            None
        };
        let owned = || self.owned_session(chat_id);
        let previous = match latest {
            Some(to) => to,
            None => owned().and_then(|known| known.harness),
        };
        let Some(previous) = previous else {
            return;
        };
        if previous == harness_id {
            return;
        }
        if !doc.doc().provider_switched()
            && let Some(known) = owned().filter(|known| known.harness == Some(previous))
            && !known.session_id.is_empty()
        {
            let mark = zeron_doc::ProviderSessionMark {
                session_id: known.session_id,
                cwd: known.cwd,
                seen: entries
                    .iter()
                    .rposition(|entry| entry.role != MessageRole::User)
                    .map(|index| entries[index].id.clone()),
            };
            if let Err(err) = doc.doc().set_provider_session(previous.as_str(), &mark) {
                tracing::warn!(chat = %chat_id, error = %err, "provider session record write failed");
            }
        }
        let label = |id: HarnessId| {
            self.registry
                .descriptors()
                .into_iter()
                .find(|descriptor| descriptor.id == id)
                .map_or_else(|| id.as_str().to_owned(), |descriptor| descriptor.name)
        };
        let created_at = now_ms();
        let id = format!("switch:{created_at}");
        let unanswered = entries
            .iter()
            .rev()
            .take_while(|entry| entry.role == MessageRole::User)
            .count();
        let inserted = doc.doc().insert_message_before_last(
            unanswered,
            &SessionMessageEntry {
                id: id.clone(),
                role: MessageRole::System,
                parts: vec![MessagePart::Switch {
                    id,
                    from: label(previous),
                    to: label(harness_id),
                    from_harness: Some(previous.as_str().to_owned()),
                    to_harness: Some(harness_id.as_str().to_owned()),
                }],
                created_at,
                device_id: self.device_id.clone(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            },
        );
        match inserted.and_then(|_| doc.doc().mark_provider_switched()) {
            Ok(()) => tracing::info!(chat = %chat_id, from = previous.as_str(), to = harness_id.as_str(), "chat switched provider"),
            Err(err) => tracing::warn!(chat = %chat_id, error = %err, "provider switch marker write failed"),
        }
    }

    /// `prompt` with the conversation a side chat's provider session lacks in
    /// front of it, or `None` when it lacks nothing. A fresh session gets the
    /// whole transcript (`current` excluded); a continued one only the fork's
    /// copied history before the seam, until that went out to that session.
    /// Native commands are never wrapped: providers route a leading `/name`.
    fn fork_history_prompt(
        &self,
        chat_id: &str,
        doc: &SessionDoc,
        harness_id: HarnessId,
        prompt: &str,
        current: Option<&str>,
        provider: ProviderSession<'_>,
    ) -> Option<String> {
        let side_chat = self
            .workspace()
            .and_then(|ws| ws.chat(chat_id).ok().flatten())
            .is_some_and(|chat| chat.parent_chat_id.is_some());
        let switched = doc.provider_switched();
        if native_command(prompt, harness_id) || !(side_chat || switched) {
            return None;
        }
        let mut entries = doc.read_entries().unwrap_or_default();
        let watermark = match provider {
            ProviderSession::Continued(session) if switched => doc
                .provider_session(harness_id.as_str())
                .filter(|mark| session.is_none_or(|id| id == mark.session_id))
                .map(|mark| mark.seen),
            _ => None,
        };
        if let Some(seen) = &watermark {
            let start = seen
                .as_deref()
                .and_then(|seen| entries.iter().position(|entry| entry.id == seen))
                .map_or(0, |index| index + 1);
            entries.drain(..start);
        }
        entries.retain(|entry| Some(entry.id.as_str()) != current);
        let end = match provider {
            ProviderSession::Fresh => entries.len(),
            ProviderSession::Continued(_) if watermark.is_some() => entries.len(),
            ProviderSession::Continued(session) => {
                let delivered = doc.fork_history_session();
                if session.is_some() && delivered.as_deref() == session {
                    return None;
                }
                entries.iter().rposition(|entry| {
                    entry.parts.iter().any(|part| {
                        matches!(
                            part,
                            zeron_doc::MessagePart::Fork { .. }
                                | zeron_doc::MessagePart::Switch { .. }
                        )
                    })
                })?
            }
        };
        let history: Vec<_> = entries[..end]
            .iter()
            .map(|entry| {
                let text = entry
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        zeron_doc::MessagePart::Text { text, .. } => Some(text.clone()),
                        zeron_doc::MessagePart::Tool { call, output, .. } => Some(format!(
                            "Tool: {}\n{}",
                            serde_json::to_string(call).unwrap_or_default(),
                            clip_chars(
                                output.as_deref().unwrap_or_default(),
                                HISTORY_TOOL_OUTPUT_CHARS
                            )
                        )),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (entry.role, clip_chars(&text, HISTORY_MESSAGE_CHARS))
            })
            .filter(|(_, text)| !text.is_empty())
            .map(|(role, text)| serde_json::json!({ "role": role, "text": text }))
            .collect();
        // Newest messages win the budget: what was just said matters most.
        let mut budget = HISTORY_REPLAY_CHARS;
        let mut kept = history
            .iter()
            .rev()
            .take_while(|message| {
                let cost = message.to_string().len();
                let fits = cost <= budget;
                budget = budget.saturating_sub(cost);
                fits
            })
            .count();
        // A single message over the whole budget still goes out (clipped).
        kept = kept.max(1.min(history.len()));
        let omitted = history.len() - kept;
        let history = &history[omitted..];
        if history.is_empty() {
            return None;
        }
        let intro = if watermark.is_some() {
            "While you were away, this conversation continued with a different AI provider. The messages you missed are below; answer the final request as the continuation of the whole conversation. Tool calls listed below ran on the other provider — do not assume their state is still live."
        } else if switched {
            "This conversation was started with a different AI provider and you are now taking over. The earlier messages are below as context; answer the final request as the continuation of that conversation. Tool calls listed below ran on the previous provider — do not assume their state is still live."
        } else {
            "Continue this side conversation using the following prior conversation as context."
        };
        let omitted_note = if omitted > 0 {
            format!("({omitted} earlier messages omitted for length.)\n")
        } else {
            String::new()
        };
        Some(format!(
            "{intro}\n{omitted_note}<conversation>\n{}\n</conversation>\n\n{}",
            serde_json::to_string(history).unwrap_or_default(),
            prompt
        ))
    }
}

/// Whether the provider routes this prompt as a native command: its delivered
/// text leads with `/name` (Codex `command_request`, OpenCode commands, Claude
/// slash commands). Anything put in front of it would make it model input.
const SHUTDOWN_NOTE: &str = "Run interrupted by engine shutdown";

fn native_command(prompt: &str, harness: HarnessId) -> bool {
    use zeron_proto::invocation::{self, Invocation};
    let selected = invocation::invocation_links(prompt)
        .into_iter()
        .next()
        .is_some_and(|(range, selection)| {
            prompt[..range.start].trim().is_empty()
                && match selection {
                    Invocation::Command { .. } => true,
                    Invocation::Skill { command: Some(command), .. } => command.harness == harness,
                    Invocation::Skill { .. } => false,
                }
        });
    if selected {
        return true;
    }
    let delivered = if harness == HarnessId::Codex {
        invocation::invocation_prompt(prompt)
    } else {
        invocation::harness_prompt(prompt, harness)
    };
    invocation::leading_command(&delivered)
        .is_some_and(|(name, _)| invocation::valid_skill_command_name(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pickup {
    Resume,
    AwaitingAnswer,
    Exhausted,
    Skip,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TurnTail {
    reached_provider: bool,
    awaiting_input: bool,
    finished: bool,
}

fn turn_tail(events: impl Iterator<Item = AgentEvent>, closed: bool) -> TurnTail {
    let mut tail = TurnTail { finished: true, ..TurnTail::default() };
    let mut answered = std::collections::HashSet::new();
    for (index, event) in events.enumerate() {
        match event {
            AgentEvent::Done { .. } if index == 0 && closed => continue,
            AgentEvent::Done { .. } => break,
            AgentEvent::Subagent { .. } => continue,
            AgentEvent::InputResolved { request_id, .. } => {
                answered.insert(request_id);
            }
            AgentEvent::InputRequested { request_id, .. } => {
                tail.reached_provider = true;
                tail.awaiting_input |= !answered.contains(&request_id);
            }
            AgentEvent::TextDelta { text } | AgentEvent::ReasoningDelta { text } => {
                tail.reached_provider |= !text.trim().is_empty();
            }
            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::GeneratedImage { .. }
            | AgentEvent::Visual { .. }
            | AgentEvent::AssistantMessageCompleted { .. }
            | AgentEvent::Steered { .. } => tail.reached_provider = true,
            _ => {}
        }
        tail.finished = false;
    }
    tail
}

fn answered_in(entries: &[SessionMessageEntry], user_id: &str) -> bool {
    entries
        .iter()
        .skip_while(|entry| entry.id != user_id)
        .skip(1)
        .any(|entry| {
            entry.role == MessageRole::Assistant
                && entry.parts.iter().any(|part| match part {
                    MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => {
                        !text.trim().is_empty()
                    }
                    MessagePart::Tool { .. } | MessagePart::Input { .. } => true,
                    _ => false,
                })
        })
}

fn revival_prompt(original: &str) -> String {
    format!(
        "Your previous turn was cut off before it finished (Crew restarted). Continue where you \
         left off. Anything you already completed, such as edits, commands or commits, is done: \
         check before repeating it. The request is below for reference; if it is not in your \
         history yet, handle it now.\n\n<request>\n{original}\n</request>"
    )
}

/// A turn is in flight: streaming, or parked on a question it is still owed an
/// answer to. Notably NOT a persistent session that has parked between turns —
/// that holds a warm child with nothing outstanding.
fn is_active(session: &Session) -> bool {
    matches!(
        session.status,
        SessionStatus::Working | SessionStatus::AwaitingInput
    )
}

// ── subagent docs ───────────────────────────────────────────────────────────

/// The per-subagent doc id: `{chatId}--sub--{suffix}`. Constrained by the
/// edge's `ID_RE` (`^[A-Za-z0-9_-]{1,128}$` — the same id names the ChatRoom
/// `chat2/{id}/ws` and the frozen blob `blob/{chatId}/{id}`): a clean, short
/// tool-use id rides verbatim; anything unclean or over budget hashes.
pub(crate) fn subagent_doc_id(chat_id: &str, tool_use_id: &str) -> String {
    let clean = tool_use_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let budget = 128usize.saturating_sub(chat_id.len() + "--sub--".len());
    if clean && !tool_use_id.is_empty() && tool_use_id.len() <= budget {
        return format!("{chat_id}--sub--{tool_use_id}");
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(tool_use_id.as_bytes());
    let mut hex = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    format!("{chat_id}--sub--{hex}")
}

/// Spawn gate: whether tagged traffic for `id` gets a subagent doc, and with
/// it a unit on the badge. Only a spawn chip of THIS chat does, one this run
/// folded or one already in the doc: those are the rows the lanes card lists.
/// Claude also tags the agents a subagent starts with their own call ids,
/// chips of that subagent's doc; sinks for them badged chats over an empty
/// card (#999). Traffic for any other id stays in the run journal.
fn is_chat_spawn(id: &str, folded: &[MessagePart], doc: &SessionDoc) -> bool {
    folded.iter().any(|part| {
        matches!(part, MessagePart::Tool { id: part_id, call, .. }
            if part_id == id && call.is_subagent_spawn())
    }) || doc.has_subagent_spawn(id)
}

/// Test-only instrumentation: how often `drive_run`'s coalesced commit branch
/// fires. The sink-dirty regression test proves the branch ticks once per
/// commit window instead of spinning on a deadline left in the past.
/// Thread-local so parallel tests' runs can't bleed into the count: the
/// test's current-thread runtime runs its spawned `drive_run` on its own
/// thread.
#[cfg(test)]
thread_local! {
    static FLUSH_TICKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A live subagent transcript sink: its own doc (opened by id — the room
/// `chat2/{docId}/ws` dials automatically, so viewers sync it like a chat),
/// one streaming assistant entry folded from the tagged events. The held
/// doc Arc pins the doc warm for the LRU while the subagent runs.
struct SubagentSink {
    doc_id: String,
    doc: crate::doc_host::DocWriter,
    entry_id: String,
    started_at: i64,
    entry_index: Option<usize>,
    written: Vec<MessagePart>,
    folded: Vec<MessagePart>,
    dirty: bool,
}

impl SubagentSink {
    fn flush(&mut self, device_id: &str) {
        if !self.dirty {
            return;
        }
        // An empty fold still counts as flushed: leaving dirty set would hold
        // the run loop's coalesced-commit guard open on a deadline already in
        // the past, spinning it until the subagent emits real content.
        self.dirty = false;
        if self.folded.is_empty() {
            return;
        }
        let rendered = render_parts(&self.folded);
        let result = match self.entry_index {
            Some(ix) => {
                let mut w = SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written));
                let r = w.sync(&rendered);
                let (ix, written) = w.into_state();
                self.entry_index = Some(ix);
                self.written = written;
                r
            }
            None => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(mut w) => {
                        let r = w.sync(&rendered);
                        let (ix, written) = w.into_state();
                        self.entry_index = Some(ix);
                        self.written = written;
                        r
                    }
                    Err(e) => Err(e),
                }
            }
        };
        if let Err(err) = result {
            // Fail soft: a broken subagent doc degrades to chip-only, never
            // errors the chat.
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent sink flush failed");
        }
    }

    /// A parent→subagent steer ([`AgentEvent::UserMessage`], tagged): close
    /// the open assistant segment (it reads Complete above the steer), write
    /// the steer as its own USER entry, and reset so the next tagged delta
    /// opens a fresh assistant entry below it — the subagent transcript then
    /// reads like any steered chat.
    fn push_user(&mut self, device_id: &str, text: &str) {
        let rendered = render_parts(&self.folded);
        let closed = match self.entry_index.take() {
            Some(ix) => SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written))
                .finish(&rendered, MessageStatus::Complete),
            None if !self.folded.is_empty() => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(w) => w.finish(&rendered, MessageStatus::Complete),
                    Err(e) => Err(e),
                }
            }
            None => Ok(()),
        };
        if let Err(err) = closed {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent segment close failed");
        }
        let entry = SessionMessageEntry {
            id: new_id(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_owned(),
            }],
            created_at: now_ms(),
            device_id: device_id.to_owned(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        if let Err(err) = self.doc.push_message(&entry) {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent steer write failed");
        }
        self.entry_id = new_id();
        self.started_at = now_ms();
        self.written = Vec::new();
        self.folded = Vec::new();
        self.dirty = false;
    }

    /// Final flush + entry finalize; returns the transcript JSON for the
    /// frozen blob (entries as the client renders them).
    fn finish(mut self, device_id: &str, status: MessageStatus) -> Option<String> {
        let rendered = render_parts(&self.folded);
        let finished = match self.entry_index {
            Some(ix) => SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written))
                .finish(&rendered, status),
            None if !self.folded.is_empty() => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(w) => w.finish(&rendered, status),
                    Err(e) => Err(e),
                }
            }
            None => Ok(()),
        };
        if let Err(err) = finished {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent sink finish failed");
        }
        let entries = zeron_doc::join_continuation_entries(self.doc.read_entries().ok()?);
        serde_json::to_string(&entries).ok()
    }
}

/// The parent-chip refresh a tagged event implies, for the in-place (parked)
/// path — LIFECYCLE ONLY, mirroring the fold (live tails were rejected:
/// per-delta chip rewrites grew the parent doc for the whole run).
fn subagent_chip_update(event: &AgentEvent) -> Option<&'static str> {
    match event {
        AgentEvent::Done { status, .. } => Some(match status {
            DoneStatus::Errored => "failed",
            _ => "done",
        }),
        _ => Some("running"),
    }
}

/// Apply the render-parts privacy policy: strip heavy/sensitive tool inputs before doc
/// entry. Full inputs live only in the local run journal.
fn render_parts(parts: &[MessagePart]) -> Vec<MessagePart> {
    parts
        .iter()
        .map(|part| match part {
            MessagePart::Tool {
                id,
                call,
                is_error,
                resolved,
                output,
                diff,
                output_ref,
                output_bytes,
                diff_ref,
                diff_stats,
                subagent_ref,
                subagent_status,
                subagent_tail,
            } => MessagePart::Tool {
                id: id.clone(),
                call: sanitize_tool_call(call),
                is_error: *is_error,
                resolved: *resolved,
                // Output summaries, diff stats, and sidecar refs are
                // deliberately kept: unlike raw tool inputs they are the
                // transcript's record of what happened, and the strip already
                // bounded them (docs/chat2-sync.md A1).
                output: output.clone(),
                diff: diff.clone(),
                output_ref: output_ref.clone(),
                output_bytes: *output_bytes,
                diff_ref: diff_ref.clone(),
                diff_stats: diff_stats.clone(),
                subagent_ref: subagent_ref.clone(),
                subagent_status: *subagent_status,
                subagent_tail: subagent_tail.clone(),
            },
            other => other.clone(),
        })
        .collect()
}

/// The persisted assistant text of a folded segment (workspace preview source).
fn folded_text(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sync_segment<'a>(
    doc: &'a SessionDoc,
    writer: &mut Option<SegmentWriter<'a>>,
    entry_id: &str,
    device_id: &str,
    started_at: i64,
    folded: &[MessagePart],
) -> Result<(), DocError> {
    if folded.is_empty() {
        return Ok(());
    }
    let rendered = render_parts(folded);
    if writer.is_none() {
        *writer = Some(SegmentWriter::begin(doc, entry_id, device_id, started_at)?);
    }
    if let Some(w) = writer.as_mut() {
        w.sync(&rendered)?;
    }
    Ok(())
}

fn finish_segment<'a>(
    doc: &'a SessionDoc,
    writer: Option<SegmentWriter<'a>>,
    entry_id: &str,
    device_id: &str,
    started_at: i64,
    folded: &[MessagePart],
    status: MessageStatus,
) -> Result<(), DocError> {
    let rendered = render_parts(folded);
    match writer {
        Some(w) => w.finish(&rendered, status),
        None if !folded.is_empty() => {
            SegmentWriter::begin(doc, entry_id, device_id, started_at)?.finish(&rendered, status)
        }
        None => Ok(()),
    }
}

/// Resume bookkeeping for one run task: which user entry the run answers (so
/// the startup-crash retry re-dispatches idempotently against the same doc
/// entry), whether `dispatch` injected the resume id itself (only
/// engine-injected resumes retry — a caller-specified resume fails loudly),
/// and whether this run already IS the retry (one attempt only).
struct RunResumeState {
    idle: bool,
    voice_active: Arc<VoiceActivity>,
    user_message_id: String,
    resume_injected: bool,
    startup_retry: bool,
    revival: Option<Revival>,
    stops_seen: u64,
    fork_history_sent: Arc<std::sync::atomic::AtomicBool>,
    /// Set once the harness stream is open (see [`RunHandle::started`]).
    started: Arc<std::sync::atomic::AtomicBool>,
}

fn cursor_unstarted_history(
    doc: &SessionDoc,
    current_id: &str,
    prompt: &str,
    has_session: bool,
) -> Result<String, DocError> {
    // Convert each message before JSON encoding. Rewriting canonical chips in
    // the encoded envelope can introduce unescaped quotes or newlines and can
    // cause Cursor's current message to be converted twice.
    let prompt = zeron_proto::invocation::harness_prompt(prompt, HarnessId::Cursor);
    let entries = doc.read_entries()?;
    let preceding: Vec<_> = entries
        .iter()
        .take_while(|entry| entry.id != current_id)
        .collect();
    // With an existing session, only bridge the tail whose assistant never
    // produced content. A process can die before writing its SDK-side receipt,
    // even though an older session ID still exists.
    let after = if has_session {
        preceding
            .iter()
            .rposition(|entry| {
                entry.role == MessageRole::Assistant
                    && entry.parts.iter().any(|part| match part {
                        MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => {
                            !text.is_empty()
                        }
                        MessagePart::Tool { .. } | MessagePart::Input { .. } => true,
                        _ => false,
                    })
            })
            .map_or(0, |i| i + 1)
    } else {
        0
    };
    let previous: Vec<String> = preceding
        .iter()
        .skip(after)
        .filter(|entry| entry.role == MessageRole::User)
        .map(|entry| {
            entry
                .parts
                .iter()
                .filter_map(|part| match part {
                    MessagePart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .map(|text| zeron_proto::invocation::harness_prompt(&text, HarnessId::Cursor))
        .collect();
    if previous.is_empty() {
        return Ok(prompt);
    }
    Ok(format!(
        "The preceding user messages may not have reached a Cursor checkpoint before startup stopped. Retain this JSON as conversation history; do not rerun prior tools or side effects. Respond to the current message.\n{}",
        serde_json::json!({"previousUserMessages": previous, "currentUserMessage": prompt})
    ))
}

#[allow(clippy::too_many_arguments)]
async fn drive_run(
    inner: Arc<Inner>,
    chat_id: String,
    run_id: String,
    harness: Arc<dyn Harness>,
    mut request: RunRequest,
    doc: crate::doc_host::DocWriter,
    mut controls: RunControls,
    mut engine_rx: mpsc::UnboundedReceiver<AgentEvent>,
    mut cancel_rx: watch::Receiver<bool>,
    mut supervision_rx: watch::Receiver<usize>,
    resume_state: RunResumeState,
    turn: TurnControl,
    _provider_run: ProviderRun,
    inference_changed: Option<Arc<tokio::sync::Notify>>,
    side_chats_only: bool,
) {
    let device_id = inner.device_id.clone();
    // Captured for post-run auto-titling (the request moves into the harness).
    let harness_id = harness.id();
    let user_prompt = request.prompt.clone();
    let run_cwd = request.cwd.clone();
    let mut run_session: Option<(String, String)> = None;
    if request.resume.is_none() {
        let _ = doc.clear_context_usage();
    }
    // The host stamps its own MCP server onto every run it drives, so the
    // agent can spawn and talk to side chats through the engine it runs in.
    if request.mcp.is_none() {
        request.mcp = inner.zeron_mcp(&chat_id, side_chats_only);
    }
    // Kept whole for the startup-crash retry (same user entry; dispatch
    // re-injects the stored resume id). Option so the retry branch (inside
    // the event loop) can take ownership.
    let mut retry_request = Some(RunRequest {
        resume: None,
        ..request.clone()
    });
    if let Some(continuation) = resume_state
        .revival
        .as_ref()
        .and_then(|revival| revival.continuation.clone())
    {
        request.prompt = continuation;
    }
    // A side chat owns a fresh provider session. Bootstrap it from the frozen
    // conversation, never resume (and mutate) the parent's provider session.
    let mut carried_history = false;
    let provider = match request.resume.as_deref() {
        None => ProviderSession::Fresh,
        resumed => ProviderSession::Continued(resumed),
    };
    if !resume_state.idle
        && let Some(prompt) = inner.fork_history_prompt(
            &chat_id,
            &doc,
            harness_id,
            &request.prompt,
            Some(&resume_state.user_message_id),
            provider,
        )
    {
        request.prompt = prompt;
        carried_history = true;
        resume_state
            .fork_history_sent
            .store(true, std::sync::atomic::Ordering::Release);
    }
    // Startup can stop before the SDK saves user text, with no new session
    // ID or receipt. Bridge that unacknowledged tail from our transcript;
    // a fresh session needs all prior user text, not just the latest tail.
    let prepared = if harness_id == HarnessId::Cursor {
        cursor_unstarted_history(
            &doc,
            &resume_state.user_message_id,
            &request.prompt,
            request.resume.is_some(),
        )
        .map(|prompt| request.prompt = prompt)
        .map_err(|e| zeron_harness::HarnessError::Protocol(e.to_string()))
    } else {
        Ok(())
    };
    // Waiting here keeps dispatch and the shared queue-flush watcher responsive.
    // The pending-update marker still orders new subprocesses after installation.
    // Share the lease with the adapter so child cleanup outlives this event loop.
    let interrupt = controls.interrupt.clone();
    let mut _execution_lease = None;
    let started = match prepared {
        Ok(()) => {
            let lease = tokio::select! {
                biased;
                _ = controls.interrupt.cancelled() => None,
                lease = inner.registry.execution_lease(harness_id) => Some(Arc::new(lease)),
            };
            if let Some(lease) = lease {
                _execution_lease = Some(lease.clone());
                controls.execution_lease = Some(lease);
                if let Some(listener) = inner.turn_listener.get() {
                    listener(&chat_id, &request.cwd);
                }
                let mut wire_request = request;
                if !matches!(harness_id, HarnessId::Cursor | HarnessId::Opencode) {
                    wire_request.prompt =
                        zeron_proto::invocation::harness_prompt(&wire_request.prompt, harness_id);
                }
                wire_request.env.insert(
                    zeron_proto::ORIGIN_ENV.into(),
                    zeron_proto::chat_origin(&chat_id),
                );
                let environment = inner
                    .registry
                    .prepare_run(harness_id, &mut wire_request)
                    .await;
                let admission = match environment {
                    Err(message) => Err(zeron_harness::HarnessError::NotSignedIn(message)),
                    Ok(()) if resume_state.idle => Ok(()),
                    Ok(()) => {
                        let runs = lock(&inner.runs);
                        runs.get(&chat_id).filter(|h| h.run_id == run_id)
                            .ok_or_else(|| zeron_harness::HarnessError::Protocol(
                                "Crew turn disappeared during startup".into(),
                            ))
                            .and_then(|handle| inner.start_turn(&chat_id, handle)
                                .map_err(|error| zeron_harness::HarnessError::Protocol(error.to_string())))
                    }
                };
                match admission {
                    Err(error) => Err(error),
                    Ok(()) => {
                        tokio::select! {
                            biased;
                            _ = interrupt.cancelled() => Ok(futures::stream::once(async {
                                Ok(AgentEvent::Done {
                                    status: DoneStatus::Interrupted,
                                    result: None,
                                    error: None,
                                    session_id: None,
                                })
                            }).boxed()),
                            result = async {
                                if resume_state.idle {
                                    harness.start_idle(wire_request, controls).await
                                } else {
                                    harness.run(wire_request, controls).await
                                }
                            } => result,
                        }
                    }
                }
            } else {
                Ok(futures::stream::once(async {
                    Ok(AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: None,
                    })
                })
                .boxed())
            }
        }
        Err(error) => Err(error),
    };
    let mut stream = match started {
        Ok(stream) => {
            resume_state
                .started
                .store(true, std::sync::atomic::Ordering::Release);
            stream
        }
        Err(err) => {
            let message = err.to_string();
            tracing::warn!(chat = %chat_id, harness = ?harness_id, error = %message, "run failed to start");
            // The journal alone is live-only: without an entry the transcript
            // shows "Run failed" with no reason (an OpenCode server that never
            // booted looked exactly like that).
            let parts = [MessagePart::Error {
                id: "e0".into(),
                message: message.clone(),
            }];
            if let Err(err) = finish_segment(
                &doc,
                None,
                &new_id(),
                &device_id,
                now_ms(),
                &parts,
                MessageStatus::Complete,
            ) {
                tracing::warn!(chat = %chat_id, error = %err, "start failure entry failed");
            }
            inner.publish(
                &chat_id,
                &AgentEvent::Error {
                    message: message.clone(),
                },
            );
            inner.publish(
                &chat_id,
                &AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(message),
                    session_id: None,
                },
            );
            inner.set_run_status(&chat_id, &run_id, SessionStatus::Errored, false);
            inner.remove_run(&chat_id, &run_id);
            return;
        }
    };

    let doc_ref: &SessionDoc = &doc;
    let mut folded: Vec<MessagePart> = Vec::new();
    // Every tool id this run has folded, across segment resets. Adapters
    // re-emit shape-bearing `tool_call_update`s (title/rawInput refreshes,
    // long-running completions) as full ToolCall events; once the fold has
    // reset at a steer/park boundary those ids are gone from `folded`, and
    // folding the echo would mint an orphan chip mid-text in the NEXT
    // segment — the mid-word transcript splits.
    let mut seen_tools: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_images = std::collections::HashSet::new();
    for entry in doc_ref.read_entries().unwrap_or_default() {
        for part in entry.parts {
            if let MessagePart::Image { id, .. } = part {
                seen_images.insert(format!("{chat_id}\0{id}"));
                if let Some(tool_id) = id.strip_suffix(":image") {
                    seen_tools.insert(tool_id.to_owned());
                }
            }
        }
    }
    let mut prepared_events = std::collections::VecDeque::new();
    let mut entry_id = new_id();
    let mut segment_started = now_ms();
    let mut writer: Option<SegmentWriter<'_>> = None;
    let mut dirty = false;
    let mut flush_at = tokio::time::Instant::now();
    // Set when the engine interrupts the run: the harness gets this long to end its own
    // stream (its token was cancelled); past it, a terminal Done is synthesized.
    let mut interrupt_deadline: Option<tokio::time::Instant> = None;
    let mut interrupted = false;
    let mut saw_session_started = false;
    // Liveness heartbeat: this loop RUNNING is proof the harness stream is
    // open, so freshness must not depend on events arriving. Silent stretches
    // are normal and UNBOUNDED — a long tool call, redacted thinking, an
    // agent waiting on an external process, a question parked for an hour —
    // and each starved the UI's 45s staleness gate in turn (working strip /
    // AwaitingInput dot vanishing mid-run, both user-reported). No stall
    // timeout here by design (a first port was rejected — agents may
    // legitimately be quiet for >10min): a live child means Working, dying
    // paths each carry their own error, and engine death stops these ticks
    // so the gate still catches real crashes. touch_session throttles at 10s.
    let mut live_heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    live_heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // PERSISTENT SESSION (zeron runsBySession): a completed turn on a
    // steerable harness parks here instead of ending the run — the child and
    // its steering mailbox stay warm, and the next user message (dispatch
    // routes into a live run) starts the next turn with zero respawn/resume
    // latency. `Some(when)` = idle since then; the 30-min reaper below ends
    // a session nobody comes back to (zeron SESSION_IDLE_MS). A voice
    // orchestrator holds Codex and its MCP servers (~200 MB) for calls only,
    // so it goes after 5 minutes. A call holds the reaper off, and the window
    // restarts at hang-up. `ZERON_SESSION_IDLE_MS` overrides both (tests).
    let session_idle = std::env::var("ZERON_SESSION_IDLE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_secs(
            if zeron_proto::voice::is_orchestrator_chat(&chat_id) {
                5 * 60
            } else {
                30 * 60
            },
        ));
    // A live subagent stretches the window, but never unbounded: every
    // driver's subagent terminal is a best-effort vendor signal (claude's
    // untagged task_notification, grok's subagent_finished, codex thread
    // closure), and one lost signal would otherwise pin the child — and this
    // loop's registry heartbeat — until the chat is archived. A subagent
    // silent this long (4h by default) is presumed lost; the reap stamps its
    // chip failed.
    let subagent_silence = session_idle * 8;
    let mut idle_since: Option<tokio::time::Instant> =
        resume_state.idle.then(tokio::time::Instant::now);
    // A parked runtime whose agent reports background work (see
    // `TurnControl::background_live`) is not idle: reaping it killed that
    // work. The reaper re-checks a full idle window later instead.
    let mut reap_deferred_until: Option<tokio::time::Instant> = None;
    // The reaper last deferred for background work. Its end is not idleness
    // either: the agent reports the drained set a moment before the turn the
    // completion wakes, and reaping in that gap lost the wake (issue #831).
    let mut held_for_background = false;
    let steerable = harness.supports_steering();
    let stops_turn_in_place = steerable && harness.stops_turn_in_place();
    // TURN-QUIESCE WATCHDOG (2026-08-12 stuck-Working incident): a harness
    // that loses a turn's Done — the adapter never settles `session/prompt`
    // even though the agent finished — strands Working forever: the live
    // heartbeat above keeps the row fresh, and there is no per-turn timeout
    // by design. This is NOT that stall timeout: it never ends the run or
    // errors anything. When the stream has been silent past the window AND
    // the fold shows completed output with nothing in flight (no unresolved
    // tool, no open question), the turn parks exactly like a Done would —
    // segment finalized Complete, status Idle, child and mailbox warm. A
    // false trip (the agent was quietly waiting on something invisible)
    // costs a status dip: the parked-resume path below re-arms Working the
    // moment output flows again, and nothing is lost. `ZERON_TURN_QUIESCE_MS`
    // overrides the window; 0 disables.
    // RETIRED for native drivers: a harness whose every turn shape ends with
    // a deterministic wire Done (claude/codex/cursor native) needs no
    // quiesce backstop — arming one only risks false parks on long silent
    // work. The env knob can configure a diagnostic window, but a prompt
    // with an authoritative completion signal must still await that signal.
    // ACP retains the watchdog only for unowned self-continued activity.
    let deterministic_turn_end = harness.deterministic_turn_end();
    let authoritative_prompt_end = harness.authoritative_prompt_end();
    let quiesce_after: Option<std::time::Duration> = match std::env::var("ZERON_TURN_QUIESCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(0) => None,
        Some(ms) => Some(std::time::Duration::from_millis(ms)),
        None if deterministic_turn_end => None,
        None => Some(std::time::Duration::from_secs(120)),
    };
    let mut last_stream_activity = tokio::time::Instant::now();
    // SELF-CONTINUED turns get a much SHORTER quiesce window. A turn the
    // agent starts on its own (background-task wake) can never receive a
    // harness Done: the adapter has no `session/prompt` outstanding to
    // settle — verified in claude-agent-acp's autonomous-result lane, which
    // consumes the SDK's turn-end without emitting anything; codex shows
    // the same shape. The watchdog is that turn shape's ONLY settle path,
    // so the default 120s window read as 2min of stuck-Working after every
    // background notification (user report 2026-08-13). The in-flight
    // fold gate below still protects running tools; reasoning heartbeats
    // push the window during real thinking. `ZERON_SELF_TURN_QUIESCE_MS`
    // overrides; 0 falls back to the normal window. An explicit
    // `ZERON_TURN_QUIESCE_MS=0` still disables the watchdog entirely.
    let self_quiesce_after: Option<std::time::Duration> =
        match std::env::var("ZERON_SELF_TURN_QUIESCE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(0) => None,
            Some(ms) => Some(std::time::Duration::from_millis(ms)),
            None => Some(std::time::Duration::from_secs(20)),
        };
    let mut self_continued_turn = false;
    // Spawn chips that have SETTLED (tagged Done seen). Content events for a
    // settled chip with no live sink are dropped — a straggler frame after
    // the freeze must not mint a new doc entry or wedge the transcript back
    // into Streaming. Only a steer (UserMessage) legitimately REOPENS a
    // settled subagent: it announces more work is coming.
    let mut settled_subagents: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Tagged ids that failed the spawn gate ([`is_chat_spawn`]): not looked
    // up again until a call with that id folds.
    let mut foreign_subagents: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Live subagent sinks, parent tool-use id → transcript doc state.
    let mut subagents: std::collections::HashMap<String, SubagentSink> =
        std::collections::HashMap::new();
    // Last tagged subagent event. A background subagent outlives the turn that
    // spawned it, and its traffic never un-parks the chat, so the idle reaper
    // measures from here as well as from the park.
    let mut last_subagent_activity: Option<tokio::time::Instant> = None;

    let mut final_completed_turn = None;
    let mut final_outcome = None;
    let final_status = loop {
        if !lock(&inner.runs).get(&chat_id).is_some_and(|h| h.run_id == run_id) {
            break SessionStatus::Idle;
        }
        let mut event: AgentEvent = if let Some(event) = prepared_events.pop_front() {
            event
        } else {
            let raw_event = tokio::select! {
                biased;
                changed = cancel_rx.changed(), if !interrupted => {
                    let _ = changed;
                    interrupted = true;
                    interrupt_deadline = Some(
                        tokio::time::Instant::now() + std::time::Duration::from_secs(3),
                    );
                    continue;
                }
                Ok(()) = supervision_rx.changed() => continue,
                _ = tokio::time::sleep_until(
                    interrupt_deadline.unwrap_or_else(tokio::time::Instant::now)
                ), if interrupt_deadline.is_some() => AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                },
                _ = async {
                    match &inference_changed {
                        Some(changed) => changed.notified().await,
                        None => futures::future::pending::<()>().await,
                    }
                }, if !interrupted => {
                    let active = lock(&inner.statuses).get(&chat_id).is_some_and(is_active);
                    if idle_since.is_some() && active {
                        idle_since = None;
                        self_continued_turn = true;
                        entry_id = new_id();
                        segment_started = now_ms();
                        inner.set_run_status(&chat_id, &run_id, SessionStatus::Working, true);
                    }
                    last_stream_activity = tokio::time::Instant::now();
                    continue;
                }
                _ = live_heartbeat.tick() => {
                    inner.touch_session(&chat_id, &run_id);
                    continue;
                }
                // A hang-up restarts the idle window, however long the call.
                _ = resume_state.voice_active.ended.notified() => {
                    if idle_since.is_some() {
                        idle_since = Some(tokio::time::Instant::now());
                    }
                    continue;
                }
                // An accepted update must not wait behind a warm between-turn
                // child for the full idle-reaper window. The completed turn is
                // already durable, so retire the parked process cleanly and let
                // the queued exclusive lease proceed.
                _ = tokio::time::sleep_until(tokio::time::Instant::now()),
                    if idle_since.is_some()
                        && *supervision_rx.borrow() == 0
                        && !resume_state.voice_active.live()
                        && inner.registry.runtime_retirement_requested(harness_id)
                        // New sends are still routed here while an update
                        // waits for idle: never retire under an accepted one.
                        && !inner.has_pending_steers(&chat_id, &run_id)
                        && !turn.background_live()
                        && !inner.inference_in_flight(&chat_id, &run_id)
                        && (subagents.is_empty()
                            || last_subagent_activity.is_none_or(|at| at.elapsed() >= subagent_silence)) =>
                {
                    let runs = lock(&inner.runs);
                    if let Some(handle) = runs.get(&chat_id).filter(|h| h.run_id == run_id) {
                        if *handle.supervision.borrow() != 0 || handle.inference_requests > 0 { continue; }
                        handle.interrupt_token.cancel();
                    }
                    break SessionStatus::Idle;
                }
                // Idle reaper (zeron SESSION_IDLE_MS): a parked persistent session
                // nobody returned to within its window releases its child. The turn
                // was finalized at Done, so this end is clean — no aborted stamp.
                // A live background subagent is somebody still using the child:
                // the window counts from its last activity, and stretches to
                // `subagent_silence` while its sink is open.
                _ = tokio::time::sleep_until(
                    {
                        let window = if subagents.is_empty() { session_idle } else { subagent_silence };
                        let base = idle_since
                            .map(|at| at.max(last_subagent_activity.unwrap_or(at)) + window);
                        match (base, reap_deferred_until) {
                            (Some(base), Some(deferred)) => base.max(deferred),
                            (base, deferred) => base.or(deferred).unwrap_or_else(tokio::time::Instant::now),
                        }
                    }
                ), if idle_since.is_some() && !resume_state.voice_active.live()
                    && *supervision_rx.borrow() == 0 => {
                    // A call that began after this wait was armed holds it off.
                    if resume_state.voice_active.live() {
                        continue;
                    }
                    // The driver's own report of background work (Claude's
                    // background agents and shells) is authoritative — it
                    // lists every task the process still runs — so it holds
                    // the runtime for as long as that work lives.
                    if turn.background_live() || inner.inference_in_flight(&chat_id, &run_id) {
                        tracing::info!(chat = %chat_id, "idle reaper deferred: the agent reports background work");
                        reap_deferred_until = Some(tokio::time::Instant::now() + session_idle);
                        held_for_background = true;
                        continue;
                    }
                    if std::mem::take(&mut held_for_background) {
                        // The work just ended: a full idle window from here.
                        reap_deferred_until = Some(tokio::time::Instant::now() + session_idle);
                        continue;
                    }
                    tracing::info!(
                        chat = %chat_id,
                        live_subagents = subagents.len(),
                        "reaping idle persistent session"
                    );
                    // Serialize retirement with retention admission; release restores
                    // the original deadline, rather than granting another idle window.
                    let runs = lock(&inner.runs);
                    if let Some(handle) = runs.get(&chat_id).filter(|h| h.run_id == run_id) {
                        if *handle.supervision.borrow() != 0 || handle.inference_requests > 0 { continue; }
                        handle.interrupt_token.cancel();
                    }
                    break SessionStatus::Idle;
                }
                // Engine-made events are final: a `show_image` image is
                // already in uploads, outside the generated-image intake root.
                Some(event) = engine_rx.recv() => {
                    prepared_events.push_back(event);
                    continue;
                }
                next = stream.next() => match next {
                    Some(Ok(event)) => event,
                    // A stream error while PARKED is a post-turn child death —
                    // the turn was already finalized, so the run ends clean
                    // instead of stamping a completed session Errored.
                    Some(Err(err)) if idle_since.is_some() => {
                        tracing::warn!(chat = %chat_id, error = %err, "parked session child died; ending clean");
                        break SessionStatus::Idle;
                    }
                    Some(Err(err)) => AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(err.to_string()),
                        session_id: None,
                    },
                    None if interrupted => AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: None,
                    },
                    // Stream end while PARKED idle: a per-turn adapter closing
                    // after its final Done — a clean end, not a crash (the turn
                    // was already finalized). Persistent adapters keep the
                    // stream open and never hit this.
                    None if idle_since.is_some() => break SessionStatus::Idle,
                    None => AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some("harness stream ended without Done".into()),
                        session_id: None,
                    },
                },
                _ = tokio::time::sleep_until(flush_at), if dirty || subagents.values().any(|s| s.dirty) => {
                    #[cfg(test)]
                    FLUSH_TICKS.with(|t| t.set(t.get() + 1));
                    // Coalesced STREAM_COMMIT_MS tick: one doc commit per window
                    // (parent + any dirty subagent docs).
                    if dirty {
                        if let Err(err) = sync_segment(
                            doc_ref, &mut writer, &entry_id, &device_id, segment_started, &folded,
                        ) {
                            tracing::warn!(chat = %chat_id, error = %err, "segment sync failed");
                        }
                        dirty = false;
                    }
                    for sink in subagents.values_mut() {
                        sink.flush(&device_id);
                    }
                    continue;
                }
                // Turn-quiesce watchdog (see the knob above). Armed only when the
                // fold says nothing is in flight: an unresolved tool part is a
                // command still running (legitimately silent for minutes — the
                // rejected stall-timeout case), an unresolved input part is a
                // question awaiting the user. The live-plan chip is exempt from
                // the tool check: it is a singleton that never resolves. An EMPTY
                // fold still arms — a Steered boundary that no output ever
                // follows is one of the wedge shapes — it just parks without
                // writing a segment (an empty finalize would leave a stub entry).
                _ = tokio::time::sleep_until({
                    let mut window = quiesce_after.unwrap_or_default();
                    if self_continued_turn && let Some(short) = self_quiesce_after {
                        window = window.min(short);
                    }
                    last_stream_activity + window
                }), if quiesce_after.is_some()
                    && (self_continued_turn || !authoritative_prompt_end)
                    && idle_since.is_none()
                    && !interrupted
                    && steerable
                    && !inner.inference_in_flight(&chat_id, &run_id)
                    && !folded.iter().any(|p| match p {
                        MessagePart::Tool { id, resolved: false, .. } => {
                            id != zeron_proto::LIVE_PLAN_TOOL_ID
                        }
                        MessagePart::Input { resolved: false, .. } => true,
                        _ => false,
                    }) =>
                {
                    tracing::warn!(
                        chat = %chat_id,
                        quiet_ms = quiesce_after.unwrap_or_default().as_millis() as u64,
                        "turn quiesced: stream silent after completed output with no \
                         turn-end; parking (suspected missing harness Done)"
                    );
                    // Some adapters close a completed response through this
                    // engine watchdog instead of a native Done. Preserve that
                    // completion notice, but never notify for an empty boundary
                    // or while an accepted steer still awaits delivery.
                    let completed_turn = ((!folded.is_empty() || writer.is_some())
                        && !inner.has_pending_steers(&chat_id, &run_id)
                        && !inner.continues_with_sent_message(&chat_id))
                        .then(|| entry_id.clone());
                    if !folded.is_empty() || writer.is_some() {
                        if let Err(err) = finish_segment(
                            doc_ref,
                            writer.take(),
                            &entry_id,
                            &device_id,
                            segment_started,
                            &folded,
                            MessageStatus::Complete,
                        ) {
                            tracing::warn!(chat = %chat_id, error = %err, "quiesce segment finish failed");
                        }
                        inner.note_message(&chat_id, &folded_text(&folded));
                    }
                    folded.clear();
                    dirty = false;
                    entry_id = new_id();
                    segment_started = now_ms();
                    idle_since = Some(tokio::time::Instant::now());
                    reap_deferred_until = None;
                    self_continued_turn = false;
                    inner.set_status_with_completion(
                        &chat_id, Some(&run_id), SessionStatus::Idle, false, completed_turn,
                        Some(DoneStatus::Completed),
                    );
                    continue;
                }
            };

            prepared_events.extend(
                inner
                    .prepare_generated_image(&chat_id, raw_event, &mut seen_images)
                    .await,
            );
            let Some(event) = prepared_events.pop_front() else {
                continue;
            };
            event
        };
        // An asynchronous preparation can outlive the bounded terminate wait.
        if !lock(&inner.runs).get(&chat_id).is_some_and(|h| h.run_id == run_id) {
            break SessionStatus::Idle;
        }

        // ── subagent routing ───────────────────────────────────────────
        // Tagged events NEVER fold into the parent transcript: they stream
        // into the subagent's own doc, and the parent keeps only the spawn
        // chip (ref + status + one-line tail) — refreshed through the live
        // fold while its segment still streams, else stamped in place (the
        // eager-done norm: the chip's entry finished before the background
        // subagent spoke). Handled BEFORE the parked gate so a parked
        // session's background traffic still reaches the subagent doc
        // without un-parking the chat.
        if let AgentEvent::Subagent {
            parent_tool_use_id,
            event: sub_event,
        } = &event
        {
            inner.publish(&chat_id, &event);
            last_subagent_activity = Some(tokio::time::Instant::now());
            // Background subagents can outlive the turn and be the only
            // traffic for minutes: keep the row (and its count) inside the
            // UI's staleness window while any is live.
            if !subagents.is_empty() {
                inner.touch_session(&chat_id, &run_id);
            }
            let is_steer = matches!(
                sub_event.as_ref(),
                AgentEvent::UserMessage { .. } | AgentEvent::Steered { .. }
            );
            if is_steer {
                settled_subagents.remove(parent_tool_use_id);
            } else if settled_subagents.contains(parent_tool_use_id)
                && !subagents.contains_key(parent_tool_use_id)
            {
                // Straggler after the freeze: chip-only silence (the old
                // pre-viz behavior), never a reopened doc.
                continue;
            }
            let sub_id = subagent_doc_id(&chat_id, parent_tool_use_id);
            let chip_streaming = folded
                .iter()
                .any(|p| matches!(p, MessagePart::Tool { id, .. } if id == parent_tool_use_id));
            let sink_known = subagents.contains_key(parent_tool_use_id);
            // A Done with NO sink (a subagent that never streamed — codex
            // turn ends can beat registration) is chip-only: minting a doc
            // just to freeze it empty helps no one, and stamping the ref
            // would link the chip to that never-created doc (an empty tab
            // on click).
            let done_only = !sink_known && matches!(sub_event.as_ref(), AgentEvent::Done { .. });
            let opens_sink =
                !sink_known && !done_only && !foreign_subagents.contains(parent_tool_use_id) && {
                    let spawn = is_chat_spawn(parent_tool_use_id, &folded, doc_ref);
                    if !spawn {
                        foreign_subagents.insert(parent_tool_use_id.clone());
                    }
                    spawn
                };
            if chip_streaming {
                if !sink_known && !done_only {
                    for p in folded.iter_mut() {
                        if let MessagePart::Tool {
                            id,
                            call,
                            subagent_ref,
                            ..
                        } = p
                            && id == parent_tool_use_id
                            // Genus gate: a subagent ref may only ever bind
                            // to a SPAWN call — mis-keyed tagged traffic
                            // must not turn an ordinary chip into a spawn
                            // link (empty tab on click).
                            && call.is_subagent_spawn()
                        {
                            *subagent_ref = Some(sub_id.clone());
                        }
                    }
                }
                zeron_doc::fold_event_into_parts(&mut folded, &event);
                if !dirty {
                    dirty = true;
                    flush_at = tokio::time::Instant::now()
                        + std::time::Duration::from_millis(STREAM_COMMIT_MS);
                }
            }
            // Open the sink lazily; an open failure degrades to chip-only.
            if opens_sink {
                let opened = inner.doc_host().and_then(|host| match host.open(&sub_id) {
                    Ok(handle) => Some(handle.writer()),
                    Err(err) => {
                        tracing::warn!(doc = %sub_id, error = %err, "subagent doc open failed (chip-only)");
                        None
                    }
                });
                if let Some(sub_doc) = opened {
                    subagents.insert(
                        parent_tool_use_id.clone(),
                        SubagentSink {
                            doc_id: sub_id.clone(),
                            doc: sub_doc,
                            entry_id: new_id(),
                            started_at: now_ms(),
                            entry_index: None,
                            written: Vec::new(),
                            folded: Vec::new(),
                            dirty: false,
                        },
                    );
                    inner.set_running_subagents(&chat_id, &run_id, subagents.len());
                    if !chip_streaming {
                        let _ = doc_ref.update_subagent_chip(
                            parent_tool_use_id,
                            Some(&sub_id),
                            Some("running"),
                            None,
                        );
                    }
                }
            }
            let done = matches!(sub_event.as_ref(), AgentEvent::Done { .. });
            if done {
                settled_subagents.insert(parent_tool_use_id.clone());
                if !chip_streaming {
                    // A resumed run can finish an older chip without ever
                    // opening a sink. The chip's lifecycle is independent of
                    // whether this run received transcript content.
                    let _ = doc_ref.update_subagent_chip(
                        parent_tool_use_id,
                        None,
                        subagent_chip_update(sub_event),
                        None,
                    );
                }
            }
            if let Some(sink) = subagents.get_mut(parent_tool_use_id) {
                if let AgentEvent::UserMessage { text } = sub_event.as_ref() {
                    // A steer splits ENTRIES, not parts — handled at the
                    // sink level (the fold ignores UserMessage).
                    sink.push_user(&device_id, text);
                    continue;
                }
                let was_clean = !sink.dirty;
                zeron_doc::fold_event_into_parts(&mut sink.folded, sub_event);
                sink.dirty = true;
                // A sink waking on its own must arm the same commit window
                // the parent's dirty flag does — `flush_at` is otherwise only
                // rescheduled on the parent's flip, so a past deadline would
                // fire the commit branch on every event instead of once per
                // STREAM_COMMIT_MS window.
                if was_clean && !dirty && flush_at <= tokio::time::Instant::now() {
                    flush_at = tokio::time::Instant::now()
                        + std::time::Duration::from_millis(STREAM_COMMIT_MS);
                }
                if done {
                    let status = match sub_event.as_ref() {
                        AgentEvent::Done {
                            status: DoneStatus::Errored,
                            ..
                        } => MessageStatus::Complete,
                        AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            ..
                        } => MessageStatus::Aborted,
                        _ => MessageStatus::Complete,
                    };
                    let sink = subagents.remove(parent_tool_use_id).expect("checked");
                    inner.set_running_subagents(&chat_id, &run_id, subagents.len());
                    let doc_id = sink.doc_id.clone();
                    // FREEZE: the finished transcript uploads as a static R2
                    // blob (`blob/{chatId}/{subDocId}`) so viewers of
                    // finished subagents never wake the doc's room; dropping
                    // the sink unpins the doc for the LRU and the room
                    // idles. The live doc remains the fallback.
                    if let Some(json) = sink.finish(&device_id, status)
                        && let Some(host) = inner.doc_host()
                    {
                        host.upload_tool_sidecar(
                            &chat_id,
                            zeron_doc::SidecarPayload {
                                part_id: doc_id,
                                output: Some(json),
                                diff: None,
                            },
                        );
                    }
                }
            }
            continue;
        }

        // Any stream activity proves the run is alive — keep the session's
        // freshness inside the UI's 45s staleness window (throttled), and
        // push the quiesce watchdog's window out.
        inner.touch_session(&chat_id, &run_id);
        last_stream_activity = tokio::time::Instant::now();
        // The engine's input bridge is the sole authority on input requests:
        // it mints the id and parks the resolver BEFORE emitting the event,
        // so a legitimate id is always pending here. A harness emitting its
        // own copy (a different id no resolver knows) would fold an
        // unanswerable twin chip into the doc — and answering the twin would
        // never resume the run. Dropped BEFORE the parked gate below: letting
        // it un-park the session and then dropping it would disarm the idle
        // reaper with no turn running (a leaked child no reap ever ends).
        if let AgentEvent::InputRequested { request_id, .. } = &event {
            let pending = lock(&inner.runs)
                .get(&chat_id)
                .map(|h| h.pending_inputs.clone());
            let known = pending.is_some_and(|p| lock(&p).contains_key(request_id));
            if !known {
                tracing::warn!(
                    chat = %chat_id,
                    request = %request_id,
                    "dropping harness-emitted InputRequested (unknown id; \
                     the engine input bridge owns this lifecycle)"
                );
                continue;
            }
        }
        if let AgentEvent::PlanUsage {
            windows,
            plan_label,
        } = &event
        {
            let credential = lock(&inner.runs)
                .get(&chat_id)
                .and_then(|run| run.credential.clone());
            if let Some(credential) = credential {
                inner
                    .registry
                    .record_plan_usage(
                        harness_id,
                        &credential,
                        windows.clone(),
                        plan_label.clone(),
                    )
                    .await;
            }
            continue;
        }
        // Capacity/occupancy can settle after Done; updating it must not reopen a turn.
        if let AgentEvent::ContextUsage { tokens, window } = &event {
            if let Err(err) = doc_ref.update_context_usage(*tokens, *window) {
                tracing::warn!(%chat_id, error = %err, "context usage write failed");
            }
            continue;
        }
        if matches!(event, AgentEvent::TurnStarted) {
            if idle_since.is_some() && !interrupted {
                let protection = async {
                    if interrupt.is_cancelled() { return Ok(false); }
                    if inner.boat_lease.required() && !inner.boat_lease.active(&run_id) {
                        tokio::select! {
                            biased;
                            result = inner.boat_lease.protect() => result.map_err(EngineError::Other)?,
                            _ = interrupt.cancelled() => return Ok(false),
                        }
                    }
                    let runs = lock(&inner.runs);
                    let handle = runs.get(&chat_id).filter(|h| h.run_id == run_id)
                        .ok_or_else(|| EngineError::Other("Stale Crew native turn".into()))?;
                    if handle.retiring.load(std::sync::atomic::Ordering::Acquire)
                        || handle.interrupt_token.is_cancelled() {
                        return Ok(false);
                    }
                    inner.start_turn(&chat_id, handle)?;
                    // The last auxiliary response may drop concurrently. Keep
                    // ownership locked until Working is visible, so its Drop
                    // cannot release this newly admitted foreground turn.
                    inner.set_status_with_completion_locked(
                        &runs, &chat_id, Some(&run_id), SessionStatus::Working, true, None, None,
                    );
                    Ok::<bool, EngineError>(true)
                }.await;
                match protection {
                    Ok(true) => {
                        idle_since = None;
                        self_continued_turn = false;
                        entry_id = new_id();
                        segment_started = now_ms();
                    }
                    Ok(false) => {
                        // Cancellation is not a provider denial. Keep its
                        // provenance instead of inspecting the token after an
                        // error, since protection failure can cancel it too.
                        interrupted = true;
                        interrupt.cancel();
                        event = AgentEvent::Done {
                            status: DoneStatus::Interrupted, result: None, error: None, session_id: None,
                        };
                    }
                    Err(error) => {
                        interrupted = true;
                        interrupt.cancel();
                        event = AgentEvent::Done {
                            status: DoneStatus::Errored, result: None, error: Some(error.to_string()), session_id: None,
                        };
                    }
                }
            }
            if matches!(event, AgentEvent::TurnStarted) { continue; }
        }
        // Provider failure is not a user Stop: surface a durable recoverable
        // error even when the harness closes before the injected Error arrives.
        if inner.boat_lease.failed(&run_id)
            && let AgentEvent::Done { status, error, .. } = &mut event
        {
            interrupted = true;
            *status = DoneStatus::Errored;
            *error = Some(crate::boat_lease::FAILURE.into());
        }
        // PARKED: a steer boundary, a terminal Done, or SELF-CONTINUED OUTPUT
        // re-opens the session; everything else stays gated. The ACP child
        // keeps forwarding `session/update` frames after a turn completes,
        // and they split two ways:
        //
        // - Post-turn NOISE — late tool_call_updates for commands folded in a
        //   prior segment, command refreshes, reasoning heartbeats. Treating
        //   those as "the next turn" re-armed Working with no Done ever
        //   coming (the eternally-running session bug) and folded orphan
        //   parts into a phantom segment. Still dropped.
        // - SELF-CONTINUED WORK — Claude Code re-invokes itself when a
        //   background task finishes (turns no prompt started) and streams
        //   real output for them. Dropping those LOST transcript content
        //   (2026-08-12: "Build finished successfully…" streamed by the
        //   agent, absent from the doc). Fresh text or a genuinely new tool
        //   call resumes the session: new segment, Working, and the turn
        //   settles again via Done — or via the quiesce watchdog, which is
        //   what makes this resume safe where the naive version was not.
        //
        // The RESUME_GATE separates the two by arrival time: a finished
        // turn's tail flush lands within milliseconds of its Done, while a
        // self-continued turn starts a whole new agent round trip (seconds
        // at minimum, minutes in the incident). Inside the gate everything
        // non-boundary stays inert, exactly as before.
        let turn_was_active = idle_since.is_none();
        const RESUME_GATE: std::time::Duration = std::time::Duration::from_secs(1);
        if idle_since.is_some() {
            let self_continued = idle_since
                .is_some_and(|parked_at| parked_at.elapsed() >= RESUME_GATE)
                && (matches!(
                    &event,
                    AgentEvent::TextDelta { text } if !text.is_empty()
                ) || matches!(
                    &event,
                    AgentEvent::ToolCall { id, .. }
                        if id == zeron_proto::LIVE_PLAN_TOOL_ID || !seen_tools.contains(id)
                ));
            if self_continued {
                // Admission happened before the managed relay forwarded inference.
                // Re-register only the turn bookkeeping if its previous Done raced
                // delivery of this output; never perform late provider protection.
                let protection = {
                    let runs = lock(&inner.runs);
                    runs.get(&chat_id).filter(|h| h.run_id == run_id)
                        .ok_or_else(|| EngineError::Other("Crew turn disappeared while resuming".into()))
                        .and_then(|handle| inner.start_turn(&chat_id, handle))
                };
                if let Err(error) = protection {
                    if let Some(handle) = lock(&inner.runs).get(&chat_id).filter(|h| h.run_id == run_id) {
                        let _ = handle.engine_tx.send(AgentEvent::Done {
                            status: DoneStatus::Errored, result: None, error: Some(error.to_string()), session_id: None,
                        });
                        let _ = handle.cancel.send(true);
                        handle.interrupt_token.cancel();
                    }
                    continue;
                }
                tracing::info!(
                    chat = %chat_id,
                    "parked session resumed by self-continued agent output"
                );
                idle_since = None;
                self_continued_turn = true;
                // The park cleared the fold; rotate to a fresh entry and
                // fall through — this event is the new segment's first part.
                entry_id = new_id();
                segment_started = now_ms();
                inner.set_run_status(&chat_id, &run_id, SessionStatus::Working, true);
            } else {
                match &event {
                    AgentEvent::SessionStarted { .. }
                        if resume_state.idle && !saw_session_started => {}
                    AgentEvent::Steered { .. } => {
                        idle_since = None;
                        inner.set_run_status(&chat_id, &run_id, SessionStatus::Working, true);
                    }
                    AgentEvent::Done { .. } => {
                        idle_since = None;
                    }
                    // A question with NO turn behind it (post-turn permission
                    // noise): answer it empty right here. Un-parking into
                    // AwaitingInput would disarm the idle reaper with no Done
                    // ever coming — stranded status, leaked warm child.
                    AgentEvent::InputRequested { request_id, .. } => {
                        let resolver = lock(&inner.runs)
                            .get(&chat_id)
                            .and_then(|h| lock(&h.pending_inputs).remove(request_id));
                        if let Some(tx) = resolver {
                            let _ = tx.send(Vec::new());
                        }
                        tracing::debug!(chat = %chat_id, "parked session: post-turn input request auto-declined");
                        continue;
                    }
                    // A stale answer settling after its turn already closed:
                    // nothing is running — stay parked.
                    AgentEvent::InputResolved { .. } => continue,
                    _ => continue,
                }
            }
        }
        // Empty reasoning deltas are PURE heartbeats: redacted thinking and
        // tool-input-generation windows stream them with no text. They fold
        // to nothing, so journaling/publishing them is only noise (hundreds
        // per long turn observed) — the touch above already did their job.
        if matches!(&event, AgentEvent::ReasoningDelta { text } if text.is_empty()) {
            continue;
        }

        // Stale tool echoes: a ToolCall/ToolResult naming an id folded in a
        // PRIOR segment (the fold reset at a steer or park since) belongs to
        // a chip that already rendered in its own entry. Folding the echo
        // would splice a phantom chip into the middle of the current
        // segment's streaming text (the mid-word transcript splits). Dropped;
        // same-segment refreshes still land in place.
        let in_segment = |folded: &[MessagePart], id: &str| {
            folded
                .iter()
                .any(|p| matches!(p, MessagePart::Tool { id: pid, .. } if pid == id))
        };
        match &event {
            // The live plan chip is a deliberate singleton (every update
            // reuses `LIVE_PLAN_TOOL_ID` so the fold refreshes in place):
            // treating its reappearance after a park/steer reset as a stale
            // echo dropped the todo list for the rest of the run — from the
            // first boundary on, plans never rendered again.
            AgentEvent::ToolCall { id, .. } if id == zeron_proto::LIVE_PLAN_TOOL_ID => {}
            AgentEvent::ToolResult { id, .. } if id == zeron_proto::LIVE_PLAN_TOOL_ID => {}
            AgentEvent::ToolCall { id, .. } => {
                if !in_segment(&folded, id) && seen_tools.contains(id) {
                    continue;
                }
                seen_tools.insert(id.clone());
                // This call may be (or become) a spawn: look again.
                foreign_subagents.remove(id);
            }
            AgentEvent::ToolResult { id, .. }
                if !in_segment(&folded, id) && seen_tools.contains(id) =>
            {
                continue;
            }
            _ => {}
        }

        // Startup-crash retry: a run that dies before ever starting (errored
        // Done, no SessionStarted, nothing streamed) means the AGENT CHILD
        // failed to come up — not that the injected resume id was bad. Since
        // the ACP conversion (2026-08-08) a stale id is handled inside the
        // harness (`session/load` falls back to `session/new`), so the old
        // guess here — tombstone the id, retry fresh — fired only on child
        // startup failures and permanently severed GOOD conversations (user
        // incident 2026-08-13). The id stays; retry ONCE against the same
        // user entry, resume and all, in case the crash was transient. A
        // helper that is down hard fails the retry too and surfaces its
        // crash text (the harness now appends exit status + stderr).
        if resume_state.resume_injected
            && !resume_state.idle
            && !resume_state.startup_retry
            && !saw_session_started
            && folded.is_empty()
            && !interrupted
            && matches!(
                &event,
                AgentEvent::Done {
                    status: DoneStatus::Errored,
                    ..
                }
            )
            && let Some(retry) = retry_request.take()
        {
            tracing::warn!(
                chat = %chat_id,
                "run died before session start; retrying once (resume kept)"
            );
            if let Some(revival) = resume_state.revival.clone() {
                lock(&inner.revivals).insert(chat_id.clone(), revival);
            }
            inner.remove_run(&chat_id, &run_id);
            let engine = SessionsEngine {
                inner: inner.clone(),
            };
            let chat = chat_id.clone();
            let message_id = resume_state.user_message_id.clone();
            let stops_seen = resume_state.stops_seen;
            tokio::spawn(async move {
                // The user entry write inside dispatch is idempotent by
                // message id; `startup_retry` makes this attempt final.
                if let Err(err) = engine
                    .dispatch_with(
                        &chat,
                        harness_id,
                        retry,
                        Some(message_id),
                        true,
                        Some(stops_seen),
                    )
                    .await
                {
                    tracing::error!(chat = %chat, error = %err, "startup-crash retry dispatch failed");
                    // No run is coming: leaving the row Working would spin
                    // the session forever with nothing behind it.
                    engine
                        .inner
                        .set_status(&chat, SessionStatus::Errored, false);
                }
            });
            return;
        }

        // A steer boundary splits the assistant entry exactly where the fold resets.
        if let AgentEvent::Steered {
            next_assistant_message_id,
            ..
        } = &event
        {
            inner.publish(&chat_id, &event);
            // A steer boundary means a real prompt owns the turn again — its
            // Done will come; the short self-continued window stands down.
            self_continued_turn = false;
            if let Err(err) = finish_segment(
                doc_ref,
                writer.take(),
                &entry_id,
                &device_id,
                segment_started,
                &folded,
                MessageStatus::Complete,
            ) {
                tracing::warn!(chat = %chat_id, error = %err, "segment finish failed");
            }
            inner.note_message(&chat_id, &folded_text(&folded));
            folded.clear();
            dirty = false;
            entry_id = next_assistant_message_id.clone().unwrap_or_else(new_id);
            segment_started = now_ms();
            // The elapsed timer is per user message, not per child process: a
            // steer boundary restarts it (matches the parked-resume path and
            // the composer's optimistic overlay, which already reads 0:00).
            inner.set_run_status(&chat_id, &run_id, SessionStatus::Working, true);
            // The boundary confirms delivery of the oldest accepted steer —
            // retire its at-least-once ledger entry.
            let confirmed = lock(&inner.runs)
                .get(&chat_id)
                .filter(|h| h.run_id == run_id)
                .and_then(|h| lock(&h.routed_steers).pop_front());
            // Consumed: a steer carrying the fork history delivered it to
            // this runtime's provider session.
            if confirmed.is_some_and(|steer| steer.fork_history)
                && let Some(session) = lock(&inner.harness_sessions)
                    .get(&chat_id)
                    .map(|known| known.session_id.clone())
                    .filter(|id| !id.is_empty())
            {
                let _ = doc.set_fork_history_session(&session);
            }
            continue;
        }

        match &event {
            AgentEvent::SessionStarted {
                session_id, cwd, ..
            } => {
                saw_session_started = true;
                // The event's own cwd (where the harness actually created the
                // session) scopes the stored id, not the request's.
                inner.remember_harness_session(&chat_id, session_id, cwd, harness_id);
                note_fork_history_session(&doc, carried_history, session_id);
                if !resume_state.idle {
                    Inner::note_provider_session(
                        &doc,
                        harness_id,
                        session_id,
                        cwd,
                        Some(&resume_state.user_message_id),
                    );
                }
                run_session = Some((session_id.clone(), cwd.clone()));
            }
            AgentEvent::Done {
                session_id: Some(session_id),
                ..
            } => {
                inner.remember_harness_session(&chat_id, session_id, &run_cwd, harness_id);
                note_fork_history_session(&doc, carried_history, session_id);
                let cwd = run_session
                    .take()
                    .filter(|(started, _)| started == session_id)
                    .map_or_else(|| run_cwd.clone(), |(_, cwd)| cwd);
                run_session = Some((session_id.clone(), cwd));
            }
            AgentEvent::InputRequested { .. } => {
                // Known-id guaranteed: the unknown-id twin was dropped above,
                // before the parked gate.
                inner.set_run_status(&chat_id, &run_id, SessionStatus::AwaitingInput, false);
            }
            AgentEvent::InputResolved { .. } => {
                inner.set_run_status(&chat_id, &run_id, SessionStatus::Working, false);
            }
            _ => {}
        }

        inner.publish(&chat_id, &event);

        // Defensive rule from zeron: a mid-run SessionStarted re-emission (Claude SDK
        // background re-invocations) must not wipe the segment being written.
        let skip_fold = matches!(&event, AgentEvent::SessionStarted { .. }) && !folded.is_empty();
        if !skip_fold {
            fold_event_into_parts(&mut folded, &event);
            // R2 sidecar PARKED (2026-08-10, product call): the fold's
            // summary/stats ARE the doc's whole record — no refs stamped, no
            // uploads. Full outputs survive only in the host's local run
            // journal. To reintroduce: `zeron_doc::sidecar_payload(&event)`
            // → `apply_sidecar_refs` → `doc_host.upload_tool_sidecar`, all
            // still in place and tested.
        }

        if let AgentEvent::Done { status, .. } = &event {
            // A question still pending at turn end can never be legitimately
            // answered (its turn is over): drain the resolvers NOW, or a late
            // `respond_input` finds one, emits InputResolved, and un-parks
            // the session into Working with no turn behind it — stranded
            // Working, timer forever, reaper disarmed. Empty answers unblock
            // the harness-side bridge like an interrupt does.
            let pending = lock(&inner.runs)
                .get(&chat_id)
                .filter(|h| h.run_id == run_id)
                .map(|h| h.pending_inputs.clone());
            if let Some(pending) = pending {
                for (_, tx) in lock(&pending).drain() {
                    let _ = tx.send(Vec::new());
                }
            }
            let message_status = match status {
                DoneStatus::Interrupted => MessageStatus::Aborted,
                DoneStatus::Completed | DoneStatus::Errored => MessageStatus::Complete,
            };
            // No dangling chips: a run that ends for ANY reason (completed,
            // errored, interrupted) terminally resolves its input parts — an
            // unresolved question must not outlive the run that asked it
            // (its resolver died with the run; an answer could never land).
            for part in folded.iter_mut() {
                if let MessagePart::Input { resolved, .. } = part {
                    *resolved = true;
                }
            }
            // A Done landing on a PARKED session with nothing streamed (the
            // idle reaper's or an interrupt's own teardown) has no entry to
            // finalize — writing one would leave an empty aborted stub.
            let nothing_streamed = writer.is_none() && folded.is_empty();
            if !nothing_streamed {
                if let Err(err) = finish_segment(
                    doc_ref,
                    writer.take(),
                    &entry_id,
                    &device_id,
                    segment_started,
                    &folded,
                    message_status,
                ) {
                    tracing::warn!(chat = %chat_id, error = %err, "final segment finish failed");
                }
                inner.note_message(&chat_id, &folded_text(&folded));
            }
            if let Some((session_id, cwd)) = &run_session {
                Inner::note_provider_session(&doc, harness_id, session_id, cwd, None);
            }
            if *status == DoneStatus::Completed {
                // A cleanly completed turn resets the auto-resume revival
                // budget: only consecutive crash-revive-crash cycles spend it.
                inner.journal.clear_resume_attempts(&chat_id);
            }
            // Exchange completed on an untitled chat → name it (fire-and-forget;
            // interrupted/errored turns never trigger naming).
            if *status == DoneStatus::Completed
                && let Some(titles) = inner.titles.get()
            {
                titles.maybe_generate(&chat_id, harness_id, &user_prompt, &run_cwd);
            }
            // A turn stopped in place: the runtime lives on, parked like a
            // completed turn. Steers it had accepted but not yet delivered
            // were cancelled with the turn (never delivered, never
            // confirmed) — drop them, or they would hold the parked session
            // Working forever.
            let stopped_in_place =
                *status == DoneStatus::Interrupted && stops_turn_in_place && !interrupted;
            if stopped_in_place
                && let Some(ledger) = lock(&inner.runs)
                    .get(&chat_id)
                    .filter(|h| h.run_id == run_id)
                    .map(|h| h.routed_steers.clone())
            {
                lock(&ledger).clear();
            }
            // An accepted steer awaiting its boundary owns the continuation:
            // the previous Done is an internal handoff, not a completion ping.
            // So does a message the user already sent that is still on its
            // way (see `continues_with_sent_message`); rows the user queued
            // themselves still notify.
            let pending_steer = inner.has_pending_steers(&chat_id, &run_id);
            let completed_turn = (*status == DoneStatus::Completed
                && !interrupted
                && turn_was_active
                && !pending_steer
                && !inner.continues_with_sent_message(&chat_id))
            .then(|| entry_id.clone());
            // PERSISTENT SESSION: a cleanly completed turn on a steerable
            // harness PARKS instead of ending — child + mailbox stay warm for
            // the next routed dispatch; per-turn state resets for it.
            if (*status == DoneStatus::Completed || stopped_in_place)
                && steerable
                && !interrupted
                && !inner.registry.runtime_retirement_requested(harness_id)
            {
                folded.clear();
                dirty = false;
                entry_id = new_id();
                segment_started = now_ms();
                // Resume-retry is strictly a first-turn concern.
                saw_session_started = true;
                idle_since = Some(tokio::time::Instant::now());
                reap_deferred_until = None;
                self_continued_turn = false;
                // Accepted steers still own this runtime. Publishing Idle
                // here lets the ordinary queue overtake that continuation.
                inner.set_status_with_completion(
                    &chat_id, Some(&run_id),
                    if pending_steer {
                        SessionStatus::Working
                    } else {
                        SessionStatus::Idle
                    },
                    false,
                    completed_turn,
                    (!pending_steer && turn_was_active).then_some(*status),
                );
                continue;
            }
            final_completed_turn = completed_turn;
            // Retiring an already parked process is not a new turn outcome.
            final_outcome = turn_was_active.then_some(*status);
            break match status {
                DoneStatus::Errored => SessionStatus::Errored,
                _ => SessionStatus::Idle,
            };
        }

        if !folded.is_empty() && !dirty {
            dirty = true;
            flush_at =
                tokio::time::Instant::now() + std::time::Duration::from_millis(STREAM_COMMIT_MS);
        }
    };

    // Any subagent still streaming when the run ends freezes as-is: the
    // parent process is gone, so nothing more can arrive on this stream.
    for (parent_id, sink) in subagents.drain() {
        let doc_id = sink.doc_id.clone();
        let _ = doc_ref.update_subagent_chip(&parent_id, None, Some("failed"), None);
        if let Some(json) = sink.finish(&device_id, MessageStatus::Aborted)
            && let Some(host) = inner.doc_host()
        {
            host.upload_tool_sidecar(
                &chat_id,
                zeron_doc::SidecarPayload {
                    part_id: doc_id,
                    output: Some(json),
                    diff: None,
                },
            );
        }
    }
    inner.set_running_subagents(&chat_id, &run_id, 0);

    // Close this exact runtime's admission before draining and publishing Idle:
    // a sender waiting for mailbox capacity must fall back to a fresh runtime,
    // never append an accepted message after its final orphan drain.
    let orphans = inner.retire_run(&chat_id, &run_id);
    inner.set_status_with_completion(
        &chat_id,
        Some(&run_id),
        final_status,
        false,
        final_completed_turn,
        final_outcome,
    );
    if interrupted {
        inner.mark_shutdown_cut(&chat_id, &run_id);
    }
    inner.remove_run(&chat_id, &run_id);
    // A Stop cancels the messages it found waiting; one accepted after it
    // began is still the user's next message and must run.
    let orphans: Vec<RoutedSteer> = if inner
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
    {
        Vec::new()
    } else if interrupted {
        orphans.into_iter().filter(|s| s.after_stop).collect()
    } else {
        orphans
    };
    if !orphans.is_empty() {
        // The dying run accepted these into its mailbox but never confirmed a
        // Steered boundary (idle-reaper race, a mid-turn error discarding
        // queued boundary steers, a parked child death). Their user entries
        // are already in the transcript — a message that shows as sent must
        // never silently not run. Re-dispatch each as a fresh turn
        // (write_user_message dedupes by id; resume is engine-injected).
        let engine = SessionsEngine {
            inner: inner.clone(),
        };
        let chat = chat_id.clone();
        tokio::spawn(async move {
            for steer in orphans {
                let Some(mut request) = steer
                    .request
                    .map(|request| *request)
                    .or_else(|| engine.last_request(&chat))
                else {
                    tracing::warn!(chat = %chat, "orphaned steer lost: no run config to re-dispatch");
                    break;
                };
                request.prompt = steer.prompt.clone();
                request.resume = None;
                tracing::info!(chat = %chat, "re-dispatching steer orphaned by a dying run");
                let Some(host) = engine.inner.doc_host() else {
                    tracing::warn!(chat = %chat, "orphaned steer lost: doc host unavailable");
                    break;
                };
                if let Err(err) = host
                    .dispatch_with_source_context(
                        &engine,
                        &chat,
                        harness_id,
                        request,
                        Some(steer.message_id.clone()),
                    )
                    .await
                {
                    tracing::warn!(chat = %chat, error = %err, "orphaned steer re-dispatch failed");
                    engine
                        .inner
                        .set_status(&chat, SessionStatus::Errored, false);
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_recovery_converts_rich_messages_before_json_encoding() {
        let doc = zeron_doc::SessionDoc::init("cursor-rich-recovery").unwrap();
        let skill = zeron_proto::invocation::Invocation::Skill {
            name: "review \"quoted\"".into(),
            path: "/repo/quoted \"path\"/SKILL.md".into(),
            command: None,
        }
        .link();
        let previous = format!("Previous {skill}\nSecond line with \\ and \"quotes\"");
        doc.push_message(&zeron_doc::SessionMessageEntry {
            id: "u1".into(),
            role: zeron_doc::MessageRole::User,
            parts: vec![zeron_doc::MessagePart::Text {
                id: "u1-text".into(),
                text: previous.clone(),
            }],
            created_at: 0,
            device_id: "test".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        })
        .unwrap();
        let current = format!("Current {skill}\nKeep **Markdown**");
        let delivered = super::cursor_unstarted_history(&doc, "u2", &current, false).unwrap();
        let (_, json) = delivered.split_once('\n').unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed["currentUserMessage"],
            zeron_proto::invocation::harness_prompt(&current, zeron_proto::HarnessId::Cursor)
        );
        assert_eq!(
            parsed["previousUserMessages"][0],
            zeron_proto::invocation::harness_prompt(&previous, zeron_proto::HarnessId::Cursor)
        );
    }

    #[test]
    fn cursor_without_a_session_id_retains_only_preceding_user_messages() {
        let doc = zeron_doc::SessionDoc::init("cursor-unstarted").unwrap();
        for (id, role, text) in [
            (
                "u1",
                zeron_doc::MessageRole::User,
                "first interrupted request",
            ),
            ("a1", zeron_doc::MessageRole::Assistant, "partial output"),
            ("u2", zeron_doc::MessageRole::User, "current request"),
            ("u3", zeron_doc::MessageRole::User, "future pending request"),
        ] {
            doc.push_message(&zeron_doc::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: format!("{id}-text"),
                    text: text.into(),
                }],
                created_at: 0,
                device_id: "test".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        }
        assert_eq!(
            super::cursor_unstarted_history(&doc, "u1", "first interrupted request", false)
                .unwrap(),
            "first interrupted request"
        );
        let prompt = super::cursor_unstarted_history(&doc, "u2", "current request", false).unwrap();
        assert!(prompt.contains("first interrupted request"));
        assert!(prompt.contains("current request"));
        assert!(!prompt.contains("partial output"));
        assert!(!prompt.contains("future pending request"));
        assert!(prompt.contains("do not rerun prior tools or side effects"));
        assert_eq!(
            super::cursor_unstarted_history(&doc, "u2", "current request", true).unwrap(),
            "current request",
            "content from the preceding turn is already in the native session"
        );
        assert!(
            super::cursor_unstarted_history(&doc, "u3", "future pending request", true)
                .unwrap()
                .contains("current request"),
            "an unacknowledged prompt after an older checkpoint must survive"
        );
        assert_eq!(doc.read_entries().unwrap().len(), 4);
    }

    use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

    #[tokio::test]
    async fn generated_image_failure_is_sanitized_even_inside_subagents() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionsEngine::new(
            "host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()),
            Arc::new(HarnessRegistry::new()),
        );
        let source = "/outside/private/source.png";
        let event = AgentEvent::Subagent {
            parent_tool_use_id: "child".into(),
            event: Box::new(AgentEvent::GeneratedImage {
                id: "i:image".into(),
                path: source.into(),
                name: "secret".into(),
                mime_type: "bad".into(),
            }),
        };
        let events = sessions
            .inner
            .prepare_generated_image("chat", event, &mut std::collections::HashSet::new())
            .await;
        assert_eq!(events.len(), 2);
        let mut parts = vec![];
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "i".into(),
                call: zeron_proto::ToolCall::Unknown {
                    name: "Generate image".into(),
                    input: None,
                },
            },
        );
        for event in &events {
            sessions.inner.publish("chat", event);
            let AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } = event
            else {
                panic!("lost owner")
            };
            assert_eq!(parent_tool_use_id, "child");
            fold_event_into_parts(&mut parts, event);
        }
        assert!(matches!(
            parts[0],
            MessagePart::Tool {
                is_error: true,
                resolved: true,
                ..
            }
        ));
        assert!(matches!(parts[1], MessagePart::Error { .. }));
        let journal =
            serde_json::to_string(&sessions.inner.journal.replay("chat", 0).unwrap()).unwrap();
        assert!(!journal.contains(source));
        assert!(!journal.contains("secret"));
    }

    /// The backwards journal scan must answer exactly what the old full forward
    /// replay did: newest non-empty id, cwd from the governing `SessionStarted`.
    #[tokio::test]
    async fn journal_harness_session_matches_forward_scan() {
        fn started(id: &str, cwd: &str) -> AgentEvent {
            AgentEvent::SessionStarted {
                harness: zeron_proto::HarnessId::ClaudeCode,
                model: "m".into(),
                tools: Vec::new(),
                cwd: cwd.into(),
                session_id: id.into(),
                assistant_message_id: "a".into(),
            }
        }
        fn done(id: Option<&str>) -> AgentEvent {
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: id.map(Into::into),
            }
        }
        fn text() -> AgentEvent {
            AgentEvent::TextDelta { text: "t".into() }
        }
        /// The pre-change implementation, verbatim, over a forward replay.
        fn forward(events: Vec<(u64, AgentEvent)>) -> Option<(String, String)> {
            let mut current_cwd = String::new();
            let mut found: Option<(String, String)> = None;
            for (_, event) in events {
                match event {
                    AgentEvent::SessionStarted {
                        session_id, cwd, ..
                    } => {
                        current_cwd = cwd;
                        if !session_id.is_empty() {
                            found = Some((session_id, current_cwd.clone()));
                        }
                    }
                    AgentEvent::Done {
                        session_id: Some(session_id),
                        ..
                    } if !session_id.is_empty() => {
                        found = Some((session_id, current_cwd.clone()));
                    }
                    _ => {}
                }
            }
            found
        }

        /// (chat id, journaled events, expected `(session id, cwd)`).
        type Case = (
            &'static str,
            Vec<AgentEvent>,
            Option<(&'static str, &'static str)>,
        );
        let cases: Vec<Case> = vec![
            ("empty", vec![], None),
            ("no-ids", vec![text(), done(None), done(Some(""))], None),
            (
                "started-only",
                vec![started("s1", "/a"), text()],
                Some(("s1", "/a")),
            ),
            (
                "done-inherits-run-cwd",
                vec![started("s1", "/a"), text(), done(Some("s2"))],
                Some(("s2", "/a")),
            ),
            (
                "done-before-any-started",
                vec![text(), done(Some("s1"))],
                Some(("s1", "")),
            ),
            (
                "empty-started-sets-cwd-for-later-done",
                vec![started("s1", "/a"), started("", "/b"), done(Some("s2"))],
                Some(("s2", "/b")),
            ),
            (
                "later-empty-started-does-not-override",
                vec![
                    started("s1", "/a"),
                    done(Some("s2")),
                    started("", "/b"),
                    text(),
                ],
                Some(("s2", "/a")),
            ),
            (
                "newest-run-wins",
                vec![
                    started("s1", "/a"),
                    done(Some("s1")),
                    started("s3", "/c"),
                    text(),
                ],
                Some(("s3", "/c")),
            ),
            (
                "empty-done-ids-ignored",
                vec![started("s1", "/a"), done(Some("")), done(None)],
                Some(("s1", "/a")),
            ),
            (
                "only-empty-started-then-done-from-older-run",
                vec![
                    started("s1", "/a"),
                    done(Some("s9")),
                    started("", "/b"),
                    done(None),
                ],
                Some(("s9", "/a")),
            ),
        ];

        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionsEngine::new(
            "host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()),
            Arc::new(HarnessRegistry::new()),
        );
        for (chat, events, want) in cases {
            for event in &events {
                sessions.inner.journal.append(chat, event).unwrap();
            }
            let want = want.map(|(id, cwd)| (id.to_string(), cwd.to_string()));
            let replayed = sessions.inner.journal.replay(chat, 0).unwrap();
            assert_eq!(forward(replayed), want, "{chat}: reference");
            assert_eq!(
                sessions
                    .inner
                    .journal_harness_session(chat)
                    .map(|(id, cwd, _)| (id, cwd)),
                want,
                "{chat}"
            );
        }
        // Torn tail: the last complete SessionStarted still governs.
        let path = dir.path().join("journals").join("started-only.jsonl");
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut f, b"{\"seq\":3,\"event\":{\"type\":\"done\"").unwrap();
        assert_eq!(
            sessions.inner.journal_harness_session("started-only"),
            Some(("s1".into(), "/a".into(), Some(HarnessId::ClaudeCode)))
        );
    }

    fn request() -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: "first".into(),
            harness: None,
            model: Some("grok-4.6".into()),
            reasoning: Some(zeron_proto::ReasoningLevel::High),
            model_options: serde_json::Map::new(),
            cwd: "/tmp".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            access: Default::default(),
            plan: false,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
            env: Default::default(),
        }
    }

    struct IdleStartupFixture {
        idle_calls: Arc<std::sync::atomic::AtomicUsize>,
        inference_calls: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl zeron_harness::Harness for IdleStartupFixture {
        fn id(&self) -> HarnessId {
            HarnessId::Codex
        }
        fn display_name(&self) -> &str {
            "Offline idle fixture"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[]
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
            Ok(vec![])
        }
        async fn start_idle(
            &self,
            request: RunRequest,
            _controls: zeron_harness::RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            use futures::StreamExt;
            self.idle_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(request.prompt.is_empty());
            assert_eq!(request.resume.as_deref(), Some("old-native-thread"));
            let event = if self.fail {
                AgentEvent::Done {
                    status: zeron_proto::DoneStatus::Errored,
                    result: None,
                    error: Some("offline startup failed".into()),
                    session_id: None,
                }
            } else {
                AgentEvent::SessionStarted {
                    harness: HarnessId::Codex,
                    model: "fixture".into(),
                    tools: vec![],
                    cwd: request.cwd,
                    session_id: "new-native-thread".into(),
                    assistant_message_id: "fixture-empty".into(),
                }
            };
            Ok(futures::stream::iter([Ok(event)])
                .chain(futures::stream::pending())
                .boxed())
        }
        async fn run(
            &self,
            _request: RunRequest,
            _controls: zeron_harness::RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            self.inference_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(zeron_harness::HarnessError::Protocol(
                "fixture forbids inference".into(),
            ))
        }
    }

    async fn idle_startup_case(fail: bool) {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let idle_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inference_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry = crate::HarnessRegistry::new();
        registry.register(Arc::new(IdleStartupFixture {
            idle_calls: idle_calls.clone(),
            inference_calls: inference_calls.clone(),
            fail,
        }));
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            Arc::new(registry),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        core.workspace
            .create_chat("idle", None, Some(&core.device_id), None, None)
            .unwrap();
        let mut request = request();
        request.prompt.clear();
        core.sessions
            .inner
            .remember_harness_session(
                "idle",
                "old-native-thread",
                &request.cwd,
                HarnessId::Codex,
            );
        core.sessions.start_idle("idle", request).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let completed = if fail {
                    core.sessions
                        .session_status("idle")
                        .is_some_and(|s| s.status == SessionStatus::Errored)
                } else {
                    core.sessions
                        .inner
                        .resume_for("idle", "/tmp", HarnessId::Codex)
                        .as_deref()
                        == Some("new-native-thread")
                };
                if completed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(idle_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(inference_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let entries = core
            .doc_host
            .open("idle")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap();
        assert!(
            !entries
                .iter()
                .any(|entry| entry.role == zeron_doc::MessageRole::User)
        );
        if !fail {
            assert!(entries.is_empty());
            assert_eq!(
                core.sessions.session_status("idle").unwrap().status,
                SessionStatus::Idle
            );
            assert_eq!(
                core.workspace
                    .chat_harness_session("idle")
                    .unwrap()
                    .session_id,
                "new-native-thread"
            );
            let events = core.sessions.inner.journal.replay("idle", 0).unwrap();
            assert!(events.iter().any(|(_, event)| matches!(event, AgentEvent::SessionStarted { session_id, .. } if session_id == "new-native-thread")));
        }
        core.sessions.shutdown().await;
    }
    #[tokio::test]
    async fn voice_idle_startup_failure_never_retries_as_inference() {
        idle_startup_case(true).await;
    }
    #[tokio::test]
    async fn voice_idle_startup_remembers_thread_without_creating_messages() {
        idle_startup_case(false).await;
    }

    #[test]
    fn live_routing_requires_the_same_runtime_configuration() {
        let initial = request();
        let config = RuntimeConfig::new(
            HarnessId::Grok,
            &initial,
            Arc::new(zeron_harness::AcpHarness::grok()),
        );

        let mut follow_up = initial.clone();
        follow_up.prompt = "second".into();
        follow_up.resume = Some("session-1".into());
        assert!(config.can_route(HarnessId::Grok, &follow_up));
        assert!(!config.can_route(HarnessId::Devin, &follow_up));

        // Images ride the mailbox: they never force a replacement runtime.
        follow_up.attachments.push("/tmp/image.png".into());
        assert!(config.can_route(HarnessId::Grok, &follow_up));

        // Another client's spelling of the same option value is the same runtime.
        let mut spelled = initial.clone();
        spelled
            .model_options
            .insert("fastMode".into(), serde_json::json!(true));
        let config = RuntimeConfig::new(
            HarnessId::Grok,
            &spelled,
            Arc::new(zeron_harness::AcpHarness::grok()),
        );
        follow_up
            .model_options
            .insert("fastMode".into(), serde_json::json!("true"));
        assert!(config.can_route(HarnessId::Grok, &follow_up));

        follow_up.model = Some("grok-4.5".into());
        assert!(!config.can_route(HarnessId::Grok, &follow_up));
        follow_up.model = initial.model.clone();

        follow_up.reasoning = Some(zeron_proto::ReasoningLevel::Medium);
        assert!(!config.can_route(HarnessId::Grok, &follow_up));
    }

    #[test]
    fn claude_routing_compares_what_the_cli_launches_with() {
        let claude = || -> Arc<dyn Harness> { Arc::new(zeron_harness::ClaudeHarness::new()) };
        let mut initial = request();
        initial.model = Some("claude-opus-5-5".into());
        initial
            .model_options
            .insert("fastMode".into(), serde_json::json!(false));
        let config = RuntimeConfig::new(HarnessId::ClaudeCode, &initial, claude());

        // The CLI never reads the sandbox level; an option that launches no
        // flag in either spelling is the same runtime.
        let mut follow_up = initial.clone();
        follow_up.sandbox = SandboxLevel::DangerFullAccess;
        follow_up.model_options.clear();
        follow_up.worktree = None;
        assert!(config.can_route(HarnessId::ClaudeCode, &follow_up));

        follow_up
            .model_options
            .insert("fastMode".into(), serde_json::json!("true"));
        assert!(!config.can_route(HarnessId::ClaudeCode, &follow_up));
        follow_up.model_options.clear();

        follow_up.cwd = "/elsewhere".into();
        assert!(!config.can_route(HarnessId::ClaudeCode, &follow_up));
    }

    #[test]
    fn clean_tool_ids_ride_verbatim_and_fit_id_re() {
        let id = subagent_doc_id("chat-abc123", "toolu_01EjFLnNhCiMR2PBNKcVvkT");
        assert_eq!(id, "chat-abc123--sub--toolu_01EjFLnNhCiMR2PBNKcVvkT");
        assert!(id.len() <= 128);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn unclean_or_oversized_ids_hash_deterministically() {
        let dirty = subagent_doc_id("chat", "call/with:odd chars");
        assert!(
            dirty
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{dirty}"
        );
        assert_eq!(dirty, subagent_doc_id("chat", "call/with:odd chars"));
        let long_chat = "c".repeat(100);
        let long = subagent_doc_id(&long_chat, &"t".repeat(64));
        assert!(long.len() <= 128, "{}", long.len());
        // Distinct inputs stay distinct.
        assert_ne!(
            subagent_doc_id("chat", "a:b"),
            subagent_doc_id("chat", "a:c")
        );
    }

    #[tokio::test]
    async fn subagent_sink_flush_clears_dirty_when_nothing_folded() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            crate::doc_host::DocHostConfig {
                device_id: "dev-test".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("chat-sub-flush--sub--t1").unwrap();
        let mut sink = SubagentSink {
            doc_id: "chat-sub-flush--sub--t1".into(),
            doc: handle.writer(),
            entry_id: "entry-1".into(),
            started_at: 0,
            entry_index: None,
            written: Vec::new(),
            folded: Vec::new(),
            dirty: true,
        };
        // Nothing to write is still a completed flush — left set, the flag
        // holds the run loop's commit guard open on a past deadline.
        sink.flush("dev-test");
        assert!(!sink.dirty);
    }

    /// Feed-by-hand harness: the test pushes wire events through a channel
    /// and decides when the stream ends.
    struct FeedHarness {
        id: HarnessId,
        feed: Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
        steering: Option<mpsc::UnboundedSender<mpsc::Receiver<SteerMessage>>>,
    }

    #[async_trait::async_trait]
    impl Harness for FeedHarness {
        fn id(&self) -> HarnessId {
            self.id
        }
        fn display_name(&self) -> &str {
            "Feed"
        }
        fn supports_steering(&self) -> bool {
            true
        }
        fn deterministic_turn_end(&self) -> bool {
            self.id == HarnessId::Omp
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::StepBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[zeron_proto::ReasoningLevel::Medium]
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
            Ok(vec![])
        }
        async fn run(
            &self,
            _request: RunRequest,
            controls: RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            let mut feed = self
                .feed
                .lock()
                .unwrap()
                .take()
                .expect("FeedHarness serves one run per test");
            if let Some(steering) = &self.steering {
                steering.send(controls.steering).unwrap();
            }
            Ok(futures::stream::poll_fn(move |cx| feed.poll_recv(cx).map(|e| e.map(Ok))).boxed())
        }
    }

    #[tokio::test]
    async fn namespace_markers_follow_turns_and_inference_not_parked_harnesses() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let markers = dir.path().join("tasks");
        std::fs::create_dir(&markers).unwrap();
        let (feed, rx) = mpsc::unbounded_channel();
        let (mailboxes, mut harness_calls) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness { id: HarnessId::Mock, feed: Mutex::new(Some(rx)), steering: Some(mailboxes) }));
        let idle_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        registry.register(Arc::new(IdleStartupFixture {
            idle_calls: idle_calls.clone(),
            inference_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail: false,
        }));
        let mut sessions = SessionsEngine::new("host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry));
        let inner = Arc::get_mut(&mut sessions.inner).unwrap();
        inner.boat_lease = BoatLease::new(Ok(None),
            Arc::new(|_, _| Box::pin(async { panic!("Namespace invoked Boat protection") })));
        inner.namespace_tasks = crate::namespace_tasks::tests::tasks_in(&markers);
        sessions.set_doc_host(DocHost::new(Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).unwrap()),
            crate::doc_host::DocHostConfig { device_id: "host".into(), default_harness: HarnessId::Mock, edge: None }));
        sessions.inner.remember_harness_session(
            "prewarm",
            "old-native-thread",
            "/tmp",
            HarnessId::Codex,
        );
        let mut idle = request();
        idle.prompt.clear();
        sessions.start_idle("prewarm", idle).await.unwrap();
        while idle_calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let marker_count = || std::fs::read_dir(&markers).unwrap().count();
        assert_eq!(marker_count(), 0, "prewarm must not mark active work");

        let run = sessions.dispatch("chat", HarnessId::Mock, request(), Some("first".into())).await.unwrap();
        let mut mailbox = harness_calls.recv().await.unwrap();
        assert_eq!(marker_count(), 1);
        let done = || AgentEvent::Done { status: DoneStatus::Completed, result: None, error: None, session_id: None };
        async fn parked(sessions: &SessionsEngine) {
            let mut statuses = sessions.watch_sessions();
            while !statuses.borrow().iter().any(|s| s.chat_id == "chat" && s.status == SessionStatus::Idle) {
                statuses.changed().await.unwrap();
            }
        }
        feed.send(done()).unwrap();
        parked(&sessions).await;
        assert_eq!(marker_count(), 0);
        let stale = sessions.inner.inference_admission("chat", "stale-run", &CancellationToken::new()).unwrap();
        assert!((stale.check)().await.is_err());
        assert_eq!(marker_count(), 0);
        let interrupt = lock(&sessions.inner.runs).get("chat").unwrap().interrupt_token.clone();
        let gate = sessions.inner.inference_admission("chat", &run, &interrupt).unwrap();
        let activity = (gate.check)().await.unwrap();
        assert_eq!(marker_count(), 1, "autonomous inference owns its marker before output");
        assert!(sessions.any_active());
        feed.send(done()).unwrap();
        parked(&sessions).await;
        assert_eq!(marker_count(), 1, "Done cannot release an in-flight response");
        drop(activity);
        assert_eq!(marker_count(), 0);

        assert_eq!(sessions.steer("chat", "next", Some("next".into())).await.unwrap(), SteerOutcome::Accepted);
        assert_eq!(mailbox.recv().await.unwrap().prompt, "next");
        assert_eq!(marker_count(), 1);
        feed.send(AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: None }).unwrap();
        feed.send(done()).unwrap();
        parked(&sessions).await;
        assert_eq!(marker_count(), 0);
        std::fs::remove_dir(&markers).unwrap();
        assert!(sessions.steer("chat", "denied", Some("denied".into())).await.is_err());
        assert!(mailbox.try_recv().is_err(), "failed marker creation admitted a steer");
        std::fs::create_dir(&markers).unwrap();
        let activity = (gate.check)().await.unwrap();
        assert_eq!(marker_count(), 1);
        sessions.inner.retire_run("chat", &run);
        assert!((gate.check)().await.is_err());
        drop(activity);
        drop(feed);
        sessions.shutdown().await;
        assert_eq!(marker_count(), 0);
    }

    #[tokio::test]
    async fn terminal_retirement_rejects_a_steer_already_waiting_for_mailbox_capacity() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let (feed, rx) = mpsc::unbounded_channel();
        let (mailboxes, mut harness_calls) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness { id: HarnessId::Mock, feed: Mutex::new(Some(rx)), steering: Some(mailboxes) }));
        let mut sessions = SessionsEngine::new("host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry));
        Arc::get_mut(&mut sessions.inner).unwrap().boat_lease = BoatLease::new(Ok(None),
            Arc::new(|_, _| Box::pin(async { panic!("non-Boat test invoked the provider") })));
        sessions.set_doc_host(DocHost::new(Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).unwrap()),
            crate::doc_host::DocHostConfig { device_id: "host".into(), default_harness: HarnessId::Mock, edge: None }));
        let run = sessions.dispatch("chat", HarnessId::Mock, request(), Some("initial".into())).await.unwrap();
        let mut mailbox = harness_calls.recv().await.unwrap();
        feed.send(AgentEvent::Done { status: DoneStatus::Completed, result: None, error: None, session_id: None }).unwrap();
        let mut statuses = sessions.watch_sessions();
        while !statuses.borrow().iter().any(|s| s.chat_id == "chat" && s.status == SessionStatus::Idle) {
            statuses.changed().await.unwrap();
        }
        // A logical Done parks, not retires. A stale task cannot retire this run.
        assert!(sessions.inner.retire_run("chat", "obsolete-run").is_empty());
        assert_eq!(sessions.steer("chat", "before exit", Some("accepted".into())).await.unwrap(), SteerOutcome::Accepted);
        assert_eq!(mailbox.recv().await.unwrap().prompt, "before exit");
        let sender = lock(&sessions.inner.runs).get("chat").unwrap().steer_tx.clone();
        for _ in 0..32 {
            sender.try_send(SteerMessage { prompt: "backpressure".into(), message_id: None, attachments: vec![], config: None }).unwrap();
        }
        let racing = sessions.steer("chat", "racing exit", Some("racing".into()));
        tokio::pin!(racing);
        assert!(futures::poll!(&mut racing).is_pending());
        let orphans = sessions.inner.retire_run("chat", &run);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].message_id, "accepted");
        // Publication can wake a queue consumer while the exact retiring handle
        // is still installed. The already-reserved sender must re-check it too.
        sessions.inner.set_status_with_completion(
            "chat",
            Some(&run),
            SessionStatus::Idle,
            false,
            None,
            None,
        );
        assert_eq!(mailbox.recv().await.unwrap().prompt, "backpressure");
        assert_eq!(racing.await.unwrap(), SteerOutcome::NotSteerable);
        assert_eq!(sessions.steer("chat", "after idle", Some("after".into())).await.unwrap(), SteerOutcome::NotSteerable);
        assert!(lock(&lock(&sessions.inner.runs).get("chat").unwrap().routed_steers).is_empty());
        while let Ok(message) = mailbox.try_recv() { assert_eq!(message.prompt, "backpressure"); }
        assert!(sessions.inner.retire_run("chat", &run).is_empty());
        drop(feed);
        sessions.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn boat_admission_precedes_harness_and_both_parked_mailbox_paths() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let (commands_tx, mut commands) = mpsc::unbounded_channel::<oneshot::Sender<Result<(), String>>>();
        let (feed, rx) = mpsc::unbounded_channel();
        let (mailboxes, mut harness_calls) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness { id: HarnessId::Mock, feed: Mutex::new(Some(rx)), steering: Some(mailboxes) }));
        let idle_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        registry.register(Arc::new(IdleStartupFixture {
            idle_calls: idle_calls.clone(),
            inference_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail: false,
        }));
        let mut sessions = SessionsEngine::new(
            "host".into(), Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry),
        );
        Arc::get_mut(&mut sessions.inner).unwrap().boat_lease = BoatLease::new(
            Ok(Some("bx_23456789".into())), Arc::new(move |_, cancel| {
                let (reply, result) = oneshot::channel();
                commands_tx.send(reply).unwrap();
                Box::pin(async move {
                    tokio::select! {
                        result = result => result.unwrap(),
                        _ = cancel.cancelled() => Err("cancelled".into()),
                    }
                })
            }),
        );
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).unwrap());
        sessions.set_doc_host(DocHost::new(store, crate::doc_host::DocHostConfig {
            device_id: "host".into(), default_harness: HarnessId::Mock, edge: None,
        }));
        sessions.inner.remember_harness_session(
            "prewarm",
            "old-native-thread",
            "/tmp",
            HarnessId::Codex,
        );
        let mut idle = request();
        idle.prompt.clear();
        sessions.start_idle("prewarm", idle.clone()).await.unwrap();
        while idle_calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(std::time::Duration::from_secs(600)).await;
        assert!(commands.try_recv().is_err(), "idle prewarm requested Boat protection");
        let rejected = sessions.dispatch("chat", HarnessId::Mock, request(), Some("rejected".into()));
        tokio::pin!(rejected);
        tokio::select! {
            reply = commands.recv() => reply.unwrap().send(Err("denied".into())).unwrap(),
            _ = &mut rejected => panic!("dispatch bypassed protection"),
        }
        assert!(rejected.await.is_err());
        assert!(harness_calls.try_recv().is_err());

        let dispatch = sessions.dispatch("chat", HarnessId::Mock, request(), Some("first".into()));
        tokio::pin!(dispatch);
        tokio::select! {
            reply = commands.recv() => reply.unwrap().send(Ok(())).unwrap(),
            _ = &mut dispatch => panic!("harness admitted before confirmation"),
        }
        let live_run = dispatch.await.unwrap();
        let mut steering = harness_calls.recv().await.unwrap();
        let events_before = sessions.inner.journal.replay("chat", 0).unwrap().len();
        sessions.inner.abandon_retry("chat", true);
        assert_eq!(sessions.inner.journal.replay("chat", 0).unwrap().len(), events_before);
        assert!(
            sessions
                .inner
                .journal
                .shutdown_interrupts()
                .unwrap()
                .into_iter()
                .all(|(chat_id, _)| chat_id != "chat"),
            "a stopped retry must not journal over the replacement run"
        );
        assert_eq!(
            sessions.session_status("chat").unwrap().status,
            SessionStatus::Working
        );
        // A retired task can finish after the bounded terminate wait and after
        // a replacement owns this chat. Neither status nor TTL belongs to it.
        sessions.inner.remove_run("chat", "retired-run");
        sessions.inner.set_status_with_completion(
            "chat", Some("retired-run"), SessionStatus::Idle, false, Some("stale-completion".into()),
            None,
        );
        sessions.inner.set_run_status("chat", "retired-run", SessionStatus::Working, true);
        sessions.inner.set_status("chat", SessionStatus::Errored, false);
        let current = sessions.session_status("chat").unwrap();
        assert_eq!(current.status, SessionStatus::Working);
        assert_eq!(current.last_completed_turn, None);
        assert!(sessions.inner.boat_lease.active(&live_run));

        async fn parked(sessions: &SessionsEngine) {
            let mut statuses = sessions.watch_sessions();
            while !statuses.borrow().iter().any(|s| s.chat_id == "chat" && s.status == SessionStatus::Idle) {
                statuses.changed().await.unwrap();
            }
        }
        let done = || AgentEvent::Done { status: DoneStatus::Completed, result: None, error: None, session_id: None };
        feed.send(done()).unwrap();
        parked(&sessions).await;
        commands.recv().await.unwrap().send(Ok(())).unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(600)).await;
        assert!(commands.try_recv().is_err(), "parked runtime renewed");
        let stale = sessions.inner.inference_admission("chat", "retired-run", &CancellationToken::new()).unwrap();
        assert!((stale.check)().await.is_err());
        assert!(commands.try_recv().is_err(), "stale inference touched the provider");
        let interrupt = lock(&sessions.inner.runs).get("chat").unwrap().interrupt_token.clone();
        let autonomous = sessions.inner.inference_admission("chat", &live_run, &interrupt).unwrap();
        let admission = (autonomous.check)();
        tokio::pin!(admission);
        tokio::select! {
            reply = commands.recv() => reply.unwrap().send(Ok(())).unwrap(),
            _ = &mut admission => panic!("autonomous inference bypassed Boat confirmation"),
        }
        let activity = admission.await.unwrap();
        assert!(sessions.inner.boat_lease.active(&live_run));
        assert!(sessions.inner.inference_in_flight("chat", &live_run));
        assert_eq!(sessions.session_status("chat").unwrap().status, SessionStatus::Working);
        // No TextDelta/ToolCall has occurred. The request already owns protection.
        tokio::time::advance(std::time::Duration::from_secs(300)).await;
        commands.recv().await.unwrap().send(Ok(())).unwrap();
        tokio::task::yield_now().await;
        feed.send(done()).unwrap();
        parked(&sessions).await;
        assert!(sessions.inner.boat_lease.active(&live_run), "Done released an in-flight proxy stream");
        drop(activity);
        commands.recv().await.unwrap().send(Ok(())).unwrap();
        tokio::task::yield_now().await;
        assert!(!sessions.inner.boat_lease.active(&live_run));

        let steer = sessions.steer("chat", "explicit", Some("second".into()));
        tokio::pin!(steer);
        let reply = tokio::select! {
            reply = commands.recv() => reply.unwrap(),
            _ = &mut steer => panic!("idle steer bypassed renewed protection"),
        };
        assert!(steering.try_recv().is_err());
        reply.send(Ok(())).unwrap();
        assert_eq!(steer.await.unwrap(), SteerOutcome::Accepted);
        assert_eq!(steering.recv().await.unwrap().prompt, "explicit");
        feed.send(AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: None }).unwrap();
        feed.send(done()).unwrap();
        parked(&sessions).await;
        commands.recv().await.unwrap().send(Ok(())).unwrap();
        tokio::task::yield_now().await;

        let mut warm = request();
        warm.prompt = "warm".into();
        let dispatch = sessions.dispatch("chat", HarnessId::Mock, warm, Some("third".into()));
        tokio::pin!(dispatch);
        let reply = tokio::select! {
            reply = commands.recv() => reply.unwrap(),
            _ = &mut dispatch => panic!("warm dispatch bypassed renewed protection"),
        };
        assert!(steering.try_recv().is_err());
        reply.send(Ok(())).unwrap();
        dispatch.await.unwrap();
        assert_eq!(steering.recv().await.unwrap().prompt, "warm");
        feed.send(AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: None }).unwrap();
        feed.send(done()).unwrap();
        parked(&sessions).await;
        commands.recv().await.unwrap().send(Ok(())).unwrap();
        tokio::task::yield_now().await;
        drop(feed);
        sessions.shutdown().await;
        assert!(sessions.start_idle("after-shutdown", idle).await.is_err());
    }


    #[tokio::test(start_paused = true)]
    async fn cancelling_native_start_during_protection_is_interrupted_not_errored() {
        native_start_protection_outcome(false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn native_start_ready_provider_error_takes_precedence_over_cancellation() {
        native_start_protection_outcome(true).await;
    }

    async fn native_start_protection_outcome(provider_fails: bool) {
        let dir = tempfile::tempdir().unwrap();
        let (feed, rx) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness {
            id: HarnessId::Omp, feed: Mutex::new(Some(rx)), steering: None,
        }));
        let mut sessions = SessionsEngine::new(
            "host".into(), Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry),
        );
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let called = Arc::new(tokio::sync::Notify::new());
        let provider_calls = calls.clone();
        let provider_called = called.clone();
        let (failure, failure_rx) = oneshot::channel::<Result<(), String>>();
        let failure_rx = Arc::new(Mutex::new(Some(failure_rx)));
        Arc::get_mut(&mut sessions.inner).unwrap().boat_lease = BoatLease::new(
            Ok(Some("bx_23456789".into())), Arc::new(move |_, cancel| {
                let call = provider_calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                provider_called.notify_one();
                let failure = (provider_fails && call >= 2).then(|| lock(&failure_rx).take().unwrap());
                Box::pin(async move {
                    if call < 2 { return Ok(()); }
                    if let Some(failure) = failure { return failure.await.unwrap(); }
                    cancel.cancelled().await;
                    Err("provider request cancelled".into())
                })
            }),
        );
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).unwrap());
        sessions.set_doc_host(DocHost::new(store, crate::doc_host::DocHostConfig {
            device_id: "host".into(), default_harness: HarnessId::Omp, edge: None,
        }));
        let run = sessions.dispatch("chat", HarnessId::Omp, request(), None).await.unwrap();
        let interrupt = lock(&sessions.inner.runs)["chat"].interrupt_token.clone();
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed, result: None, error: None, session_id: None,
        }).unwrap();
        sessions.watch_sessions().wait_for(|rows| rows.iter().any(|row|
            row.chat_id == "chat" && row.status == SessionStatus::Idle)).await.unwrap();
        // Initial admission and its idle TTL reset finish before the held request.
        while calls.load(std::sync::atomic::Ordering::Acquire) < 2 { called.notified().await; }
        feed.send(AgentEvent::TurnStarted).unwrap();
        while calls.load(std::sync::atomic::Ordering::Acquire) < 3 { called.notified().await; }
        if provider_fails {
            // Make both select branches ready before the driver is polled.
            interrupt.cancel();
            failure.send(Err("denied".into())).unwrap();
            sessions.watch_sessions().wait_for(|rows| rows.iter().any(|row|
                row.chat_id == "chat" && row.status == SessionStatus::Errored)).await.unwrap();
        } else {
            assert!(sessions.terminate("chat").await.unwrap());
        }
        assert!(!sessions.is_live("chat", &run));
        assert_eq!(sessions.session_status("chat").unwrap().status,
            if provider_fails { SessionStatus::Errored } else { SessionStatus::Idle });
        let Some((_, AgentEvent::Done { status, error, .. })) = sessions.inner.journal.last_event("chat").unwrap() else {
            panic!("native admission did not journal its completion");
        };
        assert_eq!(status, if provider_fails { DoneStatus::Errored } else { DoneStatus::Interrupted });
        assert_eq!(error.is_some(), provider_fails);
        assert!(!sessions.inner.boat_lease.active(&run));
        let entries = sessions.doc_handle("chat").unwrap().doc().read_entries().unwrap();
        assert_eq!(entries.iter().flat_map(|entry| &entry.parts).any(|part|
            matches!(part, MessagePart::Error { .. })), provider_fails);
        sessions.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn native_auxiliary_inference_releases_lease_without_reopening_completed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (feed, rx) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness {
            id: HarnessId::Omp, feed: Mutex::new(Some(rx)), steering: None,
        }));
        let mut sessions = SessionsEngine::new(
            "host".into(), Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry),
        );
        let allow = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let provider_allow = allow.clone();
        Arc::get_mut(&mut sessions.inner).unwrap().boat_lease = BoatLease::new(
            Ok(Some("bx_23456789".into())), Arc::new(move |_, _| {
                let allowed = provider_allow.load(std::sync::atomic::Ordering::Acquire);
                Box::pin(async move { if allowed { Ok(()) } else { Err("denied".into()) } })
            }),
        );
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).unwrap());
        sessions.set_doc_host(DocHost::new(store, crate::doc_host::DocHostConfig {
            device_id: "host".into(), default_harness: HarnessId::Omp, edge: None,
        }));
        let run = sessions.dispatch("chat", HarnessId::Omp, request(), None).await.unwrap();
        let (_, mut events) = sessions.subscribe("chat", 0).unwrap();
        let (interrupt, token) = {
            let runs = lock(&sessions.inner.runs);
            let handle = &runs["chat"];
            (handle.interrupt_token.clone(), handle.supervision_token.clone())
        };
        let supervision = sessions.retain_supervision("chat", &token).unwrap();
        feed.send(AgentEvent::TextDelta { text: "complete".into() }).unwrap();
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed, result: None, error: None, session_id: None,
        }).unwrap();
        sessions.watch_sessions().wait_for(|rows| rows.iter().any(|row|
            row.chat_id == "chat" && row.status == SessionStatus::Idle)).await.unwrap();
        let completed = sessions.session_status("chat").unwrap().last_completed_turn;
        assert!(completed.is_some());
        assert!(!sessions.inner.boat_lease.active(&run));

        // OMP compresses skill descriptions outside the foreground agent loop.
        // Those requests own protection, but never owe another native Done.
        let admission = sessions.inner.inference_admission("chat", &run, &interrupt).unwrap();
        let first = (admission.check)().await.unwrap();
        let second = (admission.check)().await.unwrap();
        assert!(sessions.inner.boat_lease.active(&run));
        assert!(sessions.inner.inference_in_flight("chat", &run));
        assert_eq!(sessions.session_status("chat").unwrap().status, SessionStatus::Idle);
        drop(first);
        assert!(sessions.inner.boat_lease.active(&run), "overlapping auxiliary stream lost protection");
        drop(second);
        tokio::task::yield_now().await;
        let status = sessions.session_status("chat").unwrap();
        assert_eq!(status.status, SessionStatus::Idle);
        assert_eq!(status.last_completed_turn, completed);
        assert!(!sessions.inner.boat_lease.active(&run));
        assert!(!sessions.inner.inference_in_flight("chat", &run));
        assert!(sessions.is_live("chat", &run), "supervised warm runtime was retired");

        async fn wait_status(sessions: &SessionsEngine, status: SessionStatus) {
            sessions.watch_sessions().wait_for(|rows| rows.iter().any(|row|
                row.chat_id == "chat" && row.status == status)).await.unwrap();
        }
        let done = || AgentEvent::Done {
            status: DoneStatus::Completed, result: None, error: None, session_id: None,
        };
        // An autonomous native start must protect work before any model request.
        feed.send(AgentEvent::TurnStarted).unwrap();
        wait_status(&sessions, SessionStatus::Working).await;
        assert!(sessions.inner.boat_lease.active(&run));
        assert!(!sessions.inner.inference_in_flight("chat", &run));
        let foreground = (admission.check)().await.unwrap();
        let auxiliary = (admission.check)().await.unwrap();
        drop(foreground);
        drop(auxiliary);
        assert_eq!(sessions.session_status("chat").unwrap().status, SessionStatus::Working);
        assert!(sessions.inner.boat_lease.active(&run), "model EOF ended a native foreground turn");
        let lingering = (admission.check)().await.unwrap();
        feed.send(done()).unwrap();
        wait_status(&sessions, SessionStatus::Idle).await;
        assert!(sessions.inner.boat_lease.active(&run), "native Done released a live response");
        drop(lingering);
        assert!(!sessions.inner.boat_lease.active(&run));

        // Exercise the final auxiliary Drop on both sides of native admission.
        // The runs lock makes protection + Working publication indivisible.
        for drop_before_start in [true, false] {
            let auxiliary = (admission.check)().await.unwrap();
            feed.send(AgentEvent::TurnStarted).unwrap();
            if drop_before_start {
                drop(auxiliary);
                wait_status(&sessions, SessionStatus::Working).await;
            } else {
                wait_status(&sessions, SessionStatus::Working).await;
                drop(auxiliary);
            }
            assert!(sessions.inner.boat_lease.active(&run));
            assert!(!sessions.inner.inference_in_flight("chat", &run));
            feed.send(done()).unwrap();
            wait_status(&sessions, SessionStatus::Idle).await;
            assert!(!sessions.inner.boat_lease.active(&run));
        }
        allow.store(false, std::sync::atomic::Ordering::Release);
        feed.send(AgentEvent::TurnStarted).unwrap();
        wait_status(&sessions, SessionStatus::Errored).await;
        assert!(interrupt.is_cancelled(), "unprotected native start was not interrupted");
        assert!(!sessions.inner.boat_lease.active(&run));
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event.event, AgentEvent::TurnStarted), "native control escaped into the event stream");
        }
        drop(supervision);
        sessions.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn supervision_subscription_retains_exact_run_until_last_drop() {
        async fn park(core: &crate::EngineCore, registry: &HarnessRegistry) -> (mpsc::UnboundedSender<AgentEvent>, String, String) {
            let (feed, rx) = mpsc::unbounded_channel();
            registry.register(Arc::new(FeedHarness { id: HarnessId::Mock, feed: Mutex::new(Some(rx)), steering: None }));
            let run = core.sessions.dispatch("supervised", HarnessId::Mock, request(), None).await.unwrap();
            feed.send(AgentEvent::Done {
                status: DoneStatus::Completed, result: None, error: None, session_id: None,
            }).unwrap();
            core.sessions.watch_sessions().wait_for(|rows| rows.iter().any(|row|
                row.chat_id == "supervised" && row.status == SessionStatus::Idle)).await.unwrap();
            let token = lock(&core.sessions.inner.runs)["supervised"].supervision_token.clone();
            (feed, run, token)
        }
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        let core = crate::EngineCore::assemble(dir.path(), registry.clone(), HarnessId::Mock, None).unwrap();
        let client = zeron_rpc::memory_client(core.rpc_service());
        let method = zeron_rpc::methods::RETAIN_SESSION_SUPERVISION;
        let elapsed = std::time::Duration::from_secs(31 * 60);
        let (_feed, ordinary, expired) = park(&core, &registry).await;
        let retired = lock(&core.sessions.inner.runs)["supervised"].interrupt_token.clone();
        tokio::time::advance(elapsed).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), retired.cancelled()).await.unwrap();
        assert!(!core.sessions.is_live("supervised", &ordinary));

        let (_feed, run, token) = park(&core, &registry).await;
        for (chat, capability) in [("other", token.as_str()), ("supervised", expired.as_str())] {
            assert!(client.subscribe_checked(method, serde_json::json!({"chatId": chat, "token": capability})).await.is_err());
        }
        let params = serde_json::json!({"chatId": "supervised", "token": token});
        let mut first = client.subscribe_checked(method, params.clone()).await.unwrap();
        let mut last = client.subscribe_checked(method, params.clone()).await.unwrap();
        assert_eq!(first.recv().await.unwrap(), serde_json::json!({"chatId":"supervised", "runId":run, "retained":true}));
        assert!(last.recv().await.unwrap()["retained"].as_bool().unwrap());
        let retired = lock(&core.sessions.inner.runs)["supervised"].interrupt_token.clone();
        registry.begin_update(HarnessId::Mock);
        tokio::time::advance(elapsed).await;
        tokio::task::yield_now().await;
        assert!(core.sessions.is_live("supervised", &run));
        drop(first);
        tokio::task::yield_now().await;
        assert!(core.sessions.is_live("supervised", &run));
        registry.end_update(HarnessId::Mock);
        drop(last);
        tokio::time::timeout(std::time::Duration::from_secs(1), retired.cancelled()).await.unwrap();
        assert!(!core.sessions.is_live("supervised", &run));

        let (feed, _, token) = park(&core, &registry).await;
        let stale = core.sessions.retain_supervision("supervised", &token).unwrap();
        let mut stream = client.subscribe_checked(method, serde_json::json!({"chatId":"supervised", "token":token})).await.unwrap();
        stream.recv().await.unwrap();
        drop(feed); // Harness stream death ends even a retained run and its RPC stream.
        assert!(stream.recv().await.is_none());
        assert!(stale.interrupt.is_cancelled());
        assert!(core.sessions.retain_supervision("supervised", &token).is_err());
        let (_feed, replacement, new_token) = park(&core, &registry).await;
        let current = core.sessions.retain_supervision("supervised", &new_token).unwrap();
        drop(stale);
        tokio::time::advance(elapsed).await;
        tokio::task::yield_now().await;
        assert!(core.sessions.is_live("supervised", &replacement));
        core.sessions.shutdown().await;
        assert!(current.interrupt.is_cancelled());
        assert!(core.sessions.retain_supervision("supervised", &new_token).is_err());
    }

    // A tagged subagent event that folds to NO parts used to leave its sink
    // dirty forever: the commit branch's guard stayed true on a deadline in
    // the past and the run loop burned a core until real content arrived.
    #[tokio::test]
    async fn empty_subagent_events_commit_once_per_window_not_per_tick() {
        use super::*;
        FLUSH_TICKS.with(|t| t.set(0));
        let (feed, rx) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness {
            id: HarnessId::Mock,
            feed: Mutex::new(Some(rx)),
            steering: None,
        }));
        let dir = tempfile::tempdir().unwrap();
        let core =
            crate::EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
                .unwrap();
        let chat = "chat-sink-spin";
        core.sessions
            .dispatch(chat, HarnessId::Mock, request(), None)
            .await
            .unwrap();
        feed.send(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: "/tmp".into(),
            session_id: "hs-1".into(),
            assistant_message_id: "a1".into(),
        })
        .unwrap();
        // A background subagent's shape: its spawn's turn ends, and it speaks
        // after the parent's last commit window, so its sink wakes with the
        // parent clean, parked and past its deadline.
        feed.send(AgentEvent::ToolCall {
            id: "tool-1".into(),
            call: zeron_proto::ToolCall::Unknown {
                name: "Agent: probe".into(),
                input: None,
            },
        })
        .unwrap();
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        })
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(3 * STREAM_COMMIT_MS)).await;
        // Empty events trickling in inside one window: the first must arm it
        // rather than commit, and no later one commits on its own.
        for _ in 0..12 {
            feed.send(AgentEvent::Subagent {
                parent_tool_use_id: "tool-1".into(),
                event: Box::new(AgentEvent::ReasoningDelta {
                    text: String::new(),
                }),
            })
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Well past several commit windows with the stream still open: the
        // loop must idle between commits, not spin on a past deadline.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let ticks = FLUSH_TICKS.with(|t| t.get());
        assert!(
            (1..=8).contains(&ticks),
            "commit branch fired {ticks} times in 1s for 12 empty subagent events"
        );
        assert_eq!(
            core.sessions
                .session_status(chat)
                .map(|session| (session.status, session.running_subagents)),
            Some((SessionStatus::Idle, 1)),
            "parked, with the subagent's sink open"
        );
        core.sessions.shutdown().await;
    }

    #[tokio::test]
    async fn imported_interrupted_request_is_admitted_once_without_prompt_reconstruction() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let workspace = crate::WorkspaceHost::open(store.clone(), crate::workspace_host::WorkspaceHostConfig {
            device_id: "destination".into(), device_name: "Host".into(), platform: "linux".into(),
            org_id: "project".into(), user_id: "owner".into(), edge: None, local_only: false,
            owner_registry: true,
        }).unwrap();
        let chat_id = "local-chat-recovery";
        workspace.create_chat(chat_id, None, Some("destination"), None, Some("/original/worktree".into())).unwrap();
        let mut request = request();
        request.harness = Some(HarnessId::Omp);
        request.resume = Some("exact-native-session".into());
        request.cwd = "/original/worktree".into();
        request.prompt = "exact interrupted request, not the transcript tail".into();
        request.attachments = vec!["/original/profile/uploads/image.png".into()];
        request.model_options.insert("crewAccountId".into(), "exact-account".into());
        let mut chat = workspace.chat(chat_id).unwrap().unwrap();
        chat.harness_session_id = request.resume.clone();
        chat.harness_session_cwd = Some(request.cwd.clone());
        chat.config = Some(zeron_proto::ChatConfig {
            harness: HarnessId::Omp, model: request.model.clone(), reasoning: request.reasoning,
            model_options: request.model_options.clone(), sandbox: request.sandbox,
            access: request.access, plan: request.plan,
        });
        workspace.import_chat_row(&chat).unwrap();
        let host = DocHost::new(store.clone(), crate::DocHostConfig {
            device_id: "destination".into(), default_harness: HarnessId::Omp, edge: None,
        });
        host.set_workspace(workspace);
        let handle = host.open(chat_id).unwrap();
        handle.write_user_message("original-user", "transcript is not request reconstruction", now_ms()).unwrap();
        let journal = RunJournal::open(dir.path().join("journals")).unwrap();
        journal.append(chat_id, &AgentEvent::Done {
            status: DoneStatus::Interrupted, result: None, error: None, session_id: request.resume.clone(),
        }).unwrap();
        assert!(journal.stale_sessions().unwrap().is_empty());
        let mut intent = zeron_sync::crew_import::ImportedRecoveryIntent {
            command_id: "legacy-recovery-exact-command".into(), chat_id: chat_id.into(),
            source_chat_id: chat_id.into(), source_device_id: "source".into(), destination_device_id: "destination".into(),
            owner_subject: "owner".into(), project_id: "project".into(), deployment_id: Some("staging".into()),
            native_session_id: "exact-native-session".into(), harness: HarnessId::Omp, request: request.clone(),
            agent_account_id: Some("exact-account".into()), user_message_id: "original-user".into(),
            updated_at: now_ms(), journal_seq: 0, source_file: "legacy-journals/exact.recovery".into(), source_sha256: "source-evidence".into(),
        };
        let scope = zeron_proto::CollaborationScope {
            project_id: "project".into(), deployment_id: Some("staging".into()), session_id: None,
            unknown: Default::default(),
        };
        assert!(admit_imported_recovery(&host, &intent, "other-device", "owner", &scope).is_err());
        assert!(admit_imported_recovery(&host, &intent, "destination", "other-owner", &scope).is_err());
        assert!(admit_imported_recovery(&host, &intent, "destination", "owner", &scope).unwrap());
        assert!(!admit_imported_recovery(&host, &intent, "destination", "owner", &scope).unwrap());
        let commands = handle.doc().read_commands().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].payload, zeron_doc::SessionCommandPayload::Run {
            request, message_id: "original-user".into(),
        });
        assert_eq!(handle.doc().read_entries().unwrap().len(), 1);
        store.mark_processed(&intent.command_id).unwrap();
        let reopened = DocHost::new(store.clone(), crate::DocHostConfig {
            device_id: "destination".into(), default_harness: HarnessId::Omp, edge: None,
        });
        reopened.set_workspace(host.workspace().unwrap().clone());
        assert!(!admit_imported_recovery(&reopened, &intent, "destination", "owner", &scope).unwrap());
        assert_eq!(reopened.open(chat_id).unwrap().doc().read_commands().unwrap().len(), 1);
        intent.command_id = "legacy-recovery-unknown-claim".into();
        store.mark_processed(&intent.command_id).unwrap();
        assert!(!admit_imported_recovery(&host, &intent, "destination", "owner", &scope).unwrap());
        assert_eq!(handle.doc().read_commands().unwrap().len(), 1);
        let workspace = host.workspace().unwrap();
        let mut chat = workspace.chat(chat_id).unwrap().unwrap();
        chat.crew = Some(serde_json::from_value(serde_json::json!({
            "ownerSubject":"owner", "sessionId":chat_id, "scope":scope,
            "originalEpochHistory":{"sandboxId":"box","lifecycleEpoch":1,"coordinatorLifecycleEpoch":2,"sourceDeviceId":"comet-scaffold-box",
                "originalStatus":"stopped","nativeSessionId":"exact-native-session","sourceEvidenceSha256":"a".repeat(64)},
        })).unwrap());
        workspace.import_chat_row(&chat).unwrap();
        intent.command_id = "must-not-replay-in-new-epoch".into();
        assert!(!admit_imported_recovery(&host, &intent, "destination", "owner", &scope).unwrap());
        assert_eq!(handle.doc().read_commands().unwrap().len(), 1);
    }

    fn persist_original_epoch_fixture(
        store: &zeron_sync::DocsStore,
        id: &str,
        outcomes: &std::collections::BTreeMap<String, String>,
    ) -> zeron_proto::OriginalEpochHistory {
        use sha2::Digest as _;
        let principal = zeron_proto::CollaborationPrincipal {
            subject: "owner".into(), email: None, project_id: "project".into(),
            deployment_id: Some("staging".into()), session_id: Some(id.into()),
            capabilities: vec![], unknown: Default::default(),
        };
        let proof = serde_json::json!({
            "version":1, "kind":"terminal-original-epoch", "migrationId":"migration",
            "sourceSha256":"a".repeat(64), "coordinatorLifecycleEpoch":2,
            "nativeSessionId":"original-native",
            "commandOutcomesSha256":format!("{:x}", sha2::Sha256::digest(serde_json::to_vec(outcomes).unwrap())),
            "scope":{"ownerSubject":"owner", "projectId":"project", "deploymentId":"staging",
                "logicalSessionId":id, "sandboxId":"sandbox", "deviceId":"comet-scaffold-sandbox", "lifecycleEpoch":1},
            "terminal":{"status":"stopped", "providerAction":"kill", "terminationReason":"provider_missing",
                "terminatedAt":"2026-10-21T01:00:00Z", "providerNotFoundObservedAt":"2026-10-21T01:00:01Z",
                "physicalSandboxId":"sandbox", "retainedSnapshotIds":[]},
        });
        let proof_json = serde_json::to_string(&proof).unwrap();
        let history = zeron_sync::crew_import::validate_original_epoch_bootstrap(
            proof_json.as_bytes(), outcomes, &principal,
        ).unwrap();
        let key = format!("original-epoch-bootstrap:{id}:{}", history.lifecycle_epoch);
        let request = serde_json::to_vec(&serde_json::json!({
            "proofJson":proof_json, "commandOutcomes":outcomes, "principal":principal,
        })).unwrap();
        store.claim_crew_command(&key, &request).unwrap();
        for id in outcomes.keys() { store.mark_processed(id).unwrap(); }
        store.finish_crew_command(&key, &serde_json::to_vec(&history).unwrap()).unwrap();
        history
    }

    #[tokio::test]
    async fn original_pending_commands_survive_sweep_and_retry_before_terminal_sync() {
        use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
        for retry_first in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let id = "11111111-1111-4111-8111-111111111111";
            let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
            let history = persist_original_epoch_fixture(&store, id,
                &std::collections::BTreeMap::from([("historical".into(), "rejected".into())]));
            let sessions = SessionsEngine::new("host".into(),
                Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()),
                Arc::new(HarnessRegistry::new()));
            // Reload verified persisted evidence, not a synced marker or injected runtime map.
            sessions.load_crew_import(dir.path()).unwrap();
            assert_eq!(sessions.original_epoch_history(id), Some(&history));
            let host = DocHost::new(store.clone(), crate::DocHostConfig {
                device_id: "host".into(), default_harness: HarnessId::Omp, edge: None,
            });
            sessions.set_doc_host(host.clone());
            host.set_sessions(sessions.clone());
            let handle = host.open(id).unwrap();
            let original = SessionCommandEntry {
                id: "historical".into(), issued_by: "comet-scaffold-sandbox".into(), issued_at: now_ms(),
                payload: SessionCommandPayload::Run { request: request(), message_id: "old-message".into() },
                based_on: None, expires_at: None, status: SessionCommandStatus::Pending, resolution: None,
                accepted_at: None,
            };
            handle.doc().queue_command(&original).unwrap();
            if retry_first { host.retry_delivery(id).unwrap(); }
            host.drain_commands(&handle).await;
            host.retry_delivery(id).unwrap();
            let commands = handle.doc().read_commands().unwrap();
            assert_eq!(commands.len(), 1, "history must never mint a retry attempt");
            assert_eq!(commands[0].status, SessionCommandStatus::Pending);
            assert!(commands[0].resolution.is_none());
            assert!(handle.doc().read_entries().unwrap().is_empty());
            // The source terminal outcome can arrive later; no local crash outcome replaces it.
            handle.doc().set_command_status("historical", SessionCommandStatus::Rejected, Some("source outcome")).unwrap();
            host.retry_delivery(id).unwrap();
            host.drain_commands(&handle).await;
            let commands = handle.doc().read_commands().unwrap();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].status, SessionCommandStatus::Rejected);
            assert_eq!(commands[0].resolution.as_deref(), Some("source outcome"));

            // A current command in the same bootstrapped chat still gets ordinary crash recovery.
            let mut current = original;
            current.id = "current-crash".into();
            current.payload = SessionCommandPayload::Run { request: request(), message_id: "current-message".into() };
            store.mark_processed(&current.id).unwrap();
            handle.doc().queue_command(&current).unwrap();
            if !retry_first { host.drain_commands(&handle).await; }
            host.retry_delivery(id).unwrap();
            let commands = handle.doc().read_commands().unwrap();
            assert_eq!(commands.len(), 3, "only the current crash may mint a retry");
            let crashed = commands.iter().find(|c| c.id == "current-crash").unwrap();
            assert_eq!(crashed.status, SessionCommandStatus::Rejected);
            assert!(crashed.resolution.as_deref().unwrap().contains("interrupted before completion"));
            assert!(commands.iter().any(|c| c.id != "historical" && c.id != "current-crash"
                && matches!(&c.payload, SessionCommandPayload::Run { message_id, .. } if message_id == "current-message")));
            sessions.shutdown().await;
        }
    }

    #[tokio::test]
    async fn original_epoch_requires_trusted_start_and_never_reuses_old_native_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let workspace = crate::WorkspaceHost::open(store.clone(), crate::workspace_host::WorkspaceHostConfig {
            device_id: "importer".into(), device_name: "Host".into(), platform: "linux".into(),
            org_id: "project".into(), user_id: "owner".into(), edge: None, local_only: false,
            owner_registry: true,
        }).unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        workspace.create_chat(id, None, Some("importer"), None, Some("/original/repo".into())).unwrap();
        let history = persist_original_epoch_fixture(&store, id, &Default::default());
        let mut chat = workspace.chat(id).unwrap().unwrap();
        chat.harness_session_id = history.native_session_id.clone();
        chat.harness_session_cwd = Some("/original/repo".into());
        chat.crew = Some(serde_json::from_value(serde_json::json!({
            "ownerSubject":"owner", "sessionId":id,
            "scope":{"projectId":"project","deploymentId":"staging","sessionId":id},
            "originalEpochHistory":history,
        })).unwrap());
        workspace.import_chat_row(&chat).unwrap();
        let (feed, events) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness { id: HarnessId::Mock, feed: Mutex::new(Some(events)), steering: None }));
        let sessions = SessionsEngine::new("importer".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()), Arc::new(registry));
        sessions.inner.original_epoch_histories.set(
            zeron_sync::crew_import::imported_original_epoch_histories(dir.path()).unwrap(),
        ).unwrap();
        let host = DocHost::new(store.clone(), crate::DocHostConfig {
            device_id: "importer".into(), default_harness: HarnessId::Omp, edge: None,
        });
        host.set_workspace(workspace.clone());
        sessions.set_doc_host(host.clone());
        host.set_sessions(sessions.clone());
        let document = host.open(id).unwrap();
        document.write_user_message("original-message", "preserved source history", now_ms()).unwrap();
        assert!(sessions.ensure_execution_allowed(id, None).is_err());
        let metadata = chat.crew.take();
        workspace.import_chat_row(&chat).unwrap();
        assert!(sessions.ensure_execution_allowed(id, None).is_err(), "dropping index metadata cannot drop trusted provenance");
        chat.crew = metadata;
        workspace.import_chat_row(&chat).unwrap();
        assert!(host.queue_command(id, zeron_doc::SessionCommandPayload::Run {
            request: request(), message_id: "must-not-write".into(),
        }).is_err());
        assert!(document.doc().read_commands().unwrap().is_empty());
        // The stopped coordinator epoch is not a new execution, even though
        // it is newer than the original writer and has a confirmed binding.
        let stopped_device = crate::scaffold::device_id_for("sandbox", 2);
        chat.device_id = stopped_device.clone();
        chat.harness_session_id = Some(String::new());
        chat.harness_session_cwd = None;
        chat.crew.as_mut().unwrap().environment = Some(zeron_proto::SessionEnvironment {
            source: zeron_proto::SessionEnvironmentSource::Scaffold {
                sandbox_id: "sandbox".into(), region: None, lifecycle: zeron_proto::ScaffoldLifecycle::Ready,
                lifecycle_epoch: Some(2), links: Default::default(),
            }, name: None, owner_principal: "owner".into(), scope: chat.crew.as_ref().unwrap().scope.clone(),
            source_ref: None, last_activity_at: None, database_environment: None, unknown: Default::default(),
        });
        workspace.import_chat_row(&chat).unwrap();
        let stopped_key = format!("original-epoch-start:{id}:1:2");
        store.claim_crew_command(&stopped_key, &serde_json::to_vec(&history).unwrap()).unwrap();
        store.finish_crew_command(&stopped_key, &serde_json::to_vec(&serde_json::json!({
            "sandboxId":"sandbox","lifecycleEpoch":2,"deviceId":stopped_device,
        })).unwrap()).unwrap();
        assert!(sessions.ensure_execution_allowed(id, None).is_err(), "fresh execution must be newer than coordinator epoch 2, not just original epoch 1");
        // A client-projected larger epoch and cleared association are not authority.
        let device = crate::scaffold::device_id_for("sandbox", 3);
        chat.device_id = device.clone();
        chat.harness_session_id = Some(String::new());
        chat.harness_session_cwd = None;
        chat.crew.as_mut().unwrap().environment = Some(zeron_proto::SessionEnvironment {
            source: zeron_proto::SessionEnvironmentSource::Scaffold {
                sandbox_id: "sandbox".into(), region: None, lifecycle: zeron_proto::ScaffoldLifecycle::Ready,
                lifecycle_epoch: Some(3), links: Default::default(),
            }, name: None, owner_principal: "owner".into(), scope: chat.crew.as_ref().unwrap().scope.clone(),
            source_ref: None, last_activity_at: None, database_environment: None, unknown: Default::default(),
        });
        workspace.import_chat_row(&chat).unwrap();
        assert!(sessions.ensure_execution_allowed(id, None).is_err());
        let key = format!("original-epoch-start:{id}:1:3");
        store.claim_crew_command(&key, &serde_json::to_vec(&history).unwrap()).unwrap();
        assert!(sessions.ensure_execution_allowed(id, None).is_err(), "unknown start outcome is not permission");
        store.finish_crew_command(&key, &serde_json::to_vec(&serde_json::json!({
            "sandboxId":"sandbox","lifecycleEpoch":3,"deviceId":device,
        })).unwrap()).unwrap();
        sessions.reset_original_epoch_native_association(id);
        assert!(sessions.ensure_execution_allowed(id, None).is_ok());
        assert_eq!(
            sessions
                .inner
                .resume_for(id, "/original/repo", HarnessId::Mock),
            None
        );
        assert!(sessions.ensure_execution_allowed(id, Some("original-native")).is_err());
        assert!(sessions.ensure_execution_allowed("different-chat", Some("original-native")).is_err());
        let old_selector = dir.path().join("native-resumes/original-native.jsonl");
        lock(&sessions.inner.imported_native_histories).insert("original-native".into(), old_selector.clone());
        assert!(sessions.ensure_execution_allowed("different-chat", old_selector.to_str()).is_err());
        let raw_archive = dir.path().join("native-histories/original.jsonl");
        sessions.inner.original_epoch_archives.set(vec![raw_archive.clone()]).unwrap();
        assert!(sessions.ensure_execution_allowed("different-chat", raw_archive.to_str()).is_err());
        let messages = document.doc().read_entries().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "original-message");
        let mut fresh = request();
        fresh.cwd = "/original/repo".into();
        let run = sessions.dispatch(id, HarnessId::Mock, fresh, Some("fresh-user-request".into())).await.unwrap();
        assert!(!run.is_empty());
        assert_eq!(sessions.last_request(id).unwrap().resume, None);
        feed.send(AgentEvent::SessionStarted {
            harness: HarnessId::Mock, model: "mock-1".into(), tools: vec![], cwd: "/original/repo".into(),
            session_id: "fresh-native".into(), assistant_message_id: "fresh-response".into(),
        }).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while workspace.chat(id).unwrap().unwrap().harness_session_id.as_deref() != Some("fresh-native") {
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        assert!(sessions.ensure_execution_allowed(id, Some("fresh-native")).is_ok());
        assert_eq!(workspace.chat(id).unwrap().unwrap().crew.unwrap().original_epoch_history, Some(history));
        assert_eq!(document.doc().read_entries().unwrap()[0].id, "original-message");
        feed.send(AgentEvent::Done { status: DoneStatus::Completed, result: None, error: None, session_id: Some("fresh-native".into()) }).unwrap();
        drop(feed);
        sessions.shutdown().await;
    }

    #[tokio::test]
    async fn imported_native_launch_uses_copy_without_rewriting_canonical_identity() {
        let dir = tempfile::tempdir().unwrap();
        let chat_id = "local-chat-0123456789abcdef";
        let histories = dir.path().join("native-histories");
        std::fs::create_dir(&histories).unwrap();
        let copy = histories.join("session.jsonl");
        std::fs::write(&copy, "{\"type\":\"session\",\"id\":\"original-native\",\"cwd\":\"/original/repo\"}\n").unwrap();
        let journal_hash = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&copy).unwrap()));
        let report = serde_json::json!({
            "version":1,"sourceProfileRoot":"/old/profile","chatIds":[chat_id],
            "nativeSessionIds":{(chat_id):"original-native"},
            "nativeHistories":{"original-native":"native-histories/session.jsonl"},
            "filesSha256":{"native-histories/session.jsonl":journal_hash},"commandsRetained":0,"annotationsRetained":0,
            "requiresReauthentication":true
        });
        std::fs::write(dir.path().join("crew-import-report.json"), serde_json::to_vec(&report).unwrap()).unwrap();
        let sessions = SessionsEngine::new("host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()),
            Arc::new(HarnessRegistry::new()));
        sessions.load_crew_import(dir.path()).unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let workspace = crate::WorkspaceHost::open(store.clone(), crate::workspace_host::WorkspaceHostConfig {
            device_id: "host".into(), device_name: "Host".into(), platform: "linux".into(),
            org_id: "project".into(), user_id: "owner".into(), edge: None, local_only: false,
            owner_registry: true,
        }).unwrap();
        workspace.create_chat(chat_id, None, Some("host"), None, Some("/original/repo".into())).unwrap();
        workspace.set_chat_harness_session(
            chat_id,
            "original-native",
            "/original/repo",
            Some(HarnessId::Omp),
        );
        let host = DocHost::new(store, crate::DocHostConfig {
            device_id: "host".into(), default_harness: HarnessId::Omp, edge: None,
        });
        host.set_workspace(workspace);
        sessions.set_doc_host(host.clone());
        // A new engine has no live-process cache: resume comes from the durable row.
        assert!(lock(&sessions.inner.harness_sessions).is_empty());
        let id = sessions
            .inner
            .resume_for(chat_id, "/original/repo", HarnessId::Omp)
            .unwrap();
        assert_eq!(id, "original-native");
        let copied_path = dir.path().join("native-resumes/original-native.jsonl")
            .canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(sessions.native_launch_selector(&id), copied_path);
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&copied_path).unwrap());
        assert!(
            sessions
                .inner
                .resume_for(chat_id, "/other/repo", HarnessId::Omp)
                .is_none()
        );
        std::fs::remove_file(&copied_path).unwrap();
        // Missing launch data remains a failed exact-path resume, never a lookup
        // against another user's or the old installed app's native store.
        assert_eq!(sessions.native_launch_selector(&id), copied_path);
        host.shutdown_workers().await;
    }
}
