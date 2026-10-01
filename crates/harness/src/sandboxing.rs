//! Agent processes confined by the run's sandbox (docs/sandbox.md).
//!
//! A spawn site asks [`agent_command`] for its `Command` instead of
//! `Command::new(exe)`. With the sandbox off that is exactly
//! `Command::new(exe)`; otherwise the command starts the OS sandbox's
//! wrapper (Seatbelt's `sandbox-exec`, bubblewrap, or Zeron's own Landlock
//! helper), which ends with the agent's executable. Everything the site adds
//! afterwards (arguments, environment, folder, stdio) applies to the agent
//! unchanged. The agent's tools, subagents and MCP servers are its children,
//! so they inherit the confinement.
//!
//! A run that asks for a sandbox this machine can't provide is refused, never
//! run unconfined.

use std::path::Path;

use zeron_proto::{AgentPolicy, HarnessId, McpServer, RunRequest, SandboxMode};
use zeron_sandbox::SandboxSpec;

use crate::HarnessError;
use crate::process::Command;

/// The sandbox modes a harness can offer on this machine, on top of whatever
/// sandbox it has natively: none without an OS backend.
pub fn os_sandboxes() -> Vec<SandboxMode> {
    if zeron_sandbox::best_backend() == zeron_sandbox::Backend::None {
        vec![SandboxMode::Off]
    } else {
        vec![
            SandboxMode::Off,
            SandboxMode::WorkspaceWrite,
            SandboxMode::ReadOnly,
        ]
    }
}

/// Zeron's OS sandbox confines this run (its harness's own sandbox must then
/// stay out of the way: nested Seatbelt profiles are refused).
pub fn confined(request: &RunRequest) -> bool {
    request.policy.sandbox != SandboxMode::Off
}

/// `Command::new(exe)`, confined by `request.policy.sandbox` for `harness`
/// working in `cwd`.
pub fn agent_command(
    harness: HarnessId,
    request: &RunRequest,
    exe: &Path,
    cwd: &Path,
) -> Result<Command, HarnessError> {
    policy_command(harness, &request.policy, request.mcp.as_ref(), exe, cwd)
}

/// [`agent_command`] for spawn sites that hold the policy, not the request.
pub fn policy_command(
    harness: HarnessId,
    policy: &AgentPolicy,
    mcp: Option<&McpServer>,
    exe: &Path,
    cwd: &Path,
) -> Result<Command, HarnessError> {
    if policy.sandbox == SandboxMode::Off {
        return Ok(Command::new(exe));
    }
    let home = crate::executable::home_or_current_dir();
    let cwd = if cwd.as_os_str().is_empty() {
        home.as_path()
    } else {
        cwd
    };
    let mut spec = SandboxSpec::for_agent(harness, policy, cwd, &home);
    // The agent's `zeron mcp` server dials the engine on loopback.
    if let Some(port) = mcp
        .and_then(|mcp| mcp.env.get("ZERON_IPC_PORT"))
        .and_then(|port| port.parse::<u16>().ok())
    {
        spec = spec.with_loopback_port(port);
    }
    let wrapped = zeron_sandbox::wrap(&spec, exe, &[]).map_err(|error| {
        HarnessError::Protocol(format!(
            "This chat asks for a {} sandbox, but it couldn't be applied: {error}. \
             Turn the sandbox off for this chat to run it unconfined.",
            sandbox_label(policy.sandbox)
        ))
    })?;
    if !wrapped.enforcement.notes.is_empty() {
        tracing::info!(
            harness = ?harness,
            backend = ?wrapped.enforcement.backend,
            notes = ?wrapped.enforcement.notes,
            "agent sandbox caveats"
        );
    }
    let mut cmd = Command::new(&wrapped.program);
    cmd.args(&wrapped.args);
    // The wrapper's own variables, then the agent's (e.g. a self-updater
    // switched off so it doesn't write outside its state).
    let agent_env = zeron_sandbox::default_agent_paths(harness, &home).env;
    for (key, value) in wrapped.env.iter().chain(&agent_env) {
        cmd.env(key, value);
    }
    Ok(cmd)
}

fn sandbox_label(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::Off => "no",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::ReadOnly => "read-only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(sandbox: SandboxMode) -> RunRequest {
        let mut request: RunRequest = serde_json::from_value(serde_json::json!({
            "prompt": "x", "model": null, "reasoning": null, "cwd": "/tmp",
            "sandbox": "workspace-write", "resume": null
        }))
        .unwrap();
        request.policy.sandbox = sandbox;
        request
    }

    #[test]
    fn off_is_the_plain_command() {
        let cmd = agent_command(
            HarnessId::ClaudeCode,
            &request(SandboxMode::Off),
            Path::new("/usr/bin/true"),
            Path::new("/tmp"),
        )
        .unwrap();
        assert_eq!(cmd.as_std().get_program(), "/usr/bin/true");
        assert_eq!(cmd.as_std().get_args().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_sandboxed_run_starts_the_wrapper_and_ends_with_the_agent() {
        let ws = tempfile::tempdir().unwrap();
        let mut req = request(SandboxMode::WorkspaceWrite);
        req.cwd = ws.path().display().to_string();
        let cmd = agent_command(
            HarnessId::ClaudeCode,
            &req,
            Path::new("/usr/bin/true"),
            ws.path(),
        )
        .unwrap();
        assert_eq!(cmd.as_std().get_program(), "/usr/bin/sandbox-exec");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(
            args.last().map(|a| a.to_os_string()),
            Some("/usr/bin/true".into())
        );
        assert!(os_sandboxes().contains(&SandboxMode::ReadOnly));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_confined_agent_can_write_its_workspace_and_nothing_else() {
        let ws = tempfile::tempdir().unwrap();
        let ws_path = ws.path().canonicalize().unwrap();
        let outside = dirs_outside();
        let mut req = request(SandboxMode::WorkspaceWrite);
        req.cwd = ws_path.display().to_string();
        let mut cmd =
            agent_command(HarnessId::ClaudeCode, &req, Path::new("/bin/sh"), &ws_path).unwrap();
        cmd.arg("-c").arg(format!(
            "echo ok > '{}/inside' && (echo no > '{}' 2>/dev/null && echo leaked || echo blocked)",
            ws_path.display(),
            outside.display()
        ));
        cmd.current_dir(&ws_path);
        let out = cmd.output().await.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(ws_path.join("inside").exists(), "{stdout}");
        assert!(stdout.contains("blocked"), "{stdout}");
        assert!(!outside.exists());
    }

    #[cfg(target_os = "macos")]
    fn dirs_outside() -> std::path::PathBuf {
        let home = crate::executable::home_or_current_dir();
        home.join(format!(".zeron-sandbox-probe-{}", std::process::id()))
    }
}
