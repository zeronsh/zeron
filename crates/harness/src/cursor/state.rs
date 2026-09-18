//! Exclusive ownership of a Cursor store while its shim is alive. The OS
//! releases the lock on crashes; recovery only runs after acquiring it.
use crate::HarnessError;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct Lease {
    _file: File,
    pub(super) store_dir: Option<PathBuf>,
}

pub(super) fn state_root() -> PathBuf {
    let root = std::env::var_os("ZERON_CURSOR_STATE_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::executable::home_or_current_dir().join(".zeron/cursor-state"));
    if root.is_absolute() {
        root
    } else {
        std::env::current_dir().unwrap_or_default().join(root)
    }
}

impl Lease {
    pub(super) async fn acquire(root: &Path, resume: Option<&str>) -> Result<Self, HarnessError> {
        let store_dir = if let Some(id) = resume {
            // Agent ids are filenames in the shim's marker index.
            if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
                return Err(HarnessError::Protocol("Invalid Cursor session id".into()));
            }
            match std::fs::read_to_string(root.join("by-agent").join(id)) {
                Ok(path) => {
                    let dir = PathBuf::from(path.trim());
                    if path.trim().is_empty() || !dir.is_dir() {
                        return Err(HarnessError::Protocol("Cursor conversation storage is missing; restore its state directory before resuming".into()));
                    }
                    Some(dir)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            }
        } else {
            Some(root.join("agents").join(uuid::Uuid::new_v4().to_string()))
        };
        let lock_dir = store_dir
            .clone()
            .unwrap_or_else(|| root.join("legacy-locks"));
        std::fs::create_dir_all(&lock_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&lock_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let name = if store_dir.is_some() {
            ".zeron-owner.lock".into()
        } else {
            format!(
                "{:x}.lock",
                Sha256::digest(resume.unwrap_or_default().as_bytes())
            )
        };
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock_dir.join(name))?;
        let lock = async {
            loop {
                match file.try_lock() {
                    Ok(()) => return Ok(()),
                    Err(std::fs::TryLockError::WouldBlock) => {
                        tokio::time::sleep(Duration::from_millis(25)).await
                    }
                    Err(std::fs::TryLockError::Error(e)) => return Err(HarnessError::Io(e)),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), lock).await
            .map_err(|_| HarnessError::Protocol("This Cursor conversation is still running. Stop the current run before retrying.".into()))??;
        Ok(Self {
            _file: file,
            store_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn overlapping_resume_waits_for_previous_owner_to_release() {
        let root = tempfile::tempdir().unwrap();
        let first = Lease::acquire(root.path(), None).await.unwrap();
        std::fs::create_dir_all(root.path().join("by-agent")).unwrap();
        std::fs::write(
            root.path().join("by-agent/agent-test"),
            first.store_dir.as_ref().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let second = Lease::acquire(root.path(), Some("agent-test"));
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut second)
                .await
                .is_err()
        );
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), second)
                .await
                .unwrap()
                .is_ok()
        );
    }
}
