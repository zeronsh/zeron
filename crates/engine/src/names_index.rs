//! Names-only index of a folder that fff must not index: the host's home
//! folder (chats that still run in `~`, the new-project folder search), and
//! a plain folder too large to index live, such as a folder of projects.
//!
//! Home is never handed to fff: after its scan, fff opens every file up to
//! 10 MB to tell text from binary, which downloads every cloud-only file
//! under home (OneDrive on Windows, iCloud Drive and File Provider folders
//! on macOS). A large plain folder costs the same reads, again on every
//! rescan, and its watcher can't apply the nested repositories' ignore rules
//! (see `workspace_search`). This walk reads directory listings only, and
//! skips hidden folders, dependency folders and gitignored paths (home also
//! skips its cloud and machine folders). It holds no file contents: content
//! search reads the listed files when someone searches, and only then
//! decides which are binary.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime};

use crate::workspace_search::{
    ContentMatch, ContentSearch, NameKinds, NameMatch, normalize_name_query,
};

/// Paths kept at most; a larger folder answers from the first ones walked.
pub const MAX_ENTRIES: usize = 250_000;
/// Content search skips larger files, like fff.
const MAX_GREP_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// A file with a NUL byte in this many leading bytes is binary (git reads
/// 8,000 bytes, Zed 1,024).
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Content search previews are cut to this many bytes.
const MAX_PREVIEW_BYTES: usize = 512;
/// Name lists longer than this are fuzzy matched on several threads.
const PARALLEL_MATCH_FROM: usize = 20_000;

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

pub struct NamesIndex {
    /// Sorted by path.
    entries: Vec<NamePath>,
    pub built: Instant,
}

struct NamePath {
    /// Root-relative, `/`-separated.
    path: String,
    is_dir: bool,
    /// Seconds since the Unix epoch; 0 when unknown.
    modified: u64,
    /// Bytes; 0 for folders.
    size: u64,
    /// A cloud-only file, which opening downloads.
    cloud: bool,
}

impl NamesIndex {
    /// Walk `root` (canonical); `home` adds home's cloud and machine folder
    /// rules. `cancelled` stops the walk early.
    pub fn build(root: &Path, home: bool, cancelled: &AtomicBool) -> Self {
        let collected: Mutex<Vec<NamePath>> = Mutex::new(Vec::new());
        walker(root, home).run(|| {
            let collected = &collected;
            let mut batch = Batch {
                items: Vec::new(),
                sink: collected,
            };
            Box::new(move |entry| {
                if cancelled.load(Ordering::Relaxed) {
                    return ignore::WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return ignore::WalkState::Continue;
                };
                if entry.depth() == 0 {
                    return ignore::WalkState::Continue;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    return ignore::WalkState::Continue;
                };
                let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                // Free on Windows (the listing carries it), one stat elsewhere.
                let metadata = if is_dir { None } else { entry.metadata().ok() };
                let modified = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_secs());
                batch.items.push(NamePath {
                    path: relative.to_string_lossy().replace('\\', "/"),
                    is_dir,
                    modified,
                    size: metadata.as_ref().map_or(0, std::fs::Metadata::len),
                    cloud: metadata.as_ref().is_some_and(is_cloud_placeholder),
                });
                if batch.items.len() >= 512 && batch.flush() >= MAX_ENTRIES {
                    return ignore::WalkState::Quit;
                }
                ignore::WalkState::Continue
            })
        });
        let mut entries = collected
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.truncate(MAX_ENTRIES);
        entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        Self {
            entries,
            built: Instant::now(),
        }
    }

    /// Fuzzy match with fff's matcher (frizbee, smart case): the query's
    /// characters in order in the path, ranked by the better of the path's
    /// and the name's score, shallower paths first on a tie. (Adding the two
    /// would put a long name that scatters the query, matched twice, above
    /// `src/main.rs` for "srcmainrs".) An empty query lists files by
    /// recency, or no directories at all.
    pub fn search(&self, query: &str, limit: usize, kinds: NameKinds) -> Vec<NameMatch> {
        let query = normalize_name_query(query);
        let query = query.trim();
        let wanted = |is_dir: bool| match kinds {
            NameKinds::FilesAndDirectories => true,
            NameKinds::Files => !is_dir,
            NameKinds::Directories => is_dir,
        };
        if query.is_empty() {
            if kinds == NameKinds::Directories {
                return Vec::new();
            }
            let mut files: Vec<&NamePath> =
                self.entries.iter().filter(|entry| !entry.is_dir).collect();
            files.sort_by(|a, b| {
                b.modified
                    .cmp(&a.modified)
                    .then_with(|| a.path.cmp(&b.path))
            });
            return files
                .into_iter()
                .take(limit)
                .map(|entry| NameMatch {
                    path: entry.path.clone(),
                    is_dir: entry.is_dir,
                    score: 0,
                })
                .collect();
        }
        let candidates: Vec<&NamePath> = self
            .entries
            .iter()
            .filter(|entry| wanted(entry.is_dir))
            .collect();
        let config = neo_frizbee::Config::default().sort(neo_frizbee::SortStrategy::Unsorted);
        let mut matcher = neo_frizbee::Matcher::new(query, &config);
        let paths: Vec<&str> = candidates.iter().map(|entry| entry.path.as_str()).collect();
        let by_path = if paths.len() >= PARALLEL_MATCH_FROM {
            matcher.match_list_parallel(&paths, 0)
        } else {
            matcher.match_list(&paths)
        };
        let matched: Vec<&NamePath> = by_path
            .iter()
            .map(|found| candidates[found.index as usize])
            .collect();
        let names: Vec<&str> = matched.iter().map(|entry| file_name(&entry.path)).collect();
        let mut name_scores = vec![0u16; matched.len()];
        for found in matcher.match_list(&names) {
            name_scores[found.index as usize] = found.score;
        }
        let mut found: Vec<(i64, &NamePath)> = by_path
            .iter()
            .zip(&matched)
            .zip(name_scores)
            .map(|((by_path, entry), name)| (i64::from(by_path.score.max(name)), *entry))
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

    /// Plain-text content search over the listed files of `root`, with smart
    /// case, the way fff greps without its content index. Reads the most
    /// recently changed files first, on several threads, skips binary,
    /// oversized and cloud-only files, takes at most `per_file` matching
    /// lines from a file, and stops at `limit` lines or once `budget` runs
    /// out (`truncated`).
    pub fn search_content(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        per_file: usize,
        budget: std::time::Duration,
    ) -> ContentSearch {
        let ignore_case = !query.chars().any(char::is_uppercase);
        let Ok(pattern) = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(ignore_case)
            .build()
        else {
            return ContentSearch::default();
        };
        // Opening a cloud-only file downloads it.
        let mut files: Vec<&NamePath> = self
            .entries
            .iter()
            .filter(|entry| {
                !entry.is_dir
                    && !entry.cloud
                    && entry.size > 0
                    && entry.size <= MAX_GREP_FILE_SIZE
                    && !fff_search::is_known_binary_extension(Path::new(&entry.path))
            })
            .collect();
        files.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.path.cmp(&b.path))
        });
        let deadline = Instant::now() + budget;
        let next = AtomicUsize::new(0);
        let found_lines = AtomicUsize::new(0);
        let out_of_time = AtomicBool::new(false);
        let results: Mutex<Vec<(usize, Vec<ContentMatch>)>> = Mutex::new(Vec::new());
        // Mostly waiting on file reads, so more threads than a CPU-bound task.
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(16))
            .unwrap_or(4);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        // Files are claimed in order, so the claimed ones are
                        // always a prefix of the list.
                        if found_lines.load(Ordering::Relaxed) > limit {
                            return;
                        }
                        if Instant::now() >= deadline {
                            out_of_time.store(true, Ordering::Relaxed);
                            return;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = files.get(index) else {
                            return;
                        };
                        let matches = grep_file(root, &file.path, &pattern, per_file);
                        if !matches.is_empty() {
                            found_lines.fetch_add(matches.len(), Ordering::Relaxed);
                            lock(&results).push((index, matches));
                        }
                    }
                });
            }
        });
        tracing::debug!(
            searched = next.into_inner().min(files.len()),
            files = files.len(),
            "names index content search"
        );
        let mut results = results
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        results.sort_unstable_by_key(|(index, _)| *index);
        let mut matches: Vec<ContentMatch> = results
            .into_iter()
            .flat_map(|(_, matches)| matches)
            .collect();
        let truncated = matches.len() > limit || out_of_time.into_inner();
        matches.truncate(limit);
        ContentSearch {
            matches,
            truncated,
            indexing: false,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The matching lines of one file, or none when the file is binary (a NUL
/// byte in its first [`BINARY_SNIFF_BYTES`]). Reads at most
/// [`MAX_GREP_FILE_SIZE`] bytes, in case it grew since the walk.
fn grep_file(
    root: &Path,
    relative: &str,
    pattern: &regex::Regex,
    per_file: usize,
) -> Vec<ContentMatch> {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(root.join(relative)) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_GREP_FILE_SIZE)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Vec::new();
    }
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let text: &str = &text;
    let mut found = Vec::new();
    let mut line: Option<FoundLine> = None;
    // Newlines before `counted` are already in `number`.
    let mut number = 1u64;
    let mut counted = 0;
    let mut line_start = 0;
    for hit in pattern.find_iter(text) {
        if let Some(current) = line.as_mut()
            && hit.start() < current.end
        {
            current.ranges.push((hit.start(), hit.end()));
            continue;
        }
        if let Some(done) = line.take() {
            found.push(done.into_match(text, relative));
            if found.len() == per_file {
                return found;
            }
        }
        for (offset, byte) in text.as_bytes()[counted..hit.start()].iter().enumerate() {
            if *byte == b'\n' {
                number += 1;
                line_start = counted + offset + 1;
            }
        }
        counted = hit.start();
        let end = text[hit.start()..]
            .find('\n')
            .map_or(text.len(), |at| hit.start() + at);
        line = Some(FoundLine {
            number,
            start: line_start,
            end,
            ranges: vec![(hit.start(), hit.end())],
        });
    }
    if let Some(done) = line {
        found.push(done.into_match(text, relative));
    }
    found
}

/// A matching line: its byte span in the file and the matches' spans.
struct FoundLine {
    number: u64,
    start: usize,
    end: usize,
    ranges: Vec<(usize, usize)>,
}

impl FoundLine {
    fn into_match(self, text: &str, relative: &str) -> ContentMatch {
        let line = text[self.start..self.end].trim_end_matches('\r');
        let indent = line.len() - line.trim_start().len();
        let mut preview_end = line.len().min(indent + MAX_PREVIEW_BYTES);
        while !line.is_char_boundary(preview_end) {
            preview_end -= 1;
        }
        let preview = &line[indent..preview_end];
        let column = self.ranges[0].0 - self.start;
        ContentMatch {
            path: relative.to_owned(),
            line: self.number,
            column: column as u64,
            preview: preview.to_owned(),
            ranges: self
                .ranges
                .iter()
                .map(|(start, end)| {
                    let start = (start - self.start).saturating_sub(indent);
                    let end = (end - self.start).saturating_sub(indent);
                    (
                        start.min(preview.len()) as u32,
                        end.min(preview.len()) as u32,
                    )
                })
                .filter(|(start, end)| start < end)
                .collect(),
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the plain folder `root` (canonical) holds more than `limit`
/// files, counted the way fff would index them outside a repository: hidden,
/// dependency and gitignored paths left out. Stops as soon as it knows.
pub fn holds_more_files_than(root: &Path, limit: usize) -> bool {
    let files = AtomicUsize::new(0);
    walker(root, false).run(|| {
        let files = &files;
        Box::new(move |entry| {
            let is_file = entry
                .ok()
                .and_then(|entry| entry.file_type())
                .is_some_and(|kind| kind.is_file());
            if is_file && files.fetch_add(1, Ordering::Relaxed) >= limit {
                return ignore::WalkState::Quit;
            }
            ignore::WalkState::Continue
        })
    });
    files.into_inner() > limit
}

fn walker(root: &Path, home: bool) -> ignore::WalkParallel {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4);
    ignore::WalkBuilder::new(root)
        .hidden(true)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(false)
        .threads(threads)
        .filter_entry(move |entry| entry.depth() == 0 || !skipped(entry, home))
        .build_parallel()
}

struct Batch<'a> {
    items: Vec<NamePath>,
    sink: &'a Mutex<Vec<NamePath>>,
}

impl Batch<'_> {
    /// Move the batch into the shared list; returns the list's length.
    fn flush(&mut self) -> usize {
        let mut all = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        all.append(&mut self.items);
        all.len()
    }
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if !self.items.is_empty() {
            self.flush();
        }
    }
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// Folders the walk never enters. Only folders are checked: listing a
/// cloud-only file's name downloads nothing, while listing a cloud-only
/// folder fetches its contents.
fn skipped(entry: &ignore::DirEntry, home: bool) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    let name = entry.file_name().to_string_lossy().to_lowercase();
    if name == ".git" || SKIPPED_DIRS.contains(&name.as_str()) {
        return true;
    }
    if home
        && entry.depth() == 1
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
    entry
        .metadata()
        .is_ok_and(|metadata| is_cloud_placeholder(&metadata))
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

    fn folder(paths: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
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

    fn paths(index: &NamesIndex, query: &str, kinds: NameKinds) -> Vec<String> {
        index
            .search(query, 50, kinds)
            .into_iter()
            .map(|found| found.path)
            .collect()
    }

    #[test]
    fn skips_cloud_machine_hidden_and_dependency_folders() {
        let (_dir, root) = folder(&[
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
        let index = NamesIndex::build(&root, true, &AtomicBool::new(false));
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["boxes/plan.md", "dev/notes/plan.md"]
        );
    }

    #[test]
    fn only_home_skips_its_cloud_and_machine_folders() {
        let (_dir, root) = folder(&[
            "Library/src/plan.md",
            "box/plan.md",
            "app/node_modules/pkg/plan.md",
            ".cache/plan.md",
        ]);
        let index = NamesIndex::build(&root, false, &AtomicBool::new(false));
        assert_eq!(
            paths(&index, "plan", NameKinds::Files),
            ["box/plan.md", "Library/src/plan.md"]
        );
    }

    #[test]
    fn counts_the_files_fff_would_index() {
        let (_dir, root) = folder(&[
            "a/one.rs",
            "a/two.rs",
            "b/three.rs",
            "b/node_modules/pkg/index.js",
            "b/node_modules/pkg/lib.js",
            ".cache/blob",
            ".cache/blob2",
            "c/.git/",
            "c/.gitignore",
            "c/dist/out.js",
            "c/dist/out2.js",
        ]);
        std::fs::write(root.join("c/.gitignore"), "dist/\n").unwrap();
        // one.rs, two.rs and three.rs: dependency, hidden and gitignored
        // files are left out.
        assert!(holds_more_files_than(&root, 2));
        assert!(!holds_more_files_than(&root, 3));
    }

    #[test]
    fn fuzzy_ranks_names_before_paths_and_filters_kinds() {
        let (_dir, root) = folder(&[
            "comet/",
            "dev/comet/src/main.rs",
            "dev/comet-notes.md",
            "x/comets/a.rs",
        ]);
        let index = NamesIndex::build(&root, true, &AtomicBool::new(false));
        assert_eq!(
            paths(&index, "comet", NameKinds::Directories),
            ["comet", "dev/comet", "x/comets", "dev/comet/src"]
        );
        assert_eq!(
            paths(&index, "comet", NameKinds::Files)[0],
            "dev/comet-notes.md"
        );
        // The query's characters in order, not a substring.
        assert_eq!(
            paths(&index, "cmtnts", NameKinds::Files),
            ["dev/comet-notes.md"]
        );
        assert!(paths(&index, "zzz", NameKinds::Files).is_empty());
        assert!(paths(&index, "", NameKinds::Directories).is_empty());
    }

    #[test]
    fn a_query_across_folders_beats_a_long_name_that_scatters_it() {
        let (_dir, root) = folder(&[
            "models/assignProjectMembersEndpointApiV1TenantProjectsPostParams.ts",
            "app/src/main.rs",
        ]);
        let index = NamesIndex::build(&root, false, &AtomicBool::new(false));
        assert_eq!(
            paths(&index, "srcmainrs", NameKinds::Files)[0],
            "app/src/main.rs"
        );
    }

    #[test]
    fn an_empty_query_lists_files_by_recency() {
        let (_dir, root) = folder(&["old.md", "deep/er/new.md", "middle.md"]);
        let now = SystemTime::now();
        for (path, age) in [("old.md", 300), ("deep/er/new.md", 0), ("middle.md", 100)] {
            std::fs::File::options()
                .write(true)
                .open(root.join(path))
                .unwrap()
                .set_modified(now - std::time::Duration::from_secs(age))
                .unwrap();
        }
        let index = NamesIndex::build(&root, false, &AtomicBool::new(false));
        assert_eq!(
            paths(&index, "", NameKinds::FilesAndDirectories),
            ["deep/er/new.md", "middle.md", "old.md"]
        );
    }
}
