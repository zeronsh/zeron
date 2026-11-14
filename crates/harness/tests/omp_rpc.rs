#![cfg(feature = "native-fixture")]
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, Harness, OmpHarness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};
fn fixture() -> OmpHarness {
    OmpHarness::new()
        .with_executable(env!("CARGO_BIN_EXE_harness-omp-fixture"))
        .with_graces(Duration::from_millis(100), Duration::from_millis(100))
}
fn harness() -> OmpHarness {
    // Never consult the developer's own OMP settings.
    fixture()
        .with_agent_dir(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("omp-rpc-agent"))
}
fn request(cwd: &std::path::Path, prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}
fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (tx, rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering: rx,
            interrupt: token.clone(),
            request_input: Box::new(|_| {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(vec![]);
                rx
            }),
            turn: Default::default(),
        },
        tx,
        token,
    )
}
async fn collect_in(h: &OmpHarness, req: RunRequest) -> Vec<AgentEvent> {
    let (c, tx, _) = controls();
    drop(tx);
    let stream = h.run(req, c).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
        .await
        .unwrap()
}
async fn collect(prompt: &str) -> Vec<AgentEvent> {
    let dir = tempfile::tempdir().unwrap();
    let h = harness().with_session_store(dir.path().join("index"));
    collect_in(&h, request(dir.path(), prompt)).await
}
fn dones(events: &[AgentEvent]) -> Vec<DoneStatus> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
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
fn unknown_commands(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("unknown-commands")).ok()
}

#[tokio::test]
async fn terminal_contract_follows_prompt_results_and_local_completions() {
    for (prompt, status) in [
        ("hello", DoneStatus::Completed),
        ("error", DoneStatus::Errored),
        ("reject", DoneStatus::Errored),
        ("retry", DoneStatus::Completed),
        ("auto-compact", DoneStatus::Completed),
        ("/noop", DoneStatus::Completed),
        ("handled", DoneStatus::Completed),
        ("/compact", DoneStatus::Completed),
        ("/session", DoneStatus::Completed),
        ("/name Test", DoneStatus::Completed),
        ("/autocompact off", DoneStatus::Completed),
        ("background", DoneStatus::Completed),
        ("crash", DoneStatus::Errored),
    ] {
        let events = collect(prompt).await;
        assert_eq!(dones(&events), vec![status], "{prompt}: {events:?}");
        match prompt {
            "hello" => assert_eq!(text(&events), "reply:hello"),
            "retry" | "auto-compact" => assert_eq!(text(&events), format!("reply:{prompt}")),
            "/compact" | "/session" | "/name Test" | "/autocompact off" => assert_eq!(
                text(&events),
                format!("ran {}", prompt.split_whitespace().next().unwrap())
            ),
            // The turn stays open until the session settles.
            "background" => assert!(text(&events).ends_with("background done"), "{events:?}"),
            "error" => assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Done { error: Some(error), .. }
                    if error.contains("mock provider failure"))),
                "{events:?}"
            ),
            "reject" => assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Done { error: Some(error), .. }
                    if error.contains("preflight rejected"))),
                "{events:?}"
            ),
            "crash" => assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Done { error: Some(error), .. }
                    if error.contains("mock crash diagnostic"))),
                "{events:?}"
            ),
            _ => {}
        }
        // The startup "extension loaded" toast is not part of the answer.
        assert!(!text(&events).contains("extension loaded"), "{events:?}");
    }
}

/// An explicit agent dir is forced on the child, even over an inherited named
/// profile (which would make OMP ignore it).
#[tokio::test]
async fn agent_dir_is_forced_only_when_explicit() {
    const CASE: &str = "ZERON_OMP_AGENT_DIR_CASE";
    let explicit = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("omp-rpc-agent");
    let run = || async {
        let dir = tempfile::tempdir().unwrap();
        let h = harness().with_session_store(dir.path().join("index"));
        text(&collect_in(&h, request(dir.path(), "agent-dir")).await)
    };
    let expected = format!("reply:agent-dir:{}|", explicit.display());
    if std::env::var_os(CASE).is_some() {
        assert_eq!(run().await, expected);
        return;
    }
    assert_eq!(run().await, expected);
    // Re-run with an inherited named profile; never mutate this process's env.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "agent_dir_is_forced_only_when_explicit",
            "--nocapture",
        ])
        .env(CASE, "profile")
        .env("OMP_PROFILE", "work")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{output:?}"
    );
}

#[tokio::test]
async fn resumes_the_same_session_after_process_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness().with_session_store(dir.path().join("index"));
    let mut id = None;
    for prompt in ["first", "second"] {
        let mut req = request(dir.path(), prompt);
        req.resume = id.clone();
        let events = collect_in(&h, req).await;
        for event in events {
            if let AgentEvent::Done {
                session_id, status, ..
            } = event
            {
                assert_eq!(status, DoneStatus::Completed);
                if id.is_some() {
                    assert_eq!(id, session_id);
                }
                id = session_id;
            }
        }
    }
    assert_eq!(id.as_deref(), Some("omp-fixture-session"));
    // OMP session files open with a title slot; resolution reads past it.
    let file = std::fs::read_to_string(dir.path().join("omp-fixture-session.jsonl")).unwrap();
    let first: serde_json::Value = serde_json::from_str(file.lines().next().unwrap()).unwrap();
    assert_eq!(first["type"], "title", "{file}");
}

#[tokio::test]
async fn unrestorable_session_starts_fresh_with_a_visible_notice() {
    // The engine resumes a chat's stored id on every dispatch, so a hard error
    // here would leave the chat unusable for good.
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "hello");
    req.resume = Some("missing-session".into());
    let h = harness().with_session_store(dir.path().join("index"));
    let events = collect_in(&h, req).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::Error { message }
            if message.contains("missing-session") && message.contains("without the previous context"))),
        "{events:?}"
    );
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done {
                status, session_id, ..
            } => Some((*status, session_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        done,
        vec![(DoneStatus::Completed, Some("omp-fixture-session".into()))]
    );
}

#[tokio::test]
async fn a_session_without_a_turn_resumes_fresh_without_a_lost_context_notice() {
    // OMP writes the session file at the first turn; a local command starts none.
    let dir = tempfile::tempdir().unwrap();
    let h = harness().with_session_store(dir.path().join("index"));
    let events = collect_in(&h, request(dir.path(), "/session")).await;
    let id = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done { session_id, .. } => session_id.clone(),
            _ => None,
        })
        .unwrap();
    assert!(!dir.path().join("omp-fixture-session.jsonl").exists());
    let mut req = request(dir.path(), "hello");
    req.resume = Some(id);
    let events = collect_in(&h, req).await;
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
        "{events:?}"
    );
    assert_eq!(dones(&events), vec![DoneStatus::Completed]);
}

#[tokio::test]
async fn a_session_switching_command_refreshes_the_session_identity() {
    let events = collect("/new").await;
    let started: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionStarted { session_id, .. } => Some(session_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["omp-fixture-session", "omp-fixture-session-2"]);
    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::Done { session_id: Some(id), .. }
            if id == "omp-fixture-session-2")
        ),
        "{events:?}"
    );
}

#[tokio::test]
async fn oversized_state_arrives_chunked_only_after_protocol_negotiation() {
    for v1_only in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big-state"), "").unwrap();
        if v1_only {
            std::fs::write(dir.path().join("v1-only"), "").unwrap();
        }
        let h = harness().with_session_store(dir.path().join("index"));
        let events = collect_in(&h, request(dir.path(), "hello")).await;
        if v1_only {
            assert_eq!(dones(&events), vec![DoneStatus::Errored], "{events:?}");
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Done { error: Some(error), .. }
                    if error.contains("transport limit"))),
                "{events:?}"
            );
        } else {
            assert_eq!(dones(&events), vec![DoneStatus::Completed], "{events:?}");
            assert_eq!(text(&events), "reply:hello");
        }
    }
}

#[tokio::test]
async fn steers_confirm_on_consumption_and_interrupt_is_terminal_once() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, token) = controls();
    let mut stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), "slow"), c)
        .await
        .unwrap();
    tx.send(SteerMessage {
        prompt: "redirect".into(),
        message_id: Some("user-id".into()),
        attachments: Vec::new(),
        config: None,
    })
    .await
    .unwrap();
    let mut confirmed = 0;
    let mut done = vec![];
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Steered {
                    next_assistant_message_id,
                    ..
                } => {
                    assert_ne!(next_assistant_message_id.as_deref(), Some("user-id"));
                    confirmed += 1;
                    token.cancel();
                }
                AgentEvent::Done { status, .. } => done.push(status),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(confirmed, 1);
    assert_eq!(done, vec![DoneStatus::Interrupted]);
    // OMP has no clear_queue: interrupt uses abort_and_restore_queue.
    assert_eq!(unknown_commands(dir.path()), None);
}

#[tokio::test]
async fn idle_mailbox_starts_another_turn_without_restarting_process() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    let mut stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), "first"), c)
        .await
        .unwrap();
    let mut sender = Some(tx);
    let mut done = 0;
    let mut confirmed = 0;
    let mut started = 0;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Done { status, .. } => {
                    assert_eq!(status, DoneStatus::Completed);
                    done += 1;
                    if let Some(tx) = sender.take() {
                        tx.send(SteerMessage::text("second")).await.unwrap();
                    }
                }
                AgentEvent::Steered { .. } => confirmed += 1,
                AgentEvent::SessionStarted { .. } => started += 1,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(done, 2);
    assert_eq!(confirmed, 1);
    assert_eq!(started, 1, "the same process serves the next turn");
}

#[tokio::test]
async fn steering_mode_is_always_selected_for_the_session() {
    // OMP's set_steering_mode is session-scoped, so it never overrides config.yml.
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(agent.join("config.yml"), "steeringMode: one-at-a-time\n").unwrap();
    let h = harness()
        .with_agent_dir(&agent)
        .with_session_store(dir.path().join("index"));
    let events = collect_in(&h, request(dir.path(), "hello")).await;
    assert_eq!(dones(&events), vec![DoneStatus::Completed]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("steering-mode")).ok(),
        Some("all".into())
    );
}

#[tokio::test]
async fn discovers_models_from_the_cli_and_the_omp_command_catalog() {
    let h = harness();
    let models = h.models().await.unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["mock/mock", "mock/mock-2"],
        "non-chat models are not offered"
    );
    let dir = tempfile::tempdir().unwrap();
    let commands = h.commands_for(dir.path()).await.unwrap();
    for expected in ["compact", "new", "noop", "skill:probe"] {
        assert!(commands.iter().any(|c| c.name == expected), "{expected}");
    }
    let compact = commands.iter().find(|c| c.name == "compact").unwrap();
    assert_eq!(compact.input_hint.as_deref(), Some("[instructions]"));
    let skills = h.skills(dir.path()).await.unwrap().unwrap();
    let probe = skills.iter().find(|s| s.name == "probe").unwrap();
    assert_eq!(probe.path, "harness-skill:omp:probe");
    assert_eq!(
        unknown_commands(dir.path()),
        None,
        "Pi's get_commands is not served"
    );
}

#[tokio::test]
async fn extension_dialogs_roundtrip_without_autoaccepting_and_preserve_editor_text() {
    let dir = tempfile::tempdir().unwrap();
    let (mut c, tx, _) = controls();
    drop(tx);
    c.request_input = Box::new(|questions| {
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].header, "Oh My Pi");
        assert_eq!(questions[0].prefill.as_deref(), Some("initial\nvalue"));
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(vec![zeron_proto::UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["second".into()],
        }]);
        rx
    });
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(5),
        harness()
            .with_session_store(dir.path().join("index"))
            .run(request(dir.path(), "/dialog select"), c)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
    )
    .await
    .unwrap();
    let reply = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TextDelta { text } => {
                serde_json::from_str::<serde_json::Value>(text.trim()).ok()
            }
            _ => None,
        })
        .expect("dialog reply notification");
    assert_eq!(reply["value"], "second", "{events:?}");
    assert_eq!(dones(&events), vec![DoneStatus::Completed]);
}

async fn wait_for_queued_steers(dir: &std::path::Path, expected: &[String]) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let queued = std::fs::read(dir.join("pending-steers.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok());
            if queued.as_deref() == Some(expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the entire burst must reach OMP before the blocked model step finishes");
}

async fn wait_until_blocked(
    stream: &mut futures::stream::BoxStream<
        'static,
        Result<AgentEvent, zeron_harness::HarnessError>,
    >,
) -> Option<String> {
    let mut assistant = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::SessionStarted {
                    assistant_message_id,
                    ..
                } => assistant = Some(assistant_message_id),
                AgentEvent::TextDelta { text } if text == "waiting" => return,
                AgentEvent::Done { .. } => panic!("model step ended before release"),
                _ => {}
            }
        }
        panic!("fixture did not start the blocked model step");
    })
    .await
    .unwrap();
    assistant
}

#[tokio::test]
async fn steering_burst_reaches_one_model_step_and_confirms_each_message_on_consumption() {
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (c, tx, token) = controls();
        let mut stream = harness()
            .with_session_store(dir.path().join("index"))
            .run(request(dir.path(), "burst-start"), c)
            .await
            .unwrap();
        let mut assistant = wait_until_blocked(&mut stream).await;
        // Exceed the engine mailbox size and include duplicate text. Delivery
        // receipts must still be one per original message, in the original order.
        let prompts: Vec<_> = (0..40)
            .map(|i| {
                if i == 20 {
                    // A command name without '/' is still ordinary user text.
                    "noop".into()
                } else {
                    format!("redirect {}", i / 2)
                }
            })
            .collect();
        for (i, prompt) in prompts.iter().enumerate() {
            tx.send(SteerMessage {
                prompt: prompt.clone(),
                message_id: Some(format!("user-{i}")),
                attachments: Vec::new(),
                config: None,
            })
            .await
            .unwrap();
        }
        drop(tx);
        wait_for_queued_steers(dir.path(), &prompts).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), stream.next())
                .await
                .is_err(),
            "queue acceptance must not confirm consumption or finish the turn"
        );
        if cancel {
            token.cancel();
        } else {
            std::fs::write(dir.path().join("release-burst"), "").unwrap();
        }
        let mut confirms = 0;
        let mut done = vec![];
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::Steered {
                        assistant_message_id,
                        next_assistant_message_id,
                    } => {
                        assert_eq!(assistant_message_id, assistant);
                        assert!(next_assistant_message_id.is_some());
                        assistant = next_assistant_message_id;
                        confirms += 1;
                    }
                    AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                    AgentEvent::Done { status, .. } => done.push(status),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        if cancel {
            assert_eq!(confirms, 0);
            assert_eq!(done, vec![DoneStatus::Interrupted]);
            assert!(!text.contains("redirect"), "{text}");
        } else {
            assert_eq!(confirms, prompts.len());
            assert_eq!(done, vec![DoneStatus::Completed]);
            assert_eq!(
                text,
                format!("reply:burst-startreply:{}", prompts.join("\n"))
            );
        }
    }
}

#[tokio::test]
async fn handled_input_between_steers_keeps_its_own_delivery_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    let mut stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), "burst-start"), c)
        .await
        .unwrap();
    wait_until_blocked(&mut stream).await;
    for prompt in ["redirect first", "handled", "redirect last"] {
        tx.send(SteerMessage::text(prompt)).await.unwrap();
    }
    drop(tx);
    // OMP finishes the handled input at once; only the others queue.
    wait_for_queued_steers(
        dir.path(),
        &["redirect first".into(), "redirect last".into()],
    )
    .await;
    let mut receipts = 0;
    tokio::time::timeout(Duration::from_secs(5), async {
        while receipts < 1 {
            if let AgentEvent::Steered { .. } = stream.next().await.unwrap().unwrap() {
                receipts += 1;
            }
        }
    })
    .await
    .expect("the handled input is confirmed when it completes");
    std::fs::write(dir.path().join("release-burst"), "").unwrap();
    let mut text = String::new();
    let mut completions = 0;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Steered { .. } => receipts += 1,
                AgentEvent::TextDelta { text: delta } => {
                    if delta.contains("redirect first") {
                        assert_eq!(receipts, 3, "each queued input has its own receipt");
                    }
                    text.push_str(&delta);
                }
                AgentEvent::Done { status, .. } => {
                    assert_eq!(status, DoneStatus::Completed);
                    completions += 1;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(receipts, 3);
    assert_eq!(completions, 1);
    assert_eq!(text, "reply:burst-startreply:redirect first\nredirect last");
}

#[tokio::test]
async fn a_refused_steer_gets_no_delivery_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    let mut stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), "burst-start"), c)
        .await
        .unwrap();
    wait_until_blocked(&mut stream).await;
    for prompt in ["reject", "redirect"] {
        tx.send(SteerMessage::text(prompt)).await.unwrap();
    }
    drop(tx);
    // The second input is sent only once OMP has answered the first.
    wait_for_queued_steers(dir.path(), &["redirect".into()]).await;
    std::fs::write(dir.path().join("release-burst"), "").unwrap();
    let mut receipts = 0;
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Steered { .. } => receipts += 1,
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(receipts, 1, "only the input OMP accepted is confirmed");
    assert!(text.ends_with("reply:redirect"), "{text}");
}

#[tokio::test]
async fn late_extension_notifications_do_not_reopen_completed_turns() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    let mut stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), "late-notify"), c)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(
            stream.next().await.unwrap().unwrap(),
            AgentEvent::Done { .. }
        ) {}
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), stream.next())
            .await
            .is_err()
    );
    tx.send(SteerMessage::text("next")).await.unwrap();
    drop(tx);
    let events: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
            .await
            .unwrap();
    assert_eq!(dones(&events), vec![DoneStatus::Completed]);
}

/// A model or thinking switch reaches the live process before the message
/// it was sent with: no new process, no lost session.
#[tokio::test]
async fn a_model_switch_applies_to_the_live_process() {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    let mut opening = request(dir.path(), "which-model");
    opening.model = Some("mock/mock".into());
    let mut switched = opening.clone();
    switched.model = Some("mock/mock-2".into());
    switched.reasoning = Some(zeron_proto::ReasoningLevel::High);
    let driver = harness().with_session_store(dir.path().join("index"));
    assert!(driver.reconfigures_in_place(&opening, &switched));
    let mut stream = driver.run(opening, c).await.unwrap();
    let mut text = String::new();
    let mut dones = 0;
    let mut started = 0;
    while dones < 2 {
        let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match event {
            AgentEvent::SessionStarted { .. } => started += 1,
            AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
            AgentEvent::Done { .. } => {
                dones += 1;
                text.push('|');
                if dones == 1 {
                    tx.send(SteerMessage {
                        config: Some(Box::new(switched.clone())),
                        ..SteerMessage::text("which-model")
                    })
                    .await
                    .unwrap();
                }
            }
            _ => {}
        }
    }
    assert_eq!(started, 1);
    assert!(text.contains("reply:mock/medium|"), "{text}");
    assert!(text.contains("reply:mock-2/high|"), "{text}");
}
