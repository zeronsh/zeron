//! CheckoutDiffSync — checkout-scoped working-tree diff production (feature-inventory
//! §3.5; port of zeron's `checkout-diff-sync.ts` + `git-metadata-sync.ts`).
//!
//! Chats do not own working-tree state: a concrete Git checkout does. This service
//! groups this device's chats by their canonical checkout identity (`chat.cwd` →
//! [`Repos::checkout_identity`]), computes one bounded atomic snapshot per checkout,
//! and publishes it three ways:
//!
//! - the local `WatchCheckoutDiffs` stream (a watch channel of every checkout's
//!   latest [`CheckoutDiff`]);
//! - a [`DiffSidecar`] JSON `POST {edge}/diff/{chatId}` for every syncing chat of
//!   the checkout (bearer = engine edge token), so "review pending changes while
//!   the host sleeps" works;
//! Checkout snapshots remain live checkout state. Conversation branch identity
//! is captured by the command host and is never rewritten from this watcher;
//! otherwise one checkout change would relabel every chat sharing that folder.
//!
//! Archived chats are ignored: they get no entry, no watcher, and no captures
//! (an entry whose every chat was archived is dropped on the next reconcile;
//! unarchiving re-adds it).
//!
//! Fast recursive watches on the process-wide [`FsWatchHub`] (one watcher thread
//! for every checkout; debounced [`WATCH_DEBOUNCE`]) are backed by a slow
//! 2-minute repair tick because native watchers may coalesce or drop events.
//! Snapshots carry a sha256 checksum; an unchanged checksum publishes nothing.
//!
//! Reconcile is deliberately damped, because it runs on *every* workspace chat
//! row change — including the `checkoutId` writes reconcile itself makes.
//! Checkout identities are memoized per cwd (chat-watch reconciles spawn
//! no git; the repair tick revalidates), and an entry whose chats vanish is torn
//! down only after [`REPAIR_INTERVAL`] of continuous absence rather than on the
//! first pass that misses it. See [`resolve_identity`] for the incident this
//! guards against.
//!
//! Beyond the working-tree watch, the service also answers one-shot
//! `GetCheckoutDiff` captures for the Changes pane's scopes: *branch changes*
//! (vs `merge-base(baseRef, HEAD)`, same capture path with the base overridden)
//! and *latest turn* (vs a temp-index `write-tree` snapshot taken when a turn
//! dispatches — see [`CheckoutDiffSync::note_turn_start`]). Those snapshots
//! live in Zeron's own object store, never the checkout's — see
//! [`turn_snapshot`].

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;

use zeron_proto::{Chat, CheckoutDiff, DiffFileSummary};

use crate::EngineError;
use crate::doc_host::EdgeConfig;
use crate::fs_watch::{FsWatch, FsWatchHub};
use crate::repos::{CheckoutIdentity, Repos};
use crate::workspace_host::WorkspaceHost;

mod git_status;
mod turn_snapshot;

pub use turn_snapshot::{TreeSnapshot, snapshot_tree};

/// Hard cap on the unified patch (plus untracked hunks) — "Partial snapshot".
pub const MAX_PATCH_BYTES: usize = 3 * 1024 * 1024;
pub const MAX_DIFF_SOURCE_BYTES: usize = 2 * 1024 * 1024;
/// Trailing debounce after a filesystem event burst.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// Slow repair pass: re-reconcile + re-sync every checkout.
const REPAIR_INTERVAL: Duration = Duration::from_secs(120);
/// How long an archived chat stays tracked after its last viewer leaves —
/// flicking between chats must not tear down and rebuild (full capture) its
/// checkout every time.
const INTEREST_LINGER: Duration = Duration::from_secs(30);
/// Max subdirectories a checkout may have before we skip its live recursive
/// watch (one OS watch per dir; past this the watcher thread's own bookkeeping
/// costs more than instant diffs are worth). A normal source tree is well
/// under this; a node_modules/vendored tree blows past it. The repair tick
/// still covers skipped checkouts.
const MAX_WATCH_DIRS: usize = 8_000;
/// Byte ceiling for the tracked-change metadata (`--name-status` and `--numstat`
/// each, or `--raw --numstat` at twice this) and the status listings.
const MAX_DIFF_METADATA_BYTES: usize = 2 * 1024 * 1024;
/// Most changed paths the gate stats; past it a capture never vouches for a skip.
const MAX_GATE_PATHS: usize = 4096;
/// A file stamped this recently before a capture may still change within the
/// timestamp's granularity, so the capture's fingerprint is not trusted.
const RACY_WINDOW: Duration = Duration::from_secs(2);
/// Repair ticks an idle, watched checkout may skip in a row before one runs
/// its gate anyway, as a safety net for events the OS watcher dropped.
const REPAIR_FORCE_EVERY: u32 = 10;
/// `git hash-object -t tree /dev/null` — diff base for repos with no commits yet.
const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Latest-only diff sidecar published to each chat's session DO slot
/// (`POST /diff/{chatId}`; shape: edge/src/session-doc/sidecar.ts).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSidecar {
    pub chat_id: String,
    pub device_id: String,
    pub checkout_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    pub patch: String,
    pub files: Vec<DiffFileSummary>,
    pub additions: u32,
    pub deletions: u32,
    pub truncated: bool,
    /// Epoch millis.
    pub published_at: i64,
}

/// One bounded atomic snapshot of a checkout's working tree.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffSnapshot {
    pub git_status: Option<(Vec<zeron_proto::GitFileStatus>, bool)>,
    pub branch: String,
    pub head_sha: Option<String>,
    pub patch: String,
    pub files: Vec<DiffFileSummary>,
    pub additions: u32,
    pub deletions: u32,
    pub truncated: bool,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileTextPair {
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub old_content_hash: Option<String>,
    pub new_content_hash: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

struct CheckoutEntry {
    identity: CheckoutIdentity,
    chats: Mutex<Vec<Chat>>,
    /// Last published checksum — unchanged snapshots publish nothing.
    checksum: Mutex<Option<String>>,
    /// Set when a reconcile pass finds no chats for this checkout; cleared the
    /// moment chats reappear. The entry (watchers, checksum state, published
    /// diff) is only torn down after `orphan_grace` of *continuous* absence:
    /// tearing down and re-adding restarts a full capture, which costs seconds
    /// of CPU on a big checkout, so a single flapping chat-watch emission or
    /// transient identity failure must never destroy a live entry.
    orphaned_since: Mutex<Option<std::time::Instant>>,
    /// A kick arrived while orphaned and was skipped — a chat-less checkout
    /// is not worth a capture. Reconcile re-kicks if chats come back.
    skipped_while_orphaned: AtomicBool,
    /// Kick channel into the entry's debounce/sync task.
    kick_tx: mpsc::UnboundedSender<()>,
    /// Keeps the recursive fs watches alive on the shared [`FsWatchHub`];
    /// dropped on entry close. Filled asynchronously — watcher setup (budget
    /// walk + FSEvents registration) can block for seconds, so [`add_entry`]
    /// does it off the runtime and attaches the result here once ready.
    watchers: Mutex<Vec<FsWatch>>,
    /// What the watcher callbacks have reported; see [`WatchSignals`].
    signals: Arc<WatchSignals>,
    /// Gate fingerprint of the last successful capture; a matching one skips
    /// the capture. Cleared whenever the entry's state may have moved under it.
    gate: Mutex<Option<GateState>>,
}

/// Watcher health for one checkout, shared with its watch callbacks, so the
/// repair tick can tell an idle watched checkout from one it must re-check.
struct WatchSignals {
    /// Bumped by every watch event that can change the diff.
    events: AtomicU64,
    /// `events` as read just before the last successful capture began;
    /// [`NEVER_CAPTURED`] until one completes. Equal to `events` means nothing
    /// has happened since that capture.
    captured_events: AtomicU64,
    /// The recursive watch on the worktree itself is installed (not just the
    /// git dir of an over-budget tree, whose edits only the repair tick sees).
    covers_worktree: AtomicBool,
    /// Consecutive repair ticks skipped for this checkout; see [`REPAIR_FORCE_EVERY`].
    quiet_ticks: AtomicU32,
}

const NEVER_CAPTURED: u64 = u64::MAX;

impl WatchSignals {
    fn new() -> Self {
        Self {
            events: AtomicU64::new(0),
            captured_events: AtomicU64::new(NEVER_CAPTURED),
            covers_worktree: AtomicBool::new(false),
            quiet_ticks: AtomicU32::new(0),
        }
    }
}

impl CheckoutEntry {
    /// The live watcher covers the whole checkout and has reported nothing
    /// since the last capture: a repair re-check could only find the same diff.
    fn watch_is_quiet(&self) -> bool {
        let signals = &self.signals;
        if !signals.covers_worktree.load(Ordering::Acquire) {
            return false;
        }
        let captured = signals.captured_events.load(Ordering::Acquire);
        if captured != signals.events.load(Ordering::Acquire) {
            return false; // never captured, or events since
        }
        let watchers = lock(&self.watchers);
        !watchers.is_empty() && watchers.iter().all(FsWatch::is_live)
    }

    /// The repair tick's kick: skipped while the watcher is healthy and quiet
    /// (bar a periodic safety-net pass), otherwise a capture that the gate
    /// cheapens to one `git status` when nothing changed.
    fn repair_kick(&self) {
        if self.watch_is_quiet() {
            let skipped = self.signals.quiet_ticks.fetch_add(1, Ordering::AcqRel) + 1;
            if skipped < REPAIR_FORCE_EVERY {
                return;
            }
        }
        self.signals.quiet_ticks.store(0, Ordering::Release);
        let _ = self.kick_tx.send(());
    }
}

/// Working-tree snapshot recorded when a chat's turn dispatches — the diff
/// base for the Changes pane's "Latest turn" scope. In-memory only: after an
/// engine restart the scope is unavailable until the next turn.
#[derive(Debug, Clone)]
pub struct TurnSnapshot {
    /// Canonical checkout root the tree was captured in.
    pub root: PathBuf,
    /// The tracked + untracked (unignored) tree, with the object store that
    /// holds it. Replacing or dropping the snapshot deletes that store.
    pub tree: TreeSnapshot,
    pub at: chrono::DateTime<chrono::Utc>,
}

struct DiffSyncInner {
    repos: Repos,
    workspace: WorkspaceHost,
    device_id: String,
    edge: Option<EdgeConfig>,
    http: reqwest::Client,
    entries: Mutex<HashMap<String, Arc<CheckoutEntry>>>,
    /// Serializes [`reconcile`] passes. Concurrent passes (chat-watch task vs.
    /// `reconcile_now`) can both observe a checkout as missing and both
    /// `add_entry` it — the second insert silently replaces the first entry,
    /// discarding its checksum state and kicking a redundant full capture.
    reconcile_gate: tokio::sync::Mutex<()>,
    /// cwd → resolved checkout identity. See [`resolve_identity`].
    identities: Mutex<HashMap<String, MemoizedIdentity>>,
    /// How long an entry may sit chat-less before reconcile removes it.
    orphan_grace: Duration,
    diffs_tx: watch::Sender<Vec<CheckoutDiff>>,
    statuses_tx: watch::Sender<Vec<zeron_proto::CheckoutGitStatus>>,
    /// chat_id → turn-start tree (see [`TurnSnapshot`]).
    turn_trees: Mutex<HashMap<String, TurnSnapshot>>,
    /// The tasks hold `Weak` refs, but an in-flight iteration holds an
    /// upgraded Arc — the token cuts it so no sidecar HTTP outlives shutdown.
    cancel: CancellationToken,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// chat_id → viewer interest; see [`CheckoutDiffSync::retain_chat`].
    interest: Mutex<HashMap<String, Interest>>,
    /// How long released interest keeps an archived chat tracked.
    interest_linger: Duration,
    /// Wakes the supervisor to reconcile when an archived chat's interest
    /// starts or lapses.
    interest_changed: Arc<Notify>,
    /// Serializes discards, whether or not their checkout has an entry: an
    /// entry can appear between the lookup and the lock, and two discards of
    /// one root must never interleave. File-system watchers and read-only
    /// captures may still run concurrently.
    discard_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Interest {
    holders: usize,
    /// When `holders` last fell to zero; interest lingers `interest_linger`.
    released_at: Option<Instant>,
}

/// Keeps a chat tracked by diff sync while alive, even if archived. See
/// [`CheckoutDiffSync::retain_chat`].
pub struct ChatInterest {
    inner: Weak<DiffSyncInner>,
    chat_id: String,
}

impl Drop for ChatInterest {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        {
            let mut interest = lock(&inner.interest);
            let Some(state) = interest.get_mut(&self.chat_id) else {
                return;
            };
            state.holders = state.holders.saturating_sub(1);
            if state.holders > 0 {
                return;
            }
            state.released_at = Some(Instant::now());
        }
        // Only an archived chat's entry depends on interest; reconcile once the
        // linger runs out so its checkout is released.
        if is_archived_here(&inner, &self.chat_id)
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let notify = inner.interest_changed.clone();
            let linger = inner.interest_linger;
            runtime.spawn(async move {
                tokio::time::sleep(linger + Duration::from_millis(50)).await;
                notify.notify_one();
            });
        }
    }
}

fn is_archived_here(inner: &DiffSyncInner, chat_id: &str) -> bool {
    inner
        .workspace
        .watch_chats()
        .borrow()
        .iter()
        .any(|chat| chat.id == chat_id && chat.archived && chat.device_id == inner.device_id)
}

/// Chats with live or lingering interest; prunes lapsed records.
fn interested_chats(inner: &DiffSyncInner) -> HashSet<String> {
    let mut interest = lock(&inner.interest);
    interest.retain(|_, state| {
        state.holders > 0
            || state
                .released_at
                .is_some_and(|at| at.elapsed() < inner.interest_linger)
    });
    interest.keys().cloned().collect()
}

/// Every git process the diff-sync paths start (captures, the status gate and
/// the identity resolves behind reconcile), for idle-cost checks.
static GIT_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// Per working directory, so a test can count only its own fixture's spawns
/// while the rest of the suite runs git in parallel.
#[cfg(test)]
static GIT_SPAWNS_BY_DIR: std::sync::LazyLock<Mutex<HashMap<PathBuf, u64>>> =
    std::sync::LazyLock::new(Mutex::default);

/// Count one git spawn made with `cwd` as its working directory.
pub(crate) fn note_git_spawn(cwd: Option<&Path>) {
    GIT_SPAWNS.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    if let Some(cwd) = cwd {
        *lock(&GIT_SPAWNS_BY_DIR)
            .entry(cwd.to_path_buf())
            .or_default() += 1;
    }
    #[cfg(not(test))]
    let _ = cwd;
}

/// Git processes diff sync has started since boot.
pub fn git_spawn_count() -> u64 {
    GIT_SPAWNS.load(Ordering::Relaxed)
}

/// Spawns made in `dir` or below it.
#[cfg(test)]
fn git_spawns_under(dir: &Path) -> u64 {
    lock(&GIT_SPAWNS_BY_DIR)
        .iter()
        .filter(|(cwd, _)| cwd.starts_with(dir))
        .map(|(_, count)| *count)
        .sum()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct CheckoutDiffSync {
    inner: Arc<DiffSyncInner>,
}

impl CheckoutDiffSync {
    /// Build and start the sync loop: follows the workspace chat watch and runs the
    /// 2-minute repair tick. Requires a tokio runtime.
    pub fn start(
        repos: Repos,
        workspace: WorkspaceHost,
        device_id: &str,
        edge: Option<EdgeConfig>,
    ) -> Self {
        // Grace = one repair interval: an entry must survive at least one full
        // fresh revalidation pass before reconcile may tear it down.
        Self::start_with_orphan_grace(repos, workspace, device_id, edge, REPAIR_INTERVAL)
    }

    /// [`CheckoutDiffSync::start`] with an explicit orphan grace — test hook so
    /// removal-after-grace is exercisable without waiting out [`REPAIR_INTERVAL`].
    #[doc(hidden)]
    pub fn start_with_orphan_grace(
        repos: Repos,
        workspace: WorkspaceHost,
        device_id: &str,
        edge: Option<EdgeConfig>,
        orphan_grace: Duration,
    ) -> Self {
        Self::start_with_timings(
            repos,
            workspace,
            device_id,
            edge,
            orphan_grace,
            INTEREST_LINGER,
        )
    }

    /// [`CheckoutDiffSync::start_with_orphan_grace`] plus an explicit interest
    /// linger — test hook for archived-chat interest release.
    #[doc(hidden)]
    pub fn start_with_timings(
        repos: Repos,
        workspace: WorkspaceHost,
        device_id: &str,
        edge: Option<EdgeConfig>,
        orphan_grace: Duration,
        interest_linger: Duration,
    ) -> Self {
        let (diffs_tx, _) = watch::channel(Vec::new());
        // Turn snapshots are in-memory state: whatever an earlier run left on
        // disk is unreachable now.
        turn_snapshot::sweep_stale_stores(repos.data_dir());
        let sync = Self {
            inner: Arc::new(DiffSyncInner {
                repos,
                workspace: workspace.clone(),
                device_id: device_id.to_string(),
                edge,
                http: reqwest::Client::new(),
                entries: Mutex::new(HashMap::new()),
                reconcile_gate: tokio::sync::Mutex::new(()),
                identities: Mutex::new(HashMap::new()),
                orphan_grace,
                diffs_tx,
                statuses_tx: watch::channel(Vec::new()).0,
                turn_trees: Mutex::new(HashMap::new()),
                cancel: CancellationToken::new(),
                supervisor: Mutex::new(None),
                interest: Mutex::new(HashMap::new()),
                interest_linger,
                interest_changed: Arc::new(Notify::new()),
                discard_lock: tokio::sync::Mutex::new(()),
            }),
        };
        let task = tokio::spawn(diff_sync_task(
            Arc::downgrade(&sync.inner),
            workspace.watch_chats(),
            sync.inner.interest_changed.clone(),
            sync.inner.cancel.clone(),
        ));
        *lock(&sync.inner.supervisor) = Some(task);
        sync
    }

    /// Stop the sync graph and wait for the supervisor to exit. Per-entry
    /// tasks observe the same token, so an in-flight `sync_entry` drops its
    /// sidecar POST instead of finishing it under a replaced runtime.
    /// Idempotent.
    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        let task = lock(&self.inner.supervisor).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// `WatchCheckoutDiffs` source: every tracked checkout's latest diff.
    pub fn watch_diffs(&self) -> watch::Receiver<Vec<CheckoutDiff>> {
        self.inner.diffs_tx.subscribe()
    }

    /// Consumers filter this shared cache; subscribing never starts another Git scan.
    pub fn watch_git_statuses(&self) -> watch::Receiver<Vec<zeron_proto::CheckoutGitStatus>> {
        self.inner.statuses_tx.subscribe()
    }

    /// Regroup this device's chats by checkout identity, then (re)build watchers.
    /// Public for tests (the background task calls it on every chat change).
    pub async fn reconcile_now(&self) {
        let chats = self.inner.workspace.watch_chats().borrow().clone();
        reconcile(&self.inner, chats, false).await;
    }

    /// Repair-tick path for tests: fresh identity revalidation, then re-check
    /// every tracked checkout the watcher cannot vouch for.
    #[doc(hidden)]
    pub async fn repair_now(&self) {
        let chats = self.inner.workspace.watch_chats().borrow().clone();
        repair_pass(&self.inner, chats).await;
    }

    /// Tracked checkout roots, each with its count of live fs watches — test
    /// hook (watches attach asynchronously after an entry is added).
    #[doc(hidden)]
    pub fn tracked_checkouts(&self) -> Vec<(PathBuf, usize)> {
        let mut tracked: Vec<(PathBuf, usize)> = lock(&self.inner.entries)
            .values()
            .map(|entry| {
                let live = lock(&entry.watchers).iter().filter(|w| w.is_live()).count();
                (entry.identity.root.clone(), live)
            })
            .collect();
        tracked.sort();
        tracked
    }

    /// Keep `chat_id` tracked while the returned guard lives — even when the
    /// chat is archived. Archived chats otherwise cost nothing (no entry,
    /// watch, capture or sidecar), but an archived chat someone has open must
    /// still show its checkout's live diff, accept a discard, and get its
    /// `checkoutId` stamped. The engine holds one guard per open transcript
    /// stream; released interest lingers [`INTEREST_LINGER`].
    pub fn retain_chat(&self, chat_id: &str) -> ChatInterest {
        let first = {
            let mut interest = lock(&self.inner.interest);
            let state = interest.entry(chat_id.to_string()).or_default();
            state.holders += 1;
            let lapsed = state
                .released_at
                .take()
                .is_none_or(|at| at.elapsed() >= self.inner.interest_linger);
            state.holders == 1 && lapsed
        };
        // Non-archived chats are tracked anyway — no reconcile needed.
        if first && is_archived_here(&self.inner, chat_id) {
            self.inner.interest_changed.notify_one();
        }
        ChatInterest {
            inner: Arc::downgrade(&self.inner),
            chat_id: chat_id.to_string(),
        }
    }

    /// Kick an immediate sync of every tracked checkout (repair-tick path).
    pub fn sync_all(&self) {
        for entry in lock(&self.inner.entries).values() {
            let _ = entry.kick_tx.send(());
        }
    }

    /// A turn is starting for `chat_id` in `cwd`: snapshot the checkout's tree
    /// in the background so "Latest turn" has a base. Best-effort — failures
    /// only log; a chat outside a checkout simply records nothing.
    pub fn note_turn_start(&self, chat_id: &str, cwd: &str) {
        let inner = Arc::downgrade(&self.inner);
        let chat_id = chat_id.to_string();
        let cwd = PathBuf::from(cwd);
        tokio::spawn(async move {
            let Some(inner) = inner.upgrade() else { return };
            let identity = match inner.repos.checkout_identity(&cwd).await {
                Ok(identity) => identity,
                Err(_) => {
                    // Not a checkout: no turn to diff, not the previous one.
                    lock(&inner.turn_trees).remove(&chat_id);
                    return;
                }
            };
            match snapshot_tree(&inner.repos, &identity.root).await {
                Ok(tree) => {
                    lock(&inner.turn_trees).insert(
                        chat_id,
                        TurnSnapshot {
                            root: identity.root,
                            tree,
                            at: chrono::Utc::now(),
                        },
                    );
                }
                Err(err) => {
                    // Keeping the previous base would silently diff two turns.
                    lock(&inner.turn_trees).remove(&chat_id);
                    tracing::debug!(chat = %chat_id, error = %err,
                        "diff-sync: turn snapshot failed");
                }
            }
        });
    }

    /// The recorded turn-start snapshot for a chat, if any turn dispatched
    /// since boot.
    pub fn turn_snapshot(&self, chat_id: &str) -> Option<TurnSnapshot> {
        lock(&self.inner.turn_trees).get(chat_id).cloned()
    }

    /// Discard the complete uncommitted state of a checkout after verifying
    /// that the UI acted on the latest full snapshot. A checkout without an
    /// entry (e.g. only archived chats, interest not yet reconciled) is
    /// discarded directly; there is just no live diff to republish.
    pub async fn discard_working_tree(
        &self,
        identity: &CheckoutIdentity,
        expected_checksum: &str,
    ) -> Result<DiffSnapshot, EngineError> {
        let entry = lock(&self.inner.entries).get(&identity.id).cloned();
        let inner = self.inner.clone();
        let expected_checksum = expected_checksum.to_owned();
        let Some(entry) = entry else {
            let root = identity.root.clone();
            return tokio::spawn(async move {
                let _guard = inner.discard_lock.lock().await;
                discard_working_tree(&inner.repos, &root, &expected_checksum).await
            })
            .await
            .map_err(|error| EngineError::Other(format!("discard task failed: {error}")))?;
        };
        // Own task: a started discard always runs to completion even if the
        // RPC caller goes away, and the nested git captures start from a fresh
        // worker stack instead of stacking on the RPC dispatcher's frames
        // (which overflowed the 2 MiB worker stack in debug builds).
        tokio::spawn(async move {
            let _guard = inner.discard_lock.lock().await;
            let result =
                discard_working_tree(&inner.repos, &entry.identity.root, &expected_checksum).await;

            // Publish immediately instead of waiting for the watcher debounce.
            // The watcher kick remains useful if an external writer races this
            // operation after the final capture. The discard rewrote files
            // behind the gate's back: capture for real.
            *lock(&entry.gate) = None;
            sync_entry(&inner, &entry).await;
            let _ = entry.kick_tx.send(());
            result
        })
        .await
        .map_err(|error| EngineError::Other(format!("discard task failed: {error}")))?
    }
}

// ---------------------------------------------------------------------------
// Reconcile: chats ⇄ checkout entries
// ---------------------------------------------------------------------------

/// A memoized identity plus what the filesystem looked like when it was
/// resolved, so a repair tick can confirm it without spawning git.
#[derive(Clone)]
struct MemoizedIdentity {
    identity: CheckoutIdentity,
    stamp: Option<IdentityStamp>,
}

/// Stats that move whenever a cwd's checkout identity could: the worktree's
/// `.git` entry (replaced by a re-clone or `git init`), a `.git` directly in
/// the cwd (a nested repository), and the git dir's `HEAD`.
#[derive(Clone, PartialEq, Eq)]
struct IdentityStamp {
    root_dot_git: FileStamp,
    cwd_dot_git: Option<FileStamp>,
    head: Option<FileStamp>,
}

/// `(mtime, size, inode)` of one path, without following symlinks. A
/// directory's own mtime and size only track churn among its entries (every
/// index lock inside `.git` moves them), so for one only the inode counts.
#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    modified: Option<Duration>,
    len: u64,
    ino: u64,
}

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        #[cfg(unix)]
        let ino = {
            use std::os::unix::fs::MetadataExt as _;
            meta.ino()
        };
        #[cfg(not(unix))]
        let ino = 0;
        if meta.is_dir() {
            return Some(Self {
                modified: None,
                len: 0,
                ino,
            });
        }
        Some(Self {
            modified: meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()),
            len: meta.len(),
            ino,
        })
    }
}

impl IdentityStamp {
    /// `None` when the worktree has no `.git` entry to anchor on.
    fn read(cwd: &str, identity: &CheckoutIdentity) -> Option<Self> {
        Some(Self {
            root_dot_git: FileStamp::of(&identity.root.join(".git"))?,
            cwd_dot_git: FileStamp::of(&Path::new(cwd).join(".git")),
            head: FileStamp::of(&identity.git_dir.join("HEAD")),
        })
    }
}

/// Resolve `cwd` to its canonical checkout identity, preferring the memo.
///
/// [`Repos::checkout_identity`] spawns `git rev-parse` twice, and reconcile
/// runs on *every* workspace chat row change — including the `branch` and
/// `checkoutId` writes [`sync_entry`] itself makes. Resolving fresh on every
/// pass had two failure modes that combined into a runaway loop on a checkout
/// whose captures cost seconds of CPU:
///
/// 1. every publish fanned out into a storm of `git` spawns (fd pressure);
/// 2. one transient spawn failure (e.g. EMFILE) made the chat ungroupable, so
///    reconcile tore its entry down and re-added it on the next pass — and
///    every re-add kicks a full capture, whose row writes trigger the next
///    reconcile. Back-to-back `git diff` forever, on an idle tree.
///
/// So: chat-watch reconciles reuse the memo (no git at all), the repair tick
/// revalidates (`fresh`) — spawning git only when the memo's [`IdentityStamp`]
/// no longer matches the filesystem — and a failed fresh resolve keeps the
/// memo unless the directory is actually gone.
async fn resolve_identity(
    inner: &Arc<DiffSyncInner>,
    cwd: &str,
    fresh: bool,
) -> Option<CheckoutIdentity> {
    let memo = lock(&inner.identities).get(cwd).cloned();
    if let Some(memo) = &memo
        && (!fresh
            || memo.stamp.as_ref().is_some_and(|stamp| {
                IdentityStamp::read(cwd, &memo.identity).as_ref() == Some(stamp)
            }))
    {
        return Some(memo.identity.clone());
    }
    match inner.repos.checkout_identity(Path::new(cwd)).await {
        Ok(identity) => {
            let stamp = IdentityStamp::read(cwd, &identity);
            lock(&inner.identities).insert(
                cwd.to_string(),
                MemoizedIdentity {
                    identity: identity.clone(),
                    stamp,
                },
            );
            Some(identity)
        }
        Err(err) => {
            if !Path::new(cwd).exists() {
                lock(&inner.identities).remove(cwd);
                tracing::debug!(cwd = %cwd, error = %err, "diff-sync: checkout gone");
                return None;
            }
            let cached = memo.map(|memo| memo.identity);
            match &cached {
                Some(_) => tracing::debug!(cwd = %cwd, error = %err,
                    "diff-sync: identity resolve failed; keeping memoized identity"),
                None => tracing::debug!(cwd = %cwd, error = %err, "diff-sync: not a checkout"),
            }
            cached
        }
    }
}

async fn reconcile(inner: &Arc<DiffSyncInner>, chats: Vec<Chat>, fresh: bool) {
    // One pass at a time — see `reconcile_gate`.
    let _gate = inner.reconcile_gate.lock().await;
    // Group this device's cwd-bearing chats by canonical checkout identity.
    let mut groups: HashMap<String, (CheckoutIdentity, Vec<Chat>)> = HashMap::new();
    // Dedupe resolution within this pass — many chats share one checkout.
    let mut resolved: HashMap<String, Option<CheckoutIdentity>> = HashMap::new();
    // This device's archived chats nobody has open: never grouped (no entry,
    // watcher, capture or sidecar), and an entry left holding only these is
    // dropped below. An archived chat with a viewer is grouped like any other.
    let interested = interested_chats(inner);
    let mut archived: HashSet<String> = HashSet::new();
    for chat in chats {
        if chat.device_id != inner.device_id {
            continue;
        }
        if chat.archived && !interested.contains(&chat.id) {
            archived.insert(chat.id);
            continue;
        }
        let Some(cwd) = chat.cwd.clone() else {
            continue;
        };
        let identity = match resolved.get(&cwd) {
            Some(identity) => identity.clone(),
            None => {
                let identity = resolve_identity(inner, &cwd, fresh).await;
                resolved.insert(cwd.clone(), identity.clone());
                identity
            }
        };
        let Some(identity) = identity else {
            continue;
        };
        // Stamp the row's checkoutId so every device groups this chat correctly.
        if chat.checkout_id.as_deref() != Some(identity.id.as_str())
            && let Err(err) = inner.workspace.set_chat_checkout(&chat.id, &identity.id)
        {
            tracing::debug!(chat = %chat.id, error = %err, "diff-sync: checkoutId write failed");
        }
        groups
            .entry(identity.id.clone())
            .or_insert_with(|| (identity, Vec::new()))
            .1
            .push(chat);
    }
    // A turn snapshot holds an object store on disk: keep it only for chats
    // that can still show "Latest turn". Judged on the current rows, not this
    // pass's (possibly older) list, so a chat created meanwhile keeps its own.
    {
        let current = inner.workspace.watch_chats();
        let current = current.borrow();
        let live: HashSet<&str> = current
            .iter()
            .filter(|chat| !chat.archived || interested.contains(&chat.id))
            .map(|chat| chat.id.as_str())
            .collect();
        lock(&inner.turn_trees).retain(|chat_id, _| live.contains(chat_id.as_str()));
    }

    // Close entries whose checkout has had no chats for a full grace period;
    // drop their published diff. A single pass that misses a checkout only
    // *marks* it — teardown is expensive to undo (re-add kicks a capture), so
    // absence must be sustained before we act on it. Archiving is not a flap:
    // an entry whose every chat is now an archived row goes immediately.
    let removed: Vec<String> = {
        let now = std::time::Instant::now();
        let mut entries = lock(&inner.entries);
        let mut removed = Vec::new();
        for (id, entry) in entries.iter() {
            if groups.contains_key(id) {
                let was_orphaned = lock(&entry.orphaned_since).take().is_some();
                if was_orphaned && entry.skipped_while_orphaned.swap(false, Ordering::AcqRel) {
                    let _ = entry.kick_tx.send(()); // catch up on what we skipped
                }
                continue;
            }
            let archived_away = {
                let held = lock(&entry.chats);
                !held.is_empty() && held.iter().all(|chat| archived.contains(&chat.id))
            };
            if archived_away {
                removed.push(id.clone());
                continue;
            }
            let mut orphaned = lock(&entry.orphaned_since);
            match *orphaned {
                None => *orphaned = Some(now),
                Some(since) if now.duration_since(since) >= inner.orphan_grace => {
                    removed.push(id.clone());
                }
                Some(_) => {}
            }
        }
        for id in &removed {
            entries.remove(id); // dropping the entry drops watchers + ends its task
        }
        removed
    };
    if !removed.is_empty() {
        publish_watch(inner);
    }

    // Update surviving entries; add new ones (initial sync kicked on add).
    for (checkout_id, (identity, chats)) in groups {
        let existing = lock(&inner.entries).get(&checkout_id).cloned();
        match existing {
            Some(entry) => {
                let has_new = {
                    let mut held = lock(&entry.chats);
                    let previous: HashSet<String> = held.iter().map(|c| c.id.clone()).collect();
                    let has_new = chats.iter().any(|c| !previous.contains(&c.id));
                    *held = chats;
                    has_new
                };
                if has_new {
                    let _ = entry.kick_tx.send(()); // new chat needs a sidecar now
                }
            }
            None => add_entry(inner, identity, chats),
        }
    }
}

/// True if `root`'s directory tree exceeds [`MAX_WATCH_DIRS`] — the signal that
/// a live recursive watch would cost more than it's worth. Bounded BFS: stops
/// the moment the budget is blown (never walks a whole node_modules), skips
/// symlinks (a symlinked dep cycle must not send this into a spin), and treats
/// unreadable dirs as leaves. `.git` internal churn is real diff signal, so it
/// counts toward the budget rather than being skipped.
fn exceeds_watch_budget(root: &Path) -> bool {
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut seen = 0usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type()` on the dirent does NOT follow symlinks — a symlinked
            // directory reports as a symlink and is skipped, so cyclic deps
            // (pnpm/npm) can't blow up the walk.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                seen += 1;
                if seen > MAX_WATCH_DIRS {
                    return true;
                }
                queue.push_back(entry.path());
            }
        }
    }
    false
}

/// Which paths a checkout's entry live-watches.
///
/// A recursive `notify` watch installs one OS watch per subdirectory and has
/// no way to prune subtrees. On a checkout carrying big dependency trees
/// (node_modules, target/, vendored deps) that is tens of thousands of
/// watches: the watcher thread pegs a core just maintaining them — even with
/// the tree completely idle — which starved a real device's whole async
/// runtime (presence heartbeats and IPC stalled; it showed permanently
/// offline). So the worktree root is only watched when it fits
/// [`MAX_WATCH_DIRS`].
///
/// An over-budget root still gets its GIT DIR watched (a bounded tree —
/// objects fanout + refs): commits, index moves, and branch switches then
/// refresh the diff instantly, and only raw working-tree edits wait for the
/// repair tick. Without this, a commit right after an edit left the pane
/// showing the pre-commit diff for up to two minutes (user report — the
/// dev checkout's target/ alone blows the budget). Linked worktrees keep
/// their git dir outside the root, so it rides along whenever the root's
/// watch doesn't already cover it.
fn watch_targets(identity: &CheckoutIdentity) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    let root_fits = !exceeds_watch_budget(&identity.root);
    if root_fits {
        targets.push(identity.root.clone());
    } else {
        tracing::info!(path = %identity.root.display(),
            "diff-sync: tree too large to watch live; watching the git dir, edits ride the repair tick");
    }
    let git_covered = root_fits && identity.git_dir.starts_with(&identity.root);
    if !git_covered && !exceeds_watch_budget(&identity.git_dir) {
        targets.push(identity.git_dir.clone());
    }
    targets
}

fn add_entry(inner: &Arc<DiffSyncInner>, identity: CheckoutIdentity, chats: Vec<Chat>) {
    let (kick_tx, kick_rx) = mpsc::unbounded_channel();
    let entry = Arc::new(CheckoutEntry {
        identity,
        chats: Mutex::new(chats),
        checksum: Mutex::new(None),
        orphaned_since: Mutex::new(None),
        skipped_while_orphaned: AtomicBool::new(false),
        kick_tx: kick_tx.clone(),
        watchers: Mutex::new(Vec::new()),
        signals: Arc::new(WatchSignals::new()),
        gate: Mutex::new(None),
    });
    lock(&inner.entries).insert(entry.identity.id.clone(), entry.clone());
    tokio::spawn(entry_task(
        Arc::downgrade(inner),
        Arc::downgrade(&entry),
        kick_rx,
        inner.cancel.clone(),
    ));
    let _ = kick_tx.send(()); // initial snapshot — must not wait for watchers

    // Watcher setup is genuinely blocking: the budget walk reads up to
    // MAX_WATCH_DIRS directory entries and FSEvents stream registration stalls
    // for seconds when fseventsd is contended. Doing it inline starved the whole
    // runtime (workspace watches, presence) whenever entries were (re)built, so
    // it runs on the blocking pool and attaches to the entry when ready. Events
    // occurring before attachment are covered by the initial sync; the hub's
    // rescan when each watch goes live closes the capture→attach gap.
    let weak = Arc::downgrade(&entry);
    tokio::task::spawn_blocking(move || {
        let Some(entry) = weak.upgrade() else {
            return; // entry removed before watchers were ready
        };
        let watchers = build_watchers(
            &FsWatchHub::global(),
            &entry.identity,
            &kick_tx,
            &entry.signals,
        );
        *lock(&entry.watchers) = watchers;
    });
}

/// Recursive watches on the worktree root (budget permitting) and the git
/// dir — HEAD/index churn and file edits both land here. Registered on the
/// shared [`FsWatchHub`], so a checkout costs OS watches but no thread of its
/// own; the hub's rescan signals (watch went live, events possibly lost) kick
/// like any change. Failures are fine: the initial + repair sync still keep
/// the snapshot correct. Blocking (budget walk) — call from the blocking pool.
fn build_watchers(
    hub: &Arc<FsWatchHub>,
    identity: &CheckoutIdentity,
    kick_tx: &mpsc::UnboundedSender<()>,
    signals: &Arc<WatchSignals>,
) -> Vec<FsWatch> {
    let mut watchers = Vec::new();
    for target in watch_targets(identity) {
        let tx = kick_tx.clone();
        let signals = signals.clone();
        if target == identity.root {
            signals.covers_worktree.store(true, Ordering::Release);
        }
        watchers.push(
            hub.watch(&target, notify::RecursiveMode::Recursive, move |event| {
                if is_checkout_change(event) {
                    // Count before kicking: the tick must never see a quiet
                    // watcher while a kick is already on its way.
                    signals.events.fetch_add(1, Ordering::AcqRel);
                    let _ = tx.send(());
                }
            }),
        );
    }
    watchers
}

/// Whether a watch event can change the checkout's diff. Opens and reads are
/// not changes — and the sync's own git runs open files under both watched
/// roots, so kicking on them would re-sync every checkout forever. A write
/// still arrives as a modify event before its close.
fn is_checkout_change(event: &notify::Event) -> bool {
    !matches!(event.kind, notify::EventKind::Access(_))
}

/// Per-checkout task: trailing-debounce fs kicks, then compute + publish. Runs
/// syncs sequentially — kicks during a sync accumulate and trigger another pass.
async fn entry_task(
    inner: Weak<DiffSyncInner>,
    entry: Weak<CheckoutEntry>,
    mut kick_rx: mpsc::UnboundedReceiver<()>,
    cancel: CancellationToken,
) {
    while kick_rx.recv().await.is_some() {
        // Trailing debounce: wait for the burst to settle.
        loop {
            match tokio::time::timeout(WATCH_DEBOUNCE, kick_rx.recv()).await {
                Ok(Some(())) => continue,
                Ok(None) => return, // entry closed mid-burst
                Err(_) => break,
            }
        }
        let (Some(inner), Some(entry)) = (inner.upgrade(), entry.upgrade()) else {
            return;
        };
        // No chats (deleted / moved away; teardown pending the orphan grace):
        // nobody consumes this checkout's diff, so skip the capture — repair
        // ticks and fs churn included. Reconcile re-kicks if chats return.
        if lock(&entry.orphaned_since).is_some() {
            entry.skipped_while_orphaned.store(true, Ordering::Release);
            continue;
        }
        // The upgraded Arc would let a sync outlive shutdown — race the token
        // so an in-flight sidecar POST is dropped, not completed.
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = sync_entry(&inner, &entry) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot + publish
// ---------------------------------------------------------------------------

async fn sync_entry(inner: &Arc<DiffSyncInner>, entry: &Arc<CheckoutEntry>) {
    // Events counted before the capture starts are covered by it; later ones
    // leave the entry non-quiet, so the next repair tick re-checks it.
    let events = entry.signals.events.load(Ordering::Acquire);
    let prior = lock(&entry.gate).clone();
    let (snapshot, gate) =
        match capture_gated(&inner.repos, &entry.identity.root, None, prior.as_ref()).await {
            Ok(Gated::Captured(snapshot, gate)) => (*snapshot, gate),
            Ok(Gated::Unchanged) => {
                entry
                    .signals
                    .captured_events
                    .store(events, Ordering::Release);
                return; // the tree is as last published
            }
            Err(err) => {
                tracing::debug!(checkout = %entry.identity.root.display(), error = %err,
                    "diff-sync: capture failed");
                *lock(&entry.gate) = None;
                git_status::publish(inner, entry, Vec::new(), false);
                return;
            }
        };
    *lock(&entry.gate) = gate;
    entry
        .signals
        .captured_events
        .store(events, Ordering::Release);

    if let Some((files, complete)) = &snapshot.git_status {
        git_status::publish(inner, entry, files.clone(), *complete);
    }
    if lock(&entry.checksum).as_deref() == Some(snapshot.checksum.as_str()) {
        return; // unchanged — publish nothing
    }
    *lock(&entry.checksum) = Some(snapshot.checksum.clone());

    let diff = CheckoutDiff {
        checkout_id: entry.identity.id.clone(),
        device_id: inner.device_id.clone(),
        cwd: entry.identity.root.to_string_lossy().to_string(),
        patch: snapshot.patch.clone(),
        files: snapshot.files.clone(),
        additions: snapshot.additions,
        deletions: snapshot.deletions,
        truncated: snapshot.truncated,
        checksum: snapshot.checksum.clone(),
        updated_at: chrono::Utc::now(),
    };
    {
        let entries = lock(&inner.entries);
        if !entries.contains_key(&entry.identity.id) {
            return; // closed while computing
        }
    }
    publish_watch_with(inner, Some(diff));

    // Latest-only sidecar to every syncing chat's session DO slot.
    let chats = lock(&entry.chats).clone();
    // The legacy s2 sidecar route is closed to Cloud runner tokens (and no
    // current client reads it), so a Cloud machine doesn't post it.
    if let Some(edge) = inner
        .edge
        .as_ref()
        .filter(|_| !crate::runner::is_cloud_platform())
    {
        for chat in &chats {
            let sidecar = DiffSidecar {
                chat_id: chat.id.clone(),
                device_id: inner.device_id.clone(),
                checkout_path: entry.identity.root.to_string_lossy().to_string(),
                branch: Some(snapshot.branch.clone()),
                head_sha: snapshot.head_sha.clone(),
                patch: snapshot.patch.clone(),
                files: snapshot.files.clone(),
                additions: snapshot.additions,
                deletions: snapshot.deletions,
                truncated: snapshot.truncated,
                published_at: chrono::Utc::now().timestamp_millis(),
            };
            let url = format!("{}/diff/{}", edge.url.trim_end_matches('/'), chat.id);
            // Fresh bearer per request — never the boot-time snapshot.
            let Ok(bearer) = edge.bearer().await else {
                tracing::debug!(chat = %chat.id, "diff-sync: sidecar skipped (token unavailable)");
                continue;
            };
            let result = inner
                .http
                .post(&url)
                .bearer_auth(&bearer)
                .json(&sidecar)
                .send()
                .await;
            match result {
                Ok(response) if !response.status().is_success() => {
                    tracing::debug!(chat = %chat.id, status = %response.status(),
                        "diff-sync: sidecar publish rejected");
                }
                Err(err) => {
                    tracing::debug!(chat = %chat.id, error = %err, "diff-sync: sidecar publish failed");
                }
                Ok(_) => {}
            }
        }
    }
}

/// Re-emit the watch channel from the current entries' cached diffs, replacing (or
/// inserting) `updated`.
fn publish_watch_with(inner: &Arc<DiffSyncInner>, updated: Option<CheckoutDiff>) {
    let live: HashSet<String> = lock(&inner.entries).keys().cloned().collect();
    inner.diffs_tx.send_modify(|diffs| {
        diffs.retain(|d| live.contains(&d.checkout_id));
        if let Some(updated) = updated {
            match diffs
                .iter_mut()
                .find(|d| d.checkout_id == updated.checkout_id)
            {
                Some(slot) => *slot = updated,
                None => diffs.push(updated),
            }
        }
        diffs.sort_by(|a, b| a.checkout_id.cmp(&b.checkout_id));
    });
}

fn publish_watch(inner: &Arc<DiffSyncInner>) {
    publish_watch_with(inner, None);
    let live: HashSet<String> = lock(&inner.entries).keys().cloned().collect();
    inner.statuses_tx.send_if_modified(|statuses| {
        let before = statuses.len();
        statuses.retain(|status| live.contains(&status.checkout_id));
        before != statuses.len()
    });
}

/// The repair tick: revalidate every memoized identity (git is spawned only
/// for a cwd whose [`IdentityStamp`] moved), then re-check each checkout the
/// live watcher cannot vouch for. A healthy watcher that has reported nothing
/// since the last capture costs no git at all.
async fn repair_pass(inner: &Arc<DiffSyncInner>, chats: Vec<Chat>) {
    reconcile(inner, chats, true).await;
    let entries: Vec<Arc<CheckoutEntry>> = lock(&inner.entries).values().cloned().collect();
    for entry in entries {
        entry.repair_kick();
    }
}

/// Chat-watch follower + repair tick. Holds only weak handles so dropping the
/// service tears the loop down; the token ends it eagerly on shutdown.
async fn diff_sync_task(
    inner: Weak<DiffSyncInner>,
    mut chats_rx: watch::Receiver<Vec<Chat>>,
    interest_changed: Arc<Notify>,
    cancel: CancellationToken,
) {
    let mut repair = tokio::time::interval(REPAIR_INTERVAL);
    repair.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    repair.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            changed = chats_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let Some(inner) = inner.upgrade() else { break };
                let chats = chats_rx.borrow_and_update().clone();
                // Memoized identities only: chat rows change constantly (the
                // sync itself writes them) and must never fan out into git.
                reconcile(&inner, chats, false).await;
            }
            // An archived chat gained its first viewer or its interest lapsed.
            _ = interest_changed.notified() => {
                let Some(inner) = inner.upgrade() else { break };
                let chats = chats_rx.borrow().clone();
                reconcile(&inner, chats, false).await;
            }
            _ = repair.tick() => {
                let Some(inner) = inner.upgrade() else { break };
                let chats = chats_rx.borrow().clone();
                repair_pass(&inner, chats).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Diff capture (exposed for tests)
// ---------------------------------------------------------------------------

struct Capture {
    stdout: Vec<u8>,
    truncated: bool,
}

/// Run git capturing stdout under a hard byte ceiling — the child is killed once
/// the cap is hit, so an arbitrarily large repository diff never buffers fully.
///
/// Not an `async fn`: a wrapping future would add a poll frame, and a copy of
/// the 64 KiB read buffer, to every capture on the RPC worker's stack.
fn capture_git<'a, S: AsRef<OsStr>>(
    cwd: &'a Path,
    args: &'a [S],
    max_bytes: usize,
) -> impl Future<Output = Result<Capture, EngineError>> + 'a {
    capture_git_in(cwd, args, max_bytes, None)
}

/// [`capture_git`] that can also see a turn snapshot's objects, which exist
/// only in that snapshot's store.
async fn capture_git_in<S: AsRef<OsStr>>(
    cwd: &Path,
    args: &[S],
    max_bytes: usize,
    snapshot: Option<&TreeSnapshot>,
) -> Result<Capture, EngineError> {
    note_git_spawn(Some(cwd));
    let mut cmd = tokio::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x08000000);
    }
    cmd.arg("-C").arg(cwd).args(args);
    if let Some(snapshot) = snapshot {
        snapshot.apply_to(&mut cmd);
    }
    // Read-only commands: a caller that gives up (a timed-out snapshot, a
    // dropped request) takes its git child down with it.
    cmd.kill_on_drop(true);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::Other(format!("git spawn failed: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::Other("git stdout unavailable".into()))?;
    let mut out: Vec<u8> = Vec::new();
    // Keep the read buffer out of nested async state: inline storage grows
    // every caller's future and can overflow a standard RPC worker's stack.
    let mut buf = vec![0u8; 64 * 1024];
    let mut truncated = false;
    loop {
        let n = stdout
            .read(&mut buf)
            .await
            .map_err(|e| EngineError::Other(format!("git read failed: {e}")))?;
        if n == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(out.len());
        if n > remaining {
            out.extend_from_slice(&buf[..remaining]);
            truncated = true;
            let _ = child.start_kill();
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| EngineError::Other(format!("git wait failed: {e}")))?;
    if !output.status.success() && !truncated {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(EngineError::Other(if message.is_empty() {
            format!("git exited {}", output.status)
        } else {
            format!("git: {message}")
        }));
    }
    Ok(Capture {
        stdout: out,
        truncated,
    })
}

fn split_z(value: &[u8]) -> Vec<String> {
    value
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect()
}

/// Git paths are arbitrary bytes on Unix, while the diff protocol exposes
/// paths as UTF-8 strings. Treat an unrepresentable path as a partial snapshot
/// so destructive actions cannot operate on a file the UI could not faithfully
/// display or checksum.
fn has_non_utf8_status_path(value: &[u8]) -> bool {
    let records: Vec<&[u8]> = value
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut i = 0usize;
    while i < records.len() {
        let record = records[i];
        i += 1;
        if record.len() < 3 || record[2] != b' ' {
            continue;
        }
        if std::str::from_utf8(&record[3..]).is_err() {
            return true;
        }
        if (record.starts_with(b"R") || record.starts_with(b"C")) && i < records.len() {
            if std::str::from_utf8(records[i]).is_err() {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// Summary status for a `--name-status` / `--raw` change letter.
fn status_name(code: char) -> &'static str {
    match code {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'U' => "unmerged",
        _ => "modified",
    }
}

fn parse_name_status(value: &[u8]) -> Vec<DiffFileSummary> {
    let fields = split_z(value);
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < fields.len() {
        let raw = fields[i].clone();
        i += 1;
        let code = raw.chars().next().unwrap_or('M');
        let Some(first) = fields.get(i).cloned() else {
            break;
        };
        i += 1;
        let renamed = code == 'R' || code == 'C';
        let second = if renamed {
            let s = fields.get(i).cloned();
            i += 1;
            s
        } else {
            None
        };
        out.push(DiffFileSummary {
            path: second.clone().unwrap_or_else(|| first.clone()),
            old_path: second.is_some().then_some(first),
            status: status_name(code).to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    out
}

fn apply_numstat(files: &mut [DiffFileSummary], value: &[u8]) {
    // With -z, a rename record is `adds<TAB>dels<TAB><NUL>old<NUL>new<NUL>`.
    let records: Vec<String> = value
        .split(|b| *b == 0)
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    let mut i = 0usize;
    while i < records.len() {
        let record = &records[i];
        if record.is_empty() {
            i += 1;
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let adds = parts.next().unwrap_or_default().to_string();
        let dels = parts.next().unwrap_or_default().to_string();
        let inline_path = parts.next().unwrap_or_default().to_string();
        let path = if inline_path.is_empty() {
            // Rename: the next two records are old, new.
            let new_path = records.get(i + 2).cloned().unwrap_or_default();
            i += 2;
            new_path
        } else {
            inline_path
        };
        i += 1;
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.additions = adds.parse().unwrap_or(0);
            file.deletions = dels.parse().unwrap_or(0);
            file.binary = adds == "-" || dels == "-";
        }
    }
}

fn quote_patch_path(path: &str) -> String {
    if path
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        serde_json::to_string(path).unwrap_or_else(|_| format!("\"{path}\""))
    } else {
        path.to_string()
    }
}

/// Synthesize a new-file hunk for an untracked file (git diff never shows them).
fn untracked_patch(path: &str, content: &str) -> String {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let body: String = lines
        .iter()
        .map(|line| format!("+{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let a = quote_patch_path(&format!("a/{path}"));
    let b = quote_patch_path(&format!("b/{path}"));
    format!(
        "diff --git {a} {b}\nnew file mode 100644\n--- /dev/null\n+++ {b}\n@@ -0,0 +1,{} @@\n{body}\n",
        lines.len()
    )
}

fn validate_diff_path(path: &str) -> Result<&Path, EngineError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(EngineError::Other("diff path escapes checkout".into()));
    }
    Ok(path)
}

fn decode_diff_source(
    bytes: Vec<u8>,
) -> Result<(Option<String>, Option<String>, bool), EngineError> {
    if bytes.contains(&0) {
        return Ok((None, None, true));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Ok((None, None, true)),
    };
    let hash = crate::repos::hex(&Sha256::digest(text.as_bytes()));
    Ok((Some(text), Some(hash), false))
}

async fn read_worktree_source(root: &Path, path: &Path) -> Result<Capture, EngineError> {
    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| EngineError::Other(format!("canonical checkout: {error}")))?;
    let full = root.join(path);
    let metadata = tokio::fs::symlink_metadata(&full)
        .await
        .map_err(|error| EngineError::Other(format!("read diff file metadata: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(EngineError::Other(
            "diff source is not a regular checkout file".into(),
        ));
    }
    let canonical = tokio::fs::canonicalize(&full)
        .await
        .map_err(|error| EngineError::Other(format!("canonical diff file: {error}")))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(EngineError::Other("diff source escapes checkout".into()));
    }
    if metadata.len() > MAX_DIFF_SOURCE_BYTES as u64 {
        return Ok(Capture {
            stdout: Vec::new(),
            truncated: true,
        });
    }
    let stdout = tokio::fs::read(&canonical)
        .await
        .map_err(|error| EngineError::Other(format!("read diff file: {error}")))?;
    Ok(Capture {
        stdout,
        truncated: false,
    })
}

async fn read_git_source(
    root: &Path,
    revision: &str,
    snapshot: Option<&TreeSnapshot>,
    path: &Path,
) -> Result<Capture, EngineError> {
    let spec = format!("{revision}:{}", path.to_string_lossy());
    capture_git_in(
        root,
        &["cat-file", "blob", &spec],
        MAX_DIFF_SOURCE_BYTES,
        snapshot,
    )
    .await
}

/// Read the exact old/new documents for one file in a previously captured diff.
/// Paths must come from that snapshot's file summary; callers still recheck the
/// snapshot checksum after this read to close the filesystem race.
pub async fn read_diff_file_text(
    root: &Path,
    base: &str,
    file: &DiffFileSummary,
) -> Result<DiffFileTextPair, EngineError> {
    read_diff_file_text_at(root, base, None, None, file).await
}

/// Read the exact old/new documents for one file in a diff between `base` and
/// an optional committed target. Without a target, the new source is the live
/// working tree; with one, both sources are immutable Git blobs. A `base` that
/// is a turn snapshot's tree needs that snapshot as `base_snapshot`: its
/// objects are not in the checkout.
pub(crate) async fn read_diff_file_text_at(
    root: &Path,
    base: &str,
    base_snapshot: Option<&TreeSnapshot>,
    target: Option<&str>,
    file: &DiffFileSummary,
) -> Result<DiffFileTextPair, EngineError> {
    let new_path = validate_diff_path(&file.path)?;
    let old_path = validate_diff_path(file.old_path.as_deref().unwrap_or(&file.path))?;

    let old = if file.status == "added" {
        None
    } else {
        Some(read_git_source(root, base, base_snapshot, old_path).await?)
    };
    let new = if file.status == "deleted" {
        None
    } else if let Some(target) = target {
        Some(read_git_source(root, target, None, new_path).await?)
    } else {
        Some(read_worktree_source(root, new_path).await?)
    };
    let truncated = old.as_ref().is_some_and(|source| source.truncated)
        || new.as_ref().is_some_and(|source| source.truncated);
    if truncated {
        return Ok(DiffFileTextPair {
            old_text: None,
            new_text: None,
            old_content_hash: None,
            new_content_hash: None,
            binary: false,
            truncated: true,
        });
    }
    let (old_text, old_content_hash, old_binary) = match old {
        Some(source) => decode_diff_source(source.stdout)?,
        None => (None, None, false),
    };
    let (new_text, new_content_hash, new_binary) = match new {
        Some(source) => decode_diff_source(source.stdout)?,
        None => (None, None, false),
    };
    let binary = old_binary || new_binary || file.binary;
    Ok(DiffFileTextPair {
        old_text: (!binary).then_some(old_text).flatten(),
        new_text: (!binary).then_some(new_text).flatten(),
        old_content_hash: (!binary).then_some(old_content_hash).flatten(),
        new_content_hash: (!binary).then_some(new_content_hash).flatten(),
        binary,
        truncated: false,
    })
}

/// Resolve the parent used as a commit diff's old side. Root commits compare
/// against Git's canonical empty tree.
pub(crate) async fn commit_diff_base(root: &Path, sha: &str) -> String {
    let parent_spec = format!("{sha}^");
    let parent = capture_git(root, &["rev-parse", "--verify", &parent_spec], 256)
        .await
        .map(|capture| String::from_utf8_lossy(&capture.stdout).trim().to_string())
        .unwrap_or_default();
    if parent.is_empty() {
        EMPTY_TREE_SHA.to_string()
    } else {
        parent
    }
}

pub async fn working_diff_base(root: &Path) -> Result<String, EngineError> {
    let head = capture_git(root, &["rev-parse", "--verify", "HEAD"], 256)
        .await
        .map(|capture| String::from_utf8_lossy(&capture.stdout).trim().to_string())
        .unwrap_or_default();
    Ok(if head.is_empty() {
        EMPTY_TREE_SHA.into()
    } else {
        head
    })
}

/// One bounded atomic snapshot: tracked diff vs HEAD (or the empty tree) with
/// renames, plus untracked files (via `git status --porcelain`, index untouched)
/// as synthesized new-file hunks. 3MiB patch cap with a `truncated` flag; sha256
/// checksum over branch ‖ head ‖ patch ‖ files ‖ truncated.
pub async fn capture_diff(repos: &Repos, root: &Path) -> Result<DiffSnapshot, EngineError> {
    capture_diff_against(repos, root, None).await
}

/// [`capture_diff`] with the diff base overridable: `None` keeps the
/// working-tree behavior (vs HEAD / the empty tree); `Some(committish)` diffs
/// the working tree against that base instead ("Branch changes" passes the
/// merge-base with the comparison ref). Untracked files synthesize as new
/// either way — they are new relative to any committed base.
pub async fn capture_diff_against(
    repos: &Repos,
    root: &Path,
    base_override: Option<&str>,
) -> Result<DiffSnapshot, EngineError> {
    match capture_gated(repos, root, base_override, None).await? {
        Gated::Captured(snapshot, _) => Ok(*snapshot),
        // No prior fingerprint was offered, so nothing can have matched it.
        Gated::Unchanged => Err(EngineError::Other(
            "ungated capture reported no change".into(),
        )),
    }
}

/// The result of [`capture_gated`].
enum Gated {
    /// The gate fingerprint matched the prior capture's: nothing to publish.
    Unchanged,
    /// A fresh snapshot, with the fingerprint to gate the next capture on
    /// (`None` when this capture cannot vouch for a later skip).
    Captured(Box<DiffSnapshot>, Option<GateState>),
}

/// [`capture_diff_against`] behind the D16 gate. One `git status` yields the
/// head oid, the branch and every changed path; its output plus the
/// `(mtime, size)` of the listed paths is the [`GateState`]. When that equals
/// `prior` the working tree cannot have moved, so nothing else is spawned. On
/// a change the tracked metadata (`--raw` + `--numstat`) and the patch run in
/// parallel: three spawns against the legacy six. Anything the fast path
/// cannot reproduce exactly (truncated output, an unparsable record, a failed
/// status) falls back to [`capture_diff_legacy`], so the result is always the
/// one the legacy path would give.
async fn capture_gated(
    repos: &Repos,
    root: &Path,
    base_override: Option<&str>,
    prior: Option<&GateState>,
) -> Result<Gated, EngineError> {
    // Taken before git reads anything: a file stamped at or after this may
    // still change within the timestamp's granularity (racy-git).
    let started = std::time::SystemTime::now();
    let Some(status) = capture_status(root).await else {
        let snapshot = capture_diff_legacy(repos, root, base_override).await?;
        return Ok(Gated::Captured(Box::new(snapshot), None));
    };
    let gate = if base_override.is_none() {
        let root = root.to_path_buf();
        let bytes = status.raw.clone();
        let paths = status.paths.clone();
        tokio::task::spawn_blocking(move || GateState::fingerprint(&root, &bytes, &paths, started))
            .await
            .ok()
    } else {
        None
    };
    if let (Some(prior), Some(gate)) = (prior, &gate)
        && prior.trusted
        && gate.trusted
        && prior.fingerprint == gate.fingerprint
    {
        return Ok(Gated::Unchanged);
    }

    let base: &str = match base_override {
        Some(base) => base,
        None if status.head.is_empty() => EMPTY_TREE_SHA,
        None => &status.head,
    };
    let meta_args = [
        "diff",
        "--raw",
        "--numstat",
        "-z",
        "--find-renames",
        base,
        "--",
    ];
    let patch_args = [
        "diff",
        "--no-ext-diff",
        "--no-color",
        "--find-renames",
        "--unified=3",
        base,
        "--",
    ];
    let (meta, tracked) = tokio::join!(
        capture_git(root, &meta_args, 2 * MAX_DIFF_METADATA_BYTES),
        capture_git(root, &patch_args, MAX_PATCH_BYTES),
    );
    let (meta, tracked) = (meta?, tracked?);
    let parsed = (!meta.truncated)
        .then(|| split_raw_numstat(&meta.stdout))
        .flatten();
    let Some((mut files, numstat)) = parsed else {
        let snapshot = capture_diff_legacy(repos, root, base_override).await?;
        return Ok(Gated::Captured(Box::new(snapshot), None));
    };
    apply_numstat(&mut files, numstat);
    let snapshot = finish_snapshot(
        root,
        status.branch,
        status.head,
        files,
        tracked,
        false,
        Capture {
            stdout: status.porcelain_v1,
            truncated: false,
        },
    )
    .await?;
    Ok(Gated::Captured(Box::new(snapshot), gate))
}

/// The pre-D16 capture: six sequential spawns. Kept as the fallback for what
/// the gated path declines to handle and as the oracle the equivalence tests
/// compare it against.
async fn capture_diff_legacy(
    repos: &Repos,
    root: &Path,
    base_override: Option<&str>,
) -> Result<DiffSnapshot, EngineError> {
    let head = capture_git(root, &["rev-parse", "--verify", "HEAD"], 256)
        .await
        .map(|c| String::from_utf8_lossy(&c.stdout).trim().to_string())
        .unwrap_or_default();
    let base: &str = match base_override {
        Some(base) => base,
        None if head.is_empty() => EMPTY_TREE_SHA,
        None => &head,
    };
    let branch = repos
        .current_branch(root)
        .await
        .unwrap_or_else(|_| "HEAD".into());

    let names = capture_git(
        root,
        &["diff", "--name-status", "-z", "--find-renames", base, "--"],
        MAX_DIFF_METADATA_BYTES,
    )
    .await?;
    let nums = capture_git(
        root,
        &["diff", "--numstat", "-z", "--find-renames", base, "--"],
        MAX_DIFF_METADATA_BYTES,
    )
    .await?;
    let tracked = capture_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            base,
            "--",
        ],
        MAX_PATCH_BYTES,
    )
    .await?;
    // Untracked listing via porcelain status; `--no-optional-locks` keeps this
    // read-only (a status-triggered index refresh would re-kick our own watcher).
    let status = capture_git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        2 * 1024 * 1024,
    )
    .await?;

    let mut files = parse_name_status(&names.stdout);
    apply_numstat(&mut files, &nums.stdout);
    finish_snapshot(
        root,
        branch,
        head,
        files,
        tracked,
        names.truncated || nums.truncated,
        status,
    )
    .await
}

/// Shared tail of both captures: fold the untracked files (from the porcelain
/// v1 `status`) into the tracked `files`/`patch`, then checksum.
async fn finish_snapshot(
    root: &Path,
    branch: String,
    head: String,
    mut files: Vec<DiffFileSummary>,
    tracked: Capture,
    metadata_truncated: bool,
    status: Capture,
) -> Result<DiffSnapshot, EngineError> {
    let mut patch = String::from_utf8_lossy(&tracked.stdout).to_string();
    let mut truncated = tracked.truncated || metadata_truncated || status.truncated;
    if has_non_utf8_status_path(&status.stdout) {
        truncated = true;
    }

    if tracked.truncated {
        let boundary = patch.rfind('\n').unwrap_or(0);
        patch.truncate(boundary);
        patch.push_str("\n# Zeron diff truncated\n");
    }

    // `?? path` records; rename records (`R  new\0old`) consume their extra field.
    let mut untracked: Vec<String> = Vec::new();
    let records = split_z(&status.stdout);
    let mut i = 0usize;
    while i < records.len() {
        let record = &records[i];
        i += 1;
        if record.len() < 3 {
            continue;
        }
        let (code, path) = record.split_at(2);
        if code.starts_with('R') || code.starts_with('C') {
            i += 1; // skip the origin-path field
        }
        if code == "??" {
            untracked.push(path.trim_start().to_string());
        }
    }
    untracked.sort();

    // Status now enumerates new directories as individual paths. Keep content
    // work bounded even when thousands of those files are binary or too large
    // to fit in the patch; their Git statuses remain complete independently.
    let mut untracked_budget = MAX_PATCH_BYTES;
    for path in untracked {
        if untracked_budget == 0 {
            truncated = true;
            break;
        }
        let full = root.join(&path);
        let binary;
        let mut additions = 0u32;
        let Ok(metadata) = tokio::fs::symlink_metadata(&full).await else {
            continue;
        };
        let size = metadata.len();
        if metadata.is_dir() {
            // Git keeps nested repositories atomic even with
            // `--untracked-files=all`. Surface the directory in the snapshot,
            // but never read through or clean it recursively.
            binary = true;
        } else if size > MAX_PATCH_BYTES as u64 {
            binary = true;
            truncated = true;
        } else {
            let content = if metadata.file_type().is_symlink() {
                // A Git symlink's content is its target path, never the target file.
                tokio::fs::read_link(&full)
                    .await
                    .map(|path| path.to_string_lossy().into_owned().into_bytes())
            } else {
                async {
                    let file = tokio::fs::File::open(&full).await?;
                    let mut bytes = Vec::new();
                    file.take(untracked_budget as u64 + 1)
                        .read_to_end(&mut bytes)
                        .await?;
                    Ok::<_, std::io::Error>(bytes)
                }
                .await
            };
            match content {
                Ok(bytes) => {
                    if bytes.len() > untracked_budget {
                        truncated = true;
                        break;
                    }
                    untracked_budget = untracked_budget.saturating_sub(bytes.len().max(1));
                    binary = bytes.contains(&0);
                    if !binary {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        additions = if text.is_empty() {
                            0
                        } else {
                            (text.split('\n').count() - usize::from(text.ends_with('\n'))) as u32
                        };
                        let addition = untracked_patch(&path, &text);
                        if patch.len() + addition.len() <= MAX_PATCH_BYTES {
                            if !patch.is_empty() && !patch.ends_with('\n') {
                                patch.push('\n');
                            }
                            patch.push_str(&addition);
                        } else {
                            truncated = true;
                        }
                    }
                }
                Err(_) => continue, // vanished between status and read
            }
        }
        files.push(DiffFileSummary {
            path,
            old_path: None,
            status: "added".to_string(),
            additions,
            deletions: 0,
            binary,
        });
    }

    let additions: u32 = files.iter().map(|f| f.additions).sum();
    let deletions: u32 = files.iter().map(|f| f.deletions).sum();
    let files_json = serde_json::to_string(&files)
        .map_err(|e| EngineError::Other(format!("diff files serialize: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(branch.as_bytes());
    hasher.update([0u8]);
    hasher.update(head.as_bytes());
    hasher.update([0u8]);
    hasher.update(patch.as_bytes());
    hasher.update([0u8]);
    hasher.update(files_json.as_bytes());
    hasher.update(if truncated { b"1" } else { b"0" });
    let checksum = crate::repos::hex(&hasher.finalize());

    Ok(DiffSnapshot {
        git_status: Some(git_status::parse(&status.stdout, status.truncated)),
        branch,
        head_sha: (!head.is_empty()).then_some(head),
        patch,
        files,
        additions,
        deletions,
        truncated,
        checksum,
    })
}

/// What the gate's single `git status` run reports.
struct StatusV2 {
    /// HEAD's commit; empty on an unborn branch (what `rev-parse --verify HEAD`
    /// would have printed, or failed to).
    head: String,
    /// Current branch, `HEAD` when detached (what `current_branch` returns).
    branch: String,
    /// The output as git printed it, for the gate fingerprint.
    raw: Vec<u8>,
    /// The same entries as `--porcelain=v1 -z --untracked-files=all` prints
    /// them, so the untracked scan and [`git_status::parse`] stay unchanged.
    porcelain_v1: Vec<u8>,
    /// Changed and untracked paths (destination only), to stat for the gate.
    paths: Vec<Vec<u8>>,
}

/// `None` when the status cannot be used as the legacy path's stand-in: git
/// failed, the output hit the byte ceiling, or a record is not understood.
async fn capture_status(root: &Path) -> Option<StatusV2> {
    let capture = capture_git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
        MAX_DIFF_METADATA_BYTES,
    )
    .await
    .ok()?;
    if capture.truncated {
        return None;
    }
    parse_status_v2(capture.stdout)
}

/// The path after the `spaces`th space of a v2 record (paths may contain spaces).
fn path_after_spaces(record: &[u8], spaces: usize) -> Option<&[u8]> {
    let mut seen = 0usize;
    for (i, byte) in record.iter().enumerate() {
        if *byte == b' ' {
            seen += 1;
            if seen == spaces {
                return Some(&record[i + 1..]).filter(|path| !path.is_empty());
            }
        }
    }
    None
}

fn parse_status_v2(raw: Vec<u8>) -> Option<StatusV2> {
    if !(raw.is_empty() || raw.ends_with(&[0])) {
        return None;
    }
    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    let mut head = None;
    let mut branch = None;
    let mut porcelain_v1 = Vec::new();
    let mut paths = Vec::new();
    let mut records = raw.split(|b| *b == 0).filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if let Some(oid) = record.strip_prefix(b"# branch.oid ") {
            head = Some(if oid == b"(initial)" {
                String::new()
            } else {
                lossy(oid)
            });
            continue;
        }
        if let Some(name) = record.strip_prefix(b"# branch.head ") {
            let name = lossy(name);
            branch = Some(if name.is_empty() || name == "(detached)" {
                "HEAD".to_string()
            } else {
                name
            });
            continue;
        }
        if record.starts_with(b"# ") {
            continue; // branch.upstream / branch.ab: fingerprinted, not parsed
        }
        let (xy, path, origin): (&[u8], &[u8], Option<&[u8]>) = match record {
            [b'?', b' ', path @ ..] if !path.is_empty() => (b"??", path, None),
            [kind @ (b'1' | b'2' | b'u'), b' ', _, _, b' ', ..] => {
                let spaces = match kind {
                    b'1' => 8,
                    b'2' => 9,
                    _ => 10,
                };
                let xy = &record[2..4];
                let path = path_after_spaces(record, spaces)?;
                let origin = if *kind == b'2' {
                    Some(records.next()?)
                } else {
                    None
                };
                (xy, path, origin)
            }
            _ => return None,
        };
        // v2 marks an unchanged column `.`, v1 a space.
        porcelain_v1.extend(xy.iter().map(|b| if *b == b'.' { b' ' } else { *b }));
        porcelain_v1.push(b' ');
        porcelain_v1.extend_from_slice(path);
        porcelain_v1.push(0);
        if let Some(origin) = origin {
            porcelain_v1.extend_from_slice(origin);
            porcelain_v1.push(0);
        }
        paths.push(path.to_vec());
    }
    Some(StatusV2 {
        head: head?,
        branch: branch?,
        raw,
        porcelain_v1,
        paths,
    })
}

/// Split `git diff --raw --numstat -z` into the file summaries (from the raw
/// records, `:modes shas STATUS NUL path [NUL path] NUL`) and the numstat
/// records that follow them. `None` for anything not cleanly NUL-terminated.
fn split_raw_numstat(value: &[u8]) -> Option<(Vec<DiffFileSummary>, &[u8])> {
    fn field<'a>(value: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let rest = &value[*pos..];
        let end = rest.iter().position(|b| *b == 0)?;
        *pos += end + 1;
        Some(&rest[..end])
    }
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).to_string();
    let mut files = Vec::new();
    let mut pos = 0usize;
    while value.get(pos) == Some(&b':') {
        let header = field(value, &mut pos)?;
        let code = char::from(*header.rsplit(|b| *b == b' ').next()?.first()?);
        let first = text(field(value, &mut pos)?);
        let second = if matches!(code, 'R' | 'C') {
            Some(text(field(value, &mut pos)?))
        } else {
            None
        };
        files.push(DiffFileSummary {
            path: second.clone().unwrap_or_else(|| first.clone()),
            old_path: second.is_some().then_some(first),
            status: status_name(code).to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    Some((files, &value[pos..]))
}

/// Fingerprint of a checkout's observable state, from one `git status`.
#[derive(Debug, Clone)]
struct GateState {
    fingerprint: [u8; 32],
    /// False when it cannot vouch for a later skip: a listed file was stamped
    /// within [`RACY_WINDOW`] (an edit that keeps size and timestamp could hide
    /// in the same tick), or the list passed [`MAX_GATE_PATHS`].
    trusted: bool,
}

impl GateState {
    /// `sha256(status output ‖ (path, mtime_ns, size, inode) per listed path)`.
    /// Blocking: stats every path.
    fn fingerprint(
        root: &Path,
        status: &[u8],
        paths: &[Vec<u8>],
        started: std::time::SystemTime,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(status);
        let mut trusted = paths.len() <= MAX_GATE_PATHS;
        for path in paths.iter().take(MAX_GATE_PATHS) {
            hasher.update([0u8]);
            hasher.update(path);
            match std::fs::symlink_metadata(root.join(path_argument(path))) {
                Ok(meta) => {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
                    let settled = meta
                        .modified()
                        .ok()
                        .and_then(|at| started.duration_since(at).ok())
                        .is_some_and(|age| age >= RACY_WINDOW);
                    trusted &= settled;
                    hasher.update([1u8]);
                    hasher.update(modified.unwrap_or_default().as_nanos().to_le_bytes());
                    hasher.update(meta.len().to_le_bytes());
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt as _;
                        hasher.update(meta.ino().to_le_bytes());
                    }
                }
                Err(_) => hasher.update([0u8]),
            }
        }
        Self {
            fingerprint: hasher.finalize().into(),
            trusted,
        }
    }
}

fn status_paths(status: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let records: Vec<&[u8]> = status
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let mut i = 0usize;
    while i < records.len() {
        let record = records[i];
        i += 1;
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let code = &record[..2];
        let path = record[3..].to_vec();
        if code == b"??" {
            untracked.push(path);
            continue;
        }
        if code == b"!!" {
            continue;
        }
        tracked.push(path);
        if (code.contains(&b'R') || code.contains(&b'C')) && i < records.len() {
            tracked.push(records[i].to_vec());
            i += 1;
        }
    }
    tracked.sort();
    tracked.dedup();
    untracked.sort();
    untracked.dedup();
    (tracked, untracked)
}

#[cfg(unix)]
fn path_argument(path: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    OsString::from_vec(path.to_vec())
}

#[cfg(not(unix))]
fn path_argument(path: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(path).into_owned())
}

/// Path bytes per git invocation. Windows caps a whole command line at 32,767
/// UTF-16 units (macOS/Linux at 1-2 MiB including the environment); a change
/// set past that must not fail to spawn halfway through a discard.
const MAX_PATH_ARGUMENT_BYTES: usize = 16 * 1024;

/// Split `paths` into consecutive batches of at most `budget` bytes (one NUL
/// per path included). A single path over budget still forms its own batch.
fn path_batches(paths: &[Vec<u8>], budget: usize) -> Vec<&[Vec<u8>]> {
    let mut batches = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0usize;
    for (index, path) in paths.iter().enumerate() {
        let cost = path.len() + 1;
        if index > start && bytes + cost > budget {
            batches.push(&paths[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += cost;
    }
    if start < paths.len() {
        batches.push(&paths[start..]);
    }
    batches
}

async fn run_git_for_paths(
    root: &Path,
    fixed_args: &[OsString],
    paths: &[Vec<u8>],
) -> Result<(), EngineError> {
    for batch in path_batches(paths, MAX_PATH_ARGUMENT_BYTES) {
        run_git_for_path_batch(root, fixed_args, batch).await?;
    }
    Ok(())
}

async fn run_git_for_path_batch(
    root: &Path,
    fixed_args: &[OsString],
    paths: &[Vec<u8>],
) -> Result<(), EngineError> {
    let mut command = tokio::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x08000000);
    }
    command
        .arg("-C")
        .arg(root)
        .arg("--literal-pathspecs")
        .args(fixed_args)
        .arg("--");
    for path in paths {
        command.arg(path_argument(path));
    }
    command.stdin(std::process::Stdio::null());
    let output = command
        .output()
        .await
        .map_err(|error| EngineError::Other(format!("git spawn failed: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    Err(EngineError::Other(if message.is_empty() {
        format!("git discard failed ({})", output.status)
    } else {
        format!("git: {message}")
    }))
}

/// Restore staged and unstaged tracked paths to the snapshot's HEAD, then ask
/// Git to remove only the exact untracked paths it reported. Ignored files are
/// excluded by porcelain status and `git clean` intentionally omits `-x`;
/// nested repositories and submodule contents are never recursively cleaned.
pub async fn discard_working_tree(
    repos: &Repos,
    root: &Path,
    expected_checksum: &str,
) -> Result<DiffSnapshot, EngineError> {
    let snapshot = capture_diff(repos, root).await?;
    let head = snapshot.head_sha.as_deref().ok_or_else(|| {
        EngineError::Other("cannot discard changes before the first commit".into())
    })?;
    if snapshot.truncated {
        return Err(EngineError::Other(
            "cannot safely discard a partial diff snapshot".into(),
        ));
    }
    if snapshot.checksum != expected_checksum {
        return Err(EngineError::Other(
            "working tree changed since the confirmation was opened".into(),
        ));
    }

    let status = capture_git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        2 * 1024 * 1024,
    )
    .await?;
    if status.truncated {
        return Err(EngineError::Other(
            "cannot safely discard a truncated working-tree status".into(),
        ));
    }
    let (tracked, untracked) = status_paths(&status.stdout);
    let is_directory = |path: &Vec<u8>| {
        std::fs::symlink_metadata(root.join(PathBuf::from(path_argument(path))))
            .is_ok_and(|metadata| metadata.is_dir())
    };
    if untracked.iter().any(is_directory) {
        return Err(EngineError::Other(
            "cannot discard an untracked nested repository safely".into(),
        ));
    }
    // Git tracks files and symlinks, so a tracked path that is a directory on
    // disk is a submodule or a file replaced by a directory. `git restore`
    // leaves submodule contents in place (the final check would then fail
    // after everything else was already discarded) and removes a replacing
    // directory wholesale, ignored files included. Refuse before mutating.
    if tracked.iter().any(is_directory) {
        return Err(EngineError::Other(
            "cannot discard submodule or directory changes safely".into(),
        ));
    }

    let restore_args = [
        OsString::from("restore"),
        OsString::from(format!("--source={head}")),
        OsString::from("--staged"),
        OsString::from("--worktree"),
    ];
    run_git_for_paths(root, &restore_args, &tracked).await?;
    let clean_args = [OsString::from("clean"), OsString::from("-fd")];
    run_git_for_paths(root, &clean_args, &untracked).await?;
    // `--untracked-files=all` gives exact files, so `git clean` can leave
    // their now-empty parent directories behind. Remove only empty ancestors;
    // ignored files or any concurrent writer make `remove_dir` stop safely.
    for path in &untracked {
        let full = root.join(PathBuf::from(path_argument(path)));
        let mut parent = full.parent();
        while let Some(directory) = parent {
            if directory == root || std::fs::remove_dir(directory).is_err() {
                break;
            }
            parent = directory.parent();
        }
    }

    let final_snapshot = capture_diff(repos, root).await?;
    if !final_snapshot.files.is_empty() || !final_snapshot.patch.trim().is_empty() {
        return Err(EngineError::Other(
            "some changes could not be discarded safely".into(),
        ));
    }
    Ok(final_snapshot)
}

/// Snapshot of one COMMIT's changes: first-parent (or the empty tree for a
/// root commit) diffed against the commit itself — the History pane's
/// per-commit tab. Commit-to-commit only: no working tree, no untracked
/// synthesis.
pub async fn capture_commit_diff(
    repos: &Repos,
    root: &Path,
    sha: &str,
) -> Result<DiffSnapshot, EngineError> {
    let base = commit_diff_base(root, sha).await;
    let branch = repos
        .current_branch(root)
        .await
        .unwrap_or_else(|_| "HEAD".into());
    let names = capture_git(
        root,
        &[
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            &base,
            sha,
            "--",
        ],
        2 * 1024 * 1024,
    )
    .await?;
    let nums = capture_git(
        root,
        &[
            "diff",
            "--numstat",
            "-z",
            "--find-renames",
            &base,
            sha,
            "--",
        ],
        2 * 1024 * 1024,
    )
    .await?;
    let tracked = capture_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            &base,
            sha,
            "--",
        ],
        MAX_PATCH_BYTES,
    )
    .await?;
    let mut files = parse_name_status(&names.stdout);
    apply_numstat(&mut files, &nums.stdout);
    let mut patch = String::from_utf8_lossy(&tracked.stdout).to_string();
    let truncated = tracked.truncated || names.truncated || nums.truncated;
    if tracked.truncated {
        let boundary = patch.rfind('\n').unwrap_or(0);
        patch.truncate(boundary);
        patch.push_str("\n# Comet diff truncated\n");
    }
    let additions: u32 = files.iter().map(|f| f.additions).sum();
    let deletions: u32 = files.iter().map(|f| f.deletions).sum();
    let files_json = serde_json::to_string(&files)
        .map_err(|e| EngineError::Other(format!("diff files serialize: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(branch.as_bytes());
    hasher.update([0u8]);
    hasher.update(sha.as_bytes());
    hasher.update([0u8]);
    hasher.update(patch.as_bytes());
    hasher.update([0u8]);
    hasher.update(files_json.as_bytes());
    hasher.update(if truncated { b"1" } else { b"0" });
    let checksum = crate::repos::hex(&hasher.finalize());
    Ok(DiffSnapshot {
        branch,
        head_sha: Some(sha.to_string()),
        git_status: None,
        patch,
        files,
        additions,
        deletions,
        truncated,
        checksum,
    })
}

/// `git merge-base <base_ref> HEAD` — the diff base for "Branch changes".
/// Errors when the ref is unknown or the histories are unrelated.
pub async fn merge_base(root: &Path, base_ref: &str) -> Result<String, EngineError> {
    let capture = capture_git(root, &["merge-base", base_ref, "HEAD"], 256).await?;
    let sha = String::from_utf8_lossy(&capture.stdout).trim().to_string();
    if sha.is_empty() {
        return Err(EngineError::Other(format!("no merge base with {base_ref}")));
    }
    Ok(sha)
}

/// "Latest turn" capture: tree-to-tree diff from the turn-start snapshot to a
/// fresh [`snapshot_tree`] of the current state. Both trees carry untracked
/// (unignored) files, so no synthesis is needed and a file that was already
/// untracked at turn start diffs correctly (the watch capture's synthesis
/// would misreport it as entirely new).
///
/// The fresh snapshot is layered on the turn's store, so it only writes the
/// files that changed since, and it is deleted again when this returns.
pub async fn capture_turn_diff(
    repos: &Repos,
    root: &Path,
    turn: &TreeSnapshot,
) -> Result<DiffSnapshot, EngineError> {
    let current = turn_snapshot::snapshot_tree_onto(repos, root, Some(turn)).await?;
    let turn_tree = turn.tree();
    let current_tree = current.tree();
    let head = capture_git(root, &["rev-parse", "--verify", "HEAD"], 256)
        .await
        .map(|c| String::from_utf8_lossy(&c.stdout).trim().to_string())
        .unwrap_or_default();
    let branch = repos
        .current_branch(root)
        .await
        .unwrap_or_else(|_| "HEAD".into());

    let names = capture_git_in(
        root,
        &[
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            turn_tree,
            current_tree,
            "--",
        ],
        2 * 1024 * 1024,
        Some(&current),
    )
    .await?;
    let nums = capture_git_in(
        root,
        &[
            "diff",
            "--numstat",
            "-z",
            "--find-renames",
            turn_tree,
            current_tree,
            "--",
        ],
        2 * 1024 * 1024,
        Some(&current),
    )
    .await?;
    let tracked = capture_git_in(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            turn_tree,
            current_tree,
            "--",
        ],
        MAX_PATCH_BYTES,
        Some(&current),
    )
    .await?;

    let mut files = parse_name_status(&names.stdout);
    apply_numstat(&mut files, &nums.stdout);
    let mut patch = String::from_utf8_lossy(&tracked.stdout).to_string();
    let truncated = tracked.truncated || names.truncated || nums.truncated;
    if tracked.truncated {
        let boundary = patch.rfind('\n').unwrap_or(0);
        patch.truncate(boundary);
        patch.push_str("\n# Zeron diff truncated\n");
    }

    let additions: u32 = files.iter().map(|f| f.additions).sum();
    let deletions: u32 = files.iter().map(|f| f.deletions).sum();
    let files_json = serde_json::to_string(&files)
        .map_err(|e| EngineError::Other(format!("diff files serialize: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(branch.as_bytes());
    hasher.update([0u8]);
    hasher.update(head.as_bytes());
    hasher.update([0u8]);
    hasher.update(patch.as_bytes());
    hasher.update([0u8]);
    hasher.update(files_json.as_bytes());
    hasher.update(if truncated { b"1" } else { b"0" });
    let checksum = crate::repos::hex(&hasher.finalize());

    Ok(DiffSnapshot {
        branch,
        head_sha: (!head.is_empty()).then_some(head),
        git_status: None,
        patch,
        files,
        additions,
        deletions,
        truncated,
        checksum,
    })
}

#[cfg(test)]
mod watch_budget_tests {
    use super::{
        CheckoutIdentity, MAX_WATCH_DIRS, exceeds_watch_budget, has_non_utf8_status_path,
        is_checkout_change, path_batches, watch_targets,
    };

    /// End to end through a real watcher: opening a file under a watched
    /// checkout raises access events on Linux, and before the filter each one
    /// kicked a sync whose own git reads raised more. A read must stay quiet;
    /// a write must still kick.
    #[tokio::test]
    async fn watcher_stays_quiet_on_reads_and_kicks_on_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = crate::fs_watch::FsWatchHub::new();
        let signals = std::sync::Arc::new(super::WatchSignals::new());
        let watchers =
            super::build_watchers(&hub, &identity(&root, &root.join(".git")), &tx, &signals);
        // Install now (the batch worker would within ~100 ms) and expect the
        // attach rescan's kick; then let registration settle and drop
        // anything it raised.
        hub.apply();
        assert!(watchers.iter().all(super::FsWatch::is_live));
        rx.try_recv().expect("going live kicks a sync");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        while rx.try_recv().is_ok() {}

        let _ = std::fs::read(root.join("a.txt")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(rx.try_recv().is_err(), "a read must not kick a sync");

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a write kicks a sync")
            .expect("kick channel open");
    }

    #[test]
    fn reads_do_not_kick_a_sync_but_writes_do() {
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind, RemoveKind,
        };
        for kind in [
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
        ] {
            assert!(!is_checkout_change(&notify::Event::new(kind)), "{kind:?}");
        }
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Remove(RemoveKind::File),
            EventKind::Any,
        ] {
            assert!(is_checkout_change(&notify::Event::new(kind)), "{kind:?}");
        }
    }

    #[test]
    fn path_batches_split_on_budget_and_keep_every_path() {
        let paths: Vec<Vec<u8>> = ["aaa", "bbb", "cc", "dddddddddd", "e"]
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        let batches = path_batches(&paths, 8);
        let sizes: Vec<usize> = batches.iter().map(|batch| batch.len()).collect();
        // "aaa\0bbb\0" fills 8; "cc\0" alone; the oversized path stands alone.
        assert_eq!(sizes, vec![2, 1, 1, 1]);
        assert_eq!(batches.concat(), paths);
        assert!(path_batches(&[], 8).is_empty());
    }

    #[test]
    fn non_utf8_status_paths_are_marked_unsafe_for_destructive_actions() {
        assert!(has_non_utf8_status_path(b"?? invalid-\xff.txt\0"));
        assert!(!has_non_utf8_status_path(b"?? valid.txt\0"));
    }

    #[test]
    fn small_tree_is_watchable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/a/b")).unwrap();
        std::fs::create_dir_all(root.join("src/c")).unwrap();
        std::fs::write(root.join("src/a/f.txt"), "x").unwrap();
        assert!(!exceeds_watch_budget(root));
    }

    #[test]
    fn budget_is_exceeded_and_probe_stays_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // One flat directory of MAX_WATCH_DIRS + 50 subdirs trips the budget;
        // the BFS must stop right after the threshold, not enumerate the rest.
        for i in 0..(MAX_WATCH_DIRS + 50) {
            std::fs::create_dir(root.join(format!("d{i}"))).unwrap();
        }
        assert!(exceeds_watch_budget(root));
    }

    fn identity(root: &std::path::Path, git_dir: &std::path::Path) -> CheckoutIdentity {
        CheckoutIdentity {
            id: "test".into(),
            root: root.to_path_buf(),
            git_dir: git_dir.to_path_buf(),
        }
    }

    #[test]
    fn small_checkout_watches_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git/refs")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        // The root watch covers the inline .git — no second watcher.
        assert_eq!(
            watch_targets(&identity(root, &root.join(".git"))),
            vec![root.to_path_buf()]
        );
    }

    #[test]
    fn linked_worktree_watches_root_and_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wt");
        let git_dir = tmp.path().join("main/.git/worktrees/wt");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&git_dir).unwrap();
        assert_eq!(
            watch_targets(&identity(&root, &git_dir)),
            vec![root.clone(), git_dir]
        );
    }

    #[test]
    fn over_budget_root_falls_back_to_the_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git/refs")).unwrap();
        // Blow the budget with a flat dependency-tree stand-in.
        for i in 0..(MAX_WATCH_DIRS + 50) {
            std::fs::create_dir(root.join(format!("d{i}"))).unwrap();
        }
        // Commits/index churn must still watch live even though edits can't.
        assert_eq!(
            watch_targets(&identity(root, &root.join(".git"))),
            vec![root.join(".git")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_dir_is_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("real/inner")).unwrap();
        // A self-referential symlink cycle must not send the walk into a spin.
        std::os::unix::fs::symlink(root.join("real"), root.join("real/inner/loop")).unwrap();
        assert!(!exceeds_watch_budget(root)); // terminates, under budget
    }
}

#[cfg(test)]
mod git_gating_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use super::{
        EMPTY_TREE_SHA, GateState, Gated, capture_diff_against, capture_diff_legacy, capture_gated,
        git_spawns_under, parse_status_v2, split_raw_numstat,
    };
    use crate::repos::Repos;

    fn git_out(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .output()
            .expect("git spawns")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_out(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(dir: &Path, name: &str, content: &[u8]) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Two commits of a few tracked files (text, binary, nested).
    fn base_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        write(dir, "a.txt", b"a\nb\nc\nd\ne\nf\ng\nh\n");
        write(dir, "b.txt", b"one\ntwo\n");
        write(dir, "old.txt", b"q1\nq2\nq3\nq4\nq5\nq6\n");
        write(dir, "bin.dat", b"\0\x01\x02binary");
        write(dir, "dir/c.txt", b"c1\nc2\nc3\nc4\n");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "first"]);
        write(dir, "a.txt", b"a\nb\nc\nd\ne\nf\ng\nh\ni\n");
        git(dir, &["commit", "-q", "-am", "second"]);
    }

    /// Every working-tree shape the capture has to reproduce, as
    /// `(name, checkout root)`.
    fn fixtures(tmp: &Path) -> Vec<(&'static str, PathBuf)> {
        let out = std::cell::RefCell::new(Vec::new());
        let fresh = |name: &'static str| {
            let dir = tmp.join(name);
            base_repo(&dir);
            out.borrow_mut().push((name, dir.clone()));
            dir
        };

        fresh("clean");

        let dir = fresh("modified");
        write(&dir, "a.txt", b"changed\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        write(&dir, "b.txt", b"one\ntwo\nstaged\n");
        git(&dir, &["add", "b.txt"]);
        write(&dir, "b.txt", b"one\ntwo\nstaged\nand more\n");
        std::fs::remove_file(dir.join("dir/c.txt")).unwrap();
        write(&dir, "bin.dat", b"\0\x01\x02different binary");

        let dir = fresh("untracked");
        write(&dir, "new file.txt", b"hello\nworld\n");
        write(&dir, "sub/deep/u.txt", b"no trailing newline");
        write(&dir, "empty.txt", b"");
        write(&dir, "blob.bin", b"\0\0\0");
        write(&dir, "quo\"te\\.txt", b"x\n");
        write(&dir, ".gitignore", b"ignored.log\n");
        write(&dir, "ignored.log", b"nope\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", dir.join("link")).unwrap();

        let dir = fresh("renamed");
        git(&dir, &["mv", "old.txt", "renamed name.txt"]);
        write(&dir, "renamed name.txt", b"q1\nq2\nq3\nq4\nq5\nq6\nq7\n");
        std::fs::rename(dir.join("dir/c.txt"), dir.join("dir/moved.txt")).unwrap();

        let dir = fresh("detached");
        git(&dir, &["checkout", "-q", "--detach", "HEAD~1"]);
        write(&dir, "b.txt", b"edited while detached\n");

        let dir = tmp.join("unborn");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "trunk"]);
        write(&dir, "staged.txt", b"s\n");
        git(&dir, &["add", "staged.txt"]);
        write(&dir, "loose.txt", b"l\n");
        out.borrow_mut().push(("unborn", dir));

        let main = fresh("worktree-main");
        let linked = tmp.join("worktree-linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        write(&linked, "b.txt", b"edited in the linked worktree\n");
        write(&linked, "extra.txt", b"extra\n");
        out.borrow_mut().push(("linked-worktree", linked));

        let dir = fresh("merge");
        git(&dir, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        write(&dir, "b.txt", b"side\n");
        git(&dir, &["commit", "-q", "-am", "side"]);
        git(&dir, &["checkout", "-q", "main"]);
        write(&dir, "b.txt", b"main\n");
        git(&dir, &["commit", "-q", "-am", "main"]);
        assert!(!git_out(&dir, &["merge", "side"]).status.success());
        write(&dir, "after-conflict.txt", b"x\n");

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let dir = fresh("non-utf8");
            std::fs::write(
                dir.join(std::ffi::OsStr::from_bytes(b"bad-\xff.txt")),
                "x\n",
            )
            .unwrap();
        }
        out.into_inner()
    }

    fn repos(tmp: &Path) -> Repos {
        Repos::new(&tmp.join("data"), "device")
    }

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[tokio::test]
    async fn gated_capture_equals_the_legacy_capture() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let repos = repos(&tmp_path);
        for (name, dir) in fixtures(&tmp_path) {
            let head = git_out(&dir, &["rev-parse", "--verify", "HEAD~1"]);
            let older = head
                .status
                .success()
                .then(|| String::from_utf8_lossy(&head.stdout).trim().to_string());
            let mut bases = vec![None, Some(EMPTY_TREE_SHA.to_string())];
            bases.extend(older.map(Some));
            for base in bases {
                let base = base.as_deref();
                let legacy = capture_diff_legacy(&repos, &dir, base).await;
                let gated = capture_diff_against(&repos, &dir, base).await;
                match (legacy, gated) {
                    (Ok(legacy), Ok(gated)) => {
                        assert_eq!(legacy, gated, "{name} against {base:?}");
                    }
                    (Err(legacy), Err(gated)) => {
                        // Same failure; wording may differ between the commands.
                        let _ = (legacy, gated);
                    }
                    (legacy, gated) => {
                        panic!("{name} against {base:?}: legacy {legacy:?} vs gated {gated:?}")
                    }
                }
            }
        }
    }

    /// The equivalence above would pass vacuously on empty snapshots.
    #[tokio::test]
    async fn fixtures_exercise_every_shape() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let repos = repos(&tmp_path);
        let mut shot = std::collections::HashMap::new();
        for (name, dir) in fixtures(&tmp_path) {
            shot.insert(
                name,
                capture_diff_against(&repos, &dir, None).await.unwrap(),
            );
        }
        assert!(shot["clean"].files.is_empty() && shot["clean"].head_sha.is_some());
        let statuses = |name: &str| -> Vec<String> {
            shot[name].files.iter().map(|f| f.status.clone()).collect()
        };
        assert!(statuses("modified").contains(&"deleted".to_string()));
        assert!(shot["modified"].files.iter().any(|f| f.binary));
        assert!(
            shot["untracked"]
                .files
                .iter()
                .any(|f| f.path == "new file.txt")
        );
        assert!(
            !shot["untracked"]
                .files
                .iter()
                .any(|f| f.path == "ignored.log")
        );
        assert!(statuses("renamed").contains(&"renamed".to_string()));
        assert_eq!(shot["detached"].branch, "HEAD");
        assert_eq!(shot["unborn"].head_sha, None);
        assert_eq!(shot["unborn"].branch, "trunk");
        assert_eq!(shot["linked-worktree"].branch, "feature");
        let (merge_status, _) = shot["merge"].git_status.clone().unwrap();
        assert!(
            merge_status
                .iter()
                .any(|f| f.worktree == zeron_proto::GitFileState::Unmerged)
        );
        #[cfg(unix)]
        assert!(shot["non-utf8"].truncated);
    }

    #[test]
    fn status_v2_is_translated_to_the_v1_listing() {
        let v2 = b"# branch.oid 78d33ff07ae8619f2e138856d1a0644a0cd1d47b\0# branch.head main\0\
# branch.upstream origin/main\0# branch.ab +1 -2\0\
1 .M N... 100644 100644 100644 aaaa bbbb a b.txt\0\
2 R. N... 100644 100644 100644 aaaa bbbb R100 new name\0old name\0\
u UU N... 100644 100644 100644 100644 aaaa bbbb cccc c.txt\0\
? un tracked\0"
            .to_vec();
        let parsed = parse_status_v2(v2).unwrap();
        assert_eq!(parsed.head, "78d33ff07ae8619f2e138856d1a0644a0cd1d47b");
        assert_eq!(parsed.branch, "main");
        assert_eq!(
            parsed.porcelain_v1,
            b" M a b.txt\0R  new name\0old name\0UU c.txt\0?? un tracked\0".to_vec()
        );
        assert_eq!(parsed.paths.len(), 4);
        // Unusable output is declined, never half-parsed.
        assert!(
            parse_status_v2(b"# branch.oid abc\0# branch.head main\0garbage\0".to_vec()).is_none()
        );
        assert!(parse_status_v2(b"# branch.oid abc\0# branch.head main".to_vec()).is_none());
        assert!(parse_status_v2(b"? only.txt\0".to_vec()).is_none());
    }

    #[test]
    fn raw_numstat_splits_at_the_first_numstat_record() {
        let out = b":100644 100644 aaa bbb M\0a.txt\0:100644 100644 aaa bbb R075\0old\0new\0\
1\t0\ta.txt\0" as &[u8];
        let (files, numstat) = split_raw_numstat(out).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].old_path.as_deref(), Some("old"));
        assert_eq!(files[1].status, "renamed");
        assert_eq!(numstat, b"1\t0\ta.txt\0");
        assert!(split_raw_numstat(b":100644 100644 aaa bbb M\0a.tx").is_none());
        assert_eq!(split_raw_numstat(b"").unwrap().0.len(), 0);
    }

    #[tokio::test]
    async fn gated_capture_spawns_three_git_processes_against_six() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let repos = repos(&tmp_path);
        let dir = tmp_path.join("modified");
        base_repo(&dir);
        write(&dir, "a.txt", b"changed\n");
        write(&dir, "u.txt", b"untracked\n");

        let before = git_spawns_under(&dir);
        capture_diff_legacy(&repos, &dir, None).await.unwrap();
        let legacy = git_spawns_under(&dir) - before;
        let before = git_spawns_under(&dir);
        capture_diff_against(&repos, &dir, None).await.unwrap();
        let gated = git_spawns_under(&dir) - before;
        assert_eq!((legacy, gated), (6, 3));
    }

    fn backdate(path: &Path, age: Duration) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    #[tokio::test]
    async fn unchanged_gate_skips_everything_but_the_status_run() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let repos = repos(&tmp_path);
        let dir = tmp_path.join("repo");
        base_repo(&dir);
        write(&dir, "a.txt", b"changed\n");
        write(&dir, "u.txt", b"untracked\n");
        let hour = Duration::from_secs(3600);
        backdate(&dir.join("a.txt"), hour);
        backdate(&dir.join("u.txt"), hour);

        let Gated::Captured(first, Some(gate)) =
            capture_gated(&repos, &dir, None, None).await.unwrap()
        else {
            panic!("first capture produces a snapshot and a gate");
        };
        assert!(gate.trusted);

        let before = git_spawns_under(&dir);
        let again = capture_gated(&repos, &dir, None, Some(&gate))
            .await
            .unwrap();
        assert!(matches!(again, Gated::Unchanged));
        assert_eq!(git_spawns_under(&dir) - before, 1, "only the gate's status");

        // Same size, same second: only the nanoseconds differ.
        write(&dir, "a.txt", b"CHANGED\n");
        let stamp = SystemTime::now() - hour + Duration::from_nanos(7);
        std::fs::File::options()
            .write(true)
            .open(dir.join("a.txt"))
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        let Gated::Captured(second, next) = capture_gated(&repos, &dir, None, Some(&gate))
            .await
            .unwrap()
        else {
            panic!("a same-size edit must be seen");
        };
        assert_ne!(first.checksum, second.checksum);
        assert!(next.is_some_and(|gate| gate.trusted));

        // A change in what git lists is seen as well.
        git(&dir, &["checkout", "--", "a.txt"]);
        let gate = match capture_gated(&repos, &dir, None, None).await.unwrap() {
            Gated::Captured(_, Some(gate)) => gate,
            _ => panic!("gate"),
        };
        std::fs::remove_file(dir.join("u.txt")).unwrap();
        assert!(matches!(
            capture_gated(&repos, &dir, None, Some(&gate))
                .await
                .unwrap(),
            Gated::Captured(..)
        ));
    }

    #[tokio::test]
    async fn freshly_written_files_never_vouch_for_a_skip() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let repos = repos(&tmp_path);
        let dir = tmp_path.join("repo");
        base_repo(&dir);
        write(&dir, "a.txt", b"just now\n");
        let Gated::Captured(_, Some(gate)) = capture_gated(&repos, &dir, None, None).await.unwrap()
        else {
            panic!("gate");
        };
        assert!(!gate.trusted, "an mtime inside the racy window");
        assert!(matches!(
            capture_gated(&repos, &dir, None, Some(&gate))
                .await
                .unwrap(),
            Gated::Captured(..)
        ));
    }

    #[test]
    fn gate_never_trusts_a_change_list_past_the_cap() {
        let tmp = scratch();
        let paths: Vec<Vec<u8>> = (0..super::MAX_GATE_PATHS + 1)
            .map(|i| format!("f{i}").into_bytes())
            .collect();
        let gate = GateState::fingerprint(tmp.path(), b"status", &paths, SystemTime::now());
        assert!(!gate.trusted);
    }

    async fn settled_spawns(dir: &Path) -> u64 {
        // Captures raise watch events of their own (index refreshes); wait
        // until the checkout has stopped spawning git.
        let mut last = git_spawns_under(dir);
        let mut calm = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while calm < 4 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "checkout never settled"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = git_spawns_under(dir);
            calm = if now == last { calm + 1 } else { 0 };
            last = now;
        }
        last
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_repair_tick_spawns_no_git_while_the_watcher_is_live() {
        let tmp = scratch();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = tmp_path.join("repo");
        base_repo(&dir);
        write(&dir, "a.txt", b"dirty\n");
        write(&dir, "u.txt", b"untracked\n");

        let core = crate::EngineCore::assemble(
            &tmp_path.join("data"),
            Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space("space", &core.device_id, &dir.to_string_lossy(), None, true)
            .unwrap();
        core.workspace
            .create_chat("chat", Some("space"), None, None, None)
            .unwrap();
        let sync = &core.diff_sync;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            sync.reconcile_now().await;
            let watched = sync.tracked_checkouts().iter().any(|(_, live)| *live > 0);
            if watched && !sync.watch_diffs().borrow().is_empty() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "checkout never tracked"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let idle = settled_spawns(&dir).await;
        assert!(idle > 0, "the initial capture spawned git");
        for _ in 0..3 {
            sync.repair_now().await;
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        assert_eq!(
            git_spawns_under(&dir),
            idle,
            "an idle, watched checkout costs no git at the repair tick"
        );

        // An edit still reaches the diff through the watcher.
        let before = sync.watch_diffs().borrow()[0].checksum.clone();
        write(&dir, "a.txt", b"edited again\n");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while sync.watch_diffs().borrow()[0].checksum == before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "edit never published"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        core.shutdown().await;
    }
}
