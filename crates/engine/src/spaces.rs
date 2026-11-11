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
//! A kick runs git only when something git's answer depends on changed
//! ([`GitInputs`]: the `.git` entry, HEAD and the branch it names, config,
//! packed-refs, shallow, the global config). Everything else — a rescan
//! after the shared watcher restarted, `.git` churn from commands other
//! chats run in a busy repository, the repair tick — costs a few `stat`s
//! and reapplies the last answer.
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
    /// Git probes run (tests count them).
    #[cfg(test)]
    git_probes: std::sync::atomic::AtomicUsize,
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
                #[cfg(test)]
                git_probes: std::sync::atomic::AtomicUsize::new(0),
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

/// Per-space task: trailing-debounce kicks, then recheck git presence —
/// with git itself only when its inputs changed.
async fn entry_task(
    inner: Weak<SpacesSyncInner>,
    space_id: String,
    entry: Weak<SpaceEntry>,
    mut kick_rx: mpsc::UnboundedReceiver<()>,
) {
    // The inputs and answer of the last complete probe.
    let mut last: Option<(GitInputs, GitProbe)> = None;
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
        let folder = entry.path.clone();
        let inputs = tokio::task::spawn_blocking(move || GitInputs::read(&folder))
            .await
            .ok();
        let probe = match (&last, &inputs) {
            (Some((seen, probe)), Some(now)) if seen == now => probe.clone(),
            _ => {
                #[cfg(test)]
                inner.git_probes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let in_repository = inputs.as_ref().map(|inputs| inputs.repository);
                let probe = probe_git(&inner, &space_id, &entry.path, in_repository).await;
                // A failed git call is retried on the next kick.
                last = inputs.filter(|_| probe.complete).map(|inputs| (inputs, probe.clone()));
                probe
            }
        };
        apply_probe(&inner, &space_id, probe);
    }
}

/// What a space's git check reads, cheaply (`stat`s and two tiny files):
/// git's answer can't change while these don't. Found the way git finds a
/// repository: the nearest `.git` at or above the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitInputs {
    /// A `.git` was found at or above the folder.
    repository: bool,
    /// (path, (inode, mtime ns, size)) — `None` = absent.
    stamps: Vec<(PathBuf, Option<(u64, i128, u64)>)>,
    /// Small files whose content says where to look: a `.git` file's
    /// `gitdir:`, `commondir`, HEAD.
    contents: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl GitInputs {
    fn read(folder: &Path) -> Self {
        fn stamp(path: &Path) -> Option<(u64, i128, u64)> {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(path).ok()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as i128);
            #[cfg(unix)]
            let inode = meta.ino();
            #[cfg(not(unix))]
            let inode = 0;
            // A directory's mtime moves with every lock file git makes in
            // it: only its identity counts.
            if meta.is_dir() {
                Some((inode, 0, 0))
            } else {
                Some((inode, mtime, meta.len()))
            }
        }
        let mut inputs = GitInputs { repository: false, stamps: Vec::new(), contents: Vec::new() };
        let read = |inputs: &mut GitInputs, path: PathBuf| -> Option<Vec<u8>> {
            let content = std::fs::read(&path).ok().filter(|c| c.len() <= 4096);
            inputs.contents.push((path, content.clone()));
            content
        };
        inputs.stamps.push((folder.to_path_buf(), stamp(folder)));
        // Git finds the repository from the real path (a symlinked folder).
        let resolved = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        let folder = resolved.as_path();
        // System and global configuration (remote rewrites, includes).
        inputs.stamps.push(("/etc/gitconfig".into(), stamp(Path::new("/etc/gitconfig"))));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            inputs.stamps.push((home.join(".gitconfig"), stamp(&home.join(".gitconfig"))));
            let xdg = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            let path = xdg.join("git/config");
            inputs.stamps.push((path.clone(), stamp(&path)));
        }
        let Some(dot_git) = folder.ancestors().map(|dir| dir.join(".git")).find(|p| {
            let found = stamp(p);
            inputs.stamps.push((p.clone(), found));
            found.is_some()
        }) else {
            return inputs; // not in a repository
        };
        inputs.repository = true;
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else {
            // A linked worktree or submodule: `gitdir: <path>`.
            let pointer = read(&mut inputs, dot_git.clone()).unwrap_or_default();
            let pointer = String::from_utf8_lossy(&pointer);
            let Some(target) = pointer.trim().strip_prefix("gitdir:") else {
                return inputs;
            };
            dot_git.parent().unwrap_or(folder).join(target.trim())
        };
        let common_dir = match read(&mut inputs, git_dir.join("commondir")) {
            Some(common) => git_dir.join(String::from_utf8_lossy(&common).trim()),
            None => git_dir.clone(),
        };
        let head = read(&mut inputs, git_dir.join("HEAD")).unwrap_or_default();
        let head = String::from_utf8_lossy(&head);
        let mut stamped = vec![
            common_dir.join("config"),
            git_dir.join("config.worktree"),
            common_dir.join("packed-refs"),
            common_dir.join("shallow"),
        ];
        // HEAD's branch: its first commit decides the repository's identity.
        if let Some(branch) = head.trim().strip_prefix("ref:") {
            stamped.push(common_dir.join(branch.trim()));
        }
        for path in stamped {
            let found = stamp(&path);
            inputs.stamps.push((path, found));
        }
        inputs
    }
}

/// One git check's answer.
#[derive(Clone)]
struct GitProbe {
    detected: bool,
    checkout_id: Option<String>,
    /// `None` = the check failed; the row keeps its last identity.
    repository_id: Option<Option<String>>,
    /// `None` = the check failed; the row keeps its last repository.
    github_repo: Option<Option<String>>,
    /// Every git call answered: the probe can stand for unchanged inputs.
    complete: bool,
}

/// Probe git presence. `in_repository`: whether [`GitInputs`] found a
/// `.git` — what tells "not a repository" from a git call that failed.
async fn probe_git(
    inner: &Arc<SpacesSyncInner>,
    space_id: &str,
    path: &Path,
    in_repository: Option<bool>,
) -> GitProbe {
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
    // The project's GitHub repository, so every device can offer running a
    // session on Cloud without asking this one.
    let github_repo = if detected {
        match inner.repos.github_repo_checked(path).await {
            Ok(repo) => Ok(repo),
            Err(err) => {
                tracing::debug!(space = %space_id, error = %err, "spaces: origin read failed");
                Err(())
            }
        }
    } else {
        Ok(None)
    };
    // Kept for unchanged inputs only when every answer is git's real one: a
    // failed call (git busy, a lock) is retried on the next kick.
    let complete = if detected {
        checkout_id.is_some() && repository_id.is_some() && github_repo.is_ok()
    } else {
        in_repository == Some(false)
    };
    // `None` = the read failed; the row keeps its last repository.
    let github_repo = github_repo.ok();
    GitProbe {
        detected,
        complete,
        checkout_id,
        repository_id,
        github_repo,
    }
}

/// Stamp the row with a probe's answer — write only on change.
fn apply_probe(inner: &Arc<SpacesSyncInner>, space_id: &str, probe: GitProbe) {
    let GitProbe {
        detected,
        checkout_id,
        repository_id,
        github_repo,
        complete: _,
    } = probe;
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
    let github_repo = github_repo.unwrap_or_else(|| current.github_repo.clone());
    // Cloud projects of this repository (no engine of their own) take this
    // clone's identity so they group with it — checked every pass, since
    // this space's own row is usually unchanged.
    if let (Some(repo), Some(id)) = (github_repo.as_deref(), repository_id.as_deref()) {
        match inner.workspace.align_cloud_spaces(repo, id) {
            Ok(0) => {}
            Ok(moved) => {
                tracing::info!(space = %space_id, moved, "cloud projects grouped with this clone")
            }
            Err(err) => {
                tracing::warn!(space = %space_id, error = %err, "spaces: cloud project grouping failed")
            }
        }
    }
    if current.git_detected == detected
        && current.checkout_id == checkout_id
        && current.repository_id == repository_id
        && current.github_repo == github_repo
    {
        return; // unchanged — no oplog growth
    }
    match inner.workspace.set_space_git(
        space_id,
        detected,
        checkout_id.as_deref(),
        repository_id.as_deref(),
        github_repo.as_deref(),
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

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Kicks run git only when an input of its answer changed. A busy
    /// repository (other chats staging, building, committing on other
    /// branches) and every watcher rescan cost a few `stat`s; HEAD moving to
    /// another branch, the config or the folder's repository changing still
    /// probe.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_changed_git_inputs_run_git() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(tmp.path()).unwrap().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("src/a.rs"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "one"]);
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(data.path()).unwrap());
        let workspace = WorkspaceHost::open(
            store,
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "dev".into(),
                device_name: "Dev".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        workspace
            .create_space("space", "dev", repo.to_str().unwrap(), None, false)
            .unwrap();
        let sync = SpacesSync::start(Repos::new(data.path(), "dev"), workspace.clone(), "dev");
        let probes = || sync.inner.git_probes.load(std::sync::atomic::Ordering::Relaxed);
        let settle = || tokio::time::sleep(Duration::from_millis(1_500));
        let kick = || {
            for entry in lock(&sync.inner.entries).values() {
                let _ = entry.kick_tx.send(());
            }
        };
        let detected = || {
            workspace
                .read_spaces()
                .unwrap()
                .into_iter()
                .find(|s| s.id == "space")
                .is_some_and(|s| s.git_detected)
        };
        // Positive outcomes are polled (git subprocesses on a loaded
        // machine); "nothing ran" needs the debounce to pass, so it waits.
        let reach = |n: usize, what: &'static str| {
            let probes = &probes;
            async move {
                tokio::time::timeout(Duration::from_secs(20), async {
                    while probes() < n {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect(what);
                // Let its row write land.
                settle().await;
                assert_eq!(probes(), n, "{what}: one probe");
            }
        };
        reach(1, "the first check probes").await;
        assert!(detected());

        // Churn that can't change the answer: the index, objects, a lock
        // file, a commit on another branch, the work tree, a rescan.
        std::fs::write(repo.join("src/a.rs"), "b").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["branch", "side"]);
        std::fs::write(repo.join(".git/index.lock"), "").unwrap();
        std::fs::remove_file(repo.join(".git/index.lock")).unwrap();
        kick();
        settle().await;
        kick();
        settle().await;
        assert_eq!(probes(), 1, "nothing git reads changed");

        git(&repo, &["checkout", "-q", "side"]);
        kick();
        reach(2, "HEAD moved to another branch").await;

        git(&repo, &["remote", "add", "origin", "https://github.com/o/r.git"]);
        kick();
        reach(3, "the config changed").await;
        let space = workspace.read_spaces().unwrap().into_iter().find(|s| s.id == "space").unwrap();
        assert_eq!(space.github_repo.as_deref(), Some("o/r"));

        std::fs::rename(repo.join(".git"), repo.join("git-gone")).unwrap();
        kick();
        reach(4, "the repository went away").await;
        assert!(!detected());
        // "Not a repository" is a real answer: kept while nothing changes.
        kick();
        settle().await;
        assert_eq!(probes(), 4, "an answered absence is not re-probed");
        sync.shutdown().await;
    }

    /// A linked worktree's inputs follow its own HEAD and the shared config.
    #[test]
    fn worktree_inputs_follow_its_head() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&repo, &["worktree", "add", "-q", "-b", "wt", root.join("wt").to_str().unwrap()]);
        let before = GitInputs::read(&root.join("wt"));
        assert_eq!(before, GitInputs::read(&root.join("wt")));
        git(&root.join("wt"), &["commit", "-q", "--allow-empty", "-m", "two"]);
        assert_ne!(before, GitInputs::read(&root.join("wt")), "its branch moved");
        let before = GitInputs::read(&root.join("wt"));
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "main moves"]);
        assert_eq!(before, GitInputs::read(&root.join("wt")), "another branch is no input");
        git(&repo, &["config", "zeron.test", "1"]);
        assert_ne!(before, GitInputs::read(&root.join("wt")), "the shared config");
    }

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
