//! Immutable verified copies the app loads from (contract "Snapshot"):
//! `<plugins>/.loaded/<id>/<hash16>/`.
//!
//! A snapshot is trusted only as far as it's checked (R11): every `ensure`
//! checks one already there against the files it should hold (exactly
//! those, each its signed hash, no link anywhere, no dot entry), and a
//! service checks its snapshot again before each load ([`verify_hash`]).
//! One that fails is removed and made again. Its files are read-only. The
//! retention sweep never follows a link out of `.loaded` (R25).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::manifest::{MANIFEST_FILE, Manifest};
use super::signing::{FileHashes, content_hash, sha256_hex};

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
    // The folders above it are real ones (a link would put it anywhere).
    for dir in [plugins_dir.join(LOADED_DIR), plugins_dir.join(LOADED_DIR).join(id)] {
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
            remove(&dir);
        }
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        match verify(&target, manifest_bytes, files) {
            Ok(()) => return Ok(target),
            Err(why) => {
                tracing::warn!(plugin = %id, %why, "plugins: a snapshot didn't match what it should hold; made again");
                remove(&target);
            }
        }
    }
    let parent = target.parent().ok_or("no parent")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".tmp-{}", crate::new_id()));
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        for (rel, expected) in files {
            let from = zeron_plugin_guard::path::join(source, rel)?;
            let to = zeron_plugin_guard::path::join(&temp, rel)?;
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
        read_only(&temp);
        match std::fs::rename(&temp, &target) {
            Ok(()) => Ok(()),
            // Another rescan made it first (and it's what it should be).
            Err(_) if verify(&target, manifest_bytes, files).is_ok() => {
                remove(&temp);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    })();
    if result.is_err() {
        remove(&temp);
    }
    result.map(|()| target)
}

/// Every file under `dir`, by its `/` path: refused if anything in it is
/// a link, a dot entry, or neither a file nor a folder.
fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().into_string().map_err(|_| "a name that isn't text".to_string())?;
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if name.starts_with('.') || kind.is_symlink() {
            return Err(format!("{rel} isn't part of it"));
        }
        if kind.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if kind.is_file() {
            out.insert(rel, entry.path());
        } else {
            return Err(format!("{rel} isn't a file"));
        }
        if out.len() > 20_000 {
            return Err("too many files".into());
        }
    }
    Ok(())
}

/// The snapshot folder itself, a real folder (never a link).
fn real_dir(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err("it isn't a folder of its own".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Whether the snapshot at `dir` holds exactly `files` (each its hash)
/// and `manifest_bytes`, with no link or dot entry anywhere in it.
pub fn verify(dir: &Path, manifest_bytes: &[u8], files: &FileHashes) -> Result<(), String> {
    real_dir(dir)?;
    let mut found = BTreeMap::new();
    walk(dir, "", &mut found)?;
    let manifest = found.remove(MANIFEST_FILE).ok_or("its manifest is missing")?;
    if std::fs::read(&manifest).map_err(|e| e.to_string())? != manifest_bytes {
        return Err("its manifest changed".into());
    }
    if found.len() != files.len() {
        return Err("its files aren't the signed ones".into());
    }
    for (rel, path) in &found {
        let expected = files.get(rel).ok_or_else(|| format!("{rel} isn't one of its signed files"))?;
        if &sha256_hex(&std::fs::read(path).map_err(|e| e.to_string())?) != expected {
            return Err(format!("{rel} changed"));
        }
    }
    Ok(())
}

/// Whether the snapshot at `dir` is still the version whose content hash
/// is `hash` (its manifest and every file, nothing more): what a load
/// checks, knowing only the hash.
pub fn verify_hash(dir: &Path, hash: &str) -> Result<(), String> {
    real_dir(dir)?;
    let mut found = BTreeMap::new();
    walk(dir, "", &mut found)?;
    let manifest = found.remove(MANIFEST_FILE).ok_or("its manifest is missing")?;
    let manifest = Manifest::parse(&std::fs::read(&manifest).map_err(|e| e.to_string())?)?;
    let mut files = FileHashes::new();
    for (rel, path) in found {
        files.insert(rel, sha256_hex(&std::fs::read(&path).map_err(|e| e.to_string())?));
    }
    if content_hash(&manifest, &files) != hash {
        return Err("its files aren't the version allowed".into());
    }
    Ok(())
}

/// Its files read-only (a nudge; checks are what hold).
fn read_only(dir: &Path) {
    let mut found = BTreeMap::new();
    if walk(dir, "", &mut found).is_ok() {
        for path in found.values() {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_readonly(true);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

/// Remove `path`: a link as the link (never what it points to), a folder
/// with its read-only files.
pub fn remove(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if !meta.is_dir() {
        let _ = std::fs::remove_file(path);
        return;
    }
    #[cfg(windows)]
    {
        // (Windows won't delete a read-only file.)
        fn writable(dir: &Path) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    writable(&entry.path());
                } else if kind.is_file()
                    && let Ok(meta) = entry.metadata()
                {
                    let mut perms = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(entry.path(), perms);
                }
            }
        }
        writable(path);
    }
    let _ = std::fs::remove_dir_all(path);
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
        // (Never through a link: `.loaded`, an id's folder or a snapshot that
        // is one goes as a link, and what it points to stays.)
        if std::fs::symlink_metadata(&loaded).is_ok_and(|m| !m.is_dir()) {
            remove(&loaded);
            return !self.previous.is_empty();
        }
        let Ok(ids) = std::fs::read_dir(&loaded) else {
            return !self.previous.is_empty();
        };
        for id_entry in ids.flatten() {
            let id = id_entry.file_name().to_string_lossy().into_owned();
            if !id_entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove(&id_entry.path());
                continue;
            }
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
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    remove(&entry.path());
                    continue;
                }
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
                remove(&entry.path());
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

    fn focus(plugins: &Path) -> FileHashes {
        std::fs::create_dir_all(plugins.join("focus/lib")).unwrap();
        std::fs::write(plugins.join("focus/init.luau"), "a").unwrap();
        std::fs::write(plugins.join("focus/lib/x.luau"), "b").unwrap();
        [("init.luau".to_string(), sha256_hex(b"a")), ("lib/x.luau".to_string(), sha256_hex(b"b"))].into_iter().collect()
    }

    /// R11: a snapshot planted (or changed) under `.loaded` is never taken
    /// as it is: `ensure` checks it against the signed files and makes it
    /// again; extra files, links and dot entries don't survive.
    #[test]
    fn a_planted_snapshot_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path();
        let files = focus(plugins);
        let hash = "0123456789abcdef0000";
        let target = snapshot_dir(plugins, "focus", hash);
        // Planted before Zeron ever made it: different code, and a file more.
        std::fs::create_dir_all(target.join("lib")).unwrap();
        std::fs::write(target.join("init.luau"), "evil").unwrap();
        std::fs::write(target.join("lib/x.luau"), "b").unwrap();
        std::fs::write(target.join("evil.luau"), "evil").unwrap();
        std::fs::write(target.join(MANIFEST_FILE), b"{}").unwrap();
        let snap = ensure(plugins, "focus", &plugins.join("focus"), hash, b"{}", &files).unwrap();
        assert_eq!(std::fs::read(snap.join("init.luau")).unwrap(), b"a", "the planted code was kept");
        assert!(!snap.join("evil.luau").exists(), "a planted file was kept");
        assert!(verify(&snap, b"{}", &files).is_ok());
        // Read-only.
        assert!(std::fs::metadata(snap.join("init.luau")).unwrap().permissions().readonly());

        // Changed after: found the next time, and by a load.
        let mut perms = std::fs::metadata(snap.join("init.luau")).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(snap.join("init.luau"), perms).unwrap();
        std::fs::write(snap.join("init.luau"), "evil").unwrap();
        assert!(verify(&snap, b"{}", &files).is_err());
        let snap = ensure(plugins, "focus", &plugins.join("focus"), hash, b"{}", &files).unwrap();
        assert_eq!(std::fs::read(snap.join("init.luau")).unwrap(), b"a");
        // A dot entry or a link inside: refused.
        std::fs::write(snap.join(".evil.luau"), "x").unwrap();
        assert!(verify(&snap, b"{}", &files).is_err());
        remove(&snap.join(".evil.luau"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", snap.join("lib/hosts")).unwrap();
            assert!(verify(&snap, b"{}", &files).is_err());
            remove(&snap.join("lib/hosts"));
        }
        assert!(verify(&snap, b"{}", &files).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn a_snapshot_that_is_a_link_is_replaced_and_what_it_points_to_left() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let files = focus(&plugins);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("init.luau"), "a").unwrap();
        std::fs::create_dir_all(elsewhere.join("lib")).unwrap();
        std::fs::write(elsewhere.join("lib/x.luau"), "b").unwrap();
        std::fs::write(elsewhere.join(MANIFEST_FILE), b"{}").unwrap();
        let hash = "0123456789abcdef0000";
        let target = snapshot_dir(&plugins, "focus", hash);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        // (Even one holding exactly the right files: a link is someone's.)
        std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
        let snap = ensure(&plugins, "focus", &plugins.join("focus"), hash, b"{}", &files).unwrap();
        assert!(!std::fs::symlink_metadata(&snap).unwrap().file_type().is_symlink());
        assert!(elsewhere.join("init.luau").exists());

        // An id's folder that's a link: the sweep removes the link alone.
        let victim = dir.path().join("victim");
        std::fs::create_dir_all(victim.join("0000000000000000")).unwrap();
        std::fs::write(victim.join("0000000000000000/keep.txt"), "mine").unwrap();
        std::os::unix::fs::symlink(&victim, plugins.join(LOADED_DIR).join("other")).unwrap();
        Retention::default().sweep(&plugins, [("focus".to_string(), hash[..16].to_string())].into(), Instant::now());
        assert!(victim.join("0000000000000000/keep.txt").exists(), "the sweep followed a link");
        assert!(std::fs::symlink_metadata(plugins.join(LOADED_DIR).join("other")).is_err());
        assert!(snap.exists());
    }

    /// What a load checks: the snapshot's manifest and files hash to the
    /// version allowed.
    #[test]
    fn a_load_checks_the_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path();
        let files = focus(plugins);
        let raw = br#"{"id":"focus","name":"Focus","version":"1.0.0","entry":"init.luau"}"#;
        let manifest = Manifest::parse(raw).unwrap();
        let hash = content_hash(&manifest, &files);
        let snap = ensure(plugins, "focus", &plugins.join("focus"), &hash, raw, &files).unwrap();
        assert!(verify_hash(&snap, &hash).is_ok());
        let mut perms = std::fs::metadata(snap.join("lib/x.luau")).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(snap.join("lib/x.luau"), perms).unwrap();
        std::fs::write(snap.join("lib/x.luau"), "planted").unwrap();
        assert!(verify_hash(&snap, &hash).is_err());
        std::fs::write(snap.join("lib/x.luau"), "b").unwrap();
        std::fs::write(snap.join("lib/more.luau"), "planted").unwrap();
        assert!(verify_hash(&snap, &hash).is_err(), "an extra file passed");
    }
}
