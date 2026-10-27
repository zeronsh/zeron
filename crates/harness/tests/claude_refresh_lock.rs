#![cfg(unix)]
//! A Claude child is never torn down while Claude Code is saving a refreshed
//! login: neither signalled nor sent EOF until the refresh lock is released.
//! Its own test binary — it points the process at a temp config dir.
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use zeron_harness::claude::refresh_lock;
use zeron_harness::{CancellationToken, ClaudeHarness, Harness, RunControls};
use zeron_proto::{RunRequest, SandboxLevel};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
case "$*" in
  *--input-format*) ;;
  *) echo "2.1.309 (Claude Code)"; exit 0 ;;
esac
trap 'echo term >> "$dir/events"; exit 0' TERM
: > "$dir/ready"
read line
echo '{"type":"control_response","response":{"subtype":"success","request_id":"zeron-command-probe","response":{"commands":[{"name":"review","description":"Review"}]}}}'
while read line; do :; done
echo eof >> "$dir/events"
"#;

#[tokio::test]
async fn a_probe_is_retired_only_once_no_login_refresh_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("claude-config");
    std::fs::create_dir_all(&config).unwrap();
    // SAFETY: the only test in this binary; set before any thread reads it.
    unsafe { std::env::set_var("CLAUDE_SECURESTORAGE_CONFIG_DIR", &config) };
    let script = dir.path().join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let events = dir.path().join("events");

    // Another Claude process is mid-refresh.
    let [held, _] = refresh_lock::lock_paths(&config);
    std::fs::create_dir(&held).unwrap();

    let harness = ClaudeHarness::new().with_executable(&script);
    let commands = tokio::time::timeout(Duration::from_secs(10), harness.commands())
        .await
        .expect("the probe answers without waiting for the refresh")
        .unwrap();
    assert_eq!(commands[0].name, "review");

    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !events.exists(),
        "probe torn down mid-refresh: {:?}",
        std::fs::read_to_string(&events)
    );

    std::fs::remove_dir(&held).unwrap();
    for _ in 0..100 {
        if events.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("probe never retired once the refresh was saved");
}

/// A run's teardown (here: its consumer hung up) waits out a refresh too,
/// before the CLI gets EOF or a signal. The run names its config dir in its
/// own environment, so it shares nothing with the probe test's.
#[tokio::test]
async fn a_run_is_torn_down_only_once_no_login_refresh_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("run-config");
    std::fs::create_dir_all(&config).unwrap();
    let script = dir.path().join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let events = dir.path().join("events");
    let [held, _] = refresh_lock::lock_paths(&config);
    std::fs::create_dir(&held).unwrap();

    let request = RunRequest {
        prompt: "hello".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: dir.path().display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
        mcp: None,
        env: [(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_owned(),
            config.display().to_string(),
        )]
        .into(),
    };
    let (_steer, steering) = tokio::sync::mpsc::channel(1);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        turn: Default::default(),
    };
    let stream = ClaudeHarness::new()
        .with_executable(&script)
        .run(request, controls)
        .await
        .unwrap();
    // Hang up only once the CLI listens for SIGTERM: a signal that beats
    // the trap (a loaded machine) would end it without a trace.
    for _ in 0..200 {
        if dir.path().join("ready").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(stream);

    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !events.exists(),
        "run torn down mid-refresh: {:?}",
        std::fs::read_to_string(&events)
    );
    std::fs::remove_dir(&held).unwrap();
    for _ in 0..100 {
        if events.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run never torn down once the refresh was saved");
}
