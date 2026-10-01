#![cfg(feature = "native-fixture")]
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, Harness, PiHarness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};
fn harness() -> PiHarness {
    PiHarness::new()
        .with_executable(env!("CARGO_BIN_EXE_harness-pi-fixture"))
        // Never consult the developer's own Pi settings.
        .with_agent_dir(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("pi-rpc-agent"))
        .with_graces(Duration::from_millis(100), Duration::from_millis(100))
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
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
    }
}
fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (tx, rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    (
        RunControls {
            execution_lease: None,
            steering: rx,
            interrupt: token.clone(),
            request_input: Box::new(|_| {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(vec![]);
                rx
            }),
        },
        tx,
        token,
    )
}
async fn collect(prompt: &str) -> Vec<AgentEvent> {
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    drop(tx);
    let stream = harness()
        .with_session_store(dir.path().join("index"))
        .run(request(dir.path(), prompt), c)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
        .await
        .unwrap()
}
#[tokio::test]
async fn terminal_contract_handles_normal_errors_retries_compaction_and_consumed_prompts() {
    for (prompt, status) in [
        ("hello", DoneStatus::Completed),
        ("error", DoneStatus::Errored),
        ("reject", DoneStatus::Errored),
        ("retry", DoneStatus::Completed),
        ("compact", DoneStatus::Completed),
        ("/noop", DoneStatus::Completed),
        ("handled", DoneStatus::Completed),
        ("crash", DoneStatus::Errored),
    ] {
        let events = collect(prompt).await;
        let dones: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let AgentEvent::Done { status, .. } = e {
                    Some(status)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(dones, vec![&status], "{prompt}: {events:?}");
        if prompt == "hello" {
            let text: String = events
                .iter()
                .filter_map(|e| {
                    if let AgentEvent::TextDelta { text } = e {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(text, "reply:hello");
        }
    }
}

#[tokio::test]
async fn nested_agent_environment_is_not_inherited() {
    if std::env::var_os("CLAUDECODE").is_none() {
        // Re-run in a private environment; never mutate this process's env.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "nested_agent_environment_is_not_inherited",
                "--nocapture",
            ])
            .env("CLAUDECODE", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{output:?}"
        );
        return;
    }
    let text: String = collect("env")
        .await
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "reply:env:false");
}

#[tokio::test]
async fn resumes_the_same_session_after_process_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness().with_session_store(dir.path().join("index"));
    let mut id = None;
    for text in ["first", "second"] {
        let (c, tx, _) = controls();
        drop(tx);
        let mut req = request(dir.path(), text);
        req.resume = id.clone();
        let events: Vec<_> = tokio::time::timeout(
            Duration::from_secs(5),
            h.run(req, c).await.unwrap().collect(),
        )
        .await
        .unwrap();
        for event in events {
            if let AgentEvent::Done {
                session_id, status, ..
            } = event.unwrap()
            {
                assert_eq!(status, DoneStatus::Completed);
                if id.is_some() {
                    assert_eq!(id, session_id);
                }
                id = session_id;
            }
        }
    }
    assert!(id.is_some());
}

#[tokio::test]
async fn unrestorable_session_starts_fresh_with_a_visible_notice() {
    // The engine resumes a chat's stored id on every dispatch, so a hard error
    // here would leave the chat unusable for good.
    let dir = tempfile::tempdir().unwrap();
    let (c, tx, _) = controls();
    drop(tx);
    let mut req = request(dir.path(), "hello");
    req.resume = Some("missing-session".into());
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(5),
        harness()
            .with_session_store(dir.path().join("index"))
            .run(req, c)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
    )
    .await
    .unwrap();
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
        vec![(DoneStatus::Completed, Some("pi-fixture-session".into()))]
    );
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
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Done { status, .. } => {
                    assert_eq!(status, DoneStatus::Completed);
                    done += 1;
                    if let Some(tx) = sender.take() {
                        tx.send(SteerMessage {
                            prompt: "second".into(),
                            message_id: None,
                        })
                        .await
                        .unwrap();
                    }
                }
                AgentEvent::Steered { .. } => confirmed += 1,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(done, 2);
    assert_eq!(confirmed, 1);
}

#[tokio::test]
async fn steering_mode_is_selected_only_while_unconfigured() {
    for configured in [None, Some("one-at-a-time"), Some("all")] {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        if let Some(mode) = configured {
            std::fs::write(
                agent.join("settings.json"),
                serde_json::json!({ "steeringMode": mode }).to_string(),
            )
            .unwrap();
        }
        let (c, tx, _) = controls();
        drop(tx);
        let stream = harness()
            .with_agent_dir(&agent)
            .with_session_store(dir.path().join("index"))
            .run(request(dir.path(), "hello"), c)
            .await
            .unwrap();
        let events: Vec<_> =
            tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
                .await
                .unwrap();
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::Done { status, .. } if *status == DoneStatus::Completed)
            ),
            "{events:?}"
        );
        // Pi persists set_steering_mode globally: an explicit choice must survive.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("steering-mode")).ok(),
            configured.is_none().then(|| "all".to_string()),
            "{configured:?}"
        );
    }
}

#[tokio::test]
async fn discovers_model_specific_thinking_and_extension_commands() {
    let h = harness();
    let models = h.models().await.unwrap();
    assert_eq!(models[0].id, "mock/mock");
    assert_eq!(
        models[0].reasoning_levels,
        vec![
            zeron_proto::ReasoningLevel::Low,
            zeron_proto::ReasoningLevel::Medium,
            zeron_proto::ReasoningLevel::High
        ]
    );
    assert_eq!(models[0].options[0].id, "pi_thinking");
    let dir = tempfile::tempdir().unwrap();
    let commands = h.commands_for(dir.path()).await.unwrap();
    for expected in ["noop", "skill:probe", "compact", "session"] {
        assert!(commands.iter().any(|c| c.name == expected));
    }
    let skills = h.skills(dir.path()).await.unwrap().unwrap();
    assert!(skills.iter().any(|s| s.name == "probe"));
}
#[tokio::test]
async fn rpc_control_commands_finish_without_waiting_for_agent_settled() {
    for command in [
        "/compact",
        "/session",
        "/name Test",
        "/autocompact off",
        "/steering all",
    ] {
        let events = collect(command).await;
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    AgentEvent::Done {
                        status: DoneStatus::Completed,
                        ..
                    }
                ))
                .count(),
            1,
            "{command}: {events:?}"
        );
    }
}

#[tokio::test]
async fn extension_dialogs_roundtrip_without_autoaccepting_and_preserve_editor_text() {
    for (method, label, key, expected) in [
        (
            "select",
            Some("second"),
            "value",
            serde_json::json!("second"),
        ),
        ("select", None, "cancelled", serde_json::json!(true)),
        ("confirm", Some("No"), "confirmed", serde_json::json!(false)),
        (
            "input",
            Some("custom"),
            "value",
            serde_json::json!("custom"),
        ),
        (
            "editor",
            Some("  first\nsecond\n"),
            "value",
            serde_json::json!("  first\nsecond\n"),
        ),
        ("editor", Some(""), "value", serde_json::json!("")),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, tx, _) = controls();
        drop(tx);
        c.request_input = Box::new(move |questions| {
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].multiline, method == "editor");
            assert_eq!(questions[0].prefill.as_deref(), Some("initial\nvalue"));
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(
                label
                    .map(|l| {
                        vec![zeron_proto::UserInputAnswer {
                            question_id: questions[0].id.clone(),
                            labels: vec![l.into()],
                        }]
                    })
                    .unwrap_or_default(),
            );
            rx
        });
        let events: Vec<_> = tokio::time::timeout(
            Duration::from_secs(5),
            harness()
                .with_session_store(dir.path().join("index"))
                .run(request(dir.path(), &format!("/dialog {method}")), c)
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
        assert_eq!(reply[key], expected, "{events:?}");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    AgentEvent::Done {
                        status: DoneStatus::Completed,
                        ..
                    }
                ))
                .count(),
            1
        );
    }
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
    .expect("the entire burst must reach Pi before the blocked model step finishes");
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
    for handled in ["handled", "/noop"] {
        let dir = tempfile::tempdir().unwrap();
        let (c, tx, _) = controls();
        let mut stream = harness()
            .with_session_store(dir.path().join("index"))
            .run(request(dir.path(), "burst-start"), c)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                if matches!(event.unwrap(), AgentEvent::TextDelta { text } if text == "waiting") {
                    return;
                }
            }
            panic!("fixture did not reach the blocked model step");
        })
        .await
        .unwrap();
        for prompt in ["redirect first", handled, "redirect last"] {
            tx.send(SteerMessage {
                prompt: prompt.into(),
                message_id: None,
            })
            .await
            .unwrap();
        }
        drop(tx);
        wait_for_queued_steers(dir.path(), &["redirect first".into()]).await;
        std::fs::write(dir.path().join("release-burst"), "").unwrap();
        let mut receipts = 0;
        let mut text = String::new();
        let mut completions = 0;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::Steered { .. } => receipts += 1,
                    AgentEvent::TextDelta { text: delta } => {
                        if delta.contains("redirect first") {
                            assert_eq!(
                                receipts, 1,
                                "handled input must not consume another message's receipt"
                            );
                        }
                        if delta.contains("redirect last") {
                            assert_eq!(receipts, 3);
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
        assert!(completions >= 1);
        assert_eq!(
            text,
            "reply:burst-startreply:redirect firstreply:redirect last"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn crash_interrupt_and_consumer_drop_reap_descendants_even_with_inherited_pipes() {
    for scenario in ["inherited-pipe-crash", "interrupt", "drop"] {
        let dir = tempfile::tempdir().unwrap();
        let (c, tx, token) = controls();
        let prompt = if scenario == "inherited-pipe-crash" {
            scenario
        } else {
            "tree"
        };
        let mut stream = harness()
            .with_session_store(dir.path().join("index"))
            .run(request(dir.path(), prompt), c)
            .await
            .unwrap();
        let mut terminals = vec![];
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::TextDelta { text } if text == "tree ready" => {
                        if scenario == "drop" {
                            break;
                        }
                        token.cancel();
                    }
                    AgentEvent::Done { status, error, .. } => {
                        if scenario == "inherited-pipe-crash" {
                            assert!(error.unwrap().contains("inherited pipe diagnostic"));
                        }
                        terminals.push(status);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        drop(stream);
        drop(tx);
        if scenario != "drop" {
            assert_eq!(
                terminals,
                vec![if scenario == "interrupt" {
                    DoneStatus::Interrupted
                } else {
                    DoneStatus::Errored
                }]
            );
        }
        for file in ["pi.pid", "tool.pid"] {
            let pid = std::fs::read_to_string(dir.path().join(file)).unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let stat =
                        std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                    if stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z") {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("owned descendant must be dead");
        }
    }
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
    tx.send(SteerMessage {
        prompt: "next".into(),
        message_id: None,
    })
    .await
    .unwrap();
    drop(tx);
    let events: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
            .await
            .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                }
            ))
            .count(),
        1
    );
}
