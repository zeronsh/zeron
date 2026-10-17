use super::*;
use crate::adapter_install::{NpmPin, ensure_installed_shim_controlled, launch_for_entry};
use crate::{NativeForkControls, NativeForkError};
use zeron_proto::{NativeForkAvailability, NativeForkBoundary, NativeForkPoint, NativeForkResult};

const SDK: NpmPin = NpmPin {
    name: "@earendil-works/pi-coding-agent",
    version: "0.85.1",
};
const HELPER: &str = include_str!("fork.mjs");
pub(super) const NOTICE: &str = "zeron-native-fork-v1:";

pub(super) fn configure(cmd: &mut Command) -> Result<crate::scratch::ScratchDir, HarnessError> {
    let scratch = crate::scratch::ScratchDir::new("pi-native-point")?;
    let extension = scratch.path().join("native-point.mjs");
    std::fs::write(&extension, include_str!("native-point.mjs"))?;
    cmd.arg("--extension").arg(extension);
    Ok(scratch)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn executable(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("pi version with spaces");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[tokio::test]
    async fn native_fork_unknown_pi_contract_is_unavailable_without_creation() {
        let dir = tempfile::tempdir().unwrap();
        let h = PiHarness::new().with_executable(executable(dir.path(), "echo 0.99.0"));
        let result = h.fork_support().await;
        assert!(!result.available);
        assert!(result.reason.unwrap().contains("verified Pi 0.85.1"));
    }
    #[tokio::test]
    async fn native_fork_probe_has_timeout_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "sleep 30");
        let token = crate::CancellationToken::new();
        let result = version(
            &path,
            tokio::time::Instant::now() + Duration::from_millis(50),
            &token,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
        token.cancel();
        let result = version(
            &path,
            tokio::time::Instant::now() + Duration::from_secs(1),
            &token,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }
    #[tokio::test]
    async fn native_fork_lost_response_after_creation_is_indeterminate_and_reaps_before_releasing_lease()
     {
        for cancel in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut cmd = Command::new("sh");
            crate::process::owned::configure(&mut cmd);
            cmd.args([
                "-c",
                "cat > request.json; echo $$ > helper.pid; touch provider-created; sleep 30",
            ])
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
            let store = sessions::Store::new(
                Some(dir.path().join("index")),
                Some(dir.path().join("agent")),
            );
            let request =
                json!({"mode":"fork","sourceSessionId":"source","dir":dir.path(),"entryId":"a1"});
            let interrupt = crate::CancellationToken::new();
            let lock = std::sync::Arc::new(tokio::sync::RwLock::new(()));
            let controls = NativeForkControls {
                execution_lease: Some(std::sync::Arc::new(lock.clone().read_owned().await)),
                interrupt: interrupt.clone(),
                timeout: Duration::from_millis(500),
                source_idle: true,
            };
            let deadline = tokio::time::Instant::now() + controls.timeout;
            let (result, ()) = tokio::join!(
                invoke(&mut cmd, &request, &store, controls, deadline),
                async {
                    tokio::time::timeout(Duration::from_secs(2), async {
                        while !dir.path().join("provider-created").exists() {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                    .await
                    .unwrap();
                    if cancel {
                        interrupt.cancel();
                    }
                }
            );
            assert!(matches!(result, Err(NativeForkError::Indeterminate(_))));
            assert!(dir.path().join("provider-created").exists());
            assert!(
                !dir.path().join("index").exists(),
                "unconfirmed child cannot be remembered"
            );
            let pid = std::fs::read_to_string(dir.path().join("helper.pid")).unwrap();
            assert!(
                !std::process::Command::new("kill")
                    .args(["-0", pid.trim()])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "helper was reaped"
            );
            let _writer =
                tokio::time::timeout(Duration::from_millis(100), lock.clone().write_owned())
                    .await
                    .unwrap();
        }
    }
}

async fn version(
    exe: &Path,
    deadline: tokio::time::Instant,
    interrupt: &crate::CancellationToken,
) -> Result<semver::Version, NativeForkError> {
    use tokio::io::AsyncReadExt;
    let mut cmd = Command::new(exe);
    crate::process::owned::configure(&mut cmd);
    crate::compose_child_path(&mut cmd, exe);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = Child::new(cmd.spawn().map_err(reject)?);
    let mut stdout = child.stdout.take().unwrap().take(4096);
    let read = async {
        let mut bytes = Vec::new();
        let (status, output) = tokio::join!(child.wait(), stdout.read_to_end(&mut bytes));
        output.map_err(reject)?;
        if !status.map_err(reject)?.success() {
            return Err(NativeForkError::Rejected("Version probe failed".into()));
        }
        semver::Version::parse(
            String::from_utf8_lossy(&bytes)
                .trim()
                .trim_start_matches('v'),
        )
        .map_err(reject)
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, read) => result.unwrap_or_else(|_| Err(NativeForkError::Rejected("Pi fork version check timed out".into()))),
        _ = interrupt.cancelled() => Err(NativeForkError::Rejected("Pi fork version check cancelled".into())),
    };
    child.shutdown(Duration::from_millis(100)).await;
    result
}

fn reject(error: impl std::fmt::Display) -> NativeForkError {
    NativeForkError::Rejected(error.to_string())
}

impl PiHarness {
    async fn fork_prerequisites(
        &self,
        deadline: tokio::time::Instant,
        interrupt: &crate::CancellationToken,
    ) -> Result<(), NativeForkError> {
        let exe = self.resolve_executable().map_err(reject)?;
        // Only the contract whose live entry identity and storage were tested.
        if version(&exe, deadline, interrupt).await? != semver::Version::new(0, 85, 1) {
            return Err(reject(
                "Native Pi forks require the verified Pi 0.85.1 contract",
            ));
        }
        let node = crate::executable::find_on_paths("node", Vec::new())
            .ok_or_else(|| reject("Node 22.19 or newer is required for native Pi forks"))?;
        if version(&node, deadline, interrupt).await? < semver::Version::new(22, 19, 0) {
            return Err(reject(
                "Node 22.19 or newer is required for native Pi forks",
            ));
        }
        if crate::adapter_install::installed_shim(&SDK, "pi-fork.mjs", HELPER).is_none()
            && crate::adapter_install::find_npm().is_none()
        {
            return Err(reject("npm is required to prepare the Pi fork helper"));
        }
        Ok(())
    }

    pub(super) async fn fork_support(&self) -> NativeForkAvailability {
        match self
            .fork_prerequisites(
                tokio::time::Instant::now() + Duration::from_secs(5),
                &crate::CancellationToken::new(),
            )
            .await
        {
            Ok(()) => NativeForkAvailability::available(),
            Err(error) => NativeForkAvailability::unavailable(error.to_string()),
        }
    }

    pub(super) async fn fork_helper(
        &self,
        session: &str,
        cwd: &Path,
        entry: Option<&str>,
        controls: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        let deadline = tokio::time::Instant::now() + controls.timeout;
        self.fork_prerequisites(deadline, &controls.interrupt)
            .await?;
        let store = sessions::Store::new(self.session_store.clone(), self.agent_dir.clone());
        // Require a real, validated native file; empty-session recovery is not a fork.
        let source_file = store.resolve(session, cwd).map_err(reject)?;
        let shim = ensure_installed_shim_controlled(
            SDK,
            "Pi session fork",
            "pi-fork.mjs",
            HELPER,
            Some((deadline, &controls.interrupt)),
        )
        .await
        .map_err(reject)?;
        let (node, args) = launch_for_entry(&shim).map_err(reject)?;
        if version(&node, deadline, &controls.interrupt).await? < semver::Version::new(22, 19, 0) {
            return Err(reject(
                "Node 22.19 or newer is required for native Pi forks",
            ));
        }
        if controls.interrupt.is_cancelled() {
            return Err(reject("Pi fork cancelled before creation"));
        }
        let request = json!({"mode":if entry.is_some() {"fork"} else {"check"},"sourceSessionId":session,"sourceFile":source_file,"dir":cwd,"entryId":entry});
        let mut cmd = Command::new(&node);
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, &node);
        cmd.env(
            "PI_CODING_AGENT_DIR",
            sessions::agent_dir(self.agent_dir.clone()),
        );
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        invoke(&mut cmd, &request, &store, controls, deadline).await
    }

    pub(super) async fn create_native_fork(
        &self,
        point: &NativeForkPoint,
        controls: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        point.validate().map_err(reject)?;
        if point.harness != HarnessId::Pi {
            return Err(reject("Expected the Pi harness"));
        }
        let NativeForkBoundary::PiEntry { entry_id } = &point.boundary else {
            return Err(reject("Expected a native Pi entry"));
        };
        self.fork_helper(
            &point.source_session_id,
            Path::new(&point.cwd),
            Some(entry_id),
            controls,
        )
        .await
    }
}

// Kept separate so the administrative process lifecycle can be exercised without
// installing a package or touching a real provider session.
async fn invoke(
    cmd: &mut Command,
    request: &Value,
    store: &sessions::Store,
    controls: NativeForkControls,
    deadline: tokio::time::Instant,
) -> Result<NativeForkResult, NativeForkError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let session = request["sourceSessionId"].as_str().unwrap();
    let mut child = Child::new(cmd.spawn().map_err(reject)?);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let ambiguous = |message: String| {
        if request["mode"] == "fork" {
            NativeForkError::Indeterminate(message)
        } else {
            reject(message)
        }
    };
    let operation = async {
        stdin
            .write_all(request.to_string().as_bytes())
            .await
            .map_err(|e| ambiguous(e.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| ambiguous(e.to_string()))?;
        drop(stdin);
        let mut output = Vec::new();
        (&mut stdout)
            .take(65537)
            .read_to_end(&mut output)
            .await
            .map_err(|e| ambiguous(e.to_string()))?;
        if output.len() > 65536 {
            return Err(ambiguous("Pi helper response exceeded its limit".into()));
        }
        let value: Value = serde_json::from_slice(&output).map_err(|e| ambiguous(e.to_string()))?;
        if let Some(error) = value["error"].as_str() {
            return Err(if value["indeterminate"] == true {
                ambiguous(error.into())
            } else {
                reject(error)
            });
        }
        if !child
            .wait()
            .await
            .map_err(|e| ambiguous(e.to_string()))?
            .success()
        {
            return Err(ambiguous("Pi fork helper exited unsuccessfully".into()));
        }
        let id = value["ok"]["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ambiguous("Pi helper omitted the session ID".into()))?;
        let file = value["ok"]["sessionFile"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ambiguous("Pi helper omitted the session file".into()))?;
        if (request["mode"] == "fork" && id == session)
            || (request["mode"] == "check" && id != session)
        {
            return Err(ambiguous("Pi helper returned an unexpected session".into()));
        }
        store
            .remember_fork(id, Path::new(file))
            .map_err(|e| ambiguous(e.to_string()))?;
        Ok(NativeForkResult {
            session_id: id.into(),
            cwd: request["dir"].as_str().unwrap().into(),
        })
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or_else(|_| Err(ambiguous("Pi fork timed out".into()))),
        _ = controls.interrupt.cancelled() => Err(ambiguous("Pi fork cancelled".into())),
    };
    child.shutdown(Duration::from_millis(100)).await;
    drop(controls.execution_lease);
    result
}
