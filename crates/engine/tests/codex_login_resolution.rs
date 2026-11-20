//! The codex "Add account" flow must launch the same executable the harness
//! runs — `CODEX_EXECUTABLE`, PATH discovery, and the npm `.cmd` shim layout
//! included — instead of a bare `Command::new("codex")` that only finds a
//! top-level `codex.exe`.
//!
//! Child-process probes: resolution reads process env (PATH, the override),
//! so each case re-runs this test binary with a controlled environment.

use std::path::{Path, PathBuf};

use zeron_engine::{AgentAccounts, AgentAccountsConfig};
use zeron_proto::{AgentLoginMode, AgentLoginStatus, HarnessId};

fn test_accounts(root: &Path) -> AgentAccounts {
    let config = AgentAccountsConfig {
        data_dir: root.join("data"),
        claude_config_dir: root.join("claude"),
        claude_config_file: root.join("claude.json"),
        codex_home: root.join("codex"),
        cursor_sdk_auth_file: root.join("cursor-sdk").join("auth.json"),
        // File-only: a temp config must never reach the real Keychain login.
        claude_keychain_service: None,
        antigravity_home: Some(root.join("gemini")),
        antigravity_keychain: false,
        ..AgentAccountsConfig::isolated(root)
    };
    AgentAccounts::new(config)
}

const FAKE_URL: &str = "https://auth.openai.com/authorize?probe=fake";

/// The fake CLI prints the authorize URL and then parks so the login flow
/// stays alive until the probe cancels it.
fn fake_codex(dir: &Path) -> PathBuf {
    let path = dir.join(if cfg!(windows) {
        "fake-codex.cmd"
    } else {
        "fake-codex"
    });
    if cfg!(windows) {
        std::fs::write(
            &path,
            format!(
                "@echo off\r\necho open {FAKE_URL} in your browser\r\nping -n 30 127.0.0.1 >nul\r\n"
            ),
        )
        .expect("fake codex shim");
    } else {
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"open {FAKE_URL} in your browser\"\nsleep 30\n"),
        )
        .expect("fake codex");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    path
}

#[tokio::test]
async fn login_child() {
    let Ok(expected) = std::env::var("CODEX_LOGIN_EXPECTED") else {
        return;
    };
    let root = std::env::temp_dir().join(format!("zeron-codex-login-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let accounts = test_accounts(&root);
    if expected == "true" {
        let start = accounts.start_login(HarnessId::Codex).await.expect("login starts");
        assert_eq!(start.mode, AgentLoginMode::Browser);
        assert_eq!(start.url, FAKE_URL);
        accounts.cancel_login(&start.login_id);
    } else {
        // No CLI anywhere: the sign-in installs Codex first (as Settings'
        // Install does). One that can't be installed (offline here) fails the
        // sign-in, naming what's missing.
        accounts.override_installer(zeron_engine::agent_accounts::SignInInstaller {
            installed: std::sync::Arc::new(|_| false),
            install: std::sync::Arc::new(|_| Box::pin(async { Err("offline".to_string()) })),
        });
        let start = accounts.start_login(HarnessId::Codex).await.expect("the sign-in starts by installing");
        let mut poll = accounts.poll_login(&start.login_id).await.expect("poll");
        for _ in 0..100 {
            if poll.status != AgentLoginStatus::Pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            poll = accounts.poll_login(&start.login_id).await.expect("poll");
        }
        assert_eq!(poll.status, AgentLoginStatus::Error, "{poll:?}");
        let message = poll.message.unwrap_or_default();
        assert!(message.contains("Codex") && message.contains("offline"), "names what's missing: {message}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn probe(path: &Path, override_path: Option<&Path>, expected: bool) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "login_child", "--nocapture"]);
    for key in [
        "HOME",
        "USERPROFILE",
        "FNM_DIR",
        "FNM_MULTISHELL_PATH",
        "LOCALAPPDATA",
        "NVM_SYMLINK",
        "VOLTA_HOME",
        "PNPM_HOME",
        "APPDATA",
        "CODEX_EXECUTABLE",
    ] {
        command.env_remove(key);
    }
    command
        .env("PATH", path)
        .env("CODEX_LOGIN_EXPECTED", expected.to_string())
        .env("ZERON_NO_LOGIN_SHELL", "1");
    if let Some(path) = override_path {
        command.env("CODEX_EXECUTABLE", path);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An installation reachable only through `CODEX_EXECUTABLE` must log in —
/// the exact regression where Add account failed while the harness ran.
#[test]
fn codex_login_honors_override_only_installation() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_codex(dir.path());
    // No codex anywhere on PATH: only the override can start the login.
    probe(dir.path(), Some(&fake), true);
}

/// The npm layout — a `codex.cmd` shim on PATH with the payload buried in
/// `node_modules` — must log in through the same resolution the harness uses.
/// `.cmd` discovery is a PATHEXT rule, so this only holds on Windows.
#[cfg(windows)]
#[test]
fn codex_login_runs_the_npm_cmd_shim() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // The shim plays both roles itself.
    std::fs::copy(fake_codex(&bin), bin.join("codex.cmd")).unwrap();
    probe(&bin, None, true);
}

/// No CLI anywhere: the sign-in installs it first, and one that can't be
/// installed fails naming it.
#[test]
fn codex_login_without_any_cli_installs_it_or_fails_naming_it() {
    if std::path::Path::new("/opt/homebrew/bin/codex").exists()
        || std::path::Path::new("/usr/local/bin/codex").exists()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    probe(dir.path(), None, false);
}
