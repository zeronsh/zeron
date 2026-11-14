//! Names-only index of the host's home folder, for chats that still run in
//! `~` and the new-project folder search.
//!
//! Home is never handed to fff: after its scan, fff opens every file up to
//! 10 MB to tell text from binary, which downloads every cloud-only file
//! under home (OneDrive on Windows, iCloud Drive and File Provider folders
//! on macOS). This walk reads directory listings only, and skips cloud
//! folders, hidden folders and dependency folders altogether. It holds no
//! file contents, so home has no content search.

//!
//! The walk is bounded and never polls. Its workers share a queue of folders
//! and sleep on a condition variable while it's empty, so a worker held up
//! in one folder costs the others nothing (the `ignore` crate's parallel
//! walker had its idle workers wake every millisecond until all were done:
//! one `open()` that never returned, a macOS privacy prompt nobody answers
//! for Desktop or Documents, kept ~4,000 wakeups a second going for good).
//! It ends at [`WALK_DEADLINE`] with what it has, or when cancelled (the new
//! project picker closed); a folder still being read then is skipped by
//! later walks until that read returns, so a stuck folder holds one thread,
//! not one per walk.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crate::workspace_search::{NameKinds, NameMatch, normalize_name_query};

/// Paths kept at most; a larger home answers from the first ones walked.
pub const MAX_ENTRIES: usize = 250_000;

/// A walk answers with what it has found by then.
pub const WALK_DEADLINE: Duration = Duration::from_secs(20);

/// Folders above this depth (home's own folders, and theirs) that aren't
/// repositories are listed one by one, each child a unit of its own, so the
/// workers share a home whose projects sit under one folder. A repository,
/// or anything deeper, is walked whole by one worker with its ignore files.
const EXPAND_DEPTH: usize = 3;

/// Folders a walk that ended left a worker reading (see the module docs).
static STALLED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn stalled() -> MutexGuard<'static, Option<HashSet<PathBuf>>> {
    STALLED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Stops a walk: it returns at once with what it has.
#[derive(Default)]
pub struct WalkCancel {
    cancelled: AtomicBool,
    walk: Mutex<Weak<Walk>>,
}

impl WalkCancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(walk) = self
            .walk
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upgrade()
        {
            walk.quit();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

struct Unit {
    dir: PathBuf,
    /// Below home: 1 for home's own folders.
    depth: usize,
}

#[derive(Default)]
struct Queue {
    units: VecDeque<Unit>,
    /// Workers inside a unit.
    active: usize,
    /// Folders a worker is inside now.
    reading: HashSet<PathBuf>,
    quit: bool,
}

/// One walk's shared state. Workers hold it until they leave, which a
/// worker stuck in `open()` does only once that returns.
struct Walk {
    home: PathBuf,
    queue: Mutex<Queue>,
    changed: Condvar,
    /// `queue.quit`, read without the lock between entries.
    stop: AtomicBool,
    found: Mutex<Vec<HomePath>>,
    /// Folders skipped as stalled by an earlier walk.
    skip: HashSet<PathBuf>,
    /// Times an idle worker woke (tests: idle workers must not poll).
    #[cfg(test)]
    wakes: std::sync::atomic::AtomicUsize,
}

impl Walk {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn quit(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.queue().quit = true;
        self.changed.notify_all();
    }

    fn quitting(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn push(&self, unit: Unit) {
        if self.skip.contains(&unit.dir) {
            tracing::info!(dir = %unit.dir.display(), "home walk: skipping a folder an earlier walk is still reading");
            return;
        }
        self.queue().units.push_back(unit);
        self.changed.notify_one();
    }

    /// Keep `items`; `false` once the walk has enough.
    fn keep(&self, items: &mut Vec<HomePath>) -> bool {
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        found.append(items);
        if found.len() >= MAX_ENTRIES {
            drop(found);
            self.quit();
            return false;
        }
        true
    }

    /// A worker: units until the queue is empty with nobody working (the
    /// walk is done) or the walk quits; asleep while it waits.
    fn work(self: Arc<Self>) {
        loop {
            let unit = {
                let mut queue = self.queue();
                loop {
                    if queue.quit {
                        return;
                    }
                    if let Some(unit) = queue.units.pop_front() {
                        queue.active += 1;
                        queue.reading.insert(unit.dir.clone());
                        break unit;
                    }
                    if queue.active == 0 {
                        // Done: wake the others and the walk's owner.
                        self.changed.notify_all();
                        return;
                    }
                    queue = self
                        .changed
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                    #[cfg(test)]
                    self.wakes.fetch_add(1, Ordering::Relaxed);
                }
            };
            self.visit(&unit);
            let quit = {
                let mut queue = self.queue();
                queue.active -= 1;
                queue.reading.remove(&unit.dir);
                if queue.active == 0 && queue.units.is_empty() {
                    self.changed.notify_all();
                }
                queue.quit
            };
            // A walk that ended while this folder was read marked it stalled.
            if let Some(stalled) = stalled().as_mut()
                && stalled.remove(&unit.dir)
            {
                tracing::info!(dir = %unit.dir.display(), "home walk: a stalled folder read returned");
            }
            if quit {
                return;
            }
        }
    }

    fn visit(&self, unit: &Unit) {
        #[cfg(test)]
        tests::held(&unit.dir);
        let mut items = Vec::new();
        if unit.depth < EXPAND_DEPTH && !unit.dir.join(".git").exists() {
            let Ok(listing) = std::fs::read_dir(&unit.dir) else {
                return;
            };
            for entry in listing {
                if self.quitting() {
                    return;
                }
                let Ok(entry) = entry else { continue };
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let depth = unit.depth + 1;
                if is_dir && skipped(&name, depth, || entry.metadata().ok()) {
                    continue;
                }
                let path = entry.path();
                if let Some(home_path) = self.home_path(&path, is_dir) {
                    items.push(home_path);
                }
                if is_dir {
                    self.push(Unit { dir: path, depth });
                }
            }
            self.keep(&mut items);
            return;
        }
        let base = unit.depth;
        let walk = ignore::WalkBuilder::new(&unit.dir)
            .hidden(true)
            .follow_links(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(false)
            .filter_entry(move |entry| {
                entry.depth() == 0
                    || !entry.file_type().is_some_and(|kind| kind.is_dir())
                    || !skipped(
                        &entry.file_name().to_string_lossy().to_lowercase(),
                        base + entry.depth(),
                        || entry.metadata().ok(),
                    )
            })
            .build();
        for entry in walk {
            if self.quitting() {
                return;
            }
            let Ok(entry) = entry else { continue };
            if entry.depth() == 0 {
                continue;
            }
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            if let Some(home_path) = self.home_path(entry.path(), is_dir) {
                items.push(home_path);
            }
            if items.len() >= 512 && !self.keep(&mut items) {
                return;
            }
        }
        self.keep(&mut items);
    }

    fn home_path(&self, path: &Path, is_dir: bool) -> Option<HomePath> {
        let relative = path.strip_prefix(&self.home).ok()?;
        let path = relative.to_string_lossy().replace('\\', "/");
        Some(HomePath {
            lower: path.to_lowercase(),
            path,
            is_dir,
        })
    }
}

/// Folders directly under home that a cloud client syncs (matched without
/// case; OneDrive's business folders read "OneDrive - Contoso").
const CLOUD_ROOT_PREFIXES: &[&str] = &[
    "onedrive",
    "icloud drive",
    "iclouddrive",
    "dropbox",
    "google drive",
    "my drive",
    "box",
    "box sync",
    "pcloud drive",
    "creative cloud files",
];

/// Folders directly under home that hold machine state, not user files.
/// macOS's Library also holds iCloud Drive (`Mobile Documents`) and File
/// Provider clients (`CloudStorage`).
const MACHINE_ROOTS: &[&str] = &["library", "appdata", "applications"];

/// Dependency and cache folders, at any depth.
const SKIPPED_DIRS: &[&str] = &["node_modules", "__pycache__", "venv", "site-packages"];

pub struct HomeIndex {
    entries: Vec<HomePath>,
    pub built: Instant,
}

struct HomePath {
    /// Home-relative, `/`-separated.
    path: String,
    lower: String,
    is_dir: bool,
}

impl HomeIndex {
    /// Walk `home` (canonical), for at most [`WALK_DEADLINE`]. `cancel`
    /// stops the walk early.
    pub fn build(home: &Path, cancel: &WalkCancel) -> Self {
        Self::build_within(home, cancel, WALK_DEADLINE)
    }

    fn build_within(home: &Path, cancel: &WalkCancel, deadline: Duration) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4);
        let walk = Arc::new(Walk {
            home: home.to_path_buf(),
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            found: Mutex::new(Vec::new()),
            skip: stalled().clone().unwrap_or_default(),
            #[cfg(test)]
            wakes: Default::default(),
        });
        #[cfg(test)]
        tests::LAST_WALK.with(|last| *last.borrow_mut() = Arc::downgrade(&walk));
        *cancel.walk.lock().unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&walk);
        if cancel.is_cancelled() {
            return Self {
                entries: Vec::new(),
                built: Instant::now(),
            };
        }
        walk.push(Unit {
            dir: home.to_path_buf(),
            depth: 0,
        });
        let mut spawned = 0;
        for _ in 0..threads {
            let worker = walk.clone();
            match std::thread::Builder::new()
                .name("workspace-search-home-walk".into())
                .spawn(move || worker.work())
            {
                Ok(_) => spawned += 1,
                Err(error) => tracing::warn!(%error, "home walk worker did not start"),
            }
        }
        if spawned > 0 {
            let ends = Instant::now() + deadline;
            let mut queue = walk.queue();
            while !queue.quit && (queue.active > 0 || !queue.units.is_empty()) {
                let left = ends.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                queue = walk
                    .changed
                    .wait_timeout(queue, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            // Past the deadline (or cancelled): folders still being read
            // are skipped by later walks until their reads return.
            if !queue.reading.is_empty() {
                tracing::warn!(
                    folders = ?queue.reading,
                    "home walk: answering without folders whose listing hasn't returned"
                );
                stalled()
                    .get_or_insert_with(HashSet::new)
                    .extend(queue.reading.iter().cloned());
            }
            drop(queue);
            walk.quit();
        }
        let mut entries =
            std::mem::take(&mut *walk.found.lock().unwrap_or_else(PoisonError::into_inner));
        entries.truncate(MAX_ENTRIES);
        Self {
            entries,
            built: Instant::now(),
        }
    }

    /// Case-insensitive substring match: the exact name, then names starting
    /// with the query, then names containing it, then paths containing it;
    /// shallower paths first within a tier. An empty query lists the
    /// shallowest files, or no directories at all.
    pub fn search(&self, query: &str, limit: usize, kinds: NameKinds) -> Vec<NameMatch> {
        let query = normalize_name_query(query).trim().to_lowercase();
        let wanted = |is_dir: bool| match kinds {
            NameKinds::FilesAndDirectories => true,
            NameKinds::Files => !is_dir,
            NameKinds::Directories => is_dir,
        };
        if query.is_empty() && kinds == NameKinds::Directories {
            return Vec::new();
        }
        let mut found: Vec<(i64, &HomePath)> = self
            .entries
            .iter()
            .filter(|entry| wanted(entry.is_dir) && (!query.is_empty() || !entry.is_dir))
            .filter_map(|entry| {
                if query.is_empty() {
                    return Some((0, entry));
                }
                let name = entry.lower.rsplit('/').next().unwrap_or(&entry.lower);
                let score = if name == query {
                    3
                } else if name.starts_with(&query) {
                    2
                } else if name.contains(&query) {
                    1
                } else if entry.lower.contains(&query) {
                    0
                } else {
                    return None;
                };
                Some((score, entry))
            })
            .collect();
        found.sort_by(|(a_score, a), (b_score, b)| {
            b_score
                .cmp(a_score)
                .then_with(|| depth(&a.path).cmp(&depth(&b.path)))
                .then_with(|| a.path.len().cmp(&b.path.len()))
                .then_with(|| a.path.cmp(&b.path))
        });
        found
            .into_iter()
            .take(limit)
            .map(|(score, entry)| NameMatch {
                path: entry.path.clone(),
                is_dir: entry.is_dir,
                score,
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// Folders the walk never enters (`name` a folder's, `depth` below home).
/// Only folders are checked: listing a cloud-only file's name downloads
/// nothing, while listing a cloud-only folder fetches its contents.
fn skipped(name: &str, depth: usize, metadata: impl FnOnce() -> Option<std::fs::Metadata>) -> bool {
    let name = name.to_lowercase();
    if name == ".git" || SKIPPED_DIRS.contains(&name.as_str()) {
        return true;
    }
    if depth == 1
        && (MACHINE_ROOTS.contains(&name.as_str())
            || CLOUD_ROOT_PREFIXES.iter().any(|prefix| {
                name == *prefix
                    || name
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with([' ', '-', '(']))
            }))
    {
        return true;
    }
    metadata().is_some_and(|metadata| is_cloud_placeholder(&metadata))
}

/// A folder whose contents a cloud client fetches on first listing.
#[cfg(windows)]
fn is_cloud_placeholder(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    metadata.file_attributes()
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

/// A folder whose contents a cloud client fetches on first listing.
#[cfg(target_os = "macos")]
fn is_cloud_placeholder(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
fn is_cloud_placeholder(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(paths: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        for path in paths {
            let path = dir.path().join(path);
            if path.to_string_lossy().ends_with('/') {
                std::fs::create_dir_all(&path).unwrap();
            } else {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"").unwrap();
            }
        }
        let root = std::fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn paths(index: &HomeIndex, query: &str, kinds: NameKinds) -> Vec<String> {
        index
            .search(query, 50, kinds)
            .into_iter()
            .map(|found| found.path)
            .collect()
    }

    #[test]
    fn skips_cloud_machine_hidden_and_dependency_folders() {
        let (_dir, root) = home(&[
            "dev/notes/plan.md",
            "dev/app/node_modules/pkg/plan.md",
            "OneDrive/Documents/plan.md",
            "OneDrive - Contoso/plan.md",
            "Dropbox/plan.md",
            "Library/Mobile Documents/plan.md",
            "AppData/Local/plan.md",
            ".config/plan.md",
            "boxes/plan.md",
        ]);
        let index = HomeIndex::build(&root, &WalkCancel::default());
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["boxes/plan.md", "dev/notes/plan.md"]
        );
    }

    #[test]
    fn ranks_names_before_paths_and_filters_kinds() {
        let (_dir, root) = home(&[
            "comet/",
            "dev/comet/src/main.rs",
            "dev/comet-notes.md",
            "x/comets/a.rs",
        ]);
        let index = HomeIndex::build(&root, &WalkCancel::default());
        assert_eq!(
            paths(&index, "comet", NameKinds::Directories),
            ["comet", "dev/comet", "x/comets", "dev/comet/src"]
        );
        assert_eq!(
            paths(&index, "comet", NameKinds::Files)[0],
            "dev/comet-notes.md"
        );
        assert!(paths(&index, "", NameKinds::Directories).is_empty());
        // No query: the shallowest files.
        assert_eq!(
            paths(&index, "", NameKinds::Files),
            [
                "dev/comet-notes.md",
                "x/comets/a.rs",
                "dev/comet/src/main.rs"
            ]
        );
    }

    thread_local! {
        pub(super) static LAST_WALK: std::cell::RefCell<Weak<Walk>> =
            const { std::cell::RefCell::new(Weak::new()) };
    }

    /// Folders whose read waits until released, as `open()` waits on a
    /// privacy prompt nobody answers.
    type Gate = Arc<(Mutex<bool>, Condvar)>;
    type Entered = Arc<std::sync::atomic::AtomicUsize>;
    static HELD: Mutex<Vec<(PathBuf, Gate, Entered)>> = Mutex::new(Vec::new());

    pub(super) fn held(dir: &Path) {
        let gate = HELD
            .lock()
            .unwrap()
            .iter()
            .find(|(held, _, _)| held == dir)
            .map(|(_, gate, entered)| (gate.clone(), entered.clone()));
        if let Some((gate, entered)) = gate {
            entered.fetch_add(1, Ordering::SeqCst);
            let (open, changed) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = changed.wait(open).unwrap();
            }
        }
    }

    fn hold(
        dir: &Path,
    ) -> (
        Arc<(Mutex<bool>, Condvar)>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        HELD.lock()
            .unwrap()
            .push((dir.to_path_buf(), gate.clone(), entered.clone()));
        (gate, entered)
    }

    fn release(gate: &(Mutex<bool>, Condvar)) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    /// One folder never answers: the walk answers at its deadline with the
    /// rest, its idle workers sleep instead of polling, and later walks skip
    /// that folder (no second thread stuck in it) until its read returns.
    #[test]
    fn a_folder_that_never_answers_neither_holds_the_walk_nor_keeps_it_awake() {
        let (_dir, root) = home(&[
            "Documents/proj/plan.md",
            "dev/app/src/main.rs",
            "dev/notes/plan.md",
            "Music/song.txt",
        ]);
        let stuck = root.join("Documents");
        let (gate, entered) = hold(&stuck);

        let started = Instant::now();
        let index =
            HomeIndex::build_within(&root, &WalkCancel::default(), Duration::from_millis(300));
        let walk = LAST_WALK
            .with(|last| last.borrow().upgrade())
            .expect("the stuck worker holds the walk");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "answered at its deadline"
        );
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["dev/notes/plan.md"]
        );
        assert_eq!(paths(&index, "song", NameKinds::Files), ["Music/song.txt"]);
        assert!(stalled().as_ref().is_some_and(|s| s.contains(&stuck)));
        // While it waited out the stuck folder, idle workers slept (a
        // polling worker wakes ~300 times in 300 ms).
        assert!(
            walk.wakes.load(Ordering::Relaxed) < 50,
            "{} wakes",
            walk.wakes.load(Ordering::Relaxed)
        );

        // The walk is over: its free workers left; nothing wakes while the
        // stuck one waits.
        let wakes = walk.wakes.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            walk.wakes.load(Ordering::Relaxed) - wakes <= 1,
            "idle workers must sleep, not poll"
        );
        assert_eq!(
            Arc::strong_count(&walk),
            2,
            "only the stuck worker is left (and this test)"
        );
        drop(walk);

        // A second walk doesn't send another thread into it.
        let index = HomeIndex::build_within(&root, &WalkCancel::default(), Duration::from_secs(5));
        assert_eq!(
            entered.load(Ordering::SeqCst),
            1,
            "skipped while still stuck"
        );
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["dev/notes/plan.md"]
        );

        // Its read returns: the next walk reads it again.
        release(&gate);
        let deadline = Instant::now() + Duration::from_secs(5);
        while stalled().as_ref().is_some_and(|s| s.contains(&stuck)) {
            assert!(
                Instant::now() < deadline,
                "the returned read clears the stall"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let index = HomeIndex::build_within(&root, &WalkCancel::default(), Duration::from_secs(5));
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["dev/notes/plan.md", "Documents/proj/plan.md"]
        );
    }

    /// Closing the picker cancels a walk at once, stuck folder or not.
    #[test]
    fn a_cancel_ends_the_walk_at_once() {
        let (_dir, root) = home(&["Desktop/a.txt", "dev/b.txt"]);
        let stuck = root.join("Desktop");
        let (gate, _) = hold(&stuck);
        let cancel = Arc::new(WalkCancel::default());
        let canceller = cancel.clone();
        let started = Instant::now();
        let timer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            canceller.cancel();
        });
        let _ = HomeIndex::build_within(&root, &cancel, Duration::from_secs(30));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancel ended the walk"
        );
        timer.join().unwrap();
        release(&gate);
    }

    /// Repositories keep their ignore files, at any depth.
    #[test]
    fn a_repository_keeps_its_ignore_files() {
        let (_dir, root) = home(&[
            "code/work/app/.git/",
            "code/work/app/.gitignore",
            "code/work/app/src/lib.rs",
            "code/work/app/target/debug/lib.rs",
            "solo/.git/",
            "solo/.gitignore",
            "solo/dist/lib.rs",
            "solo/lib.rs",
        ]);
        std::fs::write(root.join("code/work/app/.gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("solo/.gitignore"), "dist/\n").unwrap();
        let index = HomeIndex::build(&root, &WalkCancel::default());
        assert_eq!(
            paths(&index, "lib.rs", NameKinds::Files),
            ["solo/lib.rs", "code/work/app/src/lib.rs"]
        );
    }
}

#[cfg(test)]
mod bench {
    /// `HOME_WALK_BENCH=<dir> cargo test -p zeron-engine --lib home_walk_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn home_walk_bench() {
        let root = std::fs::canonicalize(std::env::var("HOME_WALK_BENCH").unwrap()).unwrap();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let index = super::HomeIndex::build(&root, &Default::default());
            println!(
                "WALK entries={} ms={}",
                index.len(),
                started.elapsed().as_millis()
            );
        }
    }
}
