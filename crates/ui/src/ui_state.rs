//! Persisted UI state (`ui-state.json`): the handful of choices a fast window
//! relaunch (live update, crash recovery) must give back — selected chat,
//! unsent composer drafts with their staged attachments, and which chats had
//! the right pane open.
//!
//! The file is versioned, additive JSON. Every field defaults, a file written
//! by a newer build is ignored rather than misread, and anything unreadable
//! loads as the default snapshot: this state is a convenience and must never
//! be able to block startup. The file holds unsent user text, so it is
//! private (0600) and written atomically.
//!
//! Debounced writing and the flush on quit ride on `SettingsStore`'s scheduler
//! (`settings::save_ui_snapshot`); this module is the pure data + file layer.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "ui-state.json";
/// Directory (under the data dir) holding a private copy of every staged
/// attachment mentioned by a draft. Staged images live in memory only, so a
/// relaunch can bring them back only if their bytes reach disk.
pub const ATTACHMENTS_DIR: &str = "ui-state-attachments";
/// Highest snapshot `version` this build understands.
const CURRENT_VERSION: u32 = 1;

/// Right-pane value: the surface host is open (on its picker after restore;
/// surface handles are process-local, so no specific tab can be restored).
pub const RIGHT_PANE_SURFACES: &str = "surfaces";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSnapshot {
    /// 0 = no snapshot (default); written snapshots carry [`CURRENT_VERSION`].
    pub version: u32,
    pub selected_chat: Option<String>,
    /// Per chat key; the empty key is the new-chat canvas.
    pub drafts: BTreeMap<String, DraftSnapshot>,
    /// Per-chat scroll. Reserved: the transcript's virtualized list anchors on
    /// row ids that are not stable across processes, so nothing writes this
    /// yet, but the field is part of the schema so files stay compatible.
    pub scroll: BTreeMap<String, f32>,
    /// Per chat id, what the right pane showed (see [`RIGHT_PANE_SURFACES`]).
    pub right_pane: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftSnapshot {
    pub text: String,
    pub attachments: Vec<PathBuf>,
}

impl UiSnapshot {
    /// An empty snapshot stamped with the current schema version.
    pub fn new() -> Self {
        Self {
            version: CURRENT_VERSION,
            ..Self::default()
        }
    }

    /// Whether this came from a real file (as opposed to "nothing saved").
    pub fn is_present(&self) -> bool {
        self.version > 0
    }

    /// Drop everything that refers to a chat that no longer exists. The
    /// new-chat canvas draft (empty key) is always kept.
    pub fn retain_known(&mut self, chat_exists: impl Fn(&str) -> bool) {
        self.drafts
            .retain(|key, _| key.is_empty() || chat_exists(key));
        self.right_pane.retain(|key, _| chat_exists(key));
        self.scroll.retain(|key, _| chat_exists(key));
        if self
            .selected_chat
            .as_deref()
            .is_some_and(|chat| !chat_exists(chat))
        {
            self.selected_chat = None;
        }
    }
}

/// Reads and writes `ui-state.json` under one data directory.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    data_dir: PathBuf,
    /// The file on disk has been checked (once) for a newer build's data
    /// before this store's first write; shared by clones.
    newer_checked: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Just enough of a snapshot to read its version, whatever else it holds.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: u32,
}

impl SnapshotStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            newer_checked: Default::default(),
        }
    }

    /// Sibling that keeps a newer build's file safe from our first write.
    pub fn newer_path(&self) -> PathBuf {
        self.data_dir.join(format!("{FILE_NAME}.newer"))
    }

    /// A file written by a newer build is ignored on load, but the first save
    /// would replace it — and a rollback must not destroy the drafts that
    /// build saved. Move it aside (once per store) before writing.
    fn preserve_newer_file(&self) {
        use std::sync::atomic::Ordering;
        if self.newer_checked.swap(true, Ordering::Relaxed) {
            return;
        }
        let path = self.path();
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let newer = serde_json::from_slice::<VersionProbe>(&bytes)
            .is_ok_and(|probe| probe.version > CURRENT_VERSION);
        if !newer {
            return;
        }
        let target = self.newer_path();
        let _ = std::fs::remove_file(&target);
        match std::fs::rename(&path, &target) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
                }
                tracing::warn!(
                    kept = %target.display(),
                    "ui-state from a newer build kept aside before writing ours"
                );
            }
            Err(err) => tracing::warn!(error = %err, "could not keep newer ui-state aside"),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.data_dir.join(FILE_NAME)
    }

    pub fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join(ATTACHMENTS_DIR)
    }

    /// Missing, unreadable, corrupt or newer-than-us files all yield the
    /// default snapshot; this never errors or panics.
    pub fn load(&self) -> UiSnapshot {
        let bytes = match std::fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(err) => {
                if err.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(error = %err, "ui-state unreadable; starting fresh");
                }
                return UiSnapshot::default();
            }
        };
        // Judge the version first: a newer build may have changed field types,
        // which must read as "newer" (and be preserved), not "corrupt".
        if let Ok(probe) = serde_json::from_slice::<VersionProbe>(&bytes)
            && probe.version > CURRENT_VERSION
        {
            tracing::info!(
                version = probe.version,
                "ui-state written by a newer build; ignoring it"
            );
            return UiSnapshot::default();
        }
        match serde_json::from_slice::<UiSnapshot>(&bytes) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                tracing::warn!(error = %err, "ui-state corrupt; starting fresh");
                UiSnapshot::default()
            }
        }
    }

    /// Atomic (tmp + rename), private (0600 on unix).
    pub fn save_now(&self, snapshot: &UiSnapshot) -> io::Result<()> {
        use std::io::Write;

        std::fs::create_dir_all(&self.data_dir)?;
        self.preserve_newer_file();
        let json = serde_json::to_vec(snapshot)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let path = self.path();
        let tmp = path.with_extension("json.tmp");
        // A tmp file left by a crashed write may carry looser permissions
        // than we create with; never write private text into it.
        let _ = std::fs::remove_file(&tmp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(&tmp).and_then(|mut file| {
            file.write_all(&json)?;
            file.sync_all()
        });
        match written.and_then(|()| std::fs::rename(&tmp, &path)) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                Err(err)
            }
        }
    }
}

/// What restoring `restored` into a composer that currently holds `current`
/// should leave there: `Some(text)` to set it, `None` to leave the composer
/// alone. Text the user has already typed is never replaced.
pub fn restored_draft_text(current: &str, restored: &str) -> Option<String> {
    (current.is_empty() && !restored.is_empty()).then(|| restored.to_string())
}

/// Write a private copy of a staged attachment's bytes, returning its path.
/// Layout is `<dir>/<id>/<name>`; an existing copy is reused (attachments are
/// immutable once staged).
pub fn materialize_attachment(
    dir: &Path,
    id: &str,
    name: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    let folder = dir.join(attachment_folder(id));
    let file_name = Path::new(name)
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "attachment".into());
    let path = folder.join(file_name);
    if path.is_file() {
        return Ok(path);
    }
    create_private_dir(&folder)?;
    let tmp = folder.join(".partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut file| {
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => Ok(path),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// The on-disk folder name for an attachment id: ids are uuids in practice,
/// but anything else is flattened so a folder can never escape `dir`.
pub fn attachment_folder(id: &str) -> String {
    let flat: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if flat.is_empty() || flat.chars().all(|c| c == '.') {
        "_".to_string()
    } else {
        flat
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Delete attachment copies whose attachment id is not in `keep`.
pub fn prune_attachments(dir: &Path, keep: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let keep: HashSet<String> = keep.iter().map(|id| attachment_folder(id)).collect();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !keep.contains(name.to_string_lossy().as_ref()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The subset of `paths` that may be re-staged: regular files that still
/// exist inside our own attachments directory (a hand-edited snapshot must
/// not be able to attach arbitrary files).
pub fn restorable_attachments(dir: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| {
            path.starts_with(dir)
                && !path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                && path.is_file()
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        assert_eq!(store.load(), UiSnapshot::default());
        let mut snap = UiSnapshot {
            selected_chat: Some("c1".into()),
            ..Default::default()
        };
        snap.drafts.insert(
            "c1".into(),
            DraftSnapshot {
                text: "half a thought".into(),
                attachments: vec![],
            },
        );
        store.save_now(&snap).unwrap();
        assert_eq!(store.load(), snap);
        std::fs::write(dir.path().join("ui-state.json"), b"{not json").unwrap();
        assert_eq!(store.load(), UiSnapshot::default());
    }

    #[test]
    fn a_newer_snapshot_version_is_ignored_not_misread() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ui-state.json"),
            br#"{"version":999,"selectedChat":"x"}"#,
        )
        .unwrap();
        assert_eq!(SnapshotStore::new(dir.path()).load(), UiSnapshot::default());
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        store.save_now(&UiSnapshot::default()).unwrap();
        let mode = std::fs::metadata(dir.path().join("ui-state.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_loose_tmp_file_does_not_leak_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("ui-state.json.tmp");
        std::fs::write(&tmp, b"stale").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = SnapshotStore::new(dir.path());
        store.save_now(&UiSnapshot::new()).unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_newer_builds_file_is_kept_aside_before_our_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let newer = br#"{"version":999,"selectedChat":"x","drafts":{"x":{"text":"kept"}}}"#;
        std::fs::write(dir.path().join("ui-state.json"), newer).unwrap();
        let store = SnapshotStore::new(dir.path());
        assert_eq!(store.load(), UiSnapshot::default());
        // An older leftover is replaced by the newer file we now move aside.
        std::fs::write(store.newer_path(), b"older aside").unwrap();

        let mut ours = UiSnapshot::new();
        ours.selected_chat = Some("mine".into());
        store.save_now(&ours).unwrap();
        assert_eq!(std::fs::read(store.newer_path()).unwrap(), newer.to_vec());
        assert_eq!(store.load(), ours);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.newer_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Later writes (ours, older version) never touch the kept file.
        ours.selected_chat = Some("again".into());
        store.save_now(&ours).unwrap();
        assert_eq!(std::fs::read(store.newer_path()).unwrap(), newer.to_vec());

        // A newer file whose fields have changed shape is still "newer".
        let dir = tempfile::tempdir().unwrap();
        let odd = br#"{"version":1000,"selectedChat":{"id":"x"}}"#;
        std::fs::write(dir.path().join("ui-state.json"), odd).unwrap();
        let store = SnapshotStore::new(dir.path());
        store.save_now(&UiSnapshot::new()).unwrap();
        assert_eq!(std::fs::read(store.newer_path()).unwrap(), odd.to_vec());
    }

    #[test]
    fn a_corrupt_or_same_version_file_is_not_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ui-state.json"), b"{not json").unwrap();
        let store = SnapshotStore::new(dir.path());
        store.save_now(&UiSnapshot::new()).unwrap();
        assert!(!store.newer_path().exists());
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ui-state.json"),
            br#"{"version":1,"selectedChat":"c","future":{"x":1},"drafts":{"c":{"text":"t"}}}"#,
        )
        .unwrap();
        let loaded = SnapshotStore::new(dir.path()).load();
        assert_eq!(loaded.selected_chat.as_deref(), Some("c"));
        assert_eq!(loaded.drafts["c"].text, "t");
        assert!(loaded.drafts["c"].attachments.is_empty());
        // Wrong-typed data is corrupt, not partially applied.
        std::fs::write(
            dir.path().join("ui-state.json"),
            br#"{"version":1,"selectedChat":7}"#,
        )
        .unwrap();
        assert_eq!(SnapshotStore::new(dir.path()).load(), UiSnapshot::default());
    }

    #[test]
    fn a_restored_draft_never_replaces_text_already_typed() {
        // Nothing typed: the restored draft lands.
        assert_eq!(
            restored_draft_text("", "half a thought"),
            Some("half a thought".to_string())
        );
        // Anything typed (even whitespace) wins untouched.
        assert_eq!(restored_draft_text("new words", "half a thought"), None);
        assert_eq!(restored_draft_text(" ", "half a thought"), None);
        // Nothing to restore leaves the composer alone.
        assert_eq!(restored_draft_text("", ""), None);
        assert_eq!(restored_draft_text("typed", ""), None);
    }

    #[test]
    fn retain_known_drops_vanished_chats_but_keeps_the_canvas_draft() {
        let mut snap = UiSnapshot::new();
        snap.selected_chat = Some("gone".into());
        for key in ["", "live", "gone"] {
            snap.drafts.insert(
                key.into(),
                DraftSnapshot {
                    text: key.into(),
                    attachments: vec![],
                },
            );
            snap.right_pane
                .insert(key.into(), RIGHT_PANE_SURFACES.into());
        }
        snap.retain_known(|id| id == "live");
        assert_eq!(snap.selected_chat, None);
        assert_eq!(
            snap.drafts.keys().map(String::as_str).collect::<Vec<_>>(),
            ["", "live"]
        );
        assert_eq!(
            snap.right_pane
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["live"]
        );
    }

    #[test]
    fn attachments_round_trip_through_private_copies_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(ATTACHMENTS_DIR);
        let kept = materialize_attachment(&root, "id-1", "shot.png", b"png-bytes").unwrap();
        let dropped = materialize_attachment(&root, "id-2", "old.png", b"x").unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), b"png-bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&kept).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A path-traversal name or id stays inside the directory.
        let sneaky = materialize_attachment(&root, "../id", "../../evil.png", b"e").unwrap();
        assert!(sneaky.starts_with(&root));

        prune_attachments(
            &root,
            &["id-1".to_string(), sneaky_id(&sneaky, &root)]
                .into_iter()
                .collect(),
        );
        assert!(kept.exists());
        assert!(!dropped.exists());

        // Only existing files inside our directory are restorable.
        let outside = dir.path().join("outside.png");
        std::fs::write(&outside, b"o").unwrap();
        let missing = root.join("id-9").join("gone.png");
        assert_eq!(
            restorable_attachments(&root, &[kept.clone(), outside, missing]),
            vec![kept]
        );
    }

    fn sneaky_id(path: &Path, root: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .into_owned()
    }
}
