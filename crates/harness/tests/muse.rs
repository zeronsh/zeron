//! MuseHarness integration tests against the fake `muse serve` host in
//! `tests/fixtures/fake-muse.py` (no real `muse` binary involved).

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{CancellationToken, Harness, MuseHarness, RunControls, SteerMessage};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall, UserInputAnswer,
    UserInputQuestion,
};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-muse.py");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

fn harness() -> MuseHarness {
    MuseHarness::new().with_executable(fixture_path())
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Muse),
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn controls(
    answer_label: &'static str,
) -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions: Vec<UserInputQuestion>| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec![answer_label.into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        turn: Default::default(),
    };
    (controls, steer_tx, token)
}

/// The Muse runner is persistent (it parks between turns), so collect only
/// through the first `Done`.
async fn run_first_turn(
    harness: &MuseHarness,
    req: RunRequest,
    controls: RunControls,
) -> Vec<AgentEvent> {
    let mut stream = harness.run(req, controls).await.expect("run starts");
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            let event = event.expect("stream event");
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
    })
    .await
    .expect("first turn finished in time");
    events
}

fn texts(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn harness_id_wire_name_is_muse() {
    assert_eq!(serde_json::to_string(&HarnessId::Muse).unwrap(), "\"muse\"");
    assert_eq!(
        serde_json::from_str::<HarnessId>("\"muse\"").unwrap(),
        HarnessId::Muse
    );
}

#[tokio::test]
async fn happy_path_streams_text_and_completes() {
    let (controls, _steer, _token) = controls("Alpha");
    let events = run_first_turn(&harness(), request("hello muse"), controls).await;
    let start = events.iter().find_map(|e| match e {
        AgentEvent::SessionStarted {
            harness,
            session_id,
            ..
        } => Some((*harness, session_id.clone())),
        _ => None,
    });
    assert_eq!(
        start,
        Some((
            HarnessId::Muse,
            "01900000-0000-7000-8000-000000000001".into()
        ))
    );
    assert_eq!(texts(&events), "echo: hello muse");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Done { status: DoneStatus::Completed, session_id: Some(s), .. } if s == "01900000-0000-7000-8000-000000000001"
    )));
}

#[tokio::test]
async fn tool_items_decode_to_typed_calls() {
    let (controls, _steer, _token) = controls("Alpha");
    let events = run_first_turn(&harness(), request("scenario:tool"), controls).await;
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "t1".into(),
        call: ToolCall::Exec {
            command: "echo hi".into()
        },
    }));
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "t1".into(),
        is_error: false,
        output: Some("hi".into()),
        diff: None,
    }));
}

#[tokio::test]
async fn failed_turn_reports_the_host_error() {
    let (controls, _steer, _token) = controls("Alpha");
    let events = run_first_turn(&harness(), request("scenario:fail"), controls).await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Done { status: DoneStatus::Errored, error: Some(m), .. } if m == "fake failure"
    )));
}

/// The engine injects Zeron's MCP server into every real run; the host
/// refuses it without the `sessionMcp` capability grant at initialize.
/// (Regression: real runs died at session/start and the stream closed
/// without a `Done` — "harness stream ended without Done".)
#[tokio::test]
async fn mcp_injected_runs_start_and_complete() {
    let (controls, _steer, _token) = controls("Alpha");
    let mut req = request("hello with tools");
    req.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: "/bin/echo".into(),
        args: vec!["hi".into()],
        env: Default::default(),
    });
    let events = run_first_turn(&harness(), req, controls).await;
    assert_eq!(texts(&events), "echo: hello with tools");
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn leading_skill_invocation_becomes_a_native_skill_part() {
    let (controls, _steer, _token) = controls("Alpha");
    let events = run_first_turn(&harness(), request("/fake-skill do it"), controls).await;
    assert_eq!(texts(&events), "skill:fake-skill");
}

#[tokio::test]
async fn user_input_round_trips_through_request_input() {
    let (controls, _steer, _token) = controls("Beta");
    let events = run_first_turn(&harness(), request("scenario:ask"), controls).await;
    assert_eq!(texts(&events), "picked: Beta");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        }
    )));
}

#[tokio::test]
async fn missing_resume_starts_fresh_with_a_notice() {
    let (controls, _steer, _token) = controls("Alpha");
    let mut req = request("hello again");
    req.resume = Some("missing-session".into());
    let events = run_first_turn(&harness(), req, controls).await;
    assert!(texts(&events).contains("could not restore session missing-session"));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        }
    )));
}

/// Live smoke against the real `muse` binary on PATH: the catalog plus a
/// full echo-provider turn through a shim that pins `--provider echo`.
#[tokio::test]
#[ignore = "requires the real `muse` binary"]
async fn live_models_from_real_muse() {
    let harness = MuseHarness::new();
    assert!(harness.installed(), "real muse binary should resolve");
    let models = harness.models().await.expect("live models");
    assert!(!models.is_empty(), "live catalog must not be empty");
}

#[tokio::test]
#[ignore = "requires the real `muse` binary"]
async fn live_echo_turn_from_real_muse() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = dir.path().join("muse-echo");
    std::fs::write(
        &shim,
        "#!/bin/sh\n[ \"$1\" = serve ] && shift\nexec muse serve --provider echo \"$@\"\n",
    )
    .expect("shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    // `muse serve` rejects extra serve flags; the shim only forwards its own.
    let harness = MuseHarness::new().with_executable(&shim);
    let (controls, _steer, _token) = controls("Alpha");
    let mut req = request("hello live muse");
    // Mirror the real engine: it always injects its MCP server. The host
    // really starts it, so it must speak just enough MCP to initialize.
    let mcp_py = dir.path().join("fake_mcp.py");
    std::fs::write(
        &mcp_py,
        "import json, sys\nfor line in sys.stdin:\n    try: m = json.loads(line)\n    except Exception: continue\n    if 'id' not in m: continue\n    r = {'protocolVersion': '2024-11-05', 'capabilities': {}, 'serverInfo': {'name': 'fake', 'version': '0'}} if m.get('method') == 'initialize' else ({'tools': []} if m.get('method') == 'tools/list' else {})\n    print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': r}), flush=True)\n",
    )
    .expect("fake mcp server");
    req.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: "python3".into(),
        args: vec![mcp_py.to_string_lossy().into_owned()],
        env: Default::default(),
    });
    req.cwd = dir.path().to_string_lossy().into_owned();
    let events = run_first_turn(&harness, req, controls).await;
    assert_eq!(texts(&events), "echo: hello live muse", "{events:?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn models_and_skills_come_from_the_live_host() {
    let harness = harness();
    let models = harness.models().await.expect("models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "muse-fake-1");

    let cwd = std::env::temp_dir();
    let skills = harness
        .skills(&cwd)
        .await
        .expect("skills")
        .expect("muse advertises skills");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "fake-skill");
    assert_eq!(
        skills[0].command.as_ref().map(|c| c.harness),
        Some(HarnessId::Muse)
    );

    let commands = harness.commands_for(&cwd).await.expect("commands");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "fake-skill");
}
