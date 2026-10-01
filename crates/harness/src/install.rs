//! Explicit, user-requested CLI installation. Catalog probes never call this module's executor.
use std::{path::PathBuf, time::Duration};

use tokio::io::{AsyncRead, AsyncReadExt};
use zeron_proto::HarnessId;

use crate::{
    CancellationToken, Harness, HarnessError, StderrTail,
    process::{Command, Stdio},
};

const DEADLINE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Unix,
    Mac,
    Windows,
}
impl Platform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Unix
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Archive,
    Shell(&'static str, &'static str),
    Npm(&'static str, bool),
    PowerShell(&'static str),
    Brew,
}

// Commands verified against these vendor pages with curl -fsSL on 2026-09-21.
// https://code.claude.com/docs/en/setup
// https://github.com/openai/codex/blob/main/README.md
// https://cursor.com/docs/cli/installation
// https://opencode.ai/docs/
// https://pi.dev/docs/latest
// https://docs.x.ai/developers/release-notes and https://docs.x.ai/build/enterprise
// https://hermes-agent.nousresearch.com/docs/getting-started/installation
// https://cli.devin.ai/
fn methods(id: HarnessId, platform: Platform) -> Vec<Method> {
    use HarnessId::*;
    use Method::*;
    let windows = platform == Platform::Windows;
    match id {
        Mock => vec![],
        Antigravity => vec![Archive],
        ClaudeCode if windows => vec![PowerShell("irm https://claude.ai/install.ps1 | iex")],
        ClaudeCode => vec![Shell(
            "curl -fsSL https://claude.ai/install.sh | bash",
            "bash",
        )],
        Codex if windows => vec![
            PowerShell("irm https://chatgpt.com/codex/install.ps1 | iex"),
            Npm("@openai/codex", false),
        ],
        Codex => vec![
            Shell("curl -fsSL https://chatgpt.com/codex/install.sh | sh", "sh"),
            Npm("@openai/codex", false),
        ],
        Cursor if windows => vec![PowerShell(
            "irm 'https://cursor.com/install?win32=true' | iex",
        )],
        Cursor => vec![Shell("curl https://cursor.com/install -fsS | bash", "bash")],
        Opencode if windows => vec![Npm("@opencode/cli", false)],
        Opencode => vec![
            Shell("curl -fsSL https://opencode.ai/install | bash", "bash"),
            Npm("@opencode/cli", false),
        ],
        Pi if windows => vec![Npm("@earendil-works/pi-coding-agent", true)],
        Pi => vec![
            Shell("curl -fsSL https://pi.dev/install.sh | sh", "sh"),
            Npm("@earendil-works/pi-coding-agent", true),
        ],
        Grok if windows => vec![Npm("@xai-official/grok", false)],
        Grok => vec![
            Shell("curl -fsSL https://x.ai/cli/install.sh | bash", "bash"),
            Npm("@xai-official/grok", false),
        ],
        Hermes if windows => vec![PowerShell(
            "irm https://hermes-agent.nousresearch.com/install.ps1 | iex",
        )],
        Hermes => vec![Shell(
            "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
            "bash",
        )],
        Devin if windows => vec![PowerShell(
            "irm https://static.devin.ai/cli/setup.ps1 | iex",
        )],
        Devin if platform == Platform::Mac => vec![
            Brew,
            Shell("curl -fsSL https://cli.devin.ai/install.sh | bash", "bash"),
        ],
        Devin => vec![Shell(
            "curl -fsSL https://cli.devin.ai/install.sh | bash",
            "bash",
        )],
    }
}

fn resolve(name: &str) -> Option<PathBuf> {
    if name == "npm" {
        crate::adapter_install::find_npm()
    } else {
        crate::executable::find_on_paths(name, vec![])
    }
}

fn available(
    method: Method,
    platform: Platform,
    has: &impl Fn(&str) -> bool,
    archive: bool,
) -> bool {
    match method {
        Method::Archive => archive,
        Method::Shell(_, shell) => has("sh") && has("curl") && has(shell),
        Method::Npm(..) => has("npm") && (platform == Platform::Windows || has("sh")),
        Method::PowerShell(_) => has("powershell"),
        Method::Brew => has("brew") && has("sh"),
    }
}

fn selected(id: HarnessId) -> Option<Method> {
    let platform = Platform::current();
    methods(id, platform).into_iter().find(|method| {
        available(
            *method,
            platform,
            &|name| resolve(name).is_some(),
            crate::acp::can_install(id),
        )
    })
}

pub fn can_install(id: HarnessId) -> bool {
    selected(id).is_some()
}

/// Portable documented fallback for viewers of a device without installer prerequisites.
pub fn manual_command(id: HarnessId) -> Option<&'static str> {
    use HarnessId::*;
    Some(match id {
        ClaudeCode => "curl -fsSL https://claude.ai/install.sh | bash",
        Codex => "npm install -g @openai/codex",
        Cursor => "curl https://cursor.com/install -fsS | bash",
        Opencode => "npm install -g @opencode/cli",
        Pi => "npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
        Grok => "npm install -g @xai-official/grok",
        Hermes => "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
        Devin => "curl -fsSL https://cli.devin.ai/install.sh | bash",
        Antigravity | Mock => return None,
    })
}

fn cli_and_dir(id: HarnessId) -> (&'static str, &'static str) {
    use HarnessId::*;
    match id {
        ClaudeCode => ("claude", "~/.local/bin"),
        Codex => ("codex", "~/.local/bin or the npm global bin"),
        Cursor => ("cursor-agent", "~/.local/bin or ~/.cursor/bin"),
        Opencode => ("opencode", "~/.opencode/bin or the npm global bin"),
        Pi => ("pi", "the npm global bin"),
        Grok => ("grok", "~/.grok/bin or the npm global bin"),
        Hermes => ("hermes", "~/.local/bin or ~/.hermes/bin"),
        Devin => ("devin", "~/.local/bin"),
        Antigravity => ("agy_acp_server", "~/.zeron/adapters"),
        Mock => ("mock", "PATH"),
    }
}

pub fn installed(id: HarnessId) -> bool {
    use HarnessId::*;
    match id {
        ClaudeCode => crate::ClaudeHarness::new().installed(),
        Codex => crate::CodexHarness::new().installed(),
        Cursor => crate::CursorHarness::new().installed(),
        Opencode => crate::OpencodeHarness::new().installed(),
        Pi => crate::PiHarness::new().installed(),
        Grok => crate::AcpHarness::grok().installed(),
        Hermes => crate::AcpHarness::hermes().installed(),
        Devin => crate::AcpHarness::devin().installed(),
        Antigravity => crate::AcpHarness::antigravity().installed(),
        Mock => false,
    }
}

fn invalidate_versions(id: HarnessId) {
    let (cli, _) = cli_and_dir(id);
    if id == HarnessId::Cursor {
        crate::executable::invalidate_versions(&[cli, "agent"]);
    } else {
        crate::executable::invalidate_versions(&[cli]);
    }
}

fn post_install(id: HarnessId) -> Result<(), HarnessError> {
    if installed(id) {
        return Ok(());
    }
    let (cli, dir) = cli_and_dir(id);
    Err(HarnessError::Install(format!(
        "installer finished but `{cli}` was not found on PATH; open a new terminal or add {dir} to PATH"
    )))
}

fn configure(command: &mut Command) {
    crate::acp::child::configure(command);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("ZERON_") {
            command.env_remove(key);
        }
    }
    // Preserve explicit PATH additions and include the user's login-shell toolchains.
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(path) = crate::shell_env::login_shell_path() {
        paths.extend(std::env::split_paths(path));
    }
    // npm can be found in a managed toolchain even when shell startup was disabled.
    if let Some(npm) =
        crate::adapter_install::find_npm().and_then(|p| p.parent().map(PathBuf::from))
    {
        paths.push(npm);
    }
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    command
        .env("CI", "1")
        .env("NONINTERACTIVE", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
}

fn command(method: Method) -> Result<Command, HarnessError> {
    let missing = || HarnessError::Install("installer prerequisite is no longer available".into());
    let mut command = match method {
        Method::PowerShell(script) => {
            let mut cmd = Command::new(resolve("powershell").ok_or_else(missing)?);
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ]);
            cmd
        }
        Method::Npm(package, ignore_scripts) if cfg!(windows) => {
            let mut cmd = Command::new(resolve("npm").ok_or_else(missing)?);
            cmd.args(["install", "-g"]);
            if ignore_scripts {
                cmd.arg("--ignore-scripts");
            }
            cmd.arg(package);
            cmd
        }
        Method::Shell(script, _) => shell_command(script)?,
        Method::Npm(package, ignore_scripts) => shell_command(&format!(
            "npm install -g {}{package}",
            if ignore_scripts {
                "--ignore-scripts "
            } else {
                ""
            }
        ))?,
        Method::Brew => shell_command("brew install --cask devin-cli")?,
        Method::Archive => unreachable!("archives have a separate executor"),
    };
    configure(&mut command);
    Ok(command)
}

fn shell_command(script: &str) -> Result<Command, HarnessError> {
    let mut cmd =
        Command::new(resolve("sh").ok_or_else(|| HarnessError::Install("sh not found".into()))?);
    cmd.args(["-c", script]);
    Ok(cmd)
}

/// Only the explicit Install RPC may call this; dropping the future also kills its process group.
pub async fn install_harness(id: HarnessId, cancel: CancellationToken) -> Result<(), HarnessError> {
    let method = selected(id).ok_or_else(|| {
        HarnessError::Install(
            "No supported installer or required tools available on this device".into(),
        )
    })?;
    let result = if method == Method::Archive {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(HarnessError::Install("installation cancelled".into())),
            result = tokio::time::timeout(DEADLINE, crate::acp::install_harness(id)) => result.unwrap_or_else(|_| Err(HarnessError::Install("installation timed out after 15 minutes".into()))),
        }
    } else {
        run(command(method)?, cancel, DEADLINE).await
    };
    invalidate_versions(id);
    result?;
    post_install(id)
}

/// Injection seam for integration tests; unavailable in production builds.
#[cfg(feature = "installer-fixture")]
pub async fn install_with_command(
    id: HarnessId,
    script: &str,
    cancel: CancellationToken,
) -> Result<(), HarnessError> {
    let mut cmd = shell_command(script)?;
    configure(&mut cmd);
    let result = run(cmd, cancel, DEADLINE).await;
    invalidate_versions(id);
    result?;
    post_install(id)
}

async fn capture(mut pipe: impl AsyncRead + Unpin, tail: StderrTail) {
    // Fixed-size chunks also bound memory for installers emitting an enormous line.
    // Retain a partial line until newline, and discard overflow until that newline so
    // a credential split across reads can never leak as an unmarked continuation.
    let mut chunk = [0; 1024];
    let mut line = Vec::new();
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for byte in &chunk[..n] {
                    if *byte == b'\n' || *byte == b'\r' {
                        tail.push(&crate::redact_secrets(&String::from_utf8_lossy(&line)));
                        line.clear();
                    } else if line.len() < 4096 {
                        line.push(*byte);
                    }
                }
            }
        }
    }
    tail.push(&crate::redact_secrets(&String::from_utf8_lossy(&line)));
}

async fn run(
    mut command: Command,
    cancel: CancellationToken,
    deadline: Duration,
) -> Result<(), HarnessError> {
    if cancel.is_cancelled() {
        return Err(HarnessError::Install("installation cancelled".into()));
    }
    let mut child = crate::acp::child::Child::new(
        command
            .spawn()
            .map_err(|e| HarnessError::Install(e.to_string()))?,
    );
    let tail = StderrTail::default();
    let stdout = capture(child.stdout.take().expect("piped stdout"), tail.clone());
    let stderr = capture(child.stderr.take().expect("piped stderr"), tail.clone());
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("installation cancelled".to_string()),
        _ = tokio::time::sleep(deadline) => Err("installation timed out after 15 minutes".to_string()),
        result = async { let (status, (), ()) = tokio::join!(child.wait(), stdout, stderr); status } =>
            result.map_err(|e| e.to_string()).and_then(|status| if status.success() { Ok(()) } else { Err(format!("installer exited with {status}")) }),
    };
    child.shutdown(Duration::from_millis(100)).await;
    result.map_err(|reason| {
        HarnessError::Install(format!(
            "{reason}{}",
            tail.snapshot()
                .map(|s| format!("\n{s}"))
                .unwrap_or_default()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: [HarnessId; 10] = [
        HarnessId::ClaudeCode,
        HarnessId::Codex,
        HarnessId::Cursor,
        HarnessId::Opencode,
        HarnessId::Pi,
        HarnessId::Grok,
        HarnessId::Hermes,
        HarnessId::Devin,
        HarnessId::Antigravity,
        HarnessId::Mock,
    ];

    #[test]
    fn platform_and_prerequisite_matrix() {
        for platform in [Platform::Unix, Platform::Mac, Platform::Windows] {
            for id in IDS {
                let list = methods(id, platform);
                assert_eq!(list.is_empty(), id == HarnessId::Mock);
                for method in list {
                    assert!(available(method, platform, &|_| true, true));
                    assert!(!available(method, platform, &|_| false, false));
                    match method {
                        Method::Shell(_, shell) => {
                            assert_ne!(platform, Platform::Windows);
                            for missing in ["sh", "curl", shell] {
                                assert!(!available(method, platform, &|p| p != missing, true));
                            }
                        }
                        Method::PowerShell(_) => assert_eq!(platform, Platform::Windows),
                        Method::Brew => assert_eq!(platform, Platform::Mac),
                        _ => {}
                    }
                }
            }
        }
        assert_eq!(methods(HarnessId::Devin, Platform::Mac)[0], Method::Brew);
        assert!(matches!(
            methods(HarnessId::Codex, Platform::Unix)[0],
            Method::Shell(_, "sh")
        ));
        assert!(matches!(
            methods(HarnessId::Pi, Platform::Windows)[0],
            Method::Npm(_, true)
        ));
    }

    #[tokio::test]
    async fn output_is_bounded_and_redacted_across_reads() {
        let tail = StderrTail::default();
        let mut bytes = vec![b'x'; 200_000];
        bytes.extend_from_slice(b"\nAuthorization: Bearer private-token\napi_key=secret\n");
        capture(bytes.as_slice(), tail.clone()).await;
        let text = tail.snapshot().unwrap();
        assert!(text.len() <= 1400);
        assert!(!text.contains("private-token"));
        assert!(!text.contains("secret"));
        assert!(text.contains("[REDACTED]"));
    }

    #[cfg(unix)]
    fn fixture(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        configure(&mut cmd);
        cmd
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_success_failure_timeout_and_pre_cancel() {
        run(
            fixture("test \"$CI\" = 1 && test -z \"$CLAUDECODE\" && test ! -t 0"),
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let error = run(
            fixture("echo stdout-message; echo 'api_key=private' >&2; exit 7"),
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("stdout-message"), "{error}");
        assert!(error.contains("[REDACTED]"), "{error}");
        assert!(!error.contains("private"));
        let error = run(
            fixture("sleep 60"),
            CancellationToken::new(),
            Duration::from_millis(30),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("timed out"));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            run(fixture("exit 0"), cancel, DEADLINE)
                .await
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_descendants_and_dropped_future() {
        for drop_future in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let pidfile = dir.path().join("pid");
            let mut command = fixture("sleep 60 & echo $! > \"$PIDFILE\"; wait");
            command.env("PIDFILE", &pidfile);
            let cancel = CancellationToken::new();
            let task = tokio::spawn(run(command, cancel.clone(), DEADLINE));
            tokio::time::timeout(Duration::from_secs(5), async {
                while !pidfile.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if drop_future {
                task.abort();
            } else {
                cancel.cancel();
            }
            let _ = task.await;
            let pid = std::fs::read_to_string(pidfile).unwrap().trim().to_string();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                    if stat.is_err() || stat.unwrap().split_whitespace().nth(2) == Some("Z") {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[test]
    fn manual_commands_and_missing_binary_guidance() {
        for id in IDS {
            assert_eq!(
                manual_command(id).is_some(),
                !matches!(id, HarnessId::Mock | HarnessId::Antigravity)
            );
            let (cli, dir) = cli_and_dir(id);
            assert!(!cli.is_empty() && !dir.is_empty());
        }
        assert!(
            post_install(HarnessId::Mock)
                .unwrap_err()
                .to_string()
                .contains("open a new terminal")
        );
    }
}
