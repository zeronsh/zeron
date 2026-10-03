//! Owned agent processes. Non-Windows builds retain Tokio's command and child.
//! Windows uses native creation-time Job Object membership, never an implicit shell.
#[cfg(not(windows))]
pub use std::process::Stdio;
#[cfg(not(windows))]
pub use tokio::process::{Child, ChildStdin, ChildStdout, Command};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[cfg(unix)]
pub(crate) fn signal_target(child: &Child) -> Option<i32> {
    let pid = child.id()? as i32;
    // ACP children lead a private group; other harnesses retain pid signaling.
    // SAFETY: getpgid only inspects the owned, unreaped child.
    Some(if unsafe { libc::getpgid(pid) } == pid {
        -pid
    } else {
        pid
    })
}
#[cfg(windows)]
pub(crate) fn signal_target(child: &Child) -> Option<std::sync::Arc<crate::windows_process::Job>> {
    Some(child.job.clone())
}

/// Every live descendant of `root` — children, grandchildren, … — from one
/// process-table snapshot. Agent CLIs start each shell in its OWN process
/// group (Claude's background Bash, Codex's exec sessions), so neither a
/// signal to the agent nor to its group reaches them, and a background
/// shell outlives its agent (verified live, Claude 2.1.286: its own task
/// stop and SIGTERM both leave the shell running). Snapshot while the agent
/// is alive: once it exits, its orphans are reparented and untraceable.
#[cfg(unix)]
pub(crate) async fn descendants(root: u32) -> Vec<i32> {
    let Ok(out) = tokio::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    else {
        return Vec::new();
    };
    let table: Vec<(i32, i32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(str::parse::<i32>);
            Some((cols.next()?.ok()?, cols.next()?.ok()?))
        })
        .collect();
    let mut found = Vec::new();
    let mut frontier = vec![root as i32];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in &table {
            if ppid == parent && pid > 1 && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

/// Terminate `pids` and the process groups they lead: SIGTERM, then SIGKILL
/// whatever is still running after `grace`.
#[cfg(unix)]
pub(crate) async fn terminate_tree(pids: &[i32], grace: std::time::Duration) {
    // A group leader's whole group (its own children included); anything
    // else by pid.
    let targets: Vec<i32> = pids
        .iter()
        .map(|&pid| {
            // SAFETY: getpgid only inspects; a vanished pid returns -1.
            if unsafe { libc::getpgid(pid) } == pid {
                -pid
            } else {
                pid
            }
        })
        .collect();
    let alive = |target: i32| unsafe { libc::kill(target, 0) } == 0;
    for &target in &targets {
        crate::send_signal(&target, crate::Signal::Term);
    }
    let deadline = tokio::time::Instant::now() + grace;
    while targets.iter().any(|&t| alive(t)) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for &target in &targets {
        if alive(target) {
            crate::send_signal(&target, crate::Signal::Kill);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A grandchild in its own process group (how agent CLIs start shells)
    /// survives its parent's death; the descendant snapshot still finds it
    /// and `terminate_tree` ends it.
    #[tokio::test]
    async fn tree_teardown_reaches_shells_in_their_own_process_group() {
        let mut parent = tokio::process::Command::new("sh")
            .args([
                "-c",
                "python3 -c 'import os, time; os.setpgid(0, 0); time.sleep(60)' & sleep 60",
            ])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let root = parent.id().unwrap();
        let mut tree = Vec::new();
        for _ in 0..100 {
            tree = descendants(root).await;
            if tree.len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let own_group: Vec<i32> = tree
            .iter()
            .copied()
            .filter(|&pid| unsafe { libc::getpgid(pid) } == pid)
            .collect();
        assert_eq!(own_group.len(), 1, "{tree:?}");
        // The parent dies hard (an unresponsive agent's SIGKILL): the shell
        // is orphaned, not killed.
        parent.kill().await.unwrap();
        assert_eq!(unsafe { libc::kill(own_group[0], 0) }, 0, "orphan survives");
        terminate_tree(&tree, std::time::Duration::from_secs(2)).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_ne!(unsafe { libc::kill(own_group[0], 0) }, 0, "orphan ended");
    }

    #[tokio::test]
    async fn unix_launch_retains_tokio_types_and_output() {
        let mut command: tokio::process::Command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--list")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let child: tokio::process::Child = command.spawn().unwrap();
        let result = child.wait_with_output().await.unwrap();
        assert!(result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("unix_launch_retains_tokio_types_and_output")
        );
        let result = command.output().await.unwrap();
        assert!(result.status.success());
        assert!(!result.stdout.is_empty());
    }
}

pub(crate) mod owned;
