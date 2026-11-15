#![cfg(feature = "native-fixture")]

use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    CancellationToken, DroidHarness, Harness, HarnessError, RunControls, SteerMessage, TurnControl,
};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, RunRequest, ToolCall, UserInputAnswer, UserInputQuestion,
};

fn request(cwd: &Path, prompt: &str) -> RunRequest {
    serde_json::from_value(json!({
        "prompt":prompt,"model":"fixture-mini","reasoning":"low","cwd":cwd,
        "sandbox":"workspace-write","resume":null,"access":"supervised"
    }))
    .unwrap()
}
fn harness() -> DroidHarness {
    DroidHarness::new().with_executable(env!("CARGO_BIN_EXE_harness-droid-fixture"))
}
type Events = BoxStream<'static, Result<AgentEvent, HarnessError>>;
struct Run {
    events: Events,
    steering: mpsc::Sender<SteerMessage>,
    interrupt: CancellationToken,
    turn: TurnControl,
}
async fn start(request: RunRequest) -> Run {
    start_input(
        request,
        Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
    )
    .await
}
type Input =
    Box<dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync>;
async fn start_input(request: RunRequest, input: Input) -> Run {
    let (steering, rx) = mpsc::channel(16);
    let interrupt = CancellationToken::new();
    let turn = TurnControl::default();
    let events = harness()
        .run(
            request,
            RunControls {
                realtime: None,
                execution_lease: None,
                request_input: input,
                steering: rx,
                interrupt: interrupt.clone(),
                turn: turn.clone(),
            },
        )
        .await
        .unwrap();
    Run {
        events,
        steering,
        interrupt,
        turn,
    }
}

type QuestionSet = (
    Vec<UserInputQuestion>,
    oneshot::Sender<Vec<UserInputAnswer>>,
);
async fn dialog_run(request: RunRequest) -> (Run, mpsc::UnboundedReceiver<QuestionSet>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let run = start_input(
        request,
        Box::new(move |questions| {
            let (reply, answer) = oneshot::channel();
            let _ = tx.send((questions, reply));
            answer
        }),
    )
    .await;
    (run, rx)
}
async fn dialog(
    run: &mut Run,
    questions: &mut mpsc::UnboundedReceiver<QuestionSet>,
) -> (Vec<AgentEvent>, QuestionSet) {
    let mut events = vec![];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                question = questions.recv() => return (events, question.unwrap()),
                event = run.events.next() => events.push(event.unwrap().unwrap()),
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn droid_plan_approval_matrix_and_markdown_order() {
    for (mode, legacy) in [
        (AccessMode::Supervised, false),
        (AccessMode::AutoAcceptEdits, false),
        (AccessMode::FullAccess, false),
        (AccessMode::Supervised, true),
    ] {
        let access = if legacy { AccessMode::FullAccess } else { mode };
        for response in ["Yes", "No", "dismiss", "stop"] {
            let temp = tempfile::tempdir().unwrap();
            let log = temp.path().join("frames.jsonl");
            let mut req = request(temp.path(), "plan");
            req.access = mode;
            req.auto_approve = legacy;
            req.plan = true;
            req.env
                .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
            let (mut run, mut questions) = dialog_run(req).await;
            let (events, (asked, answer)) = dialog(&mut run, &mut questions).await;
            assert_eq!(asked.len(), 1);
            assert_eq!(asked[0].header, "Approve plan");
            assert_eq!(
                asked[0].question,
                "Droid wants to start implementing this plan. Allow it?"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::ToolCall { .. }))
            );
            let mut parts = vec![];
            for event in &events {
                zeron_doc::fold_event_into_parts(&mut parts, event);
            }
            zeron_doc::fold_event_into_parts(
                &mut parts,
                &AgentEvent::InputRequested {
                    request_id: "engine-owned".into(),
                    questions: asked.clone(),
                },
            );
            assert!(
                matches!(&parts[0], zeron_doc::MessagePart::Text { text, .. } if text == "Here is the plan.\n\n# Fixture plan\n\n1. Make the change.")
            );
            assert!(matches!(&parts[1], zeron_doc::MessagePart::Input { .. }));
            match response {
                "stop" => run.turn.stop_turn(),
                "dismiss" => drop(answer),
                value => {
                    answer
                        .send(vec![UserInputAnswer {
                            question_id: asked[0].id.clone(),
                            labels: vec![value.into()],
                        }])
                        .unwrap();
                }
            }
            let all = done(&mut run.events).await;
            assert!(
                matches!(all.last(), Some(AgentEvent::Done { status, .. }) if *status == if response=="Yes" {DoneStatus::Completed} else {DoneStatus::Interrupted})
            );
            close(run).await;
            let frames = frames(&log);
            let replies: Vec<_> = frames
                .iter()
                .filter(|v| v["id"] == "permission-1" && v["type"] == "response")
                .collect();
            assert_eq!(replies.len(), 1, "{access:?} {response}");
            let expected = if response != "Yes" {
                "cancel"
            } else {
                match access {
                    AccessMode::Supervised => "proceed_once",
                    AccessMode::AutoAcceptEdits => "proceed_auto_run_medium",
                    AccessMode::FullAccess => "proceed_auto_run_high",
                }
            };
            assert_eq!(replies[0]["result"]["selectedOption"], expected);
            let settings = frames
                .iter()
                .find(|f| f["method"] == "droid.update_session_settings")
                .unwrap();
            assert_eq!(settings["params"]["interactionMode"], "spec");
            assert_eq!(
                settings["params"]["autonomyLevel"],
                match access {
                    AccessMode::Supervised => "off",
                    AccessMode::AutoAcceptEdits => "medium",
                    AccessMode::FullAccess => "high",
                }
            );
        }
    }
}

#[tokio::test]
async fn droid_invalid_plan_and_missing_option_cancel_visibly() {
    for prompt in ["plan-empty", "plan-huge", "plan-missing-option"] {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("frames.jsonl");
        let mut req = request(temp.path(), prompt);
        req.access = AccessMode::FullAccess;
        req.env
            .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
        let mut run = start_input(req, Box::new(|_| panic!("invalid plan must not ask"))).await;
        let all = done(&mut run.events).await;
        assert!(all.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
        close(run).await;
        assert!(
            frames(&log)
                .iter()
                .any(|f| f["result"]["selectedOption"] == "cancel")
        );
    }
}

#[tokio::test]
async fn droid_batch_permission_policy_never_uses_always() {
    for access in AccessMode::ALL {
        for prompt in ["permission-edit", "permission-mixed"] {
            let temp = tempfile::tempdir().unwrap();
            let log = temp.path().join("frames.jsonl");
            let mut req = request(temp.path(), prompt);
            req.access = access;
            req.env
                .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
            let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = asked.clone();
            let mut run = start_input(
                req,
                Box::new(move |questions| {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (tx, rx) = oneshot::channel();
                    tx.send(
                        questions
                            .into_iter()
                            .map(|q| UserInputAnswer {
                                question_id: q.id,
                                labels: vec!["Yes".into()],
                            })
                            .collect(),
                    )
                    .unwrap();
                    rx
                }),
            )
            .await;
            done(&mut run.events).await;
            close(run).await;
            let must_ask = access == AccessMode::Supervised
                || (access == AccessMode::AutoAcceptEdits && prompt == "permission-mixed");
            assert_eq!(
                asked.load(std::sync::atomic::Ordering::SeqCst),
                usize::from(must_ask)
            );
            let frames = frames(&log);
            let reply = frames
                .iter()
                .find(|f| f["id"] == "permission-1" && f["type"] == "response")
                .unwrap();
            assert_eq!(reply["result"]["selectedOption"], "proceed_once");
        }
    }
}

#[tokio::test]
async fn droid_questions_preserve_indices_and_cancel_on_stop() {
    for stop in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("frames.jsonl");
        let mut req = request(temp.path(), "question");
        req.env
            .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
        let (mut run, mut questions) = dialog_run(req).await;
        let (_, (asked, answer)) = dialog(&mut run, &mut questions).await;
        assert_eq!(asked[0].id, "7");
        assert!(asked[0].multi_select);
        if stop {
            run.turn.stop_turn();
        } else {
            answer
                .send(vec![
                    UserInputAnswer {
                        question_id: "7".into(),
                        labels: vec!["Red".into(), "Blue".into()],
                    },
                    UserInputAnswer {
                        question_id: "9".into(),
                        labels: vec!["Custom".into()],
                    },
                ])
                .unwrap();
        }
        done(&mut run.events).await;
        close(run).await;
        let frames = frames(&log);
        let replies: Vec<_> = frames
            .iter()
            .filter(|f| f["id"] == "question-1" && f["type"] == "response")
            .collect();
        assert_eq!(replies.len(), 1);
        if stop {
            assert_eq!(replies[0]["result"], json!({"cancelled":true,"answers":[]}));
        } else {
            assert_eq!(
                replies[0]["result"]["answers"][0],
                json!({"index":7,"question":"Choose colors","answer":"Red,Blue"})
            );
            assert_eq!(replies[0]["result"]["answers"][1]["index"], 9);
        }
    }
}
async fn next(events: &mut Events) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("bounded event")
        .expect("event stream")
        .unwrap()
}
async fn done(events: &mut Events) -> Vec<AgentEvent> {
    let mut all = vec![];
    loop {
        let event = next(events).await;
        let finished = matches!(event, AgentEvent::Done { .. });
        all.push(event);
        if finished {
            return all;
        }
    }
}
async fn close(mut run: Run) {
    run.interrupt.cancel();
    tokio::time::timeout(Duration::from_secs(10), async {
        while run.events.next().await.is_some() {}
    })
    .await
    .unwrap();
}
fn frames(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn title_run(harness: &DroidHarness, request: RunRequest) -> Vec<AgentEvent> {
    let (steer, steering) = mpsc::channel(8);
    steer
        .send(SteerMessage::text("Never send a second title prompt"))
        .await
        .unwrap();
    let events = harness
        .run_title(
            request,
            RunControls {
                realtime: None,
                execution_lease: None,
                steering,
                interrupt: CancellationToken::new(),
                turn: Default::default(),
                request_input: Box::new(|_| panic!("title runs must never ask for input")),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        events.map(Result::unwrap).collect(),
    )
    .await
    .expect("single title turn closes even with steering open")
}

fn title_request(cwd: &Path, scenario: &str) -> RunRequest {
    let mut req = request(
        cwd,
        "Implement a dangerous request\n  exactly as supplied.\n",
    );
    req.env.insert(
        "DROID_FIXTURE_LOG".into(),
        cwd.join("frames.jsonl").display().to_string(),
    );
    req.env
        .insert("DROID_FIXTURE_SCENARIO".into(), scenario.into());
    req
}

#[tokio::test]
async fn droid_title_run_is_isolated_and_tool_free() {
    assert!(zeron_harness::supports_titles(
        zeron_proto::HarnessId::Droid
    ));
    let temp = tempfile::tempdir().unwrap();
    let harness = harness();
    harness.model_catalog(false).await.unwrap();
    let mut req = title_request(temp.path(), "title");
    let prompt = req.prompt.clone();
    req.access = AccessMode::default();
    assert_eq!(req.access, AccessMode::FullAccess);
    req.auto_approve = true;
    req.plan = true;
    req.reasoning = Some(zeron_proto::ReasoningLevel::Minimal);
    req.resume = Some("must-not-load".into());
    req.worktree = Some(zeron_proto::WorktreeSpec {
        repo_path: "/must-not-use".into(),
        base: "main".into(),
        space_id: None,
    });
    req.attachments = vec!["/must-not-read.png".into()];
    let mcp = zeron_proto::McpServer {
        name: "forbidden".into(),
        command: "must-not-run".into(),
        args: vec![],
        env: Default::default(),
    };
    req.mcp = Some(mcp.clone());
    req.extra_mcp = vec![mcp];
    req.model_options
        .insert("autonomyLevel".into(), json!("high"));
    req.model_options
        .insert("systemPrompt".into(), json!("Not title instructions"));
    let events = title_run(&harness, req).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert_eq!(
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "Fixture Thread Title"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { .. } | AgentEvent::Error { .. }))
    );
    let frames = frames(&temp.path().join("frames.jsonl"));
    let methods: Vec<_> = frames.iter().filter_map(|f| f["method"].as_str()).collect();
    assert_eq!(
        methods,
        [
            "droid.initialize_session",
            "droid.update_session_settings",
            "droid.list_tools",
            "droid.add_user_message"
        ]
    );
    let init = &frames[0]["params"];
    assert_eq!(init["systemPrompt"], zeron_harness::TITLE_INSTRUCTIONS);
    assert_eq!(init["restrictToolIds"], json!(["__no-tools__"]));
    for key in [
        "disableBuiltinSkills",
        "autoRejectPermissionRequests",
        "blockOnStartupLoad",
        "blockOnMcpLoad",
    ] {
        assert_eq!(init[key], true);
    }
    assert_eq!(init["privacyLevel"], "private");
    for key in [
        "toolExecutionMode",
        "mcpServers",
        "sessionId",
        "worktree",
        "model_options",
    ] {
        assert!(init.get(key).is_none(), "{key}");
    }
    assert_eq!(
        frames[1]["params"],
        json!({
            "modelId":"fixture-mini","specModeModelId":"fixture-mini",
            "reasoningEffort":"low","specModeReasoningEffort":"low",
            "autonomyLevel":"off","interactionMode":"auto"
        })
    );
    assert_eq!(frames[2]["params"], json!({}));
    assert_eq!(frames[3]["params"], json!({"text":prompt}));
}

#[tokio::test]
async fn droid_title_run_refuses_when_tools_remain() {
    for scenario in [
        "title-tools-enabled",
        "title-tools-missing",
        "title-tools-missing-allowed",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let events = title_run(&harness(), title_request(temp.path(), scenario)).await;
        assert!(
            matches!(events.last(), Some(AgentEvent::Done {
            status:DoneStatus::Errored,error:Some(error),..
        }) if error.contains("Droid") && error.contains("tool")),
            "{scenario}: {events:?}"
        );
        let frames = frames(&temp.path().join("frames.jsonl"));
        assert!(frames.iter().any(|f| f["method"] == "droid.list_tools"));
        assert!(
            !frames
                .iter()
                .any(|f| f["method"] == "droid.add_user_message"),
            "{scenario}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        );
    }
}

#[tokio::test]
async fn droid_title_rejects_a_launch_or_settings_droid_did_not_keep() {
    for scenario in ["title-init-system", "coerced"] {
        let temp = tempfile::tempdir().unwrap();
        let events = title_run(&harness(), title_request(temp.path(), scenario)).await;
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    error: Some(_),
                    ..
                })
            ),
            "{scenario}: {events:?}"
        );
        assert!(
            !frames(&temp.path().join("frames.jsonl"))
                .iter()
                .any(|f| f["method"] == "droid.add_user_message"),
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn droid_title_refuses_server_requests_at_startup_barriers_and_during_turn() {
    let temp = tempfile::tempdir().unwrap();
    let events = title_run(&harness(), title_request(temp.path(), "title-requests")).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    let frames = frames(&temp.path().join("frames.jsonl"));
    for stage in ["startup", "settings", "tools", "turn"] {
        for (method, expected) in [
            ("permission", json!({"selectedOption":"cancel"})),
            ("ask", json!({"cancelled":true,"answers":[]})),
            ("unknown", Value::Null),
        ] {
            let id = format!("title-{stage}-{method}");
            let replies: Vec<_> = frames
                .iter()
                .filter(|f| f["id"] == id && f["type"] == "response")
                .collect();
            assert_eq!(replies.len(), 1, "{id}");
            if method == "unknown" {
                assert_eq!(replies[0]["error"]["code"], -32601);
            } else {
                assert_eq!(replies[0]["result"], expected);
            }
        }
    }
}

#[tokio::test]
async fn droid_title_effort_follows_the_cached_ladder() {
    for (model, cached, effort) in [
        ("fixture-mini-fast", true, "minimal"),
        ("fixture-mini-fast", false, "low"),
        ("auto", true, "none"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let harness = harness();
        if cached {
            harness.model_catalog(false).await.unwrap();
        }
        let mut req = title_request(temp.path(), "title");
        req.model = Some(model.into());
        req.reasoning = Some(zeron_proto::ReasoningLevel::Minimal);
        let events = title_run(&harness, req).await;
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                })
            ),
            "{events:?}"
        );
        let frames = frames(&temp.path().join("frames.jsonl"));
        let settings = &frames
            .iter()
            .find(|f| f["method"] == "droid.update_session_settings")
            .unwrap()["params"];
        assert_eq!(
            settings["reasoningEffort"], effort,
            "{model} cached={cached}"
        );
        assert_eq!(settings["specModeReasoningEffort"], effort);
    }
}

fn assistant_id(events: &[AgentEvent]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::SessionStarted {
                assistant_message_id,
                ..
            } => Some(assistant_message_id.clone()),
            _ => None,
        })
        .expect("initial assistant id")
}

fn rotation(events: &[AgentEvent], previous: &str) -> String {
    let rotations: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            AgentEvent::Steered {
                assistant_message_id,
                next_assistant_message_id,
            } => {
                assert_eq!(assistant_message_id.as_deref(), Some(previous));
                let next = next_assistant_message_id
                    .as_ref()
                    .expect("new assistant id");
                assert_ne!(next, previous);
                assert!(uuid::Uuid::parse_str(next).is_ok());
                Some((index, next.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(rotations.len(), 1);
    if let Some(text) = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { .. }))
    {
        assert!(rotations[0].0 < text, "rotate before the follow-up text");
    }
    rotations[0].1.clone()
}

#[tokio::test]
async fn droid_two_turns_share_process_and_settings_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("frames.jsonl");
    let mut req = request(temp.path(), "first");
    req.env
        .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
    let mut run = start(req).await;
    let first = done(&mut run.events).await;
    let initial = assistant_id(&first);
    assert!(
        !first
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. }))
    );
    assert!(
        first
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "first"))
    );
    assert!(matches!(
        first.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    run.steering
        .send(SteerMessage::text("second"))
        .await
        .unwrap();
    let second = done(&mut run.events).await;
    let second_id = rotation(&second, &initial);
    assert!(
        second
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "second"))
    );
    let mut third = SteerMessage::text("third");
    third.message_id = Some("idle-user-message".into());
    run.steering.send(third).await.unwrap();
    let third = done(&mut run.events).await;
    let third_id = rotation(&third, &second_id);
    assert_ne!(third_id, initial);
    assert_ne!(third_id, "idle-user-message");
    close(run).await;
    let frames = frames(&log);
    assert_eq!(
        frames
            .iter()
            .filter(|f| f["method"] == "droid.initialize_session")
            .count(),
        1
    );
    let methods: Vec<_> = frames.iter().filter_map(|f| f["method"].as_str()).collect();
    assert_eq!(
        &methods[..5],
        [
            "droid.initialize_session",
            "droid.update_session_settings",
            "droid.add_user_message",
            "droid.update_session_settings",
            "droid.add_user_message"
        ]
    );
    for frame in &frames {
        assert_eq!(frame["factoryApiVersion"], "1.0.0");
        assert_eq!(frame["type"], "request");
        assert!(frame["id"].is_string());
    }
}

#[tokio::test]
async fn droid_coerced_settings_prevent_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("frames.jsonl");
    let mut req = request(temp.path(), "must not send");
    req.env
        .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
    req.env
        .insert("DROID_FIXTURE_SCENARIO".into(), "coerced".into());
    let mut run = start(req).await;
    let all = done(&mut run.events).await;
    assert!(
        matches!(all.last(), Some(AgentEvent::Done { status:DoneStatus::Errored, error:Some(error), .. }) if error.contains("autonomyLevel"))
    );
    close(run).await;
    assert!(
        !frames(&log)
            .iter()
            .any(|f| f["method"] == "droid.add_user_message")
    );
}

#[tokio::test]
async fn droid_unrelated_settings_notification_cannot_open_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("frames.jsonl");
    let mut req = request(temp.path(), "must not send");
    req.env
        .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
    req.env
        .insert("DROID_FIXTURE_SCENARIO".into(), "no-barrier".into());
    let mut run = start(req).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("droid.update_session_settings")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), run.events.next())
            .await
            .is_err()
    );
    close(run).await;
    assert!(
        !frames(&log)
            .iter()
            .any(|f| f["method"] == "droid.add_user_message")
    );
}

#[tokio::test]
async fn droid_ack_and_idle_are_not_completion_and_stop_reuses_process() {
    let temp = tempfile::tempdir().unwrap();
    let mut run = start(request(temp.path(), "ack-only")).await;
    while !matches!(next(&mut run.events).await, AgentEvent::TextDelta { .. }) {}
    assert!(
        tokio::time::timeout(Duration::from_millis(100), run.events.next())
            .await
            .is_err()
    );
    run.turn.stop_turn();
    let stopped = done(&mut run.events).await;
    assert!(matches!(
        stopped.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    run.steering
        .send(SteerMessage::text("reused"))
        .await
        .unwrap();
    let reused = done(&mut run.events).await;
    assert!(
        reused
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "reused"))
    );
    close(run).await;
}

#[tokio::test]
async fn droid_stop_between_turns_does_not_stop_the_next_turn() {
    let temp = tempfile::tempdir().unwrap();
    let mut run = start(request(temp.path(), "first")).await;
    done(&mut run.events).await;
    run.turn.stop_turn();
    run.steering
        .send(SteerMessage::text("ack-only"))
        .await
        .unwrap();
    while !matches!(next(&mut run.events).await, AgentEvent::TextDelta { .. }) {}
    assert!(
        tokio::time::timeout(Duration::from_millis(100), run.events.next())
            .await
            .is_err()
    );
    close(run).await;
}

#[tokio::test]
async fn droid_eof_during_turn_is_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut run = start(request(temp.path(), "eof")).await;
    let all = done(&mut run.events).await;
    assert!(matches!(
        all.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Errored,
            error: Some(_),
            ..
        })
    ));
    close(run).await;
}

#[tokio::test]
async fn droid_streams_tools_reasoning_and_usage() {
    let temp = tempfile::tempdir().unwrap();
    let mut run = start(request(temp.path(), "tools")).await;
    let all = done(&mut run.events).await;
    assert!(all.iter().any(|e| matches!(e, AgentEvent::ToolCall { id, call:ToolCall::Exec { command } } if id == "tool-1" && command == "printf hello")));
    assert!(all.iter().any(|e| matches!(e, AgentEvent::ToolResult { id, output:Some(output), .. } if id == "tool-1" && output == "hello")));
    assert!(
        all.iter()
            .any(|e| matches!(e, AgentEvent::ReasoningDelta { text } if text == "thinking"))
    );
    assert!(all.iter().any(|e| matches!(
        e,
        AgentEvent::Usage {
            input_tokens: 10,
            output_tokens: 4
        }
    )));
    close(run).await;
}

#[tokio::test]
async fn droid_resume_missing_only_falls_back_and_auth_does_not() {
    for scenario in ["resume", "missing", "load-error"] {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("frames.jsonl");
        let mut req = request(temp.path(), "resumed");
        req.resume = Some("droid-fixture-session".into());
        req.env
            .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
        req.env
            .insert("DROID_FIXTURE_SCENARIO".into(), scenario.into());
        let mut run = start(req).await;
        let events = done(&mut run.events).await;
        close(run).await;
        let frames = frames(&log);
        assert_eq!(frames[0]["method"], "droid.load_session");
        assert_eq!(frames[0]["params"]["sessionId"], "droid-fixture-session");
        assert_eq!(
            frames
                .iter()
                .any(|f| f["method"] == "droid.initialize_session"),
            scenario == "missing"
        );
        assert_eq!(events.iter().any(|e|matches!(e,AgentEvent::TextDelta {text} if text.contains("without the previous context"))),scenario=="missing");
        if scenario == "load-error" {
            assert!(
                matches!(events.last(),Some(AgentEvent::Done {status:DoneStatus::Errored,error:Some(e),..}) if e.contains("/login"))
            );
            assert!(
                !frames
                    .iter()
                    .any(|f| f["method"] == "droid.add_user_message")
            );
        }
    }
}

#[tokio::test]
async fn droid_catalog_is_sessionless_and_cached() {
    let harness = harness();
    let first = harness.model_catalog(false).await.unwrap();
    assert_eq!(first.source, "live");
    assert_eq!(
        first
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["auto", "fixture-mini", "fixture-mini-fast"]
    );
    assert!(first.models[0].reasoning_levels.is_empty());
    assert_eq!(harness.model_catalog(false).await.unwrap().source, "cache");
    assert_eq!(harness.model_catalog(true).await.unwrap().source, "live");
}

#[tokio::test]
async fn droid_steering_waits_for_consumption_and_preserves_initial_turn_id() {
    for message_id in [None, Some("steer-message".to_owned())] {
        for prompt in ["steer", "ack-steer", "discard-steer"] {
            let temp = tempfile::tempdir().unwrap();
            let mut run = start(request(temp.path(), "wait")).await;
            let mut initial_events = vec![];
            loop {
                let event = next(&mut run.events).await;
                let waiting = matches!(event, AgentEvent::TextDelta { .. });
                initial_events.push(event);
                if waiting {
                    break;
                }
            }
            let initial = assistant_id(&initial_events);
            assert!(
                !initial_events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Steered { .. }))
            );
            let mut steer = SteerMessage::text(prompt);
            steer.message_id = message_id.clone();
            run.steering.send(steer).await.unwrap();
            if prompt == "ack-steer" {
                assert!(
                    matches!(next(&mut run.events).await,AgentEvent::TextDelta { text } if text=="queued")
                );
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), run.events.next())
                        .await
                        .is_err()
                );
                run.turn.stop_turn();
            }
            let events = done(&mut run.events).await;
            if prompt == "steer" {
                let next = rotation(&events, &initial);
                assert_ne!(next, "steer-message");
            } else {
                assert!(
                    !events
                        .iter()
                        .any(|e| matches!(e, AgentEvent::Steered { .. }))
                );
            }
            if prompt == "discard-steer" {
                assert!(events.iter().any(
                    |e| matches!(e,AgentEvent::Error {message} if message.contains("not delivered"))
                ));
            }
            close(run).await;
        }
    }
}

#[tokio::test]
async fn droid_structured_auth_failure_has_guidance_but_author_error_does_not() {
    for (prompt, auth) in [("auth-terminal", true), ("author-error", false)] {
        let temp = tempfile::tempdir().unwrap();
        let mut run = start(request(temp.path(), prompt)).await;
        let events = done(&mut run.events).await;
        let Some(AgentEvent::Done {
            status,
            error: Some(error),
            ..
        }) = events.last()
        else {
            panic!("missing terminal error: {events:?}");
        };
        assert_eq!(*status, DoneStatus::Errored);
        assert_eq!(error.contains("FACTORY_API_KEY"), auth);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Error { message } if message == error))
        );
        close(run).await;
    }
}

#[tokio::test]
async fn droid_child_lifecycle_and_native_progress_stay_nested() {
    let temp = tempfile::tempdir().unwrap();
    let mut run = start(request(temp.path(), "children")).await;
    while !matches!(next(&mut run.events).await,AgentEvent::TextDelta {text} if text=="child-running")
    {
    }
    assert!(run.turn.background_live());
    run.steering
        .send(SteerMessage::text("settle"))
        .await
        .unwrap();
    let events = done(&mut run.events).await;
    assert!(!run.turn.background_live());
    let text = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if parent_tool_use_id == "task-1" => match event.as_ref() {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "17. Confirmed.");
    assert_eq!(events.iter().filter(|e|matches!(e,AgentEvent::Subagent {event,..} if matches!(event.as_ref(),AgentEvent::Done{..}))).count(),1);
    close(run).await;
}

#[tokio::test]
async fn droid_child_settlement_survives_the_next_settings_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let mut req = request(temp.path(), "children");
    req.env.insert(
        "DROID_FIXTURE_SCENARIO".into(),
        "children-at-settings".into(),
    );
    let mut run = start(req).await;
    while !matches!(next(&mut run.events).await, AgentEvent::TextDelta { .. }) {}
    run.turn.stop_turn();
    done(&mut run.events).await;
    assert!(run.turn.background_live());
    run.steering
        .send(SteerMessage::text("follow-up"))
        .await
        .unwrap();
    let events = done(&mut run.events).await;
    assert!(!run.turn.background_live());
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Subagent { parent_tool_use_id, event }
        if parent_tool_use_id == "task-1" && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. }))));
    close(run).await;
}

#[tokio::test]
async fn droid_subagents_stream_after_parent_done_and_freeze_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut req = request(temp.path(), "subagents-live");
    req.env.insert(
        "FACTORY_HOME_OVERRIDE".into(),
        temp.path().display().to_string(),
    );
    let Run {
        mut events,
        steering,
        interrupt: _,
        turn,
    } = start(req).await;
    let mut all = done(&mut events).await;
    assert!(turn.background_live());
    drop(steering);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !all.iter().any(|event| matches!(event,
            AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == "task-live" && matches!(event.as_ref(), AgentEvent::ToolCall { id, call: ToolCall::ReadFile { path } } if id == "read-1" && path == "fixture.txt")
        )) {
            all.push(next(&mut events).await);
        }
    }).await.unwrap();
    assert!(!all.iter().any(|e| matches!(e, AgentEvent::Subagent { event, .. } if matches!(event.as_ref(), AgentEvent::Done { .. }))));
    std::fs::write(temp.path().join("release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.next().await {
            all.push(event.unwrap());
        }
    })
    .await
    .expect("mailbox closure waits for the children, then reaps");
    assert!(!turn.background_live());
    for (tool, status) in [
        ("task-live", DoneStatus::Completed),
        ("task-failed", DoneStatus::Errored),
    ] {
        let child: Vec<_> = all
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Subagent {
                    parent_tool_use_id,
                    event,
                } if parent_tool_use_id == tool => Some(event.as_ref()),
                _ => None,
            })
            .collect();
        assert!(matches!(child[0], AgentEvent::UserMessage { text } if text == "Read fixture.txt"));
        assert_eq!(
            child
                .iter()
                .filter(|e| matches!(e, AgentEvent::UserMessage { .. }))
                .count(),
            1
        );
        assert_eq!(
            child
                .iter()
                .filter(|e| matches!(e, AgentEvent::ToolCall { .. }))
                .count(),
            1
        );
        assert!(child.iter().any(|e| matches!(e, AgentEvent::ToolResult { id, output:Some(output), is_error, .. } if id == "read-1" && output == "fixture contents" && *is_error == (status == DoneStatus::Errored))));
        let text = child
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, format!("Checking\n\nFinal {tool}\n\n"));
        assert!(
            matches!(child.last(), Some(AgentEvent::Done { status: actual, .. }) if *actual == status)
        );
        assert_eq!(
            child
                .iter()
                .filter(|e| matches!(e, AgentEvent::Done { .. }))
                .count(),
            1
        );
    }
    let parent = all
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(parent, "parent-complete");
    assert!(!format!("{all:?}").contains("PRIVATE_"));
}

#[tokio::test]
async fn droid_config_changes_wait_for_idle_and_reuse_the_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("frames.jsonl");
    let mut req = request(temp.path(), "wait");
    req.env
        .insert("DROID_FIXTURE_LOG".into(), log.display().to_string());
    let mut run = start(req.clone()).await;
    while !matches!(next(&mut run.events).await, AgentEvent::TextDelta { .. }) {}
    let mut next_config = req;
    next_config.access = AccessMode::FullAccess;
    next_config.plan = true;
    next_config.model = Some("auto".into());
    let mut steer = SteerMessage::text("next-config");
    steer.config = Some(Box::new(next_config));
    run.steering.send(steer).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), run.events.next())
            .await
            .is_err()
    );
    assert_eq!(
        frames(&log)
            .iter()
            .filter(|f| f["method"] == "droid.update_session_settings")
            .count(),
        1
    );
    run.turn.stop_turn();
    done(&mut run.events).await;
    let next = done(&mut run.events).await;
    assert!(
        next.iter()
            .any(|e| matches!(e,AgentEvent::TextDelta{text} if text=="next-config"))
    );
    close(run).await;
    let all = frames(&log);
    assert_eq!(
        all.iter()
            .filter(|f| f["method"] == "droid.initialize_session")
            .count(),
        1
    );
    let last = all
        .iter()
        .rev()
        .find(|f| f["method"] == "droid.update_session_settings")
        .unwrap();
    assert_eq!(last["params"]["modelId"], "auto");
    assert_eq!(last["params"]["reasoningEffort"], "none");
    assert_eq!(last["params"]["specModeReasoningEffort"], "none");
    assert_eq!(last["params"]["autonomyLevel"], "high");
    assert_eq!(last["params"]["interactionMode"], "spec");
}
