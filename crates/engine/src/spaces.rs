//! SpacesSync — owner-side upkeep of space rows (git presence) plus the
//! orphan-chat repair sweep.
//!
//! A space is a synced (device, folder) pair; the folder need NOT be a git
//! repo. This service watches the workspace `spaces` rows owned by THIS device
//! and keeps their `gitDetected`/`checkoutId`/`repositoryId` stamps truthful:
//!
//! - recheck on boot / when a space row is first observed;
//! - a non-recursive watch on the space folder (on the process-wide
//!   [`FsWatchHub`], so spaces add no watcher threads) — `.git` appearing or
//!   vanishing (git init / de-git) kicks a recheck;
//! - a slow 2-minute repair tick (native watchers coalesce/drop events).
//!
//! Stamps are written ONLY on change, so steady state never grows the oplog.
//! Remote devices read `space.git_detected` straight from the doc — branch
//! pickers and the diff sidebar gate on it with zero RPCs.
//!
//! The repair tick also runs the orphan sweep: a chat created concurrently
//! with a `deleteSpace` on another device can sync in after the cascade ran,
//! leaving a dangling `spaceId`. The HOST device deletes its own such chats
//! (writer discipline — we never touch other devices' rows). The sweep only
//! runs once the registry has applied a server state this boot
//! ([`WorkspaceHost::registry_synced`]): on an un-synced replica a missing
//! space row means "we haven't heard yet", not "deleted".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use zeron_proto::Space;

use crate::fs_watch::{FsWatch, FsWatchHub};
use crate::repos::Repos;
use crate::workspace_host::WorkspaceHost;

/// Trailing debounce after a filesystem event burst.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// Slow repair pass: recheck every owned space + orphan sweep.
const REPAIR_INTERVAL: Duration = Duration::from_secs(120);

struct SpaceEntry {
    path: PathBuf,
    kick_tx: mpsc::UnboundedSender<()>,
    /// Keeps the folder watch alive on the shared [`FsWatchHub`]; dropped on
    /// entry close. Filled asynchronously — FSEvents registration blocks, so
    /// [`reconcile`] builds it off the runtime and attaches it here once ready.
    folder_watch: Mutex<Option<FsWatch>>,
}

struct SpacesSyncInner {
    repos: Repos,
    workspace: WorkspaceHost,
    device_id: String,
    entries: Mutex<HashMap<String, Arc<SpaceEntry>>>,
    /// Ends the supervisor loop eagerly on shutdown (weak refs alone only end
    /// it once the whole graph drops).
    cancel: CancellationToken,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct SpacesSync {
    inner: Arc<SpacesSyncInner>,
}

impl SpacesSync {
    /// Build and start the sync loop: follows the workspace spaces watch and
    /// runs the repair tick. Requires a tokio runtime.
    pub fn start(repos: Repos, workspace: WorkspaceHost, device_id: &str) -> Self {
        let sync = Self {
            inner: Arc::new(SpacesSyncInner {
                repos,
                workspace: workspace.clone(),
                device_id: device_id.to_string(),
                entries: Mutex::new(HashMap::new()),
                cancel: CancellationToken::new(),
                supervisor: Mutex::new(None),
            }),
        };
        let task = tokio::spawn(spaces_task(
            Arc::downgrade(&sync.inner),
            workspace.watch_spaces(),
            sync.inner.cancel.clone(),
        ));
        *lock(&sync.inner.supervisor) = Some(task);
        sync
    }

    /// Stop the supervisor loop and wait for it to exit (per-space tasks are
    /// purely local and end when their entries drop). Idempotent.
    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        let task = lock(&self.inner.supervisor).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// Reconcile + recheck now (tests / opportunistic callers).
    pub async fn reconcile_now(&self) {
        let spaces = self.inner.workspace.watch_spaces().borrow().clone();
        reconcile(&self.inner, &spaces);
        for entry in lock(&self.inner.entries).values() {
            let _ = entry.kick_tx.send(());
        }
    }
}

/// (Re)build the entry set for the spaces THIS device owns.
fn reconcile(inner: &Arc<SpacesSyncInner>, spaces: &[Space]) {
    let owned: HashMap<&str, &Space> = spaces
        .iter()
        .filter(|s| s.device_id == inner.device_id)
        .map(|s| (s.id.as_str(), s))
        .collect();

    let mut entries = lock(&inner.entries);
    entries.retain(|id, _| owned.contains_key(id.as_str()));
    for (id, space) in owned {
        if entries.contains_key(id) {
            continue; // deviceId/path are immutable — nothing to refresh
        }
        let (kick_tx, kick_rx) = mpsc::unbounded_channel();
        let entry = Arc::new(SpaceEntry {
            path: PathBuf::from(&space.path),
            kick_tx: kick_tx.clone(),
            folder_watch: Mutex::new(None),
        });
        entries.insert(id.to_string(), entry.clone());
        tokio::spawn(entry_task(
            Arc::downgrade(inner),
            id.to_string(),
            Arc::downgrade(&entry),
            kick_rx,
        ));
        let _ = kick_tx.send(()); // initial check (boot / first observed)

        // Non-recursive watch on the space folder: `.git` appearing/vanishing
        // among the direct children is exactly the signal we need. Watch
        // failures are fine — the repair tick still converges. Built off the
        // runtime (canonicalizing the path stats it), and reconcile runs on
        // the spaces-watch task.
        let weak = Arc::downgrade(&entry);
        tokio::task::spawn_blocking(move || {
            let Some(entry) = weak.upgrade() else {
                return; // entry removed before the watcher was ready
            };
            let watch = watch_folder(&FsWatchHub::global(), &entry);
            *lock(&entry.folder_watch) = Some(watch);
        });
    }
}

/// Register the space folder on the shared [`FsWatchHub`] — one watcher thread
/// for every space instead of one each. A hub rescan (the watch went live,
/// closing the check→attach gap; or events may have been lost) rechecks like
/// a `.git` change. A failed watch (logged by the hub) never signals; the
/// repair tick covers it. Blocking — call from the blocking pool.
fn watch_folder(hub: &Arc<FsWatchHub>, entry: &SpaceEntry) -> FsWatch {
    let tx = entry.kick_tx.clone();
    hub.watch(
        &entry.path,
        notify::RecursiveMode::NonRecursive,
        move |event| {
            if event.need_rescan()
                || event
                    .paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == ".git"))
            {
                let _ = tx.send(());
            }
        },
    )
}

/// Per-space task: trailing-debounce kicks, then recheck git presence.
async fn entry_task(
    inner: Weak<SpacesSyncInner>,
    space_id: String,
    entry: Weak<SpaceEntry>,
    mut kick_rx: mpsc::UnboundedReceiver<()>,
) {
    while kick_rx.recv().await.is_some() {
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
        check_space(&inner, &space_id, &entry.path).await;
    }
}

/// Probe git presence and stamp the row — write only on change.
async fn check_space(inner: &Arc<SpacesSyncInner>, space_id: &str, path: &Path) {
    let detected = inner.repos.is_repo(path).await;
    let (checkout_id, repository_id) = if detected {
        let checkout_id = match inner.repos.checkout_identity(path).await {
            Ok(identity) => Some(identity.id),
            Err(err) => {
                tracing::debug!(space = %space_id, error = %err, "spaces: checkout identity failed");
                None
            }
        };
        // `None` = the check failed; the row keeps its last identity below
        // rather than regrouping the sidebar on a transient git error.
        let repository_id = match inner.repos.repository_identity(path).await {
            Ok(identity) => Some(Some(identity)),
            Err(err) => {
                tracing::debug!(space = %space_id, error = %err, "spaces: repository identity failed");
                None
            }
        };
        (checkout_id, repository_id)
    } else {
        (None, Some(None))
    };
    let current = match inner.workspace.read_spaces() {
        Ok(spaces) => spaces.into_iter().find(|s| s.id == space_id),
        Err(err) => {
            tracing::warn!(space = %space_id, error = %err, "spaces: row read failed");
            return;
        }
    };
    let Some(current) = current else {
        return; // deleted while checking
    };
    let repository_id = repository_id.unwrap_or_else(|| current.repository_id.clone());
    if current.git_detected == detected
        && current.checkout_id == checkout_id
        && current.repository_id == repository_id
    {
        return; // unchanged — no oplog growth
    }
    match inner.workspace.set_space_git(
        space_id,
        detected,
        checkout_id.as_deref(),
        repository_id.as_deref(),
    ) {
        Ok(_) => {
            tracing::info!(space = %space_id, git = detected, "space git presence updated");
        }
        Err(err) => {
            tracing::warn!(space = %space_id, error = %err, "spaces: git stamp failed");
        }
    }
}

/// Host-side repair: delete OUR chats whose `spaceId` dangles (create-vs-delete
/// race). Chats hosted by other devices are left alone.
fn sweep_orphans(inner: &Arc<SpacesSyncInner>) {
    // NEVER sweep off a replica that hasn't heard from the server this boot:
    // a gapped/pre-heal/offline-at-boot view is missing rows, and "space row
    // absent" on it is not evidence the space was deleted — sweeping there
    // globally destroys live chats (2026-08-23 missing-sessions audit).
    if !inner.workspace.registry_synced() {
        tracing::debug!("spaces: orphan sweep skipped (registry not yet synced this boot)");
        return;
    }
    let spaces = inner.workspace.watch_spaces().borrow().clone();
    let live: std::collections::HashSet<&str> = spaces.iter().map(|s| s.id.as_str()).collect();
    let chats = inner.workspace.watch_chats().borrow().clone();
    for chat in chats {
        if chat.device_id != inner.device_id {
            continue;
        }
        let Some(space_id) = chat.space_id.as_deref() else {
            continue;
        };
        if live.contains(space_id) {
            continue;
        }
        tracing::info!(chat = %chat.id, space = %space_id, "deleting orphaned chat (space gone)");
        if let Err(err) = inner.workspace.delete_chat(&chat.id) {
            tracing::warn!(chat = %chat.id, error = %err, "spaces: orphan delete failed");
        }
    }
}

/// Spaces-watch follower + repair tick. Weak handles so dropping the service
/// tears the loop down; the token ends it eagerly on shutdown.
async fn spaces_task(
    inner: Weak<SpacesSyncInner>,
    mut spaces_rx: watch::Receiver<Vec<Space>>,
    cancel: CancellationToken,
) {
    let mut repair = tokio::time::interval(REPAIR_INTERVAL);
    repair.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    repair.tick().await; // consume the immediate first tick
    {
        let Some(inner) = inner.upgrade() else { return };
        let spaces = spaces_rx.borrow().clone();
        reconcile(&inner, &spaces);
    }
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            changed = spaces_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let Some(inner) = inner.upgrade() else { break };
                let spaces = spaces_rx.borrow_and_update().clone();
                reconcile(&inner, &spaces);
            }
            _ = repair.tick() => {
                let Some(inner) = inner.upgrade() else { break };
                let spaces = spaces_rx.borrow().clone();
                reconcile(&inner, &spaces);
                for entry in lock(&inner.entries).values() {
                    let _ = entry.kick_tx.send(());
                }
                sweep_orphans(&inner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder watch rides the shared hub: going live sends one recheck
    /// (the check→attach gap), `.git` appearing among the direct children
    /// kicks, deeper churn does not (non-recursive), and a vanished folder
    /// has no live watch and never signals.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn folder_watch_kicks_on_attach_and_git_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let (kick_tx, mut kick_rx) = mpsc::unbounded_channel();
        let entry = Arc::new(SpaceEntry {
            path: root.clone(),
            kick_tx,
            folder_watch: Mutex::new(None),
        });
        let hub = FsWatchHub::new();
        let watch = {
            let (hub, entry) = (hub.clone(), entry.clone());
            tokio::task::spawn_blocking(move || watch_folder(&hub, &entry))
                .await
                .unwrap()
        };
        tokio::time::timeout(Duration::from_secs(5), kick_rx.recv())
            .await
            .expect("going live kicks a recheck")
            .expect("kick channel open");
        assert!(watch.is_live());
        *lock(&entry.folder_watch) = Some(watch);
        tokio::time::sleep(Duration::from_millis(300)).await;
        while kick_rx.try_recv().is_ok() {}

        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(kick_rx.try_recv().is_err(), "nested churn must not kick");

        std::fs::create_dir(root.join(".git")).unwrap();
        tokio::time::timeout(Duration::from_secs(5), kick_rx.recv())
            .await
            .expect("`.git` appearing kicks a recheck")
            .expect("kick channel open");

        let (missing_tx, mut missing_rx) = mpsc::unbounded_channel();
        let missing = Arc::new(SpaceEntry {
            path: root.join("gone"),
            kick_tx: missing_tx,
            folder_watch: Mutex::new(None),
        });
        let failed = {
            let hub = hub.clone();
            tokio::task::spawn_blocking(move || {
                let watch = watch_folder(&hub, &missing);
                hub.apply();
                watch
            })
            .await
            .unwrap()
        };
        assert!(!failed.is_live(), "a vanished folder has no live watch");
        assert!(missing_rx.try_recv().is_err(), "and never signals");
    }
}
