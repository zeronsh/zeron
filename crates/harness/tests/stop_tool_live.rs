//! Stopping one running command against the real CLIs: the agent starts a
//! 45 second command, the host stops just that call, the command dies and the
//! agent carries on in the same turn. `#[ignore]`d (a few cents on a small
//! model): `cargo test -p zeron-harness --test stop_tool_live -- --ignored`.
//!
//! The command is `python3 -c "import time; time.sleep(45)"`: Claude Code
//! refuses a bare `sleep N`.

#![cfg(unix)]

use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::mpsc;

use zeron_harness::{
    CancellationToken, ClaudeHarness, CodexHarness, Harness, RunControls, TurnControl,
};
use zeron_proto::{AgentEvent, DoneStatus, ReasoningLevel, RunRequest, SandboxLevel, ToolCall};

const PROMPT: &str = "Run exactly this one shell command, then say what happened in one sentence: \
python3 -c \"import time; time.sleep(45)\"";

fn scratch() -> tempfile::TempDir {
    let base = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("Documents/dev/tmp/stop-tool-live");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn request(cwd: &std::path::Path) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: PROMPT.into(),
        harness: None,
        model: None,
        reasoning: Some(ReasoningLevel::Low),
        model_options: serde_json::Map::new(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
        system_prompt: None,
    }
}

/// Run the prompt, stop its command the moment it starts, and return the
/// events and how long the whole turn took.
async fn stop_the_command(
    harness: &dyn Harness,
    request: RunRequest,
) -> (Vec<AgentEvent>, Duration) {
    let (_steer_tx, steering) = mpsc::channel(8);
    let turn = TurnControl::default();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        steering,
        interrupt: CancellationToken::new(),
        turn: turn.clone(),
    };
    assert!(harness.stops_tools());
    let started = Instant::now();
    let mut stream = harness.run(request, controls).await.expect("run starts");
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = stream.next().await {
            let event = event.expect("event");
            if let AgentEvent::ToolCall {
                id,
                call: ToolCall::Exec { .. },
            } = &event
            {
                turn.stop_tool(id.clone());
            }
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
    })
    .await
    .expect("the turn finished");
    (events, started.elapsed())
}

fn assert_command_killed_and_turn_went_on(events: &[AgentEvent], elapsed: Duration) {
    assert!(elapsed < Duration::from_secs(40), "the command ran its 45s");
    let result_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. }))
        .unwrap_or_else(|| panic!("the stopped command did not fail: {events:#?}"));
    assert!(
        events[result_at..]
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if !text.trim().is_empty())),
        "the agent said nothing after the command failed: {events:#?}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
#[ignore = "runs the real claude CLI"]
async fn claude_stops_one_command_and_keeps_going() {
    let dir = scratch();
    let mut request = request(dir.path());
    request.model = Some("haiku".into());
    let (events, elapsed) = stop_the_command(&ClaudeHarness::new(), request).await;
    assert_command_killed_and_turn_went_on(&events, elapsed);
}

#[tokio::test]
#[ignore = "runs the real codex CLI"]
async fn codex_stops_one_command_and_keeps_going() {
    let dir = scratch();
    // `ZERON_LIVE_CODEX` names the real binary when `codex` is a version
    // manager's shim that wants a project directory.
    let harness = match std::env::var_os("ZERON_LIVE_CODEX") {
        Some(path) => CodexHarness::new().with_executable(path),
        None => CodexHarness::new(),
    };
    let (events, elapsed) = stop_the_command(&harness, request(dir.path())).await;
    assert_command_killed_and_turn_went_on(&events, elapsed);
}
