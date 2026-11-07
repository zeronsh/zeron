//! Ties a spawned `git` and everything it starts to the lifetime of a guard.
//!
//! On Windows `git` (Git for Windows' `cmd\git.exe`) is a launcher that spawns
//! the real `git.exe`. `Child::start_kill` and `kill_on_drop` only end the
//! launcher, so a capture that is truncated or cancelled leaves the real git
//! running, blocked on a pipe nobody reads. A job object with
//! `KILL_ON_JOB_CLOSE` kills the whole tree when the guard drops. Elsewhere git
//! is the process itself and the guard is a no-op.

pub(crate) struct GitTree(#[cfg(windows)] isize);

impl GitTree {
    /// Adopt a freshly spawned child. Best effort: on failure the child simply
    /// keeps the plain `kill_on_drop` behaviour.
    #[cfg(windows)]
    pub(crate) fn adopt(child: &tokio::process::Child) -> Self {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let Some(process) = child.raw_handle() else {
            return Self(0);
        };
        // SAFETY: plain Win32 calls on handles we own; `process` outlives the call.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Self(0);
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
                && AssignProcessToJobObject(job, process as _) != 0;
            if !ok {
                CloseHandle(job);
                return Self(0);
            }
            Self(job as isize)
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn adopt(_child: &tokio::process::Child) -> Self {
        Self()
    }
}

#[cfg(windows)]
impl Drop for GitTree {
    fn drop(&mut self) {
        if self.0 != 0 {
            // Closing the last handle kills every process still in the job.
            // SAFETY: the handle came from CreateJobObjectW and is closed once.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0 as _) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::GitTree;
    use std::time::Duration;

    /// Dropping the guard must end a git that is still running, and so its children.
    #[tokio::test]
    async fn dropping_the_guard_kills_a_running_git() {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["-c", "alias.hang=!sleep 60", "hang"]);
        let mut child = cmd.spawn().expect("git spawns");
        let tree = GitTree::adopt(&child);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(child.try_wait().unwrap().is_none(), "git still running");
        drop(tree);
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("git was killed with the guard")
            .unwrap();
    }
}
