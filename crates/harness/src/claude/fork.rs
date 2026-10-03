use super::*;
use crate::adapter_install::{NpmPin, ensure_installed_shim_controlled, launch_for_entry};
use crate::{NativeForkControls, NativeForkError};
use zeron_proto::{NativeForkAvailability, NativeForkBoundary, NativeForkPoint, NativeForkResult};

const SDK: NpmPin = NpmPin {
    name: "@anthropic-ai/claude-agent-sdk",
    version: "0.3.284",
};
const HELPER: &str = include_str!("fork.mjs");

async fn check_node(
    node: &std::path::Path,
    deadline: tokio::time::Instant,
    interrupt: Option<&crate::CancellationToken>,
) -> Result<(), NativeForkError> {
    use tokio::io::AsyncReadExt;
    let mut child = Command::new(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| NativeForkError::Rejected(e.to_string()))?;
    let mut stdout = child.stdout.take().unwrap().take(4096);
    let stopped = async {
        match interrupt {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let probe = async {
        let mut output = Vec::new();
        let (status, read) = tokio::join!(child.wait(), stdout.read_to_end(&mut output));
        let status = status.map_err(|e| NativeForkError::Rejected(e.to_string()))?;
        read.map_err(|e| NativeForkError::Rejected(e.to_string()))?;
        let major = String::from_utf8_lossy(&output)
            .trim()
            .trim_start_matches('v')
            .split('.')
            .next()
            .and_then(|v| v.parse::<u32>().ok());
        if !status.success() || major.is_none_or(|v| v < 18) {
            return Err(NativeForkError::Rejected(
                "Node 18 or newer is required for native Claude forks".into(),
            ));
        }
        Ok(())
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, probe) => result.unwrap_or_else(|_| Err(NativeForkError::Rejected("Node version check timed out".into()))),
        _ = stopped => Err(NativeForkError::Rejected("Node version check cancelled".into())),
    };
    // Even read-only probes must exit before releasing the provider lease.
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

pub(super) async fn support() -> NativeForkAvailability {
    let Some(node) = crate::executable::find_on_paths("node", Vec::new()) else {
        return NativeForkAvailability::unavailable(
            "Node 18 or newer is required for native Claude forks",
        );
    };
    match check_node(
        &node,
        tokio::time::Instant::now() + Duration::from_secs(5),
        None,
    )
    .await
    {
        Ok(()) => {
            if crate::adapter_install::installed_shim(&SDK, "fork.mjs", HELPER).is_some()
                || crate::adapter_install::find_npm().is_some()
            {
                NativeForkAvailability::available()
            } else {
                NativeForkAvailability::unavailable(
                    "npm is required to prepare the Claude fork helper",
                )
            }
        }
        _ => NativeForkAvailability::unavailable(
            "Node 18 or newer is required for native Claude forks",
        ),
    }
}

pub(super) async fn helper(
    request: serde_json::Value,
    controls: NativeForkControls,
) -> Result<NativeForkResult, NativeForkError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let deadline = tokio::time::Instant::now() + controls.timeout;
    let available = support().await;
    if !available.available {
        return Err(NativeForkError::Rejected(
            available.reason.unwrap_or_default(),
        ));
    }
    // Managed installation is shared with the SDK adapters. The execution lease
    // spans preparation, the helper, and child reaping.
    let shim = ensure_installed_shim_controlled(
        SDK,
        "Claude session fork",
        "fork.mjs",
        HELPER,
        Some((deadline, &controls.interrupt)),
    )
    .await
    .map_err(|e| NativeForkError::Rejected(e.to_string()))?;
    let (node, args) =
        launch_for_entry(&shim).map_err(|e| NativeForkError::Rejected(e.to_string()))?;
    // launch_for_entry may choose Node beside npm rather than the PATH Node.
    check_node(&node, deadline, Some(&controls.interrupt)).await?;
    if controls.interrupt.is_cancelled() {
        return Err(NativeForkError::Rejected(
            "Claude fork cancelled before creation".into(),
        ));
    }
    let cwd = request["dir"].as_str().unwrap_or("").to_string();
    let mut cmd = Command::new(&node);
    crate::compose_child_path(&mut cmd, &node);
    cmd.args(args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| NativeForkError::Rejected(e.to_string()))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let operation = async {
        stdin
            .write_all(request.to_string().as_bytes())
            .await
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        drop(stdin);
        let mut output = Vec::new();
        (&mut stdout)
            .take(65536)
            .read_to_end(&mut output)
            .await
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        let value: serde_json::Value = serde_json::from_slice(&output)
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        if let Some(error) = value["error"].as_str() {
            return Err(if value["indeterminate"] == true {
                NativeForkError::Indeterminate(error.into())
            } else {
                NativeForkError::Rejected(error.into())
            });
        }
        let id = value["ok"]["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                NativeForkError::Indeterminate("Claude helper returned no session ID".into())
            })?;
        Ok(NativeForkResult {
            session_id: id.into(),
            cwd,
        })
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or_else(|_| Err(NativeForkError::Indeterminate("Claude fork timed out".into()))),
        _ = controls.interrupt.cancelled() => Err(NativeForkError::Indeterminate("Claude fork cancelled".into())),
    };
    let _ = child.start_kill();
    let _ = child.wait().await;
    drop(controls.execution_lease);
    result
}

pub(super) async fn fork(
    point: &NativeForkPoint,
    controls: NativeForkControls,
) -> Result<NativeForkResult, NativeForkError> {
    point.validate().map_err(NativeForkError::Rejected)?;
    let NativeForkBoundary::ClaudeMessage { uuid } = &point.boundary else {
        return Err(NativeForkError::Rejected(
            "Expected a Claude transcript UUID".into(),
        ));
    };
    helper(serde_json::json!({"sourceSessionId":point.source_session_id,"dir":point.cwd,"upToMessageId":uuid}), controls).await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_fork_node_probe_checks_version_and_reaps_on_stop() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let node = dir.path().join("fake node");
        for (version, expected) in [("v18.0.0", true), ("v17.9.1", false), ("unknown", false)] {
            std::fs::write(&node, format!("#!/bin/sh\nprintf '%s' {version}\n")).unwrap();
            std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                check_node(
                    &node,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                    None
                )
                .await
                .is_ok(),
                expected
            );
        }
        let pid_file = dir.path().join("pid");
        std::fs::write(
            &node,
            format!(
                "#!/bin/sh\nprintf '%s' $$ > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        for cancel in [false, true] {
            let token = crate::CancellationToken::new();
            if cancel {
                let token = token.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    token.cancel();
                });
            }
            let result = check_node(
                &node,
                tokio::time::Instant::now()
                    + Duration::from_millis(if cancel { 5000 } else { 500 }),
                Some(&token),
            )
            .await;
            assert!(result.is_err());
            let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "Node must be reaped");
        }
    }
}
