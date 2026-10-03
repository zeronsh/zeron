//! Device-local worktree destinations. Existing chats retain their saved cwd.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use zeron_proto::{WorktreeSettings, WorktreeSettingsStatus};

use crate::EngineError;

pub(crate) struct WorktreePreferences {
    path: PathBuf,
    default_root: PathBuf,
    environment_override: Option<PathBuf>,
    settings: Mutex<WorktreeSettings>,
}

impl WorktreePreferences {
    pub fn open(
        data_dir: &Path,
        default_root: PathBuf,
        environment_override: Option<PathBuf>,
    ) -> Self {
        let path = data_dir.join("worktree-settings.json");
        let settings = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WorktreeSettings>(&bytes).ok())
            .filter(|settings| {
                !settings.use_custom_directory
                    || settings
                        .custom_directory
                        .as_deref()
                        .is_some_and(|path| !path.is_empty() && Path::new(path).is_absolute())
            })
            .unwrap_or_default();
        Self {
            path,
            default_root,
            environment_override,
            settings: Mutex::new(settings),
        }
    }

    fn root_for(&self, settings: &WorktreeSettings) -> PathBuf {
        self.environment_override.clone().unwrap_or_else(|| {
            if settings.use_custom_directory {
                settings
                    .custom_directory
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.default_root.clone())
            } else {
                self.default_root.clone()
            }
        })
    }

    pub fn root(&self) -> PathBuf {
        self.root_for(&self.settings.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn status(&self) -> WorktreeSettingsStatus {
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        WorktreeSettingsStatus {
            effective_directory: self.root_for(&settings).to_string_lossy().into_owned(),
            default_directory: self.default_root.to_string_lossy().into_owned(),
            environment_override: self
                .environment_override
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            settings,
        }
    }

    pub async fn set(
        &self,
        mut next: WorktreeSettings,
    ) -> Result<WorktreeSettingsStatus, EngineError> {
        if self.environment_override.is_some() {
            return Err(EngineError::Other(
                "Worktree location is controlled by ZERON_WORKTREES_DIR on this device".into(),
            ));
        }
        if next.use_custom_directory {
            let path = next
                .custom_directory
                .clone()
                .ok_or_else(|| EngineError::Other("Choose a worktree folder".into()))?;
            // A disconnected disk must not block an engine worker or leave the
            // settings save pending forever. A late probe cannot publish settings.
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::Builder::new()
                .name("worktree-folder-check".into())
                .spawn(move || {
                    let _ = tx.send(validate_directory(&path));
                })?;
            let path = tokio::time::timeout(Duration::from_secs(6), rx)
                .await
                .map_err(|_| EngineError::Other("Worktree folder check timed out".into()))?
                .map_err(|_| EngineError::Other("Worktree folder check failed".into()))??;
            next.custom_directory = Some(path.to_string_lossy().into_owned());
        }
        {
            let mut current = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
            // Disabling must work even if the previous disk is disconnected,
            // and preserve the last validated directory for the next enable.
            if !next.use_custom_directory {
                next.custom_directory = current.custom_directory.clone();
            }
            let parent = self.path.parent().expect("settings have a data directory");
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer_pretty(&mut temp, &next)
                .map_err(|error| EngineError::Other(error.to_string()))?;
            temp.write_all(b"\n")?;
            temp.as_file().sync_all()?;
            temp.persist(&self.path)
                .map_err(|error| EngineError::Io(error.error))?;
            *current = next;
        }
        Ok(self.status())
    }
}

fn validate_directory(value: &str) -> Result<PathBuf, EngineError> {
    let value = value.trim();
    let home = crate::repos::home_dir();
    let path = zeron_proto::device_paths::expand_home(value, &home.to_string_lossy())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(value));
    if !path.is_absolute() {
        return Err(EngineError::Other(
            "Choose an absolute folder path on the selected device".into(),
        ));
    }
    if path.components().any(|part| {
        part.as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b".git")
    }) {
        return Err(EngineError::Other(
            "Choose a folder outside Git's .git directory".into(),
        ));
    }
    #[cfg(windows)]
    crate::repos::validate_windows_worktree_path(&path.to_string_lossy())?;
    std::fs::create_dir_all(&path)?;
    let path = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    crate::repos::validate_windows_worktree_path(&path.to_string_lossy())?;
    if path.components().any(|part| {
        part.as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b".git")
    }) {
        return Err(EngineError::Other(
            "Choose a folder outside Git's .git directory".into(),
        ));
    }
    // Test actual write access instead of relying on permission bits/ACLs.
    let _probe = tempfile::Builder::new()
        .prefix(".zeron-write-check-")
        .tempfile_in(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(path: &Path) -> WorktreeSettings {
        WorktreeSettings {
            use_custom_directory: true,
            custom_directory: Some(path.to_string_lossy().into_owned()),
        }
    }

    #[tokio::test]
    async fn location_persists_and_disabling_preserves_the_last_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        let chosen = tmp.path().join("another disk").join("checkouts");
        let prefs = WorktreePreferences::open(tmp.path(), default.clone(), None);
        assert_eq!(prefs.root(), default);
        let saved = prefs.set(custom(&chosen)).await.unwrap();
        assert_eq!(
            PathBuf::from(saved.effective_directory),
            chosen.canonicalize().unwrap()
        );
        assert_eq!(
            std::fs::read_dir(&chosen).unwrap().count(),
            0,
            "write probe is removed"
        );
        let reloaded = WorktreePreferences::open(tmp.path(), default.clone(), None);
        assert_eq!(prefs.status(), reloaded.status());
        // A disconnected location must not prevent reverting to the default.
        std::fs::remove_dir_all(&chosen).unwrap();
        let disabled = reloaded.set(WorktreeSettings::default()).await.unwrap();
        assert!(!disabled.settings.use_custom_directory);
        assert_eq!(
            disabled.settings.custom_directory,
            saved.settings.custom_directory
        );
        assert_eq!(reloaded.root(), default);
        assert!(!chosen.exists());
        let restarted = WorktreePreferences::open(tmp.path(), default, None);
        assert_eq!(restarted.status(), disabled);
    }

    #[tokio::test]
    async fn invalid_paths_and_failed_persistence_leave_the_previous_settings_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let prefs = WorktreePreferences::open(tmp.path(), tmp.path().join("default"), None);
        let original = prefs.status();
        let file = tmp.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        for path in [
            PathBuf::from("relative"),
            PathBuf::new(),
            file,
            tmp.path().join(".git/worktrees"),
        ] {
            assert!(
                prefs.set(custom(&path)).await.is_err(),
                "reject {}",
                path.display()
            );
            assert_eq!(prefs.status(), original);
        }
        // A failed atomic replacement must not publish an in-memory change.
        std::fs::create_dir(tmp.path().join("worktree-settings.json")).unwrap();
        assert!(prefs.set(custom(&tmp.path().join("valid"))).await.is_err());
        assert_eq!(prefs.status(), original);
    }

    #[tokio::test]
    async fn explicit_override_wins_over_saved_preferences() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        let prefs = WorktreePreferences::open(tmp.path(), default.clone(), None);
        prefs.set(custom(&tmp.path().join("custom"))).await.unwrap();
        let forced = tmp.path().join("forced");
        let overridden = WorktreePreferences::open(tmp.path(), default, Some(forced.clone()));
        assert_eq!(overridden.root(), forced);
        assert_eq!(
            overridden.status().environment_override,
            Some(forced.to_string_lossy().into_owned())
        );
        assert!(overridden.set(WorktreeSettings::default()).await.is_err());
        assert_eq!(prefs.status().settings, overridden.status().settings);
    }

    #[test]
    fn corrupt_or_incomplete_preferences_use_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        for data in [
            "broken",
            "{}",
            r#"{"useCustomDirectory":true}"#,
            r#"{"useCustomDirectory":true,"customDirectory":"relative"}"#,
        ] {
            std::fs::write(tmp.path().join("worktree-settings.json"), data).unwrap();
            assert_eq!(
                WorktreePreferences::open(tmp.path(), default.clone(), None).root(),
                default
            );
        }
    }
}
