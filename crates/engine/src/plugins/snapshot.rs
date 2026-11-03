//! Immutable verified copies the app loads from (contract "Snapshot"):
//! `<plugins>/.loaded/<id>/<hash16>/`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::manifest::MANIFEST_FILE;
use super::signing::{FileHashes, sha256_hex};

pub const LOADED_DIR: &str = ".loaded";
/// How long a superseded snapshot stays for an app that's still reloading.
pub const KEEP_PREVIOUS: Duration = Duration::from_secs(60);

pub fn snapshot_dir(plugins_dir: &Path, id: &str, hash: &str) -> PathBuf {
    plugins_dir
        .join(LOADED_DIR)
        .join(id)
        .join(&hash[..hash.len().min(16)])
}

/// Make `<id>/<hash16>/` from the plugin's folder unless it's already there.
/// Copies into a temp dir, checks every copy against `files`, then renames
/// it into place: a snapshot is never modified once it exists.
pub fn ensure(
    plugins_dir: &Path,
    id: &str,
    // The plugin's own folder, copied from: `<data>/plugins/<id>` for yours,
    // the app's folder of them for Zeron's own (never under `plugins_dir`).
    source: &Path,
    hash: &str,
    manifest_bytes: &[u8],
    files: &FileHashes,
) -> Result<PathBuf, String> {
    let target = snapshot_dir(plugins_dir, id, hash);
    if target.is_dir() {
        return Ok(target);
    }
    let parent = target.parent().ok_or("no parent")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".tmp-{}", crate::new_id()));
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        for (rel, expected) in files {
            let from = source.join(rel);
            let to = temp.join(rel);
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            // Refuse a link that appeared since the scan.
            let meta = std::fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
            if !meta.file_type().is_file() {
                return Err(format!("{rel} changed"));
            }
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
            let copied = std::fs::read(&to).map_err(|e| e.to_string())?;
            if &sha256_hex(&copied) != expected {
                return Err(format!("{rel} changed while it was copied"));
            }
        }
        std::fs::write(temp.join(MANIFEST_FILE), manifest_bytes).map_err(|e| e.to_string())?;
        match std::fs::rename(&temp, &target) {
            Ok(()) => Ok(()),
            // Another rescan made it first.
            Err(_) if target.is_dir() => {
                let _ = std::fs::remove_dir_all(&temp);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temp);
    }
    result.map(|()| target)
}

/// Which snapshot each id used last, and since when it was superseded.
#[derive(Default)]
pub struct Retention {
    current: HashMap<String, String>,
    previous: HashMap<String, (String, Instant)>,
}

impl Retention {
    /// Record the current `{id → hash16}` and delete every snapshot that is
    /// neither current nor an id's previous one superseded under
    /// [`KEEP_PREVIOUS`] ago. Returns whether some previous one is still kept
    /// (the caller sweeps again later).
    pub fn sweep(
        &mut self,
        plugins_dir: &Path,
        current: HashMap<String, String>,
        now: Instant,
    ) -> bool {
        for (id, old) in &self.current {
            if current.get(id) != Some(old) {
                self.previous.insert(id.clone(), (old.clone(), now));
            }
        }
        self.previous.retain(|id, (hash, since)| {
            now.duration_since(*since) < KEEP_PREVIOUS && current.get(id) != Some(hash)
        });
        self.current = current;
        let loaded = plugins_dir.join(LOADED_DIR);
        let Ok(ids) = std::fs::read_dir(&loaded) else {
            return !self.previous.is_empty();
        };
        for id_entry in ids.flatten() {
            let id = id_entry.file_name().to_string_lossy().into_owned();
            let Ok(hashes) = std::fs::read_dir(id_entry.path()) else {
                continue;
            };
            let mut keep: HashSet<&str> = HashSet::new();
            if let Some(hash) = self.current.get(&id) {
                keep.insert(hash);
            }
            if let Some((hash, _)) = self.previous.get(&id) {
                keep.insert(hash);
            }
            let mut empty = true;
            for entry in hashes.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if keep.contains(name.as_str()) {
                    empty = false;
                    continue;
                }
                if name.starts_with(".tmp-") {
                    // In flight, or left by a crash: old ones go.
                    let old = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(600));
                    if !old {
                        empty = false;
                        continue;
                    }
                }
                let _ = std::fs::remove_dir_all(entry.path());
            }
            if empty {
                let _ = std::fs::remove_dir(id_entry.path());
            }
        }
        !self.previous.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_snapshot_made_once_and_previous_kept_for_a_while() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path();
        std::fs::create_dir_all(plugins.join("focus/lib")).unwrap();
        std::fs::write(plugins.join("focus/init.luau"), "a").unwrap();
        std::fs::write(plugins.join("focus/lib/x.luau"), "b").unwrap();
        let files: FileHashes = [
            ("init.luau".to_string(), sha256_hex(b"a")),
            ("lib/x.luau".to_string(), sha256_hex(b"b")),
        ]
        .into_iter()
        .collect();
        let hash1 = "0123456789abcdef0000";
        let snap = ensure(plugins, "focus", &plugins.join("focus"), hash1, b"{}", &files).unwrap();
        assert_eq!(snap, plugins.join(".loaded/focus/0123456789abcdef"));
        assert_eq!(std::fs::read(snap.join("lib/x.luau")).unwrap(), b"b");
        assert_eq!(std::fs::read(snap.join(MANIFEST_FILE)).unwrap(), b"{}");

        // A file that changed under the copy is refused.
        std::fs::write(plugins.join("focus/init.luau"), "z").unwrap();
        assert!(ensure(plugins, "focus", &plugins.join("focus"), "ffffffffffffffff", b"{}", &files).is_err());
        assert!(!plugins.join(".loaded/focus/ffffffffffffffff").exists());
        std::fs::write(plugins.join("focus/init.luau"), "a").unwrap();
        let snap2 = ensure(plugins, "focus", &plugins.join("focus"), "1111111111111111", b"{}", &files).unwrap();

        let mut retention = Retention::default();
        let t0 = Instant::now();
        retention.sweep(
            plugins,
            [("focus".to_string(), "0123456789abcdef".to_string())].into(),
            t0,
        );
        assert!(snap.exists());
        assert!(!snap2.exists(), "unreferenced snapshot goes");
        let snap2 = ensure(plugins, "focus", &plugins.join("focus"), "1111111111111111", b"{}", &files).unwrap();
        let keeps = retention.sweep(
            plugins,
            [("focus".to_string(), "1111111111111111".to_string())].into(),
            t0,
        );
        assert!(keeps);
        assert!(snap.exists(), "previous kept");
        assert!(snap2.exists());
        retention.sweep(
            plugins,
            [("focus".to_string(), "1111111111111111".to_string())].into(),
            t0 + KEEP_PREVIOUS + Duration::from_secs(1),
        );
        assert!(!snap.exists(), "previous gone after a minute");
        assert!(snap2.exists());
        // Turned off: kept a minute, then gone with its id folder.
        retention.sweep(plugins, HashMap::new(), t0 + Duration::from_secs(120));
        assert!(snap2.exists());
        retention.sweep(plugins, HashMap::new(), t0 + Duration::from_secs(200));
        assert!(!plugins.join(".loaded/focus").exists());
    }
}
