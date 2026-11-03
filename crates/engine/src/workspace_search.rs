//! Workspace search index: one `fff-search` file picker per working folder
//! (repo, worktree or plain folder), shared by `SearchFiles` (`@`),
//! `SearchWorkspaceFiles` (file tree) and `SearchWorkspaceContent` (cmd+K).
//!
//! An index is created by the first warm or search of its root, scans in the
//! background and keeps itself current with its own watcher. Memory stays
//! bounded: an index idle for [`IDLE_EVICT`] is dropped, at most [`MAX_LIVE`]
//! are alive at once (least recently used goes first), and the content cache
//! of each one is capped at [`CACHE_BUDGET_BYTES`]. Indexes hold paths only:
//! content search greps the indexed files without a bigram prefilter. The UI
//! warms the focused chat's index once, so its first search finds it built.
//!
//! The host's home folder is the exception: fff would read every file under
//! it, cloud-only ones included, so home gets a names-only walk instead
//! ([`crate::home_index`]) and no content search.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use fff_search::file_picker::FilePicker;
use fff_search::{
    Casing, ContentCacheBudget, DirSearchConfig, FFFMode, FilePickerOptions, FuzzySearchOptions,
    GrepMode, GrepSearchOptions, MixedItemRef, MixedSearchConfig, PaginationArgs, QueryParser,
    SharedFilePicker, SharedFrecency,
};

use crate::home_index::HomeIndex;

/// An index nobody searched or warmed for this long is dropped.
pub const IDLE_EVICT: Duration = Duration::from_secs(15 * 60);
/// Live indexes at once; creating one more evicts the least recently used.
/// Each runs its own file watcher, so the cap also bounds inotify use.
pub const MAX_LIVE: usize = 7;
/// Per-index cap on cached file contents (fff defaults to up to 512 MB).
pub const CACHE_BUDGET_BYTES: u64 = 16 * 1024 * 1024;
/// A search waits this long for the initial scan before answering from
/// whatever is indexed so far (flagged `indexing`).
pub const SCAN_WAIT: Duration = Duration::from_millis(300);
/// The first-scan wait for name searches whose answer can't report that the
/// index is still building (`SearchFiles`, `SearchWorkspaceFiles`): long
/// enough for most checkouts, well inside the 6 s RPC timeout.
pub const NAME_SCAN_WAIT: Duration = Duration::from_millis(2000);
/// Interactive content searches return partial results past this budget.
pub const CONTENT_TIME_BUDGET: Duration = Duration::from_secs(2);
const REAPER_INTERVAL: Duration = Duration::from_secs(60);
/// A home index older than this is rebuilt in the background on next use
/// (it has no watcher); searches keep answering from the old one meanwhile.
const HOME_REFRESH: Duration = Duration::from_secs(60);
/// Ceiling for logging a finished scan; a scan still running then is not logged.
const SCAN_LOG_WAIT: Duration = Duration::from_secs(10 * 60);

/// Name queries follow the engine host's path syntax. fff indexes `/` paths,
/// while a backslash is a literal filename character on POSIX hosts.
pub(crate) fn normalize_name_query(query: &str) -> Cow<'_, str> {
    #[cfg(windows)]
    if query.contains('\\') {
        return Cow::Owned(query.replace('\\', "/"));
    }
    Cow::Borrowed(query)
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSearchError {
    #[error("workspace search is unavailable: {0}")]
    Unavailable(String),
}

/// Whether a root's index has finished its initial scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    Building,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKinds {
    FilesAndDirectories,
    Files,
    Directories,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMatch {
    /// Root-relative, `/`-separated.
    pub path: String,
    pub is_dir: bool,
    pub score: i64,
}

#[derive(Debug, Clone, Default)]
pub struct NameSearch {
    pub matches: Vec<NameMatch>,
    /// The initial scan had not finished; results may be incomplete.
    pub indexing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatch {
    /// Root-relative, `/`-separated.
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// 0-based byte column of the first match in the original line.
    pub column: u64,
    /// The matched line, leading whitespace trimmed and capped at 512 bytes.
    pub preview: String,
    /// Byte ranges of the matches within `preview`.
    pub ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct ContentSearch {
    pub matches: Vec<ContentMatch>,
    /// More matches exist past `limit`, or the time budget ran out.
    pub truncated: bool,
    pub indexing: bool,
}

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Clone)]
pub struct WorkspaceSearch {
    inner: Arc<Inner>,
}

struct Inner {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    /// The host's home, canonical; never indexed by fff.
    home: Option<PathBuf>,
    home_index: Mutex<HomeEntry>,
    home_built: Condvar,
    clock: Clock,
    /// Whether indexes run a file watcher; off only for eviction tests, which
    /// need many indexes but no updates (each watcher is an inotify instance).
    watch: bool,
    reaper_started: std::sync::Once,
}

struct Entry {
    picker: SharedFilePicker,
    last_used: Instant,
    /// A chat warmed it: it serves a conversation, not only a transient
    /// caller such as the new-project folder picker.
    warmed: bool,
}

/// The home folder's names-only index, with [`Entry`]'s lifecycle.
#[derive(Default)]
struct HomeEntry {
    index: Option<Arc<HomeIndex>>,
    /// A walk is running; `cancel` stops it when the index is dropped.
    building: Option<Arc<AtomicBool>>,
    last_used: Option<Instant>,
    warmed: bool,
}

impl Default for WorkspaceSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSearch {
    pub fn new() -> Self {
        let home = crate::repos::session_home_dir()
            .ok()
            .and_then(|home| std::fs::canonicalize(home).ok());
        Self::with_clock(Arc::new(Instant::now), true, home)
    }

    fn with_clock(clock: Clock, watch: bool, home: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                home,
                home_index: Mutex::new(HomeEntry::default()),
                home_built: Condvar::new(),
                clock,
                watch,
                reaper_started: std::sync::Once::new(),
            }),
        }
    }

    /// Start (or keep alive) the index for a chat's `root`.
    pub fn warm(&self, root: &Path) -> Result<IndexState, WorkspaceSearchError> {
        if self.is_home(root) {
            return Ok(match self.home_index(root, true, Duration::ZERO) {
                Some(_) => IndexState::Ready,
                None => IndexState::Building,
            });
        }
        let picker = self.acquire(root, true)?;
        Ok(if picker.wait_for_scan(Duration::ZERO) {
            IndexState::Ready
        } else {
            IndexState::Building
        })
    }

    /// Fuzzy file (and directory) names. An empty query lists files by
    /// recency (git and modification time), or no directories at all.
    pub fn search_names(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        kinds: NameKinds,
    ) -> Result<NameSearch, WorkspaceSearchError> {
        self.search_names_within(root, query, limit, kinds, SCAN_WAIT)
    }

    /// [`Self::search_names`], waiting up to `wait` for a first scan. Callers
    /// whose wire answer cannot say "still indexing" (`@`, the file tree)
    /// wait longer so a fresh index isn't served half-built.
    pub fn search_names_within(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        kinds: NameKinds,
        wait: Duration,
    ) -> Result<NameSearch, WorkspaceSearchError> {
        if self.is_home(root) {
            let index = self.home_index(root, false, wait);
            return Ok(NameSearch {
                matches: index
                    .as_ref()
                    .map(|index| index.search(query, limit, kinds))
                    .unwrap_or_default(),
                indexing: index.is_none(),
            });
        }
        let query = normalize_name_query(query);
        let query = query.as_ref();
        let picker = self.acquire(root, false)?;
        let indexing = !picker.wait_for_scan(wait);
        if limit == 0 {
            return Ok(NameSearch {
                matches: Vec::new(),
                indexing,
            });
        }
        let guard = picker.read().map_err(unavailable)?;
        let Some(fff) = guard.as_ref() else {
            // Released under us (capacity eviction): not a final empty answer.
            return Ok(NameSearch {
                matches: Vec::new(),
                indexing: true,
            });
        };
        let options = FuzzySearchOptions {
            pagination: PaginationArgs { offset: 0, limit },
            ..Default::default()
        };
        let mut matches = Vec::with_capacity(limit);
        if kinds == NameKinds::Directories {
            if query.trim().is_empty() {
                return Ok(NameSearch { matches, indexing });
            }
            let parsed = QueryParser::new(DirSearchConfig).parse(query);
            let result = fff.fuzzy_search_directories(&parsed, options);
            for (dir, score) in result.items.into_iter().zip(result.scores) {
                let path = dir.relative_path(fff).trim_end_matches('/').to_owned();
                if path.is_empty() {
                    continue;
                }
                matches.push(NameMatch {
                    path,
                    is_dir: true,
                    score: i64::from(score.total),
                });
            }
        } else if kinds == NameKinds::Files || query.trim().is_empty() {
            let parsed = QueryParser::default().parse(query);
            let result = fff.fuzzy_search(&parsed, None, options);
            for (item, score) in result.items.into_iter().zip(result.scores) {
                matches.push(NameMatch {
                    path: item.relative_path(fff),
                    is_dir: false,
                    score: i64::from(score.total),
                });
            }
        } else {
            let parsed = QueryParser::new(MixedSearchConfig).parse(query);
            let result = fff.fuzzy_search_mixed(&parsed, None, options);
            for (item, score) in result.items.into_iter().zip(result.scores) {
                let (path, is_dir) = match item {
                    MixedItemRef::File(file) => (file.relative_path(fff), false),
                    MixedItemRef::Dir(dir) => {
                        let path = dir.relative_path(fff);
                        (path.trim_end_matches('/').to_owned(), true)
                    }
                };
                if path.is_empty() {
                    continue;
                }
                matches.push(NameMatch {
                    path,
                    is_dir,
                    score: i64::from(score.total),
                });
            }
        }
        matches.truncate(limit);
        Ok(NameSearch { matches, indexing })
    }

    /// Plain-text content search with smart case. Binary and oversized files
    /// are skipped; at most `per_file` matches come from any one file.
    pub fn search_content(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        per_file: usize,
    ) -> Result<ContentSearch, WorkspaceSearchError> {
        if self.is_home(root) {
            // Home's index holds names only (see the module docs).
            return Ok(ContentSearch::default());
        }
        let picker = self.acquire(root, false)?;
        let indexing = !picker.wait_for_scan(SCAN_WAIT);
        if limit == 0 || per_file == 0 || query.trim().is_empty() {
            return Ok(ContentSearch {
                indexing,
                ..Default::default()
            });
        }
        let guard = picker.read().map_err(unavailable)?;
        let Some(fff) = guard.as_ref() else {
            // Released under us (capacity eviction): not a final empty answer.
            return Ok(ContentSearch {
                indexing: true,
                ..Default::default()
            });
        };
        let options = GrepSearchOptions {
            max_matches_per_file: per_file,
            casing: Some(Casing::Smart),
            page_limit: limit,
            mode: GrepMode::PlainText,
            time_budget_ms: CONTENT_TIME_BUDGET.as_millis() as u64,
            enforce_time_budget: true,
            ..Default::default()
        };
        // The raw-pattern API preserves whitespace, constraint-like tokens
        // and backslashes. `grep` parses filters and interprets escapes even
        // in PlainText mode, so use one literal pattern with no constraints.
        let result = fff.multi_grep(&[query], &[], &options);
        let mut truncated = result.next_file_offset != 0 || result.matches.len() > limit;
        let mut matches = Vec::with_capacity(result.matches.len().min(limit));
        for found in result.matches {
            if matches.len() == limit {
                truncated = true;
                break;
            }
            let Some(file) = result.files.get(found.file_index) else {
                continue;
            };
            let indent = found.line_content.len() - found.line_content.trim_start().len();
            let shift = indent as u32;
            matches.push(ContentMatch {
                path: file.relative_path(fff),
                line: found.line_number,
                column: found.col as u64,
                preview: found.line_content[indent..].to_owned(),
                ranges: found
                    .match_byte_offsets
                    .iter()
                    .map(|(start, end)| (start.saturating_sub(shift), end.saturating_sub(shift)))
                    .collect(),
            });
        }
        Ok(ContentSearch {
            matches,
            truncated,
            indexing,
        })
    }

    /// Block until `root`'s index finished its scan and content indexing
    /// (benchmarks); `false` on timeout or when `root` has no index.
    pub fn wait_until_indexed(&self, root: &Path, timeout: Duration) -> bool {
        let picker = lock(&self.inner.entries)
            .get(root)
            .map(|entry| entry.picker.clone());
        picker.is_some_and(|picker| picker.wait_for_indexing_complete(timeout))
    }

    /// Drop every index now (benchmarks measuring memory after eviction).
    #[doc(hidden)]
    pub fn release_all(&self) {
        let entries: Vec<_> = lock(&self.inner.entries).drain().collect();
        for (root, entry) in entries {
            release(&root, entry, "explicit");
        }
        self.release_home(|_| true);
    }

    /// Drop `root`'s index now unless a chat warmed it, for a caller done
    /// with an index it may have started. `true` if dropped.
    pub fn release_unless_warmed(&self, root: &Path) -> bool {
        if self.is_home(root) {
            return self.release_home(|home| !home.warmed);
        }
        let removed = {
            let mut entries = lock(&self.inner.entries);
            let warmed = entries.get(root).is_some_and(|entry| entry.warmed);
            if warmed { None } else { entries.remove(root) }
        };
        let Some(entry) = removed else {
            return false;
        };
        release(root, entry, "released");
        true
    }

    /// Number of live indexes.
    pub fn live(&self) -> usize {
        let home = lock(&self.inner.home_index).index.is_some();
        lock(&self.inner.entries).len() + usize::from(home)
    }

    /// Drop every index idle past [`IDLE_EVICT`] (the reaper runs this every
    /// [`REAPER_INTERVAL`]).
    pub fn evict_idle(&self) {
        let now = (self.inner.clock)();
        let expired: Vec<(PathBuf, Entry)> = {
            let mut entries = lock(&self.inner.entries);
            let roots: Vec<PathBuf> = entries
                .iter()
                .filter(|(_, entry)| now.saturating_duration_since(entry.last_used) >= IDLE_EVICT)
                .map(|(root, _)| root.clone())
                .collect();
            roots
                .into_iter()
                .filter_map(|root| entries.remove(&root).map(|entry| (root, entry)))
                .collect()
        };
        for (root, entry) in expired {
            release(&root, entry, "idle");
        }
        self.release_home(|home| {
            home.last_used
                .is_some_and(|used| now.saturating_duration_since(used) >= IDLE_EVICT)
        });
    }

    fn is_home(&self, root: &Path) -> bool {
        self.inner.home.as_deref() == Some(root)
    }

    /// Home's index, walking it in the background when missing or stale;
    /// waits up to `wait` for a first walk. `warm`: a chat is asking.
    fn home_index(&self, root: &Path, warm: bool, wait: Duration) -> Option<Arc<HomeIndex>> {
        self.start_reaper();
        let now = (self.inner.clock)();
        let mut home = lock(&self.inner.home_index);
        home.last_used = Some(now);
        home.warmed |= warm;
        let stale = home
            .index
            .as_ref()
            .is_none_or(|index| index.built.elapsed() >= HOME_REFRESH);
        if stale && home.building.is_none() {
            let cancel = Arc::new(AtomicBool::new(false));
            home.building = Some(cancel.clone());
            let inner = Arc::downgrade(&self.inner);
            let root = root.to_path_buf();
            let spawned = std::thread::Builder::new()
                .name("workspace-search-home".into())
                .spawn(move || build_home(inner, root, cancel));
            if let Err(error) = spawned {
                home.building = None;
                tracing::warn!(%error, "home search index did not start");
            }
        }
        if home.index.is_none() && home.building.is_some() && !wait.is_zero() {
            home = self
                .inner
                .home_built
                .wait_timeout_while(home, wait, |home| {
                    home.index.is_none() && home.building.is_some()
                })
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        home.index.clone()
    }

    /// Drop home's index (and stop its walk) when `drop` says so.
    fn release_home(&self, drop: impl FnOnce(&HomeEntry) -> bool) -> bool {
        let mut home = lock(&self.inner.home_index);
        if (home.index.is_none() && home.building.is_none()) || !drop(&home) {
            return false;
        }
        if let Some(cancel) = home.building.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        *home = HomeEntry::default();
        self.inner.home_built.notify_all();
        tracing::info!("workspace search home index evicted");
        true
    }

    /// `warm`: a chat is asking (see [`Entry::warmed`]).
    fn acquire(&self, root: &Path, warm: bool) -> Result<SharedFilePicker, WorkspaceSearchError> {
        self.start_reaper();
        let now = (self.inner.clock)();
        let mut evicted = None;
        let picker = {
            let mut entries = lock(&self.inner.entries);
            if let Some(entry) = entries.get_mut(root) {
                entry.last_used = now;
                entry.warmed |= warm;
                entry.picker.clone()
            } else {
                // Create first: a root fff refuses (a filesystem root, a
                // vanished folder) must not cost a healthy index its place.
                let picker = create_picker(root, self.inner.watch)?;
                if entries.len() >= MAX_LIVE {
                    evicted = lru_victim(&entries)
                        .and_then(|victim| entries.remove(&victim).map(|entry| (victim, entry)));
                }
                entries.insert(
                    root.to_path_buf(),
                    Entry {
                        picker: picker.clone(),
                        last_used: now,
                        warmed: warm,
                    },
                );
                picker
            }
        };
        if let Some((victim, entry)) = evicted {
            release(&victim, entry, "capacity");
        }
        Ok(picker)
    }

    fn start_reaper(&self) {
        let weak = Arc::downgrade(&self.inner);
        self.inner.reaper_started.call_once(move || {
            let spawned = std::thread::Builder::new()
                .name("workspace-search-reaper".into())
                .spawn(move || reap(weak));
            if let Err(error) = spawned {
                tracing::warn!(%error, "workspace search reaper did not start");
            }
        });
    }
}

fn build_home(inner: Weak<Inner>, root: PathBuf, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    let index = HomeIndex::build(&root, &cancel);
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let mut home = lock(&inner.home_index);
    // Released (or replaced) while walking: drop this result.
    if !home
        .building
        .as_ref()
        .is_some_and(|building| Arc::ptr_eq(building, &cancel))
    {
        return;
    }
    tracing::info!(
        root = %root.display(),
        entries = index.len(),
        scan_ms = started.elapsed().as_millis() as u64,
        "workspace search home index scanned"
    );
    home.index = Some(Arc::new(index));
    home.building = None;
    inner.home_built.notify_all();
}

fn reap(inner: Weak<Inner>) {
    loop {
        std::thread::sleep(REAPER_INTERVAL);
        let Some(inner) = inner.upgrade() else {
            return;
        };
        WorkspaceSearch { inner }.evict_idle();
    }
}

fn lru_victim(entries: &HashMap<PathBuf, Entry>) -> Option<PathBuf> {
    entries
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(root, _)| root.clone())
}

fn create_picker(root: &Path, watch: bool) -> Result<SharedFilePicker, WorkspaceSearchError> {
    let picker = SharedFilePicker::default();
    let started = Instant::now();
    FilePicker::new_with_shared_state(
        picker.clone(),
        SharedFrecency::noop(),
        FilePickerOptions {
            base_path: root.to_string_lossy().into_owned(),
            enable_mmap_cache: false,
            // No bigram index: it costs a third of a large repo's index memory
            // and scan CPU on every focused chat, while grep without it still
            // answers in ~20 ms on a 28k-file repo (see
            // docs/performance-workspace-search.md).
            enable_content_indexing: false,
            mode: FFFMode::Neovim,
            cache_budget: ContentCacheBudget::from_overrides(0, CACHE_BUDGET_BYTES, 0),
            watch,
            follow_symlinks: false,
            enable_fs_root_scanning: false,
            // Home gets `HomeIndex` instead (see the module docs).
            enable_home_dir_scanning: false,
            ..Default::default()
        },
    )
    .map_err(unavailable)?;
    tracing::info!(root = %root.display(), "workspace search index created");
    let logged = picker.clone();
    let root_for_log = root.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("workspace-search-scan-log".into())
        .spawn(move || {
            if !logged.wait_for_scan(SCAN_LOG_WAIT) {
                return;
            }
            let Ok(guard) = logged.read() else {
                return;
            };
            if let Some(fff) = guard.as_ref() {
                tracing::info!(
                    root = %root_for_log.display(),
                    files = fff.live_file_count(),
                    scan_ms = started.elapsed().as_millis() as u64,
                    arena_bytes = fff.arena_bytes().0,
                    "workspace search index scanned"
                );
            }
        });
    Ok(picker)
}

/// Stop the index's scan and watcher and free it. The write lock waits for
/// in-flight searches, so this runs off the caller's thread.
fn release(root: &Path, entry: Entry, reason: &'static str) {
    let root = root.to_path_buf();
    let picker = entry.picker;
    picker.cancel();
    let _ = std::thread::Builder::new()
        .name("workspace-search-release".into())
        .spawn(move || {
            let Ok(mut guard) = picker.write() else {
                return;
            };
            if let Some(mut fff) = guard.take() {
                let files = fff.live_file_count();
                let arena_bytes = fff.arena_bytes().0;
                fff.stop_background_monitor();
                drop(fff);
                drop(guard);
                // An index frees tens of MB in small blocks; glibc keeps them
                // mapped unless asked to give them back.
                #[cfg(all(target_os = "linux", target_env = "gnu"))]
                // SAFETY: malloc_trim only walks the allocator's own arenas.
                unsafe {
                    libc::malloc_trim(0);
                }
                tracing::info!(
                    root = %root.display(),
                    reason,
                    files,
                    arena_bytes,
                    "workspace search index evicted"
                );
            }
        });
}

fn unavailable(error: impl std::fmt::Display) -> WorkspaceSearchError {
    WorkspaceSearchError::Unavailable(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {

    #[cfg(unix)]
    #[test]
    fn results_reached_through_a_symlink_are_outside_the_root() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "token").unwrap();
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("leak")).unwrap();
        std::os::unix::fs::symlink(root.join("src/main.rs"), root.join("alias.rs")).unwrap();

        assert!(!escapes_through_symlink(&root, "src/main.rs", false));
        // Through a symlinked folder: never, names or contents.
        assert!(escapes_through_symlink(&root, "leak/secret", true));
        assert!(escapes_through_symlink(&root, "leak/secret", false));
        // A symlink entry itself may be listed but not read.
        assert!(!escapes_through_symlink(&root, "alias.rs", true));
        assert!(escapes_through_symlink(&root, "alias.rs", false));
        // Gone, or not a plain relative path: not served.
        assert!(escapes_through_symlink(&root, "src/missing.rs", false));
        assert!(escapes_through_symlink(&root, "../outside", false));
    }

    use super::*;

    struct ManualClock(Mutex<Instant>);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            *lock(&self.0) += by;
        }
    }

    fn manual() -> (WorkspaceSearch, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock(Mutex::new(Instant::now())));
        let reader = clock.clone();
        (
            WorkspaceSearch::with_clock(Arc::new(move || *lock(&reader.0)), false, None),
            clock,
        )
    }

    fn folder(files: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        for (path, contents) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let root = std::fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn ready(search: &WorkspaceSearch, root: &Path) -> SharedFilePicker {
        search.warm(root).unwrap();
        let picker = lock(&search.inner.entries)[root].picker.clone();
        assert!(picker.wait_for_indexing_complete(Duration::from_secs(20)));
        picker
    }

    fn names(search: &WorkspaceSearch, root: &Path, query: &str) -> Vec<String> {
        search
            .search_names(root, query, 20, NameKinds::FilesAndDirectories)
            .unwrap()
            .matches
            .into_iter()
            .map(|found| found.path)
            .collect()
    }

    #[test]
    fn creates_an_index_and_finds_files_and_directories() {
        let (_dir, root) = folder(&[("src/composer.rs", b""), ("README.md", b"")]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);

        let found = search
            .search_names(&root, "composer", 20, NameKinds::FilesAndDirectories)
            .unwrap();
        assert!(!found.indexing);
        assert_eq!(found.matches[0].path, "src/composer.rs");
        assert!(!found.matches[0].is_dir);
        let dirs = search
            .search_names(&root, "src", 20, NameKinds::FilesAndDirectories)
            .unwrap();
        assert!(dirs.matches.iter().any(|m| m.path == "src" && m.is_dir));
        let files = search
            .search_names(&root, "src", 20, NameKinds::Files)
            .unwrap();
        assert!(files.matches.iter().all(|m| !m.is_dir));
        assert_eq!(search.live(), 1);
    }

    #[test]
    fn directory_search_returns_only_directories() {
        let (_dir, root) = folder(&[
            ("projects/comet/src/main.rs", b""),
            ("projects/comet.md", b""),
        ]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);
        let found = search
            .search_names(&root, "comet", 20, NameKinds::Directories)
            .unwrap();
        assert_eq!(found.matches[0].path, "projects/comet");
        assert!(found.matches.iter().all(|m| m.is_dir));
        assert!(
            search
                .search_names(&root, "", 20, NameKinds::Directories)
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    #[cfg(windows)]
    fn name_search_accepts_both_windows_separators() {
        let (_dir, root) = folder(&[("aa/bb/cc/dd/file.rs", b"")]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);

        for (kinds, query, expected) in [
            (
                NameKinds::Files,
                "aa/bb/cc/dd/file.rs",
                "aa/bb/cc/dd/file.rs",
            ),
            (NameKinds::Directories, "aa/bb/cc/dd", "aa/bb/cc/dd"),
            (NameKinds::FilesAndDirectories, "aa/bb/cc/dd", "aa/bb/cc/dd"),
        ] {
            let forward = search.search_names(&root, query, 20, kinds).unwrap();
            assert!(forward.matches.iter().any(|m| m.path == expected));
            for alternative in [query.replace('/', "\\"), query.replacen('/', "\\", 2)] {
                let found = search.search_names(&root, &alternative, 20, kinds).unwrap();
                assert_eq!(found.matches, forward.matches);
            }
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn name_search_preserves_literal_posix_backslashes() {
        let literal = r"aa\bb\cc\dd";
        let (_dir, root) = folder(&[(r"aa\bb\cc\dd/file.rs", b""), ("aa/bb/cc/dd/file.rs", b"")]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);

        for kinds in [NameKinds::Directories, NameKinds::FilesAndDirectories] {
            let found = search.search_names(&root, literal, 20, kinds).unwrap();
            assert_eq!(found.matches[0].path, literal);
        }
        let found = search
            .search_names(&root, &format!("{literal}/file.rs"), 20, NameKinds::Files)
            .unwrap();
        assert_eq!(found.matches[0].path, format!("{literal}/file.rs"));
        let forward = search
            .search_names(&root, "aa/bb/cc/dd", 20, NameKinds::Directories)
            .unwrap();
        assert_eq!(forward.matches[0].path, "aa/bb/cc/dd");
    }

    #[test]
    fn releasing_spares_an_index_a_chat_warmed() {
        let (_a, a) = folder(&[("a.rs", b"")]);
        let (_b, b) = folder(&[("b.rs", b"")]);
        let search = WorkspaceSearch::new();
        search.warm(&a).unwrap();
        search
            .search_names(&b, "b", 5, NameKinds::Directories)
            .unwrap();

        assert!(!search.release_unless_warmed(&a));
        assert!(search.release_unless_warmed(&b));
        assert!(!search.release_unless_warmed(&b));
        let entries = lock(&search.inner.entries);
        assert!(entries.contains_key(&a));
        assert!(!entries.contains_key(&b));
    }

    #[test]
    fn the_watcher_picks_up_new_files() {
        let (_dir, root) = folder(&[("alpha.rs", b"")]);
        let search = WorkspaceSearch::new();
        let picker = ready(&search, &root);
        assert!(picker.wait_for_watcher(Duration::from_secs(20)));
        assert!(!names(&search, &root, "beta").contains(&"beta.rs".to_owned()));

        std::fs::write(root.join("beta.rs"), b"").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !names(&search, &root, "beta").contains(&"beta.rs".to_owned()) {
            assert!(Instant::now() < deadline, "watcher never indexed beta.rs");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn idle_indexes_are_evicted() {
        let (_a, a) = folder(&[("a.rs", b"")]);
        let (_b, b) = folder(&[("b.rs", b"")]);
        let (search, clock) = manual();
        search.warm(&a).unwrap();
        clock.advance(IDLE_EVICT / 2);
        search.warm(&b).unwrap();
        clock.advance(IDLE_EVICT / 2);

        search.evict_idle();

        let entries = lock(&search.inner.entries);
        assert!(!entries.contains_key(&a));
        assert!(entries.contains_key(&b));
    }

    #[test]
    fn creating_past_the_cap_evicts_the_least_recently_used() {
        let folders: Vec<_> = (0..=MAX_LIVE).map(|_| folder(&[("x.rs", b"")])).collect();
        let (search, clock) = manual();
        for (_, root) in &folders[..MAX_LIVE] {
            search.warm(root).unwrap();
            clock.advance(Duration::from_secs(1));
        }
        // Touching the oldest makes the second the least recently used.
        search.warm(&folders[0].1).unwrap();
        clock.advance(Duration::from_secs(1));

        search.warm(&folders[MAX_LIVE].1).unwrap();

        let entries = lock(&search.inner.entries);
        assert_eq!(entries.len(), MAX_LIVE);
        assert!(!entries.contains_key(&folders[1].1));
    }

    #[test]
    fn content_search_skips_binaries_and_caps_matches_per_file() {
        let (_dir, root) = folder(&[
            (
                "notes.md",
                b"needle one\nneedle two\nneedle three\nneedle four\n",
            ),
            ("other.txt", b"  indented needle\n"),
            ("blob.bin", b"needle\0\0\0binary"),
        ]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);

        let found = search.search_content(&root, "needle", 50, 3).unwrap();
        assert!(!found.matches.iter().any(|m| m.path == "blob.bin"));
        assert_eq!(
            found
                .matches
                .iter()
                .filter(|m| m.path == "notes.md")
                .count(),
            3
        );
        let other = found
            .matches
            .iter()
            .find(|m| m.path == "other.txt")
            .unwrap();
        assert_eq!(other.line, 1);
        assert_eq!(other.preview, "indented needle");
        assert_eq!(other.ranges, vec![(9, 15)]);
        assert_eq!(other.column, 11);
    }

    #[test]
    fn home_is_walked_for_names_and_never_indexed_by_fff() {
        let (_dir, home) = folder(&[("dev/plan.md", b"needle"), ("OneDrive/plan.md", b"needle")]);
        let search = WorkspaceSearch::with_clock(Arc::new(Instant::now), false, Some(home.clone()));
        let found = search
            .search_names_within(&home, "plan", 20, NameKinds::Files, NAME_SCAN_WAIT)
            .unwrap();
        assert!(!found.indexing);
        let paths: Vec<_> = found.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["dev/plan.md"]);
        assert!(
            lock(&search.inner.entries).is_empty(),
            "no fff picker for home"
        );
        assert_eq!(search.live(), 1);
        assert!(
            search
                .search_content(&home, "needle", 50, 3)
                .unwrap()
                .matches
                .is_empty()
        );
        assert!(search.release_unless_warmed(&home));
        assert_eq!(search.live(), 0);
    }

    #[test]
    fn content_search_rejects_blank_queries_without_searching() {
        let (_dir, root) = folder(&[("notes.md", b"   \n")]);
        let search = WorkspaceSearch::new();
        ready(&search, &root);
        assert!(
            search
                .search_content(&root, "  ", 50, 3)
                .unwrap()
                .matches
                .is_empty()
        );
    }
}

/// Whether `relative` (an index result under `root`) reaches outside the
/// root through a symlink. fff's watcher follows links created after the
/// first scan, so results are re-checked with ReadWorkspaceFile's rule: no
/// symlinked folder on the way, and for contents, no symlinked file either.
/// A path that no longer exists is treated as escaping (it can't be served).
pub fn escapes_through_symlink(root: &Path, relative: &str, allow_final_link: bool) -> bool {
    let components: Vec<_> = Path::new(relative).components().collect();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(component) = component else {
            return true;
        };
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return true;
        };
        let last = index + 1 == components.len();
        if metadata.file_type().is_symlink() && !(last && allow_final_link) {
            return true;
        }
    }
    false
}
