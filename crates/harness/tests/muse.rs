#![cfg(unix)]
//! The Muse driver against a stand-in `muse serve` replaying MSP sessions
//! (recorded from a real Muse Code 1.4.x host, plus hand-written error
//! shapes): see fixtures/muse/fake-muse.py.
use futures::StreamExt;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    CancellationToken, Harness, MuseHarness, RunControls, SteerMessage, TurnControl,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall, UserInputAnswer,
    UserInputQuestion,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/muse")
}

/// The fake as `muse` in its own dir (with the catalog it serves).
fn harness(dir: &Path) -> MuseHarness {
    let exe = dir.join("muse");
    std::fs::copy(fixtures().join("fake-muse.py"), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::copy(fixtures().join("models.json"), dir.join("models.json")).unwrap();
    MuseHarness::new()
        .with_executable(exe)
        .with_graces(Duration::from_millis(500), Duration::from_millis(500))
}

fn request(dir: &Path, scenario: &str, prompt: &str) -> RunRequest {
    let work = dir.join("work");
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
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: [
            (
                "MUSE_FAKE_SCENARIO".to_string(),
                fixtures()
                    .join(format!("{scenario}.ndjson"))
                    .display()
                    .to_string(),
            ),
            ("MUSE_FAKE_LOG".to_string(), dir.display().to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

struct Controls {
    steer: mpsc::Sender<SteerMessage>,
    interrupt: CancellationToken,
    turn: TurnControl,
    asked: mpsc::UnboundedReceiver<Vec<UserInputQuestion>>,
}

fn controls(answers: Vec<UserInputAnswer>) -> (RunControls, Controls) {
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
            request_input: Box::new(move |questions| {
                let _ = asked_tx.send(questions);
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(answers.clone());
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

type Events = futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>;

/// Events up to and including the next `Done`.
async fn turn(stream: &mut Events) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("turn timed out")
            .expect("stream ended before Done")
            .unwrap();
        let done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if done {
            return events;
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
        other => panic!("turn did not end with Done: {other:?}"),
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

fn session_started(events: &[AgentEvent]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::SessionStarted {
                harness: HarnessId::Muse,
                session_id,
                ..
            } => Some(session_id.clone()),
            _ => None,
        })
        .expect("SessionStarted")
}

fn log(dir: &Path, name: &str) -> Vec<Value> {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn sent<'a>(frames: &'a [Value], method: &str) -> Vec<&'a Value> {
    frames.iter().filter(|f| f["method"] == method).collect()
}

/// The stream ends once steering closes and the runtime is idle.
async fn ends(mut stream: Events, controls: Controls) {
    drop(controls.steer);
    let rest = tokio::time::timeout(Duration::from_secs(10), async {
        let mut rest = Vec::new();
        while let Some(event) = stream.next().await {
            rest.push(event.unwrap());
        }
        rest
    })
    .await
    .expect("runtime did not shut down");
    assert!(
        rest.is_empty(),
        "unexpected events after the last turn: {rest:?}"
    );
}

#[tokio::test]
async fn a_turn_streams_text_and_usage_then_ends_on_turn_completed() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let prompt = "Respond with the following text: fixture simple ok";
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "simple", prompt), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    let session = session_started(&events);
    assert_eq!(text(&events), "fixture simple ok");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Usage {
            input_tokens: 32350,
            output_tokens: 24
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ContextUsage {
            tokens: Some(32374),
            window: Some(1007997)
        }
    )));
    // The account's subscription meters, from `usage/changed` (hand-added to
    // the recording: Muse sends it to opted-in connections only).
    let plan = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::PlanUsage {
                windows,
                plan_label,
            } => Some((windows.clone(), plan_label.clone())),
            _ => None,
        })
        .expect("PlanUsage");
    assert_eq!(plan.1.as_deref(), Some("pro"));
    assert_eq!(plan.0[0].label, "Session");
    assert!((plan.0[0].used_fraction - 0.12).abs() < 1e-6);
    assert_eq!(plan.0[1].label, "Week");
    assert_eq!(
        done(&events),
        (DoneStatus::Completed, None, Some(session.clone()))
    );
    ends(stream, controls).await;

    let launch = log(dir.path(), "launch.ndjson");
    assert_eq!(
        launch[0]["args"],
        serde_json::json!(["serve", "--trust-workspace"])
    );
    assert_eq!(launch[0]["env"]["MUSE_NO_AUTO_UPDATE"], "1");
    let frames = log(dir.path(), "inbound.ndjson");
    let start = sent(&frames, "session/start")[0];
    assert_eq!(start["params"]["sessionId"], session.as_str());
    assert_eq!(start["params"]["approvalMode"], "promptUnmatched");
    assert_eq!(start["params"]["permissionProfile"], ":ask-me");
    assert_eq!(start["params"]["providerId"], "meta");
    let root = std::fs::canonicalize(dir.path().join("work")).unwrap();
    assert_eq!(start["params"]["workspaceRoot"], root.to_str().unwrap());
    let turn_start = sent(&frames, "turn/start")[0];
    assert_eq!(turn_start["params"]["input"][0]["text"], prompt);
    assert_eq!(turn_start["params"]["displayText"], prompt);
    assert_eq!(turn_start["params"]["ifBusy"], "queue");
    assert_eq!(
        turn_start["params"]["workspaceRoots"][0],
        root.to_str().unwrap()
    );
    // Every command carries its own idempotency handle; queries none.
    assert!(
        sent(&frames, "initialize")[0]["params"]
            .get("commandId")
            .is_none()
    );
    assert!(start["params"]["commandId"].is_string());
}

#[tokio::test]
async fn auto_approve_unsandboxes_and_a_runs_own_api_key_reaches_muse() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let mut request = request(dir.path(), "simple", "hi");
    request.auto_approve = true;
    request
        .env
        .insert("META_API_KEY".into(), "from-the-run".into());
    let (run, controls) = controls(vec![]);
    let mut stream = harness.run(request, run).await.unwrap();
    turn(&mut stream).await;
    ends(stream, controls).await;
    let launch = log(dir.path(), "launch.ndjson");
    assert_eq!(
        launch[0]["args"],
        serde_json::json!(["serve", "--trust-workspace", "--disable-sandbox"])
    );
    assert_eq!(launch[0]["env"]["META_API_KEY"], "from-the-run");
    let start = &log(dir.path(), "inbound.ndjson")
        .into_iter()
        .find(|f| f["method"] == "session/start")
        .unwrap();
    assert_eq!(start["params"]["approvalMode"], "allowAll");
    assert_eq!(start["params"]["permissionProfile"], ":unrestricted");
}

#[tokio::test]
async fn an_approval_is_allowed_once_and_a_later_prompt_starts_the_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "permission", "create the marker"), run)
        .await
        .unwrap();
    let first = turn(&mut stream).await;
    assert!(first.iter().any(|e| matches!(e,
        AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command == "touch muse-approved.txt")));
    assert!(first.iter().any(|e| matches!(
        e,
        AgentEvent::ToolResult {
            is_error: false,
            ..
        }
    )));
    assert_eq!(done(&first).0, DoneStatus::Completed);
    let frames = log(dir.path(), "inbound.ndjson");
    let decide = sent(&frames, "approval/decide")[0];
    assert_eq!(decide["params"]["choiceId"], "allow_once");
    assert!(decide["params"]["requirementId"]["approvalId"].is_string());
    // The host's own approval request was answered.
    assert!(
        frames
            .iter()
            .any(|f| f.get("method").is_none() && f["id"] == 1)
    );

    controls
        .steer
        .send(SteerMessage::text("create a second marker"))
        .await
        .unwrap();
    let second = turn(&mut stream).await;
    assert!(
        matches!(second.first(), Some(AgentEvent::Steered { .. })),
        "the prompt is confirmed before its reply: {second:?}"
    );
    assert_eq!(done(&second).0, DoneStatus::Completed);
    let frames = log(dir.path(), "inbound.ndjson");
    let next = sent(&frames, "turn/start")[1];
    assert_eq!(next["params"]["ifBusy"], "steer");
    assert_eq!(next["params"]["input"][0]["text"], "create a second marker");
    assert_eq!(sent(&frames, "approval/decide").len(), 2);
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_stop_interrupts_the_turn_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    assert!(harness.stops_turn_in_place());
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "interrupt", "sleep first"), run)
        .await
        .unwrap();
    let started = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(started, AgentEvent::SessionStarted { .. }));
    controls.turn.stop_turn();
    let events = turn(&mut stream).await;
    assert_eq!(
        done(&events),
        (DoneStatus::Interrupted, None, done(&events).2)
    );
    let frames = log(dir.path(), "inbound.ndjson");
    let turn_id = sent(&frames, "turn/start")[0]["params"]["commandId"].clone();
    assert_eq!(
        sent(&frames, "turn/interrupt")[0]["params"]["turnId"],
        turn_id
    );
    // The runtime is still up: nothing more until steering closes.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), stream.next())
            .await
            .is_err()
    );
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_teardown_interrupt_ends_the_turn_and_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "interrupt", "sleep first"), run)
        .await
        .unwrap();
    stream.next().await.unwrap().unwrap();
    controls.interrupt.cancel();
    let events = turn(&mut stream).await;
    assert_eq!(done(&events).0, DoneStatus::Interrupted);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_subagent_is_a_spawn_chip_with_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "subagent", "spawn one"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    let chip = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall {
                id,
                call: ToolCall::Unknown { name, .. },
            } if name.starts_with("Agent: ") => Some((id.clone(), name.clone())),
            _ => None,
        })
        .expect("spawn chip");
    assert_eq!(chip.1, "Agent: Reply with exactly: Hello");
    // Its own spawn tool row is not repeated.
    assert!(!events.iter().any(|e| matches!(e,
        AgentEvent::ToolCall { call: ToolCall::Unknown { name, .. }, .. } if name == "subagent_spawn")));
    assert_eq!(done(&events).0, DoneStatus::Completed);
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_stored_session_is_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let mut request = request(dir.path(), "resume", "again");
    request.resume = Some("old-session".into());
    let (run, controls) = controls(vec![]);
    let mut stream = harness.run(request, run).await.unwrap();
    let events = turn(&mut stream).await;
    assert_eq!(session_started(&events), "old-session");
    assert_eq!(text(&events), "Back again.");
    assert_eq!(done(&events).2.as_deref(), Some("old-session"));
    ends(stream, controls).await;
    let frames = log(dir.path(), "inbound.ndjson");
    let resume = sent(&frames, "session/resume")[0];
    assert_eq!(resume["params"]["sessionId"], "old-session");
    assert_eq!(resume["params"]["excludeItems"], true);
    assert_eq!(
        sent(&frames, "session/setApprovalMode")[0]["params"]["mode"],
        "promptUnmatched"
    );
    assert!(sent(&frames, "session/start").is_empty());
}

#[tokio::test]
async fn a_session_muse_no_longer_has_starts_fresh_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let mut request = request(dir.path(), "resume-missing", "again");
    request.resume = Some("gone-session".into());
    let (run, controls) = controls(vec![]);
    let mut stream = harness.run(request, run).await.unwrap();
    let events = turn(&mut stream).await;
    assert!(events.iter().any(|e| matches!(e,
        AgentEvent::Error { message } if message.contains("could not restore session gone-session"))));
    assert_ne!(session_started(&events), "gone-session");
    assert_eq!(text(&events), "Fresh start.");
    assert_eq!(done(&events).0, DoneStatus::Completed);
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_crash_mid_turn_reports_muses_last_words() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, _controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "crash", "go"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    let (status, error, _) = done(&events);
    assert_eq!(status, DoneStatus::Errored);
    let error = error.unwrap();
    assert!(error.contains("Muse exited unexpectedly"), "{error}");
    assert!(error.contains("lost its session store"), "{error}");
}

#[tokio::test]
async fn a_refused_prompt_ends_its_turn_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "refused", "go"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    let (status, error, _) = done(&events);
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.unwrap().contains("usage limit reached"));
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_question_goes_to_the_user_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, mut controls) = controls(vec![UserInputAnswer {
        question_id: "q1".into(),
        labels: vec!["Changed".into()],
    }]);
    let mut stream = harness
        .run(request(dir.path(), "question", "ask me"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    let asked = controls.asked.recv().await.unwrap();
    assert_eq!(asked[0].question, "Which files?");
    assert_eq!(asked[0].options, ["All", "Changed"]);
    assert!(!asked[0].multi_select);
    assert_eq!(text(&events), "Checked the changed files.");
    let frames = log(dir.path(), "inbound.ndjson");
    let answer = sent(&frames, "userInput/answer")[0];
    assert_eq!(answer["params"]["userInputId"], "ask-1");
    assert_eq!(
        answer["params"]["answers"],
        serde_json::json!([{"questionId":"q1","selectedLabel":"Changed"}])
    );
    ends(stream, controls).await;
}

#[tokio::test]
async fn a_question_closed_unanswered_is_cancelled_not_answered() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    // No answers: the person closed the questions.
    let (run, controls) = controls(vec![]);
    let mut stream = harness
        .run(request(dir.path(), "question-dismissed", "ask me"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    assert_eq!(text(&events), "Went ahead without an answer.");
    let frames = log(dir.path(), "inbound.ndjson");
    assert!(sent(&frames, "userInput/answer").is_empty());
    let cancel = sent(&frames, "userInput/cancel")[0];
    assert_eq!(cancel["params"]["userInputId"], "ask-1");
    ends(stream, controls).await;
}

#[tokio::test]
async fn models_come_from_a_read_only_host() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let catalog = harness.model_catalog(true).await.unwrap();
    assert_eq!(
        catalog
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["muse-spark-1.2", "muse-spark-1.3"]
    );
    assert_eq!(catalog.models[0].label, "Muse Spark");
    assert_eq!(catalog.models[1].label, "Muse Spark 1.3");
    assert_eq!(catalog.models[0].reasoning_levels.len(), 5);
}

#[tokio::test]
async fn a_title_runs_on_a_read_only_host_and_ends_with_its_turn() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let (run, _controls) = controls(vec![]);
    let mut stream = harness
        .run_title(request(dir.path(), "simple", "fix the login bug"), run)
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    assert_eq!(done(&events).0, DoneStatus::Completed);
    // A title run takes no steering: the stream ends with its one turn.
    assert!(
        tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .is_none()
    );
    let launch = log(dir.path(), "launch.ndjson");
    assert_eq!(
        launch[0]["args"],
        serde_json::json!([
            "serve",
            "--disable-shell",
            "--disable-write",
            "--no-session-log"
        ])
    );
    let frames = log(dir.path(), "inbound.ndjson");
    assert_eq!(
        sent(&frames, "session/start")[0]["params"]["approvalMode"],
        "denyUnmatched"
    );
    let prompt = sent(&frames, "turn/start")[0]["params"]["input"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(prompt.starts_with(zeron_harness::TITLE_INSTRUCTIONS));
    assert!(prompt.contains("fix the login bug"));
}
