//! Prompt-stall watchdog (`ZERON_ACP_PROMPT_STALL_MS`): a grok-spec agent
//! that goes TOTALLY silent after the prompt — the wedged-shared-leader
//! signature from the field — must surface a visible error and an errored
//! Done instead of indefinite Working. Own test binary: the env knob is
//! process-global.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{AcpHarness, CancellationToken, Harness, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-acp.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

#[tokio::test]
async fn silent_agent_errors_via_the_prompt_stall_watchdog() {
    // SAFETY: single-test binary — nothing else reads env concurrently.
    unsafe {
        std::env::set_var("ZERON_ACP_PROMPT_STALL_MS", "700");
    }
    let (_steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        turn: Default::default(),
    };
    let request = RunRequest {
        mcp: None,
        prompt: "scenario:prompt-stall".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    };
    let harness = AcpHarness::grok().with_executable(fixture_path());
    let stream = harness.run(request, controls).await.expect("run starts");
    let events = tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("watchdog closed the run in time");

    let error = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .expect("visible error chip");
    assert!(
        error.contains("did not respond"),
        "error names the stall: {error}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Errored,
            error: Some(_),
            ..
        })
    ));

    // Antigravity's sliding stall watchdog: must NOT fire while a tool call is
    // open past the stall budget, then MUST fire once the tool finishes and
    // the server goes silent mid-turn.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let agy_script = dir.path().join("fake-agy-stall.py");
    std::fs::write(
        &agy_script,
        r#"#!/usr/bin/env python3
import json, sys, time

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for raw in sys.stdin:
    msg = json.loads(raw)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "agy-stall"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-stall",
            "update": {"sessionUpdate": "tool_call", "toolCallId": "tc-long", "title": "Long build", "kind": "execute", "status": "in_progress"}
        }})
        # Hold the tool open longer than the 700ms stall budget.
        time.sleep(1.1)
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-stall",
            "update": {"sessionUpdate": "tool_call_update", "toolCallId": "tc-long", "status": "completed", "rawOutput": "built"}
        }})
        # Now go totally silent without finishing the turn.
        time.sleep(30)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&agy_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (_steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let agy_controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering: steer_rx,
        interrupt: token,
        turn: Default::default(),
    };
    let agy_req = RunRequest {
        mcp: None,
        prompt: "run long tool then stall".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    };
    let agy = AcpHarness::antigravity()
        .with_executable(&agy_script)
        .with_graces(Duration::from_millis(100), Duration::from_millis(200));
    let agy_stream = agy.run(agy_req, agy_controls).await.expect("agy run starts");
    let agy_events = tokio::time::timeout(
        Duration::from_secs(10),
        agy_stream.map(|r| r.expect("agy event")).collect::<Vec<_>>(),
    )
    .await
    .expect("antigravity sliding watchdog closed the run in time");

    assert!(
        agy_events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolResult { .. })),
        "watchdog fired prematurely while tool was still open: {agy_events:?}"
    );
    let agy_error = agy_events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .expect("antigravity visible stall error");
    assert!(
        agy_error.contains("stopped responding mid-turn"),
        "expected mid-turn stall wording: {agy_error}"
    );
    assert!(matches!(
        agy_events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Errored,
            error: Some(_),
            ..
        })
    ));
}
