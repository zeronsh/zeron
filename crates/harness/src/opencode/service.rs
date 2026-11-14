//! OpenCode's background service: the one server per machine that its TUI
//! and desktop app share (2.x, `opencode service`). Attaching to it instead
//! of booting a private `opencode serve` is what makes a session live in
//! every client at once — a turn Zeron runs shows in the TUI as it streams,
//! and one continued in the TUI streams into Zeron.
//!
//! The service writes where it listens to its state file
//! (`$XDG_STATE_HOME/opencode/service.json`, else
//! `~/.local/state/opencode/service.json`): `{url, password, pid, version}`.
//! Requests authenticate with HTTP Basic, username `opencode` (verified
//! live, 2.3.0). When nothing answers there, `opencode service start` starts
//! it, as the TUI does on launch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::HarnessError;
use crate::process::{Command, Stdio};

/// How long `opencode service start` may take before Zeron gives up on the
/// service for this call (a cold start loads plugins and MCP servers).
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// What the service's state file says about it. The password is never
/// logged or shown; it only becomes the `Authorization` header.
#[derive(Clone, PartialEq, Deserialize)]
pub(super) struct ServiceInfo {
    pub url: String,
    pub password: String,
    #[serde(default)]
    pub version: Option<String>,
}

impl std::fmt::Debug for ServiceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceInfo")
            .field("url", &self.url)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl ServiceInfo {
    /// The `Authorization` header value for every request to the service.
    pub fn auth(&self) -> String {
        use base64::Engine as _;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("opencode:{}", self.password))
        )
    }
}

/// Where the service writes its state file. OpenCode follows the XDG layout
/// on every platform, Windows included.
pub(super) fn state_file() -> Option<PathBuf> {
    state_file_with(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        crate::executable::home_dir(),
    )
}

fn state_file_with(xdg_state: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_state
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".local").join("state")))?;
    Some(base.join("opencode").join("service.json"))
}

/// The service's state file, if there is one that reads.
pub(super) fn read(path: &Path) -> Option<ServiceInfo> {
    let raw = std::fs::read_to_string(path).ok()?;
    let info: ServiceInfo = serde_json::from_str(&raw).ok()?;
    (!info.url.trim().is_empty() && !info.password.is_empty()).then_some(info)
}

/// Whether the service at `info` answers as itself: a 2.x info answer with
/// our credentials (a stale file can name a port something else took).
pub(super) async fn healthy(client: &reqwest::Client, info: &ServiceInfo) -> bool {
    let url = format!("{}/api/info", info.url.trim_end_matches('/'));
    let Ok(response) = client
        .get(url)
        .header(reqwest::header::AUTHORIZATION, info.auth())
        .timeout(Duration::from_secs(3))
        .send()
        .await
    else {
        return false;
    };
    response.status().is_success()
}

/// The running service, started first when nothing answers. `None` when it
/// can't be had (an OpenCode without a service, or one that won't start):
/// the caller boots a private server instead, as before.
pub(super) async fn ensure(
    exe: &Path,
    client: &reqwest::Client,
    startup: Duration,
) -> Option<ServiceInfo> {
    let path = state_file()?;
    if let Some(info) = read(&path)
        && healthy(client, &info).await
    {
        return Some(info);
    }
    if let Err(error) = start(exe).await {
        tracing::warn!(
            target: "zeron_harness::opencode",
            %error,
            "opencode service did not start; using a private server"
        );
        return None;
    }
    // `service start` returns once the service is launched; it answers a
    // moment later.
    let deadline = tokio::time::Instant::now() + startup;
    loop {
        if let Some(info) = read(&path)
            && healthy(client, &info).await
        {
            return Some(info);
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                target: "zeron_harness::opencode",
                "opencode service started but did not answer; using a private server"
            );
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// `opencode service start`, bounded. Its output is only read for the error.
async fn start(exe: &Path) -> Result<(), HarnessError> {
    let mut cmd = Command::new(exe);
    cmd.arg("service")
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::compose_child_path(&mut cmd, exe);
    if let Some(home) = crate::executable::home_dir().filter(|h| h.is_dir()) {
        cmd.current_dir(home);
    }
    let output = tokio::time::timeout(START_TIMEOUT, cmd.output())
        .await
        .map_err(|_| HarnessError::Protocol("opencode service start timed out".into()))?
        .map_err(|e| HarnessError::Protocol(format!("opencode service start: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(HarnessError::Protocol(format!(
        "opencode service start exited with {}: {}",
        output.status,
        stderr.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_file_follows_xdg_and_falls_back_to_home() {
        let home = PathBuf::from("/home/someone");
        assert_eq!(
            state_file_with(None, Some(home.clone())),
            Some(home.join(".local/state/opencode/service.json"))
        );
        assert_eq!(
            state_file_with(Some(PathBuf::from("/xdg/state")), Some(home.clone())),
            Some(PathBuf::from("/xdg/state/opencode/service.json"))
        );
        // (A relative XDG path is ignored, as the spec says.)
        assert_eq!(
            state_file_with(Some(PathBuf::from("rel")), Some(home.clone())),
            Some(home.join(".local/state/opencode/service.json"))
        );
        assert_eq!(state_file_with(None, None), None);
    }

    #[test]
    fn a_state_file_reads_and_never_prints_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.json");
        std::fs::write(
            &path,
            r#"{"id":"x","version":"2.3.0","url":"http://127.0.0.1:49374","pid":1,"password":"secret"}"#,
        )
        .unwrap();
        let info = read(&path).unwrap();
        assert_eq!(info.url, "http://127.0.0.1:49374");
        assert_eq!(info.auth(), "Basic b3BlbmNvZGU6c2VjcmV0");
        assert!(!format!("{info:?}").contains("secret"));
        // (No password, no service to attach to.)
        std::fs::write(&path, r#"{"url":"http://127.0.0.1:1","password":""}"#).unwrap();
        assert!(read(&path).is_none());
    }
}
