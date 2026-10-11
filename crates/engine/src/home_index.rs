//! Names-only index of the host's home folder, for chats that still run in
//! `~` and the new-project folder search.
//!
//! Home is never handed to fff: after its scan, fff opens every file up to
//! 10 MB to tell text from binary, which downloads every cloud-only file
//! under home (OneDrive on Windows, iCloud Drive and File Provider folders
//! on macOS). This walk reads directory listings only, and skips cloud
//! folders, hidden folders and dependency folders altogether. It holds no
//! file contents, so home has no content search.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::workspace_search::{NameKinds, NameMatch, normalize_name_query};

/// Paths kept at most; a larger home answers from the first ones walked.
pub const MAX_ENTRIES: usize = 250_000;

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
    /// Walk `home` (canonical). `cancelled` stops the walk early.
    pub fn build(home: &Path, cancelled: &AtomicBool) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4);
        let collected: Mutex<Vec<HomePath>> = Mutex::new(Vec::new());
        ignore::WalkBuilder::new(home)
            .hidden(true)
            .follow_links(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(false)
            .threads(threads)
            .filter_entry(|entry| entry.depth() == 0 || !skipped(entry))
            .build_parallel()
            .run(|| {
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
                    let Ok(relative) = entry.path().strip_prefix(home) else {
                        return ignore::WalkState::Continue;
                    };
                    let path = relative.to_string_lossy().replace('\\', "/");
                    batch.items.push(HomePath {
                        lower: path.to_lowercase(),
                        path,
                        is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
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

struct Batch<'a> {
    items: Vec<HomePath>,
    sink: &'a Mutex<Vec<HomePath>>,
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
fn skipped(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    let name = entry.file_name().to_string_lossy().to_lowercase();
    if name == ".git" || SKIPPED_DIRS.contains(&name.as_str()) {
        return true;
    }
    if entry.depth() == 1
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
        let index = HomeIndex::build(&root, &AtomicBool::new(false));
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
        let index = HomeIndex::build(&root, &AtomicBool::new(false));
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
}
