//! A private temp root for one managed adapter run.
//!
//! Archive-installed adapters ship as PyInstaller one-file bundles. The bundle
//! bootloader unpacks its payload (the Antigravity adapter unpacks ~1 GB) into
//! a fresh `_MEI<random>` directory under the child's temp root on every
//! launch, and removes that directory only when the bootloader process exits
//! normally. Adapters are reaped through `TerminateJobObject`, which skips that
//! path entirely, so the unpack directory outlives the run and accumulates on
//! disk — one gigabyte per Antigravity session.
//!
//! Pointing the child's temp variables at a directory this process owns moves
//! the payload somewhere it can delete itself once the job is gone. Removal is
//! retried briefly: the guard drops after the job is terminated, but the killed
//! processes may still be closing handles on the way out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Temp variables the child reads to pick its unpack location. `TMPDIR` is the
/// Unix name PyInstaller honors; `TEMP`/`TMP` are the Windows pair.
const TEMP_VARS: [&str; 3] = ["TEMP", "TMP", "TMPDIR"];

/// How long to keep retrying a removal the child is still holding open.
const REMOVAL_ATTEMPTS: u32 = 10;
const REMOVAL_BACKOFF: Duration = Duration::from_millis(50);

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// Owns one child's temp root and removes it on drop.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Create a uniquely named temp root for a single child process. The label
    /// only aids debugging; the pid and a process-local counter keep concurrent
    /// runs of the same adapter apart.
    pub(crate) fn new(label: &str) -> std::io::Result<Self> {
        let label: String = label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = std::env::temp_dir().join(format!(
            "zeron-{label}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Route the child's temp files into this directory.
    pub(crate) fn apply(&self, command: &mut crate::process::Command) {
        for key in TEMP_VARS {
            command.env(key, self.path());
        }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        for attempt in 1..=REMOVAL_ATTEMPTS {
            match std::fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                // Already gone: nothing to clean up and nothing to report.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) => {
                    if attempt == REMOVAL_ATTEMPTS {
                        tracing::warn!(
                            path = %self.path.display(),
                            %error,
                            "could not remove adapter scratch directory"
                        );
                    } else {
                        std::thread::sleep(REMOVAL_BACKOFF);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard's real job: a terminated adapter leaves its `_MEI*` unpack
    /// directory behind, and the whole tree must go when the guard drops.
    #[test]
    fn drop_removes_the_unpack_tree_a_killed_child_leaves_behind() {
        let root = {
            let dir = ScratchDir::new("unit").unwrap();
            let unpack = dir.path().join("_MEI00001234");
            std::fs::create_dir_all(&unpack).unwrap();
            std::fs::write(unpack.join("payload.bin"), [0u8; 4096]).unwrap();
            assert!(unpack.join("payload.bin").is_file());
            dir.path().to_path_buf()
        };
        assert!(!root.exists(), "scratch directory survived the guard");
    }

    #[test]
    fn concurrent_scratch_dirs_do_not_collide() {
        let first = ScratchDir::new("unit").unwrap();
        let second = ScratchDir::new("unit").unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir() && second.path().is_dir());
    }

    /// `apply` is the half that decides *where* the child unpacks. Read the
    /// environment the command would hand the child instead of trusting it.
    #[test]
    fn apply_points_every_temp_variable_at_the_scratch_dir() {
        let dir = ScratchDir::new("unit").unwrap();
        let mut command = crate::process::Command::new("probe");
        dir.apply(&mut command);
        let envs: Vec<(String, Option<std::ffi::OsString>)> = command
            .as_std_mut()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_ascii_uppercase(),
                    value.map(|v| v.to_os_string()),
                )
            })
            .collect();
        for key in TEMP_VARS {
            let found = envs
                .iter()
                .find(|(name, _)| name == key)
                .and_then(|(_, value)| value.as_deref());
            assert_eq!(
                found,
                Some(dir.path().as_os_str()),
                "{key} should point at the scratch dir"
            );
        }
    }
}
