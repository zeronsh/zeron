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
        code_mode: false,
        zeron_mcp_only: false,
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
        code_mode: false,
        zeron_mcp_only: false,
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

    // Antigravity background timer (`schedule`): no ACP wire frames for longer
    // than the stall budget while `conversations/<session_id>.db` holds a
    // RUNNING (status = 2) step with non-empty `task_details` must NOT fire the
    // stall watchdog.
    let timer_home = tempfile::tempdir().unwrap();
    let timer_script = dir.path().join("fake-agy-timer.py");
    std::fs::write(
        &timer_script,
        r#"#!/usr/bin/env python3
import json, os, sqlite3, sys, time

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

home = os.environ["GEMINI_HOME"]
conv_dir = os.path.join(home, "antigravity-acp", "conversations")
os.makedirs(conv_dir, exist_ok=True)
db_path = os.path.join(conv_dir, "agy-timer.db")

for raw in sys.stdin:
    msg = json.loads(raw)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "agy-timer"}})
    elif method == "session/prompt":
        conn = sqlite3.connect(db_path)
        conn.execute("CREATE TABLE IF NOT EXISTS steps (idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, task_details BLOB)")
        conn.execute("INSERT OR REPLACE INTO steps VALUES (1, 132, 2, ?)", (b"timer-5m",))
        conn.commit()
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-timer",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Standing by."}}
        }})
        # Sleep past the 700ms stall budget with zero ACP frames while timer is RUNNING.
        time.sleep(1.2)
        conn.execute("UPDATE steps SET status = 3 WHERE idx = 1")
        conn.commit()
        conn.close()
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-timer",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": " Timer fired."}}
        }})
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
"#,
    )
    .unwrap();
    std::fs::set_permissions(&timer_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (steer_tx, steer_rx) = mpsc::channel(8);
    drop(steer_tx);
    let token = CancellationToken::new();
    let timer_controls = RunControls {
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
    let mut timer_env = std::collections::BTreeMap::new();
    timer_env.insert(
        "GEMINI_HOME".to_owned(),
        timer_home.path().to_string_lossy().into_owned(),
    );
    let timer_req = RunRequest {
        mcp: None,
        prompt: "wait on schedule timer".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        code_mode: false,
        zeron_mcp_only: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: timer_env,
        extra_mcp: Vec::new(),
    };
    let agy_timer = AcpHarness::antigravity()
        .with_executable(&timer_script)
        .with_graces(Duration::from_millis(100), Duration::from_millis(200));
    let timer_stream = agy_timer
        .run(timer_req, timer_controls)
        .await
        .expect("agy timer run starts");
    let timer_events = tokio::time::timeout(
        Duration::from_secs(10),
        timer_stream.map(|r| r.expect("timer event")).collect::<Vec<_>>(),
    )
    .await
    .expect("antigravity timer run finished cleanly");
    assert!(
        !timer_events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error { .. })),
        "watchdog false-positived while schedule timer was active: {timer_events:?}"
    );
    assert!(matches!(
        timer_events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));

    // Antigravity CCPA HTTP liveness (subagents / post-timer wakeups):
    // silent ACP stdout while making CCPA calls through `ThoughtsProxy` must
    // keep resetting the stall watchdog.
    let mock_ccpa = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_ccpa_addr = mock_ccpa.local_addr().unwrap();
    let mock_ccpa_task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = mock_ccpa.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                let _ = stream.write_all(resp).await;
                let _ = stream.flush().await;
            });
        }
    });

    let ccpa_script = dir.path().join("fake-agy-ccpa-live.py");
    std::fs::write(
        &ccpa_script,
        r#"#!/usr/bin/env python3
import json, os, sys, time, urllib.request

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

base = os.environ["AGY_ACP_CCPA_BASE_URL"].rstrip("/")

for raw in sys.stdin:
    msg = json.loads(raw)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "agy-ccpa"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-ccpa",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Subagent running."}}
        }})
        # Stay silent on ACP for ~1.3s (> 700ms stall budget), pinging CCPA every 300ms.
        for _ in range(4):
            time.sleep(0.32)
            req = urllib.request.Request(
                f"{base}/v1internal:generateContent",
                data=b'{"model":"gemini-4-neon","request":{}}',
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=5) as resp:
                resp.read()
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "agy-ccpa",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": " Done."}}
        }})
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ccpa_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (steer_tx, steer_rx) = mpsc::channel(8);
    drop(steer_tx);
    let token = CancellationToken::new();
    let ccpa_controls = RunControls {
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
    let mut ccpa_env = std::collections::BTreeMap::new();
    ccpa_env.insert(
        "AGY_ACP_CCPA_BASE_URL".to_owned(),
        format!("http://{mock_ccpa_addr}"),
    );
    let ccpa_req = RunRequest {
        mcp: None,
        prompt: "run subagent via ccpa".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        code_mode: false,
        zeron_mcp_only: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: ccpa_env,
        extra_mcp: Vec::new(),
    };
    let agy_ccpa = AcpHarness::antigravity()
        .with_executable(&ccpa_script)
        .with_graces(Duration::from_millis(100), Duration::from_millis(200));
    let ccpa_stream = agy_ccpa
        .run(ccpa_req, ccpa_controls)
        .await
        .expect("agy ccpa run starts");
    let ccpa_events = tokio::time::timeout(
        Duration::from_secs(10),
        ccpa_stream.map(|r| r.expect("ccpa event")).collect::<Vec<_>>(),
    )
    .await
    .expect("antigravity ccpa liveness run finished cleanly");
    mock_ccpa_task.abort();
    assert!(
        !ccpa_events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error { .. })),
        "watchdog false-positived while CCPA HTTP requests were active: {ccpa_events:?}"
    );
    assert!(matches!(
        ccpa_events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}
