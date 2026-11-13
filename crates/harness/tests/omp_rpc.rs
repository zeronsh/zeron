#![cfg(unix)]
//! The omp driver against a stand-in `omp --mode rpc-ui` replaying RPC
//! sessions trimmed from real omp 18.20.0 runs (see fixtures/omp/fake-omp.py
//! and crates/harness/src/omp/PROTOCOL.md).
use futures::StreamExt;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    CancellationToken, Harness, HarnessError, OmpHarness, RunControls, SteerMessage, TurnControl,
};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall,
    UserInputAnswer, UserInputQuestion,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/omp")
}

struct Rig {
    dir: tempfile::TempDir,
    harness: OmpHarness,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("omp");
    std::fs::copy(fixtures().join("fake-omp.py"), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    let harness = OmpHarness::new()
        .with_executable(&exe)
        .with_session_dir(dir.path().join("sessions"))
        .with_graces(Duration::from_millis(500), Duration::from_millis(500));
    Rig { dir, harness }
}

impl Rig {
    fn request(&self, scenario: &str, prompt: &str) -> RunRequest {
        let work = self.dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        RunRequest {
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: work.display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: AccessMode::FullAccess,
            plan: false,
            resume: None,
            attachments: vec![],
            worktree: None,
            mcp: None,
            env: [
                (
                    "OMP_FAKE_SCENARIO".to_string(),
                    fixtures()
                        .join(format!("{scenario}.ndjson"))
                        .display()
                        .to_string(),
                ),
                (
                    "OMP_FAKE_LOG".to_string(),
                    self.dir.path().display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            extra_mcp: Vec::new(),
        }
    }

    fn log(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn inbound(&self, kind: &str) -> Vec<Value> {
        self.log("inbound.ndjson")
            .into_iter()
            .filter(|f| f["type"] == kind)
            .collect()
    }

    fn args(&self) -> Vec<String> {
        let launch = self.log("launch.ndjson");
        serde_json::from_value(launch.last().unwrap()["args"].clone()).unwrap()
    }
}

struct Controls {
    steer: mpsc::Sender<SteerMessage>,
    interrupt: CancellationToken,
    turn: TurnControl,
    asked: mpsc::UnboundedReceiver<Vec<UserInputQuestion>>,
}

/// Controls whose input requests answer `labels` to every question.
fn controls(labels: Vec<&str>) -> (RunControls, Controls) {
    let labels: Vec<String> = labels.into_iter().map(str::to_owned).collect();
    let (steer, steering) = mpsc::channel(8);
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let interrupt = CancellationToken::new();
    let turn = TurnControl::default();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering,
            interrupt: interrupt.clone(),
            request_input: Box::new(move |questions: Vec<UserInputQuestion>| {
                let answers = questions
                    .iter()
                    .map(|q| UserInputAnswer {
                        question_id: q.id.clone(),
                        labels: labels.clone(),
                    })
                    .collect();
                let _ = asked_tx.send(questions);
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(answers);
                rx
            }),
            turn: turn.clone(),
        },
        Controls {
            steer,
            interrupt,
            turn,
            asked,
        },
    )
}

type Events = futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>;

/// Events up to and including the next `Done`.
async fn turn(events: &mut Events) -> Vec<AgentEvent> {
    let mut seen = vec![];
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.next())
            .await
            .unwrap_or_else(|_| panic!("no Done; saw {seen:?}"))
            .unwrap_or_else(|| panic!("stream ended; saw {seen:?}"))
            .unwrap();
        let done = matches!(event, AgentEvent::Done { .. });
        seen.push(event);
        if done {
            return seen;
        }
    }
}

/// Events until one matches `until` (inclusive).
async fn until(events: &mut Events, until: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let mut seen = vec![];
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.next())
            .await
            .unwrap_or_else(|_| panic!("timed out; saw {seen:?}"))
            .unwrap_or_else(|| panic!("stream ended; saw {seen:?}"))
            .unwrap();
        let hit = until(&event);
        seen.push(event);
        if hit {
            return seen;
        }
    }
}

fn done(events: &[AgentEvent]) -> (DoneStatus, Option<String>, Option<String>) {
    match events.last() {
        Some(AgentEvent::Done {
            status,
            error,
            session_id,
            ..
        }) => (*status, error.clone(), session_id.clone()),
        other => panic!("not done: {other:?}"),
    }
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

const SESSION: &str = "01a1d49f-0000-7000-8000-000000000001";

#[tokio::test]
async fn a_turn_streams_reply_and_usage_and_completes_on_its_prompt_result() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(
            rig.request("simple", "Reply with only the word ok"),
            controls,
        )
        .await
        .unwrap();
    let events = turn(&mut events).await;
    match &events[0] {
        AgentEvent::SessionStarted {
            harness,
            model,
            session_id,
            ..
        } => {
            assert_eq!(*harness, HarnessId::Omp);
            assert_eq!(model, "openai-codex/gpt-5.6-luna");
            assert_eq!(session_id, SESSION);
        }
        other => panic!("first event: {other:?}"),
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AvailableCommands { commands }
        if commands.iter().any(|c| c.name == "skill:review")))
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::AvailableCommands { .. }))
            .count(),
        1
    );
    assert_eq!(text(&events), "ok");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Usage {
            input_tokens: 6196,
            output_tokens: 5
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ContextUsage {
            window: Some(272000),
            ..
        }
    )));
    assert_eq!(
        done(&events),
        (DoneStatus::Completed, None, Some(SESSION.into()))
    );
    // The launch and the session's setup, as the driver asks for them.
    let args = rig.args();
    assert_eq!(&args[..2], ["--mode", "rpc-ui"]);
    assert!(
        args.windows(2).any(|w| w == ["--approval-mode", "yolo"]),
        "{args:?}"
    );
    assert!(args.windows(2).any(|w| w[0] == "--session-dir"), "{args:?}");
    assert_eq!(rig.inbound("negotiate_protocol")[0]["protocolVersion"], 2);
    assert_eq!(
        rig.inbound("set_event_filter")[0]["messageUpdates"],
        "delta"
    );
    assert_eq!(rig.inbound("set_steering_mode")[0]["mode"], "all");
    assert_eq!(rig.inbound("set_interrupt_mode")[0]["mode"], "wait");
    assert_eq!(rig.inbound("set_ask_dialog")[0]["enabled"], true);
    assert!(rig.inbound("prompt")[0].get("streamingBehavior").is_none());
}

#[tokio::test]
async fn tool_calls_carry_their_arguments_and_results() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("tool", "list files"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let call = events.iter().position(|e| matches!(e, AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command == "ls")).expect("the bash call");
    let result = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolResult { output: Some(out), is_error: false, .. } if out == "a.txt\nb.txt"))
        .expect("its result");
    assert!(call < result);
    assert!(text(&events).ends_with("Two files."));
    assert_eq!(done(&events).0, DoneStatus::Completed);
}

#[tokio::test]
async fn a_chunked_frame_is_reassembled_into_one_message() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("chunk", "long"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let reply = text(&events);
    assert!(
        reply.starts_with("éé") && reply.ends_with("END"),
        "{}",
        reply.len()
    );
    assert_eq!(reply.chars().count(), 4003);
}

#[tokio::test]
async fn a_local_slash_command_ends_its_turn_without_a_model_run() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("local", "/session info"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    assert!(text(&events).starts_with("Session: 01a1d49f"));
    assert_eq!(done(&events).0, DoneStatus::Completed);
}

#[tokio::test]
async fn a_local_command_sent_as_a_steer_answers_in_its_own_reply() {
    let rig = rig();
    let (controls, c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("local-steer", "hi"), controls)
        .await
        .unwrap();
    assert_eq!(text(&turn(&mut events).await), "hello");
    c.steer
        .send(SteerMessage::text("/session info"))
        .await
        .unwrap();
    let local = turn(&mut events).await;
    let steered = local
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("Steered");
    let output = local
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { .. }))
        .expect("output");
    assert!(steered < output, "{local:?}");
    assert_eq!(
        local
            .iter()
            .filter(|e| matches!(e, AgentEvent::Steered { .. }))
            .count(),
        1
    );
    assert_eq!(done(&local).0, DoneStatus::Completed);
}

#[tokio::test]
async fn a_provider_error_ends_the_turn_errored_with_omps_message() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("error", "hi"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let (status, error, _) = done(&events);
    assert_eq!(status, DoneStatus::Errored);
    assert_eq!(
        error.as_deref(),
        Some("429 Too Many Requests: rate limited")
    );
}

#[tokio::test]
async fn a_stop_aborts_in_place_and_the_runtime_takes_the_next_prompt() {
    let rig = rig();
    let (controls, c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("stop", "sleep"), controls)
        .await
        .unwrap();
    until(&mut events, |e| matches!(e, AgentEvent::ToolCall { .. })).await;
    c.turn.stop_turn();
    let stopped = turn(&mut events).await;
    assert!(
        stopped
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. }))
    );
    assert_eq!(done(&stopped).0, DoneStatus::Interrupted);
    assert_eq!(rig.inbound("abort_and_restore_queue").len(), 1);
    // The same process takes the next prompt as a new turn.
    c.steer
        .send(SteerMessage::text("still there?"))
        .await
        .unwrap();
    let next = turn(&mut events).await;
    assert!(next.iter().any(|e| matches!(e, AgentEvent::Steered { .. })));
    assert_eq!(text(&next), "yes");
    assert_eq!(done(&next).0, DoneStatus::Completed);
    assert_eq!(rig.log("launch.ndjson").len(), 1, "one process");
}

#[tokio::test]
async fn a_steer_joins_the_running_turn_and_is_confirmed_when_it_enters() {
    let rig = rig();
    let (controls, c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(
            rig.request("steer", "Use bash to run exactly: sleep 4; echo done."),
            controls,
        )
        .await
        .unwrap();
    until(&mut events, |e| matches!(e, AgentEvent::ToolCall { .. })).await;
    c.steer
        .send(SteerMessage::text(
            "Also include the word banana in your final reply.",
        ))
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let steered = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("Steered");
    let first = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text.starts_with("Command completed")))
        .unwrap();
    let banana = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "banana"))
        .unwrap();
    assert!(first < steered && steered < banana, "{events:?}");
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Done { .. }))
            .count(),
        1
    );
    assert_eq!(done(&events).0, DoneStatus::Completed);
    assert_eq!(rig.inbound("prompt")[1]["streamingBehavior"], "steer");
}

#[tokio::test]
async fn supervised_approvals_are_asked_and_answered() {
    for (scenario, answer) in [("approval", "Yes"), ("approval-deny", "No")] {
        let rig = rig();
        let (controls, mut c) = controls(vec![answer]);
        let mut request = rig.request(scenario, "echo");
        request.access = AccessMode::Supervised;
        let mut events = rig.harness.run(request, controls).await.unwrap();
        let events = turn(&mut events).await;
        assert_eq!(
            done(&events).0,
            DoneStatus::Completed,
            "{scenario}: {events:?}"
        );
        let asked = c.asked.recv().await.unwrap();
        assert_eq!(asked[0].header, "Approve command");
        assert!(asked[0].question.contains("`echo zeron-probe`"));
        assert!(
            rig.args()
                .windows(2)
                .any(|w| w == ["--approval-mode", "always-ask"])
        );
    }
}

#[tokio::test]
async fn a_wake_turn_after_background_work_is_its_own_turn() {
    let rig = rig();
    let (controls, c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("wake", "start a background job"), controls)
        .await
        .unwrap();
    let first = turn(&mut events).await;
    assert_eq!(text(&first), "started");
    let wake = turn(&mut events).await;
    assert!(
        matches!(wake[0], AgentEvent::SessionStarted { .. }),
        "{wake:?}"
    );
    assert_eq!(text(&wake), "job finished");
    assert_eq!(done(&wake).0, DoneStatus::Completed);
    // session_settled (after the wake) releases the runtime.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while c.turn.background_live() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "background never settled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn no_model_is_reported_as_not_signed_in() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("no-model", "hi"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let (status, error, _) = done(&events);
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.unwrap().contains("no model it can use"));
}

#[tokio::test]
async fn a_crash_mid_turn_ends_it_errored_with_omps_stderr() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("crash", "hi"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let (status, error, _) = done(&events);
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.unwrap().contains("lost its session store"));
}

#[tokio::test]
async fn zerons_mcp_tools_run_as_host_tools() {
    let rig = rig();
    let server = rig.dir.path().join("server.py");
    std::fs::write(
        &server,
        r#"import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    r = {}
    if m["method"] == "initialize": r = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}}
    elif m["method"] == "tools/list": r = {"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}
    elif m["method"] == "tools/call": r = {"content": [{"type": "text", "text": "echo:" + m["params"]["arguments"]["text"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#,
    )
    .unwrap();
    let mut request = rig.request("mcp", "echo hi");
    request.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: "python3".into(),
        args: vec!["-I".into(), server.display().to_string()],
        env: Default::default(),
    });
    let (controls, _c) = controls(vec![]);
    let mut events = rig.harness.run(request, controls).await.unwrap();
    let events = turn(&mut events).await;
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::ToolCall { call: ToolCall::Mcp { server, tool, .. }, .. }
        if server == "zeron" && tool == "echo")
    ));
    assert_eq!(done(&events).0, DoneStatus::Completed);
    let tools = &rig.inbound("set_host_tools")[0]["tools"];
    assert_eq!(tools[0]["name"], "zeron_echo");
    let result = &rig.inbound("host_tool_result")[0];
    assert_eq!(result["result"]["content"][0]["text"], "echo:hi");
}

#[tokio::test]
async fn a_slash_command_that_switches_sessions_reports_the_new_one() {
    let rig = rig();
    let (controls, _c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("session-switch", "/drop"), controls)
        .await
        .unwrap();
    let events = turn(&mut events).await;
    let fresh = "01a1d49f-0000-7000-8000-000000000002";
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::SessionStarted { session_id, .. } if session_id == fresh)
    ));
    assert_eq!(done(&events).2.as_deref(), Some(fresh));
}

#[tokio::test]
async fn resume_reopens_the_sessions_file_and_a_missing_one_starts_fresh_with_a_notice() {
    let rig = rig();
    let id = "01a1d49f-be6e-72f8-b068-12a6d1bc8460";
    let project = rig.dir.path().join("sessions");
    let file = project.join(format!("2026-11-13T11-16-41-198Z_{id}.jsonl"));
    std::fs::write(
        &file,
        format!("{{\"type\":\"title\",\"v\":1,\"title\":\"\"}}\n{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"/w\"}}\n"),
    )
    .unwrap();
    let mut request = rig.request("resume", "again");
    request.resume = Some(id.into());
    let (ctl, _c) = controls(vec![]);
    let mut events = rig.harness.run(request, ctl).await.unwrap();
    let events = turn(&mut events).await;
    assert!(
        matches!(&events[0], AgentEvent::SessionStarted { session_id, .. } if session_id == id)
    );
    assert_eq!(
        done(&events),
        (DoneStatus::Completed, None, Some(id.into()))
    );
    assert!(
        rig.args()
            .windows(2)
            .any(|w| w == ["--resume", file.to_str().unwrap()])
    );

    let rig = self::rig();
    let mut request = rig.request("resume", "again");
    request.resume = Some("0badbad0-0000-0000-0000-000000000000".into());
    let (ctl, _c) = controls(vec![]);
    let mut events = rig.harness.run(request, ctl).await.unwrap();
    let events = turn(&mut events).await;
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message } if message.contains("could not restore session"))));
    assert!(!rig.args().iter().any(|a| a == "--resume"));
    assert_eq!(done(&events).0, DoneStatus::Completed);
}

#[tokio::test]
async fn teardown_stops_the_turn_and_ends_the_stream_interrupted() {
    let rig = rig();
    let (controls, c) = controls(vec![]);
    let mut events = rig
        .harness
        .run(rig.request("stop", "sleep"), controls)
        .await
        .unwrap();
    until(&mut events, |e| matches!(e, AgentEvent::ToolCall { .. })).await;
    c.interrupt.cancel();
    let events = turn(&mut events).await;
    assert_eq!(done(&events).0, DoneStatus::Interrupted);
    assert_eq!(rig.inbound("abort_and_restore_queue").len(), 1);
}

#[tokio::test]
async fn discovery_lists_models_and_commands() {
    let rig = rig();
    let models = rig.harness.models().await.unwrap();
    assert_eq!(models[0].id, "openai-codex/gpt-5.6-luna");
    assert!(models[0].default);
    let commands = rig.harness.commands_for(rig.dir.path()).await.unwrap();
    assert!(commands.iter().any(|c| c.name == "compact"));
    let launch = rig.log("launch.ndjson");
    assert!(
        launch
            .iter()
            .all(|l| l["args"][1] == "rpc" && l["args"][2] == "--no-session")
    );
}
