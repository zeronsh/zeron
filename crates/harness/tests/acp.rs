//! AcpHarness integration tests against the fake ACP agent in
//! `tests/fixtures/fake-acp.sh` (no real `grok` binary involved).

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::acp::SignInProgress;
use zeron_harness::{
    AcpHarness, CancellationToken, Harness, HarnessError, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, SteeringMode,
    TodoItem, ToolCall, UserInputAnswer,
};

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

fn harness() -> AcpHarness {
    AcpHarness::grok().with_executable(fixture_path())
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: Some("grok-4.5".into()),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec!["Yes".into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
    };
    (controls, steer_tx, token)
}

async fn run_to_end(
    harness: &AcpHarness,
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

fn dones(events: &[AgentEvent]) -> Vec<(DoneStatus, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done { status, error, .. } => Some((*status, error.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn happy_path_maps_chunks_tools_diffs_plans_and_commands() {
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&harness(), request("scenario:happy"), controls).await;

    // SessionStarted from session/new's id.
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::SessionStarted { harness, session_id, cwd, .. }
                if *harness == HarnessId::Grok && session_id == "s-1" && cwd == "/tmp"
        )),
        "{events:?}"
    );

    // Initialize-advertised commands surface before the turn.
    let commands: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AvailableCommands { commands } => Some(commands.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 2, "{events:?}");
    assert_eq!(commands[0][0].name, "compact");
    assert_eq!(commands[0][1].input_hint.as_deref(), Some("the goal"));
    // Mid-run advertisement replaces the list.
    assert_eq!(commands[1][0].name, "deep-research");

    // Chunks; the wrong-session and non-text chunks never surface.
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "Hello".into()
    }));
    assert!(events.contains(&AgentEvent::ReasoningDelta {
        text: "thinking".into()
    }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text.contains("WRONG"))),
        "{events:?}"
    );

    // Execute tool: pending opens the call, the completed update resolves it
    // with capped multi-line output (newlines preserved verbatim).
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "t1".into(),
        call: ToolCall::Exec {
            command: "cargo test -p zeron-harness".into()
        },
    }));
    let exec_output = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                is_error: false,
                output: Some(output),
                ..
            } if id == "t1" => Some(output.clone()),
            _ => None,
        })
        .expect("exec output present");
    assert!(exec_output.starts_with("   Compiling zeron-harness"));
    assert_eq!(exec_output.lines().count(), 6, "{exec_output:?}");

    // Edit tool: single-shot completed call carries the inline diff.
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "t2".into(),
        call: ToolCall::EditFile {
            path: "/w/src/resolve.rs".into(),
            old_string: None,
            new_string: None,
        },
    }));
    let diff = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                diff: Some(diff),
                ..
            } if id == "t2" => Some(diff.clone()),
            _ => None,
        })
        .expect("edit diff present");
    assert_eq!(diff.path, "/w/src/resolve.rs");
    assert!(
        diff.old_text
            .as_deref()
            .is_some_and(|t| t.contains(".filter(|p| p.exists())")),
        "{diff:?}"
    );
    assert!(diff.new_text.contains("split_paths"), "{diff:?}");

    // Plan → stable todo chip.
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "acp-plan".into(),
        call: ToolCall::Todo {
            items: vec![
                TodoItem {
                    text: "read".into(),
                    done: true
                },
                TodoItem {
                    text: "fix".into(),
                    done: false
                },
            ]
        },
    }));

    // Context occupancy has a dedicated event; billing usage stays separate.
    assert!(events.contains(&AgentEvent::ContextUsage {
        tokens: Some(1200),
        window: Some(500000)
    }));
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::Usage { .. })));

    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn config_options_apply_requested_model_and_effort() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:config");
    req.reasoning = Some(zeron_proto::ReasoningLevel::Medium);
    let events = run_to_end(&harness(), req, controls).await;
    // The fixture answers refusal unless BOTH set_config_option calls
    // (model grok-4.5, effort medium) arrived before the prompt.
    assert!(
        events.contains(&AgentEvent::TextDelta {
            text: "configured".into()
        }),
        "{events:?}"
    );
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn resumed_first_class_model_is_switched_before_prompt() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:model-api");
    req.resume = Some("existing-grok-session".into());
    let events = run_to_end(&harness(), req, controls).await;
    assert!(
        events.contains(&AgentEvent::TextDelta {
            text: "model switched".into()
        }),
        "{events:?}"
    );
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn permission_requests_auto_accept_the_preferred_allow_option() {
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&harness(), request("scenario:permission"), controls).await;
    // The fixture answers refusal unless the harness selected "always".
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "approved".into()
    }));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn steering_extension_injects_mid_turn() {
    let (controls, steer, _token) = controls();
    let harness = harness();
    let stream = harness
        .run(request("scenario:steer-ext"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "first") {
                steer
                    .send(SteerMessage {
                        prompt: "redirect please".into(),
                        message_id: None,
                    })
                    .await
                    .expect("steer sent");
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("run finished in time");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. })),
        "{events:?}"
    );
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "steered".into()
    }));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

/// The steering response racing the turn's own end: the injection landed in
/// the dying turn, and the prompt response reached the wire first. The
/// boundary must still be emitted BEFORE the Done — a Steered after Done
/// re-armed the consumer (parked session → Working) with no next turn and no
/// Done ever coming (the stranded-Working / eternal-timer bug).
#[tokio::test]
async fn steer_racing_the_turn_end_never_emits_steered_after_done() {
    let (controls, steer, _token) = controls();
    let harness = harness();
    let stream = harness
        .run(request("scenario:steer-race"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "first") {
                steer
                    .send(SteerMessage {
                        prompt: "redirect please".into(),
                        message_id: None,
                    })
                    .await
                    .expect("steer sent");
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("run finished in time");

    assert_eq!(
        dones(&events),
        vec![(DoneStatus::Completed, None)],
        "{events:?}"
    );
    let steered = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("steer landed in the turn: a Steered boundary must exist");
    let done = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Done { .. }))
        .expect("checked above");
    assert!(
        steered < done,
        "Steered after Done strands the session: {events:?}"
    );
}

#[tokio::test]
async fn rejected_steer_queues_and_delivers_at_the_turn_boundary() {
    let (controls, steer, _token) = controls();
    let harness = harness();
    let stream = harness
        .run(request("scenario:steer-queue"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        let mut steer = Some(steer);
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "first")
                && let Some(steer) = &steer
            {
                steer
                    .send(SteerMessage {
                        prompt: "redirect please".into(),
                        message_id: None,
                    })
                    .await
                    .expect("steer sent");
            }
            // Close the mailbox once the boundary turn streams so the
            // persistent session winds down and the stream ends.
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "boundary") {
                steer = None;
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("run finished in time");

    // First turn completes, then the queued steer becomes the boundary turn.
    assert_eq!(
        dones(&events),
        vec![(DoneStatus::Completed, None), (DoneStatus::Completed, None)],
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. })),
        "{events:?}"
    );
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "boundary".into()
    }));
}

#[tokio::test]
async fn interrupt_sends_session_cancel_and_ends_interrupted() {
    let (controls, _steer, token) = controls();
    let harness = harness();
    let stream = harness
        .run(request("scenario:interrupt"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "working") {
                token.cancel();
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("run finished in time");
    assert_eq!(dones(&events), vec![(DoneStatus::Interrupted, None)]);
}

#[tokio::test]
async fn wedged_agent_escalates_to_signals_and_still_ends_interrupted() {
    let (controls, _steer, token) = controls();
    let harness = harness().with_graces(Duration::from_millis(100), Duration::from_millis(200));
    let stream = harness
        .run(request("scenario:wedge"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "working") {
                token.cancel();
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("escalation reaped the child in time");
    let dones = dones(&events);
    assert_eq!(dones.len(), 1, "{events:?}");
    assert_eq!(dones[0].0, DoneStatus::Interrupted);
}

#[tokio::test]
async fn refusal_maps_to_an_errored_done() {
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&harness(), request("scenario:refusal"), controls).await;
    let dones = dones(&events);
    assert_eq!(dones.len(), 1);
    assert_eq!(dones[0].0, DoneStatus::Errored);
    assert!(dones[0].1.as_deref().unwrap_or("").contains("refused"));
}

#[tokio::test]
async fn resume_loads_the_session_and_drops_replayed_history() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:resumed");
    req.resume = Some("s-loaded".into());
    let events = run_to_end(&harness(), req, controls).await;
    // The 600-update replay is drained without surfacing…
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text.contains("old reply"))),
        "{events:?}"
    );
    // …the loaded session id sticks, and the live turn still streams.
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { session_id, .. } if session_id == "s-loaded"
    )));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "back again".into()
    }));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}
#[test]
fn descriptor_surface_matches_registry_expectations() {
    let harness = AcpHarness::grok();
    assert_eq!(harness.id(), HarnessId::Grok);
    assert_eq!(harness.display_name(), "Grok");
    assert!(harness.supports_steering());
    assert_eq!(harness.steering_mode(), SteeringMode::TurnBoundary);
    assert_eq!(
        harness.reasoning_levels(),
        &[
            zeron_proto::ReasoningLevel::Low,
            zeron_proto::ReasoningLevel::Medium,
            zeron_proto::ReasoningLevel::High,
        ]
    );
}

#[tokio::test]
async fn models_are_discovered_from_the_acp_session() {
    // ACP is the source of truth: the fixture advertises a model config
    // option, so the picker list comes from the wire, not the static catalog.
    let harness = AcpHarness::hermes().with_executable(fixture_path());
    let models = harness.models().await.expect("discovery");
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["grok-4-fast", "grok-4.5"], "{models:?}");
    // Unmatched ids inherit the probe session's thought_level ladder.
    assert_eq!(
        models[0].reasoning_levels,
        vec![
            zeron_proto::ReasoningLevel::Low,
            zeron_proto::ReasoningLevel::Medium,
            zeron_proto::ReasoningLevel::High,
        ],
        "{models:?}"
    );
    assert_eq!(models[0].description.as_deref(), Some("Fast tier"));
    // Cached: a second call returns the same list without respawning.
    let again = harness.models().await.expect("cached");
    assert_eq!(again, models);
}

#[tokio::test]
async fn models_enrich_from_the_static_catalog_on_id_match() {
    // grok's static catalog knows "grok-4.5" — the discovered entry keeps the
    // wire label but inherits the curated description and ladder.
    let harness = AcpHarness::grok().with_executable(fixture_path());
    let models = harness.models().await.expect("discovery");
    let grok45 = models
        .iter()
        .find(|m| m.id == "grok-4.5")
        .expect("grok-4.5");
    assert_eq!(
        grok45.description.as_deref(),
        Some("xAI's coding model — 500k context"),
        "{grok45:?}"
    );
}

#[tokio::test]
async fn models_fall_back_to_the_static_catalog_when_the_probe_fails() {
    // An agent that exists but cannot speak ACP: the discovery probe fails
    // after launch, and the static catalog is served instead.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken-pi-acp");
    std::fs::write(&broken, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    let harness = AcpHarness::pi().with_executable(broken);
    assert!(harness.installed());
    let models = harness.models().await.expect("static fallback");
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["default"], "{models:?}");
}

#[tokio::test]
async fn missing_override_is_not_installed_and_fails_discovery() {
    // An override that points at nothing is not an installed agent: the
    // registry must not offer it, and discovery names the problem instead of
    // quietly serving a catalog for a binary that can never launch.
    let harness = AcpHarness::pi().with_executable("/nonexistent/never-a-pi-acp");
    assert!(!harness.installed());
    let err = harness.models().await.expect_err("missing override");
    assert!(
        matches!(err, zeron_harness::HarnessError::NotInstalled(_)),
        "{err:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hung_handshake_errors_instead_of_spinning_forever() {
    // An agent that consumes stdin and never answers initialize — the
    // "thinking for minutes, then nothing" startup class (issue #93). The
    // run must end with a Done that names the timeout, not hang.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("hung-agent.sh");
    // sleep inherits the stdio pipes and holds them open without ever
    // answering — a true wedge, not a crash.
    std::fs::write(&script, "#!/bin/sh\nexec sleep 1000\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let harness = AcpHarness::grok()
        .with_executable(&script)
        .with_handshake_timeout(Duration::from_millis(300));
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&harness, request("hi"), controls).await;
    let dones = dones(&events);
    assert_eq!(dones.len(), 1, "{events:?}");
    let (status, error) = &dones[0];
    assert_eq!(*status, DoneStatus::Errored);
    let error = error.as_deref().unwrap_or_default();
    assert!(
        error.contains("did not complete the ACP handshake"),
        "{error}"
    );
}
#[test]
fn hermes_and_pi_descriptor_surfaces_match_registry_expectations() {
    let devin = AcpHarness::devin();
    assert_eq!(devin.id(), HarnessId::Devin);
    assert_eq!(devin.display_name(), "Devin");
    assert!(devin.supports_steering());
    assert_eq!(devin.steering_mode(), SteeringMode::TurnBoundary);
    assert!(devin.reasoning_levels().is_empty());

    let hermes = AcpHarness::hermes();
    assert_eq!(hermes.id(), HarnessId::Hermes);
    assert_eq!(hermes.display_name(), "Hermes");
    assert!(hermes.supports_steering());
    assert_eq!(hermes.steering_mode(), SteeringMode::TurnBoundary);
    assert!(hermes.reasoning_levels().is_empty());

    let pi = AcpHarness::pi();
    assert_eq!(pi.id(), HarnessId::Pi);
    assert_eq!(pi.display_name(), "Pi");
    assert!(pi.supports_steering());
    assert_eq!(pi.steering_mode(), SteeringMode::TurnBoundary);
    assert_eq!(
        pi.reasoning_levels(),
        &[
            zeron_proto::ReasoningLevel::Minimal,
            zeron_proto::ReasoningLevel::Low,
            zeron_proto::ReasoningLevel::Medium,
            zeron_proto::ReasoningLevel::High,
            zeron_proto::ReasoningLevel::XHigh,
            zeron_proto::ReasoningLevel::Max,
        ]
    );
}

fn antigravity_harness() -> AcpHarness {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-antigravity-acp.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    AcpHarness::antigravity().with_executable(path)
}

async fn antigravity_config_sets(
    model: &str,
    reasoning: Option<ReasoningLevel>,
) -> Vec<AgentEvent> {
    let workspace = tempfile::tempdir().unwrap();
    let mut req = request("hi");
    req.model = Some(model.into());
    req.reasoning = reasoning;
    req.cwd = workspace.path().display().to_string();
    let (controls, _steer, _token) = controls();
    run_to_end(&antigravity_harness(), req, controls).await
}

#[tokio::test]
async fn antigravity_sign_in_reports_the_browser_url_and_authenticates() {
    let seen: std::sync::Arc<std::sync::Mutex<Vec<SignInProgress>>> = Default::default();
    let recorder = seen.clone();
    antigravity_harness()
        .sign_in(None, move |progress| {
            recorder.lock().unwrap().push(progress);
        })
        .await
        .expect("signed in");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![SignInProgress::OpenBrowser(
            "https://accounts.google.com/o/oauth2/auth?client_id=fake".into()
        )]
    );
}

fn devin_auth_fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-devin-auth.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    path
}

/// Zeron's "Add account" for Devin: an explicit method (Devin has no
/// default), a throwaway data home the new login lands in, and a url filter
/// that skips the handshake's unrelated link for the real sign-in page.
#[tokio::test]
async fn devin_sign_in_runs_the_given_method_in_the_given_environment() {
    let data = tempfile::tempdir().unwrap();
    let seen: std::sync::Arc<std::sync::Mutex<Vec<SignInProgress>>> = Default::default();
    let recorder = seen.clone();
    AcpHarness::devin()
        .with_executable(devin_auth_fixture())
        .sign_in_with(
            zeron_harness::acp::SignInOptions {
                method: Some("devin-browser".into()),
                env: vec![("XDG_DATA_HOME".into(), data.path().into())],
                url_filter: Some(|url| url.contains("redirect_uri=")),
                ..Default::default()
            },
            move |progress| recorder.lock().unwrap().push(progress),
        )
        .await
        .expect("signed in");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![SignInProgress::OpenBrowser(
            "https://app.devin.ai/auth/cli/continue?redirect_uri=http%3A%2F%2F127.0.0.1%3A45678%2Fcallback&state=s"
                .into()
        )]
    );
    assert!(data.path().join("devin/credentials.toml").is_file());
}

#[tokio::test]
async fn an_agent_without_a_sign_in_method_refuses_a_default_sign_in() {
    let error = AcpHarness::devin()
        .with_executable(devin_auth_fixture())
        .sign_in(None, |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no sign-in flow"), "{error}");
}

/// `cli_command` runs the agent's CLI itself: the same program a launch
/// resolves, without the ACP server arguments.
#[cfg(not(windows))]
#[tokio::test]
async fn cli_command_runs_the_cli_without_the_server_arguments() {
    let fixture = fixture_path();
    let command = AcpHarness::grok()
        .with_executable(&fixture)
        .cli_command(&["login", "--device-auth"])
        .await
        .unwrap();
    let std = command.as_std();
    assert_eq!(std.get_program(), fixture.as_os_str());
    let args: Vec<_> = std
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["login", "--device-auth"]);
}

#[tokio::test]
async fn antigravity_commands_include_logout() {
    let commands = antigravity_harness().commands().await.expect("commands");
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"plan"), "{names:?}");
    assert!(names.contains(&"logout"), "{names:?}");
}

#[tokio::test]
async fn antigravity_sign_out_logs_the_server_out() {
    antigravity_harness().sign_out().await.expect("signed out");
}

#[tokio::test]
async fn antigravity_run_without_sign_in_points_to_settings_instead_of_a_browser() {
    let workspace = tempfile::Builder::new()
        .prefix("needs-login")
        .tempdir()
        .unwrap();
    let mut req = request("hi");
    req.model = None;
    req.cwd = workspace.path().display().to_string();
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&antigravity_harness(), req, controls).await;
    let dones = dones(&events);
    assert_eq!(dones.len(), 1, "{events:?}");
    assert_eq!(dones[0].0, DoneStatus::Errored);
    let error = dones[0].1.as_deref().unwrap_or_default();
    assert!(
        error.contains("Settings → Providers → Antigravity and connect an account"),
        "{error}"
    );
}

#[test]
fn antigravity_descriptor_surface_matches_registry_expectations() {
    let antigravity = AcpHarness::antigravity();
    assert_eq!(antigravity.id(), HarnessId::Antigravity);
    assert_eq!(antigravity.display_name(), "Antigravity");
    assert!(antigravity.supports_steering());
    assert_eq!(antigravity.steering_mode(), SteeringMode::TurnBoundary);
    assert!(antigravity.reasoning_levels().is_empty());
}

#[tokio::test]
async fn antigravity_runs_the_picked_effort_variant_unattended() {
    let events = antigravity_config_sets("gemini-3.7-flash", Some(ReasoningLevel::Medium)).await;
    assert!(
        events.contains(&AgentEvent::TextDelta {
            text: "sets:model=gemini-3.7-flash-medium;mode=yolo;".into()
        }),
        "{events:?}"
    );
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn antigravity_stops_before_prompt_when_the_model_is_rejected() {
    let workspace = tempfile::Builder::new()
        .prefix("reject-model")
        .tempdir()
        .unwrap();
    let mut req = request("hi");
    req.model = Some("gemini-3.7-flash".into());
    req.reasoning = Some(ReasoningLevel::Medium);
    req.cwd = workspace.path().display().to_string();
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&antigravity_harness(), req, controls).await;

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { .. })),
        "{events:?}"
    );
    let done = dones(&events);
    assert_eq!(done.len(), 1, "{events:?}");
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(
        done[0]
            .1
            .as_deref()
            .is_some_and(|error| error.contains("rejected requested model")),
        "{events:?}"
    );
}

#[tokio::test]
async fn antigravity_clamps_to_an_offered_level_and_keeps_saved_variant_ids() {
    let clamped = antigravity_config_sets("gemini-3.1-pro", Some(ReasoningLevel::Medium)).await;
    assert!(
        clamped.contains(&AgentEvent::TextDelta {
            text: "sets:model=gemini-pro-agent;mode=yolo;".into()
        }),
        "{clamped:?}"
    );
    let saved = antigravity_config_sets("gemini-3.7-flash-low", Some(ReasoningLevel::High)).await;
    assert!(
        saved.contains(&AgentEvent::TextDelta {
            text: "sets:model=gemini-3.7-flash-low;mode=yolo;".into()
        }),
        "{saved:?}"
    );
}

#[tokio::test]
async fn antigravity_models_group_effort_variants_into_one_row() {
    let models = antigravity_harness().models().await.expect("discovery");
    let rows: Vec<(&str, &str, &[ReasoningLevel])> = models
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                m.label.as_str(),
                m.reasoning_levels.as_slice(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "gemini-3.7-flash",
                "Gemini 3.7 Flash",
                &[
                    ReasoningLevel::Low,
                    ReasoningLevel::Medium,
                    ReasoningLevel::High
                ][..]
            ),
            (
                "gemini-3.1-pro",
                "Gemini 3.1 Pro",
                &[ReasoningLevel::Low, ReasoningLevel::High][..]
            ),
        ]
    );
    assert!(models.iter().all(|m| m.description.is_none()), "{models:?}");
}

#[tokio::test]
async fn devin_spec_drives_the_shared_acp_wire() {
    let devin = AcpHarness::devin().with_executable(fixture_path());
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&devin, request("scenario:happy"), controls).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SessionStarted { harness, .. } if *harness == HarnessId::Devin
    )));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "Hello".into()
    }));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn prompt_complete_extension_settles_a_hung_prompt_response() {
    // The grok field hang: `_x.ai/session/prompt_complete` fires (echoing
    // the minted _meta.promptId) but the session/prompt RPC never answers.
    let (controls, _steer, _token) = controls();
    let mut stream = harness()
        .run(request("scenario:prompt-complete-hang"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async {
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
    .expect("notification settled the turn despite the hung RPC");
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "pong".into()
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
async fn stale_prompt_complete_never_settles_a_newer_turn() {
    let (controls, _steer, _token) = controls();
    let mut stream = harness()
        .run(request("scenario:prompt-complete-stale"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(10), async {
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
    .expect("real response settled the turn");
    // Exactly one Done, AFTER the real content — the stale/foreign
    // completions (emitted before the 1s pause) must not have settled first,
    // and must not have marked the turn Interrupted.
    let text = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "real answer"))
        .expect("real content precedes the settle");
    let done = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Done { .. }))
        .expect("done");
    assert!(text < done, "{events:?}");
    assert!(matches!(
        &events[done],
        AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        }
    ));
    // Grok-style `_meta` usage on the response is captured.
    assert!(events.contains(&AgentEvent::Usage {
        input_tokens: 9,
        output_tokens: 4
    }));
}

#[tokio::test]
async fn grok_subagent_lifecycle_tails_the_disk_transcript_into_tagged_events() {
    // The child session's chat_history.jsonl, one level under the sessions
    // root exactly like grok's `<root>/<urlencoded-cwd>/<session-id>/` layout
    // (entry shapes captured from a real 1.0.4 run).
    let tmp = tempfile::tempdir().unwrap();
    let child_dir = tmp.path().join("%2Ftmp").join("sub-1");
    std::fs::create_dir_all(&child_dir).unwrap();
    let history = child_dir.join("chat_history.jsonl");
    std::fs::write(
        &history,
        concat!(
            "{\"type\":\"system\",\"content\":\"You are a Grok Build subagent\"}\n",
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Count the files.\"}],\"prompt_index\":0}\n",
            "{\"type\":\"reasoning\",\"id\":\"rs-1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Listing the directory.\"}],\"encrypted_content\":\"opaque\",\"status\":\"completed\"}\n",
            "{\"type\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"id\":\"call-1-0\",\"name\":\"run_terminal_command\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}],\"model_id\":\"grok-4.6-build\"}\n",
            "{\"type\":\"tool_result\",\"tool_call_id\":\"call-1-0\",\"content\":\"a.txt\\nb.txt\"}\n",
        ),
    )
    .unwrap();
    // A mid-run append: the tail must pick it up incrementally, before the
    // wire's subagent_finished lands (the fake agent sleeps 1.4s).
    let append_to = history.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(append_to)
            .unwrap();
        writeln!(
            f,
            "{}",
            "{\"type\":\"assistant\",\"content\":\"two files\",\"model_id\":\"grok-4.6-build\"}"
        )
        .unwrap();
    });

    let (controls, _steer, _token) = controls();
    let harness = harness().with_sessions_root(tmp.path());
    let events = run_to_end(&harness, request("scenario:subagent"), controls).await;

    // The spawn chip is named after the task, claude-driver parity.
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }
                if id == "sp1" && name == "Agent: Count files"
        )),
        "{events:?}"
    );

    // Tagged transcript: every wrapped event attributes to the spawn chip,
    // and the disk entries surfaced in order — reasoning, the typed tool
    // call + result, the mid-run append — then the lifecycle Done.
    let tagged: Vec<&AgentEvent> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } => {
                assert_eq!(parent_tool_use_id, "sp1", "{events:?}");
                Some(event.as_ref())
            }
            _ => None,
        })
        .collect();
    let pos = |pred: &dyn Fn(&AgentEvent) -> bool| tagged.iter().position(|e| pred(e));
    let reasoning = pos(&|e| {
        matches!(e, AgentEvent::ReasoningDelta { text } if text.starts_with("Listing the directory."))
    })
    .expect("reasoning entry tailed");
    let tool = pos(&|e| {
        matches!(
            e,
            AgentEvent::ToolCall { id, call: zeron_proto::ToolCall::Exec { command } }
                if id == "call-1-0" && command == "ls"
        )
    })
    .expect("tool call typed from disk");
    let result = pos(&|e| {
        matches!(
            e,
            AgentEvent::ToolResult { id, is_error: false, output: Some(o), .. }
                if id == "call-1-0" && o.contains("a.txt")
        )
    })
    .expect("tool result tailed");
    let text =
        pos(&|e| matches!(e, AgentEvent::TextDelta { text } if text.starts_with("two files")))
            .expect("mid-run append tailed");
    let done = pos(&|e| {
        matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )
    })
    .expect("tagged done from subagent_finished");
    assert!(
        reasoning < tool && tool < result && result < text && text < done,
        "{tagged:?}"
    );
    // The nested spawned update (another parent session) bound nothing —
    // every wrapped event attributed to sp1 (the assert in the filter) — and
    // the parent's own turn settled cleanly with its single untagged Done.
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[cfg(unix)]
fn devin_fixture() -> (tempfile::TempDir, AcpHarness) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("devin.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, pathlib, sys, threading, time
root = pathlib.Path(__file__).parent

def emit(frame):
    print(json.dumps(dict(jsonrpc='2.0', **frame)), flush=True)

def config(model):
    return [{'id':'model', 'category':'model', 'type':'select',
             'currentValue':'gpt-old', 'options':[{'value':model, 'name':model}]}]

if sys.argv[1:] == ['models', 'list', '--format', 'json']:
    with (root / 'probes').open('a') as f: f.write('probe\n')
    state = (root / 'state').read_text()
    if state == 'hang': time.sleep(60)
    if state == 'error':
        print('account unavailable', file=sys.stderr)
        sys.exit(1)
    time.sleep(0.1)
    print(json.dumps({'families':[{'variants':[{'model_uid':state, 'label':state}]}]}))
    sys.exit(0)
assert sys.argv[1:] == ['acp'], sys.argv
selected = None

def refresh():
    # An unrelated session must not satisfy the requested-model wait.
    emit({'method':'session/update', 'params':{'sessionId':'other', 'update':{
        'sessionUpdate':'config_option_update', 'configOptions':config('gpt-new')}}})
    time.sleep(0.1)
    (root / 'refreshed').touch()
    emit({'method':'session/update', 'params':{'sessionId':'s-1', 'update':{
        'sessionUpdate':'config_option_update', 'configOptions':config('gpt-new')}}})

for line in sys.stdin:
    req = json.loads(line)
    method = req.get('method')
    result = {}
    if method == 'initialize': result = {'protocolVersion':1, 'agentCapabilities':{}}
    elif method == 'session/new':
        result = {'sessionId':'s-1', 'configOptions':config('gpt-old')}
    elif method == 'session/set_config_option':
        selected = req['params']['value']
        assert (root / 'refreshed').exists(), 'selected before own session refreshed'
        if (root / 'state').read_text() == 'reject':
            emit({'id':req['id'], 'error':{'code':-32602, 'message':'model unavailable'}})
            continue
        assert selected == 'gpt-new', selected
    elif method == 'session/prompt':
        (root / 'prompted').write_text(selected or 'default')
        result = {'stopReason':'end_turn'}
    early = (root / 'state').read_text() == 'early'
    if method == 'session/new' and early: refresh()
    emit({'id':req['id'], 'result':result})
    if method == 'session/new' and not early: threading.Thread(target=refresh, daemon=True).start()
    if method == 'session/prompt': break
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("state"), "gpt-old").unwrap();
    let harness = AcpHarness::devin().with_executable(script);
    (dir, harness)
}

#[cfg(unix)]
#[tokio::test]
async fn devin_models_refresh_between_calls_and_coalesce_overlapping_probes() {
    let (dir, harness) = devin_fixture();
    let (first, overlap) = tokio::join!(harness.models(), harness.models());
    assert_eq!(first.unwrap()[0].id, "gpt-old");
    assert_eq!(overlap.unwrap()[0].id, "gpt-old");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("probes"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::write(dir.path().join("state"), "gpt-new").unwrap();
    assert_eq!(harness.models().await.unwrap()[0].id, "gpt-new");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("probes"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[cfg(unix)]
#[tokio::test]
async fn devin_discovery_errors_and_timeouts_retain_last_good_then_recover() {
    let (dir, harness) = devin_fixture();
    let harness = harness.with_model_discovery_timeout(Duration::from_millis(500));
    assert_eq!(harness.models().await.unwrap()[0].id, "gpt-old");
    std::fs::write(dir.path().join("state"), "error").unwrap();
    assert_eq!(harness.models().await.unwrap()[0].id, "gpt-old");
    std::fs::write(dir.path().join("state"), "hang").unwrap();
    assert_eq!(harness.models().await.unwrap()[0].id, "gpt-old");
    std::fs::write(dir.path().join("state"), "gpt-new").unwrap();
    assert_eq!(harness.models().await.unwrap()[0].id, "gpt-new");
}

#[cfg(unix)]
#[tokio::test]
async fn devin_waits_for_refreshed_variant_before_prompting() {
    let (dir, harness) = devin_fixture();
    let (controls, _steer, _token) = controls();
    let mut req = request("Say hello");
    req.model = Some("gpt-new".into());
    let events = run_to_end(&harness, req, controls).await;
    assert_eq!(
        dones(&events),
        vec![(DoneStatus::Completed, None)],
        "{events:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("prompted")).unwrap(),
        "gpt-new"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn devin_rejected_model_never_prompts_with_a_different_model() {
    let (dir, harness) = devin_fixture();
    std::fs::write(dir.path().join("state"), "reject").unwrap();
    let (controls, _steer, _token) = controls();
    let mut req = request("Say hello");
    req.model = Some("gpt-new".into());
    let events = run_to_end(&harness, req, controls).await;
    assert!(
        dones(&events)
            .iter()
            .any(|(status, error)| *status == DoneStatus::Errored
                && error
                    .as_deref()
                    .is_some_and(|e| e.contains("model unavailable"))),
        "{events:?}"
    );
    assert!(!dir.path().join("prompted").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn devin_keeps_model_refresh_that_precedes_session_response() {
    let (dir, harness) = devin_fixture();
    std::fs::write(dir.path().join("state"), "early").unwrap();
    let (controls, _steer, _token) = controls();
    let mut req = request("Say hello");
    req.model = Some("gpt-new".into());
    let events = run_to_end(&harness, req, controls).await;
    assert_eq!(
        dones(&events),
        vec![(DoneStatus::Completed, None)],
        "{events:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("prompted")).unwrap(),
        "gpt-new"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn devin_missing_variant_is_bounded_and_never_prompts() {
    let (dir, harness) = devin_fixture();
    let harness = harness.with_handshake_timeout(Duration::from_millis(300));
    let (controls, _steer, _token) = controls();
    let mut req = request("Say hello");
    req.model = Some("gpt-missing".into());
    let events = run_to_end(&harness, req, controls).await;
    assert!(
        dones(&events)
            .iter()
            .any(|(status, _)| *status == DoneStatus::Errored),
        "{events:?}"
    );
    assert!(!dir.path().join("prompted").exists());
}

#[test]
fn antigravity_sign_in_preserves_relative_home_auth_in_a_separate_process() {
    let parent_cwd = tempfile::tempdir().unwrap();
    let child_home = tempfile::tempdir().unwrap();
    let gemini_home = child_home.path().join("relative-gemini-home");
    let settings = gemini_home.join("antigravity-acp");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("settings.json"),
        r#"{"auth":{"type":"oauth-business"}}"#,
    )
    .unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "antigravity_auth_path_subprocess", "--nocapture"])
        .current_dir(parent_cwd.path())
        .env("HOME", child_home.path())
        .env("GEMINI_HOME", "relative-gemini-home")
        .env("ZERON_TEST_EXPECTED_GEMINI_HOME", &gemini_home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[tokio::test]
async fn antigravity_auth_path_subprocess() {
    if std::env::var_os("ZERON_TEST_EXPECTED_GEMINI_HOME").is_none() {
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fake-antigravity-auth-path.sh");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        AcpHarness::antigravity()
            .with_executable(fixture)
            .sign_in(None, |_| {}),
    )
    .await
    .expect("sign-in timed out")
    .expect("configured business sign-in");
}

#[tokio::test]
async fn antigravity_wedge_emits_one_interrupted_done() {
    let (controls, _steer, token) = controls();
    let harness = AcpHarness::antigravity().with_executable(fixture_path());
    let stream = harness
        .run(request("scenario:wedge"), controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(Duration::from_secs(6), async move {
        let mut events = Vec::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.expect("stream event");
            if matches!(ev, AgentEvent::TextDelta { ref text } if text == "working") {
                token.cancel();
            }
            events.push(ev);
        }
        events
    })
    .await
    .expect("escalation reaped the child in time");
    let dones = dones(&events);
    assert_eq!(dones.len(), 1, "{events:?}");
    assert_eq!(dones[0].0, DoneStatus::Interrupted);
}

#[tokio::test]
async fn antigravity_load_and_prompt_auth_expiry_point_to_sign_in() {
    for resume in [false, true] {
        let workspace = tempfile::Builder::new()
            .prefix("prompt-auth")
            .tempdir()
            .unwrap();
        let mut req = request("hi");
        req.model = None;
        req.cwd = workspace.path().display().to_string();
        req.resume = resume.then(|| "expired-session".into());
        let (controls, _, _) = controls();
        let events = run_to_end(&antigravity_harness(), req, controls).await;
        let done = dones(&events);
        assert_eq!(done.len(), 1, "{events:?}");
        assert_eq!(done[0].0, DoneStatus::Errored);
        assert!(
            done[0]
                .1
                .as_deref()
                .unwrap()
                .contains("Settings → Providers → Antigravity and connect an account"),
            "{events:?}"
        );
    }
}

#[tokio::test]
async fn antigravity_empty_reply_completes_once() {
    let workspace = tempfile::Builder::new()
        .prefix("empty-reply")
        .tempdir()
        .unwrap();
    let mut req = request("hi");
    req.model = None;
    req.cwd = workspace.path().display().to_string();
    let (controls, _, _) = controls();
    let events = run_to_end(&antigravity_harness(), req, controls).await;
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn antigravity_unknown_saved_model_fails_clearly() {
    let events = antigravity_config_sets("unknown-saved-model", None).await;
    let done = dones(&events);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(
        done[0]
            .1
            .as_deref()
            .unwrap()
            .contains("unknown-saved-model")
    );
}

#[tokio::test]
async fn antigravity_spawn_failure_returns_an_error_for_the_engine_to_surface() {
    let workspace = tempfile::tempdir().unwrap();
    let missing = workspace.path().join("missing-acp-server");
    let harness = AcpHarness::antigravity().with_executable(&missing);
    let (controls, _, _) = controls();
    let result = harness.run(request("hi"), controls).await;
    let Err(error) = result else {
        panic!("spawn failure must reach the engine");
    };
    assert!(error.to_string().contains("missing-acp-server"), "{error}");
}

fn pi_fixture() -> AcpHarness {
    AcpHarness::pi()
        .with_executable(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-pi-acp.sh"),
        )
        .with_graces(Duration::from_millis(100), Duration::from_millis(150))
}

#[tokio::test]
async fn pi_crash_reports_status_and_stderr_once() {
    for (prompt, status, tail) in [
        ("crash", "exit code 23", "last stderr context"),
        ("signal-crash", "signal 9", "signal context"),
        (
            "inherited-pipe-crash",
            "exit code 25",
            "inherited pipe context",
        ),
    ] {
        let (controls, _steer, _) = controls();
        let events = run_to_end(&pi_fixture(), request(prompt), controls).await;
        let done = dones(&events);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, DoneStatus::Errored);
        let error = done[0].1.as_deref().unwrap();
        assert!(error.contains(status) && error.contains(tail), "{error}");
    }
}

#[tokio::test]
async fn pi_idle_crash_then_load_preserves_session() {
    let (ctl, _steer, _) = controls();
    let events = run_to_end(&pi_fixture(), request("idle-crash"), ctl).await;
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
    let session = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Done { session_id, .. } => session_id.clone(),
            _ => None,
        })
        .unwrap();
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("resumed");
    req.resume = Some(session);
    let events = run_to_end(&pi_fixture(), req, ctl).await;
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "reply:resumed".into()
    }));
}

#[tokio::test]
async fn pi_failed_load_announces_lost_context() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("fresh");
    req.resume = Some("missing".into());
    let events = run_to_end(&pi_fixture(), req, ctl).await;
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message } if message.contains("without the previous context"))));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn pi_frames_and_model_effort_round_trip() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("frames");
    req.model = Some("mock/model".into());
    req.reasoning = Some(ReasoningLevel::Max);
    let events = run_to_end(&pi_fixture(), req, ctl).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text.len() == 1024 * 1024 + 17))
    );
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
    assert_eq!(pi_fixture().models().await.unwrap()[0].id, "mock/model");
}

#[tokio::test]
async fn pi_error_stop_reason_is_failed() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let events = run_to_end(&pi_fixture(), request("error"), ctl).await;
    assert_eq!(dones(&events)[0].0, DoneStatus::Errored);
}

#[tokio::test]
async fn pi_interrupt_error_and_duplicate_terminal_settle_once() {
    let (ctl, _steer, token) = controls();
    let mut stream = pi_fixture()
        .run(request("interrupt-error"), ctl)
        .await
        .unwrap();
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if matches!(&event, AgentEvent::TextDelta { text } if text == "working") {
                token.cancel();
            }
            events.push(event);
        }
        events
    })
    .await
    .unwrap();
    assert_eq!(dones(&events), vec![(DoneStatus::Interrupted, None)]);
}

#[tokio::test]
async fn pi_interrupt_kills_tool_process_group() {
    let (ctl, _steer, token) = controls();
    let mut stream = pi_fixture().run(request("tree"), ctl).await.unwrap();
    let mut tree_pids = Vec::new();
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if let AgentEvent::TextDelta { text } = &event
                && let Some(pid) = text.strip_prefix("tree:")
            {
                tree_pids.push(pid.parse::<i32>().unwrap());
                token.cancel();
            }
            events.push(event);
        }
        events
    })
    .await
    .unwrap();
    assert_eq!(dones(&events), vec![(DoneStatus::Interrupted, None)]);
    assert_eq!(tree_pids.len(), 2);
    for pid in tree_pids {
        // A zombie awaiting the host reaper is dead; no tool may remain running.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
            "{stat}"
        );
    }
}

#[tokio::test]
async fn pi_rejected_model_config_keeps_default_and_runs_prompt() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("config-rejected");
    req.model = Some("mock/reject".into());
    let events = run_to_end(&pi_fixture(), req, ctl).await;
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
}

#[tokio::test]
async fn pi_dropping_stream_terminates_tool_tree() {
    let (ctl, _steer, _) = controls();
    let mut stream = pi_fixture().run(request("tree"), ctl).await.unwrap();
    let mut pids = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while pids.len() < 2 {
            if let AgentEvent::TextDelta { text } = stream.next().await.unwrap().unwrap()
                && let Some(pid) = text.strip_prefix("tree:")
            {
                pids.push(pid.parse::<i32>().unwrap());
            }
        }
    })
    .await
    .unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if pids.iter().all(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("consumer shutdown must terminate the tool tree");
}

#[tokio::test]
async fn antigravity_strips_echoed_background_task_wakeups_from_the_reply() {
    let workspace = tempfile::tempdir().unwrap();
    let mut req = request("echo-wakeup");
    req.model = None;
    req.cwd = workspace.path().display().to_string();
    let (controls, _steer, _token) = controls();
    let events = run_to_end(&antigravity_harness(), req, controls).await;

    let reply: String = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reply, "Waiting for the build.\n\n\n\nThe build finished.");
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

fn robust_harness() -> AcpHarness {
    AcpHarness::grok().with_executable(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-robust-acp.py"),
    )
}

#[tokio::test]
async fn noise_and_large_crlf_frame_preserve_the_complete_turn() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("frames");
    req.model = None;
    req.cwd = std::env::temp_dir().display().to_string();
    let events = run_to_end(&robust_harness(), req, ctl).await;
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "x".repeat(2 * 1024 * 1024));
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn queued_updates_are_drained_before_completion_under_backpressure() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("burst");
    req.model = None;
    req.cwd = std::env::temp_dir().display().to_string();
    let mut stream = robust_harness().run(req, ctl).await.unwrap();
    // Fill both bounded queues before allowing the consumer to progress.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut text = String::new();
    let mut completed = false;
    let mut done = 0;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::TextDelta { text: chunk } => {
                    assert!(!completed);
                    text.push_str(&chunk);
                }
                AgentEvent::AssistantMessageCompleted { .. } => completed = true,
                AgentEvent::Done { status, .. } => {
                    assert_eq!(status, DoneStatus::Completed);
                    assert_eq!(text, (0..300).map(|i| format!("{i},")).collect::<String>());
                    done += 1;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(completed);
    assert_eq!(done, 1);
}

#[tokio::test]
async fn foreign_notifications_and_permissions_cannot_affect_parent_turn() {
    let (ctl, steer, _) = controls();
    drop(steer);
    let mut req = request("foreign");
    req.model = None;
    req.cwd = std::env::temp_dir().display().to_string();
    let events = run_to_end(&robust_harness(), req, ctl).await;
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "foreign permission rejected");
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None)]);
}

#[tokio::test]
async fn cancel_watchdog_ignores_late_settlement_for_all_acp_specs() {
    for adapter in [
        AcpHarness::grok(),
        AcpHarness::pi(),
        AcpHarness::antigravity(),
    ] {
        let adapter = adapter
            .with_executable(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-robust-acp.py"),
            )
            .with_graces(Duration::from_millis(50), Duration::from_millis(50));
        for scenario in ["wedge", "late-settle"] {
            let (ctl, _steer, token) = controls();
            let mut req = request(scenario);
            req.model = None;
            req.cwd = std::env::temp_dir().display().to_string();
            let mut stream = adapter.run(req, ctl).await.unwrap();
            let mut events = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(event) = stream.next().await {
                    let event = event.unwrap();
                    if matches!(&event, AgentEvent::TextDelta { text } if text == "ready") {
                        token.cancel();
                    }
                    assert!(
                        !matches!(event, AgentEvent::Usage { .. }),
                        "late usage in {scenario}"
                    );
                    events.push(event);
                }
            })
            .await
            .unwrap();
            assert_eq!(
                dones(&events),
                vec![(DoneStatus::Interrupted, None)],
                "{scenario}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_idle_stream_reaps_warm_adapter() {
    let (ctl, _steer, _) = controls();
    let mut req = request("idle-pid");
    req.model = None;
    req.cwd = std::env::temp_dir().display().to_string();
    let adapter =
        robust_harness().with_graces(Duration::from_millis(50), Duration::from_millis(50));
    let mut stream = adapter.run(req, ctl).await.unwrap();
    let mut pid = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::TextDelta { text } => pid = Some(text.parse::<u32>().unwrap()),
                AgentEvent::Done { .. } => break,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    let path = PathBuf::from(format!("/proc/{}", pid.unwrap()));
    assert!(path.exists(), "mailbox keeps the idle adapter warm");
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropped consumer reaps the idle child");
}

#[tokio::test]
async fn antigravity_stdout_sign_in_and_sibling_environment_on_every_spawn() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("server");
    let sibling = dir.path().join("localharness_external");
    std::fs::write(&sibling, "fixture").unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-antigravity-acp.sh");
    let original = std::fs::read_to_string(fixture).unwrap();
    let modified = original.replace(
        "printf 'Sign in here: https://accounts.google.com/o/oauth2/auth?client_id=fake\\n' >&2",
        "printf 'Open the following link to authenticate the ACP server: https://accounts.google.com/o/oauth2/auth?client_id=fake\\n'",
    );
    assert_ne!(original, modified);
    let checks = format!(
        "[ \"$ANTIGRAVITY_HARNESS_PATH\" = '{}' ] || exit 3\n[ \"$PYTHONUNBUFFERED\" = 1 ] || exit 4\n",
        sibling.display(),
    );
    std::fs::write(
        &server,
        modified.replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{checks}"), 1),
    )
    .unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    let harness = AcpHarness::antigravity().with_executable(&server);
    let seen: std::sync::Arc<std::sync::Mutex<Vec<SignInProgress>>> = Default::default();
    let recorder = seen.clone();
    harness
        .sign_in(None, move |p| recorder.lock().unwrap().push(p))
        .await
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![SignInProgress::OpenBrowser(
            "https://accounts.google.com/o/oauth2/auth?client_id=fake".into(),
        )]
    );
    harness.sign_out().await.unwrap();
    assert!(!harness.models().await.unwrap().is_empty());
    assert!(!harness.commands().await.unwrap().is_empty());
    let (ctl, _steer, _) = controls();
    let mut req = request("hello");
    req.model = None;
    req.cwd = dir.path().display().to_string();
    assert_eq!(
        dones(&run_to_end(&harness, req, ctl).await),
        vec![(DoneStatus::Completed, None)]
    );
}

async fn pi_boundary_steer(scenario: &str, trigger_on_done: bool) {
    let adapter = AcpHarness::pi().with_executable(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-robust-acp.py"),
    );
    let (ctl, steer, _) = controls();
    let mut steer = Some(steer);
    let mut req = request(scenario);
    req.model = None;
    req.cwd = std::env::temp_dir().display().to_string();
    let mut stream = adapter.run(req, ctl).await.unwrap();
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let trigger = if trigger_on_done {
                matches!(event, AgentEvent::Done { .. })
            } else {
                matches!(&event, AgentEvent::TextDelta { text } if text == "first")
            };
            if trigger && let Some(sender) = steer.take() {
                sender
                    .send(zeron_harness::SteerMessage {
                        prompt: "second".into(),
                        message_id: None,
                    })
                    .await
                    .unwrap();
            }
            events.push(event);
        }
    })
    .await
    .unwrap();
    assert_eq!(dones(&events), vec![(DoneStatus::Completed, None); 2]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TextDelta { text } if text == "second"))
            .count(),
        1
    );
    let first_done = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Done { .. }))
        .unwrap();
    let second_text = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "second"))
        .unwrap();
    assert!(first_done < second_text);
}

#[tokio::test]
async fn pi_without_steering_extension_queues_live_steer_once() {
    pi_boundary_steer("steer-live", false).await;
}

#[tokio::test]
async fn pi_without_steering_extension_dispatches_idle_steer_immediately() {
    pi_boundary_steer("steer-idle", true).await;
}

#[test]
fn antigravity_detection_and_missing_server_never_install() {
    use std::os::unix::fs::PermissionsExt;
    for scenario in [
        "cli-only",
        "partial",
        "server",
        "par",
        "exe",
        "override",
        "invalid-override",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let name = match scenario {
            "server" => "agy_acp_server",
            "par" => "agy_acp_server.par",
            "exe" => "agy_acp_server.exe",
            "override" => "custom-server",
            _ => "agy",
        };
        let exe = bin.join(name);
        std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let adapters = dir.path().join("adapters");
        if scenario == "partial" {
            let partial = adapters.join("antigravity-acp/1.1.1");
            std::fs::create_dir_all(&partial).unwrap();
            std::fs::write(partial.join("agy_acp_server.par"), "incomplete").unwrap();
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "antigravity_detection_subprocess", "--nocapture"])
            .env("HOME", dir.path())
            .env("PATH", &bin)
            .env("SHELL", "/nonexistent-shell")
            .env("ZERON_ADAPTERS_DIR", &adapters)
            .env("ZERON_TEST_DETECTION", scenario)
            .env_remove("ANTIGRAVITY_ACP_EXECUTABLE");
        if scenario == "override" {
            child.env("ANTIGRAVITY_ACP_EXECUTABLE", &exe);
        }
        if scenario == "invalid-override" {
            child.env("ANTIGRAVITY_ACP_EXECUTABLE", bin.join("missing"));
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!adapters.join(".tmp-antigravity-acp-1.1.1").exists());
    }
}

#[tokio::test]
async fn antigravity_detection_subprocess() {
    let Ok(scenario) = std::env::var("ZERON_TEST_DETECTION") else {
        return;
    };
    let harness = AcpHarness::antigravity();
    let installed = matches!(scenario.as_str(), "server" | "par" | "exe" | "override");
    assert_eq!(harness.installed(), installed);
    if installed {
        assert!(harness.resolve_program(false).await.is_ok());
        return;
    }
    for block in [false, true] {
        assert!(matches!(
            harness.resolve_program(block).await,
            Err(HarnessError::NotInstalled(_))
        ));
    }
    assert!(matches!(
        harness.models().await,
        Err(HarnessError::NotInstalled(_))
    ));
    assert!(matches!(
        harness.commands().await,
        Err(HarnessError::NotInstalled(_))
    ));
    assert!(matches!(
        harness
            .sign_in(None, |_| panic!("no sign-in progress when missing"))
            .await,
        Err(HarnessError::NotInstalled(_))
    ));
    let (ctl, _, _) = controls();
    assert!(matches!(
        harness.run(request("hello"), ctl).await,
        Err(HarnessError::NotInstalled(_))
    ));
    let adapters = PathBuf::from(std::env::var_os("ZERON_ADAPTERS_DIR").unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        std::fs::read_dir(adapters)
            .map(|entries| entries
                .map(Result::unwrap)
                .all(|entry| entry.file_name() == "antigravity-acp"))
            .unwrap_or(true)
    );
}

#[tokio::test]
async fn all_acp_harnesses_use_project_scoped_session_command_updates() {
    for h in [
        AcpHarness::devin(),
        AcpHarness::grok(),
        AcpHarness::hermes(),
        AcpHarness::pi(),
        AcpHarness::antigravity(),
    ] {
        let h = h.with_executable(fixture_path());
        for name in ["project-a", "project-b"] {
            let cwd = tempfile::tempdir().unwrap();
            std::fs::write(cwd.path().join(".command-fixture"), name).unwrap();
            let commands = h
                .commands_for(&cwd.path().canonicalize().unwrap())
                .await
                .unwrap();
            assert_eq!(commands.len(), 1, "{:?}", h.id());
            assert_eq!(commands[0].name, name, "{:?}", h.id());
        }
    }
}

#[tokio::test]
async fn shared_acp_skills_require_explicit_native_command_classification() {
    use zeron_proto::invocation::{Invocation, harness_prompt};
    for h in [
        AcpHarness::devin(),
        AcpHarness::grok(),
        AcpHarness::hermes(),
        AcpHarness::pi(),
        AcpHarness::antigravity(),
    ] {
        let h = h.with_executable(fixture_path());
        let cwd = tempfile::tempdir().unwrap();
        let skill_dir = cwd.path().join(".agents/skills/zeron-fixture-review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: zeron-fixture-review\ndescription: Review changes\n---\nReview the changes.").unwrap();
        let command_name = if h.id() == HarnessId::Pi {
            "skill:zeron-fixture-review"
        } else {
            "zeron-fixture-review"
        };
        std::fs::write(cwd.path().join(".command-fixture"), command_name).unwrap();
        let skills = h
            .skills(&cwd.path().canonicalize().unwrap())
            .await
            .unwrap()
            .unwrap();
        let skill = skills
            .into_iter()
            .find(|s| s.name == "zeron-fixture-review")
            .unwrap();
        assert_eq!(skill.command.is_some(), h.id() == HarnessId::Pi);
        let invocation = Invocation::Skill {
            name: skill.name,
            path: skill.path,
            command: skill.command,
        };
        assert_eq!(
            harness_prompt(&format!("{} inspect tests", invocation.link()), h.id()),
            if h.id() == HarnessId::Pi {
                format!("/{command_name} inspect tests")
            } else {
                format!("Use the skill {} inspect tests", invocation.prompt_text())
            }
        );
    }
}
