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
    /// The descriptor of the locked file, for a handoff manifest.
    #[cfg(unix)]
    pub(super) fn raw_fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self._file.as_raw_fd()
    }

    /// Give the lock's descriptor up WITHOUT closing it (a same-process
    /// successor owns it now; closing would drop the flock for that original
    /// too). The exec path never calls this.
    #[cfg(unix)]
    pub(super) fn leak(self) {
        use std::os::fd::IntoRawFd;
        let _ = self._file.into_raw_fd();
    }

    /// Re-wrap an inherited (duplicated) descriptor of the lock file: the flock
    /// belongs to the open file description, which the duplicate shares, so it
    /// stays held.
    #[cfg(unix)]
    pub(super) fn adopt(fd: std::os::fd::OwnedFd, store_dir: Option<PathBuf>) -> Self {
        Self {
            _file: File::from(fd),
            store_dir,
        }
    }

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
    /// A live update hands the lease over as an open, flock-ed file: the
    /// adopter re-wraps a DUPLICATE (sharing the open file description), so the
    /// conversation stays locked through the handoff and is released only when
    /// the adopter lets go.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_store_lease_stays_held_by_the_duplicate_after_the_original_closes() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let root = tempfile::tempdir().unwrap();
        let first = Lease::acquire(root.path(), None).await.unwrap();
        let store = first.store_dir.clone().unwrap();
        std::fs::create_dir_all(root.path().join("by-agent")).unwrap();
        std::fs::write(
            root.path().join("by-agent/agent-test"),
            store.to_str().unwrap(),
        )
        .unwrap();

        // The old image exports the descriptor; the new one dups it.
        let inherited = first.raw_fd();
        let copy = unsafe { libc::fcntl(inherited, libc::F_DUPFD_CLOEXEC, 3) };
        assert!(copy >= 3);
        let adopted = Lease::adopt(unsafe { OwnedFd::from_raw_fd(copy) }, Some(store.clone()));
        // The old owner goes away WITHOUT closing (the exec path drops nothing).
        first.leak();

        // Still locked: another open of the lock file cannot take it.
        let locked = |path: &Path| {
            let probe = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock))
        };
        let lock_file = adopted_lock_path(&store);
        assert!(locked(&lock_file), "the adopted lease must hold the lock");
        // The engine closes the inherited original at commit: the DUPLICATE
        // alone keeps the lock (same open file description).
        unsafe { libc::close(inherited) };
        assert!(locked(&lock_file), "the duplicate alone keeps the lock");
        drop(adopted);
        assert!(!locked(&lock_file), "released once the adopter lets go");
    }

    fn adopted_lock_path(store: &Path) -> PathBuf {
        store.join(".zeron-owner.lock")
    }

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
