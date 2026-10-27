//! ClaudeHarness integration tests against the fake CLI in
//! `tests/fixtures/fake-claude.sh` (no real `claude` binary involved).
//! A live smoke test against the real CLI lives at the bottom, `#[ignore]`d.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    CancellationToken, ClaudeHarness, Harness, HarnessError, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall, UserInputAnswer,
    UserInputQuestion,
};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-claude.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

fn harness() -> ClaudeHarness {
    ClaudeHarness::new().with_executable(fixture_path())
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
    }
}

/// Controls whose `request_input` answers every question with `answer_label`.
fn controls(
    answer_label: &'static str,
) -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
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

async fn run_to_end(
    harness: &ClaudeHarness,
    req: RunRequest,
    controls: RunControls,
) -> Vec<AgentEvent> {
    let stream = harness.run(req, controls).await.expect("run starts");
    tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time")
}

#[tokio::test]
async fn happy_path_normalizes_events_and_tags_subagents() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:happy"), controls).await;

    // One SessionStarted despite the re-emitted init frame.
    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionStarted {
                harness,
                model,
                tools,
                session_id,
                ..
            } => Some((harness, model, tools, session_id)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "init must be deduped: {events:?}");
    let (h, model, tools, session_id) = starts[0];
    assert_eq!(*h, HarnessId::ClaudeCode);
    assert_eq!(model, "claude-fable-5");
    assert_eq!(tools, &vec!["Bash".to_string(), "Read".to_string()]);
    assert_eq!(session_id, "sess-1");

    assert!(events.contains(&AgentEvent::ReasoningDelta {
        text: "pondering".into()
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "Hello".into()
    }));

    // Subagent frames (parent_tool_use_id set) arrive TAGGED — never as bare
    // parent-feed events.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::TextDelta { text } if text.contains("SUBAGENT")
        )),
        "subagent delta leaked into the parent feed: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCall { id, .. } | AgentEvent::ToolResult { id, .. } if id == "sub-tool"
        )),
        "subagent tool frames leaked into the parent feed: {events:?}"
    );
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::TextDelta {
            text: "SUBAGENT".into()
        }),
    }));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::ToolCall {
            id: "sub-tool".into(),
            call: ToolCall::Exec {
                command: "echo sub".into()
            },
        }),
    }));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "sub-1".into(),
        event: Box::new(AgentEvent::ToolResult {
            id: "sub-tool".into(),
            is_error: false,
            output: None,
            diff: None,
        }),
    }));

    // Typed tool decoding: Bash -> Exec, mcp__server__tool -> Mcp.
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "tool-1".into(),
        call: ToolCall::Exec {
            command: "ls -la".into()
        },
    }));
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "tool-2".into(),
        call: ToolCall::Mcp {
            server: "linear".into(),
            tool: "search".into(),
            input: Some(serde_json::json!({"q": "bug"})),
        },
    }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantMessageCompleted { .. }))
    );
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "tool-1".into(),
        is_error: false,
        output: None,
        diff: None,
    }));
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "tool-2".into(),
        is_error: true,
        output: None,
        diff: None,
    }));

    // Informational rate-limit frames stay quiet.
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));

    assert!(events.contains(&AgentEvent::Usage {
        input_tokens: 10,
        output_tokens: 20
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("done!".into()),
            error: None,
            session_id: Some("sess-1".into()),
        })
    );
}

/// A run whose Zeron server is side-chats-only launches the CLI without its
/// subagent tools (`Agent`, which the init frame lists as `Task`, and
/// `Workflow`); any other run keeps them.
#[tokio::test]
async fn a_side_chats_only_run_launches_without_subagent_tools() {
    let mut server = zeron_proto::McpServer {
        name: "zeron".into(),
        command: "zeron".into(),
        args: vec!["mcp".into()],
        env: Default::default(),
    };
    let mut tools = Vec::new();
    for side_chats_only in [false, true] {
        if side_chats_only {
            server.env.insert(
                zeron_proto::McpServer::SIDE_CHATS_ONLY_ENV.into(),
                "1".into(),
            );
        }
        let mut req = request("scenario:native-subagents");
        req.mcp = Some(server.clone());
        let (controls, _steer, _token) = controls("A");
        let events = run_to_end(&harness(), req, controls).await;
        tools.push(
            events
                .iter()
                .find_map(|e| match e {
                    AgentEvent::SessionStarted { tools, .. } => Some(tools.clone()),
                    _ => None,
                })
                .expect("the session starts"),
        );
    }
    assert_eq!(tools, [vec!["Bash", "Task", "Workflow"], vec!["Bash"]]);
}

#[tokio::test]
async fn eager_done_forwards_wake_turn_as_second_done() {
    // The background-subagent shape (live-verified 2.1.228): result #1 is
    // eager — the run must NOT hold the turn for the subagent — and the wake
    // turn's frames flow through the SAME stream, settling with result #2.
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:wake"), controls).await;

    let done_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AgentEvent::Done { .. }).then_some(i))
        .collect();
    assert_eq!(
        done_positions.len(),
        2,
        "eager done + wake done: {events:?}"
    );

    // One SessionStarted total — the wake init is deduped.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            .count(),
        1
    );

    // The subagent's interior streams tagged BETWEEN the two dones.
    let tagged_position = events
        .iter()
        .position(|e| {
            matches!(e, AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == "toolu_agent")
        })
        .expect("tagged subagent traffic present");
    assert!(
        done_positions[0] < tagged_position && tagged_position < done_positions[1],
        "subagent interior must stream between the eager done and the wake done: {events:?}"
    );

    // The wake turn's own (untagged) output precedes the second done.
    let wake_text = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "subagent finished"))
        .expect("wake-turn delta present");
    assert!(done_positions[0] < wake_text && wake_text < done_positions[1]);

    // Both dones settle Completed with the same session id.
    for i in done_positions {
        assert!(matches!(
            &events[i],
            AgentEvent::Done {
                status: DoneStatus::Completed,
                session_id: Some(id),
                ..
            } if id == "sess-wake"
        ));
    }
}

#[tokio::test]
async fn ask_user_question_round_trips_through_the_control_channel() {
    // The questions must reach the ENGINE's input bridge (`request_input`) —
    // and the harness must NOT emit its own `InputRequested`/`InputResolved`
    // twins: the bridge owns that lifecycle (it mints the request id the
    // resolver is parked under; a harness-emitted copy folded an unanswerable
    // duplicate chip into the doc).
    let asked: Arc<Mutex<Vec<UserInputQuestion>>> = Arc::new(Mutex::new(Vec::new()));
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let _steer = steer_tx;
    let token = CancellationToken::new();
    let seen = asked.clone();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            seen.lock().unwrap().extend(questions.iter().cloned());
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec!["B".into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        turn: Default::default(),
    };
    let events = run_to_end(&harness(), request("scenario:askuser"), controls).await;

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].header, "Choice");
    assert_eq!(asked[0].question, "Pick one");
    assert_eq!(asked[0].options, vec!["A".to_string(), "B".to_string()]);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::InputRequested { .. } | AgentEvent::InputResolved { .. }
        )),
        "harness must not emit input lifecycle events itself: {events:?}"
    );

    // "answered" proves both control round-trips: the plain Bash can_use_tool
    // was auto-allowed AND the answers reached the CLI as updatedInput.answers
    // keyed by question text.
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("answered".into()),
            error: None,
            session_id: Some("sess-ask".into()),
        })
    );
}

#[tokio::test]
async fn questions_closed_unanswered_deny_the_tool() {
    // No answers is the person closing the questions: the CLI must read a
    // deny that says so, not an allow with empty picks.
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let _steer = steer_tx;
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering: steer_rx,
        interrupt: CancellationToken::new(),
        turn: Default::default(),
    };
    let events = run_to_end(&harness(), request("scenario:closed-question"), controls).await;
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("declined".into()),
            error: None,
            session_id: Some("sess-ask".into()),
        })
    );
}

#[tokio::test]
async fn ultrathink_preserves_selected_commands_on_initial_and_steered_sends() {
    use zeron_proto::ReasoningLevel;
    use zeron_proto::invocation::{Invocation, SkillCommand, harness_prompt};

    let invocations = [
        Invocation::Command {
            name: "review".into(),
        },
        Invocation::Skill {
            name: "review".into(),
            path: "/repo/.claude/skills/review/SKILL.md".into(),
            command: Some(SkillCommand {
                name: "review".into(),
                harness: HarnessId::ClaudeCode,
            }),
        },
    ];
    for invocation in invocations {
        for prefix in ["", " ", "   ", "\n", "\r\n  "] {
            let prompt = harness_prompt(
                &format!("{prefix}{} scenario:command-echo", invocation.link()),
                HarnessId::ClaudeCode,
            );
            let expected = format!("{prefix}/review scenario:command-echo");
            let (initial_controls, _steer, _token) = controls("A");
            let mut initial = request(&prompt);
            initial.reasoning = Some(ReasoningLevel::Ultrathink);
            let events = run_to_end(&harness(), initial, initial_controls).await;
            assert!(
                events.contains(&AgentEvent::TextDelta {
                    text: expected.clone()
                }),
                "{events:?}"
            );

            let (steer_controls, steer, _token) = controls("A");
            steer
                .send(SteerMessage {
                    prompt,
                    message_id: None,
                    attachments: Vec::new(),
                    config: None,
                })
                .await
                .unwrap();
            let mut initial = request("scenario:steer");
            initial.reasoning = Some(ReasoningLevel::Ultrathink);
            let events = run_to_end(&harness(), initial, steer_controls).await;
            assert!(
                events.contains(&AgentEvent::TextDelta {
                    text: format!("steered:{expected}")
                }),
                "{events:?}"
            );
        }
    }
}

#[tokio::test]
async fn steering_lines_are_written_to_stdin_mid_run() {
    let (controls, steer, _token) = controls("A");
    steer
        .send(SteerMessage {
            prompt: "redirect please".into(),
            message_id: None,
            attachments: Vec::new(),
            config: None,
        })
        .await
        .expect("steer queued");
    let events = run_to_end(&harness(), request("scenario:steer"), controls).await;

    let steered = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Steered {
                assistant_message_id,
                next_assistant_message_id,
            } => Some((
                assistant_message_id.clone(),
                next_assistant_message_id.clone(),
            )),
            _ => None,
        })
        .expect("Steered emitted");
    let boundary = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .unwrap();
    let continuation = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "-still-first"))
        .unwrap();
    assert!(
        continuation < boundary,
        "sending input must not split unconsumed response text"
    );
    assert!(steered.0.is_some() && steered.1.is_some());
    assert_ne!(steered.0, steered.1);

    // The fake CLI echoes the steer line's content back as a delta.
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "steered:redirect please".into()
    }));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn interrupt_escalates_to_sigterm_and_ends_with_interrupted_done() {
    let harness = ClaudeHarness::new()
        .with_executable(fixture_path())
        .with_graces(Duration::from_millis(100), Duration::from_millis(500));
    let (controls, _steer, token) = controls("A");
    let mut stream = harness
        .run(request("scenario:interrupt"), controls)
        .await
        .expect("run starts");

    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::SessionStarted { .. }) {
                token.cancel(); // interrupt as soon as the session is up
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("interrupt completed in time");

    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Interrupted,
            result: None,
            error: None,
            session_id: Some("sess-int".into()),
        })
    );
}

#[tokio::test]
async fn error_codes_map_to_readable_messages() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:error"), controls).await;

    let errors: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        errors.contains(&"Claude usage limit reached — try again after the limit resets."),
        "assistant error code not mapped: {errors:?}"
    );
    assert!(
        errors.contains(
            &"Claude 5-hour limit reached — the turn was blocked. Try again after it resets."
        ),
        "rejected rate_limit_event not mapped: {errors:?}"
    );

    // Empty `errors` array on the result falls back to subtype wording.
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done {
            status: DoneStatus::Errored,
            result: None,
            error: Some("The run hit the maximum number of turns.".into()),
            session_id: Some("sess-err".into()),
        })
    );
}

#[tokio::test]
async fn missing_binary_is_not_installed() {
    let harness = ClaudeHarness::new().with_executable("/nonexistent/claude-nowhere");
    let (controls, _steer, _token) = controls("A");
    let err = harness
        .run(request("scenario:happy"), controls)
        .await
        .err()
        .expect("spawn fails");
    assert!(matches!(err, HarnessError::NotInstalled(_)), "{err:?}");
}

#[tokio::test]
async fn missing_working_directory_is_not_a_missing_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("deleted-project");
    let (controls, _steer, _token) = controls("A");
    let err = harness()
        .run(
            RunRequest {
                cwd: gone.to_string_lossy().into_owned(),
                ..request("scenario:happy")
            },
            controls,
        )
        .await
        .err()
        .expect("spawn fails");
    match err {
        HarnessError::Io(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::NotADirectory);
            assert!(error.to_string().contains("does not exist"), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

/// Replay a captured frame file (`tests/fixtures/claude/<name>`) through the
/// whole driver path (wire parse → normalize → run loop), not just the
/// normalizer, returning everything it emitted. The fake CLI reads the prompt
/// line, then plays the capture. A captured can_use_tool control_request is
/// auto-allowed on stdin, which the replayer ignores.
async fn replay_fixture(name: &str) -> Vec<AgentEvent> {
    let frames = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claude")
        .join(name);
    let dir = tempfile::tempdir().expect("tmp dir");
    let cli = dir.path().join("replay.sh");
    std::fs::write(
        &cli,
        format!(
            "#!/bin/sh\nread -r _first || exit 1\ncat '{}'\n",
            frames.display()
        ),
    )
    .expect("replayer written");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let harness = ClaudeHarness::new().with_executable(&cli);
    let (controls, _steer, _token) = controls("A");
    // Its own config root: a typed fork's transcript is looked for there.
    let mut req = request("replay");
    req.env
        .insert("CLAUDE_CONFIG_DIR".into(), dir.path().display().to_string());
    run_to_end(&harness, req, controls).await
}

#[tokio::test]
async fn captured_live_background_subagent_frames_replay_correctly() {
    // Frames captured VERBATIM from claude 2.1.228 (2026-08-17): a turn that
    // spawns a background Agent subagent — eager result while it runs, tagged
    // subagent traffic, then the wake turn (second init, same session id,
    // second result).
    let events = replay_fixture("live-2.1.228-background-subagent.jsonl").await;

    // One SessionStarted (the wake init dedupes), two Dones (eager + wake).
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            .count(),
        1,
        "{events:?}"
    );
    let dones: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AgentEvent::Done { .. }).then_some(i))
        .collect();
    assert_eq!(dones.len(), 2, "eager done + wake done: {events:?}");

    // The parent-feed Agent spawn is a plain tool call; the subagent's own
    // Bash call arrives tagged with the spawning tool-use id, between the
    // two dones, and never as a bare parent event.
    let spawn_id = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall { id, call } => match call {
                ToolCall::Unknown { name, .. } if name.starts_with("Agent") => Some(id.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("Agent spawn tool call in the parent feed");
    // The synthesized opening user message rides WITH the spawn (before the
    // eager done); the child's own interior streams between the two dones.
    let opening: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == spawn_id
                        && matches!(event.as_ref(), AgentEvent::UserMessage { .. })
            )
            .then_some(i)
        })
        .collect();
    assert_eq!(opening.len(), 1, "one seeded opening prompt: {events:?}");
    assert!(opening[0] < dones[0], "opening rides with the spawn");
    let tagged: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == spawn_id
                        && !matches!(event.as_ref(), AgentEvent::UserMessage { .. })
            )
            .then_some(i)
        })
        .collect();
    assert!(!tagged.is_empty(), "tagged subagent traffic: {events:?}");
    assert!(
        tagged.iter().all(|i| dones[0] < *i && *i < dones[1]),
        "subagent interior streams between the eager and wake dones"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Subagent { event, .. }
                if matches!(event.as_ref(), AgentEvent::ToolCall { call: ToolCall::Exec { .. }, .. })
        )),
        "subagent Bash call arrives tagged: {events:?}"
    );
    // A background subagent's thinking reaches the stream without being
    // asked (captured lines 62 and 72): it becomes the subagent's reasoning.
    let thoughts = events
        .iter()
        .filter(|e| {
            matches!(e, AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == spawn_id
                    && matches!(event.as_ref(), AgentEvent::ReasoningDelta { text } if !text.is_empty()))
        })
        .count();
    assert_eq!(thoughts, 2, "{events:?}");
}

#[tokio::test]
async fn captured_live_forked_skill_frames_replay_correctly() {
    // Frames from claude 2.1.310 (2026-10-24): a `context: fork` skill that
    // spawns two background agents. Each kept frame is verbatim except init
    // (only the fields the driver reads); hook and rate-limit frames and most
    // of the agents' own frames are cut.
    let events = replay_fixture("live-2.1.310-forked-skill.jsonl").await;
    let tagged = |parent: &str| {
        events.iter().position(|e| {
            matches!(e, AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == parent)
        })
    };
    let skill = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall {
                id,
                call: ToolCall::Unknown { name, .. },
            } if name == "Skill" => Some(id.clone()),
            _ => None,
        })
        .expect("the Skill call, as decoded from its own frame");
    // Its task start re-decodes the chip as a spawn, ahead of all its traffic.
    let promoted = events
        .iter()
        .position(|e| {
            matches!(e, AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }
                if *id == skill && name == "Agent: /fanout-bg")
        })
        .expect("the forked skill re-decoded as a spawn");
    assert!(
        promoted < tagged(&skill).expect("skill traffic"),
        "{events:?}"
    );
    // The agents the skill spawns stream tagged with their OWN call ids. Those
    // are spawn chips of the skill's transcript, never of this feed.
    let feed_spawns: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(feed_spawns, [skill.as_str()]);
    let nested: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if *parent_tool_use_id == skill => match event.as_ref() {
                AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => Some(id.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(nested.len(), 2, "{events:?}");
    for agent in nested {
        assert!(tagged(agent).is_some(), "{agent} streams under its own id");
    }
    assert!(
        events.iter().any(|e| matches!(e,
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == skill
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        )),
        "the skill settles like any spawn: {events:?}"
    );
}

#[tokio::test]
async fn captured_live_forwarded_foreground_subagent_replays_in_order() {
    // Frames from claude 2.1.310 run with CLAUDE_CODE_FORWARD_SUBAGENT_TEXT=1
    // (2026-10-24): a foreground Agent told to write a sentence, run `echo hi`,
    // then write another. Without the variable the CLI sends only its
    // tool_use and tool_result blocks. Cut like the forked-skill capture, and
    // the main thread's frames before its Agent call too; the subagent's
    // second thought is reworded (sanitized).
    let events = replay_fixture("live-2.1.310-foreground-subagent-forwarded.jsonl").await;
    let spawn = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => Some(id.clone()),
            _ => None,
        })
        .expect("the Agent call");
    let shape: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if *parent_tool_use_id == spawn => match event.as_ref() {
                AgentEvent::ReasoningDelta { .. } => Some("thinking"),
                AgentEvent::TextDelta { text } if text.starts_with("The sky is blue.") => {
                    Some("sky")
                }
                AgentEvent::TextDelta { text } if text.starts_with("The grass is green.") => {
                    Some("grass")
                }
                AgentEvent::ToolCall {
                    call: ToolCall::Exec { command },
                    ..
                } if command == "echo hi" => Some("exec"),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        shape,
        ["thinking", "sky", "exec", "thinking", "grass"],
        "{events:?}"
    );
}

#[tokio::test]
async fn captured_live_typed_forked_command_replays_into_its_chip() {
    // Frames from claude 2.1.310 run with Zeron's launch flags and
    // CLAUDE_CODE_FORWARD_SUBAGENT_TEXT=1 (2026-10-25): `/fanout` typed as the
    // prompt, a `context: fork` skill whose two foreground agents sleep, then
    // reply alpha and bravo. The fork starts with no tool id, and its
    // notification and the session's init come before its transcript, which
    // is replayed tagged `forked-command-fanout`. Cut like the forked-skill
    // capture, the agents' own work too; two thoughts are reworded and
    // unsigned (sanitized).
    let events = replay_fixture("live-2.1.310-typed-forked-command.jsonl").await;
    const CHIP: &str = "forked-command-fanout-a4d8fe55a62c09757";
    let spawns: Vec<(usize, &str, &ToolCall)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => {
                Some((i, id.as_str(), call))
            }
            _ => None,
        })
        .collect();
    let started = events
        .iter()
        .position(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        .expect("init");
    // The fork's chip is the feed's one spawn, from its task start: before
    // the init the CLI sends only once the fork is over.
    assert!(
        matches!(&spawns[..], [(i, id, ToolCall::Unknown { name, .. })]
            if *id == CHIP && name == "Agent: /fanout" && *i < started),
        "{events:?}"
    );
    let shape: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if *parent_tool_use_id == CHIP => match event.as_ref() {
                AgentEvent::UserMessage { text }
                    if text.starts_with("Base directory for this skill:") =>
                {
                    Some("prompt")
                }
                AgentEvent::ReasoningDelta { text } if !text.is_empty() => Some("thinking"),
                AgentEvent::ToolCall { call, .. } if call.is_subagent_spawn() => Some("agent"),
                AgentEvent::ToolResult { .. } => Some("result"),
                AgentEvent::TextDelta { text } if text.starts_with("alpha bravo") => Some("reply"),
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                } => Some("done"),
                _ => Some("other"),
            },
            _ => None,
        })
        .collect();
    // Its prompt once (the replay's echo dropped), its replay, then done.
    assert_eq!(
        shape,
        [
            "prompt", "thinking", "agent", "agent", "result", "result", "thinking", "reply", "done"
        ],
        "{events:?}"
    );
    // Settled once, just before the turn's Done, its notification long gone.
    let done = events
        .iter()
        .rposition(|e| matches!(e, AgentEvent::Done { .. }))
        .expect("the turn's Done");
    assert!(
        matches!(&events[done - 2..=done], [
            AgentEvent::Subagent { parent_tool_use_id, event },
            AgentEvent::ToolResult { id, is_error: false, .. },
            AgentEvent::Done { status: DoneStatus::Completed, .. },
        ] if parent_tool_use_id == CHIP && id == CHIP
            && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })),
        "{events:?}"
    );
    // The agents it ran stream under their own ids.
    for agent in [
        "toolu_01USxAVdw1ZUxX6QKE3PogsG",
        "toolu_01LbgVmu6g5jVM5sn2dtSM7c",
    ] {
        assert!(
            events.iter().any(|e| matches!(e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if parent_tool_use_id == agent
                        && matches!(event.as_ref(), AgentEvent::UserMessage { .. }))),
            "{agent}: {events:?}"
        );
    }
}

/// Live smoke against the REAL claude CLI (2.1.x, must be installed + authed):
/// one trivial turn through the stdio permission channel, ending on the
/// result frame. `cargo test -p zeron-harness --test claude -- --ignored`.
#[tokio::test]
#[ignore = "spawns the real claude CLI; needs install + auth + network"]
async fn live_real_cli_single_turn() {
    let harness = ClaudeHarness::new();
    let mut req = request("Reply with exactly the word: pong");
    req.model = Some("haiku".into());
    req.cwd = std::env::temp_dir().display().to_string();
    req.auto_approve = false; // exercise --permission-prompt-tool stdio
    let (controls, _steer, _token) = controls("A");
    let mut stream = harness.run(req, controls).await.expect("run starts");
    // The session PARKS after the turn (steering mailbox still open, the CLI
    // waits for more stdin) — collect up to the first Done, not stream end.
    let events = tokio::time::timeout(Duration::from_secs(120), async {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            let done = matches!(ev, AgentEvent::Done { .. });
            events.push(ev);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("live turn finished in time");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. })),
        "{events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Slash-command discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn commands_come_from_the_initialize_control_request() {
    let h = harness();
    let commands = h.commands().await.expect("discovery succeeds");
    assert_eq!(
        commands.len(),
        2,
        "nameless entries are dropped: {commands:?}"
    );
    assert_eq!(commands[0].name, "review");
    assert_eq!(commands[0].description, "Review a pull request");
    assert_eq!(commands[0].input_hint.as_deref(), Some("[pr number]"));
    assert_eq!(commands[1].name, "compact");
    assert_eq!(commands[1].input_hint, None, "empty hint reads as None");

    // Cached: the second call reuses the first probe's result (the fake has
    // exited; a re-probe against a dead binary path would still work here,
    // but object identity of the cached list is the cheap assertion).
    let again = h.commands().await.expect("cache hit");
    assert_eq!(again, commands);
}

/// Live smoke against the real CLI: `cargo test -p zeron-harness --test
/// claude -- --ignored live_commands`. No model turn, no API cost.
#[tokio::test]
#[ignore]
async fn live_commands_discovery() {
    let h = ClaudeHarness::new();
    let commands = h.commands().await.expect("live discovery");
    assert!(!commands.is_empty());
    eprintln!("{} commands, first: {:?}", commands.len(), commands.first());
}

#[tokio::test]
async fn title_run_disables_tools_and_denies_unexpected_permissions() {
    let (controls, _steer, token) = controls("Yes");
    let mut stream = harness()
        .run_title(request("scenario:title"), controls)
        .await
        .unwrap();
    let events = tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
        events
    })
    .await
    .unwrap();
    token.cancel();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "Fix Login Flow")),
        "{events:?}"
    );
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
async fn command_discovery_tracks_project_changes() {
    let h = harness();
    for name in ["project-a", "project-b"] {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join(".command-fixture"), name).unwrap();
        let commands = h
            .commands_for(&cwd.path().canonicalize().unwrap())
            .await
            .unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, name);
    }
}

#[tokio::test]
async fn shared_skill_colliding_with_builtin_keeps_file_delivery() {
    use zeron_proto::invocation::{Invocation, harness_prompt};
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    let directory = cwd.path().join(".agents/skills/compact");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("SKILL.md");
    std::fs::write(&path, "---\nname: compact\n---\nCompact JSON fixtures.").unwrap();

    let harness = harness();
    let commands = harness.commands_for(cwd.path()).await.unwrap();
    assert!(commands.iter().any(|command| command.name == "compact"));
    let skill = harness
        .skills(cwd.path())
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .find(|skill| skill.path == path.to_string_lossy())
        .unwrap();
    assert!(skill.command.is_none());
    let invocation = Invocation::Skill {
        name: skill.name,
        path: skill.path,
        command: skill.command,
    };
    assert_eq!(
        harness_prompt(
            &format!("{} data.json", invocation.link()),
            HarnessId::ClaudeCode
        ),
        format!("Use the skill {} data.json", invocation.prompt_text())
    );
}

#[tokio::test]
async fn claude_skills_follow_native_availability_and_dollar_selection_keeps_arguments() {
    use zeron_proto::{
        HarnessId,
        invocation::{Invocation, harness_prompt},
    };
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    for name in ["review", "disabled-plugin"] {
        let directory = cwd.path().join(".claude/skills").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test\n---\nInstructions"),
        )
        .unwrap();
    }
    let skills = harness().skills(cwd.path()).await.unwrap().unwrap();
    assert!(!skills.iter().any(|skill| skill.name == "disabled-plugin"));
    let skill = skills
        .into_iter()
        .find(|skill| skill.name == "review")
        .unwrap();
    assert_eq!(
        skill.command.as_ref().unwrap().harness,
        HarnessId::ClaudeCode
    );
    let invocation = Invocation::Skill {
        name: skill.name,
        path: skill.path,
        command: skill.command,
    };
    assert_eq!(
        harness_prompt(&format!("{} 123", invocation.link()), HarnessId::ClaudeCode),
        "/review 123"
    );
}

/// Rapid `now` steers: the CLI replays only the last one. The replay must
/// confirm every earlier steer too, and the run must still end.
#[tokio::test]
async fn a_replay_confirms_superseded_steers_and_the_turn_ends() {
    let (controls, steer, _token) = controls("A");
    for prompt in ["first steer", "second steer"] {
        steer
            .send(SteerMessage {
                prompt: prompt.into(),
                message_id: None,
                attachments: Vec::new(),
                config: None,
            })
            .await
            .expect("steer queued");
    }
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        run_to_end(&harness(), request("scenario:superseded-steers"), controls),
    )
    .await
    .expect("run must end");
    let steered = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Steered { .. }))
        .count();
    assert_eq!(steered, 2, "{events:?}");
    let dones: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .collect();
    assert_eq!(dones.len(), 1, "{events:?}");
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "answered-both".into()
    }));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

fn steer_msg(prompt: &str) -> SteerMessage {
    SteerMessage {
        prompt: prompt.into(),
        message_id: None,
        attachments: Vec::new(),
        config: None,
    }
}

/// The premature-"done" bug: a `now` steer aborts the streaming turn, whose
/// result was held for the steer — and released by a 5s quiet timer while
/// the steered turn ran a long tool. The engine read that stale Done as the
/// steered turn completing (sound, notification, parked mid-turn).
#[tokio::test]
async fn a_steered_turn_running_a_long_tool_is_not_marked_done() {
    let (controls, steer, _token) = controls("A");
    steer.send(steer_msg("stop and sleep")).await.unwrap();
    let stream = harness()
        .run(request("scenario:lifecycle-now-steer"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(
        Duration::from_secs(20),
        stream.map(|r| r.expect("event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run ends");
    let dones: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .collect();
    assert_eq!(
        dones.len(),
        1,
        "exactly the steered turn's own end: {events:?}"
    );
    let done_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Done { .. }))
        .unwrap();
    let tool_result_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolResult { id, .. } if id == "tool-sleep"))
        .expect("tool result");
    assert!(
        done_at > tool_result_at,
        "Done before the tool finished: {events:?}"
    );
    assert!(matches!(
        dones[0],
        AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        }
    ));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Steered { .. }))
            .count(),
        1
    );
}

/// A message queued behind a turn that just finished: that turn's result is
/// a boundary; the run's one Done is the queued message's own.
#[tokio::test]
async fn a_message_queued_behind_a_finishing_turn_owns_the_turn_end() {
    let (controls, steer, _token) = controls("A");
    steer.send(steer_msg("follow up")).await.unwrap();
    let events = run_to_end(
        &harness(),
        request("scenario:lifecycle-queued-behind-end"),
        controls,
    )
    .await;
    let dones = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .count();
    assert_eq!(dones, 1, "{events:?}");
    let steered_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("steered");
    let second_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "second answer"))
        .expect("second answer");
    assert!(steered_at < second_at);
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

/// A message sent while a typed fork runs waits it out (`next`: the chip is
/// an open tool), so the fork's result is only that message's boundary: 2.1.310
/// starts the message after it. The chip settles at that result, before the
/// turn splits. Settled after the split, its tool result reached the engine
/// as a stale echo of the turn before, and the chip never resolved.
#[tokio::test]
async fn a_typed_fork_settles_before_a_message_waiting_on_it_splits_the_turn() {
    let (controls, steer, _token) = controls("A");
    let mut stream = harness()
        .run(request("scenario:typed-fork-steer-boundary"), controls)
        .await
        .expect("run starts");
    macro_rules! next {
        () => {
            tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .expect("event in time")
                .expect("stream open")
                .expect("stream event")
        };
    }
    const CHIP: &str = "forked-command-probe-t1";
    // The chip, then the prompt its tab opens with: the fork is running.
    loop {
        if matches!(next!(), AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == CHIP && matches!(event.as_ref(), AgentEvent::UserMessage { .. }))
        {
            break;
        }
    }
    steer.send(steer_msg("follow up")).await.unwrap();
    let mut events = Vec::new();
    loop {
        let event = next!();
        let done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if done {
            break;
        }
    }
    let chip_done_at = events.iter().position(|e| {
        matches!(e, AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == CHIP
                && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. }))
    });
    let chip_result_at = events.iter().position(
        |e| matches!(e, AgentEvent::ToolResult { id, is_error: false, .. } if id == CHIP),
    );
    let steered_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }));
    let (Some(chip_done_at), Some(chip_result_at), Some(steered_at)) =
        (chip_done_at, chip_result_at, steered_at)
    else {
        panic!("no settle or no split: {events:?}");
    };
    assert!(
        chip_done_at < chip_result_at && chip_result_at < steered_at,
        "the chip settled after the turn split: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Steered { .. }))
            .count(),
        1,
        "{events:?}"
    );
    // The run's first Done is the message's own, after its reply: the
    // fork's result was only the boundary.
    assert!(
        matches!(
            events.last(),
            Some(AgentEvent::Done { status: DoneStatus::Completed, result: Some(result), .. })
                if result == "second answer"
        ),
        "{events:?}"
    );
    assert!(
        events[steered_at..].contains(&AgentEvent::TextDelta {
            text: "second answer".into()
        }),
        "{events:?}"
    );
}

/// "Send now" while a turn end is held for a queued steer: the stop's one
/// Done waits for the stop to settle, so the replacement prompt — sent the
/// moment that Done arrives — runs to its own end, never settled by the
/// stopped turn's late result.
#[tokio::test]
async fn send_now_over_a_held_turn_end_never_settles_the_replacement() {
    use zeron_harness::TurnControl;
    let (mut controls, steer, _token) = controls("A");
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    let mut stream = harness()
        .run(request("scenario:stop-held"), controls)
        .await
        .expect("run starts");
    steer.send(steer_msg("queued steer")).await.unwrap();
    macro_rules! next {
        () => {
            tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .expect("event in time")
                .expect("stream open")
                .expect("stream event")
        };
    }
    loop {
        if matches!(next!(), AgentEvent::TextDelta { text } if text == "first answer") {
            break;
        }
    }
    // Let the result be held behind the queued steer, then stop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    turn.stop_turn();
    let stopped = loop {
        if let done @ AgentEvent::Done { .. } = next!() {
            break done;
        }
    };
    assert!(
        matches!(
            stopped,
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                ..
            }
        ),
        "{stopped:?}"
    );
    // The replacement goes out the moment the stop settles.
    steer.send(steer_msg("replacement")).await.unwrap();
    let mut after = Vec::new();
    let end = loop {
        match next!() {
            done @ AgentEvent::Done { .. } => break done,
            event => after.push(event),
        }
    };
    assert!(
        after.contains(&AgentEvent::TextDelta {
            text: "replacement".into()
        }),
        "the replacement turn was settled before its reply: {after:?} then {end:?}"
    );
    assert!(
        matches!(
            end,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        ),
        "{end:?}"
    );
}

/// After a stop, a replacement that answers within the stop's settle window
/// from a CLI that confirms nothing (no lifecycle, no replay) still ends: its
/// result is reconciled like any unconfirmed steer's, never dropped as the
/// stopped turn reporting late (which left the chat Working).
#[tokio::test]
async fn a_quick_unconfirmed_replacement_after_a_stop_still_completes() {
    use zeron_harness::TurnControl;
    let (mut controls, steer, _token) = controls("A");
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    let mut stream = harness()
        .run(request("scenario:stop-unconfirmed-replacement"), controls)
        .await
        .expect("run starts");
    macro_rules! next {
        () => {
            tokio::time::timeout(Duration::from_secs(15), stream.next())
                .await
                .expect("event in time")
                .expect("stream open")
                .expect("stream event")
        };
    }
    loop {
        if matches!(next!(), AgentEvent::TextDelta { text } if text == "working") {
            break;
        }
    }
    turn.stop_turn();
    loop {
        if let AgentEvent::Done { status, .. } = next!() {
            assert_eq!(status, DoneStatus::Interrupted);
            break;
        }
    }
    steer.send(steer_msg("replacement")).await.unwrap();
    let mut after = Vec::new();
    let end = loop {
        match next!() {
            done @ AgentEvent::Done { .. } => break done,
            event => after.push(event),
        }
    };
    assert!(
        after.contains(&AgentEvent::TextDelta {
            text: "new reply".into()
        }),
        "{after:?}"
    );
    assert!(
        matches!(
            end,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        ),
        "{end:?}"
    );
}

/// A stopped turn reporting a second, delayed result — no new prompt, no
/// new content — is dropped whole: no second Done (an Errored one failed
/// the parked session), and none of its usage.
#[tokio::test]
async fn a_stopped_turns_delayed_duplicate_result_is_dropped() {
    use zeron_harness::TurnControl;
    let (mut controls, _steer, _token) = controls("A");
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    let mut stream = harness()
        .run(request("scenario:stop-late-duplicate"), controls)
        .await
        .expect("run starts");
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("event in time")
            .expect("stream open")
            .expect("stream event");
        if matches!(event, AgentEvent::TextDelta { text } if text == "working") {
            break;
        }
    }
    turn.stop_turn();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("event in time")
            .expect("stream open")
            .expect("stream event");
        if let AgentEvent::Done { status, .. } = event {
            assert_eq!(status, DoneStatus::Interrupted);
            break;
        }
    }
    let mut after = Vec::new();
    let quiet = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.next().await {
            after.push(event.expect("stream event"));
        }
    })
    .await;
    assert!(quiet.is_err(), "the stream ended: {after:?}");
    assert!(
        !after
            .iter()
            .any(|e| matches!(e, AgentEvent::Done { .. } | AgentEvent::Usage { .. })),
        "the stopped turn's late duplicate was forwarded: {after:?}"
    );
}

/// A typed fork in flight when a stop goes unreported (answered, but no
/// result before the settle window closes) settles with the turn the driver
/// ends itself. Otherwise its chip runs on in the parked session, and keeps
/// the next message waiting for it (`next`).
#[tokio::test]
async fn a_stop_the_cli_never_reports_settles_its_typed_fork() {
    use zeron_harness::TurnControl;
    let (mut controls, steer, _token) = controls("A");
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    let mut stream = harness()
        .run(request("scenario:stop-typed-fork"), controls)
        .await
        .expect("run starts");
    macro_rules! next {
        () => {
            tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .expect("event in time")
                .expect("stream open")
                .expect("stream event")
        };
    }
    const CHIP: &str = "forked-command-probe-t1";
    // The chip, then the prompt its tab opens with.
    loop {
        if matches!(next!(), AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == CHIP && matches!(event.as_ref(), AgentEvent::UserMessage { .. }))
        {
            break;
        }
    }
    turn.stop_turn();
    let mut events = Vec::new();
    loop {
        let event = next!();
        let done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if done {
            break;
        }
    }
    assert!(
        matches!(&events[..], [
            AgentEvent::Subagent { parent_tool_use_id, event },
            AgentEvent::ToolResult { id, is_error: false, .. },
            AgentEvent::Done { status: DoneStatus::Interrupted, .. },
        ] if parent_tool_use_id == CHIP && id == CHIP
            && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. })),
        "{events:?}"
    );
    // The chip's tool ended with the turn: nothing holds the next message.
    steer.send(steer_msg("Again.")).await.unwrap();
    loop {
        if let AgentEvent::TextDelta { text } = next!() {
            assert_eq!(text, "priority:now");
            break;
        }
    }
}

/// A queued message the CLI cancels before starting releases the held turn
/// end at once, from the lifecycle frame — not after a quiet timer.
#[tokio::test]
async fn a_cancelled_queued_message_releases_the_turn_end_immediately() {
    let (controls, steer, _token) = controls("A");
    steer.send(steer_msg("never started")).await.unwrap();
    let started = std::time::Instant::now();
    let mut stream = harness()
        .run(request("scenario:lifecycle-queued-cancelled"), controls)
        .await
        .expect("run starts");
    let status = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            if let AgentEvent::Done { status, .. } = event.expect("event") {
                return status;
            }
        }
        panic!("stream ended without Done");
    })
    .await
    .expect("released");
    assert_eq!(status, DoneStatus::Completed);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// A follow-up sent with another model/effort is adopted by the live process
/// instead of replacing it (which killed its background agents and shells).
#[tokio::test]
async fn a_changed_model_and_effort_apply_to_the_live_process() {
    use zeron_proto::ReasoningLevel;
    assert!(harness().reconfigures_in_place(&request("a"), &{
        let mut next = request("b");
        next.model = Some("claude-haiku-4-5".into());
        next
    }));
    let mut cwd_moved = request("b");
    cwd_moved.cwd = "/elsewhere".into();
    assert!(!harness().reconfigures_in_place(&request("a"), &cwd_moved));

    let (controls, steer, _token) = controls("A");
    let mut launch = request("scenario:reconfigure");
    launch.model = Some("claude-sonnet-5-5".into());
    launch.reasoning = Some(ReasoningLevel::Low);
    launch
        .model_options
        .insert("fastMode".into(), serde_json::json!(true));
    let mut next = launch.clone();
    next.model = Some("claude-haiku-4-5".into());
    next.reasoning = Some(ReasoningLevel::High);
    next.model_options.clear();
    let mut switched = steer_msg("switched");
    switched.config = Some(Box::new(next.clone()));
    steer.send(switched).await.unwrap();
    let mut same = steer_msg("same again");
    same.config = None;
    steer.send(same).await.unwrap();
    drop(steer);
    let events = run_to_end(&harness(), launch, controls).await;
    assert!(
        events.contains(&AgentEvent::TextDelta {
            text: "on haiku".into()
        }),
        "{events:?}"
    );
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
}

/// "Marked done the moment I sent it": resuming a session whose background
/// tasks died with its old process, the CLI settles them with empty results
/// while the new prompt is still queued. Those are not the prompt's end.
#[tokio::test]
async fn results_settling_killed_tasks_on_resume_do_not_end_the_prompt() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(
        &harness(),
        request("Reply PONG scenario:resume-killed-tasks"),
        controls,
    )
    .await;
    let dones: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .collect();
    assert_eq!(dones.len(), 1, "{events:?}");
    assert!(
        matches!(dones[0], AgentEvent::Done { status: DoneStatus::Completed, result: Some(r), .. } if r == "PONG"),
        "{events:?}"
    );
    let pong = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "PONG"))
        .expect("reply");
    assert!(
        events
            .iter()
            .position(|e| matches!(e, AgentEvent::Done { .. }))
            .unwrap()
            > pong
    );
}

/// A steer the CLI never replays must not hold the turn end forever.
#[tokio::test]
async fn an_unreplayed_steer_releases_the_turn_end() {
    let (controls, steer, _token) = controls("A");
    steer
        .send(SteerMessage {
            prompt: "absorbed steer".into(),
            message_id: None,
            attachments: Vec::new(),
            config: None,
        })
        .await
        .expect("steer queued");
    let started = std::time::Instant::now();
    let mut stream = harness()
        .run(request("scenario:absorbed-steer"), controls)
        .await
        .expect("run starts");
    let mut steered = 0;
    let done = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while let Some(event) = stream.next().await {
            match event.expect("event") {
                AgentEvent::Steered { .. } => steered += 1,
                AgentEvent::Done { status, .. } => return status,
                _ => {}
            }
        }
        panic!("stream ended without Done");
    })
    .await
    .expect("turn end must be released");
    assert_eq!(done, DoneStatus::Completed);
    assert_eq!(steered, 1);
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));
}

// ---------------------------------------------------------------------------
// Stopping a turn in place
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_stopped_turn_keeps_the_process_and_its_background_work() {
    use zeron_harness::TurnControl;
    let (mut controls, steer, _token) = controls("A");
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    assert!(harness().stops_turn_in_place());
    let mut stream = harness()
        .run(request("scenario:stop-in-place"), controls)
        .await
        .expect("run starts");
    async fn next_event(
        stream: &mut futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
    ) -> Option<AgentEvent> {
        tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("event in time")
            .map(|r| r.expect("stream event"))
    }
    loop {
        match next_event(&mut stream).await {
            Some(AgentEvent::TextDelta { text }) if text == "working" => break,
            Some(_) => {}
            None => panic!("stream ended before the turn streamed"),
        }
    }
    assert!(
        turn.background_live(),
        "background_tasks_changed must report the live task"
    );

    turn.stop_turn();
    let stopped = loop {
        match next_event(&mut stream).await {
            Some(done @ AgentEvent::Done { .. }) => break done,
            Some(_) => {}
            None => panic!("a stopped turn must not end the process's stream"),
        }
    };
    assert!(
        matches!(
            stopped,
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                error: None,
                ..
            }
        ),
        "{stopped:?}"
    );
    assert!(
        turn.background_live(),
        "the stop spared the background task"
    );

    // The same process takes the next prompt — with its image inlined.
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("shot.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfake").unwrap();
    steer
        .send(SteerMessage {
            prompt: "carry on".into(),
            message_id: None,
            attachments: vec![image.to_string_lossy().into_owned()],
            config: None,
        })
        .await
        .unwrap();
    let mut resumed = Vec::new();
    loop {
        match next_event(&mut stream).await {
            Some(done @ AgentEvent::Done { .. }) => {
                resumed.push(done);
                break;
            }
            Some(event) => resumed.push(event),
            None => panic!("stream ended before the next turn finished"),
        }
    }
    assert!(
        resumed.contains(&AgentEvent::TextDelta {
            text: "resumed:image=yes".into()
        }),
        "{resumed:?}"
    );
    assert!(
        resumed
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. }))
    );
    assert!(matches!(
        resumed.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert!(!turn.background_live());

    // A stop that lands between turns still settles: the CLI answers and no
    // result follows, so the driver reports the (empty) turn stopped.
    turn.stop_turn();
    assert!(matches!(
        next_event(&mut stream).await,
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));

    drop(steer);
    assert!(
        next_event(&mut stream).await.is_none(),
        "closing the mailbox ends the run"
    );
}

fn done(events: &[AgentEvent]) -> (DoneStatus, Option<String>, Option<String>) {
    match events
        .iter()
        .rev()
        .find(|e| matches!(e, AgentEvent::Done { .. }))
    {
        Some(AgentEvent::Done {
            status,
            result,
            error,
            ..
        }) => (*status, result.clone(), error.clone()),
        _ => panic!("no done: {events:?}"),
    }
}

fn session_lost(events: &[AgentEvent]) -> bool {
    events.iter().any(
        |e| matches!(e, AgentEvent::Error { message } if message.contains("could not be restored")),
    )
}

/// A resumed run against `config_dir` (the CLI's history root).
fn resume_request(session: &str, config_dir: &std::path::Path) -> RunRequest {
    let mut req = request("scenario:resume-check");
    req.resume = Some(session.into());
    req.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        config_dir.to_string_lossy().into_owned(),
    );
    req
}

/// The CLI has no conversation for a resumed session: it says so and
/// exits, and reused forever that id broke the chat for good. The run
/// carries on in a fresh session — the prompt replayed to it — and says so.
#[tokio::test]
async fn an_unrestorable_session_is_relaunched_fresh() {
    // No readable history: only the CLI can tell, and it does.
    let unknown = tempfile::tempdir().unwrap();
    for session in ["stale-1", "stderr-only-1"] {
        let (controls, _steer, _token) = controls("A");
        let events = run_to_end(
            &harness(),
            resume_request(session, &unknown.path().join("missing")),
            controls,
        )
        .await;
        assert!(session_lost(&events), "{session}: {events:?}");
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::SessionStarted { session_id, .. } if session_id == "sess-fresh"
            )),
            "{session}: {events:?}"
        );
        assert_eq!(
            done(&events),
            (DoneStatus::Completed, Some("resumed=none".into()), None),
            "{session}"
        );
    }
}

/// Claude's own history says there is nothing to resume: the CLI is never
/// asked to.
#[tokio::test]
async fn a_session_without_conversation_is_not_resumed() {
    let config = tempfile::tempdir().unwrap();
    let project = config.path().join("projects").join("-tmp");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("stale-2.jsonl"),
        "{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("live-1.jsonl"),
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
    )
    .unwrap();

    let (first, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), resume_request("stale-2", config.path()), first).await;
    assert!(session_lost(&events), "{events:?}");
    assert_eq!(done(&events).1.as_deref(), Some("resumed=none"));

    // A real conversation resumes as ever, without a notice.
    let (again, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), resume_request("live-1", config.path()), again).await;
    assert!(!session_lost(&events), "{events:?}");
    assert_eq!(done(&events).1.as_deref(), Some("resumed=live-1"));

    // No transcript here is no verdict: the CLI may keep its history under
    // another config root (a wrapper setting its own), so it is still asked
    // — and context survives the relaunch.
    let (elsewhere, _steer, _token) = controls("A");
    let events = run_to_end(
        &harness(),
        resume_request("kept-1", config.path()),
        elsewhere,
    )
    .await;
    assert!(!session_lost(&events), "{events:?}");
    assert_eq!(done(&events).1.as_deref(), Some("resumed=kept-1"));
    // Truly gone, the CLI's own answer still starts it fresh.
    let (gone, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), resume_request("stale-3", config.path()), gone).await;
    assert!(session_lost(&events), "{events:?}");
    assert_eq!(done(&events).1.as_deref(), Some("resumed=none"));
}

/// A crash in any turn after the first used to end the stream with no Done
/// at all; and the message must carry the exit and the last stderr line,
/// which land just after stdout closes.
#[tokio::test]
async fn a_crash_in_a_later_turn_ends_it_with_the_reason() {
    let (controls, steer, _token) = controls("A");
    let mut stream = harness()
        .run(request("scenario:crash-later-turn"), controls)
        .await
        .expect("run starts");
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("event in time")
            .expect("first turn ends")
            .expect("stream event");
        let first_done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if first_done {
            break;
        }
    }
    assert_eq!(done(&events).0, DoneStatus::Completed);
    steer.send(steer_msg("keep going")).await.unwrap();
    let rest: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time");
    let (status, _, error) = done(&rest);
    assert_eq!(status, DoneStatus::Errored, "{rest:?}");
    let error = error.expect("crash message");
    assert!(error.contains("exit code 3"), "{error}");
    assert!(error.contains("heap out of memory"), "{error}");
}

/// The CLI waits for an answer to every control request: one we do not
/// serve gets an error reply rather than silence.
#[tokio::test]
async fn unknown_control_requests_are_answered() {
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), request("scenario:unknown-control"), controls).await;
    assert_eq!(done(&events).1.as_deref(), Some("answered"));
}

/// A model switch the CLI refused is shown, and not taken as applied: the
/// next prompt with the same configuration asks again. Refused again the
/// same way, it is not shown a second time.
#[tokio::test]
async fn a_refused_model_switch_is_shown_and_retried() {
    let (controls, steer, _token) = controls("A");
    let mut launch = request("scenario:set-model-fails");
    launch.model = Some("claude-sonnet-5-5".into());
    let mut next = launch.clone();
    next.model = Some("claude-nonexistent".into());
    let switch = |prompt: &str| {
        let mut msg = steer_msg(prompt);
        msg.config = Some(Box::new(next.clone()));
        msg
    };
    let mut stream = harness().run(launch, controls).await.expect("run starts");
    let mut events = Vec::new();
    let mut dones = 0;
    while dones < 2 {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("event in time")
            .expect("stream open")
            .expect("stream event");
        if matches!(event, AgentEvent::Done { .. }) {
            dones += 1;
            if dones == 1 {
                steer.send(switch("second")).await.unwrap();
            }
        }
        events.push(event);
    }
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Error { message }
                if message.contains("model") && message.contains("Model not available")
        )),
        "{events:?}"
    );
    // The same configuration again: the switch is asked for once more.
    steer.send(switch("third")).await.unwrap();
    drop(steer);
    let rest: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time");
    assert_eq!(done(&rest).1.as_deref(), Some("third"), "{rest:?}");
    assert!(
        !rest.iter().any(
            |e| matches!(e, AgentEvent::Error { message } if message.contains("could not change"))
        ),
        "{rest:?}"
    );
}

/// What the CLI started (MCP servers, tools) goes with it when it exits on
/// its own, not only on an interrupt.
#[tokio::test]
async fn processes_the_cli_started_end_with_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let mut req = request("scenario:grandchild");
    req.env.insert(
        "ZERON_TEST_PIDFILE".into(),
        pidfile.to_string_lossy().into_owned(),
    );
    let (controls, _steer, _token) = controls("A");
    let events = run_to_end(&harness(), req, controls).await;
    assert_eq!(done(&events).0, DoneStatus::Completed);
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The stream ends before teardown finishes; give it its grace.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    // SAFETY: signal 0 only probes whether the pid exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        if std::time::Instant::now() > deadline {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("the CLI's child outlived the run");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A result line that will not parse still ends its turn; a lone surrogate
/// half in a delta is repaired rather than dropped.
#[tokio::test]
async fn an_unreadable_result_still_ends_the_turn() {
    let (controls, _steer, _token) = controls("A");
    let mut stream = harness()
        .run(request("scenario:unreadable-result"), controls)
        .await
        .expect("run starts");
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the turn ends without the CLI exiting")
            .expect("stream open")
            .expect("stream event");
        let is_done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if is_done {
            break;
        }
    }
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "half \u{FFFD}".into()
    }));
    assert_eq!(done(&events).0, DoneStatus::Completed);
}

/// Discovery that fails says why: the CLI's exit and its last stderr line,
/// not a bare "exited before answering".
#[tokio::test]
async fn a_failed_discovery_probe_reports_exit_and_stderr() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("claude");
    std::fs::write(
        &exe,
        "#!/bin/sh\necho 'Error: settings.json is not valid JSON' >&2\nexit 2\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = ClaudeHarness::new()
        .with_executable(&exe)
        .commands()
        .await
        .expect_err("probe fails");
    let error = error.to_string();
    assert!(error.contains("exit code 2"), "{error}");
    assert!(error.contains("settings.json is not valid JSON"), "{error}");
}

/// The real CLI, resuming a session it has never seen: it refuses, and the
/// run continues in a fresh session. An invalid key keeps it free — the
/// fresh session fails its API call and ends errored, which is the point:
/// it ran. Run with `cargo test -p zeron-harness --test claude -- --ignored
/// live_unrestorable`.
#[tokio::test]
#[ignore = "spawns the real claude CLI"]
async fn live_unrestorable_session_relaunches_fresh() {
    let config = tempfile::tempdir().unwrap();
    let mut req = request("Reply PONG");
    req.cwd = config.path().to_string_lossy().into_owned();
    req.resume = Some("11111111-2222-3333-4444-555555555555".into());
    // No `projects` dir yet: the history can't be checked, so the CLI is
    // asked to resume and the relaunch path runs.
    req.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        config.path().join("cfg").to_string_lossy().into_owned(),
    );
    req.env
        .insert("ANTHROPIC_API_KEY".into(), "sk-ant-invalid".into());
    let (controls, steer, _token) = controls("A");
    drop(steer);
    let stream = ClaudeHarness::new()
        .run(req, controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(
        Duration::from_secs(60),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time");
    eprintln!("{events:#?}");
    assert!(session_lost(&events));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { session_id, .. }
            if session_id != "11111111-2222-3333-4444-555555555555"
    )));
    assert_eq!(done(&events).0, DoneStatus::Errored);
}
