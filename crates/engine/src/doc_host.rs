//! DocHost — per-chat `SessionDoc` handles: snapshot persistence (debounced), edge room
//! sync (offline-tolerant), and the HOST-ONLY durable command executor.
//!
//! Pragmatic port of zeron's `session-docs.ts` + the `main.ts` executor (spec:
//! feature-inventory §3.3, ARCHITECTURE §2 "command plane"):
//! - the doc IS the outbox: commands and user entries commit locally and sync whenever a
//!   room connection exists; the engine is fully functional with sync disabled;
//! - on every doc change (local commit or remote import) the handle re-emits the joined
//!   transcript to watchers, drains pending commands, and schedules a snapshot save;
//! - command drain: evaluate via `evaluate_command` (with the DocsStore processed
//!   ledger), mark processed BEFORE execute, execute through the sessions engine, then
//!   write the outcome status back into the doc as the sole outcome writer.
//!
//! Chat ownership is gated on the workspace doc (`chats[chat_id].deviceId`), with
//! claim-on-first-command for unknown chats. Queueing a command for a chat hosted on
//! another device POSTs a durable nudge to that device's room (§7 cold-chat delivery);
//! the host's relay receives it and warm-opens the doc, which drains the queue.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use base64::Engine as _;
use loro::ToJson as _;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use zeron_doc::{
    COMMAND_DEFAULT_TTL_MS, CommandBasedOn, CommandDisposition, DocError, EvaluationContext,
    MessagePart, MessageRole, MessageStatus, QueueDeliveryGate, QueuedMessage, SessionCommandEntry,
    SessionCommandPayload, SessionCommandStatus, SessionDoc, SessionMessageEntry, SubagentStatus,
    evaluate_command,
};
use zeron_proto::{ConversationSourceContext, HarnessId, UserInputAnswer, UserInputQuestion};
use zeron_sync::DocsStore;

use crate::http_error::describe_http_error;
use crate::project_actions::{
    ProjectActionSetupHandoff, ProjectActionsStore, launch_project_setup_action,
};
use crate::sessions::{SessionsEngine, SteerOutcome};
use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, Terminals, new_id, now_ms};

/// Debounce window for local snapshot saves after a doc change.
const SNAPSHOT_DEBOUNCE_MS: u64 = 1_000;
const TAIL_INTERVAL: Duration = Duration::from_secs(5);
/// Minimum spacing between tail PUTs outside a streaming turn, except at a
/// local turn end. A remote turn arrives as several assistant segments with a
/// gap between them; each gap looks quiesced, so this bounds the PUTs a remote
/// turn can cause to one per second.
const TAIL_IDLE_FLOOR: Duration = Duration::from_secs(1);
const STREAM_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(600);
/// Failure backoff for the tail PUT and the threshold checkpoint: doubling
/// from the minimum to the cap, so an outage costs a handful of attempts per
/// chat per hour instead of one per wake.
const TAIL_RETRY_MIN: Duration = Duration::from_secs(5);
const TAIL_RETRY_MAX: Duration = Duration::from_secs(120);
const CHECKPOINT_RETRY_MIN: Duration = Duration::from_secs(15);
const CHECKPOINT_RETRY_MAX: Duration = Duration::from_secs(300);
/// Shutdown spends at most this long pushing deferred tails and checkpoints.
const SHUTDOWN_SYNC_BUDGET: Duration = Duration::from_secs(5);

#[derive(Default)]
struct SyncCadence {
    tail_attempt: Option<tokio::time::Instant>,
    tail_hash: Option<[u8; 32]>,
    /// The doc version the last successful PUT covered: an unchanged version
    /// means an unchanged tail, so a pass can stop before materializing it.
    tail_version: Option<loro::VersionVector>,
    tail_failures: u32,
    tail_retry_at: Option<tokio::time::Instant>,
    /// A pass found a changed tail it could not send yet (throttled or
    /// failed). Shutdown flushes these; a clean chat costs shutdown nothing.
    tail_deferred: bool,
    checkpoint_failures: u32,
    checkpoint_retry_at: Option<tokio::time::Instant>,
    stream_since_checkpoint: Option<tokio::time::Instant>,
    session_streaming: Option<bool>,
}

fn backoff(failures: u32, min: Duration, max: Duration) -> Duration {
    min.saturating_mul(1u32 << failures.saturating_sub(1).min(16))
        .min(max)
}

fn earliest(
    a: Option<tokio::time::Instant>,
    b: Option<tokio::time::Instant>,
) -> Option<tokio::time::Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn checkpoint_due(streaming: bool, rows: u64, bytes: u64, stale: bool) -> bool {
    if streaming {
        rows > 2000 || bytes > 4 * 1024 * 1024 || stale
    } else {
        rows > 200 || bytes > 512 * 1024
    }
}

fn checkpoint_stale(since: Option<tokio::time::Instant>, now: tokio::time::Instant) -> bool {
    since.is_some_and(|since| now.duration_since(since) >= STREAM_CHECKPOINT_INTERVAL)
}

fn cadence_streaming(writers: usize, session_streaming: Option<bool>) -> bool {
    writers > 0 && session_streaming.unwrap_or(true)
}

/// Only `Working` streams. A turn parked on a question (`AwaitingInput`)
/// writes nothing: it must flush like a finished turn (the asker is waiting on
/// a reader's view of the question) and must not keep a 1 Hz tick alive for as
/// long as the question stays open.
fn session_streams(status: zeron_proto::SessionStatus) -> bool {
    matches!(status, zeron_proto::SessionStatus::Working)
}

/// An edit client renews every 20s. Sixty seconds tolerates two missed
/// heartbeats without turning a vanished client into an invisible permanent
/// lock. Expiry fails closed into ReviewRequired rather than releasing.
pub const QUEUE_EDIT_LEASE_MS: i64 = 60_000;

/// Warm-doc LRU: how many unwatched, run-less docs stay fully open. Everything
/// beyond this (and beyond [`zeron_doc::DOC_LRU_BYTE_BUDGET`]) is evicted
/// oldest-access-first — reopening from the SQLite snapshot measured within
/// ~11ms of a warm doc, so the cap trades no perceptible open latency.
const WARM_DOC_CAP: usize = 12;

/// Resident-memory estimate per compressed snapshot byte. Loro snapshots are
/// columnar+compressed; the in-memory doc plus mirror runs well above the blob
/// size. A rough multiplier is enough here — the budget is a safety ceiling,
/// the count cap does the day-to-day work.
const RESIDENT_BYTES_PER_SNAPSHOT_BYTE: usize = 6;

/// Floor per open doc (room socket buffers, tasks) regardless of content size.
const DOC_RESIDENT_FLOOR_BYTES: usize = 512 * 1024;

/// Connection reuse grace is independent of document eviction. Caller-owned
/// handles and explicit writers protect the open-to-subscribe handoff.
const SYNC_IDLE_MS: i64 = 10_000;
const SYNC_QUANTUM_MS: i64 = 30_000;
const ACTIVE_SYNC_CAP: usize = 28;
const SYNC_ADMISSION_BATCH: usize = 4;
const SYNC_CATCH_UP_QUANTUM_MS: i64 = 300_000;
/// A quiesce for a Cloud sleep the stop never followed lifts after this.
const QUIESCE_LEASE: std::time::Duration = std::time::Duration::from_secs(180);

// All live-transport scenarios share the process budget, including tests in
// sibling modules. Hold through shutdown so one fixture cannot rotate another.
#[cfg(test)]
static SYNC_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncAdmission {
    Join,
    Seed,
}

/// Queued-attachment transfer pacing: chunk pushes are bounded per call (a
/// stalled-but-open relay link never fails on its own) and a timeout marks
/// the link suspect; attempts retry on this backoff, cut short by the online
/// bus / system wake.
const TRANSFER_CHUNK_B64: usize = 60_000;
const TRANSFER_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const TRANSFER_COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const TRANSFER_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(2);
const TRANSFER_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);
/// A command whose attachment bytes are still in transit waits at most this
/// long before the drain rejects it loudly (and the transfer task gives up on
/// the same clock) — a chat must never wedge behind bytes that aren't coming.
const ATTACHMENT_WAIT_MAX: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const ATTACHMENT_WAIT_MAX_MS: i64 = ATTACHMENT_WAIT_MAX.as_millis() as i64;
/// Re-check cadence while a chat's queue is deferred on in-transit bytes
/// (the happy path is event-driven — UploadCommit kicks the drain — this
/// timer only covers the give-up transition and missed kicks).
const ATTACHMENT_WAIT_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

/// Transfer attempt outcome: transient failures retry (the link may heal),
/// permanent ones stop (the host actively refused, or the bytes are gone).
enum TransferError {
    Transient(String),
    Permanent(String),
}

/// Peer-relay delivery fallback pacing (`spawn_command_delivery`): the grace
/// the normal rows→edge path gets before the relay road opens, the poll while
/// waiting, the relay retry curve, its per-call deadline, and the give-up cap
/// (the command stays durably queued in the doc regardless).
const ROWS_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
const ROWS_POLL: std::time::Duration = std::time::Duration::from_secs(1);
const RELAY_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(5);
const RELAY_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);
const RELAY_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const RELAY_GIVE_UP: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Hosts below this stamped engine version don't serve `RelayCommand`.
const RELAY_MIN_VERSION: (u64, u64, u64) = (0, 2, 12);

/// Edge connection config. The bearer is a **provider**, never a snapshot:
/// every room (re)connect and HTTP request re-reads it, so WorkOS access-token
/// refreshes (~1h expiry) take effect without an engine restart. Dev bearers
/// (which never expire) ride the same seam as a [`zeron_rpc::StaticToken`].
#[derive(Clone)]
pub struct EdgeConfig {
    /// Edge base URL (`http(s)://…`); rewritten to `ws(s)` for the room socket.
    pub url: String,
    /// Fresh-bearer provider (the relay's `TokenSource`), consulted per
    /// connect/request. Temporary failures preserve the signed-in session.
    pub token: Arc<dyn zeron_rpc::TokenSource>,
    /// This engine's device id, carried on room dials (`&device=`) so the
    /// edge can attribute sockets in logs. Debugging the 2026-08-04 deaf
    /// socket meant reverse-engineering devices from rotating IPv6 privacy
    /// addresses; never again. Empty = omitted (tests).
    pub device_id: String,
}

impl std::fmt::Debug for EdgeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeConfig")
            .field("url", &self.url)
            .field("token", &"<provider>")
            .finish()
    }
}

impl EdgeConfig {
    pub fn new(url: impl Into<String>, token: Arc<dyn zeron_rpc::TokenSource>) -> Self {
        Self {
            url: url.into(),
            token,
            device_id: String::new(),
        }
    }

    /// Attribute this engine's room sockets in edge logs.
    pub fn with_device(mut self, device_id: impl Into<String>) -> Self {
        self.device_id = device_id.into();
        self
    }

    /// Fixed bearer — dev mode and tests, where tokens never expire.
    pub fn with_static_token(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self::new(url, Arc::new(zeron_rpc::StaticToken(token.into())))
    }

    /// The current bearer, or a distinct signed-out/temporarily-unavailable error.
    pub async fn bearer(&self) -> Result<String, zeron_rpc::TokenError> {
        self.token.token().await
    }

    pub fn token_changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.token.subscribe()
    }

    /// A per-dial room URL provider for `path` (e.g. `/session/{chatId}/ws`):
    /// the bearer is re-fetched before every connect, so reconnects after a
    /// token expiry present a fresh `?token=` instead of the boot-time one.
    pub fn room_url(&self, path: impl Into<String>) -> Arc<dyn zeron_sync::UrlProvider> {
        self.room_url_with_host(path, None)
    }

    fn room_url_with_host(
        &self,
        path: impl Into<String>,
        host_device: Option<String>,
    ) -> Arc<dyn zeron_sync::UrlProvider> {
        let ws_base = self.url.replacen("http", "ws", 1);
        Arc::new(EdgeRoomUrl {
            base: format!("{}{}", ws_base.trim_end_matches('/'), path.into()),
            token: self.token.clone(),
            device_id: self.device_id.clone(),
            host_device,
        })
    }
}

struct EdgeRoomUrl {
    base: String,
    token: Arc<dyn zeron_rpc::TokenSource>,
    device_id: String,
    host_device: Option<String>,
}

impl zeron_sync::UrlProvider for EdgeRoomUrl {
    fn url(&self) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        let token = self.token.clone();
        let base = self.base.clone();
        let device = self.device_id.clone();
        let host = self.host_device.clone();
        Box::pin(async move {
            let token = token.token().await.map_err(zeron_sync::SyncError::from)?;
            let mut url = format!("{base}?token={token}");
            if !device.is_empty() {
                url.push_str(&format!("&device={device}"));
            }
            if let Some(host) = host {
                let mut parsed = reqwest::Url::parse(&url)
                    .map_err(|e| zeron_sync::SyncError::Protocol(e.to_string()))?;
                parsed.query_pairs_mut().append_pair("hostDevice", &host);
                url = parsed.into();
            }
            Ok(url)
        })
    }
}

#[derive(Debug, Clone)]
pub struct DocHostConfig {
    pub device_id: String,
    /// Harness for doc-command runs on chats without a workspace `config` row.
    pub default_harness: HarnessId,
    /// When present, each opened chat joins its edge session room. `None` = fully
    /// offline operation (local snapshots only).
    pub edge: Option<EdgeConfig>,
}

struct DocHostInner {
    store: Arc<DocsStore>,
    /// Writes every local commit's outbox row off the committing thread.
    outbox: Arc<crate::outbox_writer::OutboxWriter>,
    config: DocHostConfig,
    /// Set-once (first wins), cleared by `shutdown_workers`: sessions and
    /// doc-host reference each other through Arcs, so a retired runtime's
    /// graph only drops once this back-edge is severed.
    sessions: Mutex<Option<SessionsEngine>>,
    /// Quiesced for a Cloud sleep ([`DocHost::quiesce_for_sleep`]): drains
    /// leave new work pending in the room, for the machine once it wakes.
    quiesced: AtomicBool,
    /// Bumped per quiesce: only the newest one's lease lifts it.
    quiesce_generation: AtomicU64,
    /// Drains between their quiesce check and the end of their dispatch (a
    /// run they start is registered — and Working — by then).
    dispatching: Arc<AtomicUsize>,
    workspace: OnceLock<WorkspaceHost>,
    /// Worktree materialization for Run commands (see `set_repos`).
    repos: OnceLock<crate::repos::Repos>,
    project_action_runtime: OnceLock<(ProjectActionsStore, Terminals)>,
    /// Cancels every worker spawned through `spawn_worker` — the loops'
    /// own exit conditions (weak handle death, closed channels) don't cover
    /// runtime replacement, where Edge-capable tasks must stop doing
    /// network work even while something still pins the graph.
    shutdown: CancellationToken,
    edge_disconnected: AtomicBool,
    /// Tracks every spawned worker so `shutdown_workers` can await them.
    tasks: TaskTracker,
    handles: Mutex<HashMap<String, Arc<ChatDocHandle>>>,
    document_loads: AtomicU64,
    /// Snapshot blobs the salvage sweep imported. A salvage-checked chat
    /// must not move this on a later boot.
    #[cfg(test)]
    salvage_imports: AtomicU64,
    focus_clock: AtomicU64,
    /// Serialize cold opens without blocking access to already-live handles.
    opening: Mutex<()>,
    /// chat2 seeds in flight (one per chat — reopen storms must not race
    /// duplicate rebuild+checkpoint POSTs; benign server-side, wasteful).
    seeding: Mutex<HashSet<String>>,
    /// chat2 quiet-waiters armed (one per chat): the cutover watcher re-arms
    /// on every registry change, and a long run would stack a tick loop per
    /// change without this.
    seed_waiting: Mutex<HashSet<String>>,
    /// Attachment-wait re-drain timers armed (one per chat): a command
    /// deferred on in-transit attachment bytes re-checks on a cadence, and
    /// each deferral must not stack another timer.
    drain_waiting: Mutex<HashSet<String>>,
    /// Uploads store (engine assembly) — resolves `pending://` attachment
    /// refs and jails transfer reads to the uploads dir.
    uploads: OnceLock<crate::uploads::Uploads>,
    /// Connectivity watch (`WatchConnectivity`): lazily-started monitor
    /// publishes the edge posture on change (see `watch_connectivity`).
    connectivity: OnceLock<watch::Sender<zeron_proto::Connectivity>>,
    connectivity_started: AtomicBool,
    /// In-flight queued-attachment transfers, published per landed chunk
    /// (see `watch_transfers`). Entries live exactly as long as bytes are
    /// moving: added when a file's push starts, removed on commit or failure
    /// (a retry re-adds), so consumers can render a real percent while the
    /// relay leg runs and fall back to indeterminate otherwise.
    transfers: watch::Sender<Vec<zeron_proto::TransferProgress>>,
    connectivity_grace: Mutex<DegradeGrace>,
    /// Command ids currently BETWEEN mark-processed and their resolution in a
    /// drain. Distinguishes "executing right now" from "consumed by the
    /// ledger but dead" (a crash between mark and resolve): the drain
    /// terminalizes the latter as Rejected instead of leaving a forever-
    /// Pending entry no retry could ever reach (2026-08-19 swallowed-send).
    executing: Mutex<HashSet<String>>,
    /// Peer links (engine assembly, edge runtimes only) — the transport that
    /// pushes queued attachment bytes to a remote host.
    links: OnceLock<Arc<zeron_rpc::LinkCache>>,
    /// Shared client for sidecar blob PUT/GET (30s timeout, uploads.rs
    /// discipline — diff_sync's untimed client hung on dead links).
    http: reqwest::Client,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The queued-attachment transfers a command's `pending://` refs imply —
/// shared by the retry escort and the retry re-issue.
fn command_transfers(entry: &SessionCommandEntry) -> Vec<crate::uploads::AttachmentTransfer> {
    let refs: Vec<String> = match &entry.payload {
        SessionCommandPayload::Run { request, .. } => request
            .attachments
            .iter()
            .filter(|p| crate::uploads::is_pending_ref(p))
            .cloned()
            .collect(),
        SessionCommandPayload::Steer { prompt, .. } => crate::uploads::pending_refs_in(prompt),
        _ => Vec::new(),
    };
    refs.iter()
        .filter_map(|r| crate::uploads::parse_pending_ref(r))
        .map(
            |(upload_id, file_name)| crate::uploads::AttachmentTransfer {
                upload_id: upload_id.to_string(),
                file_name: file_name.to_string(),
            },
        )
        .collect()
}

/// Retires a transfer's progress entry on drop — the one exit point for
/// `push_attachments`' many returns (commit landed, chunk timeout, host
/// refusal), so no failure path can leave a phantom ring behind.
struct TransferProgressGuard<'a> {
    host: &'a DocHost,
    upload_id: &'a str,
}

impl Drop for TransferProgressGuard<'_> {
    fn drop(&mut self) {
        self.host.transfer_progress_clear(self.upload_id);
    }
}

/// How long raw degradation must persist before connectivity reports it.
/// Room joins, idle-link wakes, and navigation dials resolve well under a
/// second on healthy networks; real outages outlive this comfortably. Recovery
/// is never delayed.
const DEGRADE_GRACE: std::time::Duration = std::time::Duration::from_secs(4);

/// One tracked degradation source in [`DegradeGrace`].
enum GraceKey<'a> {
    OsPath,
    Registry,
    Chat(&'a str),
}

/// Show-slow / hide-fast hysteresis over raw connectivity signals. Pure over
/// injected `Instant`s so the grace window is unit-testable.
#[derive(Default)]
struct DegradeGrace {
    os_path: Option<std::time::Instant>,
    registry: Option<std::time::Instant>,
    chats: HashMap<String, std::time::Instant>,
}

impl DegradeGrace {
    /// Feed one raw sample; returns whether to REPORT the source as degraded.
    /// Healthy clears the timer instantly; degraded reports only once it has
    /// persisted for [`DEGRADE_GRACE`].
    fn degraded(&mut self, key: GraceKey, raw: bool, now: std::time::Instant) -> bool {
        let slot: &mut Option<std::time::Instant> = match key {
            GraceKey::OsPath => &mut self.os_path,
            GraceKey::Registry => &mut self.registry,
            GraceKey::Chat(id) => {
                if raw && !self.chats.contains_key(id) {
                    self.chats.insert(id.to_string(), now);
                }
                match self.chats.get_mut(id) {
                    Some(_) if !raw => {
                        self.chats.remove(id);
                        return false;
                    }
                    Some(since) => return now.duration_since(*since) >= DEGRADE_GRACE,
                    None => return false,
                }
            }
        };
        if !raw {
            *slot = None;
            return false;
        }
        let since = *slot.get_or_insert(now);
        now.duration_since(since) >= DEGRADE_GRACE
    }

    /// Drop timers for chats that no longer have open docs.
    fn retain_chats(&mut self, keep: impl Fn(&str) -> bool) {
        self.chats.retain(|id, _| keep(id));
    }
}

/// One read of an open chat's admission flags and client. Connectivity and
/// sync-state reporting share it so a tick does not re-fetch each handle.
struct ChatConnectionSnapshot {
    sync_started: bool,
    sync_requested: bool,
    stats: Option<zeron_sync::ChatStatsSnapshot>,
    delivery_live: bool,
}

impl ChatConnectionSnapshot {
    fn read(handle: &ChatDocHandle) -> Self {
        let sync_started = handle.sync_started.load(Ordering::Acquire);
        let sync_requested = handle.sync_requested.load(Ordering::Acquire);
        let client = lock(&handle.chat2);
        Self {
            sync_started,
            sync_requested,
            stats: client.as_ref().map(|client| client.stats()),
            delivery_live: sync_started
                && client.as_ref().is_some_and(|client| client.delivery_live()),
        }
    }

    fn sync_expected(&self) -> bool {
        self.sync_started || self.sync_requested
    }
}

#[derive(Clone)]
pub struct DocHost {
    inner: Arc<DocHostInner>,
}

/// How a taken queue row reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueSend {
    /// Nothing is running: start a turn with it.
    NextTurn,
    /// Something is running and can take input mid-turn: steer it in.
    Steer,
    /// The user said now: stop what is running first.
    Interrupt,
}

const ATTACHMENT_ONLY_PROMPT: &str = "See the attached image(s).";
const ATTACHMENT_PROMPT_HEADER: &str = "Attached images (local files — open them to view):";

/// Queue rows keep the user's editable text separate from attachment paths.
/// Rebuild the transcript/harness transport only when the row is dispatched.
///
/// Older clients stored the already-expanded prompt in `text`; recognize the
/// exact trailer implied by `attachments` so those rows are not expanded a
/// second time after an upgrade.
fn queued_message_prompt(text: &str, attachments: &[String]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let refs = attachments
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let trailer = format!("\n\n{ATTACHMENT_PROMPT_HEADER}\n{refs}");
    let body = text.strip_suffix(&trailer).unwrap_or(text);
    let body = if body.trim().is_empty() {
        ATTACHMENT_ONLY_PROMPT
    } else {
        body
    };
    format!("{body}{trailer}")
}

fn queue_text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BeginQueueEditOutcome {
    Acquired {
        lease_id: String,
        text: String,
        attachments: Vec<String>,
        base_text_hash: String,
        expires_at_ms: i64,
    },
    Locked {
        owner_device_id: String,
        expires_at_ms: i64,
    },
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RenewQueueEditOutcome {
    Renewed { expires_at_ms: i64 },
    Lost,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishQueueEditAction {
    Commit,
    Cancel,
    Discard,
    ReleaseUnchanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FinishQueueEditOutcome {
    Committed,
    Cancelled,
    Discarded,
    Released,
    Conflict { current_text: String },
    Lost,
    Missing,
}

/// Content and its historical presentation cutoff travel atomically, even
/// when the watch coalesces several backfill and live commits.
#[derive(Clone, Default)]
pub struct TranscriptSnapshot {
    /// The joined transcript. Entries are shared with the incremental mirror
    /// (and across snapshots), so an unchanged entry is the same `Arc`.
    pub entries: Arc<Vec<zeron_doc::SharedEntry>>,
    pub replay_baseline: Arc<zeron_doc::TranscriptBaseline>,
}

/// One open chat doc: the `SessionDoc`, its change plumbing, and the room client.
pub struct ChatDocHandle {
    chat_id: String,
    device_id: String,
    doc: Arc<SessionDoc>,
    messages_tx: watch::Sender<TranscriptSnapshot>,
    /// Serialize historical imports with publication so an async doc-change
    /// task cannot publish recovered content before its presentation cutoff.
    transcript_import: Mutex<()>,
    transcript_history: Arc<Mutex<crate::transcript_history::TranscriptHistory>>,
    /// The joined transcript, kept current from doc diff events while watched
    /// so a commit costs the entries it touched, not the whole transcript.
    transcript_mirror: Arc<zeron_doc::TranscriptMirror>,
    /// What the doc callback saw change since the publisher last woke. Set
    /// before `changed_tx` fires; the publisher takes it, so a wake that finds
    /// no `messages`/`queue` bit republishes nothing.
    publish_bits: Arc<zeron_doc::AtomicChangeMask>,
    /// The same for the dispatcher (drains + snapshot deadline), kept apart so
    /// one consumer taking its bits never hides a change from the other.
    dispatch_bits: Arc<zeron_doc::AtomicChangeMask>,
    /// Bumped only when the `commands` ledger changed. The control worker and
    /// the prompt/control wait loops wake on this instead of every doc change
    /// or a 20 ms poll.
    commands_tx: watch::Sender<u64>,
    /// The decoded ledger, re-read only after a `commands` change.
    commands_cache: Arc<zeron_doc::CommandsCache>,
    /// Pending-message queue watch (WatchQueue). Cheap to rebuild — a handful
    /// of short rows — so unlike the transcript mirror it publishes on every
    /// change without a dirty flag.
    queue_tx: watch::Sender<Vec<QueuedMessage>>,
    /// Serializes everything that TAKES from the queue. Both the doc-change
    /// task and the turn-end status watcher call `drain_queue`, and nothing
    /// keeps those two apart: without this they interleave across the
    /// `dispatch` await, each taking a different head and each sending, so a
    /// queue meant to release one message releases all of them.
    ///
    /// It also covers the gap a send-now's interrupt opens — between stopping
    /// the turn and starting its own the chat reads Idle, and an idle chat with
    /// a queue is exactly what the flush drains.
    drain_lock: tokio::sync::Mutex<()>,
    /// Serialize prompt commands while still allowing interrupt/input controls.
    command_drain_lock: tokio::sync::Mutex<()>,
    /// Queue rows held as explicit steers for a turn-boundary agent. They
    /// lead ordinary queued rows, in the order they were steered.
    steered_rows: Mutex<Vec<String>>,
    /// Sends held for a turn-boundary agent's turn end (rather than queued
    /// by the user), with the run configuration each was sent with: the row
    /// carries only its text and images.
    held_sends: Mutex<HashMap<String, zeron_proto::RunRequest>>,
    /// The latest Stop command that has acted. A prompt sent before it that
    /// only executes now (a loaded host, a slow agent) was cancelled by it:
    /// running it would start the stopped work again in a fresh runtime.
    stopped_by: Mutex<Option<String>>,
    /// An explicit user interrupt freezes automatic queue delivery. The next
    /// explicit prompt or queue send resumes it; incidental doc/status changes
    /// must not turn Cancel into "send the next row".
    queue_paused: AtomicBool,
    /// True when the doc changed while nobody watched: the mirror rebuild is
    /// deferred to the next `watch_messages` attach instead of paid per commit.
    mirror_dirty: AtomicBool,
    /// Epoch ms of the last open/watch touch — the LRU eviction key.
    last_access: AtomicI64,
    /// User navigation only. Opens, watches, writes and reconnects never bump it.
    last_focus: AtomicU64,
    sync_focus_served: AtomicU64,
    /// Last known snapshot blob size — the eviction budget estimate's input.
    snapshot_bytes: AtomicUsize,
    /// The sync generation this handle was BUILT for (1 = legacy s2,
    /// 2 = chat2). Gen-1 handles no longer join any room (the s2 client is
    /// gone); they serve the local fat doc read-only until the host's seed
    /// flips the chat to chat2. The staleness checks compare this against
    /// the registry — inferring mode from `chat2_local_sub` misread
    /// edge-less chat2 handles (no subscription is ever installed offline)
    /// as stale s2 and retired them on every open, dropping the doc out
    /// from under live runs.
    room_gen: u32,
    /// A threshold checkpoint POST is in flight (review H1: the quiesce
    /// tick must not stack concurrent full-snapshot uploads).
    checkpointing: Arc<AtomicBool>,
    sync_cadence: Mutex<SyncCadence>,
    writer_wake: Arc<tokio::sync::Notify>,
    turn_end_pending: Arc<AtomicBool>,
    /// Set when a chat2 seed replaced this handle's lineage on disk: every
    /// further snapshot save from this handle is a stale FAT doc that would
    /// clobber the thin one — retired handles never persist again (unless no
    /// thin lineage exists on disk at all; `save_snapshot` double-checks, so
    /// a doc that was never seeded can't lose its only copy).
    retired: AtomicBool,
    purged: AtomicBool,
    /// chat2 relay client (docs/chat2-sync.md C3) — populated once the
    /// registry names roomGen 2 for this chat and the join resolves.
    chat2: Mutex<Option<zeron_sync::ChatClient>>,
    sync_started: AtomicBool,
    sync_requested: AtomicBool,
    sync_background: AtomicBool,
    sync_last_started: AtomicI64,
    sync_wait_since: AtomicI64,
    /// A fairness admission gets a full turn, even under repeated navigation.
    sync_service_until: AtomicI64,
    sync_wake_version: AtomicI64,
    sync_wake_ticket: AtomicU64,
    sync_cancel: Mutex<CancellationToken>,
    writers: Arc<AtomicUsize>,
    pub(crate) persistence: Option<Arc<crate::chat_persistence::ChatPersistence>>,
    /// Local commits made before the relay connects (the dial can take up
    /// to a minute; offline, forever): buffered here by the subscription
    /// below and drained into the client on join (review B3 — a user
    /// message typed during the dial must not silently never sync).
    chat2_pending_local: Mutex<Vec<(String, Vec<u8>)>>,
    publication_failed: AtomicBool,
    /// Local-update feed into the chat2 client (drop = unsubscribe).
    chat2_local_sub: Mutex<Option<loro::Subscription>>,
    publication_barrier: Mutex<Option<Arc<zeron_sync::SnapshotBarrier>>>,
    own_ops_order: Mutex<Option<Arc<Mutex<crate::chat2_host::OwnOpsOrder>>>>,
    #[cfg(test)]
    before_outbox_submit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Doc subscription (drop = unsubscribe) — bumps the change watch on every commit.
    _sub: loro::Subscription,
}

impl crate::outbox_writer::OutboxSink for ChatDocHandle {
    /// The outbox writer has attempted this update's row. Runs under the client
    /// lock, as the local-update hook used to: the join's store+drain holds the
    /// same lock, so the update is either in the outbox that drain reads or
    /// enqueued here after the client exists, never dropped between.
    fn outbox_written(&self, batch_id: String, bytes: Vec<u8>, durable: bool) {
        let client_guard = lock(&self.chat2);
        if !durable {
            self.publication_failed.store(true, Ordering::Release);
            lock(&self.chat2_pending_local).push((batch_id.clone(), bytes.clone()));
            tracing::error!(chat = %self.chat_id, "chat2: durable outbox write failed");
        }
        if let Some(client) = &*client_guard {
            // The row is already written (or held back as non-durable): do
            // not INSERT it a second time.
            client.enqueue_batch_journaled(batch_id, bytes, durable);
        }
    }
}

/// A running agent explicitly owns a writer lease until its final cleanup.
/// Reference counting remains a conservative compatibility guard for read APIs.
struct ResetFlag(Arc<AtomicBool>);
impl Drop for ResetFlag {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Upper bound on one wait for a ledger change in the command wait loops; the
/// loop then re-checks its non-ledger conditions (deadline, claim set, turn
/// state) from the cached ledger. Not a poll: a ledger change wakes it at once.
const WAIT_RECHECK: std::time::Duration = std::time::Duration::from_millis(250);

/// Wait until the ledger generation moves or `limit` passes. `false` when the
/// sender is gone (the handle is being torn down).
async fn wait_for_ledger(rx: &mut watch::Receiver<u64>, limit: std::time::Duration) -> bool {
    match tokio::time::timeout(limit, rx.changed()).await {
        Ok(changed) => changed.is_ok(),
        Err(_) => true,
    }
}

/// How a command drain pass ended. `complete` only when it ran to "no eligible
/// pending command"; every early exit (not host, quiesced, no sessions, an
/// attachment wait, a ledger write failure) is incomplete, so the change worker
/// retries it on the next doc change of ANY kind, exactly as it did when every
/// change ran a drain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainOutcome {
    pub complete: bool,
}

impl DrainOutcome {
    const COMPLETE: Self = Self { complete: true };
    const INCOMPLETE: Self = Self { complete: false };
}

/// One drain counted in [`DocHostInner::dispatching`] until dropped.
struct DispatchGuard(Arc<AtomicUsize>);
impl Drop for DispatchGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct DocWriter {
    doc: Arc<SessionDoc>,
    writers: Arc<AtomicUsize>,
    wake: Arc<tokio::sync::Notify>,
    turn_end_pending: Arc<AtomicBool>,
}
impl std::ops::Deref for DocWriter {
    type Target = SessionDoc;
    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}
impl Drop for DocWriter {
    fn drop(&mut self) {
        if self.writers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.turn_end_pending.store(true, Ordering::Release);
        }
        self.wake.notify_one();
    }
}
impl Drop for ChatDocHandle {
    fn drop(&mut self) {
        lock(&self.sync_cancel).cancel();
        self.writer_wake.notify_one();
    }
}

/// The provider the chat's own latest switch seam moved to: `None` when the
/// chat never switched, `Some(None)` for a seam naming no known harness.
/// Seams above a fork's seam were copied from the source chat and don't
/// count: the fork's own provider history starts there.
pub(crate) fn latest_switch(entries: &[SessionMessageEntry]) -> Option<Option<HarnessId>> {
    for entry in entries.iter().rev() {
        for part in &entry.parts {
            match part {
                MessagePart::Switch { to_harness, .. } => {
                    return Some(to_harness.as_deref().and_then(HarnessId::from_slug));
                }
                MessagePart::Fork { .. } => return None,
                _ => {}
            }
        }
    }
    None
}

impl ChatDocHandle {
    /// The command ledger, shared until the next `commands` change. Every
    /// reader of this chat's ledger goes through here instead of a deep
    /// `read_commands`, which scales with chat age.
    pub(crate) fn commands(&self) -> Result<Arc<Vec<SessionCommandEntry>>, DocError> {
        self.commands_cache.get(&self.doc)
    }

    /// Deep ledger reads the commands cache has performed (perf tests assert
    /// that a stream of message commits leaves it untouched).
    #[doc(hidden)]
    pub fn commands_cache_fills(&self) -> u64 {
        self.commands_cache.fills()
    }

    /// Cross-check failures of the commands cache against `read_commands`.
    #[doc(hidden)]
    pub fn commands_cache_mismatches(&self) -> u64 {
        self.commands_cache.mismatches()
    }

    /// The turn ending now is followed at once by a message the user has
    /// already sent: one still on its way through the command log (a burst
    /// the agent outran), or one held for a turn-boundary agent's turn end.
    /// The work goes on, so the end is not a completion to notify.
    pub(crate) fn continues_with_sent_message(&self) -> bool {
        // Not yet in the transcript: delivery writes the user's message
        // before dispatching it, so the command that started the turn now
        // ending (still marked pending a moment longer) never counts.
        let in_flight = self.commands().is_ok_and(|commands| {
            let pending: Vec<&str> = commands
                .iter()
                .filter(|c| c.status == SessionCommandStatus::Pending)
                .filter_map(|c| match &c.payload {
                    SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
                    SessionCommandPayload::Steer { message_id, .. } => {
                        Some(message_id.as_deref().unwrap_or(c.id.as_str()))
                    }
                    _ => None,
                })
                .collect();
            !pending.is_empty()
                && self.doc.read_entries().is_ok_and(|entries| {
                    pending
                        .iter()
                        .any(|id| !entries.iter().any(|e| e.id == *id))
                })
        });
        if in_flight {
            return true;
        }
        if self.queue_paused.load(Ordering::Acquire) {
            return false;
        }
        let Ok(Some(head)) = self.doc.read_queue().map(|q| q.into_iter().next()) else {
            return false;
        };
        head.delivery_gate.is_none()
            && (lock(&self.held_sends).contains_key(&head.id)
                || lock(&self.steered_rows).contains(&head.id))
    }

    /// Live views and agents retain their documents and resist idle retirement.
    /// At capacity their transports can yield without dropping these leases.
    fn sync_protected(&self) -> bool {
        self.messages_tx.receiver_count() > 0
            || self.queue_tx.receiver_count() > 0
            || self.writers.load(Ordering::Acquire) > 0
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub(crate) fn commit_voice(
        &self,
        transcript: &zeron_proto::voice::VoiceTranscript,
    ) -> Result<Option<String>, EngineError> {
        let _owner = lock(&self.transcript_import);
        zeron_doc::voice::commit_voice_transcript(
            &self.doc,
            transcript,
            &self.device_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(EngineError::from)
    }

    pub fn doc(&self) -> &SessionDoc {
        &self.doc
    }

    pub fn writer(&self) -> DocWriter {
        if self.writers.fetch_add(1, Ordering::AcqRel) == 0 {
            lock(&self.sync_cadence).stream_since_checkpoint = Some(tokio::time::Instant::now());
        }
        self.writer_wake.notify_one();
        DocWriter {
            doc: self.doc.clone(),
            writers: self.writers.clone(),
            wake: self.writer_wake.clone(),
            turn_end_pending: self.turn_end_pending.clone(),
        }
    }

    pub fn doc_arc(&self) -> Arc<SessionDoc> {
        self.doc.clone()
    }

    /// Joined transcript watch — re-sent on every doc change (WatchDocMessages).
    ///
    /// Attach-time refresh: the mirror is only maintained while watched, so a
    /// doc that changed unwatched materializes here, once, instead of on every
    /// commit it sat through in the background.
    pub fn watch_messages(&self) -> watch::Receiver<TranscriptSnapshot> {
        self.touch();
        // Attach is a user signal: verify a quiet room is actually alive
        // (a doc-wedged DO keeps answering pings while delivering nothing,
        // and the background probe cadence can be hours out). Coalescing
        // no-op on a healthy or recently-active room.
        if let Some(chat2) = lock(&self.chat2).as_ref() {
            chat2.probe();
        }
        // Subscribe BEFORE the dirty check: a commit racing this attach then
        // sees a live receiver and publishes, instead of re-marking dirty
        // after our refresh and leaving the new watcher a cleared mirror.
        let _import = lock(&self.transcript_import);
        let rx = {
            if self.messages_tx.receiver_count() == 0 {
                // A new viewing session must not inherit the former viewer's
                // live-part protection, even if no commit happened while away.
                *lock(&self.transcript_history) = Default::default();
                self.transcript_mirror.reset();
                self.mirror_dirty.store(true, Ordering::Release);
            }
            self.messages_tx.subscribe()
        };
        if self.mirror_dirty.load(Ordering::Acquire) {
            self.publish_messages_locked();
        }
        rx
    }

    /// Queue watch — the composer's held messages, re-sent on every doc change.
    pub fn watch_queue(&self) -> watch::Receiver<Vec<QueuedMessage>> {
        self.touch();
        let rx = self.queue_tx.subscribe();
        self.publish_queue();
        rx
    }

    /// Where a newly steered row goes: after the rows already steered, ahead
    /// of every ordinary row. Records `id` as steered.
    fn steer_slot(&self, id: &str) -> Result<usize, DocError> {
        let mut steered = lock(&self.steered_rows);
        let queue = self.doc.read_queue()?;
        steered.retain(|row| queue.iter().any(|q| &q.id == row));
        let slot = queue
            .iter()
            .take_while(|row| steered.contains(&row.id))
            .count();
        steered.push(id.to_string());
        Ok(slot)
    }

    fn publish_queue(&self) {
        self.publish_queue_with(|| self.doc.read_queue());
    }

    fn publish_queue_with(&self, read: impl FnOnce() -> Result<Vec<QueuedMessage>, DocError>) {
        // Serialize the read with the send. A publisher paused after its read
        // must not overwrite a drain's newer snapshot when it resumes.
        self.queue_tx.send_if_modified(|slot| match read() {
            Ok(items) if *slot != items => {
                *slot = items;
                true
            }
            Ok(_) => false,
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "queue read failed");
                false
            }
        });
    }

    fn touch(&self) {
        self.last_access.store(now_ms(), Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        lock(&self.chat2).is_some()
    }

    /// Write a complete user message entry, idempotent by id (the client-minted message
    /// id — a re-executed command or optimistic echo never duplicates the entry).
    pub fn write_user_message(
        &self,
        message_id: &str,
        text: &str,
        created_at: i64,
    ) -> Result<(), DocError> {
        if self.doc.read_entries()?.iter().any(|e| e.id == message_id) {
            return Ok(());
        }
        self.doc.push_message(&SessionMessageEntry {
            id: message_id.to_string(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_string(),
            }],
            created_at,
            device_id: self.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })
    }

    /// Record that the chat moved from provider `from` to provider `to`: a
    /// system entry carrying the [`MessagePart::Switch`] seam, plus the
    /// `providerSwitched` meta flag that makes the next fresh provider session
    /// owe the transcript. Idempotent against a retried dispatch: nothing is
    /// written while the chat's latest seam already points at `to`. Returns
    /// whether a seam was written.
    pub fn write_provider_switch(
        &self,
        from: (HarnessId, &str),
        to: (HarnessId, &str),
        created_at: i64,
    ) -> Result<bool, DocError> {
        let ((from_id, from), (to_id, to)) = (from, to);
        let entries = self.doc.read_entries()?;
        if entries.is_empty() {
            return Ok(false);
        }
        if latest_switch(&entries) == Some(Some(to_id)) {
            return Ok(false);
        }
        let id = format!("switch:{created_at}");
        // The send that switches wrote its message first: the seam goes
        // before it (and any other unanswered message), so it reads as the
        // new provider's question, not the old one's.
        let unanswered = entries
            .iter()
            .rev()
            .take_while(|entry| entry.role == MessageRole::User)
            .count();
        self.doc.insert_message_before_last(
            unanswered,
            &SessionMessageEntry {
                id: id.clone(),
                role: MessageRole::System,
                parts: vec![MessagePart::Switch {
                    id,
                    from: from.to_string(),
                    to: to.to_string(),
                    from_harness: Some(from_id.as_str().to_owned()),
                    to_harness: Some(to_id.as_str().to_owned()),
                }],
                created_at,
                device_id: self.device_id.clone(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            },
        )?;
        self.doc.mark_provider_switched()?;
        Ok(true)
    }

    /// Recovery sweep: settle this device's running subagent chips (including
    /// chips in completed parent turns), then stamp abandoned `streaming`
    /// entries `aborted`, appending
    /// `note` as a visible error part so the transcript says WHY the turn
    /// ended (zeron folded "Run interrupted by backend restart" the same
    /// way). Returns the stamped entries' `(id, created_at)` — recovery uses
    /// them for the resume-freshness check.
    pub fn mark_abandoned_streams(&self, note: &str) -> Result<Vec<(String, i64)>, DocError> {
        let mut stamped = Vec::new();
        let mut chips_changed = false;
        for entry in self.doc.read_entries()? {
            if entry.role != MessageRole::Assistant || entry.device_id != self.device_id {
                continue;
            }
            for part in &entry.parts {
                if let MessagePart::Tool {
                    id,
                    subagent_status: Some(SubagentStatus::Running),
                    ..
                } = part
                {
                    chips_changed |=
                        self.doc
                            .update_subagent_chip(id, None, Some("failed"), None)?;
                }
            }
            if entry.status == Some(MessageStatus::Streaming)
                && self
                    .doc
                    .set_message_status(&entry.id, MessageStatus::Aborted)?
            {
                let part_id = format!("{}-recovery", entry.id);
                if let Err(err) = self.doc.append_error_part(&entry.id, &part_id, note) {
                    tracing::warn!(chat = %self.chat_id, error = %err, "recovery note append failed");
                }
                stamped.push((entry.id.clone(), entry.created_at));
            }
        }
        if chips_changed || !stamped.is_empty() {
            self.publish_messages();
        }
        Ok(stamped)
    }

    fn publish_messages(&self) {
        let _import = lock(&self.transcript_import);
        self.publish_messages_locked();
    }

    // Caller holds transcript_import, shared with attach and mirror clearing.
    fn publish_messages_locked(&self) {
        zeron_counters::add(zeron_counters::C::PublishMessagesCalls, 1);
        let _timer = zeron_counters::timer(zeron_counters::C::PublishMessagesNanos);
        self.mirror_dirty.store(false, Ordering::Release);
        // Incremental: only the entry maps the observer saw change are
        // re-decoded. The mirror cross-checks itself against the full rebuild
        // (debug builds, or ZERON_PERF_VERIFY=1 sampled) and falls back to it.
        let joined = self.transcript_mirror.refresh(&self.doc);
        let replay_baseline = self
            .transcript_mirror
            .with_raw(|raw| lock(&self.transcript_history).snapshot(self.doc.doc(), raw));
        // send_replace: update the watch even with no subscribers yet, so a
        // late subscriber's first borrow sees the current transcript.
        self.messages_tx.send_replace(TranscriptSnapshot {
            entries: joined,
            replay_baseline,
        });
    }

    /// The pre-mirror publish: a full `read_entries` + join, kept as the
    /// reference the incremental path is checked against.
    #[cfg(test)]
    fn publish_messages_full(&self) -> Result<TranscriptSnapshot, DocError> {
        let entries = self.doc.read_entries()?;
        let replay_baseline =
            lock(&self.transcript_history).snapshot(self.doc.doc(), &mut entries.iter());
        Ok(TranscriptSnapshot {
            entries: Arc::new(
                zeron_doc::join_continuation_entries(entries)
                    .into_iter()
                    .map(Arc::new)
                    .collect(),
            ),
            replay_baseline,
        })
    }

    pub(crate) fn import_transcript<T>(&self, import: impl FnOnce() -> T) -> T {
        let _guard = lock(&self.transcript_import);
        import()
    }

    /// Per-commit publish path: unwatched docs just mark the mirror dirty —
    /// rebuilding a full transcript nobody reads was a per-tick cost on every
    /// open doc (and kept a second transcript copy hot).
    fn publish_messages_if_watched(&self) {
        // Serialize the receiver check AND clear with attach. Otherwise an
        // unwatched worker can clear the mirror after a new watcher rebuilt it.
        let _import = lock(&self.transcript_import);
        if self.messages_tx.receiver_count() == 0 {
            self.mirror_dirty.store(true, Ordering::Release);
            // Shrink the stale mirror: watch_messages rebuilds on attach.
            self.messages_tx.send_replace(TranscriptSnapshot::default());
            *lock(&self.transcript_history) = Default::default();
            self.transcript_mirror.reset();
        } else {
            self.publish_messages_locked();
        }
    }

    /// Rough resident cost for the LRU budget.
    fn resident_estimate(&self) -> usize {
        let bytes = self
            .snapshot_bytes
            .load(Ordering::Relaxed)
            .max(self.persistence.as_ref().map_or(0, |p| p.snapshot_bytes()));
        (bytes * RESIDENT_BYTES_PER_SNAPSHOT_BYTE).max(DOC_RESIDENT_FLOOR_BYTES)
    }
}

/// Preserve the decoder's treatment of malformed and scalar list entries.
fn messages_list_empty(doc: &loro::LoroDoc) -> Result<bool, String> {
    if doc.get_list("messages").is_empty() {
        return Ok(true);
    }
    SessionDoc::from_doc(doc.clone())
        .read_entries()
        .map(|entries| entries.is_empty())
        .map_err(|err| err.to_string())
}

struct SalvageScan {
    imports: u64,
    kind: SalvageScanKind,
}

enum SalvageScanKind {
    /// Already salvage-checked. No snapshot was read.
    Skip,
    /// Persisted transcript is non-empty; scan may certify a stable full base.
    Healthy,
    /// Empty, and nothing on disk can refill it. Left unmarked.
    Absent,
    /// Blank chat2 doc with a non-empty fat source to re-append.
    Recover {
        bytes: Vec<u8>,
        entries: Vec<SessionMessageEntry>,
    },
}

fn import_snapshot(bytes: &[u8], imports: &mut u64) -> Result<loro::LoroDoc, String> {
    let raw = loro::LoroDoc::new();
    raw.import(bytes).map_err(|err| err.to_string())?;
    *imports += 1;
    Ok(raw)
}

/// Load, import, and decide. Runs on the blocking pool: the boot sweep used
/// to do this on an async worker for every journaled chat.
fn scan_salvage(
    store: &DocsStore,
    chat_id: &str,
    edge_configured: bool,
) -> Result<SalvageScan, String> {
    if store
        .salvage_checked(chat_id)
        .map_err(|err| err.to_string())?
    {
        return Ok(SalvageScan {
            imports: 0,
            kind: SalvageScanKind::Skip,
        });
    }
    let mut imports = 0u64;
    let stored = store
        .load_chat_state(chat_id)
        .map_err(|err| err.to_string())?;
    if let Some(state) = &stored {
        // A pre-chat2 snapshot may be discarded by open() during adoption;
        // its entries are a recovery SOURCE, not proof the new doc is healthy.
        if state.epoch >= crate::chat2_host::CHAT2_DOC_EPOCH || !edge_configured {
            let raw = &state.doc;
            imports += 1;
            let snapshot_healthy = !messages_list_empty(raw)?;
            let updates = store
                .pending_chat_updates(chat_id)
                .map_err(|err| err.to_string())?;
            if snapshot_healthy
                && updates.is_empty()
                && state.delta_count == 0
                && !state.needs_compaction
            {
                store
                    .mark_salvage_checked(chat_id, &state.base_snapshot, state.epoch)
                    .map_err(|err| err.to_string())?;
            }
            for (_, update) in updates {
                raw.import(&update).map_err(|err| err.to_string())?;
            }
            if !messages_list_empty(raw)? {
                return Ok(SalvageScan {
                    imports,
                    kind: SalvageScanKind::Healthy,
                });
            }
        }
    }
    let rollback_id = format!("{chat_id}.pre-chat2");
    let source = store
        .load_snapshot(&rollback_id)
        .map_err(|err| err.to_string())?
        .or_else(|| {
            stored
                .filter(|s| s.epoch < crate::chat2_host::CHAT2_DOC_EPOCH)
                .and_then(|s| s.doc.export(loro::ExportMode::Snapshot).ok())
        });
    let Some(bytes) = source else {
        return Ok(SalvageScan {
            imports,
            kind: SalvageScanKind::Absent,
        });
    };
    let raw = import_snapshot(&bytes, &mut imports)?;
    let fat = SessionDoc::from_doc(raw);
    if messages_list_empty(fat.doc())? {
        return Ok(SalvageScan {
            imports,
            kind: SalvageScanKind::Absent,
        });
    }
    let rebuilt = zeron_doc::rebuild::rebuild_thin_doc(&fat).map_err(|err| err.to_string())?;
    let entries = rebuilt.doc.read_entries().map_err(|err| err.to_string())?;
    if entries.is_empty() {
        return Ok(SalvageScan {
            imports,
            kind: SalvageScanKind::Absent,
        });
    }
    Ok(SalvageScan {
        imports,
        kind: SalvageScanKind::Recover { bytes, entries },
    })
}

impl DocHost {
    pub fn new(store: Arc<DocsStore>, config: DocHostConfig) -> Self {
        let host = Self {
            inner: Arc::new(DocHostInner {
                outbox: Arc::new(crate::outbox_writer::OutboxWriter::new(store.clone())),
                store,
                config,
                sessions: Mutex::new(None),
                quiesced: AtomicBool::new(false),
                quiesce_generation: AtomicU64::new(0),
                dispatching: Arc::new(AtomicUsize::new(0)),
                workspace: OnceLock::new(),
                repos: OnceLock::new(),
                project_action_runtime: OnceLock::new(),
                shutdown: CancellationToken::new(),
                edge_disconnected: AtomicBool::new(false),
                tasks: TaskTracker::new(),
                handles: Mutex::new(HashMap::new()),
                document_loads: AtomicU64::new(0),
                #[cfg(test)]
                salvage_imports: AtomicU64::new(0),
                focus_clock: AtomicU64::new(0),
                opening: Mutex::new(()),
                seeding: Mutex::new(HashSet::new()),
                seed_waiting: Mutex::new(HashSet::new()),
                drain_waiting: Mutex::new(HashSet::new()),
                uploads: OnceLock::new(),
                connectivity: OnceLock::new(),
                connectivity_started: AtomicBool::new(false),
                transfers: watch::channel(Vec::new()).0,
                connectivity_grace: Mutex::new(DegradeGrace::default()),
                executing: Mutex::new(HashSet::new()),
                links: OnceLock::new(),
                http: reqwest::Client::builder()
                    .pool_max_idle_per_host(2)
                    .pool_idle_timeout(std::time::Duration::from_secs(10))
                    .connect_timeout(std::time::Duration::from_secs(15))
                    .read_timeout(std::time::Duration::from_secs(30))
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
            }),
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            host.spawn_sync_scheduler();
            host.spawn_remote_wake_delivery();
        }
        host
    }

    /// Every background task rides the tracker, raced against the shutdown
    /// token: the loops' own exits stay authoritative in normal operation;
    /// the token is the retirement override.
    fn spawn_worker(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = fut => {}
            }
        });
    }

    /// `spawn_worker` for sites that pre-resolve a runtime handle (callers
    /// reachable from bare sync contexts, where `tasks.spawn` would panic).
    fn spawn_worker_on(
        &self,
        runtime: &tokio::runtime::Handle,
        fut: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn_on(
            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = fut => {}
                }
            },
            runtime,
        );
    }

    /// The sessions engine, once wired. `None` before assembly or after
    /// `shutdown_workers` — callers treat both as "executor unavailable".
    fn sessions(&self) -> Option<SessionsEngine> {
        lock(&self.inner.sessions).clone()
    }

    /// Entered by a drain that may start work; `None` while quiesced. Count
    /// first, check second (both SeqCst): a quiesce that set its flag then
    /// either sees this drain counted or this drain sees the flag.
    fn dispatch_guard(&self) -> Option<DispatchGuard> {
        self.inner.dispatching.fetch_add(1, Ordering::SeqCst);
        let guard = DispatchGuard(self.inner.dispatching.clone());
        (!self.inner.quiesced.load(Ordering::SeqCst)).then_some(guard)
    }

    /// Before a Cloud sleep stops this engine: start nothing the stop would
    /// cut off. Commands arriving from here on stay pending in their chat's
    /// room and run on the woken machine (its boot reconcile finds them).
    /// `Err(why)` when a turn is in flight or a command is already waiting:
    /// the sleep is declined and work resumes at once. A quiesce the stop
    /// never follows lifts itself after [`QUIESCE_LEASE`].
    pub async fn quiesce_for_sleep(&self) -> Result<(), String> {
        let generation = self.inner.quiesce_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.quiesced.store(true, Ordering::SeqCst);
        // Drains already past their check finish dispatching first.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.inner.dispatching.load(Ordering::SeqCst) > 0 {
            if tokio::time::Instant::now() >= deadline {
                self.lift_quiesce();
                return Err("work is still being dispatched".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let running = self
            .sessions()
            .map_or(0, |sessions| sessions.active_count());
        let busy = if running > 0 {
            Some(format!("{running} chat(s) have a turn in flight"))
        } else if self.has_waiting_commands() {
            Some("a message is waiting to run".to_string())
        } else {
            None
        };
        if let Some(why) = busy {
            self.lift_quiesce();
            return Err(why);
        }
        let host = self.clone();
        self.spawn_worker(async move {
            tokio::time::sleep(QUIESCE_LEASE).await;
            if host.inner.quiesce_generation.load(Ordering::SeqCst) == generation {
                tracing::warn!(
                    "quiesced for a sleep that never stopped this engine; resuming work"
                );
                host.lift_quiesce();
            }
        });
        Ok(())
    }

    /// End a quiesce and run everything that queued meanwhile.
    pub fn lift_quiesce(&self) {
        if !self.inner.quiesced.swap(false, Ordering::SeqCst) {
            return;
        }
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
    }

    /// A hosted chat has a command no drain has executed yet.
    fn has_waiting_commands(&self) -> bool {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
        handles.iter().any(|handle| {
            self.is_host(&handle.chat_id)
                && handle.commands().is_ok_and(|commands| {
                    commands
                        .iter()
                        .any(|c| c.status == SessionCommandStatus::Pending && !is_processed(&c.id))
                })
        })
    }

    /// Wire the sessions engine (engine assembly; see `SessionsEngine::set_doc_host`).
    pub fn set_sessions(&self, sessions: SessionsEngine) {
        let statuses = sessions.watch_sessions();
        {
            // First set wins (the OnceLock contract this slot replaced).
            let mut slot = lock(&self.inner.sessions);
            if slot.is_none() {
                *slot = Some(sessions);
            }
        }
        // Commands may already be pending in warm-opened docs.
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
        self.spawn_sync_status_watcher(statuses.clone());
        self.spawn_queue_flush_watcher(statuses);
    }

    /// The turn-end hook for held messages. Doc changes drive `drain_queue` for
    /// everything else, but a turn ENDING is not a doc change the queue can
    /// see — so watch session status instead and re-drain every warm chat.
    /// `drain_queue` is a cheap no-op for empty queues and busy agents, which
    /// is why this can afford to be indiscriminate.
    fn spawn_queue_flush_watcher(&self, mut statuses: watch::Receiver<Vec<zeron_proto::Session>>) {
        let host = self.clone();
        self.spawn_worker(async move {
            while statuses.changed().await.is_ok() {
                let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
                for handle in handles {
                    host.drain_queue(&handle).await;
                }
            }
        });
    }

    /// The cached streaming state of this chat's local session, seeded from the
    /// engine's current status on first use. The status watcher only sees
    /// changes: a handle opened while its session was already idle (a resumed
    /// or parked run) would otherwise read as "writer without status" and tick
    /// at 1 Hz on streaming thresholds until some unrelated status change.
    fn session_active(&self, handle: &ChatDocHandle) -> Option<bool> {
        if let Some(known) = lock(&handle.sync_cadence).session_streaming {
            return Some(known);
        }
        let seeded = self
            .sessions()
            .and_then(|sessions| sessions.session_status(&handle.chat_id))
            .map(|session| session_streams(session.status))?;
        Some(
            *lock(&handle.sync_cadence)
                .session_streaming
                .get_or_insert(seeded),
        )
    }

    fn spawn_sync_status_watcher(&self, mut statuses: watch::Receiver<Vec<zeron_proto::Session>>) {
        let host = self.clone();
        self.spawn_worker(async move {
            while statuses.changed().await.is_ok() {
                let snapshot = statuses.borrow_and_update().clone();
                let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
                for handle in handles {
                    if let Some(session) = snapshot
                        .iter()
                        .find(|s| s.chat_id == handle.chat_id && s.device_id == handle.device_id)
                    {
                        let active = session_streams(session.status);
                        let mut cadence = lock(&handle.sync_cadence);
                        let previous = cadence.session_streaming.replace(active);
                        if previous != Some(active) {
                            if active {
                                cadence.stream_since_checkpoint = Some(tokio::time::Instant::now());
                            } else if previous == Some(true) {
                                handle.turn_end_pending.store(true, Ordering::Release);
                            }
                            handle.writer_wake.notify_one();
                        }
                    }
                }
            }
        });
    }

    /// Retire this host's workers (runtime replacement, e.g. sign-out): cancel
    /// and await every spawned task, drop every open chat handle (ending the
    /// weak-keyed room/join loops and watcher streams), and sever the sessions
    /// back-edge so the replaced engine graph can actually drop. Idempotent.
    pub async fn shutdown_workers(&self) {
        // Cadence defers sync work to a later pass: that pass must not be the
        // one the cancel below cuts off.
        self.flush_deferred_sync().await;
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
        // Deliver all queued commits before stopping the room actors. Their
        // final persistence may itself run a snapshot barrier.
        let outbox = self.inner.outbox.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || outbox.flush()).await {
            tracing::error!(%error, "shutdown outbox flush failed");
        }
        // Stop room actors before the final snapshot so shutdown does not
        // leave a scheduled debounce behind an already-dropped document.
        let clients: Vec<_> = lock(&self.inner.handles)
            .values()
            .filter_map(|handle| lock(&handle.chat2).take())
            .collect();
        futures::future::join_all(clients.into_iter().map(|client| client.shutdown())).await;
        // Snapshot open docs BEFORE releasing their handles: the handles map
        // holds the only strong doc refs, and an unflushed doc dies with it.
        let host = self.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || host.flush_all()).await {
            tracing::error!(%error, "shutdown snapshot flush failed");
        }
        // Take the map under the lock, drop the handles outside it.
        let handles = std::mem::take(&mut *lock(&self.inner.handles));
        drop(handles);
        lock(&self.inner.seeding).clear();
        lock(&self.inner.seed_waiting).clear();
        lock(&self.inner.sessions).take();
    }

    /// One last pass of the deferred sync duties for every open chat, before
    /// the workers are cancelled: a checkpoint that streaming thresholds held
    /// back (down to the turn-end thresholds, since nothing streams any more)
    /// and a tail that was throttled, failed or flagged by a turn end that no
    /// pass has consumed. Chats owing nothing cost a few atomic reads. Bounded
    /// by [`SHUTDOWN_SYNC_BUDGET`] so an unreachable edge cannot hold shutdown.
    async fn flush_deferred_sync(&self) {
        if self.inner.config.edge.is_none() {
            return;
        }
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        let work = handles.into_iter().map(|handle| async move {
            if handle.retired.load(Ordering::Relaxed) {
                return;
            }
            // Cancelling would drop an in-flight checkpoint: let it land, then
            // judge the thresholds on the numbers it left behind.
            while handle.checkpointing.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let Some(stats) = lock(&handle.chat2).as_ref().map(|client| client.stats()) else {
                return;
            };
            if checkpoint_due(false, stats.row_count, stats.row_bytes, false)
                && let Some(reset) = self.begin_chat2_checkpoint(&handle)
            {
                self.run_chat2_checkpoint(handle.clone(), "shutdown", reset)
                    .await;
            }
            let owed = handle.turn_end_pending.swap(false, Ordering::AcqRel)
                || lock(&handle.sync_cadence).tail_deferred;
            if owed {
                self.chat2_tail_maintenance(&handle, false, true).await;
            }
        });
        if tokio::time::timeout(SHUTDOWN_SYNC_BUDGET, futures::future::join_all(work))
            .await
            .is_err()
        {
            tracing::warn!("shutdown: deferred chat2 sync did not finish within its budget");
        }
    }

    /// Freeze every open queue before settling live runs during shutdown.
    /// Interrupting a run publishes Idle, which normally wakes the turn-end
    /// queue drainer; without this barrier quitting the host could promote a
    /// queued row in the narrow window before workers are retired.
    pub fn pause_all_queues(&self) {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            handle.queue_paused.store(true, Ordering::Release);
        }
    }

    /// Test-only retirement sentinel: reports true once the doc-host graph
    /// has actually been freed.
    #[doc(hidden)]
    pub fn retirement_probe(&self) -> Box<dyn Fn() -> bool + Send + Sync> {
        let weak = Arc::downgrade(&self.inner);
        Box::new(move || weak.upgrade().is_none())
    }

    /// Wire the repos engine (engine assembly) — worktree materialization for
    /// Run commands carrying a [`zeron_proto::WorktreeSpec`].
    pub fn set_repos(&self, repos: crate::repos::Repos) {
        let _ = self.inner.repos.set(repos);
    }

    pub fn set_project_action_runtime(
        &self,
        project_actions: ProjectActionsStore,
        terminals: Terminals,
    ) {
        let _ = self
            .inner
            .project_action_runtime
            .set((project_actions, terminals));
    }

    /// Wire the uploads store (engine assembly) — `pending://` ref resolution
    /// and the transfer-read jail.
    pub fn set_uploads(&self, uploads: crate::uploads::Uploads) {
        let _ = self.inner.uploads.set(uploads);
    }

    /// Wire the peer-link cache (engine assembly, edge runtimes only) — the
    /// transport for queued attachment transfers to a remote host.
    pub fn set_links(&self, links: Arc<zeron_rpc::LinkCache>) {
        let _ = self.inner.links.set(links);
    }

    /// Re-evaluate every open chat's commands and queue NOW. Called after an
    /// upload commit lands bytes on this device: a Run or queued row deferred
    /// on those bytes (`pending://` refs not yet on disk) becomes executable
    /// the moment its transfer completes — event-driven, not timer luck.
    pub fn kick_drains(&self) {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
    }

    /// Wire the workspace host (engine assembly) — the source of chat-ownership rows.
    pub fn set_workspace(&self, workspace: WorkspaceHost) {
        let chats = workspace.watch_chats();
        if self.inner.workspace.set(workspace).is_ok() {
            self.spawn_cutover_watcher(chats);
            self.spawn_migration_sweep();
        }
    }

    /// Host migration sweep: proactively seed this device's own s2 chats
    /// onto chat2, one per tick. The lazy open()-triggered seed could never
    /// fire in real usage — idle chats are never opened, a headless host's
    /// only opens are nudges (which arrive exactly when a run starts), and
    /// viewing a chat used to pin it. Every safety gate stays (s2 room
    /// joined, 2s frontier quiet, no live writer, no pending commands,
    /// frontier seal at the flip) — only the trigger changes: the host owns
    /// migrating its chats. Aborted seeds retry after a gap; a migrated
    /// fleet makes the tick a cheap no-op scan.
    fn spawn_migration_sweep(&self) {
        const TICK: std::time::Duration = std::time::Duration::from_secs(30);
        const RETRY_GAP_MS: i64 = 10 * 60 * 1000;
        let host = self.clone();
        self.spawn_worker(async move {
            let mut attempted: HashMap<String, i64> = HashMap::new();
            loop {
                tokio::time::sleep(TICK).await;
                let Some(edge) = host.inner.config.edge.clone() else {
                    return; // edge-less engine: nothing to migrate onto
                };
                let Some(ws) = host.workspace() else { continue };
                let chats: Vec<zeron_proto::Chat> = ws.watch_chats().borrow().clone();
                let device = host.inner.config.device_id.clone();
                let now = now_ms();
                let candidate = chats.into_iter().find(|c| {
                    c.device_id == device
                        && c.room_gen.unwrap_or(1) < 2
                        && !attempted
                            .get(&c.id)
                            .is_some_and(|t| now - *t <= RETRY_GAP_MS)
                });
                let Some(chat) = candidate else { continue };
                attempted.insert(chat.id.clone(), now);
                // A cached (warm) handle never re-enters open()'s build path,
                // so arm the quiet-waiter directly; a cold chat opens, which
                // arms it on the way up.
                let cached = lock(&host.inner.handles).get(&chat.id).cloned();
                match cached {
                    Some(handle) => {
                        if handle.room_gen < 2 && !handle.retired.load(Ordering::Relaxed) {
                            tracing::debug!(chat = %chat.id, "migration sweep: arming seed on warm s2 chat");
                            host.spawn_chat2_seed_when_quiet(edge, &chat.id, &handle);
                        }
                    }
                    None => {
                        tracing::debug!(chat = %chat.id, "migration sweep: opening cold s2 chat to seed");
                        if let Err(err) = host.open(&chat.id) {
                            tracing::warn!(chat = %chat.id, error = %err, "migration sweep open failed");
                        }
                    }
                }
            }
        });
    }

    /// Live cutover convergence: when a registry chat row flips to roomGen 2
    /// while this device holds an s2-mode handle, retire it NOW — a viewer
    /// watching the chat at flip time otherwise stays frozen on the dead s2
    /// room until they happen to reopen (the host writes only to chat2 from
    /// the flip on). Dropping the handle ends the watch streams cleanly; the
    /// UI's transcript/standing watches resubscribe and the fresh open takes
    /// the chat2 adopt path. A handle with a LIVE local writer (a running
    /// turn's doc ref) is left alone — the host never flips mid-run, and a
    /// racing writer must never lose its doc out from under it.
    fn spawn_cutover_watcher(&self, mut chats: watch::Receiver<Vec<zeron_proto::Chat>>) {
        let host = self.clone();
        self.spawn_worker(async move {
            loop {
                if chats.changed().await.is_err() {
                    return; // workspace host gone (shutdown)
                }
                let flipped: Vec<String> = chats
                    .borrow_and_update()
                    .iter()
                    .filter(|c| c.room_gen.unwrap_or(1) >= 2)
                    .map(|c| c.id.clone())
                    .collect();
                if flipped.is_empty() {
                    continue;
                }
                let mut dropped: Vec<String> = Vec::new();
                let mut stuck_live: Vec<(String, Arc<ChatDocHandle>)> = Vec::new();
                {
                    let mut handles = lock(&host.inner.handles);
                    for chat_id in flipped {
                        let Some(handle) = handles.get(&chat_id) else {
                            continue;
                        };
                        if handle.room_gen >= 2 {
                            continue; // already chat2-mode
                        }
                        let live_writer = Arc::strong_count(&handle.doc) > 1;
                        if live_writer {
                            // A run is writing into this s2 doc while the
                            // registry already says chat2 — the born-gen2
                            // race (this open beat its own CreateChat mint,
                            // 2026-08-11: transcript never reached any other
                            // device). No thin lineage exists on disk yet,
                            // so retiring here would suppress the doc's only
                            // persistence and abort the quiet-seed. Leave it
                            // live; seed once the run quiesces — the seed
                            // posts the chat2 checkpoint, persists the thin
                            // lineage, and retires the handle itself.
                            stuck_live.push((chat_id, handle.clone()));
                            continue;
                        }
                        handle.retired.store(true, Ordering::Relaxed);
                        handles.remove(&chat_id);
                        tracing::info!(chat = %chat_id,
                            "s2 handle dropped on chat2 cutover; watchers resubscribe onto the new room");
                        dropped.push(chat_id);
                    }
                }
                for (chat_id, handle) in stuck_live {
                    if host.is_host(&chat_id)
                        && let Some(edge) = host.inner.config.edge.clone()
                    {
                        host.spawn_chat2_seed_when_quiet(edge, &chat_id, &handle);
                    }
                }
                // Watchers resubscribe on their own — but a NUDGE-opened
                // handle has none, and its s2 room never carried the queued
                // command anyway (the sender pushed to chat2). On a born-
                // chat2 chat the nudge beats the registry row by design
                // (direct HTTP vs room sync), so the first open lands here
                // and dying silently strands the first message until the
                // next nudge — every new remote session's first send sat
                // ~30s+ until the user re-sent (user report). If we host
                // the chat, reopen NOW: the fresh open dials the chat2
                // room and the change-driven drain executes the command.
                for chat_id in dropped {
                    if !host.is_host(&chat_id) {
                        continue;
                    }
                    match host.open(&chat_id) {
                        Ok(_) => tracing::info!(chat = %chat_id,
                            "reopened as chat2 after cutover drop (host, pending work possible)"),
                        Err(err) => tracing::warn!(chat = %chat_id, error = %err,
                            "chat2 reopen after cutover drop failed"),
                    }
                }
            }
        });
    }

    /// The workspace host, once wired (tests may assemble a DocHost without one).
    pub fn workspace(&self) -> Option<&WorkspaceHost> {
        self.inner.workspace.get()
    }

    pub fn device_id(&self) -> &str {
        &self.inner.config.device_id
    }

    /// Open (or return) the chat's doc handle: load the local snapshot (or init fresh),
    /// start the change-driven task, and request budgeted sync when configured.
    pub fn open(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        let handle = self.open_local(chat_id)?;
        self.activate_sync(&handle);
        Ok(handle)
    }

    /// Explicit viewport navigation; automatic watch retries and MCP reads use
    /// `open` instead. The sequence also orders focuses in the same millisecond.
    pub fn focus_chat(&self, chat_id: &str) -> Result<(), EngineError> {
        let handle = self.open_local(chat_id)?;
        let focus = self.inner.focus_clock.fetch_add(1, Ordering::AcqRel) + 1;
        handle.last_focus.fetch_max(focus, Ordering::AcqRel);
        self.activate_sync(&handle);
        Ok(())
    }

    /// Materialize one authoritative local document without acquiring a network
    /// connection. Durable publication is installed before exposing any writer.
    pub fn open_local(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        // The registry names the sync room generation (docs/chat2-sync.md
        // M2): absent row / absent field = legacy s2. Read it BEFORE the
        // cached-handle check — a cached s2-mode handle for a chat another
        // device has since cut over to chat2 would otherwise serve its frozen
        // fat lineage forever (the host writes only to the chat2 room now;
        // this device's s2 room has gone permanently silent).
        let chat_row = self
            .workspace()
            .and_then(|w| w.chat(chat_id).ok().flatten());
        // A row that EXISTS without `roomGen` is a pre-cutover legacy chat
        // (gen 1). A MISSING row is a chat being born right now: its
        // CreateChat mint (which stamps roomGen 2) is racing this open —
        // the composer attaches the transcript watch before its own mutate
        // lands, and a nudge beats registry sync by design. Defaulting the
        // absent row to 1 minted brand-new s2 rooms post-cutover: the host
        // ran the whole session against a room no other device reads (they
        // follow the row's gen 2 to an empty chat2 room), the run's live doc
        // ref blocked every heal, and the transcript never synced anywhere
        // (2026-08-11).
        let registry_gen = match chat_row.as_ref() {
            Some(row) => row.room_gen.unwrap_or(1),
            None => 2,
        };
        {
            let mut handles = lock(&self.inner.handles);
            if let Some(handle) = handles.get(chat_id) {
                let stale = registry_gen >= 2 && handle.room_gen < 2;
                if (stale || handle.retired.load(Ordering::Relaxed)) && !self.pinned(handle) {
                    // A seed flipped this chat under a cached fat handle
                    // (review B1): drop it so this open converges onto the
                    // thin lineage + chat2 room. Retire only at the drop:
                    // marking a PINNED stale handle retired while it kept
                    // serving suppressed the only persistence a stuck-live
                    // doc had and aborted its quiet-seed (born-gen2 race,
                    // 2026-08-11) — the cutover watcher and the seed itself
                    // own converging pinned handles.
                    handle.retired.store(true, Ordering::Relaxed);
                    handles.remove(chat_id);
                } else {
                    handle.touch();
                    return Ok(handle.clone());
                }
            }
        }
        let opening = lock(&self.inner.opening);
        if let Some(handle) = lock(&self.inner.handles).get(chat_id) {
            handle.touch();
            return Ok(handle.clone());
        }
        // B2/M5 guard: the LOCAL epoch is the second cutover signal. A crash
        // between the thin save and the registry flip (or a not-yet-synced
        // registry) must NOT route an epoch-2 doc back onto s2 — the s2
        // room's fat doc would merge into the unrelated thin lineage and
        // duplicate every message. Local epoch >= 2 forces the chat2 branch
        // and best-effort completes the flip.
        self.inner.document_loads.fetch_add(1, Ordering::Relaxed);
        let stored = self.inner.store.load_chat_state(chat_id)?;
        let persisted_base = stored
            .as_ref()
            .filter(|s| s.epoch >= crate::chat2_host::CHAT2_DOC_EPOCH)
            .map(|s| {
                (
                    s.doc.oplog_vv(),
                    s.snapshot_bytes,
                    s.delta_count,
                    s.delta_bytes,
                    s.needs_compaction,
                )
            });
        let stored = stored.map(|s| (s.doc, s.cursor, s.epoch, s.snapshot_bytes));
        let stored_epoch = stored.as_ref().map(|(_, _, e, _)| *e).unwrap_or(0);
        let room_gen = if stored_epoch >= crate::chat2_host::CHAT2_DOC_EPOCH {
            if registry_gen < 2
                && let Some(ws) = self.workspace()
            {
                let _ = ws.set_chat_room_gen(chat_id, 2);
                tracing::info!(chat = %chat_id,
                    "completed interrupted chat2 flip (local epoch 2, registry said s2)");
            }
            2
        } else {
            registry_gen
        };
        let deferred_adoption = room_gen >= 2
            && stored_epoch < crate::chat2_host::CHAT2_DOC_EPOCH
            && self.inner.config.edge.is_none()
            && stored.is_some();
        let mut snapshot_len = 0usize;
        let mut chat2_cursor = 0u64;
        let mut requeue_commands: Vec<SessionCommandEntry> = Vec::new();
        let doc = if room_gen >= 2 {
            match stored {
                Some((raw, cursor, epoch, base_len))
                    if epoch >= crate::chat2_host::CHAT2_DOC_EPOCH =>
                {
                    snapshot_len = base_len;
                    chat2_cursor = cursor;
                    SessionDoc::from_doc(raw)
                }
                Some((raw, _cursor, epoch, base_len)) if self.inner.config.edge.is_none() => {
                    // Offline/edge-less: adopting would blank a readable
                    // transcript with no way to catch up (review B4). Keep
                    // the old doc read-only-ish; the adopt runs on the next
                    // online open.
                    tracing::info!(chat = %chat_id, old_epoch = epoch,
                        "chat2 adopt deferred (no edge configured)");
                    snapshot_len = base_len;
                    SessionDoc::from_doc(raw)
                }
                Some((raw, _cursor, epoch, _base_len)) => {
                    // M3 discard-and-adopt: this device's doc predates the
                    // chat2 lineage. Keep the old snapshot under a suffixed
                    // id for rollback, carry over OUR OWN unresolved
                    // commands, and start fresh — the chat2 catch-up
                    // (checkpoint + rows) repopulates the transcript. This
                    // is the self-repair path: no user action, ever.
                    tracing::info!(chat = %chat_id, old_epoch = epoch,
                        "chat2 adopt: discarding pre-chat2 local doc (rollback copy kept)");
                    let rollback_id = format!("{chat_id}.pre-chat2");
                    // A re-adopt after a mid-catch-up crash reruns this path
                    // with a near-empty doc under `chat_id` — the FIRST
                    // rollback copy is the real transcript; never overwrite
                    // it (review B5).
                    if matches!(self.inner.store.load_snapshot(&rollback_id), Ok(None))
                        && let Ok(bytes) = raw.export(loro::ExportMode::Snapshot)
                    {
                        let _ = self.inner.store.save_snapshot(&rollback_id, &bytes);
                    }
                    {
                        let old_doc = SessionDoc::from_doc(raw);
                        if let Ok(commands) = old_doc.read_commands() {
                            requeue_commands = commands
                                .into_iter()
                                .filter(|c| {
                                    c.status == SessionCommandStatus::Pending
                                        && c.issued_by == self.inner.config.device_id
                                })
                                .collect();
                        }
                    }
                    SessionDoc::init(chat_id)?
                }
                None => {
                    // Born on chat2 (or a cold reader's first open): stamp
                    // the epoch-2 lineage NOW. Plain snapshot saves preserve
                    // an existing row's epoch but default a NEW row to 0 —
                    // without this stamp, the next open reads "pre-chat2
                    // doc" and the M3 adopt DISCARDS everything written
                    // since (caught by the restart_resume suite: first-turn
                    // transcripts vanished on reopen).
                    let doc = SessionDoc::init(chat_id)?;
                    if let Ok(snapshot) = doc.export_snapshot() {
                        let _ = self.inner.store.save_snapshot_with_cursor(
                            chat_id,
                            &snapshot,
                            0,
                            crate::chat2_host::CHAT2_DOC_EPOCH,
                        );
                    }
                    doc
                }
            }
        } else {
            match stored {
                Some((raw, _, _, base_len)) => {
                    snapshot_len = base_len;
                    SessionDoc::from_doc(raw)
                }
                None => SessionDoc::init(chat_id)?,
            }
        };
        // Recover committed outgoing operations even when the snapshot debounce
        // did not run before a crash. Imported updates do not echo as local writes.
        if room_gen >= 2 {
            for (_, bytes) in self.inner.store.pending_chat_updates(chat_id)? {
                doc.doc()
                    .import(&bytes)
                    .map_err(|e| EngineError::Other(e.to_string()))?;
            }
        }
        let doc = Arc::new(doc);
        let persistence = (room_gen >= 2 && !deferred_adoption).then(|| {
            let persistence = crate::chat_persistence::ChatPersistence::new(
                &doc,
                self.inner.store.clone(),
                chat_id.to_string(),
                chat2_cursor,
            );
            if let Some((vv, bytes, count, delta_bytes, needs_compaction)) = persisted_base {
                persistence.restore_base(vv, bytes, count, delta_bytes, needs_compaction);
            }
            persistence
        });
        let changed_persistence = persistence.clone();

        let (changed_tx, changed_rx) = watch::channel(0u64);
        let (commands_tx, _) = watch::channel(0u64);
        let publish_bits = Arc::new(zeron_doc::AtomicChangeMask::default());
        let dispatch_bits = Arc::new(zeron_doc::AtomicChangeMask::default());
        let commands_cache = Arc::new(zeron_doc::CommandsCache::new());
        let (messages_tx, _) = watch::channel(TranscriptSnapshot::default());
        let transcript_history = Arc::new(Mutex::new(
            crate::transcript_history::TranscriptHistory::default(),
        ));
        let history = transcript_history.clone();
        let transcript_mirror = Arc::new(zeron_doc::TranscriptMirror::new());
        let mirror = transcript_mirror.clone();
        let watched = messages_tx.clone();
        let weak_doc = Arc::downgrade(&doc);
        let (callback_publish, callback_dispatch) = (publish_bits.clone(), dispatch_bits.clone());
        let (callback_cache, callback_commands) = (commands_cache.clone(), commands_tx.clone());
        let writer_wake = Arc::new(tokio::sync::Notify::new());
        let turn_end_pending = Arc::new(AtomicBool::new(false));
        let writers = Arc::new(AtomicUsize::new(0));
        let status_writers = writers.clone();
        let status_wake = writer_wake.clone();
        let sub = doc.doc().subscribe_root(Arc::new(move |diff| {
            // This callback runs before the change worker can publish. The
            // import origin belongs to the event, so concurrent local commits
            // cannot inherit a remote replay's presentation classification.
            if watched.receiver_count() > 0 {
                if let Some(doc) = weak_doc.upgrade() {
                    lock(&history).observe(doc.doc(), &diff);
                }
                mirror.observe(&diff);
            } else {
                *lock(&history) = Default::default();
                mirror.reset();
            }
            // Bits and cache first, wakes last: a consumer that wakes must
            // find its bit set and the ledger cache already stale.
            let mask = zeron_doc::ChangeMask::of_event(&diff);
            // A remote writer has no local DocWriter to drop, and its session
            // status is not visible here. A finished segment is the earliest
            // signal that a remote turn may have ended: wake maintenance now
            // instead of after the debounce. This is a hint only. A remote
            // turn is split into several assistant segments, so it must not
            // be taken for a turn end; the pass decides from the transcript
            // (a streaming entry in the tail), and the idle floor bounds PUTs.
            if status_writers.load(Ordering::Acquire) == 0 && diff.events.iter().any(|change| {
                matches!(change.path.first(), Some((_, loro::Index::Key(key))) if key.as_str() == "messages")
                    && matches!(&change.diff, loro::event::Diff::Map(delta)
                        if !delta.updated.contains_key("role") && delta.updated.get("status").is_some_and(|value|
                            value.as_ref().is_some_and(|v| v.get_deep_value().to_json_value().as_str().is_some_and(|status| status != "streaming"))))
            }) {
                status_wake.notify_one();
            }
            callback_publish.set(mask);
            callback_dispatch.set(mask);
            if mask.contains(zeron_doc::ChangeMask::COMMANDS) {
                callback_cache.invalidate();
                callback_commands.send_modify(|v| *v = v.wrapping_add(1));
            }
            if let Some(persistence) = &changed_persistence {
                persistence.dirty(false);
            }
            changed_tx.send_modify(|v| *v = v.wrapping_add(1));
        }));
        // The mirror starts dirty and empty; watch_messages materializes it
        // once on attach instead of maintaining an unwatched transcript.
        let initial_queue = doc.read_queue().unwrap_or_default();
        // A queue already present when a handle is materialized came from a
        // persisted snapshot (or a synced checkpoint), not from a prompt the
        // user submitted to this live engine. Keep that recovered work frozen
        // until an explicit prompt / Send now / Steer action thaws it. Rows
        // appended after the handle exists retain the normal automatic drain.
        let recovered_queue_pending = !initial_queue.is_empty();
        let (queue_tx, _) = watch::channel(initial_queue);

        let handle = Arc::new(ChatDocHandle {
            chat_id: chat_id.to_string(),
            device_id: self.inner.config.device_id.clone(),
            doc: doc.clone(),
            messages_tx,
            transcript_import: Mutex::default(),
            transcript_history,
            transcript_mirror,
            publish_bits,
            dispatch_bits,
            commands_tx,
            commands_cache,
            queue_tx,
            drain_lock: tokio::sync::Mutex::new(()),
            command_drain_lock: tokio::sync::Mutex::new(()),
            steered_rows: Mutex::new(Vec::new()),
            held_sends: Mutex::new(HashMap::new()),
            stopped_by: Mutex::new(None),
            queue_paused: AtomicBool::new(recovered_queue_pending),
            mirror_dirty: AtomicBool::new(true),
            last_access: AtomicI64::new(now_ms()),
            last_focus: AtomicU64::new(0),
            sync_focus_served: AtomicU64::new(0),
            snapshot_bytes: AtomicUsize::new(snapshot_len),
            room_gen,
            retired: AtomicBool::new(false),
            purged: AtomicBool::new(false),
            checkpointing: Arc::new(AtomicBool::new(false)),
            sync_cadence: Mutex::new(SyncCadence::default()),
            writer_wake,
            turn_end_pending,
            chat2: Mutex::new(None),
            sync_started: AtomicBool::new(false),
            sync_requested: AtomicBool::new(false),
            sync_background: AtomicBool::new(false),
            sync_last_started: AtomicI64::new(0),
            sync_wait_since: AtomicI64::new(0),
            sync_service_until: AtomicI64::new(0),
            sync_wake_version: AtomicI64::new(0),
            sync_wake_ticket: AtomicU64::new(0),
            sync_cancel: Mutex::new(CancellationToken::new()),
            writers,
            persistence,
            chat2_pending_local: Mutex::new(Vec::new()),
            publication_failed: AtomicBool::new(false),
            chat2_local_sub: Mutex::new(None),
            publication_barrier: Mutex::new(None),
            own_ops_order: Mutex::new(None),
            #[cfg(test)]
            before_outbox_submit: Mutex::new(None),
            _sub: sub,
        });
        // Snapshot recovery may restore several independently edited rows.
        // Each row needs its own checked expiry wake; otherwise a non-head
        // edit could remain displayed as live indefinitely.
        self.arm_existing_queue_edit_expiries(&handle);

        // Edge room join — offline-tolerant AND supervised. `ChatClient` only
        // self-reconnects AFTER a first successful join; a one-shot attempt
        // here (the pre-LRU design) left the doc silently local-only until
        // app restart whenever the dial hit a transient gap — a post-wake
        // network, `Auth::token()` momentarily `None` around a refresh, an
        // edge deploy. The LRU made that dice-roll constant (every reopen),
        // and a watched doc is pinned against eviction, so nothing ever
        // retried: the exact "transcript frozen until restart" report.
        // Retry on the workspace host's capped, jittered backoff; a system
        // wake redials immediately; eviction/purge ends the loop via `weak`.
        if self.inner.config.edge.is_some() {
            if room_gen >= 2 {
                // Subscription BEFORE the dial (review B3): every local
                // commit lands in the client when connected, else in the
                // pending buffer the join drains — nothing composed during
                // (or before) the dial is lost to the room.
                // A one-time full replay heals history stranded by older clients.
                // Its durable marker is independent of the download cursor.
                if !self.inner.store.chat_outbox_initialized(chat_id)? {
                    let updates = crate::chat2_host::publication_updates(doc.doc())
                        .map_err(EngineError::Other)?;
                    self.inner.store.initialize_chat_outbox(chat_id, &updates)?;
                }
                let weak_push = Arc::downgrade(&handle);
                let outbox = self.inner.outbox.clone();
                let publication_chat = chat_id.to_string();
                // Concurrent commits can run their hooks out of order; this
                // device's ops still go out in commit order (OwnOpsOrder).
                let own_order =
                    Arc::new(Mutex::new(crate::chat2_host::OwnOpsOrder::new(doc.doc())));
                *lock(&handle.own_ops_order) = Some(own_order.clone());
                let snapshot_order = own_order.clone();
                let snapshot_outbox = outbox.clone();
                let snapshot_weak = weak_push.clone();
                let snapshot_chat = publication_chat.clone();
                let snapshot_store = self.inner.store.clone();
                let barrier: Arc<zeron_sync::SnapshotBarrier> = Arc::new(move |chat, write| {
                    if chat != snapshot_chat {
                        return Ok(());
                    }
                    let Some(handle) = snapshot_weak.upgrade() else {
                        return Ok(());
                    };
                    if handle.retired.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    // Reconstruct only a lagging delta's dependencies, and
                    // do the SQLite read outside own_order so hooks never wait
                    // behind that read. Recheck the frontier under the lock:
                    // the hook may have caught up during reconstruction.
                    let delta_source = if let zeron_sync::SnapshotWrite::Delta(bytes) = write {
                        let needed = lock(&snapshot_order)
                            .needs_prefix(bytes)
                            .map_err(std::io::Error::other)?;
                        if needed {
                            Some(
                                snapshot_store
                                    .load_chat_state(chat)?
                                    .ok_or_else(|| {
                                        std::io::Error::other(
                                            "snapshot delta has no persisted base",
                                        )
                                    })?
                                    .doc,
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    {
                        let mut order = lock(&snapshot_order);
                        if handle.purged.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        let updates = match write {
                            zeron_sync::SnapshotWrite::Full(bytes) => order.snapshot_prefix(bytes),
                            zeron_sync::SnapshotWrite::Delta(bytes) => order.delta_prefix(bytes, || {
                                delta_source.ok_or_else(|| "snapshot delta publication frontier changed during restore".to_string())
                            }),
                        }
                        .map_err(std::io::Error::other)?;
                        for update in updates {
                            snapshot_outbox.submit(
                                chat,
                                snapshot_weak.clone(),
                                uuid::Uuid::new_v4().to_string(),
                                update,
                            );
                        }
                    }
                    snapshot_outbox.flush_writes();
                    Ok(())
                });
                self.inner.store.add_snapshot_barrier(&barrier);
                *lock(&handle.publication_barrier) = Some(barrier);
                let sub = doc
                    .doc()
                    .subscribe_local_update(Box::new(move |bytes: &Vec<u8>| {
                        if let Some(handle) = weak_push.upgrade() {
                            #[cfg(test)]
                            {
                                let hook = lock(&handle.before_outbox_submit).take();
                                if let Some(hook) = hook {
                                    hook();
                                }
                            }
                            // No I/O here: the commit may be a stream task. The
                            // outbox writer inserts the row, then hands the
                            // update to `ChatDocHandle::outbox_written`, which
                            // pushes it to the client under the client lock
                            // (the join's store+drain takes the same lock).
                            // Ordering and submitting share one lock so two
                            // hooks cannot swap their updates on the queue.
                            let mut order = lock(&own_order);
                            if handle.purged.load(Ordering::Acquire) {
                                return true;
                            }
                            for bytes in order.order(bytes) {
                                let batch_id = uuid::Uuid::new_v4().to_string();
                                outbox.submit(
                                    &publication_chat,
                                    weak_push.clone(),
                                    batch_id,
                                    bytes,
                                );
                            }
                        }
                        true
                    }));
                *lock(&handle.chat2_local_sub) = Some(sub);
                // Re-queue survives the adopt: our own pending commands
                // become fresh entries in the new lineage (the
                // processed_commands ledger still guards double execution).
                // Committed AFTER the local-update subscription above — a
                // commit before it never enters the pending buffer or the
                // client, so the requeued command would sit in the local doc
                // and never reach the room (the host would never see it).
                for command in &requeue_commands {
                    let _ = doc.queue_command(command);
                }
            }
        }
        // Publish only after the durable subscription and bootstrap are installed.
        lock(&self.inner.handles).insert(chat_id.to_string(), handle.clone());
        drop(opening);
        self.spawn_worker(chat_task(self.clone(), Arc::downgrade(&handle), changed_rx));
        self.evict_over_budget();
        Ok(handle)
    }

    /// Connection lifetime is independent of the document and its outbox.
    /// Concurrent callers activate at most one supervisor for this handle.
    /// Called on the device control link. Acceptance never cold-opens a chat.
    pub fn enqueue_wakeup(&self, chat_id: &str) -> Result<(), EngineError> {
        self.inner
            .store
            .schedule_sync_job(chat_id, if chat_id == "*" { "reconcile" } else { "wake" })?;
        Ok(())
    }

    pub fn activate_sync(&self, handle: &Arc<ChatDocHandle>) {
        handle.sync_background.store(false, Ordering::Release);
        if !handle.sync_started.load(Ordering::Acquire) {
            let _ = handle.sync_wait_since.compare_exchange(
                0,
                now_ms(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        if self.inner.config.edge.is_some() && !self.inner.edge_disconnected.load(Ordering::Acquire)
        {
            handle.sync_requested.store(true, Ordering::Release);
        }
    }

    /// Resolve an actual operation before reserving a client slot. Generation
    /// comes from the opened handle, including the local epoch override; an
    /// absent registry row is still the legitimate born-chat2 race.
    fn prepare_sync_admission(
        &self,
        handle: &ChatDocHandle,
        wake_version: Option<i64>,
    ) -> Result<Option<SyncAdmission>, EngineError> {
        if handle.room_gen >= 2 {
            return Ok(Some(SyncAdmission::Join));
        }
        let row = match self.workspace() {
            Some(workspace) => workspace.chat(&handle.chat_id)?,
            None => None,
        };
        match row {
            None => Ok(Some(SyncAdmission::Seed)),
            Some(row) if row.device_id == self.inner.config.device_id => {
                Ok(Some(SyncAdmission::Seed))
            }
            Some(row) if row.room_gen.unwrap_or(1) < 2 => {
                // Only retire the receipt examined before this decision. A
                // newer wake must survive, and outgoing updates are untouched.
                if let Some(version) = wake_version {
                    self.inner
                        .store
                        .complete_sync_job(&handle.chat_id, "wake", version)?;
                }
                handle.sync_requested.store(false, Ordering::Release);
                Ok(None)
            }
            // Registry cutover overtook a pinned legacy handle. Let the
            // cutover watcher replace it; do not lose its pending work.
            Some(_) => Ok(None),
        }
    }

    /// Cold candidates stay metadata-only until a connection can be admitted.
    /// Local epoch wins over an older registry row, exactly as in `open_local`.
    fn prepare_stored_sync_admission(
        &self,
        chat_id: &str,
        wake_version: Option<i64>,
    ) -> Result<Option<SyncAdmission>, EngineError> {
        if self.inner.store.snapshot_epoch(chat_id)? >= crate::chat2_host::CHAT2_DOC_EPOCH {
            return Ok(Some(SyncAdmission::Join));
        }
        let row = match self.workspace() {
            Some(ws) => ws.chat(chat_id)?,
            None => None,
        };
        match row {
            None => Ok(Some(SyncAdmission::Join)),
            Some(row) if row.room_gen.unwrap_or(1) >= 2 => Ok(Some(SyncAdmission::Join)),
            Some(row) if row.device_id == self.inner.config.device_id => {
                Ok(Some(SyncAdmission::Seed))
            }
            Some(_) => {
                if let Some(version) = wake_version {
                    self.inner
                        .store
                        .complete_sync_job(chat_id, "wake", version)?;
                }
                Ok(None)
            }
        }
    }

    /// One dispatcher per host; waiting chats are flags on existing handles,
    /// never a spawned task per connection request. Oldest service wins within
    /// each class, with one background admission every four selections.
    fn spawn_sync_scheduler(&self) {
        let weak = Arc::downgrade(&self.inner);
        self.spawn_worker(async move {
            use futures::{StreamExt, stream::FuturesUnordered};
            let mut turn = 0u64;
            let mut disk_cursor = String::new();
            // (winner, fairness turn). Reserve a released slot until teardown
            // ends; a disk-page change must not steal it from the winner.
            let mut handoff: Option<(String, bool)> = None;
            let mut contention_since = None;
            // Closing clients keep their admission slots until teardown ends,
            // but no individual close can suspend unrelated admissions/work.
            // Track teardown independently of this cancellable dispatcher:
            // host shutdown must join these actors before its final snapshot.
            let mut stopping = FuturesUnordered::<tokio::task::JoinHandle<String>>::new();
            let mut stopping_ids = HashSet::new();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                    Some(result) = stopping.next(), if !stopping.is_empty() => {
                        match result {
                            Ok(id) => { stopping_ids.remove(&id); }
                            Err(error) => tracing::error!(%error, "chat sync teardown failed"),
                        }
                        continue;
                    }
                }
                let Some(inner) = weak.upgrade() else { return };
                let host = Self { inner };
                host.evict_over_budget();
                if host.inner.edge_disconnected.load(Ordering::Acquire) {
                    continue;
                }
                let Some(edge) = host.inner.config.edge.clone() else {
                    continue;
                };
                // Overflow reconciliation is metadata-only and resumable. Wait
                // for registry truth before deciding the hosted set is complete.
                if let Some(ws) = host.workspace() {
                    if ws.sync_status().is_some_and(|s| s.synced) {
                        if let Ok(Some((version, cursor))) =
                            host.inner.store.reconciliation_progress()
                        {
                            let mut ids: Vec<_> = ws
                                .watch_chats()
                                .borrow()
                                .iter()
                                .filter(|c| {
                                    c.device_id == host.inner.config.device_id && c.id > cursor
                                })
                                .map(|c| c.id.clone())
                                .collect();
                            ids.sort();
                            ids.truncate(32);
                            let _ = host.inner.store.reconcile_page(version, &ids);
                        }
                    }
                }
                let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
                for handle in &handles {
                    if !handle.publication_failed.load(Ordering::Acquire) {
                        continue;
                    }
                    let _client = lock(&handle.chat2);
                    let mut failed = lock(&handle.chat2_pending_local);
                    failed.retain(|(id, bytes)| {
                        host.inner
                            .store
                            .enqueue_chat_update(&handle.chat_id, id, bytes)
                            .is_err()
                    });
                    if failed.is_empty() {
                        handle.publication_failed.store(false, Ordering::Release);
                    }
                }

                let durable = host
                    .inner
                    .store
                    .pending_sync_docs(&disk_cursor, 4)
                    .unwrap_or_default();
                if durable.is_empty() {
                    disk_cursor.clear();
                }
                // Discover actual contenders before deciding whether to retire
                // a client. A connected chat's outbox is not slot contention.
                let disk_healthy = !handles.iter()
                    .any(|h| h.publication_failed.load(Ordering::Acquire));
                // A None handle is a cold, metadata-only candidate. Discovery
                // advances at capacity without loading and evicting its history.
                let mut candidates: Vec<_> = handles.iter()
                    .filter(|h| h.sync_requested.load(Ordering::Acquire)
                        && !h.sync_started.load(Ordering::Acquire)
                        && !stopping_ids.contains(&h.chat_id))
                    .map(|h| (h.chat_id.clone(), Some(h.clone()))).collect();
                if disk_healthy {
                    for id in durable {
                        disk_cursor = id.clone();
                        if candidates.iter().any(|(chat, _)| chat == &id)
                            || stopping_ids.contains(&id)
                            || handles.iter().any(|h| h.chat_id == id && h.sync_started.load(Ordering::Acquire))
                        {
                            continue;
                        }
                        candidates.push((id, None));
                    }
                }
                if let Some((id, _)) = &handoff {
                    if !candidates.iter().any(|(chat, _)| chat == id) {
                        candidates.push((id.clone(), handles.iter().find(|h| &h.chat_id == id).cloned()));
                    }
                }
                let mut waiting = Vec::new();
                for (id, handle) in candidates {
                    let wake_version = match host.inner.store.sync_job_version(&id, "wake") {
                        Ok(version) => version,
                        Err(error) => {
                            tracing::warn!(chat = %id, %error, "sync admission deferred: wake read failed");
                            continue;
                        }
                    };
                    let admission = match &handle {
                        Some(h) => host.prepare_sync_admission(h, wake_version),
                        None => host.prepare_stored_sync_admission(&id, wake_version),
                    };
                    match admission {
                        Ok(Some(_)) => waiting.push((id, handle, wake_version)),
                        Ok(None) => {},
                        Err(error) => tracing::warn!(chat = %id, %error, "sync admission eligibility deferred"),
                    }
                }
                // Ownership/cutover changes can invalidate a reserved winner.
                if handoff.as_ref().is_some_and(|(id, _)| !waiting.iter().any(|(chat, _, _)| chat == id)) {
                    handoff = None;
                }
                for handle in &handles {
                    if !handle.sync_started.load(Ordering::Acquire)
                        || stopping_ids.contains(&handle.chat_id)
                    {
                        continue;
                    }
                    // A focus during teardown remains unserved and can
                    // reclaim a slot. Only resident, non-closing clients
                    // acknowledge focus here.
                    handle.sync_focus_served.fetch_max(handle.last_focus.load(Ordering::Acquire), Ordering::AcqRel);
                    let mut captured = handle.sync_wake_version.load(Ordering::Acquire);
                    let pending_wake = host
                        .inner
                        .store
                        .sync_job_version(&handle.chat_id, "wake")
                        .ok()
                        .flatten();
                    if let Some(version) = pending_wake.filter(|v| *v != captured) {
                        // Keep the healthy transport. Retirement of this wake
                        // requires a read started after its version was seen.
                        if let Some(client) = lock(&handle.chat2).as_ref() {
                            let ticket = client.request_catch_up();
                            handle.sync_wake_ticket.store(ticket, Ordering::Release);
                            handle.sync_wake_version.store(version, Ordering::Release);
                            captured = version;
                        }
                    }
                    let wake_read_complete = captured != 0
                        && lock(&handle.chat2).as_ref().is_some_and(|c|
                            c.catch_up_completed(handle.sync_wake_ticket.load(Ordering::Acquire)));
                    if wake_read_complete && host.wakeup_is_durable(handle)
                    {
                        let _ =
                            host.inner
                                .store
                                .complete_sync_job(&handle.chat_id, "wake", captured);
                    }
                    let caught_up = lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up());
                    // A quiet document can still be receiving its checkpoint
                    // or commands. Incomplete catch-up only yields via rotate's
                    // explicit service deadline, never the short reuse grace.
                    let idle = caught_up
                        && !handle.sync_protected()
                        && (now_ms() - handle.last_access.load(Ordering::Relaxed) >= SYNC_IDLE_MS
                            || (handle.sync_background.load(Ordering::Acquire) && caught_up))
                        && !host.outbox_pending(&handle.chat_id).unwrap_or(true);
                    if idle {
                        handle.sync_requested.store(false, Ordering::Release);
                        stopping_ids.insert(handle.chat_id.clone());
                        stopping.push(host.inner.tasks.spawn(host.stop_sync_owned(handle.clone())));
                    }
                }
                // Closing clients still own capacity until teardown is joined.
                let running = handles.iter().filter(|h|
                    h.sync_started.load(Ordering::Acquire) || stopping_ids.contains(&h.chat_id)
                ).count();
                let available = ACTIVE_SYNC_CAP.saturating_sub(running);
                let now = now_ms();
                let contended = available == 0 && !waiting.is_empty();
                let budget = zeron_sync::budget::shared().stats();
                let global_contended = !budget.resource_paused && budget.sockets >= budget.socket_limit && budget.socket_waiting > 0;
                // A teardown gap or the end of a paged disk scan is not the
                // end of contention. Resetting there starves cold jobs under
                // continuous navigation or a full set of active writers.
                if available > 0 && waiting.is_empty() {
                    contention_since = None;
                }
                let since = if !waiting.is_empty() { *contention_since.get_or_insert(now) } else { now };
                for (_, h, _) in &waiting {
                    if let Some(h) = h {
                        let _ = h.sync_wait_since.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire);
                    }
                }
                let newest_focus = waiting.iter().filter_map(|(id, h, _)| {
                    let h = h.as_ref()?;
                    let focus = h.last_focus.load(Ordering::Acquire);
                    (focus > h.sync_focus_served.load(Ordering::Acquire)).then(|| (focus, id.clone()))
                }).max();
                let oldest_waiter = waiting.iter().filter(|(_, h, _)| {
                    now - h.as_ref().map_or(since, |h| h.sync_wait_since.load(Ordering::Acquire)) >= SYNC_QUANTUM_MS
                }).min_by_key(|(id, h, _)| (
                    h.as_ref().map_or(0, |h| h.sync_last_started.load(Ordering::Acquire)), id.clone(),
                ));
                // Three focus turns, then one overdue service turn. A served
                // focus cannot immediately take its slot back after rotation.
                let fair = oldest_waiter.is_some() && (newest_focus.is_none() || (turn + 1) % 4 == 0);
                let winner = if fair {
                    oldest_waiter.map(|(id, _, _)| id.clone())
                } else {
                    newest_focus.as_ref().map(|(_, id)| id.clone())
                };
                let latest_focus = handles.iter().map(|h| h.last_focus.load(Ordering::Acquire)).max().unwrap_or(0);
                if disk_healthy && (contended || global_contended) && handoff.is_none() && stopping_ids.is_empty() {
                    let mut victims = Vec::new();
                    for h in &handles {
                        if !h.sync_started.load(Ordering::Acquire)
                            || h.sync_service_until.load(Ordering::Acquire) > now {
                            continue;
                        }
                        let protected = h.sync_protected();
                        let focus = h.last_focus.load(Ordering::Acquire);
                        let caught_up = lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up());
                        if !contended && !caught_up { continue; }
                        let pending = host.outbox_pending(&h.chat_id).unwrap_or(true);
                        let age = now - h.sync_last_started.load(Ordering::Acquire);
                        let service_complete = (caught_up && age >= SYNC_QUANTUM_MS) || age >= SYNC_CATCH_UP_QUANTUM_MS;
                        let eligible = if !protected {
                            (caught_up && !pending) || service_complete
                        } else if fair || !contended {
                            // Keep the most recently focused thread live while
                            // the other slots provide bounded service turns.
                            service_complete && (focus == 0 || focus != latest_focus || running == 1)
                        } else {
                            newest_focus.as_ref().is_some_and(|(newest, _)| *newest > focus)
                        };
                        if eligible {
                            victims.push((protected, h.writers.load(Ordering::Acquire) > 0, focus,
                                h.sync_last_started.load(Ordering::Acquire), h.clone(), caught_up, pending));
                        }
                    }
                    // Reclaim idle work first, then viewed-only docs, then
                    // writers in least-recent-user-focus order. One handoff
                    // retires one transport, never a whole batch of agents.
                    victims.sort_by_key(|(protected, writer, focus, started, h, _, _)|
                        (*protected, *writer, *focus, *started, h.chat_id.clone()));
                    if let Some((protected, _, _, _, handle, caught_up, pending)) = victims.into_iter().next() {
                        let unfinished = protected || !caught_up || pending
                            || host.inner.store.sync_job_version(&handle.chat_id, "wake").map_or(true, |v| v.is_some());
                        handle.sync_requested.store(unfinished, Ordering::Release);
                        handle.sync_wait_since.store(now, Ordering::Release);
                        handoff = winner.map(|id| (id, fair));
                        stopping_ids.insert(handle.chat_id.clone());
                        stopping.push(host.inner.tasks.spawn(host.stop_sync_owned(handle)));
                    }
                }
                let admission_limit = if disk_healthy {
                    available.min(SYNC_ADMISSION_BATCH)
                } else {
                    0
                };
                let mut admitted = 0;
                while admitted < admission_limit && !waiting.is_empty() {
                    turn += 1;
                    waiting.sort_by_key(|(id, h, _)| {
                        let protected = h.as_ref().is_some_and(|h| h.sync_protected());
                        let focus = h.as_ref().map_or(0, |h| {
                            let focus = h.last_focus.load(Ordering::Acquire);
                            if focus > h.sync_focus_served.load(Ordering::Acquire) { focus } else { 0 }
                        });
                        (
                            handoff.as_ref().map(|(chat, _)| chat) != Some(id),
                            if turn % 4 == 0 { protected } else { !protected },
                            std::cmp::Reverse(if turn % 4 == 0 { 0 } else { focus }),
                            h.as_ref().map_or(0, |h| h.sync_last_started.load(Ordering::Acquire)),
                            id.clone(),
                        )
                    });
                    let (id, cached, wake_version) = waiting.remove(0);
                    let handle = match cached {
                        Some(h) => h,
                        None => match host.open_local(&id) {
                            Ok(h) => {
                                h.sync_background.store(true, Ordering::Release);
                                h.sync_requested.store(true, Ordering::Release);
                                h
                            }
                            Err(error) => {
                                tracing::warn!(chat = %id, %error, "durable sync work deferred");
                                continue;
                            }
                        },
                    };
                    // Opening may observe a newer registry/lineage. Recheck
                    // ownership before admission, retaining the captured wake.
                    let action = match host.prepare_sync_admission(&handle, wake_version) {
                        Ok(Some(action)) => action,
                        Ok(None) => continue,
                        Err(error) => {
                            tracing::warn!(chat = %id, %error, "sync admission recheck failed");
                            continue;
                        }
                    };
                    if handle.sync_started.swap(true, Ordering::AcqRel) {
                        continue;
                    }
                    admitted += 1;
                    let fair_turn = handoff.as_ref().is_some_and(|(chat, fair)| chat == &id && *fair);
                    if handoff.as_ref().is_some_and(|(chat, _)| chat == &id) {
                        handoff = None;
                    }
                    handle.sync_wait_since.store(0, Ordering::Release);
                    handle.sync_service_until.store(if fair_turn { now_ms() + SYNC_QUANTUM_MS } else { 0 }, Ordering::Release);
                    handle.sync_focus_served.fetch_max(handle.last_focus.load(Ordering::Acquire), Ordering::AcqRel);
                    handle.sync_last_started.store(now_ms(), Ordering::Release);
                    handle.sync_wake_version.store(wake_version.unwrap_or(0), Ordering::Release);
                    handle.sync_wake_ticket.store(0, Ordering::Release);
                    match action {
                        SyncAdmission::Join => {
                            let cursor = handle.persistence.as_ref().map_or(0, |p| p.cursor());
                            host.spawn_chat2_join(edge.clone(), &handle, cursor);
                        }
                        SyncAdmission::Seed => {
                            host.spawn_chat2_seed_when_quiet(edge.clone(), &handle.chat_id, &handle);
                        }
                    }
                }
                drop(waiting);
                host.evict_over_budget();
            }
        });
    }

    fn stop_sync_owned(
        &self,
        handle: Arc<ChatDocHandle>,
    ) -> impl std::future::Future<Output = String> + Send + 'static {
        let host = self.clone();
        async move {
            host.stop_sync(&handle).await;
            handle.chat_id.clone()
        }
    }

    async fn stop_sync(&self, handle: &ChatDocHandle) {
        lock(&handle.sync_cancel).cancel();
        let client = lock(&handle.chat2).take();
        if let Some(client) = client {
            client.shutdown().await;
        }
        handle.sync_started.store(false, Ordering::Release);
    }

    /// Catch-up alone is not a durable handoff: a crash before the snapshot
    /// debounce/command drain would otherwise lose the only discovery receipt.
    fn wakeup_is_durable(&self, handle: &ChatDocHandle) -> bool {
        if handle.publication_failed.load(Ordering::Acquire) {
            return false;
        }
        if self.is_host(&handle.chat_id) {
            let Ok(commands) = handle.commands() else {
                return false;
            };
            if commands.iter().any(|c| {
                c.status == SessionCommandStatus::Pending
                    && !self.inner.store.is_processed(&c.id).unwrap_or(false)
            }) {
                return false;
            }
        }
        self.save_snapshot(handle);
        handle.persistence.as_ref().is_some_and(|p| p.is_clean())
    }

    fn spawn_sync_worker(
        &self,
        cancel: CancellationToken,
        fut: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        self.spawn_worker(async move {
            tokio::select! { _ = cancel.cancelled() => {}, _ = fut => {} }
        });
    }

    /// Install a supervised chat2 client. Construction is local-first; only
    /// caught_up() proves the checkpoint and row replay actually completed.
    fn spawn_chat2_join(&self, edge: EdgeConfig, handle: &Arc<ChatDocHandle>, cursor: u64) {
        let cancel = CancellationToken::new();
        *lock(&handle.sync_cancel) = cancel.clone();
        let priority = if handle.sync_background.load(Ordering::Acquire) {
            zeron_sync::budget::Priority::Background
        } else {
            zeron_sync::budget::Priority::Interactive
        };
        let chat = handle.chat_id.clone();
        let doc = handle.doc.clone();
        let store = self.inner.store.clone();
        let http = self.inner.http.clone();
        let device = self.inner.config.device_id.clone();
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        let mut token_changes = edge.token_changes();
        self.spawn_sync_worker(cancel.clone(), async move {
            let sink = Arc::new(crate::chat2_host::EngineChatSink::new(&doc, store, chat.clone())
                .with_handle(weak.clone()));
            // The sink holds only a Weak doc ref (a strong one made every
            // chat2 handle read as perma-pinned — LRU eviction dead); this
            // task's own strong ref dies when the join resolves.
            drop(doc);
            let fetcher = Arc::new(crate::chat2_host::EdgeCheckpointFetcher::new(
                http,
                edge.clone(),
                chat.clone(),
            ).with_priority(priority));
            let remote_host = host.remote_host_for(&chat);
            let url = edge.room_url_with_host(format!("/chat2/{chat}/ws"), remote_host.clone());
            let mut wake = zeron_sync::wake::subscribe();
            // Sibling-dial successes end a backoff wait immediately, exactly
            // like the joined clients' own reconnect loops (chat_client.rs).
            // Without this, a NEW chat whose first joins hit a network blip
            // waited out the full accumulated backoff (→30s) while every
            // established room redialed instantly on recovery — fresh sends
            // to new sessions stalled while other chats hummed (2026-08-19
            // user report, reproduced on two networks).
            let mut online = zeron_sync::wake::subscribe_online();
            let mut backoff = crate::workspace_host::JOIN_RETRY_BASE;
            loop {
                if weak.upgrade().is_none() {
                    return; // evicted or purged while dialing
                }
                // Dual transport: WS dial + a plain-HTTPS pull/push seam
                // (rows GET / POST on the same bearer auth as the checkpoint
                // fetch) — bootstraps in ~1 RTT and keeps syncing at backoff
                // cadence on networks that never pass the WS upgrade. With
                // the transport, connect resolves immediately (local-first).
                let transport = Arc::new(crate::chat2_host::EdgeChatTransport::new(
                    host.inner.http.clone(),
                    edge.clone(),
                    chat.clone(),
                    device.clone(),
                ).with_host_device(remote_host.clone()).with_priority(priority));
                let dial = tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    zeron_sync::ChatClient::connect_via_transport(
                        url.clone(),
                        sink.clone(),
                        fetcher.clone(),
                        &device,
                        cursor,
                        transport,
                    ),
                )
                .await;
                match dial {
                    Ok(Ok(client)) => {
                        if matches!(edge.bearer().await, Err(zeron_rpc::TokenError::SignedOut)) {
                            return;
                        }
                        let Some(handle) = weak.upgrade() else {
                            return; // evicted mid-dial: drop leaves the room
                        };
                        let mut events = client.events();
                        let mut lifecycle_events = client.events();
                        {
                            // Store + drain under ONE client-lock critical
                            // section: the subscription pushes to the buffer
                            // while holding this same lock, so every commit
                            // is either drained here or enqueued directly
                            // after — never dropped between (verify pass).
                            let mut client_slot = lock(&handle.chat2);
                            if host.inner.edge_disconnected.load(Ordering::Acquire) || cancel.is_cancelled() { return; }
                            // Include commits made after the sink's initial
                            // load but before installation. The same lock is
                            // held by the local-update subscription.
                            for (id, bytes) in host.inner.store.pending_chat_updates(&chat).unwrap_or_default() {
                                // Read from the outbox, so already journaled.
                                client.enqueue_batch_journaled(id, bytes, true);
                            }
                            let pending: Vec<(String, Vec<u8>)> =
                                lock(&handle.chat2_pending_local).clone();
                            for (batch_id, update) in pending {
                                client.enqueue_batch(batch_id, update);
                            }
                            *client_slot = Some(client);
                        }
                        tracing::info!(chat = %chat, "chat2 client admitted (catch-up pending)");
                        // A missed event, failed POST or actor restart must not
                        // forget rejected operations. Any author can checkpoint
                        // its own durable history, including a non-host desktop.
                        let checkpoint_host = host.clone();
                        let checkpoint_weak = weak.clone();
                        host.spawn_sync_worker(cancel.clone(), async move {
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                let Some(handle) = checkpoint_weak.upgrade() else { return };
                                if checkpoint_host.inner.edge_disconnected.load(Ordering::Acquire) { return; }
                                let known = lock(&handle.chat2).as_ref().is_some_and(|c|c.stats().server_known);
                                if known && checkpoint_host.inner.store.rejected_chat_updates(&handle.chat_id).is_ok_and(|v| !v.is_empty()) {
                                    checkpoint_host.spawn_chat2_checkpoint(&handle, "durable-rejection");
                                }
                            }
                        });

                        // Bootstrap heal: a room with NO checkpoint can't
                        // cover its rows' causal deps for cold readers — a
                        // pre-0.1.34 first contact whose init batch never
                        // went up (every reader parks every row on missing
                        // deps, transcript invisible forever), or a host
                        // whose WS pushes strand. The checkpoint is the
                        // universal patch: full doc over plain HTTP. Checked
                        // once, shortly after join (an idle chat never hits
                        // the quiesce tick, so the tick can't be the only
                        // trigger).
                        if host.is_host(&chat) {
                            let host = host.clone();
                            let weak = weak.clone();
                            host.clone().spawn_sync_worker(cancel.clone(), async move {
                                // With the pull-first transport the client
                                // constructs before any state answer — wait
                                // until the server's view is KNOWN (bounded)
                                // or the all-zero placeholder stats would
                                // misread as "no checkpoint" and upload a
                                // spurious full-doc heal on a thin link.
                                for _ in 0..40u32 {
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    let Some(handle) = weak.upgrade() else { return };
                                    let known = lock(&handle.chat2)
                                        .as_ref()
                                        .is_some_and(|c| c.stats().server_known);
                                    if known {
                                        break;
                                    }
                                }
                                let Some(handle) = weak.upgrade() else { return };
                                let no_checkpoint = lock(&handle.chat2)
                                    .as_ref()
                                    .is_some_and(|c| c.stats().server_known && c.stats().checkpoint_size == 0);
                                let has_content = handle
                                    .doc
                                    .read_entries()
                                    .map(|e| !e.is_empty())
                                    .unwrap_or(false);
                                if no_checkpoint && has_content {
                                    tracing::info!(chat = %handle.chat_id,
                                        "chat2 room has rows but no checkpoint; posting bootstrap checkpoint");
                                    host.spawn_chat2_checkpoint(&handle, "bootstrap");
                                }
                            });
                        }
                        // Host recovery duties (C3): a wiped room needs a
                        // seed checkpoint or fresh readers see only
                        // post-reset rows; rejected pushes reach peers only
                        // through a checkpoint. Watcher dies with the handle.
                        if host.is_host(&chat) {
                            let host = host.clone();
                            let weak = weak.clone();
                            let chat = chat.clone();
                            host.clone().spawn_sync_worker(cancel.clone(), async move {
                                use zeron_sync::chat_client::ChatEvent;
                                loop {
                                    match events.recv().await {
                                        Ok(ChatEvent::ServerReset) => {
                                            let Some(handle) = weak.upgrade() else { return };
                                            tracing::warn!(chat = %chat, "chat2 room reset; posting seed checkpoint");
                                            host.spawn_chat2_checkpoint(&handle, "server-reset");
                                        }
                                        Ok(ChatEvent::PushRejected) => {
                                            let Some(handle) = weak.upgrade() else { return };
                                            tracing::warn!(chat = %chat, "chat2 push rejected; compensating via checkpoint");
                                            host.spawn_chat2_checkpoint(&handle, "push-rejected");
                                        }
                                        Ok(_) => {}
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                                    }
                                }
                            });
                        }
                        drop(handle);
                        if token_changes.is_none() {
                            return;
                        }
                        loop {
                            tokio::select! {
                                event = lifecycle_events.recv() => match event {
                                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                                },
                                _ = crate::workspace_host::token_changed(&mut token_changes) => {
                                    if matches!(edge.bearer().await, Err(zeron_rpc::TokenError::SignedOut)) {
                                        if let Some(handle) = weak.upgrade() {
                                            lock(&handle.chat2).take();
                                            // Keep journaling local cleanup after credentials disappear.
                                        }
                                        tracing::info!(chat = %chat,
                                            "chat2 credentials removed; leaving room");
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(chat = %chat, error = %err,
                            backoff_ms = backoff.as_millis() as u64,
                            "chat2 join failed; retrying");
                    }
                    Err(_) => {
                        tracing::warn!(chat = %chat,
                            backoff_ms = backoff.as_millis() as u64,
                            "chat2 join timed out; retrying");
                    }
                }
                // Drain stale online events first: only successes DURING this
                // wait count, or our own last dial would cut every wait to
                // zero (same discipline as chat_client's wait_backoff).
                while online.try_recv().is_ok() {}
                tokio::select! {
                    _ = tokio::time::sleep(backoff + crate::workspace_host::join_retry_jitter()) => {
                        backoff = (backoff * 2).min(crate::workspace_host::JOIN_RETRY_CAP);
                    }
                    _ = wake.recv() => {
                        backoff = crate::workspace_host::JOIN_RETRY_BASE;
                    }
                    _ = online.recv() => {
                        backoff = crate::workspace_host::JOIN_RETRY_BASE;
                    }
                    _ = crate::workspace_host::token_changed(&mut token_changes) => {
                        backoff = crate::workspace_host::JOIN_RETRY_BASE;
                    }
                }
            }
        });
    }

    /// Host-side chat2 seed (docs/chat2-sync.md M1/M2): rebuild thin, POST
    /// the seed checkpoint, persist the thin lineage locally, THEN flip the
    /// registry — each step idempotent, a crash before the flip leaves the
    /// chat on s2 and the next open retries (M5).
    /// Defer a chat2 seed until the chat is verifiably quiet: the s2 room has
    /// joined (the rebuild must include every row the room holds — seeding
    /// from the pre-backfill local doc forked other devices' rows into the
    /// retired lineage) and the doc frontier has stopped moving (a
    /// nudge-burst in flight — an incoming queued command — must land and
    /// execute first). Holds only a weak handle: eviction ends the wait.
    fn spawn_chat2_seed_when_quiet(
        &self,
        edge: EdgeConfig,
        chat_id: &str,
        handle: &Arc<ChatDocHandle>,
    ) {
        const TICK: std::time::Duration = std::time::Duration::from_millis(500);
        const QUIET_TICKS: u32 = 4; // 2s of frontier silence
        if !lock(&self.inner.seed_waiting).insert(chat_id.to_string()) {
            return; // a quiet-waiter is already armed for this chat
        }
        let host = self.clone();
        let chat = chat_id.to_string();
        let weak = Arc::downgrade(handle);
        self.spawn_worker(async move {
            async {
                // The old join-wait gate (seed only after the s2 room had
                // backfilled) is gone with the s2 client: no device writes
                // an s2 room anymore, so the host's local doc IS the
                // authority for a straggler gen-1 chat. Only the quiet gate
                // remains (no seed under a moving frontier).
                let mut quiet = 0u32;
                let mut last_vv: Option<Vec<u8>> = None;
                loop {
                    tokio::time::sleep(TICK).await;
                    let Some(handle) = weak.upgrade() else { return };
                    if handle.retired.load(Ordering::Relaxed) {
                        return;
                    }
                    let vv = handle.doc.doc().oplog_vv().encode();
                    if last_vv.as_ref() == Some(&vv) {
                        quiet += 1;
                        if quiet >= QUIET_TICKS {
                            let doc = handle.doc.clone();
                            drop(handle);
                            host.spawn_chat2_seed(edge, &chat, doc);
                            return;
                        }
                    } else {
                        quiet = 0;
                        last_vv = Some(vv);
                    }
                }
            }
            .await;
            // Cleared on EVERY exit path so an aborted wait (evicted handle,
            // room never joined, seed handed off) can re-arm later.
            lock(&host.inner.seed_waiting).remove(&chat);
        });
    }

    fn spawn_chat2_seed(&self, edge: EdgeConfig, chat_id: &str, doc: Arc<SessionDoc>) {
        {
            let mut seeding = lock(&self.inner.seeding);
            if !seeding.insert(chat_id.to_string()) {
                return; // seed already in flight
            }
        }
        let host = self.clone();
        let chat = chat_id.to_string();
        self.spawn_worker(async move {
            let outcome = host.seed_chat2(&edge, &chat, doc).await;
            lock(&host.inner.seeding).remove(&chat);
            match outcome {
                Ok(()) => {
                    tracing::info!(chat = %chat, "chat2 seeded; registry flipped to roomGen 2");
                }
                Err(err) => {
                    tracing::warn!(chat = %chat, error = %err,
                        "chat2 seed failed; chat stays on s2 (retries next open)");
                }
            }
        });
    }

    async fn seed_chat2(
        &self,
        edge: &EdgeConfig,
        chat_id: &str,
        doc: Arc<SessionDoc>,
    ) -> Result<(), String> {
        use base64::Engine as _;
        let vv_at_rebuild = doc.doc().oplog_vv().encode();
        let rebuilt = zeron_doc::rebuild::rebuild_thin_doc(&doc).map_err(|e| e.to_string())?;
        // The seed's own doc ref must be gone before the pinned re-check
        // below: `pinned` reads `Arc::strong_count(&handle.doc) > 1`, and
        // holding this clone made that true unconditionally — every seed
        // aborted "became active mid-seed" and the cutover never flipped a
        // single chat (v0.1.32 DOA).
        drop(doc);
        if !rebuilt.sidecar.is_empty() {
            // Sidecar PARKED (docs/chat2-sync.md A2): full outputs are not
            // uploaded; they survive in the rollback snapshot + run journal.
            tracing::info!(chat = %chat_id, payloads = rebuilt.sidecar.len(),
                "chat2 seed: sidecar parked; outputs stay local");
        }
        let snapshot = rebuilt.doc.export_snapshot().map_err(|e| e.to_string())?;
        let frontier = rebuilt.doc.doc().oplog_vv().encode();
        let bearer = edge.bearer().await.map_err(|e| e.to_string())?;
        let url = format!(
            "{}/chat2/{}/checkpoint?seqCovered=0",
            edge.url.trim_end_matches('/'),
            chat_id
        );
        let _permit = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Background)
            .await
            .map_err(|e| e.to_string())?;
        let res = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&bearer)
            .header(
                "x-chat2-frontier",
                base64::engine::general_purpose::STANDARD.encode(&frontier),
            )
            .body(snapshot.clone())
            .send()
            .await
            .map_err(|e| format!("seed checkpoint POST: {}", describe_http_error(e)))?;
        if !res.status().is_success() {
            return Err(format!("seed checkpoint HTTP {}", res.status()));
        }
        // PINNED RE-CHECK before anything irreversible (review B1): if a
        // run/watcher attached during the rebuild+POST, abort — everything
        // they write would fork away from the thin lineage. The orphan
        // checkpoint in the chat2 room is harmless (wholly replaced by the
        // next seed's seqCovered=0 POST); the chat stays on s2 and the next
        // quiet open retries.
        {
            let handles = lock(&self.inner.handles);
            if let Some(handle) = handles.get(chat_id) {
                if self.seed_blocked(handle) {
                    return Err("chat became active mid-seed; aborted before flip".into());
                }
                // Frontier seal (review B1's TOCTOU): ANY doc movement since
                // the rebuild — a synced remote row, a local write that has
                // already released its doc ref — means the thin lineage is
                // missing it. Borrowed read (no Arc clone: that would trip
                // the pinned check we just passed).
                if handle.doc.doc().oplog_vv().encode() != vv_at_rebuild {
                    return Err("doc advanced mid-seed; aborted before flip".into());
                }
            }
        }
        // Rollback copy of the fat lineage BEFORE the thin one replaces it —
        // never overwriting an existing copy (review B5).
        let rollback_id = format!("{chat_id}.pre-chat2");
        if matches!(self.inner.store.load_snapshot(&rollback_id), Ok(None))
            && let Ok(Some(old)) = self.inner.store.load_chat_state(chat_id)
            && let Ok(bytes) = old.doc.export(loro::ExportMode::Snapshot)
        {
            let _ = self.inner.store.save_snapshot(&rollback_id, &bytes);
        }
        self.inner
            .store
            .save_snapshot_with_cursor(chat_id, &snapshot, 0, crate::chat2_host::CHAT2_DOC_EPOCH)
            .map_err(|e| e.to_string())?;
        // Registry flip LAST — the cutover signal every device dials by.
        let flipped = self
            .workspace()
            .ok_or("no workspace host")?
            .set_chat_room_gen(chat_id, 2)
            .map_err(|e| e.to_string())?;
        if !flipped {
            return Err("chat row vanished during seed".into());
        }
        // The live handle still holds the FAT doc on the s2 room. Retire it:
        // it must never persist again (it would clobber the thin lineage);
        // drop it entirely when unpinned so the next open converges onto
        // chat2. A pinned (watched/running) handle keeps working against s2
        // until it closes — the flip is registry-side, readers already moved.
        let dropped = {
            let mut handles = lock(&self.inner.handles);
            if let Some(handle) = handles.get(chat_id) {
                handle.retired.store(true, Ordering::Relaxed);
                // Drop unless a live WRITER holds the doc. Watchers do not
                // keep the fat handle alive: their streams end with it and
                // they resubscribe onto the chat2 adopt path (the same
                // contract the cutover watcher enforces for remote flips).
                if Arc::strong_count(&handle.doc) == 1 {
                    handles.remove(chat_id);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        tracing::info!(chat = %chat_id, handle_dropped = dropped, "chat2 seed complete");
        Ok(())
    }

    /// Boot-time transcript salvage (born-gen2 aftermath, 2026-08-11): a chat
    /// we host whose chat2 doc has NO message entries while its run journal
    /// has events lost its transcript to a stuck s2 handle (the retired flag
    /// suppressed every snapshot save; the post-restart reopen born a blank
    /// lineage). The full fat doc still exists in the legacy s2 room — the
    /// stuck engine pushed every op into it until it died — and sometimes in
    /// a `.pre-chat2` rollback on disk. Re-append its entries (thinned) into
    /// the LIVE chat2 lineage as ordinary incremental updates: no lineage
    /// replacement, no checkpoint surgery, every device converges through
    /// the normal room flow. Idempotent: a doc with any message entry is
    /// never touched, and the salvage only runs on the hosting device.
    ///
    /// An unchanged, inspected non-empty snapshot can be skipped on later
    /// boots. The run journal is never deleted or rewritten.
    pub fn spawn_transcript_salvage(&self, journals_dir: std::path::PathBuf) {
        let host = self.clone();
        self.spawn_worker(async move {
            // Let boot settle (registry load, room joins) before sweeping.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            host.salvage_hosted_chats(&journals_dir).await;
            let mut after = String::new();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let jobs = host
                    .inner
                    .store
                    .pending_sync_jobs("recovery", &after, 1)
                    .unwrap_or_default();
                let Some(chat) = jobs.into_iter().next() else {
                    after.clear();
                    continue;
                };
                after = chat.clone();
                let version = host
                    .inner
                    .store
                    .sync_job_version(&chat, "recovery")
                    .ok()
                    .flatten();
                if host.salvage_chat_transcript(&chat).await.is_ok() {
                    if let Some(version) = version {
                        let _ = host
                            .inner
                            .store
                            .complete_sync_job(&chat, "recovery", version);
                    }
                }
            }
        });
    }

    /// One pass of the boot sweep: hosted roomGen-2 chats with a non-empty
    /// journal. The journal file is only statted, never removed.
    async fn salvage_hosted_chats(&self, journals_dir: &std::path::Path) {
        let Some(ws) = self.workspace() else { return };
        let chats: Vec<zeron_proto::Chat> = ws.watch_chats().borrow().clone();
        for chat in chats {
            if chat.device_id != self.inner.config.device_id {
                continue; // only the host owns its chats' history
            }
            if chat.room_gen.unwrap_or(1) < 2 {
                continue; // still s2-mode: transcript lives in its room
            }
            let journal = journals_dir.join(format!("{}.jsonl", chat.id));
            let journaled = std::fs::metadata(&journal)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            if !journaled {
                continue; // never ran here — an empty doc is just new
            }
            if let Err(err) = self.salvage_chat_transcript(&chat.id).await {
                let _ = self.inner.store.schedule_sync_job(&chat.id, "recovery");
                tracing::warn!(chat = %chat.id, error = %err, "transcript salvage failed");
            }
        }
    }

    fn note_salvage_imports(&self, n: u64) {
        #[cfg(test)]
        self.inner.salvage_imports.fetch_add(n, Ordering::Relaxed);
        #[cfg(not(test))]
        let _ = (self, n);
    }

    async fn salvage_chat_transcript(&self, chat_id: &str) -> Result<(), String> {
        zeron_counters::add(zeron_counters::C::SalvageChats, 1);
        let _timer = zeron_counters::timer(zeron_counters::C::SalvageNanos);
        // Inspection must not register a handle or start sync. A healthy
        // history used to open one room per journal during every boot.
        // The import and the emptiness check run off the async worker.
        let store = self.inner.store.clone();
        let chat = chat_id.to_string();
        let edge_configured = self.inner.config.edge.is_some();
        let scan =
            tokio::task::spawn_blocking(move || scan_salvage(&store, &chat, edge_configured))
                .await
                .map_err(|err| err.to_string())??;
        self.note_salvage_imports(scan.imports);
        match scan.kind {
            SalvageScanKind::Skip => Ok(()),
            SalvageScanKind::Healthy => Ok(()),
            SalvageScanKind::Absent => Ok(()),
            SalvageScanKind::Recover { bytes, entries } => {
                self.recover_transcript(chat_id, bytes, entries).await
            }
        }
    }

    async fn recover_transcript(
        &self,
        chat_id: &str,
        bytes: Vec<u8>,
        entries: Vec<SessionMessageEntry>,
    ) -> Result<(), String> {
        let handle = self.open(chat_id).map_err(|e| e.to_string())?;
        if self.inner.config.edge.is_some() {
            tokio::time::timeout(std::time::Duration::from_secs(90), async {
                loop {
                    if lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up()) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await
            .map_err(|_| "recovery deferred: remote history has not converged".to_string())?;
        }
        if !messages_list_empty(handle.doc().doc())? {
            // Transcript present — nothing lost. A later scan can mark it
            // once its non-empty snapshot is durable.
            return Ok(());
        }
        // Fat source: the M3 adopt's rollback copy on disk. (The other
        // source — the legacy s2 room — went away with the s2 client; any
        // transcript that existed only there was salvaged by earlier
        // releases or is reachable in the room's storage server-side.)
        // Thin before appending (docs/chat2-sync.md A2): full outputs are
        // parked, exactly like a seed — they survive in the rollback copy
        // saved below and the run journal.
        let rollback_id = format!("{chat_id}.pre-chat2");
        if matches!(self.inner.store.has_snapshot(&rollback_id), Ok(false)) {
            let _ = self.inner.store.save_snapshot(&rollback_id, &bytes);
        }
        // Re-check emptiness at the last instant: a run that started during
        // the room fetch must not get history interleaved under it.
        let _drain = handle.drain_lock.lock().await;
        let _import = lock(&handle.transcript_import);
        if Arc::strong_count(&handle.doc) > 1 || !messages_list_empty(handle.doc().doc())? {
            return Err("doc gained entries mid-salvage; aborted".into());
        }
        for entry in &entries {
            handle
                .doc()
                .push_message(entry)
                .map_err(|e| e.to_string())?;
        }
        drop(_import);
        drop(_drain);
        // Flush, then inspect the actual persisted snapshot before marking it.
        let host = self.clone();
        let persisted = Arc::clone(&handle);
        tokio::task::spawn_blocking(move || host.save_snapshot(&persisted))
            .await
            .map_err(|err| err.to_string())?;
        let store = self.inner.store.clone();
        let chat = chat_id.to_string();
        let edge_configured = self.inner.config.edge.is_some();
        tokio::task::spawn_blocking(move || scan_salvage(&store, &chat, edge_configured))
            .await
            .map_err(|err| err.to_string())??;
        tracing::info!(chat = %chat_id, entries = entries.len(),
            "transcript salvaged into chat2 lineage");
        Ok(())
    }

    /// chat2 host duties on the doc-quiesce tick (docs/chat2-sync.md C3):
    /// - threshold checkpoint: when the room's row log passes 512KB or 200
    ///   rows, post a full checkpoint so cold readers load one compact blob
    ///   instead of replaying the log (the alert-shaped growth bound);
    /// - tail sidecar: publish the last-64 transcript JSON for thin/instant
    ///   readers (the iOS fallback path).
    ///
    /// Returns when the pass wants to run again without a wake: the trailing
    /// edge of a throttled tail, or a retry after a failed PUT or checkpoint.
    /// `None` is the idle answer: nothing owed, no timer, zero wakeups.
    async fn chat2_maintenance(
        &self,
        handle: &Arc<ChatDocHandle>,
        turn_end: bool,
    ) -> Option<tokio::time::Instant> {
        if handle.retired.load(Ordering::Relaxed) {
            return None;
        }
        let stats = match &*lock(&handle.chat2) {
            Some(client) => client.stats(),
            None => return None,
        };
        let streaming = cadence_streaming(
            handle.writers.load(Ordering::Acquire),
            self.session_active(handle),
        );
        let now = tokio::time::Instant::now();
        let (stale, checkpoint_retry_at) = {
            let cadence = lock(&handle.sync_cadence);
            (
                streaming && checkpoint_stale(cadence.stream_since_checkpoint, now),
                cadence.checkpoint_retry_at,
            )
        };
        let mut next = None;
        if checkpoint_due(streaming, stats.row_count, stats.row_bytes, stale) {
            match checkpoint_retry_at.filter(|at| now < *at) {
                Some(at) => next = Some(at),
                None => {
                    if !self.spawn_chat2_checkpoint(handle, "threshold")
                        && self.inner.edge_disconnected.load(Ordering::Acquire)
                    {
                        // Nothing was attempted, so nothing will wake us when
                        // the link returns: come back on the retry cadence.
                        next = Some(now + CHECKPOINT_RETRY_MIN);
                    }
                    // Otherwise a checkpoint is in flight (its completion
                    // wakes this task to re-evaluate) or there is no edge.
                }
            }
        }
        let tail_streaming = if streaming {
            true
        } else {
            let doc = handle.doc.clone();
            tokio::task::spawn_blocking(move || doc.has_streaming_assistant())
                .await
                .unwrap_or(false)
        };
        let tail_next = self
            .chat2_tail_maintenance(handle, tail_streaming && !turn_end, turn_end)
            .await;
        earliest(next, tail_next)
    }

    /// One tail sidecar pass. `streaming` throttles to [`TAIL_INTERVAL`];
    /// `turn_end` (a local writer dropped or the session left `Working`) sends
    /// at once. Whatever it cannot send now it reports back as a deadline:
    /// the trailing edge of a throttled tail, or the next retry of a failed
    /// PUT. Without that, a tail changed during the last throttle window of a
    /// turn, or a turn that ended during an outage, would stay stale until
    /// some later unrelated edit woke the chat.
    async fn chat2_tail_maintenance(
        &self,
        handle: &Arc<ChatDocHandle>,
        streaming: bool,
        turn_end: bool,
    ) -> Option<tokio::time::Instant> {
        let edge = self.inner.config.edge.clone()?;
        let now = tokio::time::Instant::now();
        if !turn_end {
            let cadence = lock(&handle.sync_cadence);
            let floor = if streaming {
                TAIL_INTERVAL
            } else {
                TAIL_IDLE_FLOOR
            };
            let until = [
                cadence.tail_attempt.map(|last| last + floor),
                cadence.tail_retry_at,
            ]
            .into_iter()
            .flatten()
            .max();
            if let Some(until) = until.filter(|until| now < *until) {
                drop(cadence);
                lock(&handle.sync_cadence).tail_deferred = true;
                // Gated. Whether anything is owed is settled when the
                // deadline passes: an unchanged version ends it there.
                return Some(until);
            }
        }
        let doc = handle.doc.clone();
        let (last_hash, last_version) = {
            let cadence = lock(&handle.sync_cadence);
            (cadence.tail_hash, cadence.tail_version.clone())
        };
        let encoded = tokio::task::spawn_blocking(move || {
            let version = (doc.doc().get_pending_txn_len() == 0).then(|| doc.doc().state_vv());
            if version.is_some() && version == last_version {
                return Some(None);
            }
            let mut tail =
                zeron_doc::materialize_tail_fast(&doc, 0, zeron_doc::TAIL_MESSAGE_COUNT).ok()?;
            // updatedAt is transport metadata, not a transcript change.
            let canonical = serde_json::to_vec(&tail).ok()?;
            let hash: [u8; 32] = Sha256::digest(&canonical).into();
            if Some(hash) == last_hash {
                return Some(Some((hash, None, version)));
            }
            tail.updated_at = now_ms();
            Some(Some((hash, Some(serde_json::to_vec(&tail).ok()?), version)))
        })
        .await
        .ok()
        .flatten();
        let fail = || {
            let mut cadence = lock(&handle.sync_cadence);
            cadence.tail_failures = cadence.tail_failures.saturating_add(1);
            let at = tokio::time::Instant::now()
                + backoff(cadence.tail_failures, TAIL_RETRY_MIN, TAIL_RETRY_MAX);
            cadence.tail_retry_at = Some(at);
            cadence.tail_deferred = true;
            Some(at)
        };
        let (hash, body, version) = match encoded {
            // Materialization failed: retry later rather than never.
            None => return fail(),
            // The version is the one already pushed: nothing is owed.
            Some(None) => {
                lock(&handle.sync_cadence).tail_deferred = false;
                return None;
            }
            Some(Some(sent)) => sent,
        };
        let Some(body) = body else {
            // Same content as the last PUT (a version moved without moving
            // the tail): remember the version so the next pass is free.
            let mut cadence = lock(&handle.sync_cadence);
            cadence.tail_version = version;
            cadence.tail_deferred = false;
            return None;
        };
        let Ok(bearer) = edge.bearer().await else {
            return fail();
        };
        let url = format!(
            "{}/chat2/{}/tail",
            edge.url.trim_end_matches('/'),
            handle.chat_id
        );
        let Ok(_permit) = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Background)
            .await
        else {
            return fail();
        };
        lock(&handle.sync_cadence).tail_attempt = Some(tokio::time::Instant::now());
        let sent = self
            .inner
            .http
            .put(&url)
            .bearer_auth(&bearer)
            .timeout(Duration::from_secs(15))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success());
        if !sent {
            return fail();
        }
        let mut cadence = lock(&handle.sync_cadence);
        cadence.tail_hash = Some(hash);
        cadence.tail_version = version;
        cadence.tail_failures = 0;
        cadence.tail_retry_at = None;
        cadence.tail_deferred = false;
        None
    }

    /// POST a full checkpoint for a chat2 room (one in flight per handle).
    /// Callers: the quiesce-tick threshold above, and the client recovery
    /// events (`ServerReset` — a wiped room needs a seed checkpoint or every
    /// fresh reader sees only post-reset rows; `PushRejected` — the rejected
    /// ops reach peers only through a checkpoint).
    ///
    /// Returns whether a checkpoint was started. Either way of finishing
    /// wakes the chat's maintenance task, so a pass that found one in flight
    /// re-evaluates the thresholds when it lands; a failure records a backoff
    /// that the threshold path honours.
    fn spawn_chat2_checkpoint(&self, handle: &Arc<ChatDocHandle>, reason: &'static str) -> bool {
        let Some(reset) = self.begin_chat2_checkpoint(handle) else {
            return false;
        };
        let host = self.clone();
        let handle = handle.clone();
        self.spawn_worker(async move { host.run_chat2_checkpoint(handle, reason, reset).await });
        true
    }

    /// Claim the one in-flight checkpoint slot, or `None` when the link is
    /// down, the chat has no room client or edge, or one is already running.
    fn begin_chat2_checkpoint(&self, handle: &Arc<ChatDocHandle>) -> Option<ResetFlag> {
        if self.inner.edge_disconnected.load(Ordering::Acquire) {
            return None;
        }
        self.inner.config.edge.as_ref()?;
        lock(&handle.chat2).as_ref()?;
        handle
            .checkpointing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(ResetFlag(handle.checkpointing.clone()))
    }

    async fn run_chat2_checkpoint(
        &self,
        handle: Arc<ChatDocHandle>,
        reason: &'static str,
        reset: ResetFlag,
    ) {
        let succeeded = self.post_chat2_checkpoint(&handle, reason).await;
        // Release the slot before waking: the woken pass must not find it busy.
        drop(reset);
        {
            let mut cadence = lock(&handle.sync_cadence);
            if succeeded {
                cadence.checkpoint_failures = 0;
                cadence.checkpoint_retry_at = None;
            } else {
                cadence.checkpoint_failures = cadence.checkpoint_failures.saturating_add(1);
                cadence.checkpoint_retry_at = Some(
                    tokio::time::Instant::now()
                        + backoff(
                            cadence.checkpoint_failures,
                            CHECKPOINT_RETRY_MIN,
                            CHECKPOINT_RETRY_MAX,
                        ),
                );
            }
        }
        handle.writer_wake.notify_one();
    }

    async fn post_chat2_checkpoint(
        &self,
        handle: &Arc<ChatDocHandle>,
        reason: &'static str,
    ) -> bool {
        use base64::Engine as _;
        let Some(edge) = self.inner.config.edge.clone() else {
            return false;
        };
        let stats = match &*lock(&handle.chat2) {
            Some(client) => client.stats(),
            None => return false,
        };
        let chat_id = handle.chat_id.clone();
        let publication_store = self.inner.store.clone();
        let snapshot_doc = handle.doc.clone();
        let seq_covered = stats.cursor;
        let http = self.inner.http.clone();
        let weak_note = Arc::downgrade(handle);
        let store = publication_store.clone();
        let chat = chat_id.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let rejected = store.rejected_chat_updates(&chat).unwrap_or_default();
            let snapshot = snapshot_doc.export_snapshot().ok()?;
            let frontier = loro::LoroDoc::decode_import_blob_meta(&snapshot, true)
                .ok()?
                .partial_end_vv
                .encode();
            let vv = loro::VersionVector::decode(&frontier).ok()?;
            let covered_rejections: Vec<String> = rejected
                .into_iter()
                .filter_map(|(id, bytes)| {
                    loro::LoroDoc::decode_import_blob_meta(&bytes, true)
                        .ok()
                        .filter(|m| vv.includes_vv(&m.partial_end_vv))
                        .map(|_| id)
                })
                .collect();
            Some((snapshot, frontier, covered_rejections))
        })
        .await;
        let Ok(Some((snapshot, frontier, covered_rejections))) = prepared else {
            return false;
        };

        let Ok(bearer) = edge.bearer().await else {
            return false;
        };
        let url = format!(
            "{}/chat2/{}/checkpoint?seqCovered={}",
            edge.url.trim_end_matches('/'),
            chat_id,
            seq_covered
        );
        let Ok(_permit) = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Background)
            .await
        else {
            return false;
        };
        let size = snapshot.len() as u64;
        match http
            .post(&url)
            .timeout(std::time::Duration::from_secs(300))
            .bearer_auth(&bearer)
            .header(
                "x-chat2-frontier",
                base64::engine::general_purpose::STANDARD.encode(&frontier),
            )
            .body(snapshot)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                tracing::info!(chat = %chat_id, seq_covered, reason, "chat2 checkpoint posted");
                for batch_id in &covered_rejections {
                    if let Err(err) = publication_store.acknowledge_chat_update(&chat_id, batch_id)
                    {
                        tracing::warn!(%err, "chat2: checkpoint obligation retirement failed; will retry");
                    }
                }

                if let Some(handle) = weak_note.upgrade() {
                    if handle.writers.load(Ordering::Acquire) > 0 {
                        lock(&handle.sync_cadence).stream_since_checkpoint =
                            Some(tokio::time::Instant::now());
                    }
                    if let Some(client) = &*lock(&handle.chat2) {
                        client.note_checkpoint(seq_covered, size);
                    }
                }
                true
            }
            Ok(res) => {
                tracing::warn!(chat = %chat_id, status = res.status().as_u16(),
                    "chat2 checkpoint rejected");
                false
            }
            Err(err) => {
                let err = describe_http_error(err);
                tracing::warn!(chat = %chat_id, error = %err, "chat2 checkpoint POST failed");
                false
            }
        }
    }

    /// LRU eviction: while the warm set exceeds [`WARM_DOC_CAP`] or the
    /// resident estimate exceeds `DOC_LRU_BYTE_BUDGET`, close the
    /// least-recently-touched unpinned docs. Pinned (never evicted):
    /// - watched docs (`messages_tx` has receivers — a UI transcript);
    /// - docs with a live writer (`Arc<SessionDoc>` held outside the handle —
    ///   a run streaming into it);
    /// - host-side docs with pending commands (the executor owes them work).
    ///
    /// Eviction flushes a final snapshot, so reopen loses nothing; missed
    /// remote updates re-arrive through the room join's VV backfill.
    fn evict_over_budget(&self) {
        // Do not let a cold reopen race the retiring handle's final flush.
        let _opening = lock(&self.inner.opening);
        let mut by_age: Vec<(i64, String)> = {
            let handles = lock(&self.inner.handles);
            handles
                .values()
                .map(|h| (h.last_access.load(Ordering::Relaxed), h.chat_id.clone()))
                .collect()
        };
        by_age.sort_unstable();
        for (_, chat_id) in by_age {
            let (count, estimate) = {
                let handles = lock(&self.inner.handles);
                (
                    handles.len(),
                    handles
                        .values()
                        .map(|h| h.resident_estimate())
                        .sum::<usize>(),
                )
            };
            if count <= WARM_DOC_CAP && estimate <= zeron_doc::DOC_LRU_BYTE_BUDGET {
                return;
            }
            let evicted = {
                let mut handles = lock(&self.inner.handles);
                match handles.get(&chat_id) {
                    Some(handle) if !self.pinned(handle) && Arc::strong_count(handle) == 1 => {
                        handles.remove(&chat_id)
                    }
                    _ => None,
                }
            };
            if let Some(handle) = evicted {
                // Final flush outside the map lock; ≤1s of changes could be
                // pending in the snapshot debounce.
                self.save_snapshot(&handle);
                tracing::debug!(chat = %handle.chat_id, "doc evicted (LRU)");
            }
        }
    }

    /// Seed-specific activity gate: a live WRITER (a run's doc ref) or
    /// pending host commands block a seed — watchers do NOT. Pure readers
    /// cannot lose writes, and the flip drops the handle so their streams
    /// end and resubscribe onto the chat2 adopt path. `pinned()` below keeps
    /// counting watchers for EVICTION, where a watched doc must stay
    /// resident. (Watcher-pinned seeds made "open a chat to look at it"
    /// self-defeating: the act of viewing blocked its own migration.)
    fn seed_blocked(&self, handle: &Arc<ChatDocHandle>) -> bool {
        if Arc::strong_count(&handle.doc) > 1 {
            return true;
        }
        if self.is_host(&handle.chat_id) {
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            match handle.commands() {
                Ok(commands) => commands
                    .iter()
                    .any(|c| c.status == SessionCommandStatus::Pending && !is_processed(&c.id)),
                // Unreadable ledger: never flip blind.
                Err(_) => true,
            }
        } else {
            false
        }
    }

    fn pinned(&self, handle: &Arc<ChatDocHandle>) -> bool {
        if self.inner.outbox.has_in_flight(&handle.chat_id) {
            return true;
        }
        if handle.sync_started.load(Ordering::Acquire) {
            return true;
        }
        // Durable batches may outlive this handle. Only failed disk writes
        // require retaining the in-memory copy until persistence recovers.
        if handle.publication_failed.load(Ordering::Acquire) {
            return true;
        }
        if handle.messages_tx.receiver_count() > 0 || handle.queue_tx.receiver_count() > 0 {
            return true;
        }
        // The handle itself holds one doc ref; more means a live writer.
        if Arc::strong_count(&handle.doc) > 1 {
            return true;
        }
        if self.is_host(&handle.chat_id) {
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            match handle.commands() {
                Ok(commands) => commands
                    .iter()
                    .any(|c| c.status == SessionCommandStatus::Pending && !is_processed(&c.id)),
                // Unreadable ledger: keep the doc, never evict blind.
                Err(_) => true,
            }
        } else {
            false
        }
    }

    /// Probe every open chat's room (window-focus liveness sweep). Each
    /// room ignores the hint unless it has been broadcast-quiet ≥30s.
    pub fn probe_open_chats(&self) {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            // chat2 rooms verify liveness on user signals — a
            // deaf-but-ponging DO otherwise freezes a watched transcript
            // for the whole background probe quiet window.
            if let Some(chat2) = lock(&handle.chat2).as_ref() {
                chat2.probe();
            }
        }
    }

    /// Window-focus fast path: one cheap HTTP probe of the edge decides
    /// whether to un-park every reconnect backoff NOW (success → online
    /// event → immediate redials with fresh backoff) or to leave them
    /// backing off (failure — a dial can't succeed either, so don't burn
    /// the attempt). Recovery rides the "user looked at the app" event
    /// instead of timer luck.
    pub fn probe_edge_reachability(&self) {
        let Some(edge) = self.inner.config.edge.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let url = format!("{}/health", edge.url.trim_end_matches('/'));
        let http = self.inner.http.clone();
        self.spawn_worker_on(&runtime, async move {
            let res = http
                .get(&url)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await;
            if let Ok(res) = res
                && res.status().is_success()
            {
                zeron_sync::wake::notify_online();
            }
        });
    }

    /// The in-flight queued-attachment transfer set: current entries first,
    /// then a fresh snapshot per landed chunk (see `push_attachments`).
    pub fn watch_transfers(&self) -> watch::Receiver<Vec<zeron_proto::TransferProgress>> {
        self.inner.transfers.subscribe()
    }

    /// Publish one transfer's progress (upserted by uploadId).
    fn transfer_progress_set(&self, upload_id: &str, file_name: &str, done: u64, total: u64) {
        self.inner.transfers.send_modify(|list| {
            match list.iter_mut().find(|t| t.upload_id == upload_id) {
                Some(t) => {
                    t.done = done;
                    t.total = total;
                }
                None => list.push(zeron_proto::TransferProgress {
                    upload_id: upload_id.to_string(),
                    file_name: file_name.to_string(),
                    done,
                    total,
                }),
            }
        });
    }

    /// Retire a transfer's progress entry (commit landed, or the attempt
    /// failed and the retry will re-publish).
    fn transfer_progress_clear(&self, upload_id: &str) {
        self.inner.transfers.send_modify(|list| {
            list.retain(|t| t.upload_id != upload_id);
        });
    }

    /// The connectivity stream: current posture first, then every change.
    /// Lazily starts a monitor — a 1s recompute over in-memory stats
    /// (atomics + small locks), published only when the value changes. The
    /// retry countdown renders client-side from `retry_at_ms`, so quiet
    /// periods emit nothing at all.
    pub fn watch_connectivity(&self) -> watch::Receiver<zeron_proto::Connectivity> {
        let tx = self
            .inner
            .connectivity
            .get_or_init(|| watch::channel(self.compute_connectivity()).0);
        let rx = tx.subscribe();
        if !self.inner.connectivity_started.swap(true, Ordering::SeqCst)
            && tokio::runtime::Handle::try_current().is_ok()
        {
            let host = self.clone();
            self.spawn_worker(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let next = host.compute_connectivity();
                    if let Some(tx) = host.inner.connectivity.get() {
                        tx.send_if_modified(|cur| {
                            if *cur == next {
                                false
                            } else {
                                *cur = next;
                                true
                            }
                        });
                    }
                }
            });
        }
        rx
    }

    /// One snapshot of the edge posture: OS path status beats registry-room
    /// state beats per-chat rooms. `Disabled` (the default) = local profile.
    ///
    /// Degradation is HYSTERETIC (v0.2.12 feedback): a room mid-join, an
    /// idle link waking for a send, or a navigation-triggered dial all read
    /// "disconnected" for a few hundred ms on a healthy network — surfacing
    /// those flashed amber warnings and "Queued" badges at every chat
    /// switch. Raw degradation must persist [`DEGRADE_GRACE`] before it is
    /// reported; recovery reports instantly.
    fn compute_connectivity(&self) -> zeron_proto::Connectivity {
        use zeron_proto::{ChatConnectivity, Connectivity, ConnectivityState};
        let workspace = self.workspace();
        let edge_expected = self.inner.config.edge.is_some()
            || workspace.as_ref().is_some_and(|w| w.edge_expected());
        if !edge_expected {
            return Connectivity::default();
        }
        let now = std::time::Instant::now();
        let mut handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        handles.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        let mut grace = lock(&self.inner.connectivity_grace);
        grace.retain_chats(|id| handles.iter().any(|handle| handle.chat_id == id));
        let chats = handles
            .into_iter()
            .map(|handle| {
                let snapshot = ChatConnectionSnapshot::read(&handle);
                let stats = snapshot.stats.unwrap_or_default();
                // An idle chat deliberately has no room. Clear any previous
                // timer so selecting it starts a fresh grace window.
                let connected = !grace.degraded(
                    GraceKey::Chat(&handle.chat_id),
                    snapshot.sync_expected() && !stats.connected,
                    now,
                );
                ChatConnectivity {
                    sync_state: self.chat_sync_state_for_handle(&handle, &snapshot),
                    chat_id: handle.chat_id.clone(),
                    connected,
                    delivery_live: snapshot.delivery_live,
                    pending_pushes: stats.pending_pushes,
                }
            })
            .collect();
        let reconnect = workspace.as_ref().and_then(|w| w.reconnect_state());
        let registry_connected = workspace
            .as_ref()
            .and_then(|w| w.sync_status())
            .is_some_and(|s| s.connected);
        let path_offline =
            grace.degraded(GraceKey::OsPath, zeron_sync::wake::path_is_offline(), now);
        let registry_down = grace.degraded(GraceKey::Registry, !registry_connected, now);
        let (state, retry_at_ms, last_failure) = if path_offline {
            (
                ConnectivityState::Offline,
                0,
                reconnect.and_then(|r| r.last_failure),
            )
        } else if !registry_down {
            (ConnectivityState::Connected, 0, None)
        } else {
            let reconnect = reconnect.unwrap_or_default();
            (
                ConnectivityState::Reconnecting,
                reconnect.retry_at_ms,
                reconnect.last_failure,
            )
        };
        Connectivity {
            state,
            retry_at_ms,
            last_failure,
            chats,
        }
    }

    pub fn chat_sync_state(&self, chat_id: &str) -> zeron_proto::ChatSyncState {
        use zeron_proto::ChatSyncState as S;
        let handle = lock(&self.inner.handles).get(chat_id).cloned();
        let Some(handle) = handle else {
            return S::Local;
        };
        let snapshot = ChatConnectionSnapshot::read(&handle);
        self.chat_sync_state_for_handle(&handle, &snapshot)
    }

    fn chat_sync_state_for_handle(
        &self,
        handle: &ChatDocHandle,
        snapshot: &ChatConnectionSnapshot,
    ) -> zeron_proto::ChatSyncState {
        use zeron_proto::ChatSyncState as S;
        if handle.publication_failed.load(Ordering::Acquire) {
            return S::StorageError;
        }
        if self.inner.config.edge.is_none() {
            return S::Local;
        }
        if !snapshot.sync_started {
            return if snapshot.sync_requested {
                S::Waiting
            } else {
                S::Local
            };
        }
        match snapshot.stats {
            Some(stats) if snapshot.delivery_live && stats.pending_pushes == 0 => S::Synced,
            Some(_) if !snapshot.delivery_live && zeron_sync::wake::path_is_offline() => S::Offline,
            _ => S::Connecting,
        }
    }

    pub fn sync_resources(&self) -> serde_json::Value {
        let handles = lock(&self.inner.handles);
        let mut reasons =
            serde_json::json!({"views":0,"writers":0,"storageFailures":0,"connections":0});
        let mut waiting = 0usize;
        let mut oldest = 0i64;
        for h in handles.values() {
            if h.messages_tx.receiver_count() > 0 || h.queue_tx.receiver_count() > 0 {
                reasons["views"] = (reasons["views"].as_u64().unwrap() + 1).into();
            }
            if h.writers.load(Ordering::Acquire) > 0 {
                reasons["writers"] = (reasons["writers"].as_u64().unwrap() + 1).into();
            }
            if h.publication_failed.load(Ordering::Acquire) {
                reasons["storageFailures"] =
                    (reasons["storageFailures"].as_u64().unwrap() + 1).into();
            }
            if h.sync_started.load(Ordering::Acquire) {
                reasons["connections"] = (reasons["connections"].as_u64().unwrap() + 1).into();
            } else if h.sync_requested.load(Ordering::Acquire) {
                waiting += 1;
                oldest = oldest.max(now_ms() - h.last_access.load(Ordering::Relaxed));
            }
        }
        #[cfg(target_os = "linux")]
        let open_fds = std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|fds| fds.count());
        #[cfg(not(target_os = "linux"))]
        let open_fds: Option<usize> = None;
        #[cfg(unix)]
        let fd_limit = unsafe {
            let mut limit: libc::rlimit = std::mem::zeroed();
            (libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0).then_some(limit.rlim_cur as u64)
        };
        #[cfg(not(unix))]
        let fd_limit: Option<u64> = None;
        serde_json::json!({
            "budget": zeron_sync::budget::shared().stats(),
            "activeClientLimit": ACTIVE_SYNC_CAP,
            "openDocuments": handles.len(), "waitingDocuments": waiting,
            "documentLoads": self.inner.document_loads.load(Ordering::Relaxed),
            "oldestWaitingAccessAgeMs": oldest, "retainedBy": reasons,
            "openFileDescriptors": open_fds, "fileDescriptorLimit": fd_limit,
            "durable": self.inner.store.sync_work_counts().ok().map(|(batches, jobs)| serde_json::json!({"pendingBatches": batches, "pendingJobs": jobs})),
        })
    }

    /// Per-open-chat room introspection for SyncStatus / `zeron sync`.
    /// `None` room = still dialing (join retry loop) or edge-less.
    pub fn sync_statuses(&self) -> Vec<(String, Option<zeron_sync::ChatStatsSnapshot>)> {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        let mut rows: Vec<(String, Option<zeron_sync::ChatStatsSnapshot>)> = handles
            .iter()
            .map(|h| {
                (
                    h.chat_id.clone(),
                    lock(&h.chat2).as_ref().map(|client| client.stats()),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// Drop a chat's doc unconditionally and delete its local snapshot — the
    /// chat is gone (DeleteChat / DeleteSpace cascade). Watchers see the
    /// stream end; a racing writer keeps its orphaned doc until the run ends.
    pub fn purge_chat(&self, chat_id: &str) {
        let _opening = lock(&self.inner.opening);
        let removed = lock(&self.inner.handles).remove(chat_id);
        if let Some(handle) = &removed {
            // A racing orphaned writer may already have entered its callback.
            // Fence its submit before draining and deleting the outbox.
            let order = lock(&handle.own_ops_order).clone();
            let _order = order.as_ref().map(|order| lock(order));
            handle.purged.store(true, Ordering::Release);
        }
        if let Some(handle) = &removed {
            lock(&handle.chat2_local_sub).take();
            lock(&handle.publication_barrier).take();
            if let Some(persistence) = &handle.persistence {
                persistence.stop();
            }
            lock(&handle.chat2).take();
        }
        self.inner.outbox.flush();
        if let Err(err) = self.inner.store.delete_snapshot(chat_id) {
            tracing::warn!(chat = %chat_id, error = %err, "snapshot delete failed");
        }
    }

    /// Composer path: append an immutable pending command entry (rule 1). Durable by
    /// construction — the change subscription kicks the drain, so a local host executes
    /// immediately and an offline doc simply holds the entry until it syncs.
    pub fn queue_command(
        &self,
        chat_id: &str,
        payload: SessionCommandPayload,
    ) -> Result<String, EngineError> {
        self.queue_command_with_transfers(chat_id, payload, Vec::new())
    }

    /// [`Self::queue_command`] plus queued-attachment transfers: the command's
    /// `pending://` refs name bytes already committed to THIS device's uploads
    /// dir; when another device hosts the chat, a background task pushes them
    /// over the peer link (retry until they land) while the command is
    /// already durably queued. The send is a local write — attachment bytes
    /// chase it, never gate it (2026-08-19 incident).
    pub fn queue_command_with_transfers(
        &self,
        chat_id: &str,
        payload: SessionCommandPayload,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) -> Result<String, EngineError> {
        let handle = self.open(chat_id)?;
        let id = new_id();
        let now = now_ms();
        let based_on = handle.doc.read_entries()?.last().map(|m| CommandBasedOn {
            turn_id: Some(m.id.clone()),
            frontier: None,
        });
        let is_message = matches!(
            payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        );
        let entry = SessionCommandEntry {
            id: id.clone(),
            payload,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now,
            based_on,
            expires_at: Some(now + COMMAND_DEFAULT_TTL_MS),
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        handle.doc.queue_command(&entry)?;
        // The command is accepted only once its outbox row is written.
        self.inner.outbox.flush();
        // Sending a message revives an archived chat: the user is acting in it
        // again, so the LWW row flips back to active on every device. Best-
        // effort — the command itself is durable regardless.
        if is_message {
            self.unarchive_on_send(chat_id);
        }
        // The cold host must discover the command even after its room rows
        // are ACKed. Persist that wake independently and recover it on restart.
        self.nudge_remote_host(chat_id);
        self.spawn_command_delivery(chat_id, entry, transfers);
        Ok(id)
    }

    /// A send revives an archived chat on every device (best-effort).
    fn unarchive_on_send(&self, chat_id: &str) {
        let Some(workspace) = self.workspace() else {
            return;
        };
        match workspace.chat(chat_id) {
            Ok(Some(chat)) if chat.archived => {
                if let Err(err) = workspace.set_chat_archived(chat_id, false) {
                    tracing::warn!(chat = %chat_id, error = %err, "unarchive on send failed");
                }
            }
            _ => {}
        }
    }

    /// Hold a message for later: append it to the doc's queue. Any device may
    /// write here (unlike `messages`), and the change subscription kicks
    /// [`Self::drain_queue`], so a queue that lands while the agent is already
    /// idle goes straight out instead of waiting for a turn that never comes.
    pub fn queue_message(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
    ) -> Result<String, EngineError> {
        self.queue_message_with_behavior(chat_id, text, attachments, false)
    }

    /// Append a message while preserving the submitter's active-turn policy
    /// on the synchronized row. This matters when another device hosts the
    /// chat: the host, not the submitting UI, decides when to drain it.
    pub fn queue_message_with_behavior(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
    ) -> Result<String, EngineError> {
        let handle = self.open(chat_id)?;
        let id = new_id();
        handle.doc.push_queued(&QueuedMessage {
            id: id.clone(),
            text: text.to_string(),
            attachments,
            hold_for_turn_end,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now_ms(),
            edited_at: None,
            delivery_gate: None,
        })?;
        handle.publish_queue();
        // Same reasoning as a command: the user is acting in this chat again.
        self.unarchive_on_send(chat_id);
        self.nudge_remote_host(chat_id);
        Ok(id)
    }

    /// Retype a queued message. Empty text deletes the row — emptying the box
    /// is how you say "drop it". `false` when the row is already gone.
    pub fn update_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.set_queued_text(id, text, now_ms())?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Reorder a queued message (drag, or the up/down buttons).
    pub fn move_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        to_index: usize,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.move_queued(id, to_index)?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Cancel one queued message at the chat host. Removal shares the same
    /// lock as automatic and explicit delivery, so the acknowledgement is the
    /// linearization point: `true` guarantees this host did not take the row.
    pub async fn remove_queued_message(
        &self,
        chat_id: &str,
        id: &str,
    ) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let removed = handle.doc.remove_queued(id)?;
        if removed {
            handle.publish_queue();
        }
        Ok(removed)
    }

    /// Acquire the host-side right to edit one queued row. This operation and
    /// every queue take share `drain_lock`, making the ACK the linearization
    /// point: after Acquired the row cannot race into the agent.
    pub async fn begin_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        owner_device_id: &str,
        owner_instance_id: &str,
    ) -> Result<BeginQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(BeginQueueEditOutcome::Missing);
        };
        if let Some(QueueDeliveryGate::Editing {
            owner_device_id,
            expires_at_ms,
            ..
        }) = &item.delivery_gate
            && *expires_at_ms > now
        {
            return Ok(BeginQueueEditOutcome::Locked {
                owner_device_id: owner_device_id.clone(),
                expires_at_ms: *expires_at_ms,
            });
        }

        let lease_id = new_id();
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let gate = QueueDeliveryGate::Editing {
            lease_id: lease_id.clone(),
            owner_device_id: owner_device_id.to_string(),
            owner_instance_id: owner_instance_id.to_string(),
            acquired_at_ms: now,
            expires_at_ms,
            base_text_hash: queue_text_hash(&item.text),
        };
        let base_text_hash = queue_text_hash(&item.text);
        if !handle.doc.set_queued_delivery_gate(id, Some(&gate))? {
            return Ok(BeginQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        // A crash immediately after the client sees Acquired must not reopen
        // the row as sendable from a pre-lease snapshot.
        self.save_snapshot(&handle);
        self.arm_queue_edit_expiry(&handle, id, &lease_id, expires_at_ms);
        Ok(BeginQueueEditOutcome::Acquired {
            lease_id,
            text: item.text,
            attachments: item.attachments,
            base_text_hash,
            expires_at_ms,
        })
    }

    /// Extend an edit lease. An already-expired generation is never revived;
    /// it remains blocked and will be surfaced as ReviewRequired.
    pub async fn renew_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
    ) -> Result<RenewQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(RenewQueueEditOutcome::Missing);
        };
        let QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        } = item
            .delivery_gate
            .unwrap_or(QueueDeliveryGate::ReviewRequired {
                previous_lease_id: String::new(),
                owner_device_id: String::new(),
                since_ms: now,
                base_text_hash: String::new(),
            })
        else {
            return Ok(RenewQueueEditOutcome::Lost);
        };
        if current != lease_id || expires_at_ms <= now {
            if current == lease_id && expires_at_ms <= now {
                let review = QueueDeliveryGate::ReviewRequired {
                    previous_lease_id: current,
                    owner_device_id,
                    since_ms: now,
                    base_text_hash,
                };
                let _ = handle.doc.set_queued_delivery_gate(id, Some(&review));
                handle.publish_queue();
                self.save_snapshot(&handle);
            }
            return Ok(RenewQueueEditOutcome::Lost);
        }
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let renewed = QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        };
        if !handle.doc.set_queued_delivery_gate(id, Some(&renewed))? {
            return Ok(RenewQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        self.arm_queue_edit_expiry(&handle, id, lease_id, expires_at_ms);
        Ok(RenewQueueEditOutcome::Renewed { expires_at_ms })
    }

    /// Resolve an edit lease. A late finish may still resolve the matching
    /// ReviewRequired generation, but can never affect a newer lease.
    pub async fn finish_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        self.finish_queued_message_edit_with_attachments(
            chat_id,
            id,
            lease_id,
            action,
            text,
            expected_text_hash,
            None,
        )
        .await
    }

    pub async fn finish_queued_message_edit_with_attachments(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
        attachments: Option<&[String]>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        if action == FinishQueueEditAction::Commit
            && (text.is_none() || expected_text_hash.is_none())
        {
            return Err(EngineError::Other(
                "commit requires text and expectedTextHash".into(),
            ));
        }
        let handle = self.open(chat_id)?;
        let outcome = {
            let _drain = handle.drain_lock.lock().await;
            let Some(item) = handle
                .doc
                .read_queue()?
                .into_iter()
                .find(|item| item.id == id)
            else {
                return Ok(FinishQueueEditOutcome::Missing);
            };
            let (current_lease, base_text_hash) = match &item.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    base_text_hash,
                    ..
                }) => (lease_id, base_text_hash),
                Some(QueueDeliveryGate::ReviewRequired {
                    previous_lease_id,
                    base_text_hash,
                    ..
                }) => (previous_lease_id, base_text_hash),
                None => return Ok(FinishQueueEditOutcome::Lost),
            };
            if current_lease != lease_id {
                return Ok(FinishQueueEditOutcome::Lost);
            }
            if action == FinishQueueEditAction::Commit
                && (expected_text_hash != Some(base_text_hash.as_str())
                    || queue_text_hash(&item.text) != *base_text_hash)
            {
                return Ok(FinishQueueEditOutcome::Conflict {
                    current_text: item.text,
                });
            }

            let replacement = match action {
                FinishQueueEditAction::Commit => Some(text.unwrap_or_default()),
                FinishQueueEditAction::Cancel | FinishQueueEditAction::ReleaseUnchanged => None,
                FinishQueueEditAction::Discard => Some(""),
            };
            if !handle.doc.finish_queued_edit_with_attachments(
                id,
                replacement,
                if action == FinishQueueEditAction::Commit {
                    attachments
                } else {
                    None
                },
                now_ms(),
            )? {
                return Ok(FinishQueueEditOutcome::Missing);
            }
            handle.publish_queue();
            self.save_snapshot(&handle);
            match action {
                FinishQueueEditAction::Commit => FinishQueueEditOutcome::Committed,
                FinishQueueEditAction::Cancel => FinishQueueEditOutcome::Cancelled,
                FinishQueueEditAction::Discard => FinishQueueEditOutcome::Discarded,
                FinishQueueEditAction::ReleaseUnchanged => FinishQueueEditOutcome::Released,
            }
        };
        // Turn-end may already have happened while the edit was open.
        self.drain_queue(&handle).await;
        Ok(outcome)
    }

    fn arm_existing_queue_edit_expiries(&self, handle: &Arc<ChatDocHandle>) {
        let Ok(queue) = handle.doc.read_queue() else {
            return;
        };
        for item in queue {
            if let Some(QueueDeliveryGate::Editing {
                lease_id,
                expires_at_ms,
                ..
            }) = item.delivery_gate
            {
                self.arm_queue_edit_expiry(handle, &item.id, &lease_id, expires_at_ms);
            }
        }
    }

    fn arm_queue_edit_expiry(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        expires_at_ms: i64,
    ) {
        let delay_ms = expires_at_ms.saturating_sub(now_ms()).max(0) as u64;
        let host = self.clone();
        let handle = handle.clone();
        let id = id.to_string();
        let lease_id = lease_id.to_string();
        self.spawn_worker(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            host.expire_queued_message_edit(&handle, &id, &lease_id, expires_at_ms)
                .await;
        });
    }

    /// Expire exactly the lease generation that scheduled this wake. A stale
    /// timer from before a renewal observes a different deadline and no-ops;
    /// timers for other rows are completely independent.
    async fn expire_queued_message_edit(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        scheduled_expires_at_ms: i64,
    ) {
        let changed = {
            let _drain = handle.drain_lock.lock().await;
            let Ok(queue) = handle.doc.read_queue() else {
                return;
            };
            let Some(item) = queue.into_iter().find(|item| item.id == id) else {
                return;
            };
            let Some(QueueDeliveryGate::Editing {
                lease_id: current_lease_id,
                owner_device_id,
                expires_at_ms,
                base_text_hash,
                ..
            }) = item.delivery_gate
            else {
                return;
            };
            if current_lease_id != lease_id
                || expires_at_ms != scheduled_expires_at_ms
                || expires_at_ms > now_ms()
            {
                return;
            }
            let review = QueueDeliveryGate::ReviewRequired {
                previous_lease_id: current_lease_id,
                owner_device_id,
                since_ms: now_ms(),
                base_text_hash,
            };
            let Ok(changed) = handle.doc.set_queued_delivery_gate(id, Some(&review)) else {
                return;
            };
            if changed {
                handle.publish_queue();
                self.save_snapshot(handle);
            }
            changed
        };
        if changed {
            // If this was the head, the drain now observes ReviewRequired. If
            // it was not, publishing still updates every client's row state.
            self.drain_queue(handle).await;
        }
    }

    /// "Send this one now": take it out of the queue and put it in front of the
    /// agent, interrupting whatever is running. Deliberately blunt — it is the
    /// explicit override. The empty-composer Enter gesture reaches this path
    /// only when the selected provider cannot steer the row. `false` when
    /// another device already took it.
    pub async fn send_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        // Sending one now sends ONE: the lock keeps the flush out of the idle
        // window the interrupt opens, and out of the take itself.
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        if let Err(err) = self
            .dispatch_queued(&handle, &item, QueueSend::Interrupt)
            .await
        {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            // Put it back rather than swallowing what the user typed — at the
            // head, because the user just said this one was the urgent one.
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Promote one held row without ever interrupting a turn. A live,
    /// steerable turn receives it as steering; if that turn has already ended,
    /// it starts normally as the next turn. Turn-boundary providers retain it
    /// in their mailbox until ready. Attachment-bearing rows stay untouched.
    pub async fn steer_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        if !candidate.attachments.is_empty() {
            return Err(EngineError::Other(
                "messages with attachments cannot be steered mid-turn".into(),
            ));
        }
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        if self
            .sessions()
            .is_some_and(|sessions| sessions.defers_to_turn_end(chat_id, None))
        {
            // Send next: lead the ordinary rows; the drain delivers it the
            // moment the current turn ends.
            let Some(item) = handle.doc.take_queued(id)? else {
                return Ok(false);
            };
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
            handle.queue_paused.store(false, Ordering::Release);
            handle.publish_queue();
            return Ok(true);
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        // Always attempt the non-interrupting path first. If no turn exists,
        // `dispatch_queued` falls through to NextTurn; if a new turn appeared
        // after our capability check, the prompt steers that turn instead.
        if let Err(err) = self.dispatch_queued(&handle, &item, QueueSend::Steer).await {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Host-only: hand queued messages to the agent when there is somewhere to
    /// put them.
    ///
    /// - Idle: send the head as the next turn (and loop — the agent is free).
    /// - Turn in flight: hold. The turn-end watcher comes back for it.
    ///
    /// One at a time by design: each send changes the status this reads.
    pub async fn drain_queue(&self, handle: &Arc<ChatDocHandle>) {
        let Some(_dispatch) = self.dispatch_guard() else {
            return; // quiesced for a Cloud sleep; lifting it re-drains
        };
        let Some(sessions) = self.sessions() else {
            return; // executor not wired yet; the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        // One drain at a time per chat. Waiters are cheap: whoever takes the
        // lock next re-reads the queue and the status, so a drain that became
        // unnecessary while it waited simply finds nothing to do.
        let _drain = handle.drain_lock.lock().await;
        if handle.queue_paused.load(Ordering::Acquire) {
            return;
        }
        loop {
            let Ok(Some(head)) = handle.doc.read_queue().map(|q| q.into_iter().next()) else {
                return;
            };
            match &head.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    owner_device_id,
                    expires_at_ms,
                    base_text_hash,
                    ..
                }) if *expires_at_ms <= now_ms() => {
                    let review = QueueDeliveryGate::ReviewRequired {
                        previous_lease_id: lease_id.clone(),
                        owner_device_id: owner_device_id.clone(),
                        since_ms: now_ms(),
                        base_text_hash: base_text_hash.clone(),
                    };
                    let _ = handle.doc.set_queued_delivery_gate(&head.id, Some(&review));
                    handle.publish_queue();
                    self.save_snapshot(handle);
                    return;
                }
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    expires_at_ms,
                    ..
                }) => {
                    self.arm_queue_edit_expiry(handle, &head.id, lease_id, *expires_at_ms);
                    return;
                }
                Some(QueueDeliveryGate::ReviewRequired { .. }) => return,
                None => {}
            }
            // In flight, not just Working: an agent parked on a question owns
            // the turn too, and the composer queues on the same reading. Taking
            // `AwaitingInput` for idle would send the follow-up as a fresh turn
            // and abandon the question.
            if sessions.turn_in_flight(&handle.chat_id) {
                return; // All queued messages wait, including rows from older clients.
            }
            // A row from another device names its images by `pending://` ref
            // while the bytes chase it over the peer link. Hold it (in order)
            // until they land — UploadCommit re-drains — rather than handing
            // the agent refs it cannot open.
            if !self.missing_row_attachments(&head).is_empty() {
                tracing::info!(chat = %handle.chat_id, row = %head.id,
                    "queued row held: attachment bytes in transit");
                self.arm_attachment_wait(handle);
                return;
            }
            let send = QueueSend::NextTurn;
            // Take it only once we know it is going out — a row that stays in
            // the queue on a failed send is recoverable; a vanished one is not.
            let Ok(Some(item)) = handle.doc.take_queued(&head.id) else {
                return;
            };
            lock(&handle.steered_rows).retain(|row| row != &item.id);
            handle.publish_queue();
            if let Err(err) = self.dispatch_queued(handle, &item, send).await {
                tracing::warn!(chat = %handle.chat_id, error = %err, "queued send failed");
                handle.queue_paused.store(true, Ordering::Release);
                let _ = handle.doc.insert_queued(0, &item);
                handle.publish_queue();
                return;
            }
        }
    }

    /// A Stop acts on everything sent before it, in the order the user sent
    /// it. Controls bypass the prompt drain (so a stalled provider stays
    /// stoppable), which let a Stop execute before a send queued ahead of it
    /// had dispatched: with no turn yet it did nothing and the turn then ran
    /// in full; mid-turn, the earlier steer landed after the Stop and read as
    /// the user's next message, re-running in the fresh runtime. Wait for
    /// earlier prompts to dispatch (milliseconds) — bounded short while a
    /// turn is in flight, so a provider wedged on backpressure still stops.
    async fn await_earlier_prompts(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        stop_id: &str,
    ) {
        // Subscribe BEFORE the first read: a ledger change after it flags the
        // receiver, so a wake can never be lost between read and wait.
        let mut ledger = handle.commands_tx.subscribe();
        let started = tokio::time::Instant::now();
        loop {
            let bound = if sessions.turn_in_flight(&handle.chat_id) {
                std::time::Duration::from_secs(2)
            } else {
                std::time::Duration::from_secs(15)
            };
            let remaining = bound.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return;
            }
            let Ok(commands) = handle.commands() else {
                return;
            };
            let Some(at) = commands.iter().position(|c| c.id == stop_id) else {
                return;
            };
            let earlier_prompt = commands[..at].iter().any(|c| {
                c.status == SessionCommandStatus::Pending
                    && matches!(
                        c.payload,
                        SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                    )
            });
            if !earlier_prompt {
                return;
            }
            // Wake on a ledger change. The tick only re-evaluates the bound
            // above (a turn starting shortens it); it reads nothing.
            if !wait_for_ledger(&mut ledger, remaining.min(WAIT_RECHECK)).await {
                return;
            }
        }
    }

    /// A prompt queued after a control that is still executing (a Stop
    /// tearing the runtime down) waits for it: running ahead of it put the
    /// user's next message into the runtime the Stop was killing.
    async fn await_earlier_controls(&self, handle: &Arc<ChatDocHandle>, prompt_id: &str) {
        let mut ledger = handle.commands_tx.subscribe();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            let Ok(commands) = handle.commands() else {
                return;
            };
            let Some(at) = commands.iter().position(|c| c.id == prompt_id) else {
                return;
            };
            let earlier_control = {
                let executing = lock(&self.inner.executing);
                commands[..at].iter().any(|c| {
                    c.status == SessionCommandStatus::Pending
                        && !matches!(
                            c.payload,
                            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                        )
                        && executing.contains(&c.id)
                })
            };
            if !earlier_control {
                return;
            }
            // A control leaves `executing` right after it resolves, and the
            // resolve is a ledger change; the tick covers a control whose
            // outcome write failed, which only the claim set can tell us.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if !wait_for_ledger(&mut ledger, remaining.min(WAIT_RECHECK)).await {
                return;
            }
        }
    }

    /// Stop the active turn without treating the resulting Idle transition as
    /// permission to release the next queued message. The same lock used by
    /// drains closes the race between clicking Cancel and the status watcher.
    /// A parked runtime still running background work is stopped too: the
    /// idle reaper spares it, so Stop is what ends that work.
    async fn interrupt_and_pause_queue(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
    ) -> Result<bool, EngineError> {
        let _drain = handle.drain_lock.lock().await;
        if !sessions.turn_in_flight(&handle.chat_id)
            && !sessions.holds_background_work(&handle.chat_id)
        {
            return Ok(false);
        }
        handle.queue_paused.store(true, Ordering::Release);
        self.cancel_held_sends(handle);
        match sessions.interrupt(&handle.chat_id).await {
            Ok(true) => Ok(true),
            Ok(false) => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok(false)
            }
            Err(err) => {
                handle.queue_paused.store(false, Ordering::Release);
                Err(err)
            }
        }
    }

    /// A Stop cancels the messages sent during the turn it stops: the sends
    /// and steers held for that turn's end leave the queue (the user's own
    /// queued rows stay, paused) and land in the transcript unrun — never
    /// to start the stopped work again in the next runtime.
    fn cancel_held_sends(&self, handle: &Arc<ChatDocHandle>) {
        let held: Vec<String> = {
            let mut sends = lock(&handle.held_sends);
            let mut steered = lock(&handle.steered_rows);
            let ids = sends.keys().cloned().chain(steered.drain(..)).collect();
            sends.clear();
            ids
        };
        let mut changed = false;
        for id in held {
            let Ok(Some(row)) = handle.doc.take_queued(&id) else {
                continue;
            };
            changed = true;
            if let Err(err) = handle.write_user_message(&row.id, &row.text, row.issued_at) {
                tracing::warn!(chat = %handle.chat_id, error = %err, "cancelled held send write failed");
            }
        }
        if changed {
            handle.publish_queue();
        }
    }

    /// Send one taken queue row.
    async fn dispatch_queued(
        &self,
        handle: &Arc<ChatDocHandle>,
        item: &QueuedMessage,
        send: QueueSend,
    ) -> Result<(), EngineError> {
        let Some(sessions) = self.sessions() else {
            return Err(EngineError::Other("sessions engine not wired".into()));
        };
        let chat_id = &handle.chat_id;
        // Keep the queue row's identity when it becomes a real user message.
        // The submitting viewport learns this id from QueueMessage and can
        // therefore wait without disturbing the active turn's runway, then
        // anchor the prompt only once this exact row reaches the transcript.
        let message_id = item.id.clone();
        if !self.missing_row_attachments(item).is_empty() {
            return Err(EngineError::Other(
                "this message's images are still uploading".into(),
            ));
        }
        let mut attachments = item.attachments.clone();
        // A send held for the turn end goes out exactly as it was sent: its
        // text never carried the queue's image note.
        let held = lock(&handle.held_sends).remove(&item.id);
        let mut prompt = if held.is_some() {
            item.text.clone()
        } else {
            queued_message_prompt(&item.text, &attachments)
        };
        self.resolve_attachment_refs(&mut prompt, &mut attachments);
        if send == QueueSend::Steer {
            match sessions
                .steer(chat_id, &prompt, Some(message_id.clone()))
                .await?
            {
                SteerOutcome::Accepted => return Ok(()),
                SteerOutcome::DeferredByUpdate => {
                    return Err(EngineError::Other(
                        "agent update pending; the message remains queued".into(),
                    ));
                }
                // The run died under us between the status read and the send;
                // fall through and start a fresh turn with it.
                SteerOutcome::NotSteerable => {}
            }
        }
        // Same reading of "busy" as the drain: a turn parked on a question is
        // still a turn, and it has to be stopped before this one starts.
        // The message replaces the turn, not the agent: its background work
        // keeps running (only the user's Stop tears that down).
        if send == QueueSend::Interrupt && sessions.turn_in_flight(chat_id) {
            sessions.stop_turn(chat_id).await?;
        }
        let previous = sessions.last_request(chat_id);
        // ... and on its model, effort and options, not the chat's saved ones.
        let request = held.or_else(|| {
            self.request_from_chat_row(chat_id, &prompt)
                .map(|mut current| {
                    if let Some(previous) = &previous {
                        current.auto_approve = previous.auto_approve;
                        current.worktree = previous.worktree.clone();
                    }
                    current
                })
                .or(previous)
        });
        let Some(mut request) = request else {
            return Err(EngineError::Other(
                "no live run and no prior run config".into(),
            ));
        };
        request.prompt = prompt;
        request.resume = None; // dispatch re-derives the harness session
        request.attachments = attachments;
        let harness = self.harness_for_request(chat_id, &request);
        self.dispatch_with_source_context(&sessions, chat_id, harness, request, Some(message_id))
            .await?;
        Ok(())
    }

    /// Persist remote host discovery separately from the outgoing room rows.
    /// A row ACK does not prove a cold host has been told to open its room.
    fn nudge_remote_host(&self, chat_id: &str) {
        if self.inner.config.edge.is_none() || self.remote_host_for(chat_id).is_none() {
            return;
        }
        if let Err(err) = self.inner.store.schedule_sync_job(chat_id, "remote-wake") {
            tracing::warn!(chat = %chat_id, %err, "remote host wake persistence failed");
        }
    }

    /// The chat's host device when it is NOT this engine (mirrors
    /// `nudge_remote_host`'s ownership read).
    fn remote_host_for(&self, chat_id: &str) -> Option<String> {
        let workspace = self.workspace()?;
        let host_device = match workspace.chat(chat_id) {
            Ok(Some(chat)) => chat.device_id,
            _ => return None,
        };
        (host_device != self.inner.config.device_id).then_some(host_device)
    }

    /// Durable-delivery escort for one queued command aimed at a REMOTE host:
    ///
    /// 1. push any queued attachment bytes over the peer link (retry until
    ///    they land — the relayed command must never outrun its bytes);
    /// 2. give the normal path (chat2 rows → edge → host's room) a short
    ///    grace to ack;
    /// 3. rows still not at the edge but the peer link alive → relay-forward
    ///    the entry itself ([`zeron_rpc::methods::RELAY_COMMAND`]). The
    ///    host's processed ledger claims the client-minted id, so the doc
    ///    row arriving later dedupes to a no-op — exactly-once by
    ///    construction (the 2026-08-18 03:45 incident shape: nudges flowed,
    ///    rows didn't; there was no second road for the command).
    ///
    /// Stops the moment any path lands. No-op for locally-hosted chats.
    fn spawn_command_delivery(
        &self,
        chat_id: &str,
        entry: SessionCommandEntry,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return; // bare sync callers (unit tests) skip rather than panic
        };
        let host = self.clone();
        let chat = chat_id.to_string();
        self.spawn_worker_on(&runtime, async move {
            let Some(target) = host.remote_host_for(&chat) else {
                return; // local host (or no row yet claimed remotely)
            };
            if !transfers.is_empty() && !host.deliver_attachments(&chat, &transfers).await {
                return; // gave up; the drain's wait cap surfaces the failure
            }
            let mut wake = zeron_sync::wake::subscribe();
            let mut online = zeron_sync::wake::subscribe_online();
            let give_up = tokio::time::Instant::now() + RELAY_GIVE_UP;
            let grace_end = tokio::time::Instant::now() + ROWS_GRACE;
            while tokio::time::Instant::now() < grace_end {
                if host.rows_flushed(&chat) {
                    return; // rows on the edge — the normal path has it
                }
                tokio::time::sleep(ROWS_POLL).await;
            }
            let mut backoff = RELAY_BACKOFF_BASE;
            loop {
                if host.rows_flushed(&chat) {
                    return; // the normal path won while we were retrying
                }
                match host.relay_command(&target, &chat, &entry).await {
                    Ok(outcome) => {
                        tracing::info!(chat = %chat, device = %target, command = %entry.id,
                            outcome, "command delivered via peer relay");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(chat = %chat, device = %target, error = %err,
                            backoff_ms = backoff.as_millis() as u64,
                            "peer-relay delivery retrying");
                    }
                }
                if tokio::time::Instant::now() >= give_up {
                    tracing::warn!(chat = %chat, command = %entry.id,
                        "peer-relay delivery gave up; command remains queued in the doc");
                    return;
                }
                while wake.try_recv().is_ok() {}
                while online.try_recv().is_ok() {}
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = wake.recv() => {}
                    _ = online.recv() => {}
                }
                backoff = (backoff * 2).min(RELAY_BACKOFF_CAP);
            }
        });
    }

    /// Step 1 of the escort: push staged bytes until they land (event-driven
    /// backoff), `true` on success. Re-resolves the host each attempt (a
    /// claim can move the chat).
    async fn deliver_attachments(
        &self,
        chat: &str,
        transfers: &[crate::uploads::AttachmentTransfer],
    ) -> bool {
        let mut wake = zeron_sync::wake::subscribe();
        let mut online = zeron_sync::wake::subscribe_online();
        let mut backoff = TRANSFER_BACKOFF_BASE;
        let deadline = tokio::time::Instant::now() + ATTACHMENT_WAIT_MAX;
        loop {
            let Some(target) = self.remote_host_for(chat) else {
                return true; // became locally hosted: bytes already here
            };
            match self.push_attachments(&target, transfers).await {
                Ok(()) => {
                    tracing::info!(chat = %chat, device = %target,
                        count = transfers.len(), "queued attachments delivered");
                    // The bytes beat the drain's next look — kick it via the
                    // durable nudge (the host's UploadCommit already kicked
                    // its local drains too).
                    self.nudge_remote_host(chat);
                    return true;
                }
                Err(TransferError::Permanent(err)) => {
                    tracing::warn!(chat = %chat, device = %target, error = %err,
                        "queued attachment transfer failed permanently");
                    return false;
                }
                Err(TransferError::Transient(err)) => {
                    tracing::warn!(chat = %chat, device = %target, error = %err,
                        backoff_ms = backoff.as_millis() as u64,
                        "queued attachment transfer retrying");
                }
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(chat = %chat, "queued attachment transfer gave up (wait cap)");
                return false;
            }
            while wake.try_recv().is_ok() {}
            while online.try_recv().is_ok() {}
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = wake.recv() => {}
                _ = online.recv() => {}
            }
            backoff = (backoff * 2).min(TRANSFER_BACKOFF_CAP);
        }
    }

    /// Every local chat2 batch acked while connected — our rows are ON the
    /// edge, and the host's own room connection will deliver them. (A room
    /// that hasn't joined keeps pre-join updates in a local buffer, so
    /// `connected` is load-bearing here, not just the empty queue.)
    fn rows_flushed(&self, chat_id: &str) -> bool {
        let handle = lock(&self.inner.handles).get(chat_id).cloned();
        handle
            .and_then(|h| lock(&h.chat2).as_ref().map(|c| c.stats()))
            .is_some_and(|s| s.connected && s.pending_pushes == 0)
    }

    /// One relay attempt: version-gate the host, then forward the entry over
    /// the peer link. Timeouts mark the link suspect (drop + redial next
    /// attempt); a host-side refusal is permanent for THIS attempt but the
    /// escort keeps retrying until the give-up cap (the refusal may be
    /// "attachments not landed yet").
    async fn relay_command(
        &self,
        target: &str,
        chat_id: &str,
        entry: &SessionCommandEntry,
    ) -> Result<&'static str, String> {
        let supported = self
            .workspace()
            .and_then(|ws| ws.read_devices().ok())
            .into_iter()
            .flatten()
            .find(|d| d.id == target)
            .and_then(|d| d.version.as_deref().and_then(zeron_proto::version_triple))
            .is_some_and(|v| v >= RELAY_MIN_VERSION);
        if !supported {
            return Err("host does not support relay delivery (version gate)".into());
        }
        let links = self
            .inner
            .links
            .get()
            .ok_or_else(|| "peer links not wired".to_string())?;
        let client = links
            .client(target)
            .await
            .map_err(|e| format!("peer link: {e}"))?;
        let params = serde_json::json!({ "chatId": chat_id, "entry": entry });
        let call = client.call(zeron_rpc::methods::RELAY_COMMAND, params);
        match tokio::time::timeout(RELAY_CALL_TIMEOUT, call).await {
            Err(_) => {
                links.invalidate(target);
                Err("relay call timed out; peer link suspect".into())
            }
            Ok(Err(zeron_rpc::RpcError::Failed(err))) => Err(format!("host refused: {err}")),
            Ok(Err(err)) => {
                links.invalidate(target);
                Err(format!("relay call failed: {err}"))
            }
            Ok(Ok(reply)) => Ok(match reply.get("outcome").and_then(|v| v.as_str()) {
                Some("executed") => "executed",
                Some("duplicate") => "duplicate",
                Some("expired") => "expired",
                Some("superseded") => "superseded",
                _ => "accepted",
            }),
        }
    }

    /// User-driven retry (the failed-send affordance): re-kick every
    /// delivery road for a chat whose queued sends haven't been adopted —
    /// fresh chat2 socket (a zombie room is the usual suspect), host nudge,
    /// a local drain pass, and a fresh delivery escort per still-pending
    /// command with its attachment transfers re-derived from the entries'
    /// `pending://` refs (idempotent: re-pushing landed bytes re-commits the
    /// same file; the processed ledger keeps execution exactly-once).
    pub fn retry_delivery(&self, chat_id: &str) -> Result<(), EngineError> {
        let handle = self.open(chat_id)?;
        if let Some(chat2) = lock(&handle.chat2).as_ref() {
            chat2.redial();
        }
        self.nudge_remote_host(chat_id);
        let commands = handle.commands()?;
        let pending: Vec<SessionCommandEntry> = commands
            .iter()
            .filter(|c| {
                c.status == SessionCommandStatus::Pending
                    && !self.inner.store.is_processed(&c.id).unwrap_or(false)
            })
            .cloned()
            .collect();
        for entry in pending {
            let transfers = command_transfers(&entry);
            self.spawn_command_delivery(chat_id, entry, transfers);
        }
        // Dead attempts: a Run/Steer whose user message never landed and
        // whose command can never execute again — Rejected (execute failed,
        // or the dead-command sweep terminalized it), or consumed by the
        // ledger with no outcome and not currently executing (crash between
        // mark and resolve). Exactly-once is per command ID, so a
        // user-driven retry mints a FRESH attempt: new id, same payload and
        // message id (the executor's user-entry pre-write dedupes by message
        // id). One re-issue per message — the LATEST attempt speaks for it.
        let messages = handle.doc.read_entries().unwrap_or_default();
        let message_landed = |mid: &str| messages.iter().any(|m| m.id == mid);
        let mut latest_dead: HashMap<String, &SessionCommandEntry> = HashMap::new();
        for c in commands.iter() {
            let Some(mid) = (match &c.payload {
                SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
                SessionCommandPayload::Steer { message_id, .. } => message_id.as_deref(),
                _ => None,
            }) else {
                continue;
            };
            if message_landed(mid) {
                continue;
            }
            let dead = match c.status {
                // Rejected: execute failed or the sweep terminalized a crash
                // window. Expired: the entry outlived its TTL undelivered —
                // an explicit user retry is exactly the consent to re-send.
                SessionCommandStatus::Rejected | SessionCommandStatus::Expired => true,
                SessionCommandStatus::Pending => {
                    self.inner.store.is_processed(&c.id).unwrap_or(false)
                        && !lock(&self.inner.executing).contains(&c.id)
                }
                _ => false,
            };
            if !dead {
                continue;
            }
            // A LIVE pending attempt for the same message (queued or being
            // escorted above) makes a re-issue a duplicate — skip.
            let live_attempt = commands.iter().any(|o| {
                o.id != c.id
                    && o.status == SessionCommandStatus::Pending
                    && !self.inner.store.is_processed(&o.id).unwrap_or(false)
                    && match (&o.payload, &c.payload) {
                        (
                            SessionCommandPayload::Run { message_id: a, .. },
                            SessionCommandPayload::Run { message_id: b, .. },
                        ) => a == b,
                        (
                            SessionCommandPayload::Steer { message_id: a, .. },
                            SessionCommandPayload::Steer { message_id: b, .. },
                        ) => a == b,
                        _ => false,
                    }
            });
            if live_attempt {
                continue;
            }
            match latest_dead.entry(mid.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if c.issued_at > slot.get().issued_at {
                        slot.insert(c);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(c);
                }
            }
        }
        for old in latest_dead.values() {
            if old.status == SessionCommandStatus::Pending {
                // Terminalize the consumed-but-dead original so the doc tells
                // the truth and the next retry pass doesn't see it again.
                self.resolve_command(
                    &handle,
                    &old.id,
                    SessionCommandStatus::Rejected,
                    Some("interrupted before completion — superseded by retry"),
                );
            }
            let now = now_ms();
            let reissue = SessionCommandEntry {
                id: new_id(),
                payload: old.payload.clone(),
                issued_by: self.inner.config.device_id.clone(),
                issued_at: now,
                based_on: messages.last().map(|m| CommandBasedOn {
                    turn_id: Some(m.id.clone()),
                    frontier: None,
                }),
                expires_at: Some(now + COMMAND_DEFAULT_TTL_MS),
                status: SessionCommandStatus::Pending,
                resolution: None,
            };
            tracing::info!(chat = %chat_id, old = %old.id, new = %reissue.id,
                "retry re-issues a dead send attempt");
            handle.doc.queue_command(&reissue)?;
            self.inner.outbox.flush();
            let transfers = command_transfers(&reissue);
            self.spawn_command_delivery(chat_id, reissue, transfers);
        }
        // Locally-hosted (or already-synced) commands: a drain pass is the
        // whole retry.
        if tokio::runtime::Handle::try_current().is_ok() {
            let host = self.clone();
            let handle = handle.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
            });
        }
        Ok(())
    }

    /// Host side of [`zeron_rpc::methods::RELAY_COMMAND`]: evaluate the
    /// forwarded entry against OUR doc (dedupe/TTL/supersede rules apply
    /// unchanged), claim its client-minted id in the processed ledger, then
    /// execute. The claim is what makes the doc row arriving later — over
    /// chat2 sync — a no-op in the drain: exactly-once across both roads.
    pub async fn ingest_relayed_command(
        &self,
        chat_id: &str,
        entry: SessionCommandEntry,
    ) -> Result<&'static str, EngineError> {
        let handle = self.open(chat_id)?;
        // The sender sequences attachment transfers BEFORE the relay; refuse
        // (retryably) rather than run without the images.
        if !self.missing_attachments(&entry).is_empty() {
            return Err(EngineError::Other("attachments not landed yet".into()));
        }
        let sessions = self
            .sessions()
            .ok_or_else(|| EngineError::Other("executor unavailable".into()))?;
        let _prompt_guard = if matches!(
            entry.payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        ) {
            Some(handle.command_drain_lock.lock().await)
        } else {
            None
        };
        let commands = handle.commands()?;
        let messages = handle.doc.read_entries().unwrap_or_default();
        let current_turn_id = messages.last().map(|m| m.id.clone());
        let turn_is_past = |turn_id: &str| messages.iter().any(|m| m.id == turn_id);
        let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
        let disposition = evaluate_command(
            &entry,
            &EvaluationContext {
                is_processed: &is_processed,
                now_ms: now_ms(),
                entries: &commands,
                current_turn_id: current_turn_id.as_deref(),
                turn_is_past: &turn_is_past,
            },
        );
        if matches!(disposition, CommandDisposition::Skip) {
            return Ok("duplicate");
        }
        // In-flight claim first (the drain's dead-command sweep must see this
        // id as alive, not crashed, while the execute below runs).
        if !lock(&self.inner.executing).insert(entry.id.clone()) {
            return Ok("duplicate");
        }
        // Claim BEFORE executing (the drain's own mark-before-execute rule).
        let marked = self.inner.store.mark_processed(&entry.id);
        let result = match marked {
            Err(err) => Err(err.into()),
            Ok(false) => Ok("duplicate"),
            Ok(true) => match disposition {
                CommandDisposition::Skip => Ok("duplicate"),
                CommandDisposition::Expired => Ok("expired"),
                CommandDisposition::Superseded => Ok("superseded"),
                CommandDisposition::Execute => match {
                    if _prompt_guard.is_some() {
                        self.await_earlier_controls(&handle, &entry.id).await;
                    }
                    self.execute(&sessions, &handle, &entry).await
                } {
                    Ok(_) => Ok("executed"),
                    Err(err) => Err(err),
                },
            },
        };
        lock(&self.inner.executing).remove(&entry.id);
        result
    }

    /// One transfer attempt: chunked `UploadChunk` + `UploadCommit` straight
    /// over the peer link (same wire the UI's legacy path used, so old and
    /// new engines interoperate). Timeouts mark the link suspect —
    /// `invalidate` drops the cached socket so the retry dials fresh instead
    /// of feeding a zombie pipe forever (2026-08-19 incident).
    async fn push_attachments(
        &self,
        target: &str,
        transfers: &[crate::uploads::AttachmentTransfer],
    ) -> Result<(), TransferError> {
        use TransferError::{Permanent, Transient};
        let Some(links) = self.inner.links.get() else {
            return Err(Permanent("peer links not wired".into()));
        };
        let Some(uploads) = self.inner.uploads.get() else {
            return Err(Permanent("uploads not wired".into()));
        };
        let client = links
            .client(target)
            .await
            .map_err(|e| Transient(format!("peer link: {e}")))?;
        for transfer in transfers {
            // Bytes come from the uploads jail only — a transfer names an
            // upload identity, never an arbitrary path.
            let source = uploads.pending_target(&transfer.upload_id, &transfer.file_name);
            let bytes = tokio::fs::read(&source)
                .await
                .map_err(|e| Permanent(format!("staged attachment missing: {e}")))?;
            // Progress entry for the sender's thumbnail ring, updated per
            // landed chunk. The guard retires it on EVERY exit — commit,
            // timeout, refusal — so a dead attempt falls back to the
            // indeterminate spinner and the retry re-publishes from 0.
            let total = bytes.len() as u64;
            self.transfer_progress_set(&transfer.upload_id, &transfer.file_name, 0, total);
            let _progress = TransferProgressGuard {
                host: self,
                upload_id: &transfer.upload_id,
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let mut start = 0usize;
            let mut seq = 0u64;
            loop {
                let end = (start + TRANSFER_CHUNK_B64).min(b64.len());
                let params = serde_json::json!({
                    "uploadId": transfer.upload_id, "seq": seq, "data": &b64[start..end],
                });
                let call = client.call(zeron_rpc::methods::UPLOAD_CHUNK, params);
                match tokio::time::timeout(TRANSFER_CHUNK_TIMEOUT, call).await {
                    Err(_) => {
                        links.invalidate(target);
                        return Err(Transient("chunk push timed out; peer link suspect".into()));
                    }
                    Ok(Err(zeron_rpc::RpcError::Failed(err))) => {
                        return Err(Permanent(format!("host refused chunk: {err}")));
                    }
                    Ok(Err(err)) => {
                        links.invalidate(target);
                        return Err(Transient(format!("chunk push failed: {err}")));
                    }
                    Ok(Ok(_)) => {}
                }
                start = end;
                seq += 1;
                // b64 → raw: 4 chars carry 3 bytes; min-clamp absorbs the
                // final chunk's padding overshoot.
                let done = ((start as u64) * 3 / 4).min(total);
                self.transfer_progress_set(&transfer.upload_id, &transfer.file_name, done, total);
                if start >= b64.len() {
                    break;
                }
            }
            let params = serde_json::json!({
                "uploadId": transfer.upload_id, "fileName": transfer.file_name,
            });
            let call = client.call(zeron_rpc::methods::UPLOAD_COMMIT, params);
            match tokio::time::timeout(TRANSFER_COMMIT_TIMEOUT, call).await {
                Err(_) => {
                    links.invalidate(target);
                    return Err(Transient("commit timed out; peer link suspect".into()));
                }
                Ok(Err(zeron_rpc::RpcError::Failed(err))) => {
                    return Err(Permanent(format!("host refused commit: {err}")));
                }
                Ok(Err(err)) => {
                    links.invalidate(target);
                    return Err(Transient(format!("commit failed: {err}")));
                }
                Ok(Ok(_)) => {}
            }
        }
        Ok(())
    }

    /// Upload a tool result's full output/diff to the R2 sidecar
    /// (`PUT {edge}/blob/{chatId}/{partId}[.diff]`, docs/chat2-sync.md A2).
    /// Fire-and-forget: the doc already carries the summary, so a lost upload
    /// degrades to "full output unavailable" — it must never block or fail
    /// the run. Offline/edge-less engines skip silently.
    pub fn upload_tool_sidecar(&self, chat_id: &str, payload: zeron_doc::SidecarPayload) {
        let Some(edge) = self.inner.config.edge.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return; // bare sync callers (unit tests) skip rather than panic
        };
        let http = self.inner.http.clone();
        let base = format!(
            "{}/blob/{}/{}",
            edge.url.trim_end_matches('/'),
            chat_id,
            encode_part_segment(&payload.part_id)
        );
        self.spawn_worker_on(&runtime, async move {
            let Ok(bearer) = edge.bearer().await else {
                return; // token unavailable; serve the local summary
            };
            let mut puts: Vec<(String, &'static str, Vec<u8>)> = Vec::new();
            if let Some(output) = &payload.output {
                puts.push((
                    base.clone(),
                    "text/plain; charset=utf-8",
                    output.clone().into_bytes(),
                ));
            }
            if let Some(diff) = &payload.diff
                && let Ok(json) = serde_json::to_vec(diff)
            {
                puts.push((format!("{base}.diff"), "application/json", json));
            }
            for (url, content_type, body) in puts {
                let sent = http
                    .put(&url)
                    .bearer_auth(&bearer)
                    .header("content-type", content_type)
                    .body(body)
                    .send()
                    .await;
                match sent {
                    Ok(res) if res.status().is_success() => {}
                    Ok(res) => tracing::warn!(url, status = res.status().as_u16(),
                        "tool sidecar upload rejected"),
                    Err(err) => {
                        let err = describe_http_error(err);
                        tracing::warn!(url, error = %err, "tool sidecar upload failed (best-effort)")
                    }
                }
            }
        });
    }

    /// Fetch a sidecar blob by its doc-resident ref (`{chatId}/{partId}` or
    /// `…​.diff`) — the UI's lazy "Show full output" path, served over RPC
    /// because the UI crate has no HTTP client or edge bearer.
    pub async fn fetch_tool_blob(&self, blob_ref: &str) -> Result<String, EngineError> {
        // Same shape `apply_sidecar_refs` writes; anything else is a forged ref.
        let valid = blob_ref.split_once('/').is_some_and(|(chat, part)| {
            !chat.is_empty()
                && chat.len() <= 128
                && chat
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && !part.is_empty()
                && part.len() <= 200
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:#~-".contains(&b))
        });
        if !valid {
            return Err(EngineError::Other(format!("bad blob ref: {blob_ref}")));
        }
        let Some(edge) = self.inner.config.edge.clone() else {
            return Err(EngineError::Other("offline: no edge configured".into()));
        };
        let bearer = edge.bearer().await?;
        // `valid` above guarantees the split; re-split to encode the part
        // segment for transport (PART_RE allows `#`, which a raw URL would
        // truncate as a fragment — the 2026-08-10 silent-collision bug).
        let (chat, part) = blob_ref.split_once('/').expect("validated above");
        let url = format!(
            "{}/blob/{}/{}",
            edge.url.trim_end_matches('/'),
            chat,
            encode_part_segment(part)
        );
        let res = self
            .inner
            .http
            .get(&url)
            .bearer_auth(&bearer)
            .send()
            .await
            .map_err(|e| {
                EngineError::Other(format!("sidecar fetch failed: {}", describe_http_error(e)))
            })?;
        if !res.status().is_success() {
            return Err(EngineError::Other(format!(
                "sidecar fetch: HTTP {}",
                res.status().as_u16()
            )));
        }
        res.text().await.map_err(|e| {
            EngineError::Other(format!(
                "sidecar body read failed: {}",
                describe_http_error(e)
            ))
        })
    }

    /// §2.2 writer discipline: we host a chat iff its workspace row's `deviceId` is
    /// ours; a chat with no row is claimable (claim-on-first-command). Without a
    /// wired workspace host (bare-DocHost tests) every open chat is ours — M2's
    /// behavior, now the degenerate case.
    fn is_host(&self, chat_id: &str) -> bool {
        self.workspace().is_none_or(|ws| ws.is_host(chat_id))
    }

    /// Chat-config harness when the workspace row carries one, else the default.
    pub(crate) fn harness_for(&self, chat_id: &str) -> HarnessId {
        self.workspace()
            .and_then(|ws| ws.chat_config(chat_id))
            .map(|config| config.harness)
            .unwrap_or(self.inner.config.default_harness)
    }

    /// The harness a request dispatches on: the request's own pick when it
    /// carries one (rides the command plane, immune to registry-row races),
    /// else [`Self::harness_for`].
    pub(crate) fn harness_for_request(
        &self,
        chat_id: &str,
        request: &zeron_proto::RunRequest,
    ) -> HarnessId {
        request.harness.unwrap_or_else(|| self.harness_for(chat_id))
    }

    /// Drain pending commands (host-only): evaluate → mark processed BEFORE execute →
    /// execute → write the outcome as the sole outcome writer.
    pub async fn drain_commands(&self, handle: &Arc<ChatDocHandle>) -> DrainOutcome {
        self.drain_command_kind(handle, false).await
    }

    async fn drain_command_kind(
        &self,
        handle: &Arc<ChatDocHandle>,
        controls_only: bool,
    ) -> DrainOutcome {
        // A quiesced engine still serves controls (interrupt, answers): only
        // new work waits for the machine to wake.
        let _dispatch = if controls_only {
            None
        } else {
            match self.dispatch_guard() {
                Some(guard) => Some(guard),
                None => return DrainOutcome::INCOMPLETE, // quiesced for a Cloud sleep; lifting it re-drains
            }
        };
        let Some(sessions) = self.sessions() else {
            return DrainOutcome::INCOMPLETE; // executor not wired yet (or retired); the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return DrainOutcome::INCOMPLETE;
        }
        // Do not let another drain overtake a prompt waiting for mailbox
        // capacity (or preflight). Controls bypass this lock so a stalled
        // provider can still be interrupted or have its question answered.
        let mut prompt_guard = if controls_only {
            None
        } else {
            handle.command_drain_lock.try_lock().ok()
        };
        // Entries this pass decided to leave alone (processed dedupe hits).
        let mut skipped: HashSet<String> = HashSet::new();
        // Ledger ids already seen processed. A processed command never becomes
        // unprocessed, so a positive is remembered for the rest of the pass
        // instead of costing a SQLite read per command per loop. Negatives are
        // never cached: they flip when this or another drain claims the id.
        let processed_seen: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
        let is_processed = |id: &str| {
            if lock(&processed_seen).contains(id) {
                return true;
            }
            let hit = self.inner.store.is_processed(id).unwrap_or(false);
            if hit {
                lock(&processed_seen).insert(id.to_string());
            }
            hit
        };
        loop {
            let commands = match handle.commands() {
                Ok(commands) => commands,
                Err(err) => {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "command read failed");
                    return DrainOutcome::INCOMPLETE;
                }
            };
            // Dead-command sweep: Pending in the doc, consumed by the ledger,
            // and NOT mid-execution in this process — the crash window
            // between mark-processed and the outcome write. Left alone it is
            // a send no drain or retry can ever reach ("Sending…" forever,
            // 2026-08-19); terminalize it so the truth lands in the doc and
            // a user retry can mint a fresh attempt. `commands` may predate a
            // racing drain that has since resolved the command and released
            // its claim (resolve comes first), so once no claim is seen the
            // status is read again rather than overwriting that outcome.
            let still_pending = |id: &str| {
                handle.commands().is_ok_and(|now| {
                    now.iter()
                        .any(|c| c.id == id && c.status == SessionCommandStatus::Pending)
                })
            };
            for c in commands.iter() {
                if c.status == SessionCommandStatus::Pending
                    && !skipped.contains(&c.id)
                    && is_processed(&c.id)
                    && !lock(&self.inner.executing).contains(&c.id)
                    && still_pending(&c.id)
                {
                    tracing::warn!(chat = %handle.chat_id, command = %c.id,
                        "command consumed but never resolved (crash mid-execute?); rejecting");
                    self.resolve_command(
                        handle,
                        &c.id,
                        SessionCommandStatus::Rejected,
                        Some("interrupted before completion — retry to send again"),
                    );
                    skipped.insert(c.id.clone());
                }
            }
            let Some(entry) = commands
                .iter()
                .find(|c| {
                    c.status == SessionCommandStatus::Pending
                        && !skipped.contains(&c.id)
                        && !is_processed(&c.id)
                        && (prompt_guard.is_some()
                            || !matches!(
                                c.payload,
                                SessionCommandPayload::Run { .. }
                                    | SessionCommandPayload::Steer { .. }
                            ))
                })
                .cloned()
            else {
                if prompt_guard.is_none() && !controls_only {
                    // Wait after handling controls, then re-read: simply
                    // returning here could miss a newly appended prompt.
                    prompt_guard = Some(handle.command_drain_lock.lock().await);
                    continue;
                }
                return DrainOutcome::COMPLETE;
            };
            let messages = handle.doc.read_entries().unwrap_or_default();
            let current_turn_id = messages.last().map(|m| m.id.clone());
            let turn_is_past = |turn_id: &str| messages.iter().any(|m| m.id == turn_id);
            let disposition = evaluate_command(
                &entry,
                &EvaluationContext {
                    is_processed: &is_processed,
                    now_ms: now_ms(),
                    entries: &commands,
                    current_turn_id: current_turn_id.as_deref(),
                    turn_is_past: &turn_is_past,
                },
            );
            // Queued-attachment gate (BEFORE the processed mark — a deferred
            // command must stay eligible): a Run/Steer naming `pending://`
            // refs whose bytes haven't landed on this device yet waits for
            // the transfer instead of running without its images. The wait is
            // bounded; past it the command fails loudly.
            if matches!(disposition, CommandDisposition::Execute) {
                let missing = self.missing_attachments(&entry);
                if !missing.is_empty() {
                    if now_ms().saturating_sub(entry.issued_at) < ATTACHMENT_WAIT_MAX_MS {
                        tracing::info!(chat = %handle.chat_id, command = %entry.id,
                            missing = missing.len(), "command deferred: attachment bytes in transit");
                        self.arm_attachment_wait(handle);
                        return DrainOutcome::INCOMPLETE; // preserve order; UploadCommit / the wait timer re-kick
                    }
                    if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                        tracing::error!(chat = %handle.chat_id, error = %err,
                            "processed-ledger write failed; halting drain");
                        return DrainOutcome::INCOMPLETE;
                    }
                    tracing::warn!(chat = %handle.chat_id, command = %entry.id,
                        "command rejected: attachments never arrived");
                    self.resolve_command(
                        handle,
                        &entry.id,
                        SessionCommandStatus::Rejected,
                        Some("attachments never arrived"),
                    );
                    continue;
                }
            }
            // In-flight claim: guards the dead-command sweep (an id in
            // `executing` is alive, not crashed) and serializes racing
            // drains on the same entry.
            if !lock(&self.inner.executing).insert(entry.id.clone()) {
                skipped.insert(entry.id.clone());
                continue;
            }
            // Mark BEFORE executing: a crash mid-execution must never double-run a
            // command whose side effect may already have happened.
            if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                tracing::error!(chat = %handle.chat_id, error = %err, "processed-ledger write failed; halting drain");
                lock(&self.inner.executing).remove(&entry.id);
                return DrainOutcome::INCOMPLETE;
            }
            match disposition {
                CommandDisposition::Skip => {
                    skipped.insert(entry.id.clone());
                }
                CommandDisposition::Expired => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Expired, None);
                }
                CommandDisposition::Superseded => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Superseded, None);
                }
                CommandDisposition::Execute => {
                    if matches!(
                        entry.payload,
                        SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                    ) {
                        self.await_earlier_controls(handle, &entry.id).await;
                    }
                    let (status, resolution) = match self.execute(&sessions, handle, &entry).await {
                        Ok(outcome) => outcome,
                        Err(err) => (SessionCommandStatus::Rejected, Some(err.to_string())),
                    };
                    self.resolve_command(handle, &entry.id, status, resolution.as_deref());
                }
            }
            lock(&self.inner.executing).remove(&entry.id);
        }
    }

    /// The command's `pending://` attachment refs whose bytes are NOT on this
    /// device's disk yet. Empty when the command names none, when everything
    /// has landed, or when no uploads store is wired (tests) — absence of the
    /// subsystem must never wedge a queue.
    fn missing_attachments(&self, entry: &SessionCommandEntry) -> Vec<String> {
        let refs: Vec<String> = match &entry.payload {
            SessionCommandPayload::Run { request, .. } => request
                .attachments
                .iter()
                .filter(|p| crate::uploads::is_pending_ref(p))
                .cloned()
                .collect(),
            SessionCommandPayload::Steer { prompt, .. } => crate::uploads::pending_refs_in(prompt),
            _ => Vec::new(),
        };
        if refs.is_empty() {
            return refs;
        }
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// Arm (once per chat) the deferred-command re-check loop: while a
    /// pending unprocessed command still waits on attachment bytes, re-drain
    /// on a cadence so the bounded wait actually expires even if every
    /// event-driven kick was missed.
    fn arm_attachment_wait(&self, handle: &Arc<ChatDocHandle>) {
        let chat = handle.chat_id.clone();
        if !lock(&self.inner.drain_waiting).insert(chat.clone()) {
            return;
        }
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        self.spawn_worker(async move {
            loop {
                tokio::time::sleep(ATTACHMENT_WAIT_RECHECK).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
            }
            lock(&host.inner.drain_waiting).remove(&chat);
        });
    }

    /// True while some pending, unprocessed command still waits on bytes.
    fn awaiting_attachments(&self, handle: &Arc<ChatDocHandle>) -> bool {
        let commands = handle.commands().unwrap_or_default();
        let command_waiting = commands.iter().any(|c| {
            c.status == SessionCommandStatus::Pending
                && !self.inner.store.is_processed(&c.id).unwrap_or(false)
                && !self.missing_attachments(c).is_empty()
        });
        command_waiting
            || handle
                .doc
                .read_queue()
                .ok()
                .and_then(|q| q.into_iter().next())
                .is_some_and(|head| !self.missing_row_attachments(&head).is_empty())
    }

    /// Rewrite a request's landed `pending://` refs to this device's absolute
    /// paths — in the attachments list AND the prompt text — so the harness
    /// (and the persisted user entry) see ordinary local files, exactly like
    /// the legacy pre-upload flow produced.
    fn resolve_request_attachments(&self, request: &mut zeron_proto::RunRequest) {
        self.resolve_attachment_refs(&mut request.prompt, &mut request.attachments);
    }

    /// Rewrite landed `pending://` refs in `attachments` (and wherever the
    /// prompt names them) to this device's absolute paths.
    fn resolve_attachment_refs(&self, prompt: &mut String, attachments: &mut [String]) {
        let Some(uploads) = self.inner.uploads.get() else {
            return;
        };
        for path in attachments.iter_mut() {
            if let Some(abs) = uploads.resolve_pending(path) {
                *prompt = prompt.replace(path.as_str(), &abs);
                *path = abs;
            }
        }
    }

    /// A queue row's `pending://` refs whose bytes are not on this device
    /// yet ([`Self::missing_attachments`] for rows). Covers refs in the
    /// row's text too: older clients inlined the attachment trailer.
    fn missing_row_attachments(&self, item: &QueuedMessage) -> Vec<String> {
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        let mut refs: Vec<String> = item
            .attachments
            .iter()
            .filter(|p| crate::uploads::is_pending_ref(p))
            .cloned()
            .collect();
        for r in crate::uploads::pending_refs_in(&item.text) {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// [`Self::resolve_request_attachments`] for a bare prompt (Steer).
    fn resolve_prompt_attachments(&self, prompt: &str) -> String {
        let Some(uploads) = self.inner.uploads.get() else {
            return prompt.to_string();
        };
        let mut out = prompt.to_string();
        for r in crate::uploads::pending_refs_in(prompt) {
            if let Some(abs) = uploads.resolve_pending(&r) {
                out = out.replace(&r, &abs);
            }
        }
        out
    }

    /// Host-only outcome write (ledger rule 2).
    fn resolve_command(
        &self,
        handle: &ChatDocHandle,
        command_id: &str,
        status: SessionCommandStatus,
        resolution: Option<&str>,
    ) {
        if let Err(err) = handle
            .doc
            .set_command_status(command_id, status, resolution)
        {
            tracing::warn!(
                chat = %handle.chat_id,
                command = %command_id,
                error = %err,
                "command outcome write failed"
            );
        }
    }

    /// `entry` is a prompt sent before the Stop that has already acted on
    /// this chat: the Stop cancelled it. Returns its message id.
    fn cancelled_by_stop(
        &self,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
    ) -> Option<String> {
        let message_id = match &entry.payload {
            SessionCommandPayload::Run { message_id, .. } => message_id.clone(),
            SessionCommandPayload::Steer { message_id, .. } => {
                message_id.clone().unwrap_or_else(|| entry.id.clone())
            }
            _ => return None,
        };
        let stop = lock(&handle.stopped_by).clone()?;
        let commands = handle.commands().ok()?;
        let at = commands.iter().position(|c| c.id == entry.id)?;
        let stop_at = commands.iter().position(|c| c.id == stop)?;
        (at < stop_at).then_some(message_id)
    }

    async fn execute(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        if let Some(message_id) = self.cancelled_by_stop(handle, entry) {
            // Still the user's message: it stays in the transcript, unrun.
            let prompt = match &entry.payload {
                SessionCommandPayload::Run { request, .. } => request.prompt.clone(),
                SessionCommandPayload::Steer { prompt, .. } => prompt.clone(),
                _ => String::new(),
            };
            if let Err(err) =
                handle.write_user_message(&message_id, &prompt, entry.issued_at.min(now_ms()))
            {
                tracing::warn!(chat = %chat_id, error = %err, "cancelled message write failed");
            }
            tracing::info!(chat = %chat_id, "prompt sent before a Stop that has acted: cancelled");
            return Ok((
                SessionCommandStatus::Applied,
                Some("cancelled by the Stop sent after it".into()),
            ));
        }
        match &entry.payload {
            SessionCommandPayload::Run {
                request,
                message_id,
            } => {
                let mut request = request.clone();
                // Queued-attachment refs (`pending://`) resolve to this
                // host's absolute paths before anything persists or
                // dispatches — the drain already gated on the bytes being
                // present, so every ref resolves here.
                self.resolve_request_attachments(&mut request);
                // Worktree directive (WorktreeSpec): materialize on THIS host at
                // drain time — the durable command plane replaces the sender's
                // old blocking CreateWorktree relay RPC, whose lost reply wedged
                // the composer on "Sending…" while the run proceeded anyway.
                // `take()` resolves the request before dispatch, so the journal
                // and steer→new-turn fallbacks reuse the created path instead of
                // minting another checkout.
                let worktree_spec = request.worktree.take();
                let fresh_worktree = match &worktree_spec {
                    Some(spec) => {
                        let (cwd, fresh) = self.materialize_worktree(chat_id, &spec).await?;
                        request.cwd = cwd;
                        fresh
                    }
                    None => None,
                };
                // Claim-on-first-command: a run for a chat with no workspace row
                // creates the row under our device id (we are about to host it).
                if let Some(ws) = self.workspace() {
                    ws.claim_chat(chat_id, Some(&request.cwd))?;
                    // Older clients send `~` for a projectless run; a chat
                    // created with a scratch folder runs there instead.
                    if zeron_proto::projectless::is_home_cwd(&request.cwd)
                        && let Ok(Some(chat)) = ws.chat(chat_id)
                        && let Some(cwd) = chat.cwd.filter(|cwd| {
                            chat.space_id.is_none() && zeron_proto::projectless::is_scratch_cwd(cwd)
                        })
                    {
                        request.cwd = cwd;
                    }
                    // A pre-existing row (the client's createChat raced ahead)
                    // still carries the repo folder — repoint it at the fresh
                    // worktree, and stamp the actual `zeron/<name>` branch so
                    // the footer and the title-rename flow see it.
                    if let Some(wt) = &fresh_worktree {
                        if let Err(err) = ws.set_chat_cwd(chat_id, &wt.path) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree cwd stamp failed");
                        }
                        if let Err(err) = ws.set_chat_branch(chat_id, &wt.branch) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree branch stamp failed");
                        }
                    }
                }
                if let Some(spec) = worktree_spec.as_ref()
                    && spec.space_id.is_some()
                {
                    self.complete_worktree_setup_handoff(
                        &entry.id,
                        chat_id,
                        spec,
                        fresh_worktree.as_ref(),
                    );
                }
                let harness = self.harness_for_request(chat_id, &request);
                // A row with no config renders no harness glyph (and every
                // later dispatch falls back to the engine default), so stamp
                // what this run actually executes with. Claimed rows and
                // catalog-not-loaded createChats both land here; the racing
                // real createChat carries the same picked values.
                if let Some(ws) = self.workspace()
                    && ws.chat_config(chat_id).is_none()
                {
                    let config = zeron_proto::ChatConfig {
                        harness,
                        model: request.model.clone(),
                        reasoning: request.reasoning,
                        model_options: request.model_options.clone(),
                        sandbox: request.sandbox,
                        access: request.access,
                        plan: request.plan,
                    };
                    if let Err(err) = ws.set_chat_config(chat_id, &config) {
                        tracing::warn!(chat = %chat_id, error = %err, "run-config backfill failed");
                    }
                }
                // A run on another provider than the live turn's (a stale
                // client, another device's switch) waits for the turn: the
                // provider never changes by killing a turn mid-flight.
                let other_provider = sessions
                    .live_harness(chat_id)
                    .is_some_and(|live| live != harness);
                if other_provider || sessions.defers_to_turn_end(chat_id, Some((harness, &request)))
                {
                    self.hold_until_turn_end(
                        handle,
                        message_id,
                        &request.prompt,
                        entry.issued_at,
                        false,
                        Some(&request),
                    )?;
                    return Ok((
                        SessionCommandStatus::Applied,
                        Some("held until the turn ends".into()),
                    ));
                }
                // Timestamp canonicalization: the user message lands in
                // history at the moment the user SENT it (the entry's
                // issued_at, clamped against clock skew) — not whenever this
                // host got around to draining a queued command. Idempotent by
                // id, so the dispatch path's own execution-time write dedupes
                // to a no-op. A provider switch's seam lands with it, above.
                sessions.note_provider_switch(chat_id, harness, handle);
                if let Err(err) = handle.write_user_message(
                    message_id,
                    &request.prompt,
                    entry.issued_at.min(now_ms()),
                ) {
                    tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
                }
                self.dispatch_with_source_context(
                    sessions,
                    chat_id,
                    harness,
                    request,
                    Some(message_id.clone()),
                )
                .await?;
                // A fresh user-authored turn is the deliberate action that
                // thaws a queue frozen by Cancel. Clear only after dispatch
                // succeeds so a failed send cannot silently unfreeze it.
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::Steer { prompt, message_id } => {
                self.deliver_prompt(
                    sessions,
                    handle,
                    prompt,
                    message_id.clone(),
                    entry.issued_at,
                )
                .await
            }
            SessionCommandPayload::Interrupt {} => {
                self.await_earlier_prompts(sessions, handle, &entry.id)
                    .await;
                *lock(&handle.stopped_by) = Some(entry.id.clone());
                // Sends before this Stop that are still on their way (a slow
                // dispatch the wait above gave up on) are cancelled with it.
                if let Ok(commands) = handle.commands()
                    && let Some(at) = commands.iter().position(|c| c.id == entry.id)
                {
                    let earlier = commands[..at]
                        .iter()
                        .filter(|c| c.status == SessionCommandStatus::Pending)
                        .filter_map(|c| match &c.payload {
                            SessionCommandPayload::Run { message_id, .. } => {
                                Some(message_id.clone())
                            }
                            SessionCommandPayload::Steer { message_id, .. } => {
                                Some(message_id.clone().unwrap_or_else(|| c.id.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    sessions.cancel_sent_messages(&handle.chat_id, earlier);
                }
                self.interrupt_and_pause_queue(sessions, handle).await?;
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::RespondInput {
                request_id,
                answers,
            } => {
                if sessions.respond_input(chat_id, request_id, answers.clone())? {
                    return Ok((SessionCommandStatus::Applied, None));
                }
                // No live resolver. Only a request id the doc shows as an
                // OPEN question on a SETTLED entry gets the orphan fallback:
                // a mismatched or already-resolved id is a stale/buggy answer
                // and must still reject, and a still-streaming entry's
                // question belongs to the live run (a just-consumed resolver
                // racing a second answer must not spawn a duplicate turn).
                let questions = handle.doc.read_entries().ok().and_then(|entries| {
                    entries
                        .iter()
                        .rev()
                        .filter(|e| e.status != Some(MessageStatus::Streaming))
                        .find_map(|e| {
                            e.parts.iter().find_map(|p| match p {
                                MessagePart::Input {
                                    request_id: rid,
                                    questions,
                                    resolved: false,
                                    ..
                                } if rid == request_id => Some(questions.clone()),
                                _ => None,
                            })
                        })
                });
                let Some(questions) = questions else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request".into()),
                    ));
                };
                // Closed without an answer: there is nothing to deliver, and
                // a resumed turn would only say so. Record it and stop.
                if answers.is_empty() {
                    handle.doc.resolve_input(request_id, answers)?;
                    return Ok((
                        SessionCommandStatus::Applied,
                        Some("closed unanswered".into()),
                    ));
                }
                // The run died under the question (engine restart, crash).
                // The question is still open in the doc and the command is
                // durable, so honor it anyway — stamp the part resolved and
                // deliver the answers as the next (resumed) turn, the same
                // fallback a dead-run steer takes. The question UI stays up
                // until the user answers (user requirement); this is what
                // makes that answer still WORK.
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, ""));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request and no prior run config".into()),
                    ));
                };
                request.prompt = respond_input_prompt(&questions, answers);
                request.resume = None; // dispatch re-derives the harness session
                request.attachments = Vec::new();
                if let Err(err) = handle.doc.resolve_input(request_id, answers) {
                    tracing::warn!(chat = %chat_id, request = %request_id, error = %err,
                        "orphaned input resolve failed");
                }
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, None)
                    .await?;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("answered as new turn".into()),
                ))
            }
        }
    }

    /// Park a prompt for a turn-boundary agent in the visible queue instead
    /// of its mailbox, keeping the message id so the transcript entry written
    /// at delivery is the same message. Steers lead ordinary rows.
    fn hold_until_turn_end(
        &self,
        handle: &Arc<ChatDocHandle>,
        message_id: &str,
        prompt: &str,
        issued_at: i64,
        steer: bool,
        request: Option<&zeron_proto::RunRequest>,
    ) -> Result<(), EngineError> {
        let item = QueuedMessage {
            id: message_id.to_string(),
            text: prompt.to_string(),
            attachments: request.map(|r| r.attachments.clone()).unwrap_or_default(),
            hold_for_turn_end: false,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: issued_at.min(now_ms()),
            edited_at: None,
            delivery_gate: None,
        };
        if handle.doc.read_queue()?.iter().any(|row| row.id == item.id) {
            return Ok(()); // a redelivered command: already held
        }
        if let Some(request) = request {
            lock(&handle.held_sends).insert(item.id.clone(), request.clone());
        }
        if steer {
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
        } else {
            handle.doc.push_queued(&item)?;
        }
        // Sending is the deliberate action that thaws a queue frozen by Cancel.
        handle.queue_paused.store(false, Ordering::Release);
        handle.publish_queue();
        Ok(())
    }

    /// Put a typed prompt in front of a live agent: steer it in, or — with no
    /// live steerable run — deliver the durable command as the next turn.
    /// After an engine restart `last_request` is empty too, so rebuild the run
    /// config from the chat's workspace row (zeron derived dispatch config from
    /// the chat row the same way — sessions.ts:601-620); dispatch's engine-owned
    /// resume then reattaches the prior harness conversation.
    ///
    /// Turn-boundary drivers retain explicit steers until their next boundary.
    async fn deliver_prompt(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        prompt: &str,
        message_id: Option<String>,
        issued_at: i64,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        // Explicit steering uses the run mailbox when the agent reads it
        // mid-turn. A turn-boundary agent would read it only after the turn,
        // so it waits in the queue, ahead of ordinary rows, and reaches the
        // transcript when it is actually delivered. Never interrupt to hurry
        // steering.
        let prompt = self.resolve_prompt_attachments(prompt);
        // A live turn without a mailbox can't take it either: the fresh
        // dispatch below would interrupt it.
        let unsteerable_turn =
            sessions.turn_in_flight(chat_id) && !sessions.live_run_steerable(chat_id);
        if !prompt.trim().is_empty()
            && (unsteerable_turn || sessions.defers_to_turn_end(chat_id, None))
        {
            let id = message_id.unwrap_or_else(new_id);
            self.hold_until_turn_end(handle, &id, &prompt, issued_at, true, None)?;
            return Ok((
                SessionCommandStatus::Applied,
                Some("held until the turn ends".into()),
            ));
        }
        // The transcript shows the send while mailbox backpressure holds it.
        // A pending agent update instead holds it in the queue below, where a
        // transcript copy would duplicate it until the update finishes.
        if !sessions.live_run_update_pending(chat_id)
            && let Some(message_id) = message_id.as_deref()
            && let Err(err) =
                handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))
        {
            tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
        }
        match sessions
            .steer_at(chat_id, &prompt, message_id.clone(), issued_at)
            .await?
        {
            SteerOutcome::Accepted => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SteerOutcome::NotSteerable => {
                if let Some(message_id) = message_id.as_deref() {
                    handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))?;
                }
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, &prompt));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no live run and no prior run config".into()),
                    ));
                };
                request.prompt = prompt;
                request.resume = None; // dispatch re-derives the harness session
                // A reused config must not re-inline the PREVIOUS turn's
                // images; this prompt's own refs (if any) ride its text.
                request.attachments = Vec::new();
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, message_id)
                    .await?;
                handle.queue_paused.store(false, Ordering::Release);
                Ok((
                    SessionCommandStatus::Applied,
                    Some("queued as new turn".into()),
                ))
            }
            SteerOutcome::DeferredByUpdate => {
                let id = message_id.unwrap_or_else(new_id);
                self.hold_until_turn_end(handle, &id, &prompt, issued_at, true, None)?;
                // The completed turn's status publication normally re-drains
                // this queue. Also cover completion racing the enqueue itself.
                self.drain_queue(handle).await;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("held until the agent update finishes".into()),
                ))
            }
        }
    }

    async fn capture_source_context(&self, cwd: &str) -> Option<ConversationSourceContext> {
        let repos = self.inner.repos.get()?;
        let path = Path::new(cwd);
        let identity = repos.checkout_identity(path).await.ok()?;
        let branch = repos.current_branch(path).await.ok()?;
        let head_sha = repos.head_sha(path).await.ok().flatten();
        Some(ConversationSourceContext {
            checkout_id: identity.id,
            repo_root: identity.root.to_string_lossy().into_owned(),
            cwd: cwd.to_string(),
            branch,
            head_sha,
            observed_at: chrono::Utc::now(),
        })
    }

    /// Every fresh harness dispatch crosses this boundary, including
    /// steer/input fallbacks and crash recovery. Capture immediately before
    /// dispatch so the conversation records the checkout the harness will
    /// actually observe, rather than whichever checkout state was current on
    /// an earlier turn.
    pub(crate) async fn dispatch_with_source_context(
        &self,
        sessions: &SessionsEngine,
        chat_id: &str,
        harness: HarnessId,
        request: zeron_proto::RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        if let Some(workspace) = self.workspace()
            && let Some(context) = self.capture_source_context(&request.cwd).await
            && let Err(err) = workspace.set_chat_source_context(chat_id, &context)
        {
            tracing::warn!(chat = %chat_id, error = %err, "conversation source stamp failed");
        }
        sessions
            .dispatch(chat_id, harness, request, message_id)
            .await
    }

    async fn materialize_worktree(
        &self,
        chat_id: &str,
        spec: &zeron_proto::WorktreeSpec,
    ) -> Result<(String, Option<zeron_proto::Worktree>), EngineError> {
        if let Some(ws) = self.workspace()
            && let Ok(Some(chat)) = ws.chat(chat_id)
            && let Some(cwd) = chat.cwd
            && crate::workspace_host::linked_worktree_root(std::path::Path::new(&cwd)).is_some_and(
                |root| same_file::is_same_file(&root, &spec.repo_path).unwrap_or(false),
            )
        {
            tracing::info!(chat = %chat_id, cwd = %cwd, "worktree spec: reusing the chat's existing worktree");
            return Ok((cwd, None));
        }
        let repos = self
            .inner
            .repos
            .get()
            .ok_or_else(|| EngineError::Other("repos engine not wired".into()))?;
        let worktree = repos
            .create_worktree(std::path::Path::new(&spec.repo_path), &spec.base)
            .await?;
        tracing::info!(
            chat = %chat_id,
            path = %worktree.path,
            branch = %worktree.branch,
            "worktree materialized for run"
        );
        Ok((worktree.path.clone(), Some(worktree)))
    }

    fn complete_worktree_setup_handoff(
        &self,
        command_id: &str,
        chat_id: &str,
        spec: &zeron_proto::WorktreeSpec,
        fresh_worktree: Option<&zeron_proto::Worktree>,
    ) {
        let Some((project_actions, terminals)) = self.inner.project_action_runtime.get() else {
            return;
        };
        let outcome = match (spec.space_id.as_deref(), fresh_worktree) {
            (Some(space_id), Some(worktree)) => self
                .resolve_and_launch_worktree_setup(
                    project_actions,
                    terminals,
                    space_id,
                    spec,
                    worktree,
                )
                .unwrap_or_else(|err| ProjectActionSetupHandoff {
                    setup_action: None,
                    setup_error: Some(err.to_string()),
                }),
            _ => ProjectActionSetupHandoff {
                setup_action: None,
                setup_error: None,
            },
        };
        if let Some(run) = &outcome.setup_action {
            let _ = terminals.associate_chat(&run.terminal.id, chat_id);
        }
        project_actions.complete_setup_handoff(command_id, chat_id, outcome);
    }

    fn resolve_and_launch_worktree_setup(
        &self,
        project_actions: &ProjectActionsStore,
        terminals: &Terminals,
        space_id: &str,
        spec: &zeron_proto::WorktreeSpec,
        worktree: &zeron_proto::Worktree,
    ) -> Result<ProjectActionSetupHandoff, EngineError> {
        let workspace = self
            .workspace()
            .ok_or_else(|| EngineError::Other("workspace host not wired".into()))?;
        let space = workspace
            .space(space_id)?
            .ok_or_else(|| EngineError::Other("Project not found".into()))?;
        if space.device_id != self.inner.config.device_id {
            return Err(EngineError::Other(
                "Project belongs to another device".into(),
            ));
        }
        let project_root = std::fs::canonicalize(&space.path)?;
        let requested_root = std::fs::canonicalize(&spec.repo_path)?;
        if project_root != requested_root {
            return Err(EngineError::Other(
                "Project path does not match worktree repository".into(),
            ));
        }
        // The store keys configuration by the original Space path, which may
        // be a symlink. Keep canonical paths for validation and execution only.
        let setup_action = project_actions
            .setup_action(space_id, std::path::Path::new(&space.path))?
            .map(|action| {
                launch_project_setup_action(
                    terminals,
                    &action,
                    &project_root,
                    std::path::Path::new(&worktree.path),
                    120,
                    32,
                )
            })
            .transpose()?;
        Ok(ProjectActionSetupHandoff {
            setup_action,
            setup_error: None,
        })
    }

    /// A steer-turned-run with no in-process `last_request` (engine restarted
    /// since the last turn): rebuild the run config from the chat's workspace
    /// row — cwd from the row, model/reasoning/options/sandbox from its config
    /// (composer defaults otherwise). `None` without a workspace host or row.
    // (Also the RespondInput dead-run fallback's config source.)
    pub(crate) fn request_from_chat_row(
        &self,
        chat_id: &str,
        prompt: &str,
    ) -> Option<zeron_proto::RunRequest> {
        let workspace = self.workspace()?;
        let chat = match workspace.chat(chat_id) {
            Ok(chat) => chat?,
            Err(err) => {
                tracing::warn!(chat = %chat_id, error = %err, "workspace chat read failed");
                return None;
            }
        };
        let config = chat.config;
        Some(zeron_proto::RunRequest {
            mcp: None,
            prompt: prompt.to_string(),
            harness: config.as_ref().map(|c| c.harness),
            model: config.as_ref().and_then(|c| c.model.clone()),
            reasoning: config.as_ref().and_then(|c| c.reasoning),
            model_options: config
                .as_ref()
                .map(|c| c.model_options.clone())
                .unwrap_or_default(),
            cwd: chat.cwd.unwrap_or_default(),
            sandbox: config
                .as_ref()
                .map(|c| c.sandbox)
                .unwrap_or(zeron_proto::SandboxLevel::WorkspaceWrite),
            auto_approve: false,
            access: config.as_ref().map(|c| c.access).unwrap_or_default(),
            plan: config.as_ref().is_some_and(|c| c.plan),
            attachments: Vec::new(),
            resume: None,
            worktree: None,
            env: Default::default(),
        })
    }

    fn save_snapshot(&self, handle: &ChatDocHandle) {
        if handle.purged.load(Ordering::Acquire) {
            return;
        }
        if handle.retired.load(Ordering::Relaxed) {
            // A chat2 seed replaced this lineage on disk; persisting this
            // handle's fat doc would clobber the thin one. But retired with
            // NO thin lineage on disk (a stuck handle from the born-gen2
            // race) means this doc is its transcript's only copy — skipping
            // the save turned an app quit into total loss (2026-08-11);
            // persist it, and let the adopt path convert it on reopen.
            let thin_on_disk = matches!(
                self.inner.store.snapshot_epoch(&handle.chat_id),
                Ok(epoch) if epoch >= crate::chat2_host::CHAT2_DOC_EPOCH
            );
            if thin_on_disk {
                return;
            }
        }
        if let Some(persistence) = &handle.persistence {
            persistence.flush_sync();
            return;
        }
        match handle.doc.export_snapshot() {
            Ok(bytes) => {
                handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                if let Err(err) = self.inner.store.save_snapshot(&handle.chat_id, &bytes) {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot save failed");
                }
            }
            Err(err) => {
                tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot export failed");
            }
        }
    }

    /// A fork must be durable before publishing its discoverable registry row.
    pub(crate) fn persist_fork(&self, handle: &ChatDocHandle) -> Result<(), EngineError> {
        let bytes = handle.doc.export_snapshot()?;
        self.inner.store.save_snapshot(&handle.chat_id, &bytes)?;
        handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Whether `chat` has outgoing updates not yet acknowledged: still queued
    /// for the outbox writer, or in the outbox. The writer is asked first, so
    /// an update that finishes in between is seen by the store read.
    pub(crate) fn outbox_pending(&self, chat: &str) -> Result<bool, zeron_sync::StoreError> {
        if self.inner.outbox.has_in_flight(chat) {
            return Ok(true);
        }
        self.inner.store.has_pending_chat_updates(chat)
    }

    /// Persist every open doc now (shutdown path; bypasses the debounce).
    pub fn flush_all(&self) {
        // Rows for every commit so far, before any snapshot or shutdown.
        self.inner.outbox.flush();
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            self.save_snapshot(&handle);
        }
    }

    /// Close all account-scoped room memberships before graceful engine
    /// draining. Auth-aware join supervisors will not install a late client.
    pub fn disconnect_edge(&self) {
        self.inner.edge_disconnected.store(true, Ordering::Release);
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            lock(&handle.chat2).take();
            // Retain the durable subscription through agent shutdown cleanup.
        }
    }
}

/// The resumed-turn prompt for answers to a question whose run died: each
/// answer paired with its question text so the reattached conversation reads
/// naturally. Pure.
pub fn respond_input_prompt(
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
) -> String {
    let mut lines = vec!["Answering your earlier question:".to_string()];
    for answer in answers {
        let picked = answer.labels.join(", ");
        let question = questions
            .iter()
            .find(|q| q.id == answer.question_id)
            .map(|q| q.question.trim())
            .filter(|q| !q.is_empty());
        match question {
            Some(question) => lines.push(format!("{question} — {picked}")),
            None => lines.push(picked),
        }
    }
    lines.join("\n")
}

/// Percent-encode one URL path segment of a sidecar part id. PART_RE's
/// alphabet includes `#` and `:` — legal in R2 keys and doc refs, but a raw
/// `#` in a URL is a fragment delimiter (the request would silently hit the
/// truncated key, colliding parts). The Worker decodes before validating.
fn encode_part_segment(part_id: &str) -> String {
    let mut out = String::with_capacity(part_id.len());
    for byte in part_id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod transfer_progress_tests {
    use super::{DocHost, DocHostConfig, TransferProgressGuard};
    use std::sync::Arc;

    fn host() -> (tempfile::TempDir, DocHost) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).expect("store opens"));
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "dev-test".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        (dir, host)
    }

    #[tokio::test]
    async fn delta_only_transcript_is_visible_to_open_and_salvage() {
        let (_dir, host) = host();
        let source = zeron_doc::SessionDoc::init("delta-only").unwrap();
        source.read_queue().unwrap(); // open_local also accesses this unversioned root
        let store = &host.inner.store;
        store
            .save_verified_snapshot_with_cursor(
                "delta-only",
                &source.export_snapshot().unwrap(),
                1,
                2,
            )
            .unwrap();
        let vv = source.doc().oplog_vv();
        source
            .push_message(&zeron_doc::SessionMessageEntry {
                id: "delta-message".into(),
                role: zeron_doc::MessageRole::User,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: "text".into(),
                    text: "only in a delta".into(),
                }],
                created_at: 1,
                device_id: "remote".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        store
            .append_snapshot_delta(
                "delta-only",
                &source.doc().export(loro::ExportMode::updates(&vv)).unwrap(),
                2,
                &zeron_sync::DocsStore::snapshot_roots(source.doc()).unwrap(),
            )
            .unwrap();
        let scan = super::scan_salvage(store, "delta-only", false).unwrap();
        assert!(matches!(scan.kind, super::SalvageScanKind::Healthy));
        let handle = host.open_local("delta-only").unwrap();
        assert_eq!(
            handle.doc.read_entries().unwrap(),
            source.read_entries().unwrap()
        );
        assert_eq!(handle.doc.doc().oplog_vv(), source.doc().oplog_vv());
        assert_eq!(
            handle.doc.doc().get_deep_value(),
            source.doc().get_deep_value()
        );
        host.shutdown_workers().await;
        assert_eq!(
            store
                .load_chat_state("delta-only")
                .unwrap()
                .unwrap()
                .delta_count,
            0
        );
    }

    #[tokio::test]
    async fn whale_snapshot_opens_and_reopens_without_network() {
        let (_dir, host) = host();
        let source = zeron_doc::SessionDoc::init("persisted-whale").unwrap();
        for i in 0..2000 {
            source
                .push_message(&zeron_doc::SessionMessageEntry {
                    id: format!("row-{i}"),
                    role: zeron_doc::MessageRole::User,
                    parts: vec![zeron_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    }],
                    created_at: i,
                    device_id: "remote".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap();
        }
        host.inner
            .store
            .save_snapshot_with_cursor("persisted-whale", &source.export_snapshot().unwrap(), 0, 2)
            .unwrap();
        drop(source);
        let start = std::time::Instant::now();
        let handle = host.open("persisted-whale").unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 2000);
        eprintln!("offline whale cold open: {:?}", start.elapsed());
        drop(rx);
        // An unwatched commit clears the mirror; attach still serves local data.
        handle.publish_messages_if_watched();
        let start = std::time::Instant::now();
        assert_eq!(handle.watch_messages().borrow().entries.len(), 2000);
        eprintln!("offline whale rebuilt mirror: {:?}", start.elapsed());
    }

    /// The incremental publish must equal the full rebuild it replaced: same
    /// joined entries and the same replay baseline, through streaming,
    /// continuations, and a watch that goes away and comes back.
    #[tokio::test]
    async fn incremental_publish_matches_the_full_rebuild() {
        use super::ChatDocHandle;
        use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
        let (_dir, host) = host();
        let handle = host.open("mirror-eq").unwrap();
        let check = |handle: &Arc<ChatDocHandle>| {
            handle.publish_messages_if_watched();
            let full = handle.publish_messages_full().unwrap();
            let got = handle.messages_tx.borrow().clone();
            let got_entries: Vec<_> = got.entries.iter().map(|e| (**e).clone()).collect();
            let want_entries: Vec<_> = full.entries.iter().map(|e| (**e).clone()).collect();
            assert_eq!(got_entries, want_entries);
            assert_eq!(*got.replay_baseline, *full.replay_baseline);
            got
        };
        let rx = handle.watch_messages();
        handle.write_user_message("u1", "hello", 1).unwrap();
        let first = check(&handle);
        assert_eq!(first.entries.len(), 1);

        let writer = zeron_doc::SegmentWriter::begin(handle.doc(), "a1", "dev-test", 2).unwrap();
        let mut state = writer.into_state();
        let mut text = String::new();
        let mut previous = check(&handle);
        for word in ["caf\u{e9} ", "na\u{ef}ve ", "\u{65e5}\u{672c} ", "tail"] {
            text.push_str(word);
            let mut writer = zeron_doc::SegmentWriter::resume(handle.doc(), state.0, state.1);
            writer
                .sync(&[MessagePart::Text {
                    id: "p".into(),
                    text: text.clone(),
                }])
                .unwrap();
            state = writer.into_state();
            let next = check(&handle);
            // Settled entries are shared across ticks, not re-decoded copies.
            assert!(Arc::ptr_eq(&previous.entries[0], &next.entries[0]));
            previous = next;
        }
        handle
            .doc()
            .push_message(&SessionMessageEntry {
                id: "a1-cont".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "c".into(),
                    text: "continued".into(),
                }],
                created_at: 3,
                device_id: "dev-test".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: Some("a1".into()),
                duration_ms: Some(7),
            })
            .unwrap();
        let joined = check(&handle);
        assert_eq!(joined.entries.len(), 2, "continuation joins onto its root");
        assert_eq!(joined.entries[1].parts.len(), 2);

        // Unwatched commits are not observed; the next attach rebuilds.
        drop(rx);
        handle
            .doc()
            .set_message_status("u1", MessageStatus::Aborted)
            .unwrap();
        handle.publish_messages_if_watched();
        let _rx = handle.watch_messages();
        let rewatched = check(&handle);
        assert_eq!(rewatched.entries[0].status, Some(MessageStatus::Aborted));
    }

    #[tokio::test]
    async fn transcript_reattach_reads_changes_before_publish_worker() {
        let (_dir, host) = host();
        let handle = host.open("mirror-reattach").unwrap();
        let rx = handle.watch_messages();
        handle.write_user_message("root", "hello", 1).unwrap();
        handle.publish_messages_if_watched();
        assert_eq!(rx.borrow().entries.len(), 1);
        drop(rx);
        // The observer resets the unwatched mirror, but the async change
        // worker has not run yet to mark the published snapshot dirty.
        handle
            .doc()
            .set_message_status("root", zeron_doc::MessageStatus::Aborted)
            .unwrap();
        let rx = handle.watch_messages();
        assert_eq!(
            rx.borrow().entries[0].status,
            Some(zeron_doc::MessageStatus::Aborted)
        );
        assert_eq!(
            *rx.borrow().entries[0],
            handle.doc().read_entries().unwrap()[0]
        );
    }

    #[tokio::test]
    async fn transcript_attach_and_unwatched_clear_share_a_critical_section() {
        let (_dir, host) = host();
        let handle = host.open("cached").unwrap();
        handle
            .write_user_message("row", "locally persisted transcript", 0)
            .unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 1);
        drop(rx);

        // Freeze attach's critical section. An unwatched publisher must not
        // pass its receiver check and clear the mirror while attach owns it.
        let guard = super::lock(&handle.transcript_import);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_handle = handle.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_handle.publish_messages_if_watched();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        let result = done_rx.recv_timeout(std::time::Duration::from_millis(100));
        // Simulate the subscription attaching before the worker can inspect it.
        let rx = handle.messages_tx.subscribe();
        drop(guard);
        worker.join().unwrap();
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "unwatched clear escaped attach's critical section"
        );
        assert_eq!(rx.borrow().entries.len(), 1, "no empty reset after attach");
        drop(rx);
        handle.publish_messages_if_watched();
        assert!(handle.messages_tx.borrow().entries.is_empty());
        assert_eq!(
            handle.watch_messages().borrow().entries.len(),
            1,
            "offline reopen rebuilds from local content"
        );
    }

    #[test]
    fn set_upserts_by_upload_id_and_clear_retires_only_its_entry() {
        let (_dir, host) = host();
        let rx = host.watch_transfers();
        assert!(rx.borrow().is_empty());

        host.transfer_progress_set("u1", "a.png", 0, 1_000);
        host.transfer_progress_set("u2", "b.png", 0, 400);
        host.transfer_progress_set("u1", "a.png", 300, 1_000);
        let snapshot = rx.borrow().clone();
        assert_eq!(snapshot.len(), 2, "upsert must not duplicate u1");
        let u1 = snapshot.iter().find(|t| t.upload_id == "u1").unwrap();
        assert_eq!((u1.done, u1.total), (300, 1_000));

        host.transfer_progress_clear("u1");
        let snapshot = rx.borrow().clone();
        assert_eq!(snapshot.len(), 1, "u2 must survive u1's retirement");
        assert_eq!(snapshot[0].upload_id, "u2");
    }

    #[test]
    fn guard_retires_the_entry_on_every_exit_path() {
        let (_dir, host) = host();
        let rx = host.watch_transfers();
        host.transfer_progress_set("u1", "a.png", 0, 9);
        {
            let _guard = TransferProgressGuard {
                host: &host,
                upload_id: "u1",
            };
            assert_eq!(rx.borrow().len(), 1);
            // An early return / error propagation drops the guard here.
        }
        assert!(
            rx.borrow().is_empty(),
            "a failed attempt must not leave a phantom ring behind"
        );
    }

    #[test]
    fn late_subscriber_sees_current_set_first() {
        let (_dir, host) = host();
        host.transfer_progress_set("u1", "a.png", 750, 1_000);
        // watch_stream's contract: current value first, then changes — a UI
        // attaching mid-transfer must render the ring immediately.
        let rx = host.watch_transfers();
        assert_eq!(rx.borrow().len(), 1);
        assert_eq!(rx.borrow()[0].done, 750);
    }
}

#[cfg(test)]
mod source_context_tests {
    use super::{DocHost, DocHostConfig};
    use std::process::Command;
    use std::sync::Arc;

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn capture_source_context_reads_the_dispatch_checkout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "feature/captured"]);
        git(&repo, &["config", "user.name", "Zeron Test"]);
        git(&repo, &["config", "user.email", "zeron@example.com"]);
        std::fs::write(repo.join("README.md"), "capture\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "capture"]);

        let store =
            Arc::new(zeron_sync::DocsStore::open(dir.path().join("docs")).expect("store opens"));
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "device-a".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        host.set_repos(crate::repos::Repos::new(
            &dir.path().join("data"),
            "device-a",
        ));

        let context = host
            .capture_source_context(repo.to_str().unwrap())
            .await
            .expect("source context");
        assert_eq!(context.branch, "feature/captured");
        assert_eq!(context.cwd, repo.to_string_lossy());
        assert_eq!(
            context.repo_root,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert!(context.head_sha.is_some());
        assert!(!context.checkout_id.is_empty());
    }
}

#[cfg(test)]
mod degrade_grace_tests {
    use super::{DEGRADE_GRACE, DegradeGrace, GraceKey};
    use std::time::{Duration, Instant};

    #[test]
    fn blips_shorter_than_the_grace_never_report() {
        let mut g = DegradeGrace::default();
        let t0 = Instant::now();
        // A 300ms room join: degraded at t0, healthy again shortly after.
        assert!(!g.degraded(GraceKey::Chat("c1"), true, t0));
        assert!(!g.degraded(GraceKey::Chat("c1"), true, t0 + Duration::from_millis(300)));
        assert!(!g.degraded(GraceKey::Chat("c1"), false, t0 + Duration::from_millis(600)));
        // The recovery cleared the timer — a fresh blip starts from zero.
        assert!(!g.degraded(GraceKey::Chat("c1"), true, t0 + Duration::from_secs(10)));
    }

    #[test]
    fn persistent_degradation_reports_after_the_grace_and_clears_instantly() {
        let mut g = DegradeGrace::default();
        let t0 = Instant::now();
        assert!(!g.degraded(GraceKey::Registry, true, t0));
        assert!(!g.degraded(GraceKey::Registry, true, t0 + DEGRADE_GRACE / 2));
        assert!(g.degraded(GraceKey::Registry, true, t0 + DEGRADE_GRACE));
        assert!(g.degraded(GraceKey::Registry, true, t0 + DEGRADE_GRACE * 3));
        // Hide-fast: one healthy sample reports Connected immediately.
        assert!(!g.degraded(GraceKey::Registry, false, t0 + DEGRADE_GRACE * 4));
    }

    #[test]
    fn sources_are_independent_and_closed_chats_are_dropped() {
        let mut g = DegradeGrace::default();
        let t0 = Instant::now();
        assert!(!g.degraded(GraceKey::Chat("gone"), true, t0));
        assert!(!g.degraded(GraceKey::OsPath, true, t0));
        // The chat's doc closes; its timer must not leak.
        g.retain_chats(|id| id != "gone");
        assert!(g.chats.is_empty());
        // OsPath kept its own timer through the retain.
        assert!(g.degraded(GraceKey::OsPath, true, t0 + DEGRADE_GRACE));
    }

    #[tokio::test]
    async fn dormant_chat_clears_old_degradation_before_reconnection() {
        use super::{DocHost, DocHostConfig, EdgeConfig, lock};
        use std::sync::Arc;
        use std::sync::atomic::Ordering;
        use zeron_proto::{ChatSyncState, HarnessId};

        let dir = tempfile::tempdir().unwrap();
        let host = DocHost::new(
            Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap()),
            DocHostConfig {
                device_id: "local".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        let handle = host.open_local("dormant").unwrap();
        lock(&host.inner.connectivity_grace)
            .chats
            .insert("dormant".into(), Instant::now() - DEGRADE_GRACE * 2);

        let dormant = host.compute_connectivity();
        assert_eq!(dormant.chats[0].sync_state, ChatSyncState::Local);
        assert!(dormant.chats[0].connected);
        assert!(!dormant.chats[0].delivery_live);
        assert!(
            !lock(&host.inner.connectivity_grace)
                .chats
                .contains_key("dormant")
        );

        // Focus requests a connection. Its first down sample gets a new grace
        // period even though the doc was dormant longer than DEGRADE_GRACE.
        handle.sync_requested.store(true, Ordering::Release);
        let waking = host.compute_connectivity();
        assert_eq!(waking.chats[0].sync_state, ChatSyncState::Waiting);
        assert!(waking.chats[0].connected);
        assert!(!waking.chats[0].delivery_live);

        lock(&host.inner.connectivity_grace)
            .chats
            .insert("dormant".into(), Instant::now() - DEGRADE_GRACE);
        assert!(!host.compute_connectivity().chats[0].connected);

        // An active chat with the same sustained outage also reports down.
        handle.sync_started.store(true, Ordering::Release);
        handle.sync_requested.store(false, Ordering::Release);
        let active = host.compute_connectivity();
        assert_eq!(active.chats[0].sync_state, ChatSyncState::Connecting);
        assert!(!active.chats[0].connected);
        assert!(!active.chats[0].delivery_live);
        host.shutdown_workers().await;
    }
}

#[cfg(test)]
mod part_segment_tests {
    use super::encode_part_segment;

    #[test]
    fn hash_and_colon_are_escaped_unreserved_pass_through() {
        assert_eq!(encode_part_segment("m1#c1"), "m1%23c1");
        assert_eq!(encode_part_segment("tool:call_9"), "tool%3Acall_9");
        assert_eq!(encode_part_segment("plain-id_0.diff~"), "plain-id_0.diff~");
    }
}

#[cfg(test)]
mod queued_message_prompt_tests {
    use super::{ATTACHMENT_ONLY_PROMPT, ATTACHMENT_PROMPT_HEADER, queued_message_prompt};

    #[test]
    fn dispatch_adds_the_attachment_transport_to_visible_queue_text() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("inspect this", &paths),
            format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }

    #[test]
    fn legacy_expanded_rows_are_not_expanded_twice() {
        let paths = vec!["/tmp/image.png".to_string()];
        let legacy = format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png");
        assert_eq!(queued_message_prompt(&legacy, &paths), legacy);
    }

    #[test]
    fn attachment_only_rows_get_a_non_empty_prompt_body() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("", &paths),
            format!("{ATTACHMENT_ONLY_PROMPT}\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }
}

/// Per-chat background tasks. A doc change (local commit or remote import)
/// carries a [`zeron_doc::ChangeMask`] of the roots it touched, and each task
/// reacts only to the roots it owns:
///
/// - the **publisher** republishes the transcript watch when `messages`
///   changed and the queue watch when `queue` changed, and never waits on a
///   drain;
/// - the **dispatcher** drains commands when `commands` changed (or its last
///   pass was incomplete), drains the queue, and debounces snapshots;
/// - the **control worker** (below) waits on the ledger generation, not on
///   every doc change.
///
/// A streaming turn commits to `messages` only, so it no longer pays for the
/// ledger. Each holds only a weak handle so a dropped host tears it down.
async fn chat_task(host: DocHost, weak: Weak<ChatDocHandle>, mut changed_rx: watch::Receiver<u64>) {
    host.spawn_worker(sync_maintenance_task(host.clone(), weak.clone()));
    // Prompt delivery may wait for mailbox capacity. Keep a separate watcher
    // for interrupt/question controls so that wait cannot block recovery.
    let control_host = host.clone();
    let control_weak = weak.clone();
    let control_ledger = match weak.upgrade() {
        Some(handle) => handle.commands_tx.subscribe(),
        None => return,
    };
    host.spawn_worker(control_task(
        control_host,
        control_weak,
        control_ledger,
        changed_rx.clone(),
    ));
    // The publisher runs on its own so a drain that waits (mailbox capacity,
    // an attachment, the drain lock) never delays the transcript or the queue.
    host.spawn_worker(publish_task(weak.clone(), changed_rx.clone()));
    // Initial pass: the snapshot may already carry pending commands. The
    // mirror stays lazy — it materializes on the first watch attach.
    let mut last_pass_incomplete;
    {
        let Some(handle) = weak.upgrade() else { return };
        last_pass_incomplete = !host.drain_commands(&handle).await.complete;
        host.drain_queue(&handle).await;
    }
    let mut save_deadline: Option<tokio::time::Instant> = None;
    loop {
        let sleep_until = save_deadline.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            changed = changed_rx.changed() => {
                zeron_counters::add(zeron_counters::C::PublishMessagesWakes, 1);
                if changed.is_err() {
                    break; // doc handle (and its change sender) is gone
                }
                let Some(handle) = weak.upgrade() else { break };
                let mask = handle.dispatch_bits.take();
                // A pass that left work behind (not host yet, quiesced, an
                // attachment in transit) retries on any change, as it did
                // when every change drained; otherwise only a ledger change
                // can have made a command eligible.
                if last_pass_incomplete || mask.contains(zeron_doc::ChangeMask::COMMANDS) {
                    last_pass_incomplete = !host.drain_commands(&handle).await.complete;
                }
                host.drain_queue(&handle).await;
                if save_deadline.is_none() {
                    save_deadline = Some(
                        tokio::time::Instant::now()
                            + std::time::Duration::from_millis(SNAPSHOT_DEBOUNCE_MS),
                    );
                }
            }
            _ = tokio::time::sleep_until(sleep_until), if save_deadline.is_some() => {
                save_deadline = None;
                let Some(handle) = weak.upgrade() else { break };
                // chat2 has its own coalescing blocking-pool persister. The
                // legacy worker must not duplicate every scheduled export.
                if handle.persistence.is_none() { host.save_snapshot(&handle); }
                // chat2 host duties ride the same quiesce tick (C3):
                // threshold checkpoints + the tail sidecar publish.
                handle.writer_wake.notify_one();
                // Post-quiesce eviction pass: sizes just refreshed.
                host.evict_over_budget();
            }
        }
    }
}

async fn sync_maintenance_task(host: DocHost, weak: Weak<ChatDocHandle>) {
    let wake = match weak.upgrade() {
        Some(handle) => handle.writer_wake.clone(),
        None => return,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The pass's own request to run again (throttle trailing edge, failure
    // retry). Idle chats have none, so they hold no timer and never wake.
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let streaming = weak.upgrade().is_some_and(|handle| {
            cadence_streaming(
                handle.writers.load(Ordering::Acquire),
                host.session_active(&handle),
            )
        });
        tokio::select! {
            _ = wake.notified() => {},
            _ = tick.tick(), if streaming => {},
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)),
                if deadline.is_some() => {},
        }
        let Some(handle) = weak.upgrade() else { break };
        let turn_end = handle.turn_end_pending.swap(false, Ordering::AcqRel);
        deadline = host.chat2_maintenance(&handle, turn_end).await;
    }
}

/// Republish what changed: the transcript when `messages` did, the queue when
/// `queue` did. Runs on the blocking pool; never awaits a drain.
async fn publish_task(weak: Weak<ChatDocHandle>, mut changed_rx: watch::Receiver<u64>) {
    use zeron_doc::ChangeMask;
    while changed_rx.changed().await.is_ok() {
        let Some(handle) = weak.upgrade() else { break };
        let mask = handle.publish_bits.take();
        if !mask.intersects(ChangeMask::MESSAGES.union(ChangeMask::QUEUE)) {
            continue;
        }
        let _ = tokio::task::spawn_blocking(move || {
            if mask.contains(ChangeMask::MESSAGES) {
                handle.publish_messages_if_watched();
            }
            if mask.contains(ChangeMask::QUEUE) {
                handle.publish_queue();
            }
        })
        .await;
    }
}

/// Interrupts and answers: drained apart from prompts so a prompt waiting for
/// mailbox capacity cannot block them. Wakes on a ledger change; only a pass
/// that left work behind also retries on other doc changes.
async fn control_task(
    host: DocHost,
    weak: Weak<ChatDocHandle>,
    mut ledger: watch::Receiver<u64>,
    mut changes: watch::Receiver<u64>,
) {
    loop {
        let Some(handle) = weak.upgrade() else { break };
        // Mark everything up to here seen BEFORE the pass: a change during or
        // after it flags the receiver and wakes the wait below.
        ledger.borrow_and_update();
        changes.borrow_and_update();
        let outcome = host.drain_command_kind(&handle, true).await;
        drop(handle);
        let woke = if outcome.complete {
            ledger.changed().await
        } else {
            changes.changed().await
        };
        if woke.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod publication_eviction_tests {
    use super::*;

    #[test]
    fn chat2_checkpoint_regimes() {
        assert!(cadence_streaming(1, Some(true)));
        assert!(!cadence_streaming(1, Some(false)), "parked writer is idle");
        assert!(cadence_streaming(1, None), "writer without session status");
        assert!(!cadence_streaming(0, Some(true)));
        assert!(!checkpoint_due(true, 2000, 4 * 1024 * 1024, false));
        assert!(checkpoint_due(true, 2001, 0, false));
        assert!(checkpoint_due(true, 0, 4 * 1024 * 1024 + 1, false));
        assert!(!checkpoint_due(false, 200, 512 * 1024, false));
        assert!(checkpoint_due(false, 201, 0, false));
        assert!(checkpoint_due(false, 0, 512 * 1024 + 1, false));
        assert!(checkpoint_due(true, 0, 0, true));
        assert!(!checkpoint_due(false, 0, 0, true));
        let now = tokio::time::Instant::now();
        assert!(!checkpoint_stale(Some(now), now + Duration::from_secs(599)));
        assert!(checkpoint_stale(Some(now), now + Duration::from_secs(600)));
    }

    #[tokio::test]
    async fn chat2_remote_segment_completion_wakes_but_is_not_a_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("status-wake").unwrap();
        host.shutdown_workers().await;
        let drained = |handle: &Arc<ChatDocHandle>| {
            let wake = handle.writer_wake.clone();
            async move {
                tokio::time::timeout(Duration::from_millis(100), wake.notified())
                    .await
                    .is_ok()
            }
        };
        let _ = drained(&handle).await;
        let writer = zeron_doc::SegmentWriter::begin(handle.doc(), "a", "remote", 1).unwrap();
        writer.finish(&[], MessageStatus::Complete).unwrap();
        assert!(
            drained(&handle).await,
            "a finished remote segment wakes the pass"
        );
        assert!(
            !handle.turn_end_pending.load(Ordering::Acquire),
            "one segment of a multi-segment remote turn is not a turn end"
        );
        let _active = handle.writer();
        let _ = drained(&handle).await;
        let segment = zeron_doc::SegmentWriter::begin(handle.doc(), "b", "host", 2).unwrap();
        segment.finish(&[], MessageStatus::Complete).unwrap();
        assert!(!drained(&handle).await, "a local writer owns its own wakes");
    }

    #[test]
    fn chat2_only_working_streams() {
        use zeron_proto::SessionStatus as S;
        assert!(session_streams(S::Working));
        assert!(
            !session_streams(S::AwaitingInput),
            "a pending question writes nothing"
        );
        assert!(!session_streams(S::Idle));
        assert!(!session_streams(S::Errored));
    }

    #[test]
    fn chat2_retry_backoff_doubles_to_its_cap() {
        let steps: Vec<u64> = (1..=8)
            .map(|n| backoff(n, TAIL_RETRY_MIN, TAIL_RETRY_MAX).as_secs())
            .collect();
        assert_eq!(steps, [5, 10, 20, 40, 80, 120, 120, 120]);
        assert_eq!(
            backoff(u32::MAX, TAIL_RETRY_MIN, TAIL_RETRY_MAX),
            TAIL_RETRY_MAX
        );
        assert_eq!(
            backoff(0, CHECKPOINT_RETRY_MIN, CHECKPOINT_RETRY_MAX),
            CHECKPOINT_RETRY_MIN
        );
    }

    #[tokio::test]
    async fn chat2_parked_writer_uses_idle_cadence() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("parked-cadence").unwrap();
        let _writer = handle.writer();
        let (tx, rx) = watch::channel(Vec::new());
        host.spawn_sync_status_watcher(rx);
        let mut session = zeron_proto::Session {
            last_turn_outcome: None,
            terminal_activity: None,
            last_completed_turn: None,
            chat_id: handle.chat_id.clone(),
            device_id: handle.device_id.clone(),
            status: zeron_proto::SessionStatus::Working,
            started_at: None,
            updated_at: chrono::Utc::now(),
            running_subagents: 0,
            background_tasks: vec![],
        };
        tx.send_replace(vec![session.clone()]);
        tokio::time::timeout(Duration::from_secs(5), async {
            while lock(&handle.sync_cadence).session_streaming != Some(true) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        session.status = zeron_proto::SessionStatus::Idle;
        tx.send_replace(vec![session]);
        tokio::time::timeout(Duration::from_secs(5), async {
            while lock(&handle.sync_cadence).session_streaming != Some(false) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!cadence_streaming(
            handle.writers.load(Ordering::Acquire),
            lock(&handle.sync_cadence).session_streaming
        ));
        host.shutdown_workers().await;
    }

    #[test]
    fn chat2_ten_minute_cadence_simulation() {
        // One changed flush every 120 ms; maintenance runs every second.
        let (mut old_rows, mut new_rows) = (0u64, 0u64);
        let (mut old_checkpoints, mut new_checkpoints) = (0, 0);
        let mut last_commit = 0;
        let mut last_tail_second = 0;
        let mut new_puts = 0;
        for second in 1..=600 {
            let commits = second * 1000 / 120;
            let rows = commits - last_commit;
            last_commit = commits;
            old_rows += rows;
            new_rows += rows;
            if checkpoint_due(false, old_rows, 0, false) {
                old_checkpoints += 1;
                old_rows = 0;
            }
            if checkpoint_due(true, new_rows, 0, false) {
                new_checkpoints += 1;
                new_rows = 0;
            }
            if new_puts == 0 || second - last_tail_second >= 5 {
                new_puts += 1;
                last_tail_second = second;
            }
        }
        // A changed tail and the lower threshold apply at turn end.
        if last_tail_second < 600 {
            new_puts += 1;
        }
        if checkpoint_due(false, new_rows, 0, false) {
            new_checkpoints += 1;
        }
        eprintln!(
            "10-minute simulation: PUTs 600 -> {new_puts}; checkpoints {old_checkpoints} -> {new_checkpoints}"
        );
        assert_eq!((new_puts, old_checkpoints, new_checkpoints), (121, 24, 3));
    }

    /// A loopback edge that counts tail PUTs. While `healthy` is false it
    /// answers 503, so a turn can end "during an outage".
    async fn tail_edge() -> (
        String,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicBool>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, attempts) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let healthy = Arc::new(AtomicBool::new(true));
        let (received, tried, up) = (accepted.clone(), attempts.clone(), healthy.clone());
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let len: usize = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length: "))
                            .unwrap_or("0")
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + len {
                            let is_tail = headers.starts_with("put /chat2/")
                                && headers.lines().next().is_some_and(|l| l.contains("/tail "));
                            if is_tail {
                                tried.fetch_add(1, Ordering::SeqCst);
                            }
                            if is_tail && up.load(Ordering::SeqCst) {
                                received.fetch_add(1, Ordering::SeqCst);
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            } else {
                                let _ = socket.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            }
                            break;
                        }
                    }
                }
            }
        });
        (url, accepted, attempts, healthy, server)
    }

    fn edge_host(url: &str, dir: &Path) -> DocHost {
        DocHost::new(
            Arc::new(DocsStore::open(dir).unwrap()),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token(url, "test")),
            },
        )
    }

    #[tokio::test]
    async fn chat2_tail_cadence_uses_fake_edge() {
        let _guard = SYNC_TEST_LOCK.lock().await;
        let (url, puts, _, _, server) = tail_edge().await;
        let dir = tempfile::tempdir().unwrap();
        let host = edge_host(&url, dir.path());
        let handle = host.open_local("tail-cadence").unwrap();
        host.shutdown_workers().await;
        handle.write_user_message("one", "first", 1).unwrap();
        assert!(
            host.chat2_tail_maintenance(&handle, true, false)
                .await
                .is_none()
        );
        assert_eq!(puts.load(Ordering::SeqCst), 1);
        handle.write_user_message("two", "second", 2).unwrap();
        let trailing = host.chat2_tail_maintenance(&handle, true, false).await;
        assert_eq!(
            puts.load(Ordering::SeqCst),
            1,
            "throttled inside the 5 s window"
        );
        assert!(
            trailing.is_some(),
            "a throttled change must report its trailing edge"
        );
        lock(&handle.sync_cadence).tail_attempt = Some(tokio::time::Instant::now() - TAIL_INTERVAL);
        assert!(
            host.chat2_tail_maintenance(&handle, true, false)
                .await
                .is_none()
        );
        assert_eq!(
            puts.load(Ordering::SeqCst),
            2,
            "the trailing pass sends the change"
        );
        assert!(
            host.chat2_tail_maintenance(&handle, false, true)
                .await
                .is_none(),
            "an unchanged turn end sends nothing"
        );
        assert_eq!(puts.load(Ordering::SeqCst), 2);
        handle.write_user_message("three", "final", 3).unwrap();
        host.chat2_tail_maintenance(&handle, false, true).await;
        host.chat2_tail_maintenance(&handle, false, true).await;
        assert_eq!(
            puts.load(Ordering::SeqCst),
            3,
            "one changed turn-end PUT, immediately"
        );
        server.abort();
    }

    #[tokio::test]
    async fn chat2_turn_end_during_an_outage_is_pushed_once_the_edge_returns() {
        let _guard = SYNC_TEST_LOCK.lock().await;
        let (url, puts, attempts, healthy, server) = tail_edge().await;
        let dir = tempfile::tempdir().unwrap();
        let host = edge_host(&url, dir.path());
        let handle = host.open_local("tail-outage").unwrap();
        host.shutdown_workers().await;
        handle.write_user_message("one", "final answer", 1).unwrap();
        healthy.store(false, Ordering::SeqCst);
        let retry = host.chat2_tail_maintenance(&handle, false, true).await;
        let retry = retry.expect("a failed turn-end PUT must schedule a retry");
        assert_eq!(
            (attempts.load(Ordering::SeqCst), puts.load(Ordering::SeqCst)),
            (1, 0)
        );
        assert!(lock(&handle.sync_cadence).tail_deferred);
        // Wakes inside the backoff window reach the edge zero more times.
        for _ in 0..5 {
            assert_eq!(
                host.chat2_tail_maintenance(&handle, false, false).await,
                Some(retry)
            );
        }
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "no storm while backing off"
        );
        // The retry fires with the edge still down: the backoff grows.
        lock(&handle.sync_cadence).tail_retry_at = Some(tokio::time::Instant::now());
        lock(&handle.sync_cadence).tail_attempt = None;
        let again = host
            .chat2_tail_maintenance(&handle, false, false)
            .await
            .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(again > retry, "the second backoff is longer than the first");
        // Connectivity returns; the retry deadline passes.
        healthy.store(true, Ordering::SeqCst);
        lock(&handle.sync_cadence).tail_retry_at = Some(tokio::time::Instant::now());
        lock(&handle.sync_cadence).tail_attempt = None;
        assert!(
            host.chat2_tail_maintenance(&handle, false, false)
                .await
                .is_none()
        );
        assert_eq!(
            puts.load(Ordering::SeqCst),
            1,
            "the final tail lands after the outage"
        );
        assert!(!lock(&handle.sync_cadence).tail_deferred);
        assert_eq!(lock(&handle.sync_cadence).tail_failures, 0);
        // Nothing more is owed, and nothing wakes for it.
        assert!(
            host.chat2_tail_maintenance(&handle, false, true)
                .await
                .is_none()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn chat2_idle_floor_reports_its_deadline() {
        // The idle floor reports its deadline (the task test below proves the task
        // honours it); nothing else would wake an idle chat for the trailing edge.
        let _guard = SYNC_TEST_LOCK.lock().await;
        let (url, puts, _, _, server) = tail_edge().await;
        let dir = tempfile::tempdir().unwrap();
        let host = edge_host(&url, dir.path());
        let handle = host.open_local("tail-task").unwrap();
        host.shutdown_workers().await;
        let mut cadence = lock(&handle.sync_cadence);
        cadence.tail_attempt = Some(tokio::time::Instant::now());
        drop(cadence);
        handle.write_user_message("one", "x", 1).unwrap();
        let deadline = host.chat2_tail_maintenance(&handle, false, false).await;
        assert_eq!(puts.load(Ordering::SeqCst), 0);
        let deadline = deadline.expect("idle-floor throttle reports its deadline");
        assert!(deadline <= tokio::time::Instant::now() + TAIL_IDLE_FLOOR);
        tokio::time::sleep_until(deadline).await;
        assert!(
            host.chat2_tail_maintenance(&handle, false, false)
                .await
                .is_none()
        );
        assert_eq!(puts.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn chat2_checkpoint_completion_frees_the_slot_then_wakes_and_failure_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("ckpt-slot").unwrap();
        host.shutdown_workers().await;
        let _ =
            tokio::time::timeout(Duration::from_millis(20), handle.writer_wake.notified()).await;
        // A pass that finds the slot busy defers to the completion wake.
        handle.checkpointing.store(true, Ordering::Release);
        let reset = ResetFlag(handle.checkpointing.clone());
        // No room client: the checkpoint cannot be posted, i.e. it fails.
        host.run_chat2_checkpoint(handle.clone(), "test", reset)
            .await;
        assert!(!handle.checkpointing.load(Ordering::Acquire), "slot freed");
        tokio::time::timeout(Duration::from_millis(100), handle.writer_wake.notified())
            .await
            .expect("completion wakes the maintenance task");
        let cadence = lock(&handle.sync_cadence);
        assert_eq!(cadence.checkpoint_failures, 1);
        assert!(
            cadence
                .checkpoint_retry_at
                .is_some_and(|at| at > tokio::time::Instant::now()),
            "a failed checkpoint backs off rather than retrying on the next wake"
        );
    }

    #[tokio::test]
    async fn wakeup_handoff_waits_for_snapshot_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open_local("receipt").unwrap();
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER no_snapshot BEFORE INSERT ON snapshots BEGIN SELECT RAISE(FAIL,'disk unavailable'); END;").unwrap();
        handle
            .doc
            .doc()
            .get_text("body")
            .insert(0, "received before crash")
            .unwrap();
        handle.doc.doc().commit();
        assert!(
            !host.wakeup_is_durable(&handle),
            "cannot retire receipt while snapshot is only in memory"
        );
        db.execute_batch("DROP TRIGGER no_snapshot;").unwrap();
        assert!(host.wakeup_is_durable(&handle));
        host.shutdown_workers().await;
    }

    /// An edge host whose room is unreachable: commits reach only the outbox.
    fn outbox_host(store: &Arc<DocsStore>) -> DocHost {
        DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        )
    }

    fn commit_text(handle: &ChatDocHandle, text: &str) {
        let body = handle.doc.doc().get_text("body");
        body.insert(body.len_unicode(), text).unwrap();
        handle.doc.doc().commit();
    }

    #[tokio::test]
    async fn streaming_rows_per_commit() {
        let _guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("stream-rows").unwrap();
        let mut writer = zeron_doc::SegmentWriter::begin(handle.doc(), "a", "host", 1).unwrap();
        host.inner.outbox.flush();
        let before = store.pending_chat_updates("stream-rows").unwrap().len();
        let mut text = String::new();
        for _ in 0..100 {
            text.push('x');
            writer
                .sync(&[MessagePart::Text {
                    id: "p".into(),
                    text: text.clone(),
                }])
                .unwrap();
        }
        host.inner.outbox.flush();
        let rows = store.pending_chat_updates("stream-rows").unwrap().len() - before;
        eprintln!("streaming rows per commit: {rows}/100");
        assert_eq!(rows, 100);
        host.shutdown_workers().await;
    }

    /// Outbox rows replay into a fresh doc in ordinal order with no gaps.
    fn replay_in_order(store: &DocsStore, chat: &str) -> (usize, String) {
        let replay = loro::LoroDoc::new();
        let rows = store.pending_chat_updates(chat).unwrap();
        for (_, bytes) in &rows {
            let status = replay.import(bytes).unwrap();
            assert!(
                status.pending.is_none(),
                "row {chat}: depends on a later row"
            );
        }
        (rows.len(), replay.get_text("body").to_string())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commits_never_wait_on_sqlite_and_rows_keep_commit_order_exactly_once() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let chats = ["order-a", "order-b", "order-c"];
        let handles: Vec<_> = chats.iter().map(|c| host.open_local(c).unwrap()).collect();
        let outbox = host.inner.outbox.clone();
        // Opening journals one baseline row per chat.
        let baseline: Vec<usize> = chats
            .iter()
            .map(|c| store.pending_chat_updates(c).unwrap().len())
            .collect();
        let before = store.outbox_write_transactions();
        const COMMITS: usize = 100;
        {
            // The writer cannot write: every commit must still return at once.
            let _pause = outbox.pause();
            let threads: Vec<_> = handles
                .iter()
                .cloned()
                .map(|handle| {
                    std::thread::spawn(move || {
                        for i in 0..COMMITS {
                            commit_text(&handle, &format!("{i:03},"));
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            assert_eq!(store.outbox_write_transactions(), before, "a commit wrote");
            for (chat, base) in chats.iter().zip(&baseline) {
                assert!(host.outbox_pending(chat).unwrap());
                assert_eq!(store.pending_chat_updates(chat).unwrap().len(), *base);
            }
        }
        outbox.flush();
        for ((chat, handle), base) in chats.iter().zip(&handles).zip(&baseline) {
            let (rows, text) = replay_in_order(&store, chat);
            assert_eq!(
                rows,
                base + COMMITS,
                "{chat}: one row per commit, exactly once"
            );
            assert_eq!(text, handle.doc.doc().get_text("body").to_string());
        }
        // 300 rows, never more than one transaction per 64: batching, and no
        // second INSERT from the client.
        let txs = store.outbox_write_transactions() - before;
        assert!(
            txs >= 5 && txs <= 300 / 2,
            "{txs} transactions for 300 rows"
        );
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_snapshot_waits_for_the_outbox_rows_of_what_it_contains() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("barrier").unwrap();
        let outbox = host.inner.outbox.clone();
        let baseline = store.pending_chat_updates("barrier").unwrap().len();
        let pause = outbox.pause();
        commit_text(&handle, "committed");
        let saved = Arc::new(AtomicBool::new(false));
        let saver = {
            let (host, handle, saved) = (host.clone(), handle.clone(), saved.clone());
            std::thread::spawn(move || {
                host.save_snapshot(&handle);
                saved.store(true, Ordering::SeqCst);
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !saved.load(Ordering::SeqCst),
            "snapshot landed before the outbox row"
        );
        assert_eq!(
            store.pending_chat_updates("barrier").unwrap().len(),
            baseline
        );
        drop(pause);
        tokio::task::spawn_blocking(move || saver.join().unwrap())
            .await
            .unwrap();
        // Crash here: the snapshot and the outbox both hold the commit.
        assert_eq!(
            store.pending_chat_updates("barrier").unwrap().len(),
            baseline + 1
        );
        let (bytes, _, _) = store.load_snapshot_with_cursor("barrier").unwrap().unwrap();
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(restored.get_text("body").to_string(), "committed");
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_snapshot_covers_commits_whose_local_hook_has_not_submitted_yet() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("late-hook").unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *lock(&handle.before_outbox_submit) = Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let committing = {
            let handle = handle.clone();
            std::thread::spawn(move || commit_text(&handle, "already committed"))
        };
        tokio::task::spawn_blocking(move || {
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        })
        .await
        .unwrap();
        let snapshot = handle.doc.export_snapshot().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let saving = {
            let store = store.clone();
            std::thread::spawn(move || {
                store.save_snapshot("late-hook", &snapshot).unwrap();
                done_tx.send(()).unwrap();
            })
        };
        let result = tokio::task::spawn_blocking(move || {
            done_rx.recv_timeout(std::time::Duration::from_secs(5))
        })
        .await
        .unwrap();
        // Always release the hook, including on a regression, so the test cleans up.
        release_tx.send(()).unwrap();
        committing.join().unwrap();
        saving.join().unwrap();
        result.expect("snapshot barrier deadlocked with the late hook");
        host.inner.outbox.flush();
        assert_eq!(replay_in_order(&store, "late-hook").1, "already committed");
        host.shutdown_workers().await;
    }

    /// Exercise the real persister's write mutex and delta path. Reopen before
    /// any graceful shutdown can compact the chain and hide a lost delta.
    async fn delta_append_waits_for_outbox_and_reopens(hook_lag: bool) {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("delta-barrier").unwrap();
        let persistence = handle.persistence.as_ref().unwrap().clone();
        persistence.verify_deltas.store(false, Ordering::Relaxed);
        commit_text(&handle, "base,");
        persistence.flush_sync();
        // A previous delta is part of the dependency source, too.
        commit_text(&handle, "prior delta,");
        persistence.flush_delta_for_test();
        let before = store.load_chat_state("delta-barrier").unwrap().unwrap();
        assert_eq!(before.delta_count, 1);
        host.inner.outbox.flush();
        let baseline = store.pending_chat_updates("delta-barrier").unwrap().len();
        let outbox = host.inner.outbox.clone();
        let pause = outbox.pause();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let committing = if hook_lag {
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            *lock(&handle.before_outbox_submit) = Some(Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            }));
            let handle = handle.clone();
            let committing = std::thread::spawn(move || commit_text(&handle, "in flight"));
            tokio::task::spawn_blocking(move || {
                entered_rx.recv_timeout(Duration::from_secs(10)).unwrap()
            })
            .await
            .unwrap();
            Some(committing)
        } else {
            commit_text(&handle, "in flight");
            None
        };
        let expected = handle.doc.doc().oplog_vv();
        let (saved_tx, saved_rx) = std::sync::mpsc::channel();
        let saving = {
            let persistence = persistence.clone();
            std::thread::spawn(move || {
                persistence.flush_delta_for_test();
                saved_tx.send(persistence.is_clean()).unwrap();
            })
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !outbox.has_in_flight("delta-barrier") && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let submitted = outbox.has_in_flight("delta-barrier");
        let held_rows = store.pending_chat_updates("delta-barrier").unwrap().len();
        let held_deltas = store
            .load_chat_state("delta-barrier")
            .unwrap()
            .unwrap()
            .delta_count;
        // Keep the hook held: the barrier must complete independently of it.
        drop(pause);
        let saved =
            tokio::task::spawn_blocking(move || saved_rx.recv_timeout(Duration::from_secs(10)))
                .await
                .unwrap();
        if hook_lag {
            let _ = release_tx.send(());
        }
        if let Some(committing) = committing {
            committing.join().unwrap();
        }
        saving.join().unwrap();
        assert!(submitted, "barrier did not supply the late hook's prefix");
        assert_eq!(held_rows, baseline);
        assert_eq!(held_deltas, 1, "delta landed before its outbox row");
        assert!(saved.expect("delta barrier waited for the hook"));
        outbox.flush();
        assert_eq!(
            store.pending_chat_updates("delta-barrier").unwrap().len(),
            baseline + 1
        );
        let restored = store.load_chat_state("delta-barrier").unwrap().unwrap();
        assert_eq!(
            restored.delta_count, 2,
            "oracle/full fallback masked the delta path"
        );
        assert_eq!(restored.base_snapshot, before.base_snapshot);
        assert_eq!(restored.doc.oplog_vv(), expected);
        assert_eq!(
            replay_in_order(&store, "delta-barrier").1,
            "base,prior delta,in flight"
        );
        // A new store and host reconstruct from disk while the original host
        // has not run shutdown or written a newer full snapshot.
        let fresh_store = Arc::new(DocsStore::open(dir.path()).unwrap());
        assert_eq!(
            fresh_store
                .load_chat_state("delta-barrier")
                .unwrap()
                .unwrap()
                .delta_count,
            2
        );
        let reopened = DocHost::new(
            fresh_store,
            DocHostConfig {
                device_id: "reopen".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        let recovered = reopened.open_local("delta-barrier").unwrap();
        assert_eq!(recovered.doc.doc().oplog_vv(), expected);
        assert_eq!(
            recovered.doc.doc().get_text("body").to_string(),
            "base,prior delta,in flight"
        );
        drop(recovered);
        reopened.shutdown_workers().await;
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delta_append_covers_a_late_local_hook_without_the_oracle() {
        delta_append_waits_for_outbox_and_reopens(true).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delta_append_waits_for_in_flight_ops_and_reopens_without_compaction() {
        delta_append_waits_for_outbox_and_reopens(false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_accepted_command_is_in_the_outbox_before_the_call_returns() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        host.open_local("cmd").unwrap();
        let baseline = store.pending_chat_updates("cmd").unwrap().len();
        let outbox = host.inner.outbox.clone();
        let pause = outbox.pause();
        let returned = Arc::new(AtomicBool::new(false));
        let queuer = {
            let (host, returned) = (host.clone(), returned.clone());
            std::thread::spawn(move || {
                let id = host
                    .queue_command("cmd", SessionCommandPayload::Interrupt {})
                    .unwrap();
                returned.store(true, Ordering::SeqCst);
                id
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !returned.load(Ordering::SeqCst),
            "accepted before the row was written"
        );
        drop(pause);
        tokio::task::spawn_blocking(move || queuer.join().unwrap())
            .await
            .unwrap();
        assert_eq!(
            store.pending_chat_updates("cmd").unwrap().len(),
            baseline + 1
        );
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_flushes_the_outbox_writer() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("shutdown").unwrap();
        let baseline = store.pending_chat_updates("shutdown").unwrap().len();
        let outbox = host.inner.outbox.clone();
        let pause = outbox.pause();
        for i in 0..20 {
            commit_text(&handle, &format!("{i},"));
        }
        drop(handle);
        let stopping = {
            let host = host.clone();
            tokio::spawn(async move { host.shutdown_workers().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !stopping.is_finished(),
            "shutdown finished with rows unwritten"
        );
        drop(pause);
        stopping.await.unwrap();
        let (rows, text) = replay_in_order(&store, "shutdown");
        assert_eq!(rows, baseline + 20);
        assert_eq!(text, (0..20).map(|i| format!("{i},")).collect::<String>());
        // A fresh host recovers every commit from the outbox.
        let reopened = outbox_host(&store);
        let recovered = reopened.open_local("shutdown").unwrap();
        assert_eq!(recovered.doc.doc().get_text("body").to_string(), text);
        reopened.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn purge_drains_queued_rows_and_disables_orphaned_writers_and_snapshots() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = outbox_host(&store);
        let handle = host.open_local("deleted").unwrap();
        let outbox = host.inner.outbox.clone();
        let pause = outbox.pause();
        commit_text(&handle, "queued before delete");
        assert!(host.pinned(&handle), "queued updates must pin the handle");
        let purging = {
            let host = host.clone();
            std::thread::spawn(move || host.purge_chat("deleted"))
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(pause);
        tokio::task::spawn_blocking(move || purging.join().unwrap())
            .await
            .unwrap();
        commit_text(&handle, "orphaned after delete");
        handle.persistence.as_ref().unwrap().dirty(true);
        handle.persistence.as_ref().unwrap().flush_sync();
        host.save_snapshot(&handle);
        outbox.flush();
        assert!(!store.has_snapshot("deleted").unwrap());
        assert!(store.pending_chat_updates("deleted").unwrap().is_empty());
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_disk_writes_pin_edits_and_resume_admission_after_recovery() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        let handle = host.open_local("failed").unwrap();
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER injected_disk_failure BEFORE INSERT ON chat_outbox WHEN NEW.doc_id='failed' BEGIN SELECT RAISE(FAIL,'injected disk failure'); END;").unwrap();
        handle
            .doc
            .doc()
            .get_text("body")
            .insert(0, "must survive a failed write")
            .unwrap();
        handle.doc.doc().commit();
        let other = host.open("waiting-for-storage").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert_eq!(
            host.chat_sync_state("failed"),
            zeron_proto::ChatSyncState::StorageError
        );
        assert!(!other.sync_started.load(Ordering::Acquire));
        assert!(host.pinned(&handle));
        db.execute_batch("DROP TRIGGER injected_disk_failure;")
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while handle.publication_failed.load(Ordering::Acquire)
                || !other.sync_started.load(Ordering::Acquire)
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(lock(&handle.chat2_pending_local).is_empty());
        host.shutdown_workers().await;
        let reopened = DocHost::new(
            store,
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        assert_eq!(
            reopened
                .open_local("failed")
                .unwrap()
                .doc
                .doc()
                .get_text("body")
                .to_string(),
            "must survive a failed write"
        );
        reopened.shutdown_workers().await;
    }

    #[tokio::test]
    async fn caller_and_watch_protect_the_open_to_attach_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let host = DocHost::new(
            Arc::new(DocsStore::open(dir.path()).unwrap()),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        let held = host.open_local("held").unwrap();
        for i in 0..30 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        let watch = held.watch_messages();
        drop(held);
        for i in 30..60 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        drop(watch);
        host.open_local("overflow").unwrap();
        assert!(!lock(&host.inner.handles).contains_key("held"));
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opening_many_chats_bounds_active_clients() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        for i in 0..300 {
            host.open(&format!("chat-{i}")).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let running = lock(&host.inner.handles)
            .values()
            .filter(|h| h.sync_started.load(Ordering::Acquire))
            .count();
        assert!(
            running > 0 && running <= ACTIVE_SYNC_CAP,
            "active clients: {running}"
        );
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn local_open_journals_without_starting_sync_and_reuses_the_document() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "writer".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        let handle = host.open_local("local").unwrap();
        handle
            .doc
            .doc()
            .get_text("body")
            .insert(0, "offline update")
            .unwrap();
        handle.doc.doc().commit();
        assert!(!handle.sync_started.load(Ordering::Acquire));
        assert!(lock(&handle.chat2).is_none());
        assert!(!store.pending_chat_updates("local").unwrap().is_empty());
        assert!(lock(&handle.chat2_pending_local).is_empty());
        let same = host.open("local").unwrap();
        assert!(Arc::ptr_eq(&handle, &same));
        assert!(handle.sync_requested.load(Ordering::Acquire));
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn salvage_inspection_does_not_open_healthy_or_unrecoverable_chats() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        for n in 0..300 {
            let id = format!("healthy-{n}");
            let doc = SessionDoc::init(&id).unwrap();
            let entry: SessionMessageEntry = serde_json::from_value(serde_json::json!({
                "id": "message", "role": "user", "parts": [], "createdAt": 1, "deviceId": "host"
            }))
            .unwrap();
            doc.push_message(&entry).unwrap();
            store
                .save_snapshot_with_cursor(&id, &doc.export_snapshot().unwrap(), 0, 2)
                .unwrap();
            host.salvage_chat_transcript(&id).await.unwrap();
        }
        host.salvage_chat_transcript("no-recovery-source")
            .await
            .unwrap();
        assert!(lock(&host.inner.handles).is_empty());
        host.shutdown_workers().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lru_eviction_replays_unacknowledged_updates_after_reopen() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "writer".into(),
                default_harness: HarnessId::Codex,
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:1", "test")),
            },
        );
        let handle = host.open("evicted").unwrap();
        handle
            .doc
            .doc()
            .get_text("body")
            .insert(0, "unacknowledged cleanup")
            .unwrap();
        handle.doc.doc().commit();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while lock(&handle.chat2).is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(lock(&handle.chat2).as_ref().unwrap().stats().pending_pushes > 0);
        for i in 0..WARM_DOC_CAP {
            host.open(&format!("other-{i}")).unwrap();
        }

        host.stop_sync(&handle).await;
        assert!(
            !host.pinned(&handle),
            "durably queued ops need not retain the whole doc in memory"
        );
        drop(handle);
        host.open("overflow").unwrap();
        host.evict_over_budget();
        assert!(!lock(&host.inner.handles).contains_key("evicted"));
        let before = store.pending_chat_updates("evicted").unwrap();
        let reopened = host.open("evicted").unwrap();
        assert_eq!(
            reopened.doc.doc().get_text("body").to_string(),
            "unacknowledged cleanup"
        );
        assert_eq!(store.pending_chat_updates("evicted").unwrap(), before);
        drop(reopened);
        host.shutdown_workers().await;
    }
}

#[cfg(test)]
mod salvage_tests {
    use super::*;
    use crate::workspace_host::WorkspaceHostConfig;

    fn user_message(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 1,
            device_id: "host".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn workspace(store: &Arc<DocsStore>) -> WorkspaceHost {
        WorkspaceHost::open(
            store.clone(),
            WorkspaceHostConfig {
                device_id: "host".into(),
                device_name: "Test host".into(),
                platform: "linux".into(),
                org_id: "test-org".into(),
                user_id: "test-user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap()
    }

    fn host_for(store: Arc<DocsStore>) -> DocHost {
        DocHost::new(
            store,
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        )
    }

    fn emptiness_agrees(doc: &SessionDoc) {
        let old = doc.read_entries().unwrap().is_empty();
        let new = messages_list_empty(doc.doc()).unwrap();
        assert_eq!(old, new);
    }

    /// Registry writes publish on the workspace task. The boot sweep reads
    /// that watch, so a test has to let the task run before sweeping.
    async fn wait_for_chats(workspace: &WorkspaceHost, n: usize) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if workspace.watch_chats().borrow().len() == n {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "registry watch never published {n} chats"
            );
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn salvage_emptiness_agrees_on_empty_nonempty_torn_and_continuation_docs() {
        let empty = SessionDoc::init("empty").unwrap();
        assert!(empty.read_entries().unwrap().is_empty());
        emptiness_agrees(&empty);

        let full = SessionDoc::init("full").unwrap();
        full.push_message(&user_message("m1", "hello")).unwrap();
        assert!(!full.read_entries().unwrap().is_empty());
        emptiness_agrees(&full);

        // Torn intermediate: an entry map imported before its fields arrive.
        // Salvage keeps it, so both checks say non-empty.
        let torn = SessionDoc::init("torn").unwrap();
        {
            let map = torn
                .doc()
                .get_list("messages")
                .push_container(loro::LoroMap::new())
                .unwrap();
            map.insert("role", "user").unwrap();
            torn.doc().commit();
        }
        assert!(
            !torn.read_entries().unwrap().is_empty(),
            "a torn entry map salvages"
        );
        emptiness_agrees(&torn);

        // A non-map child is unsalvageable. read_entries skips it.
        let values_only = SessionDoc::init("values").unwrap();
        values_only.doc().get_list("messages").push("nope").unwrap();
        values_only.doc().commit();
        assert!(values_only.read_entries().unwrap().is_empty());
        emptiness_agrees(&values_only);

        // Loro values can themselves be objects, without a map container.
        // The decoder salvages these as entries too.
        let object_value = SessionDoc::init("object-value").unwrap();
        let value: loro::LoroValue =
            std::collections::HashMap::<String, loro::LoroValue>::new().into();
        object_value.doc().get_list("messages").push(value).unwrap();
        object_value.doc().commit();
        assert!(!object_value.read_entries().unwrap().is_empty());
        emptiness_agrees(&object_value);

        let continuation = SessionDoc::init("continuation").unwrap();
        let mut entry = user_message("c1", "more");
        entry.continuation_of = Some("missing-root".into());
        continuation.push_message(&entry).unwrap();
        assert!(!continuation.read_entries().unwrap().is_empty());
        emptiness_agrees(&continuation);
    }

    #[test]
    fn salvage_marker_tracks_snapshot_and_pending_update_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let id = "chat";
        let healthy = SessionDoc::init(id).unwrap();
        healthy.push_message(&user_message("one", "kept")).unwrap();
        let healthy_bytes = healthy.export_snapshot().unwrap();
        let blank_bytes = SessionDoc::init(id).unwrap().export_snapshot().unwrap();
        store.save_snapshot(id, &healthy_bytes).unwrap();
        store.mark_salvage_checked(id, &healthy_bytes, 0).unwrap();
        assert!(store.salvage_checked(id).unwrap());
        store.save_snapshot(id, &blank_bytes).unwrap();
        assert!(!store.salvage_checked(id).unwrap());
        // An inspection racing a replacement cannot mark the new snapshot.
        store.mark_salvage_checked(id, &healthy_bytes, 0).unwrap();
        assert!(!store.salvage_checked(id).unwrap());
        store.save_snapshot(id, &healthy_bytes).unwrap();
        store.mark_salvage_checked(id, &healthy_bytes, 0).unwrap();
        store.enqueue_chat_update(id, "update", b"update").unwrap();
        assert!(!store.salvage_checked(id).unwrap());
        store.mark_salvage_checked(id, &healthy_bytes, 0).unwrap();
        assert!(!store.salvage_checked(id).unwrap());
        store.delete_snapshot(id).unwrap();
        store.save_snapshot(id, &blank_bytes).unwrap();
        assert!(!store.salvage_checked(id).unwrap());
    }

    #[tokio::test]
    async fn salvage_retries_after_a_checked_snapshot_is_replaced_by_blank() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = host_for(store.clone());
        let id = "later-residue";
        let fat = SessionDoc::init(id).unwrap();
        fat.push_message(&user_message("original", "recover me"))
            .unwrap();
        let bytes = fat.export_snapshot().unwrap();
        store.save_snapshot_with_cursor(id, &bytes, 0, 2).unwrap();
        store
            .save_snapshot(&format!("{id}.pre-chat2"), &bytes)
            .unwrap();
        host.salvage_chat_transcript(id).await.unwrap();
        assert!(store.salvage_checked(id).unwrap());
        let blank = SessionDoc::init(id).unwrap().export_snapshot().unwrap();
        store.save_snapshot_with_cursor(id, &blank, 0, 2).unwrap();
        host.salvage_chat_transcript(id).await.unwrap();
        let entries = host.open(id).unwrap().doc().read_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "original");
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn salvage_refills_a_blank_journaled_chat_and_the_marker_skips_the_next_boot() {
        let dir = tempfile::tempdir().unwrap();
        let journals = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let workspace = workspace(&store);
        let host = host_for(store.clone());
        host.set_workspace(workspace.clone());
        let id = "lost";
        workspace
            .create_chat(id, None, Some("host"), None, None)
            .unwrap();
        let blank = SessionDoc::init(id).unwrap();
        store
            .save_snapshot_with_cursor(
                id,
                &blank.export_snapshot().unwrap(),
                0,
                crate::chat2_host::CHAT2_DOC_EPOCH,
            )
            .unwrap();
        let fat = SessionDoc::init(id).unwrap();
        fat.push_message(&user_message("u-1", "the codeword is PINEAPPLE"))
            .unwrap();
        let mut reply = user_message("a-1", "acknowledged");
        reply.role = MessageRole::Assistant;
        reply.status = Some(MessageStatus::Complete);
        fat.push_message(&reply).unwrap();
        store
            .save_snapshot(&format!("{id}.pre-chat2"), &fat.export_snapshot().unwrap())
            .unwrap();
        let journal = journals.path().join(format!("{id}.jsonl"));
        std::fs::write(
            &journal,
            "{\"seq\":1,\"event\":{\"type\":\"sessionStarted\"}}\n",
        )
        .unwrap();
        let journal_bytes = std::fs::read(&journal).unwrap();
        wait_for_chats(&workspace, 1).await;

        host.salvage_hosted_chats(journals.path()).await;
        let entries = host.open(id).unwrap().doc().read_entries().unwrap();
        assert_eq!(entries.len(), 2, "blank journaled chat was not salvaged");
        assert_eq!(entries[0].id, "u-1");
        assert_eq!(entries[1].id, "a-1");
        assert_eq!(entries[1].status, Some(MessageStatus::Complete));
        assert!(store.salvage_checked(id).unwrap());
        assert_eq!(std::fs::read(&journal).unwrap(), journal_bytes);

        let loads = store.snapshot_loads();
        let imports = host.inner.salvage_imports.load(Ordering::Relaxed);
        assert!(imports >= 1, "the salvage itself imported nothing");
        host.salvage_hosted_chats(journals.path()).await;
        assert_eq!(
            store.snapshot_loads(),
            loads,
            "marked chat loaded a snapshot on the next boot"
        );
        assert_eq!(host.inner.salvage_imports.load(Ordering::Relaxed), imports);
        assert_eq!(
            host.open(id).unwrap().doc().read_entries().unwrap().len(),
            2
        );
        assert_eq!(std::fs::read(&journal).unwrap(), journal_bytes);
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn salvage_boot_does_no_snapshot_imports_for_marked_chats() {
        let dir = tempfile::tempdir().unwrap();
        let journals = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let workspace = workspace(&store);
        let host = host_for(store.clone());
        host.set_workspace(workspace.clone());
        for n in 0..200 {
            let id = format!("marked-{n:03}");
            workspace
                .create_chat(&id, None, Some("host"), None, None)
                .unwrap();
            let doc = SessionDoc::init(&id).unwrap();
            doc.push_message(&user_message("m", "kept")).unwrap();
            store
                .save_snapshot_with_cursor(
                    &id,
                    &doc.export_snapshot().unwrap(),
                    0,
                    crate::chat2_host::CHAT2_DOC_EPOCH,
                )
                .unwrap();
            std::fs::write(journals.path().join(format!("{id}.jsonl")), b"{}\n").unwrap();
            store
                .mark_salvage_checked(
                    &id,
                    &doc.export_snapshot().unwrap(),
                    crate::chat2_host::CHAT2_DOC_EPOCH,
                )
                .unwrap();
        }
        wait_for_chats(&workspace, 200).await;
        let loads = store.snapshot_loads();
        let imports = host.inner.salvage_imports.load(Ordering::Relaxed);
        host.salvage_hosted_chats(journals.path()).await;
        assert_eq!(
            store.snapshot_loads(),
            loads,
            "marked chats still loaded snapshots"
        );
        assert_eq!(host.inner.salvage_imports.load(Ordering::Relaxed), imports);
        assert!(lock(&host.inner.handles).is_empty());
        let journal = journals.path().join("marked-000.jsonl");
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 3);
        assert_eq!(
            std::fs::read_dir(journals.path()).unwrap().count(),
            200,
            "journals were removed"
        );
        host.shutdown_workers().await;
    }
}

#[cfg(test)]
mod change_mask_tests {
    use super::*;
    use std::time::Duration;

    fn host(dir: &Path) -> DocHost {
        DocHost::new(
            Arc::new(DocsStore::open(dir).unwrap()),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        )
    }

    fn command(id: &str, payload: SessionCommandPayload) -> SessionCommandEntry {
        SessionCommandEntry {
            id: id.into(),
            payload,
            issued_by: "viewer".into(),
            issued_at: now_ms(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        }
    }

    fn steer(id: &str) -> SessionCommandEntry {
        command(
            id,
            SessionCommandPayload::Steer {
                prompt: "go".into(),
                message_id: None,
            },
        )
    }

    fn message(id: &str) -> zeron_doc::SessionMessageEntry {
        zeron_doc::SessionMessageEntry {
            id: id.into(),
            role: zeron_doc::MessageRole::User,
            parts: vec![zeron_doc::MessagePart::Text {
                id: format!("{id}-t"),
                text: "hello".into(),
            }],
            created_at: 1,
            device_id: "host".into(),
            status: Some(zeron_doc::MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    /// Let the change workers run behind a commit.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_the_roots_that_changed_wake_their_consumers() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("masks").unwrap();
        let mut messages = handle.watch_messages();
        let mut queue = handle.watch_queue();
        settle().await;
        messages.borrow_and_update();
        queue.borrow_and_update();
        let ledger_before = *handle.commands_tx.borrow();

        // A streaming-style burst: messages only.
        for i in 0..50 {
            handle.doc.push_message(&message(&format!("m{i}"))).unwrap();
        }
        settle().await;
        assert!(messages.has_changed().unwrap(), "messages republished");
        assert!(!queue.has_changed().unwrap(), "queue untouched");
        assert_eq!(
            *handle.commands_tx.borrow(),
            ledger_before,
            "ledger untouched"
        );
        messages.borrow_and_update();

        // A command: neither watch moves, the ledger generation does.
        handle.doc.queue_command(&steer("c1")).unwrap();
        settle().await;
        assert!(!messages.has_changed().unwrap(), "messages not republished");
        assert!(!queue.has_changed().unwrap());
        assert_eq!(*handle.commands_tx.borrow(), ledger_before + 1);

        // A queue row: only the queue watch moves.
        handle
            .doc
            .push_queued(&zeron_doc::QueuedMessage::new("q1", "text", "host"))
            .unwrap();
        settle().await;
        assert!(queue.has_changed().unwrap(), "queue republished");
        assert!(!messages.has_changed().unwrap());
        assert_eq!(*handle.commands_tx.borrow(), ledger_before + 1);
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn a_slow_queue_publisher_cannot_overwrite_a_newer_drain() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("queue-publish-order").unwrap();
        host.shutdown_workers().await;
        handle
            .doc
            .push_queued(&zeron_doc::QueuedMessage::new("q1", "text", "host"))
            .unwrap();
        let queue = handle.watch_queue();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let slow = {
            let handle = handle.clone();
            std::thread::spawn(move || {
                handle.publish_queue_with(|| {
                    let rows = handle.doc.read_queue();
                    read_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    rows
                })
            })
        };
        read_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.doc.take_queue_head().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let drain_publish = {
            let handle = handle.clone();
            std::thread::spawn(move || {
                handle.publish_queue();
                done_tx.send(()).unwrap();
            })
        };
        // If publication is serialized, the second publisher waits behind
        // the first. Otherwise let it finish before releasing the old read.
        let _ = done_rx.recv_timeout(Duration::from_millis(50));
        release_tx.send(()).unwrap();
        slow.join().unwrap();
        drain_publish.join().unwrap();
        assert_eq!(*queue.borrow(), handle.doc.read_queue().unwrap());
    }

    /// Plan D3 test (a): a stream of commits that never touches the ledger
    /// reads it zero times after the first pass.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streaming_commits_do_not_re_read_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("stream").unwrap();
        handle.doc.queue_command(&steer("c1")).unwrap();
        let _watch = handle.watch_messages();
        assert_eq!(handle.commands().unwrap().len(), 1);
        let fills = handle.commands_cache_fills();
        for i in 0..50 {
            handle.doc.push_message(&message(&format!("m{i}"))).unwrap();
            // Every reader the tick hits: eviction, durability, the drains.
            assert_eq!(handle.commands().unwrap().len(), 1);
            assert!(host.has_waiting_commands());
        }
        settle().await;
        assert_eq!(handle.commands_cache_fills(), fills, "no ledger re-read");
        // A real ledger write is seen by the very next read.
        handle.doc.queue_command(&steer("c2")).unwrap();
        assert_eq!(handle.commands().unwrap().len(), 2);
        assert_eq!(handle.commands_cache_fills(), fills + 1);
        assert_eq!(handle.commands_cache_mismatches(), 0);
        host.shutdown_workers().await;
    }

    /// A control that is executing holds up a later prompt only until its
    /// outcome lands: the wait wakes on the ledger write, not on a poll.
    #[tokio::test(start_paused = true)]
    async fn waiting_for_an_earlier_control_wakes_on_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("waits").unwrap();
        handle
            .doc
            .queue_command(&command("stop", SessionCommandPayload::Interrupt {}))
            .unwrap();
        handle.doc.queue_command(&steer("prompt")).unwrap();
        lock(&host.inner.executing).insert("stop".into());

        let started = tokio::time::Instant::now();
        let waiter = {
            let (host, handle) = (host.clone(), handle.clone());
            tokio::spawn(async move { host.await_earlier_controls(&handle, "prompt").await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished(), "the control is still executing");
        let fills = handle.commands_cache_fills();

        handle
            .doc
            .set_command_status("stop", SessionCommandStatus::Applied, None)
            .unwrap();
        waiter.await.unwrap();
        assert!(
            started.elapsed() < WAIT_RECHECK,
            "woke on the ledger change, not the recheck tick: {:?}",
            started.elapsed()
        );
        assert_eq!(
            handle.commands_cache_fills(),
            fills + 1,
            "one re-read for the one ledger write"
        );
        host.shutdown_workers().await;
    }

    /// The claim set is not in the ledger, so a control that finishes without
    /// an outcome write is noticed by the recheck tick, not missed.
    #[tokio::test(start_paused = true)]
    async fn waiting_for_an_earlier_control_notices_a_released_claim() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("claims").unwrap();
        handle
            .doc
            .queue_command(&command("stop", SessionCommandPayload::Interrupt {}))
            .unwrap();
        handle.doc.queue_command(&steer("prompt")).unwrap();
        lock(&host.inner.executing).insert("stop".into());
        let waiter = {
            let (host, handle) = (host.clone(), handle.clone());
            tokio::spawn(async move { host.await_earlier_controls(&handle, "prompt").await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished());
        lock(&host.inner.executing).remove("stop");
        tokio::time::timeout(WAIT_RECHECK * 2, waiter)
            .await
            .expect("the recheck tick sees the released claim")
            .unwrap();
        host.shutdown_workers().await;
    }

    #[tokio::test(start_paused = true)]
    async fn ledger_wait_retains_updates_between_read_and_wait() {
        let (tx, mut rx) = watch::channel(0u64);
        let read_generation = *rx.borrow();
        tx.send_modify(|generation| *generation += 1);
        tx.send_modify(|generation| *generation += 1);
        let started = tokio::time::Instant::now();
        assert!(wait_for_ledger(&mut rx, WAIT_RECHECK).await);
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(*rx.borrow(), read_generation + 2);
        tx.send_modify(|generation| *generation += 1);
        assert!(wait_for_ledger(&mut rx, WAIT_RECHECK).await);
        assert_eq!(started.elapsed(), Duration::ZERO);
        drop(tx);
        assert!(!wait_for_ledger(&mut rx, WAIT_RECHECK).await);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_an_earlier_prompt_wakes_on_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("prompt-wait").unwrap();
        let sessions = SessionsEngine::new(
            "host".into(),
            Arc::new(crate::run_journal::RunJournal::open(dir.path().join("journal")).unwrap()),
            Arc::new(crate::HarnessRegistry::new()),
        );
        handle.doc.queue_command(&steer("prompt")).unwrap();
        handle
            .doc
            .queue_command(&command("stop", SessionCommandPayload::Interrupt {}))
            .unwrap();
        let started = tokio::time::Instant::now();
        let waiter = {
            let (host, handle) = (host.clone(), handle.clone());
            tokio::spawn(
                async move { host.await_earlier_prompts(&sessions, &handle, "stop").await },
            )
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished());
        handle
            .doc
            .set_command_status("prompt", SessionCommandStatus::Cancelled, None)
            .unwrap();
        waiter.await.unwrap();
        assert!(started.elapsed() < WAIT_RECHECK);
        host.shutdown_workers().await;
    }

    #[test]
    fn drain_outcome_is_complete_only_for_a_finished_pass() {
        assert!(DrainOutcome::COMPLETE.complete);
        assert!(!DrainOutcome::INCOMPLETE.complete);
    }

    /// Without a wired sessions engine a pass cannot run, which is the
    /// "retry on the next change" case the dispatcher must keep.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pass_that_cannot_run_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let handle = host.open_local("incomplete").unwrap();
        assert!(!host.drain_commands(&handle).await.complete);
        host.shutdown_workers().await;
    }
    /// Controls cannot execute this prompt, and there is no set_sessions or
    /// lift_quiesce kick to rescue it. Only the dispatcher's incomplete-pass
    /// retry on the transcript event can resolve it.
    #[tokio::test]
    async fn incomplete_dispatch_retries_a_prompt_on_a_message_only_change() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        *lock(&host.inner.sessions) = Some(SessionsEngine::new(
            "host".into(),
            Arc::new(crate::run_journal::RunJournal::open(dir.path().join("journal")).unwrap()),
            Arc::new(crate::HarnessRegistry::new()),
        ));
        host.inner.quiesced.store(true, Ordering::SeqCst);
        let handle = host.open_local("incomplete-prompt").unwrap();
        let mut prompt = steer("deferred-prompt");
        prompt.expires_at = Some(0); // resolution needs no live harness
        handle.doc.queue_command(&prompt).unwrap();
        settle().await;
        assert_eq!(
            handle.commands().unwrap()[0].status,
            SessionCommandStatus::Pending
        );
        assert!(
            handle.dispatch_bits.take().is_empty(),
            "dispatcher consumed the command event"
        );

        host.inner.quiesced.store(false, Ordering::SeqCst);
        handle.doc.push_message(&message("retry-tick")).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while handle.commands().unwrap()[0].status == SessionCommandStatus::Pending {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("message-only event must retry the incomplete prompt pass");
        assert_eq!(
            handle.commands().unwrap()[0].status,
            SessionCommandStatus::Expired
        );
        host.shutdown_workers().await;
    }
}

#[cfg(test)]
#[path = "doc_host_sync_tests.rs"]
mod sync_lifecycle_tests;

#[path = "doc_host_delivery.rs"]
mod remote_delivery;
