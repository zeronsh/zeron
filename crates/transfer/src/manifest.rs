//! What a transfer carries: a flat, pre-order list of relative entries.
//!
//! The sender walks its paths into a [`Manifest`]; the receiver trusts
//! nothing in it. [`Manifest::validate`] refuses absolute paths, `..`/`.`
//! components, separators or NULs inside names, duplicates, and any entry
//! whose parent is not a directory listed before it — so no entry can land
//! outside the destination or be written through a symlink the same
//! manifest created. Symlinks travel as symlinks (their targets verbatim);
//! sockets, FIFOs and devices are left out and counted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeron_proto::{FileTransferItem, FileTransferItemKind};

/// A receiver refuses manifests larger than this.
pub const MAX_ENTRIES: usize = 1_000_000;
const MAX_PATH_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 255;
/// macOS and Windows file systems are case-insensitive by default.
const CASE_INSENSITIVE_FS: bool = cfg!(any(windows, target_os = "macos"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `/`-separated, relative to the destination.
    pub path: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub size: u64,
    /// Unix permission bits (`0o777` mask); 0 = the receiver's default.
    #[serde(default)]
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl Entry {
    pub fn is_top_level(&self) -> bool {
        !self.path.contains('/')
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<Entry>,
}

/// A sender's manifest plus where each file's bytes come from.
#[derive(Debug)]
pub struct Built {
    pub manifest: Manifest,
    /// Parallel to `manifest.entries`: the absolute source of each entry.
    pub sources: Vec<PathBuf>,
    /// Entries left out (special files, unreadable folders).
    pub skipped: u64,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| e.size)
            .sum()
    }

    pub fn file_count(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .count() as u64
    }

    /// Identifies a manifest across reconnects (a resumed transfer must be
    /// the same set of files).
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.entries).unwrap_or_default();
        hex(&Sha256::digest(bytes))
    }

    /// The top-level items (what the user picked), with sizes summed.
    pub fn items(&self) -> Vec<FileTransferItem> {
        let mut items: Vec<FileTransferItem> = Vec::new();
        for entry in &self.entries {
            let top = entry.path.split('/').next().unwrap_or_default();
            if entry.is_top_level() {
                items.push(FileTransferItem {
                    name: top.to_owned(),
                    kind: match entry.kind {
                        EntryKind::File => FileTransferItemKind::File,
                        EntryKind::Dir => FileTransferItemKind::Folder,
                        EntryKind::Symlink => FileTransferItemKind::Symlink,
                    },
                    size: 0,
                    file_count: 0,
                    path: None,
                });
            }
            if entry.kind == EntryKind::File
                && let Some(item) = items.iter_mut().rev().find(|i| i.name == top)
            {
                item.size += entry.size;
                item.file_count += 1;
            }
        }
        items
    }

    /// Refuse anything that could escape the destination or confuse it.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_for(CASE_INSENSITIVE_FS)
    }

    fn validate_for(&self, case_insensitive: bool) -> anyhow::Result<()> {
        anyhow::ensure!(!self.entries.is_empty(), "the transfer lists no files");
        anyhow::ensure!(
            self.entries.len() <= MAX_ENTRIES,
            "the transfer lists more than {MAX_ENTRIES} entries"
        );
        let mut seen: HashSet<&str> = HashSet::with_capacity(self.entries.len());
        let mut folded: HashSet<String> = HashSet::new();
        let mut dirs: HashSet<&str> = HashSet::new();
        let mut total: u64 = 0;
        for entry in &self.entries {
            validate_relative(&entry.path)?;
            anyhow::ensure!(
                seen.insert(entry.path.as_str()),
                "duplicate path in transfer: {}",
                entry.path
            );
            // Two names differing only in case would share one file (and
            // one `.part`) on this device's case-insensitive file system.
            if case_insensitive {
                anyhow::ensure!(
                    folded.insert(entry.path.to_lowercase()),
                    "{} differs from another name only in letter case, which this device's file system can't keep apart",
                    entry.path
                );
            }
            if let Some((parent, _)) = entry.path.rsplit_once('/') {
                // Pre-order: a parent is a directory listed earlier. This
                // also rules out writing beneath a symlink or a file.
                anyhow::ensure!(
                    dirs.contains(parent),
                    "{} is not inside a folder of this transfer",
                    entry.path
                );
            }
            match entry.kind {
                EntryKind::Dir => {
                    dirs.insert(entry.path.as_str());
                }
                EntryKind::File => {
                    total = total
                        .checked_add(entry.size)
                        .ok_or_else(|| anyhow::anyhow!("transfer size overflows"))?;
                }
                EntryKind::Symlink => {
                    let target = entry.target.as_deref().unwrap_or_default();
                    anyhow::ensure!(
                        !target.is_empty()
                            && target.len() <= MAX_PATH_BYTES
                            && !target.contains('\0'),
                        "invalid symlink target for {}",
                        entry.path
                    );
                }
            }
            anyhow::ensure!(
                entry.kind == EntryKind::Symlink || entry.target.is_none(),
                "only symlinks carry a target"
            );
        }
        Ok(())
    }
}

/// One relative path: non-empty `/`-separated names, none of them `.`,
/// `..`, or containing a separator, NUL or control character.
pub fn validate_relative(path: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !path.is_empty() && path.len() <= MAX_PATH_BYTES,
        "invalid path length in transfer"
    );
    for name in path.split('/') {
        validate_name(name).map_err(|e| anyhow::anyhow!("refusing {path:?}: {e}"))?;
    }
    Ok(())
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty(), "empty path component");
    anyhow::ensure!(name != "." && name != "..", "relative traversal");
    anyhow::ensure!(name.len() <= MAX_NAME_BYTES, "name too long");
    anyhow::ensure!(
        !name
            .chars()
            .any(|c| c == '\\' || c == '\0' || c.is_control()),
        "separator or control character in a name"
    );
    #[cfg(windows)]
    {
        anyhow::ensure!(
            !name
                .chars()
                .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
                && !name.ends_with(['.', ' ']),
            "name is not valid on Windows"
        );
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit());
        anyhow::ensure!(!reserved, "reserved Windows name");
    }
    Ok(())
}

/// `root` joined with a validated relative path.
pub fn join(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for name in relative.split('/') {
        path.push(name);
    }
    path
}

/// Walk `paths` into a manifest. Top-level symlinks are followed (sending
/// `latest.apk -> build/app.apk` sends the APK); nested ones are kept.
/// Duplicate top-level names get ` (2)`-style suffixes.
pub fn build(paths: &[PathBuf]) -> anyhow::Result<Built> {
    anyhow::ensure!(!paths.is_empty(), "no paths to send");
    let mut built = Built {
        manifest: Manifest::default(),
        sources: Vec::new(),
        skipped: 0,
    };
    let mut names: HashSet<String> = HashSet::new();
    for path in paths {
        anyhow::ensure!(
            path.is_absolute(),
            "paths must be absolute: {}",
            path.display()
        );
        let meta = std::fs::metadata(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("cannot send {}", path.display()))?;
        let name = unique_name(name, &mut names);
        validate_name(&name).map_err(|e| anyhow::anyhow!("cannot send {}: {e}", path.display()))?;
        if meta.is_dir() {
            push(&mut built, name.clone(), EntryKind::Dir, &meta, None, path);
            walk(&mut built, &name, path)?;
        } else if meta.is_file() {
            push(&mut built, name, EntryKind::File, &meta, None, path);
        } else {
            anyhow::bail!("{} is not a regular file or folder", path.display());
        }
    }
    anyhow::ensure!(
        built.manifest.entries.len() <= MAX_ENTRIES,
        "too many files (more than {MAX_ENTRIES})"
    );
    Ok(built)
}

fn walk(built: &mut Built, prefix: &str, dir: &Path) -> anyhow::Result<()> {
    let mut children: Vec<_> = match std::fs::read_dir(dir) {
        Ok(read) => read.filter_map(Result::ok).collect(),
        Err(error) => {
            tracing::debug!(%error, dir = %dir.display(), "skipping unreadable folder");
            built.skipped += 1;
            return Ok(());
        }
    };
    children.sort_by_key(|c| c.file_name());
    for child in children {
        anyhow::ensure!(
            built.manifest.entries.len() <= MAX_ENTRIES,
            "too many files (more than {MAX_ENTRIES})"
        );
        let path = child.path();
        let Some(name) = child.file_name().to_str().map(str::to_owned) else {
            built.skipped += 1;
            continue;
        };
        if validate_name(&name).is_err() {
            built.skipped += 1;
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            built.skipped += 1;
            continue;
        };
        let relative = format!("{prefix}/{name}");
        let kind = meta.file_type();
        if kind.is_symlink() {
            match std::fs::read_link(&path)
                .ok()
                .and_then(|t| t.to_str().map(str::to_owned))
            {
                Some(target) if !target.is_empty() => push(
                    built,
                    relative,
                    EntryKind::Symlink,
                    &meta,
                    Some(target),
                    &path,
                ),
                _ => built.skipped += 1,
            }
        } else if kind.is_dir() {
            push(built, relative.clone(), EntryKind::Dir, &meta, None, &path);
            walk(built, &relative, &path)?;
        } else if kind.is_file() {
            push(built, relative, EntryKind::File, &meta, None, &path);
        } else {
            built.skipped += 1;
        }
    }
    Ok(())
}

fn push(
    built: &mut Built,
    path: String,
    kind: EntryKind,
    meta: &std::fs::Metadata,
    target: Option<String>,
    source: &Path,
) {
    built.manifest.entries.push(Entry {
        path,
        kind,
        size: if kind == EntryKind::File {
            meta.len()
        } else {
            0
        },
        mode: mode_of(meta),
        target,
    });
    built.sources.push(source.to_path_buf());
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

fn unique_name(name: &str, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.to_owned();
    let mut n = 2;
    while !taken.insert(candidate.clone()) {
        candidate = numbered(name, n);
        n += 1;
    }
    candidate
}

/// `report.pdf` → `report (2).pdf`; `folder` → `folder (2)`.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, kind: EntryKind) -> Entry {
        Entry {
            path: path.into(),
            kind,
            size: 0,
            mode: 0,
            target: (kind == EntryKind::Symlink).then(|| "elsewhere".into()),
        }
    }

    fn manifest(entries: Vec<Entry>) -> Manifest {
        Manifest { entries }
    }

    #[test]
    fn traversal_and_malformed_paths_are_refused() {
        for bad in [
            "/etc/passwd",
            "../escape",
            "a/../../escape",
            "a/./b",
            "a//b",
            "a/",
            "",
            "a\\..\\b",
            "nul\0byte",
            "ctl\u{7}",
        ] {
            assert!(validate_relative(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(validate_relative("folder/sub/file.txt").is_ok());
        assert!(validate_relative("..hidden").is_ok());
    }

    #[test]
    fn names_differing_only_in_case_are_refused_on_case_insensitive_receivers() {
        let clash = manifest(vec![
            entry("app", EntryKind::Dir),
            entry("app/Makefile", EntryKind::File),
            entry("app/makefile", EntryKind::File),
        ]);
        clash.validate_for(false).unwrap();
        let error = clash.validate_for(true).unwrap_err().to_string();
        assert!(error.contains("letter case"), "{error}");
    }

    #[test]
    fn entries_must_sit_under_a_folder_listed_before_them() {
        let ok = manifest(vec![
            entry("app", EntryKind::Dir),
            entry("app/src", EntryKind::Dir),
            entry("app/src/main.rs", EntryKind::File),
        ]);
        ok.validate().unwrap();
        // Child before its parent.
        let out_of_order = manifest(vec![
            entry("app/a.txt", EntryKind::File),
            entry("app", EntryKind::Dir),
        ]);
        assert!(out_of_order.validate().is_err());
        // Writing through a symlink the same manifest creates.
        let through_link = manifest(vec![
            entry("link", EntryKind::Symlink),
            entry("link/passwd", EntryKind::File),
        ]);
        assert!(through_link.validate().is_err());
        // Beneath a file.
        let under_file = manifest(vec![
            entry("f", EntryKind::File),
            entry("f/g", EntryKind::File),
        ]);
        assert!(under_file.validate().is_err());
        // Duplicates.
        let duplicate = manifest(vec![
            entry("a", EntryKind::File),
            entry("a", EntryKind::File),
        ]);
        assert!(duplicate.validate().is_err());
        // Absolute or parent-relative paths.
        assert!(
            manifest(vec![entry("/abs", EntryKind::File)])
                .validate()
                .is_err()
        );
        assert!(
            manifest(vec![entry("..", EntryKind::Dir)])
                .validate()
                .is_err()
        );
        // An empty manifest is refused.
        assert!(manifest(vec![]).validate().is_err());
    }

    #[test]
    fn build_walks_folders_keeps_symlinks_and_skips_special_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("README.md"), b"hello").unwrap();
        std::fs::write(root.join("src/nested/lib.rs"), vec![7u8; 3000]).unwrap();
        let single = dir.path().join("app.apk");
        std::fs::write(&single, vec![1u8; 10]).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../README.md", root.join("src/readme-link")).unwrap();
            let fifo = std::ffi::CString::new(root.join("pipe").to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join("README.md"),
                std::fs::Permissions::from_mode(0o750),
            )
            .unwrap();
        }
        let built = build(&[root.clone(), single.clone(), single.clone()]).unwrap();
        built.manifest.validate().unwrap();
        let paths: Vec<_> = built
            .manifest
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths[0], "project");
        assert!(paths.contains(&"project/src/nested/lib.rs"));
        assert!(paths.contains(&"app.apk"));
        assert!(paths.contains(&"app (2).apk"), "{paths:?}");
        assert_eq!(built.manifest.total_bytes(), 5 + 3000 + 10 + 10);
        let items = built.manifest.items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "project");
        assert_eq!(items[0].file_count, 2);
        assert_eq!(items[0].size, 3005);
        #[cfg(unix)]
        {
            assert_eq!(built.skipped, 1, "the FIFO is left out");
            let link = built
                .manifest
                .entries
                .iter()
                .find(|e| e.path == "project/src/readme-link")
                .unwrap();
            assert_eq!(link.kind, EntryKind::Symlink);
            assert_eq!(link.target.as_deref(), Some("../README.md"));
            let readme = built
                .manifest
                .entries
                .iter()
                .find(|e| e.path == "project/README.md")
                .unwrap();
            assert_eq!(readme.mode, 0o750);
        }
        assert!(build(&[PathBuf::from("relative/path")]).is_err());
    }

    #[test]
    fn numbered_names_keep_the_extension() {
        assert_eq!(numbered("report.pdf", 2), "report (2).pdf");
        assert_eq!(numbered("folder", 3), "folder (3)");
        assert_eq!(numbered(".env", 2), ".env (2)");
    }
}
