//! Process ownership, including descendants that outlive their agent.
use std::ops::{Deref, DerefMut};

use crate::process::{Child as ProcessChild, Command};

pub(crate) fn configure(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    for key in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
        "CLAUDE_AGENT_SDK_VERSION",
    ] {
        command.env_remove(key);
    }
}

pub(crate) struct Child {
    inner: ProcessChild,
    #[cfg(unix)]
    group: Option<i32>,
}

impl Child {
    pub(crate) fn new(inner: ProcessChild) -> Self {
        Self {
            #[cfg(unix)]
            group: Some(-(inner.id().expect("newly spawned child") as i32)),
            inner,
        }
    }

    pub(crate) async fn shutdown(&mut self, grace: std::time::Duration) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            crate::send_signal(&group, crate::Signal::Term);
            // Pi uses detached bash groups and cleans them in its TERM handler.
            // Give descendants their grace even when the adapter exited first.
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                let _ = self.inner.try_wait();
                // SAFETY: signal 0 only checks the private process group.
                if unsafe { libc::kill(group, 0) } != 0 {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    crate::send_signal(&group, crate::Signal::Kill);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        crate::shutdown_child(&mut self.inner, grace).await;
    }

    pub(crate) fn terminate_group(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            crate::send_signal(&group, crate::Signal::Kill);
        }
    }
}

impl Deref for Child {
    type Target = ProcessChild;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // Tokio's kill_on_drop covers the direct child. Keep the group id
        // even after wait() has reaped it, so descendants cannot escape cleanup.
        self.terminate_group();
    }
}

impl Child {
    /// Take the process and its group out WITHOUT the group-kill `Drop`
    /// (the caller owns both now).
    fn into_parts(self) -> (ProcessChild, Option<i32>) {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped or used again, so the process is
        // moved out exactly once.
        let inner = unsafe { std::ptr::read(&this.inner) };
        #[cfg(unix)]
        let group = this.group;
        #[cfg(not(unix))]
        let group = None;
        (inner, group)
    }
}

/// The agent child of a run that can be handed across a live update
/// (`crate::handoff`): spawned by this image or adopted from the previous
/// one, leading its own process group either way, so an adopted agent's
/// descendants are signalled with it exactly as a spawned one's.
///
/// Like [`Child`], dropping it kills the whole group, with two exceptions for
/// a hand-over: [`Self::disarm`] while the run is frozen (the group may belong
/// to the next image by the time a frozen task is dropped; `arm` again on a
/// thaw) and [`Self::release`] for a same-process successor.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct GroupChild {
    handle: crate::handoff::ChildHandle,
    /// The group to signal (a negative pid), if the child leads one.
    group: Option<i32>,
    armed: bool,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl GroupChild {
    /// Take over a freshly spawned child (spawned with `kill_on_drop(false)`:
    /// the group kill below covers it).
    pub(crate) fn spawned(child: Child) -> Self {
        let (inner, group) = child.into_parts();
        Self {
            handle: crate::handoff::ChildHandle::Owned(inner),
            group,
            armed: true,
        }
    }

    /// Adopt a child of this process that leads its own group (`pgid`, as
    /// the previous image exported it). The group is only taken on when it
    /// is the child's own: a pid that is not our child, or a group it does
    /// not lead, is never signalled as a group.
    #[cfg(unix)]
    pub(crate) fn adopt(pid: i32, pgid: Option<i32>) -> std::io::Result<Self> {
        let handle = crate::handoff::ChildHandle::adopt(pid)?;
        let group = (pgid == Some(pid) && handle.id().is_some()).then_some(-pid);
        Ok(Self {
            handle,
            group,
            armed: true,
        })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.handle.id()
    }

    /// The process group id this child leads, for a hand-over.
    #[cfg(unix)]
    pub(crate) fn pgid(&self) -> Option<i32> {
        self.group.map(|group| -group)
    }

    pub(crate) async fn wait(&mut self) -> std::io::Result<crate::handoff::ExitOutcome> {
        self.handle.wait().await
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<crate::handoff::ExitOutcome>> {
        self.handle.try_wait()
    }

    #[cfg(unix)]
    pub(crate) fn signal_target(&self) -> Option<i32> {
        self.handle.signal_target()
    }

    #[cfg(windows)]
    pub(crate) fn signal_target(&self) -> Option<std::sync::Arc<crate::windows_process::Job>> {
        self.handle.signal_target()
    }

    pub(crate) fn request_group_shutdown(&self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            crate::send_signal(&group, crate::Signal::Term);
        }
    }

    /// As [`Child::shutdown`]: SIGTERM the group, give it `grace`, SIGKILL
    /// what is left, then stop and reap the child itself.
    pub(crate) async fn shutdown(&mut self, grace: std::time::Duration) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            crate::send_signal(&group, crate::Signal::Term);
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                let _ = self.handle.try_wait();
                // SAFETY: signal 0 only checks the private process group.
                if unsafe { libc::kill(group, 0) } != 0 {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    crate::send_signal(&group, crate::Signal::Kill);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        self.handle.shutdown(grace).await;
    }

    /// See [`crate::handoff::ChildHandle::hold_reaping`].
    #[cfg(unix)]
    pub(crate) fn hold_reaping(&self) -> bool {
        self.handle.hold_reaping()
    }

    #[cfg(unix)]
    pub(crate) fn release_reaping(&self) {
        self.handle.release_reaping();
    }

    /// A frozen run: dropping it must not kill the group (see the type docs).
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    /// A thawed run owns its group again.
    pub(crate) fn arm(&mut self) {
        self.armed = true;
    }

    /// Give the child and its group up without signalling or reaping
    /// anything (a same-process successor adopts them): `(pid, pgid)`, or
    /// `None` when the child is already reaped. See
    /// [`crate::handoff::ChildHandle::release`].
    #[cfg(unix)]
    pub(crate) fn release(self) -> Option<(i32, Option<i32>)> {
        let pgid = self.pgid();
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped or used again, so the handle is
        // moved out exactly once.
        let handle = unsafe { std::ptr::read(&this.handle) };
        handle.release().map(|pid| (pid, pgid))
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.armed
            && let Some(group) = self.group.take()
        {
            crate::send_signal(&group, crate::Signal::Kill);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn acp_environment_removes_nested_claude_markers() {
        let mut command = Command::new("unused");
        configure(&mut command);
        let removed: Vec<_> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_str().unwrap())
            .collect();
        assert_eq!(
            removed,
            vec![
                "CLAUDECODE",
                "CLAUDE_AGENT_SDK_VERSION",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SSE_PORT"
            ]
        );
    }
}
