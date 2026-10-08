//! FsWatchHub — one `notify` watcher shared by every subsystem that live-watches
//! folders (diff-sync checkouts, space folders).
//!
//! Every `notify::RecommendedWatcher` owns a dedicated OS thread (the FSEvents
//! run loop on macOS, the inotify loop on Linux). One watcher per checkout and
//! per space folder put ~25-30 idle threads on a normal device at boot. The hub
//! instead installs every path on a single watcher and routes each event to
//! the registrations whose path covers it, so the whole engine pays for one
//! watcher thread. Exception: inotify/kqueue get one watcher per installed
//! root (see [`SHARED_WATCHER`]); they still share the routing and batching.
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
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use notify::Watcher as _;

/// A batch applies once registrations have been quiet this long…
const BATCH_QUIET: Duration = Duration::from_millis(100);
/// …or this long after its first change, whichever comes first.
const BATCH_MAX: Duration = Duration::from_secs(1);
/// Whether a watch change restarts the stream for every path (FSEvents).
const STREAM_RESTARTS_ON_CHANGE: bool = cfg!(target_os = "macos");
/// Whether every root can share one watcher. inotify (and kqueue) build a
/// recursive watch by walking the tree *following symlinks*, so in a shared
/// instance a checkout that symlinks into another checkout (`npm link`) would
/// alias that checkout's watch descriptors: its events would be relabelled
/// and unwatching either root would strip the other. Those backends get one
/// watcher per installed root instead, exactly as before the hub.
const SHARED_WATCHER: bool = cfg!(any(target_os = "macos", windows));

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
    handler: Handler,
    /// Shared with the [`FsWatch`]: an installed OS watch covers `path`.
    attached: Arc<AtomicBool>,
}

impl Registration {
    fn covers(&self, path: &Path) -> bool {
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
    /// The shared watcher ([`SHARED_WATCHER`] backends). Created lazily on the
    /// first install. On macOS an FSEvents watcher with no paths has no
    /// run-loop thread at all.
    watcher: Option<notify::RecommendedWatcher>,
    /// One watcher per installed root on the other backends. Dropping one
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
    /// Serializes OS watch changes.
    os: Mutex<OsState>,
    batch: Mutex<Batch>,
    restarts_on_change: bool,
    /// [`SHARED_WATCHER`]; a field so tests can drive both modes.
    shared_watcher: bool,
    bridge_enabled: AtomicBool,
    #[cfg_attr(not(test), allow(dead_code))]
    stats: Stats,
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
        Arc::new(Self {
            routes: Arc::new(Mutex::new(Routes::default())),
            os: Mutex::new(OsState::default()),
            batch: Mutex::new(Batch::default()),
            restarts_on_change: STREAM_RESTARTS_ON_CHANGE,
            shared_watcher: SHARED_WATCHER,
            bridge_enabled: AtomicBool::new(true),
            stats: Stats::default(),
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
                    recursive: matches!(mode, notify::RecursiveMode::Recursive),
                    handler: Arc::new(handler),
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

    /// Note a registration change; start a batch worker unless one is pending.
    /// The worker is a short-lived plain thread (not the tokio blocking pool):
    /// a task queued on a runtime that shuts down before running it would
    /// leave `worker` set and wedge the hub for the rest of the process.
    fn schedule(self: &Arc<Self>) {
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
            tracing::debug!(error = %err, "fs-watch: batch worker spawn failed; applying inline");
            lock(&self.batch).worker = false;
            self.apply();
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
        let rescans = {
            let mut os = lock(&self.os);
            self.reconcile_os(&mut os)
        };
        let event = rescan_event();
        for handler in rescans {
            handler(&event);
        }
    }

    fn reconcile_os(&self, os: &mut OsState) -> Vec<Handler> {
        // Wanted set: distinct paths, recursive if any registration recurses.
        let mut wanted: HashMap<PathBuf, bool> = HashMap::new();
        for reg in lock(&self.routes).regs.values() {
            *wanted.entry(reg.path.clone()).or_default() |= reg.recursive;
        }
        os.failed.retain(|path| wanted.contains_key(path));
        let before = os.installed.clone();
        let mut bridge: Option<notify::RecommendedWatcher> = None;
        let mut bridge_tried = false;
        let mut touched = false;
        // A failed install may uncover paths it would have covered — iterate
        // until a pass adds no new failures (bounded: `failed` only grows).
        loop {
            let desired = minimal_cover(&wanted, &os.failed);
            if desired == os.installed {
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
                .filter(|(path, recursive)| desired.get(*path) != Some(*recursive))
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
            let mut new_failure = false;
            for (path, recursive) in desired {
                if os.installed.get(&path) == Some(&recursive) {
                    continue;
                }
                match install(
                    os,
                    &self.routes,
                    self.shared_watcher,
                    &path,
                    recursive,
                    &mut touched,
                ) {
                    Ok(()) => {
                        os.installed.insert(path, recursive);
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

        let routes = lock(&self.routes);
        let mut rescans = Vec::new();
        for reg in routes.regs.values() {
            if !is_covered(&reg.path, &os.installed) {
                reg.attached.store(false, Ordering::Release);
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
            if !was_attached || moved || (gap && is_covered(&reg.path, &before)) {
                rescans.push(reg.handler.clone());
            }
        }
        rescans
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
        let routes = self.routes.clone();
        let opened = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                dispatch(&routes, &event);
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
    routes: &Arc<Mutex<Routes>>,
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
    let new_watcher = || {
        let routes = routes.clone();
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => dispatch(&routes, &event),
            Err(err) => tracing::debug!(error = %err, "fs-watch: watcher error"),
        })
    };
    let mode = if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    if !shared {
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

fn root_covers(root: &Path, recursive: bool, path: &Path) -> bool {
    path == root || (recursive && path.starts_with(root))
}

fn is_covered(path: &Path, installed: &HashMap<PathBuf, bool>) -> bool {
    installed
        .iter()
        .any(|(root, recursive)| root_covers(root, *recursive, path))
}

fn covering_roots<'a>(
    path: &Path,
    installed: &'a HashMap<PathBuf, bool>,
) -> std::collections::BTreeSet<(&'a PathBuf, bool)> {
    installed
        .iter()
        .filter(|(root, recursive)| root_covers(root, **recursive, path))
        .map(|(root, recursive)| (root, *recursive))
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
    let candidates: Vec<(&PathBuf, bool)> = wanted
        .iter()
        .filter(|(path, _)| !failed.contains(*path))
        .map(|(path, recursive)| (path, *recursive))
        .collect();
    candidates
        .iter()
        .filter(|(path, _)| {
            !candidates
                .iter()
                .any(|(other, recursive)| *recursive && other != path && path.starts_with(other))
        })
        .map(|(path, recursive)| ((*path).clone(), *recursive))
        .collect()
}

fn dispatch(routes: &Mutex<Routes>, event: &notify::Event) {
    // Collect under the lock, call outside it: a handler must never be able to
    // stall registration bookkeeping.
    let handlers: Vec<Handler> = lock(routes)
        .regs
        .values()
        .filter(|reg| event.paths.is_empty() || event.paths.iter().any(|path| reg.covers(path)))
        .map(|reg| reg.handler.clone())
        .collect();
    for handler in handlers {
        handler(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    fn reg(path: &str, recursive: bool) -> Registration {
        Registration {
            path: PathBuf::from(path),
            recursive,
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
        let hub = Arc::into_inner(FsWatchHub::new()).expect("fresh hub");
        Arc::new(FsWatchHub {
            shared_watcher: false,
            restarts_on_change: false,
            ..hub
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
}
