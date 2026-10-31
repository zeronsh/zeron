//! CursorHarness integration tests against the fake shim in
//! `tests/fixtures/fake-cursor-shim.sh` (no node/@cursor/sdk involved).

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{CancellationToken, CursorHarness, Harness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-cursor-shim.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

fn harness() -> CursorHarness {
    CursorHarness::new().with_executable(fixture_path())
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
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
    }
}

fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
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
    (controls, steer_tx, token)
}

/// Collect events until the first Done (the session parks afterwards).
async fn run_to_first_done(
    harness: &CursorHarness,
    req: RunRequest,
    controls: RunControls,
) -> Vec<AgentEvent> {
    let mut stream = harness.run(req, controls).await.expect("run starts");
    tokio::time::timeout(Duration::from_secs(10), async {
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
    .expect("run finished in time")
}

#[tokio::test]
async fn run_sends_catalog_defaults_when_options_are_implicit() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-default");
    req.model = Some("auto-smart".into());
    let events = run_to_first_done(&harness(), req, controls).await;

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

#[tokio::test]
async fn run_preserves_explicit_options_over_catalog_defaults() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-override");
    req.model = Some("auto-smart".into());
    req.model_options
        .insert("optimize_for".into(), serde_json::json!("intelligence"));
    let events = run_to_first_done(&harness(), req, controls).await;

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

#[tokio::test]
async fn run_preserves_legacy_auto_options_without_duplicate_defaults() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-legacy");
    req.model = Some("auto-smart".into());
    req.model_options
        .insert("optimizeFor".into(), serde_json::json!("speed"));
    let events = run_to_first_done(&harness(), req, controls).await;

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

#[tokio::test]
async fn catalog_failure_does_not_block_an_existing_model_selection() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("shim.sh");
    let source = include_str!("fixtures/fake-cursor-shim.sh").replacen(
        "if [ \"$1\" = \"models\" ]; then",
        "if [ \"$1\" = \"models\" ]; then\n  emit '{\"ev\":\"fatal\",\"message\":\"rate limit\"}'\n  exit 1",
        1,
    );
    std::fs::write(&shim, source).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let harness = CursorHarness::new().with_executable(shim);
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-override");
    req.model = Some("auto-smart".into());
    req.model_options
        .insert("optimize_for".into(), serde_json::json!("intelligence"));
    let events = run_to_first_done(&harness, req, controls).await;

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

#[tokio::test]
async fn model_switch_mid_session_sends_catalog_defaults() {
    let (controls, steer, _token) = controls();
    let mut req = request("scenario:model-switch");
    req.model = Some("auto-smart".into());
    let mut stream = harness().run(req, controls).await.expect("run starts");
    let mut next = request("switch");
    next.model = Some("claude-fable-5".into());
    let mut statuses = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            if let AgentEvent::Done { status, error, .. } = event.unwrap() {
                assert_eq!(status, DoneStatus::Completed, "{error:?}");
                statuses.push(status);
                if statuses.len() == 1 {
                    steer
                        .send(SteerMessage {
                            prompt: "switch".into(),
                            message_id: None,
                            attachments: Vec::new(),
                            config: Some(Box::new(next.clone())),
                        })
                        .await
                        .unwrap();
                } else {
                    break;
                }
            }
        }
    })
    .await
    .expect("switch finished in time");
    assert_eq!(statuses.len(), 2);
}

/// The backend can reject the Max Mode variant its own catalog marks default
/// (Grok 4.7's 500k, verified live), so such models keep the bare id.
#[tokio::test]
async fn max_mode_defaults_are_not_sent() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-maxmode");
    req.model = Some("grok-4.7".into());
    let events = run_to_first_done(&harness(), req, controls).await;

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

#[tokio::test]
async fn runs_use_the_loaded_catalog_without_refreshing_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let probes = dir.path().join("probes");
    let shim = dir.path().join("shim.sh");
    let source = include_str!("fixtures/fake-cursor-shim.sh").replacen(
        "if [ \"$1\" = \"models\" ]; then",
        &format!(
            "if [ \"$1\" = \"models\" ]; then\n  echo probe >> '{}'",
            probes.display()
        ),
        1,
    );
    std::fs::write(&shim, source).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let harness = CursorHarness::new().with_executable(shim);
    harness.models().await.expect("catalog loads");
    for _ in 0..2 {
        let (controls, _steer, _token) = controls();
        let mut req = request("scenario:model-default");
        req.model = Some("auto-smart".into());
        let events = run_to_first_done(&harness, req, controls).await;
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
    assert_eq!(std::fs::read_to_string(&probes).unwrap().lines().count(), 1);
}

#[tokio::test]
async fn happy_path_maps_shim_frames_and_tags_subagents() {
    let (controls, _steer, _token) = controls();
    let events = run_to_first_done(&harness(), request("scenario:happy"), controls).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { harness, model, session_id, .. }
            if *harness == HarnessId::Cursor && model == "composer-2.5" && session_id == "agent-1"
    )));
    assert!(events.contains(&AgentEvent::ReasoningDelta {
        text: "planning".into()
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "Hello from cursor".into()
    }));
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "c1".into(),
        call: ToolCall::Exec {
            command: "ls -la".into()
        },
    }));
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "c1".into(),
        is_error: false,
        output: None,
        diff: None,
    }));

    // The task spawn is a parent chip; its interior arrives tagged, never bare.
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }
            if id == "task1" && name == "Agent: scan repo"
    )));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "task1".into(),
        event: Box::new(AgentEvent::TextDelta {
            text: "sub scanning".into()
        }),
    }));
    assert!(events.contains(&AgentEvent::Subagent {
        parent_tool_use_id: "task1".into(),
        event: Box::new(AgentEvent::ToolCall {
            id: "s1".into(),
            call: ToolCall::Search {
                pattern: "todo".into(),
                path: None,
            },
        }),
    }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { id, .. } if id == "s1")),
        "subagent tool leaked into the parent feed: {events:?}"
    );
    // The task tool's end doubles as the subagent's tagged terminal — the
    // SDK has no separate frame for it, and without this the chip stays
    // "running" forever.
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == "task1"
                && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
    )));

    assert!(events.contains(&AgentEvent::Usage {
        input_tokens: 11,
        output_tokens: 5
    }));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            session_id: Some(id),
            ..
        }) if id == "agent-1"
    ));
}

#[tokio::test]
async fn steer_after_done_becomes_the_next_turn() {
    let (controls, steer, _token) = controls();
    let harness = harness();
    let mut stream = harness
        .run(request("scenario:happy"), controls)
        .await
        .expect("run starts");

    let events = tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        let mut dones = 0;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::Done { .. }) {
                dones += 1;
                if dones == 1 {
                    steer
                        .send(SteerMessage {
                            prompt: "follow up".into(),
                            message_id: None,
                            attachments: Vec::new(),
                            config: None,
                        })
                        .await
                        .expect("steer sent");
                }
            }
            let done = dones >= 2;
            events.push(ev);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("both turns finished in time");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. })),
        "{events:?}"
    );
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "second turn".into()
    }));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Done { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn interrupt_maps_to_interrupted_done() {
    let harness = CursorHarness::new()
        .with_executable(fixture_path())
        .with_graces(Duration::from_millis(200), Duration::from_millis(500));
    let (controls, _steer, token) = controls();
    let mut stream = harness
        .run(request("scenario:interrupt"), controls)
        .await
        .expect("run starts");

    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { .. }) {
                token.cancel();
            }
            let done = matches!(ev, AgentEvent::Done { .. });
            events.push(ev);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("interrupt completed in time");

    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
}

#[tokio::test]
async fn fatal_frame_surfaces_the_auth_fix_as_an_errored_done() {
    let (controls, _steer, _token) = controls();
    let events = run_to_first_done(&harness(), request("scenario:fatal"), controls).await;
    match events.last() {
        Some(AgentEvent::Done {
            status: DoneStatus::Errored,
            error: Some(message),
            ..
        }) => {
            assert!(message.contains("CURSOR_API_KEY"), "{message}");
        }
        other => panic!("expected errored done, got {other:?}"),
    }
}

#[tokio::test]
async fn shim_crash_mid_run_reports_stderr_tail() {
    let (controls, _steer, _token) = controls();
    let events = run_to_first_done(&harness(), request("scenario:crash"), controls).await;
    match events.last() {
        Some(AgentEvent::Done {
            status: DoneStatus::Errored,
            error: Some(message),
            ..
        }) => {
            assert!(
                message.contains("shim exploded") || message.contains("exit code 3"),
                "{message}"
            );
        }
        other => panic!("expected errored done, got {other:?}"),
    }
}

#[tokio::test]
async fn model_discovery_maps_the_live_catalog() {
    let models = harness().models().await.expect("models");
    // Parameterized Auto first; its bare `default` alias twin skipped.
    assert_eq!(models.len(), 3, "{models:?}");
    assert_eq!(models[0].id, "auto-smart");
    assert_eq!(models[0].label, "Auto");
    let optimize = &models[0].options[0];
    assert_eq!(optimize.id, "optimize_for");
    assert_eq!(optimize.label, "Optimize For");
    assert_eq!(
        optimize
            .choices
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["intelligence", "balanced", "cost"]
    );
    // The default comes from the isDefault variant, not the first value.
    assert_eq!(optimize.default_choice, "balanced");
    assert_eq!(models[1].id, "claude-fable-5");
    assert_eq!(models[1].description.as_deref(), Some("Anthropic frontier"));
    // A parameter without displayName labels by id; default = first value.
    assert_eq!(models[1].options[0].id, "thinking");
    assert_eq!(models[1].options[0].default_choice, "enabled");
}

#[tokio::test]
async fn followup_crash_is_not_hidden_by_a_previous_completed_turn() {
    let (controls, steer, _token) = controls();
    let mut stream = harness()
        .run(request("scenario:followup-crash"), controls)
        .await
        .unwrap();
    let dones = tokio::time::timeout(Duration::from_secs(10), async {
        let mut dones = Vec::new();
        while let Some(event) = stream.next().await {
            if let AgentEvent::Done { status, error, .. } = event.unwrap() {
                dones.push((status, error));
                if dones.len() == 1 {
                    steer
                        .send(SteerMessage {
                            prompt: "follow up".into(),
                            message_id: None,
                            attachments: Vec::new(),
                            config: None,
                        })
                        .await
                        .unwrap();
                } else {
                    break;
                }
            }
        }
        dones
    })
    .await
    .unwrap();
    assert_eq!(dones.len(), 2);
    assert_eq!(dones[0].0, DoneStatus::Completed);
    assert_eq!(dones[1].0, DoneStatus::Errored);
    assert!(dones[1].1.as_deref().unwrap().contains("followup exploded"));
}

#[tokio::test]
async fn cancellation_racing_a_fatal_error_emits_exactly_one_terminal_event() {
    let (controls, _steer, token) = controls();
    let mut stream = harness()
        .run(request("scenario:interrupt-fatal"), controls)
        .await
        .unwrap();
    let statuses = tokio::time::timeout(Duration::from_secs(5), async {
        let mut statuses = Vec::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::TextDelta { .. } => token.cancel(),
                AgentEvent::Done { status, .. } => statuses.push(status),
                _ => {}
            }
        }
        statuses
    })
    .await
    .unwrap();
    assert_eq!(statuses, vec![DoneStatus::Interrupted]);
}

#[tokio::test]
async fn steering_spam_preserves_every_turn_in_order_and_closes_cleanly() {
    for _ in 0..10 {
        let (controls, steer, _) = controls();
        let mut stream = harness()
            .run(request("scenario:burst"), controls)
            .await
            .unwrap();
        let producer = tokio::spawn(async move {
            for n in 0..200 {
                steer
                    .send(SteerMessage {
                        prompt: format!("ITEM-{n}"),
                        message_id: None,
                        attachments: Vec::new(),
                        config: None,
                    })
                    .await
                    .unwrap();
            }
        });
        let mut texts = Vec::new();
        let mut dones = 0;
        let mut transitions = 0;
        let mut ids = std::collections::HashSet::new();
        let mut current = None;
        tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::SessionStarted {
                        assistant_message_id,
                        ..
                    } => {
                        ids.insert(assistant_message_id.clone());
                        current = Some(assistant_message_id);
                    }
                    AgentEvent::Steered {
                        assistant_message_id,
                        next_assistant_message_id,
                    } => {
                        assert_eq!(assistant_message_id, current);
                        let next = next_assistant_message_id.unwrap();
                        assert!(ids.insert(next.clone()));
                        current = Some(next);
                        transitions += 1;
                    }
                    AgentEvent::TextDelta { text } => texts.push(text),
                    AgentEvent::Done { status, .. } => {
                        assert_eq!(status, DoneStatus::Completed);
                        dones += 1;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        producer.await.unwrap();
        // Native steering acknowledges every input before starting another turn.
        texts.retain(|text| text != "INITIAL");
        assert_eq!(
            texts,
            (0..200).map(|n| format!("ITEM-{n}")).collect::<Vec<_>>()
        );
        assert!(dones >= 1);
        assert_eq!(transitions, 200);
        assert_eq!(ids.len(), 201);
    }
}

#[tokio::test]
async fn cancelling_a_native_steering_burst_stops_without_starting_another_turn() {
    for _ in 0..20 {
        let (controls, steer, token) = controls();
        let mut stream = harness()
            .run(request("scenario:burst-cancel"), controls)
            .await
            .unwrap();
        for n in 0..100 {
            steer
                .send(SteerMessage {
                    prompt: format!("LIVE-STEER-{n}"),
                    message_id: None,
                    attachments: Vec::new(),
                    config: None,
                })
                .await
                .unwrap();
        }
        token.cancel();
        drop(steer);
        let mut dones = 0;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::Steered { .. } | AgentEvent::TextDelta { .. } => {
                        assert_eq!(dones, 0, "output after cancellation completed")
                    }
                    AgentEvent::Done { status, .. } => {
                        assert_eq!(status, DoneStatus::Interrupted);
                        dones += 1;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(dones, 1);
    }
}

#[tokio::test]
async fn mcp_injection_reaches_shim_on_new_and_resumed_runs() {
    for resume in [None, Some("agent-1")] {
        let mut req = request("scenario:mcp");
        req.resume = resume.map(str::to_owned);
        req.mcp = Some(zeron_proto::McpServer {
            name: "zeron".into(),
            command: "/path with spaces/zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".into(), "origin-chat".into())].into(),
        });
        let (controls, _steer, _token) = controls();
        let events = run_to_first_done(&harness(), req, controls).await;
        assert!(events.contains(&AgentEvent::TextDelta {
            text: "mcp configured".into()
        }));
    }
}

/// A stop the SDK never settles (its cancel wedged) closes the turn at the
/// deadline and retires the shim.
#[tokio::test]
async fn a_stop_the_shim_never_settles_closes_the_turn_at_its_deadline() {
    use zeron_harness::TurnControl;
    let harness = harness().with_stop_timeout(Duration::from_millis(500));
    let (mut controls, _steer, _token) = controls();
    let turn = TurnControl::default();
    controls.turn = turn.clone();
    let mut stream = harness
        .run(request("scenario:stop-wedge"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { .. }) {
                turn.stop_turn();
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("the wedged stop settled and the stream ended");
    let dones: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(dones, [DoneStatus::Interrupted], "{events:?}");
}

#[tokio::test]
async fn attachments_reach_the_shim_as_image_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("shot.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfake").unwrap();
    let image = image.display().to_string();
    let mut req = request("scenario:images");
    req.attachments = vec![image.clone()];
    let (controls, steer, _token) = controls();
    let mut stream = harness().run(req, controls).await.expect("run starts");
    let mut texts = Vec::new();
    let mut dones = 0;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = stream.next().await {
            match ev.expect("stream event") {
                AgentEvent::Done { status, error, .. } => {
                    assert_eq!(status, DoneStatus::Completed, "{error:?}");
                    dones += 1;
                    if dones == 1 {
                        steer
                            .send(SteerMessage {
                                attachments: vec![image.clone()],
                                ..SteerMessage::text("look")
                            })
                            .await
                            .unwrap();
                    } else {
                        break;
                    }
                }
                AgentEvent::TextDelta { text } => texts.push(text),
                _ => {}
            }
        }
    })
    .await
    .expect("both turns finished");
    assert_eq!(texts, ["steer image"]);
}

/// Images over the per-image or per-message inline cap ride as path refs only.
#[tokio::test]
async fn oversized_attachments_are_not_inlined() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big.png");
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(5 * 1024 * 1024, 0);
    std::fs::write(&big, bytes).unwrap();
    let small = dir.path().join("small.png");
    std::fs::write(&small, b"\x89PNG\r\n\x1a\nsmall").unwrap();
    let mut req = request("scenario:image-cap");
    req.attachments = vec![big.display().to_string(), small.display().to_string()];
    let (controls, _steer, _token) = controls();
    let events = run_to_first_done(&harness(), req, controls).await;
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

/// The shim learns the inline image budget and the silent-send bound.
#[tokio::test]
async fn the_run_frame_carries_the_send_bounds() {
    let (controls, _steer, _token) = controls();
    let harness = harness().with_silence_timeout(Duration::from_millis(1234));
    let events = run_to_first_done(&harness, request("scenario:send-bounds"), controls).await;
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
