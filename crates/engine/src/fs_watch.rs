//! FsWatchHub — one `notify` watcher shared by every subsystem that live-watches
//! folders (diff-sync checkouts, space folders).
//!
//! Every `notify::RecommendedWatcher` owns a dedicated OS thread (the FSEvents
//! run loop on macOS, the inotify loop on Linux). One watcher per checkout and
//! per space folder put ~25-30 idle threads on a normal device at boot. The hub
//! instead installs every path on a single watcher and routes each event to
//! the registrations whose path covers it, so the whole engine pays for one
//! watcher thread for pruned trees and flat watches. Legacy recursive watches
//! on inotify/kqueue retain one instance per root (see [`SHARED_WATCHER`]).
//!
//! [`FsWatchHub::watch_tree`] walks source directories without following links,
//! respects git ignores, and installs shared NonRecursive inotify/kqueue
//! watches. FSEvents/Windows retain native recursion and filter delivery using
//! the pruned directory set, avoiding per-directory stream/handle limits. Directory
//! create/move/delete and ignore-rule changes reconcile the directory set in a
//! batch. A rescan closes the walk-to-install gap, including files populated
//! before a new directory's watch attached. Overflow broadcasts a rescan and
//! repairs the directory set, so lost creates cannot leave unwatched subtrees.
//!
//! Semantics match a private watcher per registration:
//!
//! - a [`notify::RecursiveMode::NonRecursive`] registration only sees events for
//!   its folder and that folder's direct children (notify's own rule);
//! - pathless events (inotify queue overflow → rescan) reach every registration,
//!   since an overflow loses events for all of them;
//! - dropping the [`FsWatch`] handle stops delivery immediately and releases the
//!   OS watch once no other registration needs it.
//!
//! Handlers also receive synthetic *rescan* events
//! ([`notify::Event::need_rescan`]): once when the registration's watch goes
//! live (anything before that was unobserved), and whenever events may have
//! been lost (see below). Callers treat them like any change.
//!
//! **Batching.** Registration changes are coalesced on a short-lived worker
//! thread ([`BATCH_QUIET`] of quiet, at most [`BATCH_MAX`]) and applied in one
//! pass, so a boot or a burst of entries is one reconcile, not one per path.
//!
//! **Restart gaps.** notify 7's FSEvents backend stops the stream, joins its
//! thread and starts a new stream (from "now" — the event id is not settable)
//! on *every* `watch`/`unwatch`, so each change drops events for every other
//! path for that moment. While a pass changes the main watcher, the hub runs
//! a temporary *bridge* stream on the surviving paths' common ancestor, started
//! before the first restart and stopped after the last: the surviving paths
//! stay covered throughout and nothing needs re-capturing. If the bridge cannot
//! be opened, the pass ends by sending a rescan to every registration that was
//! live across it (one per registration per pass; callers debounce). inotify
//! and Windows watches are per path and never restart, so neither applies.
//!
//! OS watches are the minimal cover of the registrations: identical paths are
//! installed once (recursive if any registration wants recursion) and a path
//! under a recursively watched ancestor is not installed separately. When the
//! covering watch of a live registration is replaced (a nested path takes over
//! from a dropped recursive ancestor, a mode change), the registration gets a
//! rescan: the swap itself was unobserved.
//!
//! Watch failures (the folder vanished, inotify limits) are logged and the
//! registration stays without a live watch, exactly like a private watcher
//! whose `watch()` failed — callers keep their repair ticks for that case.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use notify::Watcher as _;

/// A batch applies once registrations have been quiet this long…
const BATCH_QUIET: Duration = Duration::from_millis(100);
/// …or this long after its first change, whichever comes first.
const BATCH_MAX: Duration = Duration::from_secs(1);
/// Walk-install-verify rounds one [`FsWatchHub::apply`] makes before handing
/// a still-changing tree to the batch worker.
const MAX_VERIFY_PASSES: usize = 3;
/// Whether a watch change restarts the stream for every path (FSEvents).
const STREAM_RESTARTS_ON_CHANGE: bool = cfg!(target_os = "macos");
/// Whether every root can share one watcher. inotify (and kqueue) build a
/// recursive watch by walking the tree *following symlinks*, so in a shared
/// instance a checkout that symlinks into another checkout (`npm link`) would
/// alias that checkout's watch descriptors: its events would be relabelled
/// and unwatching either root would strip the other. Those backends get one
/// watcher per installed root instead, exactly as before the hub.
const SHARED_WATCHER: bool = cfg!(any(target_os = "macos", windows));

pub(crate) type TreeFilter = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// The same pruned directory walk drives both the budget and installation.
/// Explicit roots (including a checkout's git dir) are always visited.
pub(crate) fn tree_directories(
    root: &Path,
    filter: TreeFilter,
) -> impl Iterator<Item = Result<PathBuf, ignore::Error>> {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(move |entry| entry.depth() == 0 || filter(entry.path()))
        .build()
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_dir()) => {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(err) => Some(Err(err)),
        })
}

/// The pruned directory set of one tree, and whether the walk read all of it.
fn walk_tree(root: &Path, filter: &TreeFilter) -> (HashSet<PathBuf>, bool) {
    let mut dirs = HashSet::new();
    let mut complete = true;
    for entry in tree_directories(root, filter.clone()) {
        match entry {
            Ok(path) => {
                dirs.insert(path);
            }
            Err(err) => {
                complete = false;
                tracing::debug!(error = %err, "fs-watch: tree walk failed");
            }
        }
    }
    (dirs, complete)
}

pub(crate) fn exceeds_tree_budget(root: &Path, limit: usize) -> bool {
    // Count children, as the previous budget did. Errors cannot establish a
    // healthy watch, so leave those roots on their repair tick.
    let mut seen = 0;
    for entry in tree_directories(root, Arc::new(|_| true)) {
        match entry {
            Ok(path) if path == root => {}
            Ok(_) => {
                seen += 1;
                if seen > limit {
                    return true;
                }
            }
            Err(_) => return true,
        }
    }
    false
}

struct Tree {
    filter: TreeFilter,
    dirs: Mutex<HashSet<PathBuf>>,
    dirty: AtomicBool,
    rescan: AtomicBool,
    complete: AtomicBool,
}

type Handler = Arc<dyn Fn(&notify::Event) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The synthetic "anything may have changed" signal.
fn rescan_event() -> notify::Event {
    notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)
}

struct Registration {
    /// Canonical path — what FSEvents/inotify report events under.
    path: PathBuf,
    recursive: bool,
    tree: Option<Arc<Tree>>,
    handler: Handler,
    /// Shared with the [`FsWatch`]: an installed OS watch covers `path`.
    attached: Arc<AtomicBool>,
}

impl Registration {
    fn covers(&self, path: &Path) -> bool {
        if let Some(tree) = &self.tree {
            let dirs = lock(&tree.dirs);
            return path == self.path
                || dirs.contains(path)
                || path.parent().is_some_and(|parent| dirs.contains(parent));
        }
        path.starts_with(&self.path)
            && (self.recursive || path == self.path || path.parent() == Some(self.path.as_path()))
    }
}

#[derive(Default)]
struct Routes {
    next_id: u64,
    regs: HashMap<u64, Registration>,
}

#[derive(Default)]
struct OsState {
    /// Shared NonRecursive watcher, also used for recursive watches on
    /// [`SHARED_WATCHER`] backends. Created lazily on the
    /// first install. On macOS an FSEvents watcher with no paths has no
    /// run-loop thread at all.
    watcher: Option<notify::RecommendedWatcher>,
    /// One watcher per legacy recursive root on the other backends. Dropping one
    /// closes its instance, releasing every OS watch it added — including
    /// those a failed recursive `watch` left behind.
    roots: HashMap<PathBuf, notify::RecommendedWatcher>,
    /// Canonical path → recursive, as installed.
    installed: HashMap<PathBuf, bool>,
    /// Wanted paths whose watch failed. Not retried while still wanted (every
    /// FSEvents watch/unwatch restarts the stream for *all* paths); forgotten
    /// once nothing registers them, so a fresh registration retries.
    failed: HashSet<PathBuf>,
}

#[derive(Default)]
struct Batch {
    /// Bumped on every registration change.
    generation: u64,
    /// A batch worker thread is pending or applying.
    worker: bool,
}

#[derive(Default)]
struct Stats {
    /// Passes that changed the main watcher.
    passes: AtomicUsize,
    /// Bridge streams opened.
    bridges: AtomicUsize,
}

pub(crate) struct FsWatchHub {
    /// Shared with the watcher callbacks. Never held across a watcher call:
    /// FSEvents `watch`/`unwatch`/drop stop and join the run-loop thread,
    /// which may be inside the callback waiting on this lock.
    routes: Arc<Mutex<Routes>>,
    weak: Weak<FsWatchHub>,
    invalidated: Mutex<HashSet<PathBuf>>,
    /// Serializes OS watch changes.
    os: Mutex<OsState>,
    batch: Mutex<Batch>,
    restarts_on_change: bool,
    /// [`SHARED_WATCHER`]; a field so tests can drive both modes.
    shared_watcher: bool,
    bridge_enabled: AtomicBool,
    #[cfg_attr(not(test), allow(dead_code))]
    stats: Stats,
    /// Runs once a tree walk finished and before its watches install: the
    /// window in which a new directory is invisible to both.
    #[cfg(test)]
    after_walk: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// A registration. Dropping it stops delivery and releases the OS watch.
pub(crate) struct FsWatch {
    hub: Arc<FsWatchHub>,
    id: u64,
    attached: Arc<AtomicBool>,
}

impl FsWatch {
    /// Whether an installed OS watch currently covers the path. False until
    /// the registration's batch applies, and for good if the watch failed.
    pub(crate) fn is_live(&self) -> bool {
        self.attached.load(Ordering::Acquire)
    }
}

impl FsWatch {
    /// Re-walk a pruned tree on the next batch. Ignore inputs outside the
    /// registered roots (global excludes, an ancestor `.gitignore`, the
    /// `info/exclude` of a linked worktree's main repository) raise no event
    /// under the tree, so a consumer calls this from its repair tick. An
    /// unchanged tree costs a walk and sends nothing. No-op for flat watches.
    pub(crate) fn refresh(&self) {
        let tree = lock(&self.hub.routes)
            .regs
            .get(&self.id)
            .and_then(|reg| reg.tree.clone());
        if let Some(tree) = tree {
            tree.dirty.store(true, Ordering::Release);
            self.hub.schedule_with_fallback(false);
        }
    }
}

impl Drop for FsWatch {
    fn drop(&mut self) {
        if lock(&self.hub.routes).regs.remove(&self.id).is_some() {
            self.hub.schedule();
        }
    }
}

/// Resets [`Batch::worker`] if a pass panics, so later changes still get a
/// worker. A normal exit resets it in [`FsWatchHub::run_batches`] itself.
struct WorkerGuard<'a>(&'a FsWatchHub);

impl Drop for WorkerGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            lock(&self.0.batch).worker = false;
        }
    }
}

impl FsWatchHub {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            weak: weak.clone(),
            invalidated: Mutex::new(HashSet::new()),
            routes: Arc::new(Mutex::new(Routes::default())),
            os: Mutex::new(OsState::default()),
            batch: Mutex::new(Batch::default()),
            restarts_on_change: STREAM_RESTARTS_ON_CHANGE,
            shared_watcher: SHARED_WATCHER,
            bridge_enabled: AtomicBool::new(true),
            stats: Stats::default(),
            #[cfg(test)]
            after_walk: Mutex::default(),
        })
    }

    /// The process-wide hub.
    pub(crate) fn global() -> Arc<Self> {
        static HUB: OnceLock<Arc<FsWatchHub>> = OnceLock::new();
        HUB.get_or_init(Self::new).clone()
    }

    /// Route events under `path` to `handler` until the returned handle drops.
    /// Returns at once; the OS watch is installed by the next batch, which
    /// then sends `handler` a rescan event. Canonicalizes `path` (a blocking
    /// stat) — call from the blocking pool.
    pub(crate) fn watch(
        self: &Arc<Self>,
        path: &Path,
        mode: notify::RecursiveMode,
        handler: impl Fn(&notify::Event) + Send + Sync + 'static,
    ) -> FsWatch {
        self.register(
            path,
            matches!(mode, notify::RecursiveMode::Recursive),
            None,
            Arc::new(handler),
        )
    }

    fn register(
        self: &Arc<Self>,
        path: &Path,
        recursive: bool,
        tree: Option<Arc<Tree>>,
        handler: Handler,
    ) -> FsWatch {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let attached = Arc::new(AtomicBool::new(false));
        let id = {
            let mut routes = lock(&self.routes);
            routes.next_id += 1;
            let id = routes.next_id;
            routes.regs.insert(
                id,
                Registration {
                    path,
                    recursive,
                    tree,
                    handler,
                    attached: attached.clone(),
                },
            );
            id
        };
        self.schedule();
        FsWatch {
            hub: self.clone(),
            id,
            attached,
        }
    }

    /// Install a pruned tree using NonRecursive directory watches on inotify
    /// and kqueue. Native subtree backends retain their single recursive root
    /// and prune delivery with the same directory set. Unlike the legacy
    /// recursive API, the NonRecursive paths may share an inotify instance:
    /// the walk never follows a symlink. Identical directory paths are installed
    /// once, even across tree and flat consumers. Call from the blocking pool.
    pub(crate) fn watch_tree(
        self: &Arc<Self>,
        root: &Path,
        filter: TreeFilter,
        handler: impl Fn(&notify::Event) + Send + Sync + 'static,
    ) -> FsWatch {
        self.register(
            root,
            true,
            Some(Arc::new(Tree {
                filter,
                dirs: Mutex::new(HashSet::new()),
                dirty: AtomicBool::new(true),
                rescan: AtomicBool::new(false),
                complete: AtomicBool::new(false),
            })),
            Arc::new(handler),
        )
    }

    #[cfg(test)]
    pub(crate) fn inject_event(self: &Arc<Self>, event: notify::Event) {
        self.on_event(event);
    }

    fn on_event(self: &Arc<Self>, event: notify::Event) {
        use notify::event::{ModifyKind, RenameMode};
        if matches!(event.kind, notify::EventKind::Access(_)) && !event.need_rescan() {
            dispatch(&self.routes, &event);
            return;
        }
        let ignore_changed = event.paths.iter().any(|path| {
            path.file_name()
                .is_some_and(|name| name == ".gitignore" || name == ".ignore" || name == "exclude")
        });
        let removed = matches!(
            event.kind,
            notify::EventKind::Remove(_)
                | notify::EventKind::Modify(ModifyKind::Name(
                    RenameMode::From | RenameMode::Both | RenameMode::Any
                ))
        );
        let mut changed = false;
        {
            let routes = lock(&self.routes);
            for reg in routes.regs.values() {
                let Some(tree) = &reg.tree else { continue };
                let dirs = lock(&tree.dirs);
                let topology = event.need_rescan()
                    || ignore_changed
                    || event.paths.iter().any(|path| {
                        path.starts_with(&reg.path)
                            && ((matches!(
                                event.kind,
                                notify::EventKind::Create(_)
                                    | notify::EventKind::Modify(ModifyKind::Name(_))
                            ) && std::fs::symlink_metadata(path)
                                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()))
                                || (removed && dirs.contains(path)))
                    });
                if topology {
                    if event.need_rescan() {
                        lock(&self.invalidated).insert(reg.path.clone());
                    }
                    tree.dirty.store(true, Ordering::Release);
                    changed = true;
                }
            }
        }
        if changed && removed {
            lock(&self.invalidated).extend(event.paths.iter().cloned());
        }
        // Deliver the native event unchanged, including both rename paths.
        dispatch(&self.routes, &event);
        if changed {
            // Never reconcile inline on notify's own event-loop thread: a
            // watch/unwatch waits for that thread to service its command.
            self.schedule_with_fallback(false);
        }
    }

    /// Note a registration change; start a batch worker unless one is pending.
    /// The worker is a short-lived plain thread (not the tokio blocking pool):
    /// a task queued on a runtime that shuts down before running it would
    /// leave `worker` set and wedge the hub for the rest of the process.
    fn schedule(self: &Arc<Self>) {
        self.schedule_with_fallback(true);
    }

    fn schedule_with_fallback(self: &Arc<Self>, inline: bool) {
        {
            let mut batch = lock(&self.batch);
            batch.generation += 1;
            if batch.worker {
                return;
            }
            batch.worker = true;
        }
        let hub = self.clone();
        let spawned = std::thread::Builder::new()
            .name("fs-watch batch".into())
            .spawn(move || hub.run_batches());
        if let Err(err) = spawned {
            tracing::debug!(error = %err, "fs-watch: batch worker spawn failed");
            lock(&self.batch).worker = false;
            if inline {
                self.apply();
            } else {
                // Repair ticks must not trust a tree whose new directories
                // could not be attached. A later topology change retries.
                for reg in lock(&self.routes).regs.values() {
                    if reg
                        .tree
                        .as_ref()
                        .is_some_and(|tree| tree.dirty.load(Ordering::Acquire))
                    {
                        reg.attached.store(false, Ordering::Release);
                    }
                }
                dispatch(&self.routes, &rescan_event());
            }
        }
    }

    fn run_batches(&self) {
        let _guard = WorkerGuard(self);
        loop {
            let started = Instant::now();
            let mut seen = lock(&self.batch).generation;
            loop {
                std::thread::sleep(BATCH_QUIET);
                let now = lock(&self.batch).generation;
                let quiet = now == seen;
                seen = now;
                if quiet || started.elapsed() >= BATCH_MAX {
                    break;
                }
            }
            self.apply();
            // Changes that landed during the pass need another one. Release
            // `worker` under the same lock as the check: a `schedule` between
            // the two would see a worker that is about to exit and be lost.
            let mut batch = lock(&self.batch);
            if batch.generation == seen {
                batch.worker = false;
                return;
            }
        }
    }

    /// Bring the installed OS watches in line with the registrations and
    /// deliver the resulting rescan signals. Blocking. Idempotent.
    pub(crate) fn apply(&self) {
        let mut rescans: Vec<Handler> = Vec::new();
        let mut again = true;
        for _ in 0..MAX_VERIFY_PASSES {
            let (handlers, redo) = {
                let mut os = lock(&self.os);
                self.reconcile_os(&mut os)
            };
            for handler in handlers {
                if !rescans.iter().any(|seen| Arc::ptr_eq(seen, &handler)) {
                    rescans.push(handler);
                }
            }
            again = redo;
            if !again {
                break;
            }
        }
        let event = rescan_event();
        for handler in rescans {
            handler(&event);
        }
        if again {
            // A tree that keeps changing under its walk: keep converging on
            // the batch worker rather than looping here.
            if let Some(hub) = self.weak.upgrade() {
                hub.schedule_with_fallback(false);
            }
        }
    }

    fn reconcile_os(&self, os: &mut OsState) -> (Vec<Handler>, bool) {
        // Walk outside the routes lock: native events keep flowing during a
        // scan. A concurrent topology change marks dirty and schedules a pass.
        let trees: Vec<_> = lock(&self.routes)
            .regs
            .values()
            .filter_map(|reg| {
                reg.tree
                    .as_ref()
                    .map(|tree| (reg.path.clone(), tree.clone()))
            })
            .collect();
        let mut walked = Vec::new();
        for (root, tree) in &trees {
            if tree.dirty.swap(false, Ordering::AcqRel) {
                let (dirs, complete) = walk_tree(root, &tree.filter);
                for path in &dirs {
                    os.failed.remove(path);
                }
                // A walk that found the same directories attaches nothing and
                // leaves nothing unobserved, so consumers need no rescan.
                let changed =
                    dirs != *lock(&tree.dirs) || complete != tree.complete.load(Ordering::Acquire);
                *lock(&tree.dirs) = dirs;
                tree.complete.store(complete, Ordering::Release);
                if changed {
                    tree.rescan.store(true, Ordering::Release);
                }
                walked.push((root, tree));
            }
        }
        #[cfg(test)]
        if let Some(hook) = lock(&self.after_walk).take() {
            hook();
        }
        // Removed/replaced directories may have the same spelling by the
        // next pass, but their old kernel watch refers to a different inode.
        let mut invalid = std::mem::take(&mut *lock(&self.invalidated));
        let invalid_before = invalid.clone();
        // Wanted set: distinct paths, recursive only for legacy registrations.
        let mut wanted: HashMap<PathBuf, bool> = HashMap::new();
        for reg in lock(&self.routes).regs.values() {
            if let Some(tree) = &reg.tree {
                if self.shared_watcher {
                    // FSEvents and ReadDirectoryChangesW watch a whole tree
                    // natively. Per-directory FSEvent paths hit the stream's
                    // path limit; Windows flat watches can lose subtree events.
                    // Keep their native recursion and prune delivery via dirs.
                    // inotify/kqueue still install only NonRecursive paths.
                    wanted.insert(reg.path.clone(), true);
                } else {
                    for path in lock(&tree.dirs).iter() {
                        wanted.entry(path.clone()).or_default();
                    }
                }
            } else {
                *wanted.entry(reg.path.clone()).or_default() |= reg.recursive;
            }
        }
        os.failed.retain(|path| wanted.contains_key(path));
        let before = os.installed.clone();
        let mut bridge: Option<notify::RecommendedWatcher> = None;
        let mut bridge_tried = false;
        let mut touched = false;
        let mut attached_new = false;
        // A failed install may uncover paths it would have covered — iterate
        // until a pass adds no new failures (bounded: `failed` only grows).
        loop {
            let desired = minimal_cover(&wanted, &os.failed);
            if desired == os.installed && invalid.is_empty() {
                break;
            }
            if self.restarts_on_change && !bridge_tried {
                bridge_tried = true;
                bridge = self.open_bridge(&os.installed, &desired);
            }
            // Removals before additions, so an outgoing watch never overlaps an
            // incoming one on the same watcher.
            let stale: Vec<PathBuf> = os
                .installed
                .iter()
                .filter(|(path, recursive)| {
                    desired.get(*path) != Some(*recursive)
                        || invalid.iter().any(|gone| path.starts_with(gone))
                })
                .map(|(path, _)| path.clone())
                .collect();
            for path in stale {
                os.installed.remove(&path);
                if os.roots.remove(&path).is_some() {
                    touched = true;
                } else if let Some(watcher) = os.watcher.as_mut() {
                    touched = true;
                    if let Err(err) = watcher.unwatch(&path) {
                        tracing::debug!(path = %path.display(), error = %err, "fs-watch: unwatch failed");
                    }
                }
            }
            invalid.clear();
            let mut new_failure = false;
            for (path, recursive) in desired {
                if os.installed.get(&path) == Some(&recursive) {
                    continue;
                }
                match install(
                    os,
                    &self.weak,
                    self.shared_watcher,
                    &path,
                    recursive,
                    &mut touched,
                ) {
                    Ok(()) => {
                        os.installed.insert(path, recursive);
                        attached_new = true;
                    }
                    Err(err) => {
                        tracing::debug!(path = %path.display(), error = %err, "fs-watch: watch failed");
                        os.failed.insert(path);
                        new_failure = true;
                    }
                }
            }
            if !new_failure {
                break;
            }
        }
        if touched {
            self.stats.passes.fetch_add(1, Ordering::Relaxed);
        }
        // Only now — the main stream is back up — may the bridge stop.
        let bridged = bridge.is_some();
        drop(bridge);
        let gap = self.restarts_on_change && touched && !bridged;

        if os.installed.is_empty() {
            os.watcher.take();
        }
        // A directory created after its tree was walked but before its
        // parent's watch attached raised no event and is not in the set. Every
        // directory made after the installs above does raise one, so a second
        // walk closes the gap; if it differs the tree is walked again.
        let mut again = false;
        if attached_new {
            for (root, tree) in &walked {
                let (dirs, complete) = walk_tree(root, &tree.filter);
                if dirs != *lock(&tree.dirs) || complete != tree.complete.load(Ordering::Acquire) {
                    tree.dirty.store(true, Ordering::Release);
                    again = true;
                }
            }
        }
        let routes = lock(&self.routes);
        let mut rescans = Vec::new();
        for reg in routes.regs.values() {
            let covered = if let Some(tree) = &reg.tree {
                let dirs = lock(&tree.dirs);
                tree.complete.load(Ordering::Acquire)
                    && !dirs.is_empty()
                    && dirs.iter().all(|path| is_covered(path, &os.installed))
            } else {
                is_covered(&reg.path, &os.installed)
            };
            let tree_rescan = reg
                .tree
                .as_ref()
                .is_some_and(|tree| tree.rescan.swap(false, Ordering::AcqRel));
            if !covered {
                reg.attached.store(false, Ordering::Release);
                if tree_rescan {
                    rescans.push(reg.handler.clone());
                }
                continue;
            }
            let was_attached = reg.attached.swap(true, Ordering::AcqRel);
            // New: everything before the watch went live was unobserved.
            // Gap: events may have been dropped while the stream restarted.
            // Moved: the OS watch covering it was replaced (a recursive root
            // dropped for a nested one, a mode change) — the bridge only spans
            // unchanged roots, so the swap itself was unobserved.
            let moved =
                covering_roots(&reg.path, &before) != covering_roots(&reg.path, &os.installed);
            let rearmed = invalid_before.iter().any(|path| {
                reg.path.starts_with(path) || (reg.recursive && path.starts_with(&reg.path))
            });
            if !was_attached
                || moved
                || tree_rescan
                || rearmed
                || (gap && is_covered(&reg.path, &before))
            {
                rescans.push(reg.handler.clone());
            }
        }
        (rescans, again)
    }

    /// Start a stream covering every installed path that survives this pass,
    /// so the main stream's restarts drop nothing for them. One recursive
    /// watch on their common ancestor: FSEvents filters a volume-wide journal
    /// by path prefix, so a broad path costs no per-directory watches — only
    /// unrelated events to filter in [`dispatch`] for the bridge's short life.
    fn open_bridge(
        &self,
        installed: &HashMap<PathBuf, bool>,
        desired: &HashMap<PathBuf, bool>,
    ) -> Option<notify::RecommendedWatcher> {
        if !self.bridge_enabled.load(Ordering::Relaxed) {
            return None;
        }
        let survivors: Vec<&PathBuf> = installed
            .iter()
            .filter(|(path, recursive)| desired.get(*path) == Some(*recursive))
            .map(|(path, _)| path)
            .collect();
        let ancestor = common_ancestor(&survivors)?;
        let weak = self.weak.clone();
        let opened = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Some(hub) = weak.upgrade() {
                hub.on_event(event.unwrap_or_else(|_| rescan_event()));
            }
        })
        .and_then(|mut watcher| {
            watcher
                .watch(&ancestor, notify::RecursiveMode::Recursive)
                .map(|()| watcher)
        });
        match opened {
            Ok(watcher) => {
                self.stats.bridges.fetch_add(1, Ordering::Relaxed);
                Some(watcher)
            }
            Err(err) => {
                tracing::debug!(path = %ancestor.display(), error = %err,
                    "fs-watch: bridge failed; surviving watches get a rescan");
                None
            }
        }
    }

    #[cfg(test)]
    fn installed(&self) -> HashMap<PathBuf, bool> {
        lock(&self.os).installed.clone()
    }
}

fn install(
    os: &mut OsState,
    hub: &Weak<FsWatchHub>,
    shared: bool,
    path: &Path,
    recursive: bool,
    touched: &mut bool,
) -> Result<(), notify::Error> {
    // FSEvents restarts the stream for every path even when `watch` fails —
    // skip the obvious failure without touching the watcher.
    if !path.exists() {
        return Err(notify::Error::path_not_found().add_path(path.to_path_buf()));
    }
    if !recursive && !std::fs::canonicalize(path).is_ok_and(|canonical| canonical == path) {
        return Err(
            notify::Error::generic("directory changed to a symlink during watch setup")
                .add_path(path.to_path_buf()),
        );
    }
    let new_watcher = || {
        let hub = hub.clone();
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Some(hub) = hub.upgrade() {
                hub.on_event(event.unwrap_or_else(|err| {
                    tracing::debug!(error = %err, "fs-watch: watcher error");
                    rescan_event()
                }));
            }
        })
    };
    let mode = if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    if !shared && recursive {
        // On failure the watcher drops here, with any partial watches.
        let mut watcher = new_watcher()?;
        *touched = true;
        watcher.watch(path, mode)?;
        os.roots.insert(path.to_path_buf(), watcher);
        return Ok(());
    }
    if os.watcher.is_none() {
        os.watcher = Some(new_watcher()?);
    }
    *touched = true;
    os.watcher
        .as_mut()
        .expect("watcher just created")
        .watch(path, mode)
}

fn is_covered(path: &Path, installed: &HashMap<PathBuf, bool>) -> bool {
    installed.contains_key(path)
        || path
            .ancestors()
            .skip(1)
            .any(|parent| installed.get(parent) == Some(&true))
}

fn covering_roots<'a>(
    path: &Path,
    installed: &'a HashMap<PathBuf, bool>,
) -> std::collections::BTreeSet<(&'a PathBuf, bool)> {
    path.ancestors()
        .filter_map(|ancestor| {
            installed
                .get_key_value(ancestor)
                .filter(|(_, recursive)| ancestor == path || **recursive)
                .map(|(root, recursive)| (root, *recursive))
        })
        .collect()
}

fn common_ancestor(paths: &[&PathBuf]) -> Option<PathBuf> {
    let (first, rest) = paths.split_first()?;
    let mut ancestor: &Path = first;
    for path in rest {
        while !path.starts_with(ancestor) {
            ancestor = ancestor.parent()?;
        }
    }
    Some(ancestor.to_path_buf())
}

/// Paths to install: every wanted, non-failed path not already inside another
/// installable recursive path.
fn minimal_cover(
    wanted: &HashMap<PathBuf, bool>,
    failed: &HashSet<PathBuf>,
) -> HashMap<PathBuf, bool> {
    let cover: HashMap<_, _> = wanted
        .iter()
        .filter(|(path, _)| {
            !failed.contains(*path)
                && !path
                    .ancestors()
                    .skip(1)
                    .any(|parent| wanted.get(parent) == Some(&true) && !failed.contains(parent))
        })
        .map(|(path, recursive)| (path.clone(), *recursive))
        .collect();
    static VERIFY: OnceLock<bool> = OnceLock::new();
    static SAMPLES: AtomicUsize = AtomicUsize::new(0);
    if cfg!(debug_assertions)
        || (*VERIFY.get_or_init(|| {
            std::env::var_os("ZERON_PERF_VERIFY").is_some_and(|value| value == "1")
        }) && SAMPLES.fetch_add(1, Ordering::Relaxed) % 32 == 0)
    {
        let full = minimal_cover_full(wanted, failed);
        if full != cover {
            tracing::error!("fs-watch: cover oracle mismatch; rebuilding from full cover");
            return full;
        }
    }
    cover
}

/// Original pairwise cover calculation, retained as the equivalence oracle.
fn minimal_cover_full(
    wanted: &HashMap<PathBuf, bool>,
    failed: &HashSet<PathBuf>,
) -> HashMap<PathBuf, bool> {
    let candidates: Vec<_> = wanted
        .iter()
        .filter(|(path, _)| !failed.contains(*path))
        .collect();
    candidates
        .iter()
        .filter(|(path, _)| {
            !candidates
                .iter()
                .any(|(other, recursive)| **recursive && other != path && path.starts_with(other))
        })
        .map(|(path, recursive)| ((*path).clone(), **recursive))
        .collect()
}

fn dispatch(routes: &Mutex<Routes>, event: &notify::Event) {
    // Collect under the lock, call outside it: a handler must never be able to
    // stall registration bookkeeping.
    let handlers: Vec<Handler> = lock(routes)
        .regs
        .values()
        .filter(|reg| {
            event.need_rescan()
                || event.paths.is_empty()
                || event.paths.iter().any(|path| reg.covers(path))
        })
        .map(|reg| reg.handler.clone())
        .collect();
    for handler in handlers {
        handler(event);
    }
}

/// Kernel counts, rather than registration counts: useful for fixture checks
/// of deduplication and for catching notify's implicit recursive installs.
#[cfg(all(test, target_os = "linux"))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InotifyCounts {
    pub watches: usize,
    pub instances: usize,
    pub threads: usize,
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn inotify_counts() -> InotifyCounts {
    let mut counts = InotifyCounts {
        watches: 0,
        instances: 0,
        threads: 0,
    };
    for entry in std::fs::read_dir("/proc/self/fdinfo").unwrap().flatten() {
        let Ok(info) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let watches = info
            .lines()
            .filter(|line| line.starts_with("inotify wd:"))
            .count();
        // Empty inotify instances have no wd lines; identify the fd as well.
        let target = std::fs::read_link(Path::new("/proc/self/fd").join(entry.file_name()))
            .unwrap_or_default();
        if target.to_string_lossy().contains("inotify") {
            counts.instances += 1;
            counts.watches += watches;
        }
    }
    for entry in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        if std::fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|name| name.starts_with("notify-rs inoti"))
        {
            counts.threads += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    fn reg(path: &str, recursive: bool) -> Registration {
        Registration {
            path: PathBuf::from(path),
            recursive,
            tree: None,
            handler: Arc::new(|_| {}),
            attached: Arc::default(),
        }
    }

    /// Kind of signal a test handler saw.
    #[derive(Debug, PartialEq)]
    enum Seen {
        Event,
        Rescan,
    }

    fn recorder() -> (
        impl Fn(&notify::Event) + Send + Sync + 'static,
        UnboundedReceiver<Seen>,
    ) {
        let (tx, rx): (UnboundedSender<Seen>, _) = unbounded_channel();
        let handler = move |event: &notify::Event| {
            let _ = tx.send(if event.need_rescan() {
                Seen::Rescan
            } else {
                Seen::Event
            });
        };
        (handler, rx)
    }

    fn drain(rx: &mut UnboundedReceiver<Seen>) -> Vec<Seen> {
        let mut seen = Vec::new();
        while let Ok(signal) = rx.try_recv() {
            seen.push(signal);
        }
        seen
    }

    async fn expect(rx: &mut UnboundedReceiver<Seen>, want: Seen) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let got = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("{want:?} delivered before timeout"))
                .expect("channel open");
            if got == want {
                return;
            }
        }
    }

    /// Wait for the batch worker, then FSEvents' first deliveries, to settle.
    async fn settle(hub: &FsWatchHub) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&hub.batch).worker {
            assert!(Instant::now() < deadline, "batch worker idles");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        (tmp, root)
    }

    #[test]
    fn non_recursive_registration_sees_only_direct_children() {
        let flat = reg("/r/space", false);
        assert!(flat.covers(Path::new("/r/space")));
        assert!(flat.covers(Path::new("/r/space/.git")));
        assert!(!flat.covers(Path::new("/r/space/src/main.rs")));
        assert!(!flat.covers(Path::new("/r/spaceship")));
        let deep = reg("/r/space", true);
        assert!(deep.covers(Path::new("/r/space/src/main.rs")));
        assert!(!deep.covers(Path::new("/r/other/x")));
    }

    #[test]
    fn cover_dedupes_merges_modes_and_skips_nested_paths() {
        let wanted: HashMap<PathBuf, bool> = [
            ("/r/main", true),
            ("/r/main/.git/worktrees/wt", true), // inside a recursive watch
            ("/r/flat", false),
            ("/r/flat/sub", true), // a non-recursive parent covers nothing
        ]
        .into_iter()
        .map(|(p, r)| (PathBuf::from(p), r))
        .collect();
        let cover = minimal_cover(&wanted, &HashSet::new());
        let mut paths: Vec<_> = cover.keys().cloned().collect();
        paths.sort();
        assert_eq!(
            paths,
            ["/r/flat", "/r/flat/sub", "/r/main"]
                .map(PathBuf::from)
                .to_vec()
        );
        // A failed ancestor no longer covers its descendants.
        let failed: HashSet<PathBuf> = [PathBuf::from("/r/main")].into();
        assert!(
            minimal_cover(&wanted, &failed).contains_key(Path::new("/r/main/.git/worktrees/wt"))
        );
    }

    #[test]
    fn common_ancestor_of_survivors() {
        let a = PathBuf::from("/u/me/projects/a");
        let b = PathBuf::from("/u/me/projects/b/.git");
        let c = PathBuf::from("/u/me/.zeron/wt");
        assert_eq!(common_ancestor(&[&a]), Some(a.clone()));
        assert_eq!(
            common_ancestor(&[&a, &b]),
            Some(PathBuf::from("/u/me/projects"))
        );
        assert_eq!(common_ancestor(&[&a, &b, &c]), Some(PathBuf::from("/u/me")));
        assert_eq!(common_ancestor(&[]), None);
    }

    #[test]
    fn events_route_by_path_and_pathless_events_broadcast() {
        let routes = Mutex::new(Routes::default());
        let hits = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        for (id, (name, path, recursive)) in [
            ("a", "/r/a", true),
            ("b", "/r/b", true),
            ("flat", "/r", false),
        ]
        .into_iter()
        .enumerate()
        {
            let hits = hits.clone();
            lock(&routes).regs.insert(
                id as u64,
                Registration {
                    path: PathBuf::from(path),
                    recursive,
                    tree: None,
                    handler: Arc::new(move |_| lock(&hits).push(name)),
                    attached: Arc::default(),
                },
            );
        }
        let event =
            |path: &str| notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from(path));
        dispatch(&routes, &event("/r/a/src/x.rs"));
        assert_eq!(std::mem::take(&mut *lock(&hits)), vec!["a"]);
        dispatch(&routes, &event("/r/b"));
        let mut got = std::mem::take(&mut *lock(&hits));
        got.sort();
        assert_eq!(got, vec!["b", "flat"]); // `/r/b` is a direct child of `/r`
        dispatch(&routes, &notify::Event::new(notify::EventKind::Other));
        assert_eq!(lock(&hits).len(), 3, "a rescan reaches every registration");
    }

    /// End to end through one real watcher: two folders, each event reaches
    /// only its own registration, each gets one rescan when it goes live, and
    /// a dropped handle unwatches.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_watcher_routes_events_and_unwatches_on_drop() {
        let (_tmp, root) = tree();
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(a.join("deep")).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        let hub = FsWatchHub::new();
        let (on_a, mut a_rx) = recorder();
        let (on_b, mut b_rx) = recorder();
        let watch_a = hub.watch(&a, notify::RecursiveMode::Recursive, on_a);
        let watch_b = hub.watch(&b, notify::RecursiveMode::NonRecursive, on_b);
        expect(&mut a_rx, Seen::Rescan).await;
        expect(&mut b_rx, Seen::Rescan).await;
        assert!(watch_a.is_live() && watch_b.is_live());
        assert_eq!(hub.installed().len(), 2);
        settle(&hub).await;
        drain(&mut a_rx);
        drain(&mut b_rx);

        std::fs::write(a.join("deep/x.txt"), "x").unwrap();
        expect(&mut a_rx, Seen::Event).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(drain(&mut b_rx).is_empty(), "a's event must not reach b");

        std::fs::write(b.join("y.txt"), "y").unwrap();
        expect(&mut b_rx, Seen::Event).await;
        settle(&hub).await;
        drain(&mut a_rx);

        drop(watch_a);
        settle(&hub).await;
        assert_eq!(hub.installed().len(), 1, "dropped path unwatched");
        std::fs::write(a.join("deep/x.txt"), "xx").unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            drain(&mut a_rx).is_empty(),
            "a dropped handle receives nothing"
        );
        drop(watch_b);
    }

    /// A burst of registrations is one pass over the OS watcher (one stream
    /// restart on FSEvents), not one per path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_burst_applies_in_one_pass() {
        let (_tmp, root) = tree();
        let hub = FsWatchHub::new();
        let mut watches = Vec::new();
        for i in 0..8 {
            let dir = root.join(format!("d{i}"));
            std::fs::create_dir(&dir).unwrap();
            watches.push(hub.watch(&dir, notify::RecursiveMode::Recursive, |_| {}));
        }
        settle(&hub).await;
        assert!(watches.iter().all(FsWatch::is_live));
        assert_eq!(hub.installed().len(), 8);
        assert_eq!(hub.stats.passes.load(Ordering::Relaxed), 1);
        watches.clear();
        settle(&hub).await;
        assert!(hub.installed().is_empty());
        assert_eq!(
            hub.stats.passes.load(Ordering::Relaxed),
            2,
            "one pass for the drops"
        );
    }

    /// Changing the watch set restarts the FSEvents stream. With the bridge,
    /// surviving registrations stay covered and get no rescan; without it
    /// (bridge unavailable) each gets exactly one rescan per pass.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_gap_is_bridged_or_compensated() {
        let (_tmp, root) = tree();
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        for dir in [&a, &b, &c] {
            std::fs::create_dir(dir).unwrap();
        }
        let hub = FsWatchHub::new();
        let (on_a, mut a_rx) = recorder();
        let _watch_a = hub.watch(&a, notify::RecursiveMode::Recursive, on_a);
        expect(&mut a_rx, Seen::Rescan).await;
        settle(&hub).await;
        drain(&mut a_rx);

        let watch_b = hub.watch(&b, notify::RecursiveMode::Recursive, |_| {});
        settle(&hub).await;
        assert!(watch_b.is_live());
        assert_eq!(
            hub.stats.bridges.load(Ordering::Relaxed),
            1,
            "bridge opened"
        );
        assert!(
            !drain(&mut a_rx).contains(&Seen::Rescan),
            "a bridged restart needs no rescan"
        );
        std::fs::write(a.join("x"), "x").unwrap();
        expect(&mut a_rx, Seen::Event).await;
        settle(&hub).await;
        drain(&mut a_rx);

        hub.bridge_enabled.store(false, Ordering::Relaxed);
        let _watch_c = hub.watch(&c, notify::RecursiveMode::Recursive, |_| {});
        drop(watch_b);
        settle(&hub).await;
        let seen = drain(&mut a_rx);
        assert_eq!(
            seen.iter().filter(|s| **s == Seen::Rescan).count(),
            1,
            "one compensating rescan per pass: {seen:?}"
        );
    }

    #[test]
    fn missing_path_is_not_live_and_does_not_block_others() {
        let (_tmp, root) = tree();
        let hub = FsWatchHub::new();
        let (on_gone, mut gone_rx) = recorder();
        let gone = hub.watch(
            &root.join("gone"),
            notify::RecursiveMode::Recursive,
            on_gone,
        );
        let here = hub.watch(&root, notify::RecursiveMode::NonRecursive, |_| {});
        hub.apply();
        assert!(!gone.is_live());
        assert!(
            drain(&mut gone_rx).is_empty(),
            "a failed watch never attaches"
        );
        assert!(here.is_live());
        assert_eq!(hub.installed().keys().collect::<Vec<_>>(), vec![&root]);
        drop(here);
        drop(gone);
        hub.apply();
        assert!(hub.installed().is_empty());
    }

    #[test]
    fn overlapping_registrations_share_one_install_until_the_last_drops() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let hub = FsWatchHub::new();
        let flat = hub.watch(&root, notify::RecursiveMode::NonRecursive, |_| {});
        let nested = hub.watch(
            &root.join("nested"),
            notify::RecursiveMode::Recursive,
            |_| {},
        );
        let deep = hub.watch(&root, notify::RecursiveMode::Recursive, |_| {});
        hub.apply();
        // Same path merges to recursive; the nested path rides along inside it.
        assert_eq!(hub.installed(), HashMap::from([(root.clone(), true)]));
        assert!(nested.is_live());
        drop(deep);
        hub.apply();
        assert_eq!(
            hub.installed(),
            HashMap::from([(root.clone(), false), (root.join("nested"), true)])
        );
        drop(flat);
        drop(nested);
        hub.apply();
        assert!(hub.installed().is_empty());
    }

    /// A hub configured like inotify's: a watcher per root, no restarts.
    fn per_root_hub() -> Arc<FsWatchHub> {
        Arc::new_cyclic(|weak| FsWatchHub {
            weak: weak.clone(),
            shared_watcher: false,
            restarts_on_change: false,
            routes: Arc::default(),
            os: Mutex::default(),
            invalidated: Mutex::default(),
            batch: Mutex::default(),
            bridge_enabled: AtomicBool::new(true),
            stats: Stats::default(),
            after_walk: Mutex::default(),
        })
    }

    #[test]
    fn replaced_cover_rescans_the_registration() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for hub in [FsWatchHub::new(), per_root_hub()] {
            let (on_nested, mut nested_rx) = recorder();
            let _nested = hub.watch(
                &root.join("nested"),
                notify::RecursiveMode::Recursive,
                on_nested,
            );
            let deep = hub.watch(&root, notify::RecursiveMode::Recursive, |_| {});
            hub.apply();
            drain(&mut nested_rx);
            // `nested` rode inside `root`'s watch; it now gets its own, and
            // nothing observed the swap.
            drop(deep);
            hub.apply();
            assert_eq!(
                hub.installed(),
                HashMap::from([(root.join("nested"), true)])
            );
            assert!(drain(&mut nested_rx).contains(&Seen::Rescan));
            // A pass that leaves its cover alone sends nothing.
            let _other = hub.watch(&root, notify::RecursiveMode::NonRecursive, |_| {});
            hub.apply();
            assert!(!drain(&mut nested_rx).contains(&Seen::Rescan));
        }
    }

    #[tokio::test]
    async fn per_root_watchers_route_events_and_close_on_drop() {
        let (_tmp, root) = tree();
        let (a, b) = (root.join("a"), root.join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir(dir).unwrap();
        }
        let hub = per_root_hub();
        let (on_a, mut a_rx) = recorder();
        let watch_a = hub.watch(&a, notify::RecursiveMode::Recursive, on_a);
        let (on_b, mut b_rx) = recorder();
        let _watch_b = hub.watch(&b, notify::RecursiveMode::Recursive, on_b);
        hub.apply();
        assert_eq!(lock(&hub.os).roots.len(), 2);
        assert!(lock(&hub.os).watcher.is_none());
        settle(&hub).await;
        drain(&mut a_rx);
        drain(&mut b_rx);
        std::fs::write(b.join("x"), "x").unwrap();
        expect(&mut b_rx, Seen::Event).await;
        assert!(!drain(&mut a_rx).contains(&Seen::Event));
        drop(watch_a);
        hub.apply();
        assert_eq!(lock(&hub.os).roots.keys().collect::<Vec<_>>(), [&b]);
    }
    #[test]
    fn seeded_cover_matches_full_for_registration_mode_failure_and_drop_churn() {
        let seeds = std::env::var("PERF_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        for seed in 1..=seeds {
            let mut random = seed as u64;
            let mut next = || {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random
            };
            let paths: Vec<_> = (0..24)
                .map(|i| {
                    let mut path = PathBuf::from(format!("/r/{}", i % 4));
                    if i >= 4 {
                        path.push(format!("b{}", i % 3));
                    }
                    if i >= 12 {
                        path.push(format!("c{}", i % 2));
                    }
                    path
                })
                .collect();
            let mut wanted = HashMap::new();
            let mut failed = HashSet::new();
            for _ in 0..150 {
                let path = paths[next() as usize % paths.len()].clone();
                match next() % 5 {
                    0 => {
                        wanted.insert(path, true);
                    }
                    1 => {
                        wanted.insert(path, false);
                    }
                    2 => {
                        wanted.remove(&path);
                    }
                    3 => {
                        failed.insert(path);
                    }
                    _ => {
                        failed.remove(&path);
                    }
                }
                let cover = minimal_cover(&wanted, &failed);
                assert_eq!(cover, minimal_cover_full(&wanted, &failed), "seed {seed}");
                for path in &paths {
                    let full: std::collections::BTreeSet<_> = cover
                        .iter()
                        .filter(|(root, recursive)| {
                            path == *root || (**recursive && path.starts_with(root))
                        })
                        .map(|(root, recursive)| (root, *recursive))
                        .collect();
                    assert_eq!(covering_roots(path, &cover), full, "seed {seed}");
                    assert_eq!(is_covered(path, &cover), !full.is_empty(), "seed {seed}");
                }
            }
        }
    }

    fn record_events() -> (
        impl Fn(&notify::Event) + Send + Sync + 'static,
        UnboundedReceiver<notify::Event>,
    ) {
        let (tx, rx) = unbounded_channel();
        (
            move |event: &notify::Event| {
                let _ = tx.send(event.clone());
            },
            rx,
        )
    }

    async fn event_matching(
        rx: &mut UnboundedReceiver<notify::Event>,
        matches: impl Fn(&notify::Event) -> bool,
    ) -> notify::Event {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let event = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("event timeout")
                .expect("open channel");
            if matches(&event) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn pruned_tree_fans_out_native_create_modify_rename_delete_and_new_dirs() {
        use notify::EventKind;
        use notify::event::ModifyKind;
        let (_tmp, root) = tree();
        std::fs::create_dir(root.join("src")).unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let (on_b, mut b) = record_events();
        let wa = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        let wb = hub.watch_tree(&root, Arc::new(|_| true), on_b);
        hub.apply();
        assert!(wa.is_live() && wb.is_live());
        assert_eq!(
            hub.installed().len(),
            2,
            "one watch per directory for two consumers"
        );
        assert!(hub.installed().values().all(|recursive| !recursive));
        assert!(
            lock(&hub.os).roots.is_empty(),
            "no private recursive instances"
        );
        assert!(lock(&hub.os).watcher.is_some());
        for rx in [&mut a, &mut b] {
            event_matching(rx, notify::Event::need_rescan).await;
        }

        let file = root.join("src/a");
        std::fs::write(&file, "one").unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| {
                matches!(e.kind, EventKind::Create(_)) && e.paths.contains(&file)
            })
            .await;
        }
        std::fs::write(&file, "two").unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| {
                matches!(e.kind, EventKind::Modify(ModifyKind::Data(_))) && e.paths.contains(&file)
            })
            .await;
        }
        let renamed = root.join("src/b");
        std::fs::rename(&file, &renamed).unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| {
                matches!(
                    e.kind,
                    EventKind::Modify(ModifyKind::Name(_)) | EventKind::Create(_)
                ) && e.paths.contains(&renamed)
            })
            .await;
        }
        std::fs::remove_file(&renamed).unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| {
                matches!(e.kind, EventKind::Remove(_)) && e.paths.contains(&renamed)
            })
            .await;
        }
        std::fs::create_dir_all(root.join("new/deep")).unwrap();
        // Populate before watch attachment: the post-install rescan covers it.
        std::fs::write(root.join("new/deep/early"), "one").unwrap();
        settle(&hub).await;
        assert!(hub.installed().contains_key(&root.join("new/deep")));
        for rx in [&mut a, &mut b] {
            event_matching(rx, notify::Event::need_rescan).await;
        }
        let late = root.join("new/deep/late");
        std::fs::write(&late, "two").unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| e.paths.contains(&late)).await;
        }

        std::fs::rename(root.join("new"), root.join("moved")).unwrap();
        settle(&hub).await;
        assert!(!hub.installed().contains_key(&root.join("new/deep")));
        assert!(hub.installed().contains_key(&root.join("moved/deep")));
        let moved = root.join("moved/deep/late");
        std::fs::write(&moved, "three").unwrap();
        for rx in [&mut a, &mut b] {
            event_matching(rx, |e| e.paths.contains(&moved)).await;
        }
        std::fs::remove_dir_all(root.join("moved")).unwrap();
        settle(&hub).await;
        assert_eq!(hub.installed().len(), 2);
        drop(wa);
        hub.apply();
        assert!(wb.is_live());
        assert_eq!(hub.installed().len(), 2);
        drop(wb);
        hub.apply();
        assert!(hub.installed().is_empty());
        assert!(lock(&hub.os).watcher.is_none());
    }

    #[tokio::test]
    async fn pruned_tree_ignores_build_dirs_and_refreshes_ignore_rules() {
        let (_tmp, root) = tree();
        for dir in ["src", "target/deep", "node_modules/pkg", "hidden"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let _a = hub.watch_tree(
            &root,
            Arc::new(|p| p.file_name().is_none_or(|n| n != "hidden")),
            on_a,
        );
        hub.apply();
        assert_eq!(hub.installed().len(), 2);
        assert!(
            !exceeds_tree_budget(&root, 2),
            "ignored dirs do not spend the budget"
        );
        settle(&hub).await;
        while a.try_recv().is_ok() {}
        std::fs::write(root.join("target/deep/noise"), "x").unwrap();
        std::fs::write(root.join("node_modules/pkg/noise"), "x").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(a.try_recv().is_err(), "ignored subtree events stay pruned");
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(hub.installed().contains_key(&root.join("target/deep")));
        std::fs::write(root.join("target/deep/live"), "x").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&root.join("target/deep/live"))).await;
        std::fs::write(root.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(!hub.installed().contains_key(&root.join("target")));
    }

    #[tokio::test]
    async fn different_tree_filters_and_flat_consumers_share_paths_without_cross_delivery() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("private/deep")).unwrap();
        let hub = per_root_hub();
        let (on_filtered, mut filtered) = record_events();
        let (on_all, mut all) = record_events();
        let filtered_watch = hub.watch_tree(
            &root,
            Arc::new(|p| p.file_name().is_none_or(|name| name != "private")),
            on_filtered,
        );
        let all_watch = hub.watch_tree(&root, Arc::new(|_| true), on_all);
        let flat = hub.watch(&root, notify::RecursiveMode::NonRecursive, |_| {});
        hub.apply();
        settle(&hub).await;
        assert_eq!(
            hub.installed().len(),
            3,
            "flat and both tree roots share one OS watch"
        );
        while filtered.try_recv().is_ok() {}
        while all.try_recv().is_ok() {}
        let file = root.join("private/deep/file");
        std::fs::write(&file, "x").unwrap();
        event_matching(&mut all, |e| e.paths.contains(&file)).await;
        assert!(filtered.try_recv().is_err());
        drop(all_watch);
        hub.apply();
        assert_eq!(
            hub.installed().len(),
            1,
            "excluded subtree released with its last consumer"
        );
        assert!(filtered_watch.is_live() && flat.is_live());
    }

    #[tokio::test]
    async fn pruned_tree_overflow_broadcasts_and_rebuilds_missed_directories() {
        let (_tmp, root) = tree();
        let hub = per_root_hub();
        let (on_a, mut a) = recorder();
        let (on_b, mut b) = recorder();
        let _a = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        let (on_flat, mut flat_rx) = recorder();
        let _flat = hub.watch(&root, notify::RecursiveMode::NonRecursive, on_flat);
        // A second consumer of a different root still gets the loss signal.
        std::fs::create_dir(root.join("other")).unwrap();
        let _b = hub.watch_tree(&root.join("other"), Arc::new(|_| true), on_b);
        hub.apply();
        settle(&hub).await;
        drain(&mut a);
        drain(&mut b);
        drain(&mut flat_rx);
        // Simulate an event lost before delivery: create while the directory's
        // OS watch is absent, then feed notify's exact IN_Q_OVERFLOW signal.
        {
            let mut os = lock(&hub.os);
            os.watcher.as_mut().unwrap().unwatch(&root).unwrap();
        }
        std::fs::create_dir_all(root.join("missed/deep")).unwrap();
        hub.on_event(rescan_event().add_path(root.join("unrelated")));
        expect(&mut a, Seen::Rescan).await;
        expect(&mut b, Seen::Rescan).await;
        expect(&mut flat_rx, Seen::Rescan).await;
        hub.apply();
        expect(&mut flat_rx, Seen::Rescan).await; // closes the shared rearm gap
        assert!(hub.installed().contains_key(&root.join("missed/deep")));
        std::fs::write(root.join("missed/deep/file"), "x").unwrap();
        expect(&mut a, Seen::Event).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pruned_tree_symlink_into_other_root_does_not_alias_shared_instance() {
        let (_tmp, root) = tree();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir_all(b.join("deep")).unwrap();
        std::os::unix::fs::symlink(&b, a.join("linked")).unwrap();
        let hub = per_root_hub();
        let (on_a, mut a_rx) = record_events();
        let (on_b, mut b_rx) = record_events();
        let wa = hub.watch_tree(&a, Arc::new(|_| true), on_a);
        let wb = hub.watch_tree(&b, Arc::new(|_| true), on_b);
        hub.apply();
        settle(&hub).await;
        while a_rx.try_recv().is_ok() {}
        while b_rx.try_recv().is_ok() {}
        assert_eq!(hub.installed().len(), 3);
        let file = b.join("deep/file");
        std::fs::write(&file, "x").unwrap();
        event_matching(&mut b_rx, |e| e.paths.contains(&file)).await;
        assert!(
            a_rx.try_recv().is_err(),
            "symlink aliases do not relabel events"
        );
        drop(wa);
        hub.apply();
        assert!(wb.is_live());
        std::fs::write(&file, "xx").unwrap();
        event_matching(&mut b_rx, |e| e.paths.contains(&file)).await;
    }

    /// A directory created after the walk but before its parent's watch
    /// installs produces no event and is not in the walked set: nothing would
    /// ever attach it, and a later edit inside it would go unseen until some
    /// unrelated topology change.
    #[tokio::test]
    async fn directory_created_between_walk_and_install_is_still_attached() {
        let (_tmp, root) = tree();
        std::fs::create_dir(root.join("a")).unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let _a = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        let racing = root.join("a/raced/deep");
        let hook_path = racing.clone();
        *lock(&hub.after_walk) = Some(Arc::new(move || {
            std::fs::create_dir_all(&hook_path).unwrap();
        }));
        hub.apply();
        settle(&hub).await;
        assert!(
            hub.installed().contains_key(&racing),
            "installed: {:?}",
            hub.installed()
        );
        while a.try_recv().is_ok() {}
        let file = racing.join("file");
        std::fs::write(&file, "x").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&file)).await;
    }

    /// Ignore inputs outside the registered root (an ancestor `.gitignore`, the
    /// global excludes, a linked worktree's shared `info/exclude`) raise no
    /// event under it. The consumers' repair ticks call `refresh`, which must
    /// pick up the change, and cost nothing downstream when nothing changed.
    #[tokio::test]
    async fn refresh_picks_up_ignore_changes_outside_the_root() {
        let (_tmp, base) = tree();
        let root = base.join("proj");
        std::fs::create_dir_all(root.join("build/inner")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(base.join(".gitignore"), "build/\n").unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let watch = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        hub.apply();
        settle(&hub).await;
        assert!(!hub.installed().contains_key(&root.join("build")));
        while a.try_recv().is_ok() {}

        // Nothing changed: a refresh walks, but no consumer hears of it.
        watch.refresh();
        settle(&hub).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The walk itself reads directories, which raises Access events; the
        // consumers drop those.
        while let Ok(event) = a.try_recv() {
            assert!(
                matches!(event.kind, notify::EventKind::Access(_)),
                "an unchanged tree stays silent: {event:?}"
            );
        }

        std::fs::write(base.join(".gitignore"), "").unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !hub.installed().contains_key(&root.join("build")),
            "no native event reaches the hub for a file outside the root"
        );
        watch.refresh();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(hub.installed().contains_key(&root.join("build/inner")));
        let file = root.join("build/inner/out");
        std::fs::write(&file, "x").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&file)).await;
    }

    #[tokio::test]
    async fn git_info_exclude_edits_inside_the_root_reconcile_without_refresh() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::create_dir_all(root.join("out/deep")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "out/\n").unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let _watch = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        hub.apply();
        settle(&hub).await;
        assert!(!hub.installed().contains_key(&root.join("out")));
        tokio::time::sleep(Duration::from_millis(200)).await;
        while a.try_recv().is_ok() {}
        std::fs::write(root.join(".git/info/exclude"), "").unwrap();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(hub.installed().contains_key(&root.join("out/deep")));
    }

    #[tokio::test]
    async fn directory_renamed_into_the_root_from_outside_is_walked_and_rescanned() {
        let (_tmp, base) = tree();
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(outside.join("x/y")).unwrap();
        std::fs::write(outside.join("x/y/early"), "1").unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let _a = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        hub.apply();
        settle(&hub).await;
        while a.try_recv().is_ok() {}
        std::fs::rename(&outside, root.join("in")).unwrap();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(hub.installed().contains_key(&root.join("in/x/y")));
        let late = root.join("in/x/y/late");
        std::fs::write(&late, "2").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&late)).await;
    }

    #[tokio::test]
    async fn directory_renamed_out_and_back_in_stays_watched() {
        for settle_between in [false, true] {
            let (_tmp, base) = tree();
            let root = base.join("root");
            std::fs::create_dir_all(root.join("d/s")).unwrap();
            let hub = per_root_hub();
            let (on_a, mut a) = record_events();
            let _a = hub.watch_tree(&root, Arc::new(|_| true), on_a);
            hub.apply();
            settle(&hub).await;
            std::fs::rename(root.join("d"), base.join("out")).unwrap();
            if settle_between {
                settle(&hub).await;
                assert!(!hub.installed().contains_key(&root.join("d/s")));
            }
            std::fs::rename(base.join("out"), root.join("d")).unwrap();
            settle(&hub).await;
            assert!(
                hub.installed().contains_key(&root.join("d/s")),
                "settle_between={settle_between}: {:?}",
                hub.installed()
            );
            while a.try_recv().is_ok() {}
            let file = root.join("d/s/f");
            std::fs::write(&file, "x").unwrap();
            event_matching(&mut a, |e| e.paths.contains(&file)).await;
        }
    }

    /// A real IN_Q_OVERFLOW: a handler that stalls notify's reader thread lets
    /// the kernel queue (16384 events by default) fill, so the directory made
    /// afterwards is lost without a trace except the overflow event itself.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kernel_queue_overflow_rescans_and_attaches_directories_whose_creation_was_lost() {
        let queue: usize = std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let (_tmp, root) = tree();
        std::fs::create_dir(root.join("flood")).unwrap();
        let hub = per_root_hub();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let stalled = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = unbounded_channel::<notify::Event>();
        let handler_gate = gate.clone();
        let _watch = hub.watch_tree(&root, Arc::new(|_| true), move |event| {
            if !event.need_rescan()
                && !matches!(event.kind, notify::EventKind::Access(_))
                && !stalled.swap(true, Ordering::AcqRel)
            {
                let (open, signal) = &*handler_gate;
                let _held = signal
                    .wait_while(lock(open), |open| !*open)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            let _ = tx.send(event.clone());
        });
        hub.apply();
        settle(&hub).await;
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("trigger"), "x").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await; // reader is now parked
        for i in 0..queue {
            std::fs::File::create(root.join("flood").join(i.to_string())).unwrap();
        }
        std::fs::create_dir_all(root.join("lost/deep")).unwrap();
        std::fs::write(root.join("lost/deep/early"), "1").unwrap();
        {
            let (open, signal) = &*gate;
            *lock(open) = true;
            signal.notify_all();
        }
        let before_overflow = Mutex::new(Vec::new());
        event_matching(&mut rx, |event| {
            lock(&before_overflow).push(event.clone());
            event.need_rescan()
        })
        .await;
        let before_overflow = before_overflow.into_inner().unwrap();
        // The kernel dropped everything after the queue filled, so the
        // directory made last must never have been reported.
        eprintln!("events before overflow: {}", before_overflow.len());
        assert!(
            before_overflow.iter().all(|event| event
                .paths
                .iter()
                .all(|path| !path.starts_with(root.join("lost")))),
            "the creation was not lost: no overflow happened"
        );
        settle(&hub).await;
        assert!(
            hub.installed().contains_key(&root.join("lost/deep")),
            "overflow repairs the directory set"
        );
        while rx.try_recv().is_ok() {}
        let late = root.join("lost/deep/late");
        std::fs::write(&late, "2").unwrap();
        event_matching(&mut rx, |e| e.paths.contains(&late)).await;
    }

    /// Stands in for inotify ENOSPC: a directory whose watch cannot install
    /// must leave the tree not-live (the consumers' repair ticks keep running)
    /// while the rest keeps delivering, and recover once the cause is gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn unwatchable_directory_leaves_the_tree_not_live_and_recovers() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("ok")).unwrap();
        std::fs::create_dir_all(root.join("locked/inner")).unwrap();
        let lock_dir = |mode| {
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(mode))
                .unwrap()
        };
        lock_dir(0o000);
        if std::fs::read_dir(root.join("locked")).is_ok() {
            lock_dir(0o755);
            return; // privileged user: permissions do not apply
        }
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let watch = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        hub.apply();
        settle(&hub).await;
        assert!(!watch.is_live(), "an unwatchable directory is not live");
        while a.try_recv().is_ok() {}
        let file = root.join("ok/f");
        std::fs::write(&file, "x").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&file)).await;
        lock_dir(0o755);
        watch.refresh();
        event_matching(&mut a, notify::Event::need_rescan).await;
        assert!(watch.is_live());
        assert!(hub.installed().contains_key(&root.join("locked/inner")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pruned_tree_does_not_follow_symlinked_directories_or_cycles() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("real/sub")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("real/sub/loop")).unwrap();
        let hub = per_root_hub();
        let (on_a, mut a) = record_events();
        let watch = hub.watch_tree(&root, Arc::new(|_| true), on_a);
        hub.apply();
        settle(&hub).await;
        assert!(watch.is_live());
        let mut installed: Vec<_> = hub.installed().into_keys().collect();
        installed.sort();
        assert_eq!(
            installed,
            vec![root.clone(), root.join("real"), root.join("real/sub")]
        );
        while a.try_recv().is_ok() {}
        // The kernel reports a write through the alias under the real path.
        std::fs::write(root.join("alias/sub/f"), "x").unwrap();
        event_matching(&mut a, |e| e.paths.contains(&root.join("real/sub/f"))).await;
    }

    async fn churn(hub: &Arc<FsWatchHub>, root: &Path, cycles: usize) {
        for i in 0..cycles {
            let tree_watch = hub.watch_tree(root, Arc::new(|_| true), |_| {});
            let flat = hub.watch(
                &root.join("src"),
                notify::RecursiveMode::NonRecursive,
                |_| {},
            );
            let legacy = hub.watch(&root.join("src"), notify::RecursiveMode::Recursive, |_| {});
            if i % 2 == 0 {
                hub.apply();
                assert!(tree_watch.is_live() && flat.is_live() && legacy.is_live());
            }
            drop((tree_watch, flat, legacy));
            if i % 3 == 0 {
                hub.apply();
            }
        }
        settle(hub).await;
        hub.apply();
    }

    fn assert_idle(hub: &FsWatchHub) {
        let os = lock(&hub.os);
        assert!(os.installed.is_empty(), "installed: {:?}", os.installed);
        assert!(os.watcher.is_none() && os.roots.is_empty() && os.failed.is_empty());
        assert!(lock(&hub.routes).regs.is_empty());
        assert!(!lock(&hub.batch).worker);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_unregister_cycles_leave_no_watchers_or_workers() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let hub = per_root_hub();
        churn(&hub, &root, 120).await;
        assert_idle(&hub);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "process-wide kernel measurement; run alone with --ignored --nocapture"]
    async fn register_unregister_cycles_leave_no_inotify_fds_or_threads() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let before = inotify_counts();
        let hub = per_root_hub();
        churn(&hub, &root, 200).await;
        assert_idle(&hub);
        // notify's reader thread exits asynchronously after its instance closes.
        let deadline = Instant::now() + Duration::from_secs(5);
        while inotify_counts() != before {
            assert!(
                Instant::now() < deadline,
                "{:?} vs {before:?}",
                inotify_counts()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let batch_threads = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .filter(|task| {
                std::fs::read_to_string(task.path().join("comm"))
                    .is_ok_and(|name| name.starts_with("fs-watch batch"))
            })
            .count();
        assert_eq!(batch_threads, 0);
    }

    /// Handlers are called outside every hub lock and a dropped handle stops
    /// delivery: after a drop returns, nothing further may arrive beyond events
    /// already collected for dispatch, and no registration change deadlocks
    /// against a busy event stream.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsubscribing_during_an_event_stream_neither_deadlocks_nor_leaks_deliveries() {
        let (_tmp, root) = tree();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let hub = per_root_hub();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (root, stop) = (root.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Acquire) {
                    n += 1;
                    let _ = std::fs::write(root.join("src").join(format!("f{}", n % 64)), "x");
                    if n % 16 == 0 {
                        let dir = root.join(format!("d{}", n % 8));
                        let _ = std::fs::create_dir_all(dir.join("sub"));
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                }
            })
        };
        let anchor = hub.watch_tree(&root, Arc::new(|_| true), |_| {});
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut cycles = 0;
        while Instant::now() < deadline {
            let delivered = Arc::new(AtomicUsize::new(0));
            let counter = delivered.clone();
            let watch = hub.watch_tree(&root, Arc::new(|_| true), move |_| {
                counter.fetch_add(1, Ordering::AcqRel);
            });
            tokio::time::sleep(Duration::from_millis(30)).await;
            drop(watch);
            // At most a handler call already past the route lookup may land.
            tokio::time::sleep(Duration::from_millis(30)).await;
            let settled = delivered.load(Ordering::Acquire);
            tokio::time::sleep(Duration::from_millis(60)).await;
            assert_eq!(delivered.load(Ordering::Acquire), settled);
            cycles += 1;
        }
        stop.store(true, Ordering::Release);
        writer.join().unwrap();
        assert!(cycles > 5);
        drop(anchor);
        settle(&hub).await;
        hub.apply();
        assert_idle(&hub);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "process-wide kernel measurement; run alone with --ignored --nocapture"]
    fn fixture_inotify_counts_before_and_after() {
        let (_tmp, root) = tree();
        for dir in [
            ".git/refs/heads",
            "src/deep",
            "target/debug/deps",
            "node_modules/pkg/lib",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        std::fs::write(root.join("src/deep/a.rs"), "// source").unwrap();
        let idle = inotify_counts();
        let search = crate::WorkspaceSearch::new();
        search.warm(&root).unwrap();
        assert!(search.wait_until_indexed(&root, Duration::from_secs(20)));
        let deadline = Instant::now() + Duration::from_secs(20);
        while inotify_counts().instances == idle.instances {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(500));
        let fff = inotify_counts();
        let mut old_diff = notify::recommended_watcher(|_| {}).unwrap();
        let mut old_files = notify::recommended_watcher(|_| {}).unwrap();
        old_diff
            .watch(&root, notify::RecursiveMode::Recursive)
            .unwrap();
        old_files
            .watch(&root, notify::RecursiveMode::Recursive)
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let before = inotify_counts();
        drop(old_diff);
        drop(old_files);
        let deadline = Instant::now() + Duration::from_secs(5);
        while inotify_counts() != fff {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let hub = per_root_hub();
        let a = hub.watch_tree(&root, Arc::new(|_| true), |_| {});
        let b = hub.watch_tree(&root, Arc::new(|_| true), |_| {});
        let git = hub.watch_tree(&root.join(".git"), Arc::new(|_| true), |_| {});
        hub.apply();
        let deadline = Instant::now() + Duration::from_secs(5);
        while inotify_counts().threads != idle.threads + 2 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let after = inotify_counts();
        println!("fixture: idle={idle:?}, fff_only={fff:?}, before={before:?}, after={after:?}");
        assert_eq!(before.instances - idle.instances, 3);
        assert_eq!(after.instances - idle.instances, 2);
        assert_eq!(after.threads - idle.threads, 2);
        assert_eq!(
            hub.installed().len(),
            6,
            "root + src/deep + .git/refs/heads"
        );
        assert!(after.watches < before.watches);
        drop((a, b, git));
        hub.apply();
        search.release_all();
    }
}
