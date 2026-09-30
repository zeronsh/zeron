//! `zeron daemon handoff` — replace the running engine, in place, with a newer
//! binary, without stopping its agents or terminals (see
//! `zeron_engine::handoff`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use zeron_rpc::{RpcError, methods};

/// How long to wait for the handoff to complete or be refused.
const HANDOFF_DEADLINE: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// What asking an engine to hand itself over came to.
pub enum Outcome {
    /// The new binary took over; agents and terminals kept running.
    HandedOff,
    /// Work in flight cannot be carried across right now. Nothing was touched:
    /// an engine with a newer binary installed retries by itself.
    Deferred(String),
    /// The handoff failed; the engine is unchanged.
    Failed(String),
    /// This engine predates live handoff, or is a headed app.
    Unsupported,
    /// Nothing listens on the IPC port.
    NoEngine,
}

/// Ask the engine on `ipc_port` to hand itself to `exe`, then wait until the
/// successor answers (or the engine reports why not).
pub async fn request(ipc_port: u16, exe: &Path) -> Outcome {
    let exe = match std::fs::canonicalize(exe) {
        Ok(exe) => exe,
        Err(error) => return Outcome::Failed(format!("{}: {error}", exe.display())),
    };
    let target_version = binary_version(&exe);
    let url = format!("ws://127.0.0.1:{ipc_port}");
    let Ok(client) = zeron_rpc::connect_ws(&url).await else {
        return Outcome::NoEngine;
    };
    match client
        .call(methods::HANDOFF_ENGINE, serde_json::json!({ "exe": exe }))
        .await
    {
        Ok(_) => {}
        Err(RpcError::UnknownMethod(_)) => return Outcome::Unsupported,
        Err(err) => return Outcome::Failed(format!("HandoffEngine failed: {err}")),
    }

    // The engine reports "running" from the moment it accepts the request. A
    // refusal turns it "failed" (with the reason); success replaces the
    // process, so the next answer comes from the successor, which is "idle".
    let deadline = Instant::now() + HANDOFF_DEADLINE;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        if Instant::now() >= deadline {
            return Outcome::Failed("timed out waiting for the handoff to complete".into());
        }
        // A fresh dial each time: the connection dies with the old process
        // image, and the socket is served again by the successor.
        let Ok(client) = zeron_rpc::connect_ws(&url).await else {
            continue; // the gap while the process image is replaced
        };
        match client
            .call(methods::HANDOFF_STATUS, serde_json::json!({}))
            .await
        {
            Ok(status) => match status.get("state").and_then(|state| state.as_str()) {
                Some("failed") => {
                    let message = status
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("unknown reason")
                        .to_string();
                    return if status.get("busy").and_then(|busy| busy.as_bool()) == Some(true) {
                        Outcome::Deferred(message)
                    } else {
                        Outcome::Failed(message)
                    };
                }
                Some("idle") => {
                    // After a failed adoption the engine hands BACK to the old
                    // binary, which also reports "idle": the version tells
                    // the two apart.
                    let running = status.get("version").and_then(|version| version.as_str());
                    if let (Some(wanted), Some(running)) = (target_version.as_deref(), running)
                        && wanted != running
                    {
                        return Outcome::Failed(format!(
                            "the engine is running {running}, not {wanted}: the new binary could not take over, so the engine handed back"
                        ));
                    }
                    return Outcome::HandedOff;
                }
                _ => {}
            },
            Err(RpcError::Closed) => {}
            Err(err) => return Outcome::Failed(format!("HandoffStatus failed: {err}")),
        }
    }
}

/// `zeron daemon handoff`: request a handoff and turn the outcome into output
/// and an exit status.
pub async fn handoff(ipc_port: u16, exe: Option<PathBuf>) -> anyhow::Result<()> {
    let exe = match exe {
        Some(exe) => exe,
        None => std::env::current_exe().context("resolving the zeron executable path")?,
    };
    println!("handoff requested; waiting for the new engine…");
    match request(ipc_port, &exe).await {
        Outcome::HandedOff => {
            println!("handed off; agents and terminals kept running.");
            Ok(())
        }
        Outcome::Deferred(reason) => bail!("the engine could not hand off yet: {reason}"),
        Outcome::Failed(reason) => bail!("the engine did not hand off: {reason}"),
        Outcome::Unsupported => bail!(
            "this engine predates live handoff (or is a headed app); restart it with `zeron daemon restart`"
        ),
        Outcome::NoEngine => {
            bail!("no engine listening on 127.0.0.1:{ipc_port} — is `zeron headless` running?")
        }
    }
}

/// The version `exe --version` reports (`zeron 0.2.99` → `0.2.99`).
fn binary_version(exe: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}
