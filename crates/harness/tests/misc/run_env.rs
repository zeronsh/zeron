//! `RunRequest::env` (host-resolved credentials on a Cloud device) reaches the
//! agent process each native driver spawns.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use zeron_harness::{CancellationToken, ClaudeHarness, CodexHarness, Harness, RunControls};
use zeron_proto::{RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion};

/// A stand-in agent CLI: records the env it was started with into its cwd
/// (atomically, so the reader never sees a partial file) and exits.
fn env_dumper(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("agent.sh");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf 'CODEX_HOME=%s\\nGH_TOKEN=%s\\nANTHROPIC_API_KEY=%s\\n' \
         \"$CODEX_HOME\" \"$GH_TOKEN\" \"$ANTHROPIC_API_KEY\" > env.tmp\n\
         mv env.tmp env.txt\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn request(cwd: &Path) -> RunRequest {
    RunRequest {
        prompt: "hello".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
        mcp: None,
        env: [
            ("CODEX_HOME".to_string(), "/managed/codex-home".to_string()),
            ("GH_TOKEN".to_string(), "ghu_test".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-test".to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

fn controls() -> RunControls {
    let (_steer_tx, steering) = mpsc::channel(1);
    RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_: Vec<UserInputQuestion>| {
            let (tx, rx) = oneshot::channel::<Vec<UserInputAnswer>>();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering,
        interrupt: CancellationToken::new(),
        turn: Default::default(),
    }
}

async fn recorded_env(harness: &dyn Harness, dir: &Path) -> String {
    let run = harness.run(request(dir), controls()).await;
    let file = dir.join("env.txt");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !file.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the agent process was spawned");
    drop(run);
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::fs::read_to_string(file).unwrap()
}

#[tokio::test]
async fn codex_child_receives_the_run_env() {
    let dir = tempfile::tempdir().unwrap();
    let harness = CodexHarness::new()
        .with_executable(env_dumper(dir.path()))
        .with_graces(Duration::from_millis(20), Duration::from_millis(100));
    let env = recorded_env(&harness, dir.path()).await;
    assert!(env.contains("CODEX_HOME=/managed/codex-home"), "{env}");
    assert!(env.contains("GH_TOKEN=ghu_test"), "{env}");
}

#[tokio::test]
async fn claude_child_receives_the_run_env() {
    let dir = tempfile::tempdir().unwrap();
    let harness = ClaudeHarness::new()
        .with_executable(env_dumper(dir.path()))
        .with_graces(Duration::from_millis(20), Duration::from_millis(100));
    let env = recorded_env(&harness, dir.path()).await;
    assert!(env.contains("ANTHROPIC_API_KEY=sk-ant-test"), "{env}");
    assert!(env.contains("GH_TOKEN=ghu_test"), "{env}");
}
