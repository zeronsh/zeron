//! Process ownership, including descendants that outlive their agent.
use std::ops::{Deref, DerefMut};

use crate::process::{Child as ProcessChild, Command};

/// A private process group, and none of the markers a Claude Code session
/// leaves in its children's environment. Zeron is often launched from one
/// (or runs inside one, on a Cloud machine): inherited, they make an agent's
/// own Claude believe it is nested in that session — it reports to its
/// socket, IDE port and session id, and takes its effort level. The user's
/// own configuration (`CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_USE_BEDROCK`/`VERTEX`,
/// `CLAUDE_CODE_GIT_BASH_PATH`, `ANTHROPIC_*`) is kept. Every Claude Zeron
/// starts, directly or through an adapter, reports Zeron as its entrypoint.
pub(crate) fn configure(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    for key in INHERITED_SESSION_MARKERS {
        command.env_remove(key);
    }
    command.env("CLAUDE_CODE_ENTRYPOINT", "zeron");
}

const INHERITED_SESSION_MARKERS: [&str; 11] = [
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_AGENT_SDK_VERSION",
];

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
        self.shutdown_with(Vec::new(), grace).await;
    }

    /// [`Self::shutdown`], also ending `earlier`: descendants snapshotted
    /// while the agent was alive. Once it has exited and been reaped, its
    /// orphans can no longer be traced from it.
    pub(crate) async fn shutdown_with(
        &mut self,
        #[cfg_attr(not(unix), allow(unused_mut))] mut earlier: Vec<crate::process::Tracked>,
        grace: std::time::Duration,
    ) {
        // Everything the agent started, taken while it is still attached:
        // a command the agent ran in its own session (Devin does) is not in
        // the agent's process group and would outlive the runtime.
        #[cfg(unix)]
        let tree = {
            if let (Some(_), Some(pid)) = (self.group, self.inner.id()) {
                for found in crate::process::descendants(pid).await {
                    if !earlier.contains(&found) {
                        earlier.push(found);
                    }
                }
            }
            earlier
        };
        #[cfg(not(unix))]
        let _ = earlier;
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
        #[cfg(unix)]
        crate::process::terminate_tree(&tree, grace).await;
    }

    pub(crate) fn request_group_shutdown(&self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            crate::send_signal(&group, crate::Signal::Term);
        }
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
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDE_CODE_EXECPATH",
                "CLAUDE_CODE_MESSAGING_SOCKET",
                "CLAUDE_CODE_MESSAGING_TOKEN",
                "CLAUDE_CODE_SESSION_ATTENDED",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_SSE_PORT",
                "CLAUDE_EFFORT",
                "CLAUDE_PID",
            ]
        );
        assert_eq!(
            command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "CLAUDE_CODE_ENTRYPOINT")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("zeron"))
        );
        // The user's own configuration is never touched.
        for kept in [
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_GIT_BASH_PATH",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(!removed.contains(&kept), "{kept}");
        }
    }

    /// A child that has exited and been reaped can no longer be traced:
    /// what was snapshotted while it ran is still ended.
    #[tokio::test]
    async fn shutdown_ends_an_earlier_snapshot_after_the_agent_exited() {
        use std::os::unix::process::CommandExt as _;
        let mut orphan = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let orphan_pid = orphan.id() as i32;
        let mut command = Command::new("true");
        configure(&mut command);
        let mut child = Child::new(command.spawn().unwrap());
        child.wait().await.unwrap();
        child
            .shutdown_with(
                vec![crate::process::Tracked::now(orphan_pid)],
                std::time::Duration::from_secs(2),
            )
            .await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while orphan.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "orphan survived");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
