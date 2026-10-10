//! Real native Pi, isolated settings, and a local provider (no network/API spend).
#![cfg(unix)]
use futures::StreamExt;
use std::{os::unix::fs::PermissionsExt, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, Harness, PiHarness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel, UserInputAnswer};

fn isolated_pi() -> (tempfile::TempDir, PiHarness) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let agent = cwd.join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"retry":{"enabled":false}}"#,
    )
    .unwrap();
    let extensions = agent.join("extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    std::fs::write(
        extensions.join("probe.ts"),
        include_str!("fixtures/pi-rpc-probe.ts"),
    )
    .unwrap();
    let exe = PiHarness::new()
        .resolve_executable()
        .expect("Pi CLI installed");
    let quote =
        |p: &std::path::Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    let wrapper = cwd.join("pi-probe");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport PI_CODING_AGENT_DIR={}\nexec {} \"$@\"\n",
            quote(&agent),
            quote(&exe)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let harness = PiHarness::new()
        .with_executable(wrapper)
        .with_agent_dir(&agent)
        .with_session_store(cwd.join("index"));
    (dir, harness)
}

#[tokio::test]
#[ignore = "requires Pi >= 0.85.1 installed; uses only a local mock provider"]
async fn real_pi_mock_lifecycle() {
    let (dir, harness) = isolated_pi();
    let cwd = dir.path();
    let mut session = None;
    for (prompt, expected) in [
        ("/probe-noop", DoneStatus::Completed),
        ("hello", DoneStatus::Completed),
        ("/probe-new", DoneStatus::Completed),
        ("resume", DoneStatus::Completed),
        ("native-tool-loop", DoneStatus::Completed),
        ("/probe-noop", DoneStatus::Completed),
        ("/probe-input", DoneStatus::Completed),
        ("error", DoneStatus::Errored),
        ("slow", DoneStatus::Interrupted),
        ("steering slow", DoneStatus::Completed),
        ("/probe-new", DoneStatus::Completed),
        ("/probe-metadata", DoneStatus::Completed),
    ] {
        let (steer, steering) = mpsc::channel(8);
        let interrupt = CancellationToken::new();
        let controls = RunControls {
            realtime: None,
            execution_lease: None,
            steering,
            interrupt: interrupt.clone(),
            request_input: Box::new(|questions| {
                let (tx, rx) = oneshot::channel();
                tx.send(vec![UserInputAnswer {
                    question_id: questions[0].id.clone(),
                    labels: vec!["local answer".into()],
                }])
                .unwrap();
                rx
            }),
            turn: Default::default(),
        };
        let request = RunRequest {
            prompt: prompt.into(),
            harness: None,
            model: Some("zeron-probe/mock".into()),
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume: session.clone(),
            attachments: vec![],
            worktree: None,
            resume_policy: Default::default(),
            mcp: None,
        };
        let previous_session = session.clone();
        let mut stream = harness.run(request, controls).await.unwrap();
        let mut sender = Some(steer);
        let mut done = 0;
        let mut confirmed = 0;
        let mut text = String::new();
        let mut points = vec![];
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::SessionStarted { session_id, .. } => {
                        if let Some(old) = &session {
                            if prompt != "/probe-new" {
                                assert_eq!(old, &session_id);
                            }
                        }
                        session = Some(session_id);
                        if prompt == "steering slow" {
                            sender
                                .take()
                                .unwrap()
                                .send(SteerMessage {
                                    prompt: "redirect".into(),
                                    message_id: None,
                                    attachments: Vec::new(),
                                    config: None,
                                })
                                .await
                                .unwrap();
                        } else {
                            sender.take();
                        }
                        if prompt == "slow" {
                            let token = interrupt.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(150)).await;
                                token.cancel();
                            });
                        }
                    }
                    AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                    AgentEvent::Steered { .. } => confirmed += 1,
                    AgentEvent::NativeForkReady { point, .. } => points.push(point),
                    AgentEvent::Done {
                        status,
                        error,
                        session_id,
                        ..
                    } => {
                        assert_eq!(
                            session_id, session,
                            "Done must publish the current native session identity"
                        );
                        assert_eq!(status, expected, "{prompt}: {error:?}");
                        done += 1;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("native Pi run must settle");
        assert_eq!(done, 1, "{prompt}: {text}");
        if prompt == "/probe-new" {
            assert_ne!(session, previous_session);
        }
        if prompt == "/probe-input" {
            assert!(text.contains("answer:local answer"), "{text}");
        }
        if prompt == "steering slow" {
            assert_eq!(confirmed, 1);
            assert!(text.contains("MOCK:redirect"), "{text}");
        }
        if matches!(prompt, "hello" | "resume" | "native-tool-loop") {
            assert_eq!(text, format!("MOCK:{prompt}"));
            assert_eq!(points.len(), 1, "authoritative native entry: {prompt}");
            assert_eq!(Some(&points[0].source_session_id), session.as_ref());
        }
        if expected != DoneStatus::Completed || prompt.starts_with('/') {
            assert!(points.is_empty(), "no point for {prompt}: {points:?}");
        }
    }
    // Unsaved extension entries are not equivalent to an empty conversation, so
    // the UUID is not recreated. Pi has no public RPC to restore them; the chat
    // continues in a new session and says so instead of failing every message.
    let (_, steering) = mpsc::channel(1);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| oneshot::channel().1),
        turn: Default::default(),
    };
    let request = RunRequest {
        prompt: "after loss".into(),
        harness: None,
        model: Some("zeron-probe/mock".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume_policy: Default::default(),
        resume: session.clone(),
        attachments: vec![],
        worktree: None,
        mcp: None,
    };
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(20),
        harness
            .run(request, controls)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
    )
    .await
    .expect("native Pi run must settle");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error { message }
            if message.contains("without the previous context"))),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done { status, session_id, .. }
            if *status == DoneStatus::Completed && session_id.is_some() && *session_id != session)),
        "{events:?}"
    );
}

async fn wait_probe_lines(path: &std::path::Path, count: usize) -> Vec<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let lines: Vec<_> = std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Pi probe must reach the expected barrier")
}

#[tokio::test]
#[ignore = "requires Pi 0.85.1, Node >=22.19, and npm; local mock, no API spend"]
async fn real_pi_native_fork_cuts_history_and_requires_child_resume() {
    use zeron_harness::NativeForkControls;
    use zeron_proto::ResumePolicy;
    let (dir, harness) = isolated_pi();
    let cwd = dir.path();
    async fn send(
        h: &PiHarness,
        cwd: &std::path::Path,
        prompt: &str,
        resume: Option<String>,
        strict: bool,
    ) -> Vec<AgentEvent> {
        let (tx, steering) = mpsc::channel(8);
        drop(tx);
        let request = RunRequest {
            prompt: prompt.into(),
            harness: None,
            model: Some("zeron-probe/mock".into()),
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume,
            resume_policy: if strict {
                ResumePolicy::RequireExisting
            } else {
                ResumePolicy::AllowFresh
            },
            attachments: vec![],
            worktree: None,
            mcp: None,
        };
        tokio::time::timeout(
            Duration::from_secs(30),
            h.run(
                request,
                RunControls {
                    realtime: None,
                    turn: Default::default(),
                    execution_lease: None,
                    steering,
                    interrupt: CancellationToken::new(),
                    request_input: Box::new(|_| oneshot::channel().1),
                },
            )
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
        )
        .await
        .unwrap()
    }
    let first = send(&harness, cwd, "U1", None, false).await;
    let point = first
        .iter()
        .find_map(|e| match e {
            AgentEvent::NativeForkReady { point, .. } => Some(point.clone()),
            _ => None,
        })
        .expect("exact completed native entry");
    assert!(matches!(
        first.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    let source = point.source_session_id.clone();
    send(&harness, cwd, "U2", Some(source.clone()), false).await;
    let third = send(&harness, cwd, "U3", Some(source.clone()), false).await;
    let tail = third
        .iter()
        .find_map(|e| match e {
            AgentEvent::NativeForkReady { point, .. } => Some(point),
            _ => None,
        })
        .unwrap();
    let files = || {
        std::fs::read_dir(cwd.join("agent/sessions"))
            .unwrap()
            .flat_map(|e| std::fs::read_dir(e.unwrap().path()).unwrap())
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>()
    };
    let source_file = files()
        .into_iter()
        .find(|p| p.to_string_lossy().contains(&source))
        .unwrap();
    let before = std::fs::read(&source_file).unwrap();
    assert!(harness.native_fork_support(cwd).await.available);
    for selected in [&point, tail] {
        let child = harness
            .fork_native(
                selected,
                NativeForkControls {
                    execution_lease: None,
                    interrupt: CancellationToken::new(),
                    timeout: Duration::from_secs(120),
                    source_idle: true,
                },
            )
            .await
            .unwrap();
        assert_ne!(child.session_id, source);
        // Creation is storage-only: no prompt, inference, or writes to the parent.
        assert_eq!(std::fs::read(&source_file).unwrap(), before);
        let child_file = files()
            .into_iter()
            .find(|p| p.to_string_lossy().contains(&child.session_id))
            .unwrap();
        let transcript = std::fs::read_to_string(&child_file).unwrap();
        assert!(transcript.contains("U1") && !transcript.contains("fork-context"));
        if selected == &point {
            assert!(!transcript.contains("U2") && !transcript.contains("U3"));
        }
        // New process: resume after restart before the first send.
        let resumed = send(
            &harness,
            cwd,
            "fork-context",
            Some(child.session_id.clone()),
            true,
        )
        .await;
        let text: String = resumed
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let context: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            context.as_array().unwrap().len(),
            if selected == &point { 3 } else { 7 }
        );
        assert!(text.contains("U1") && !text.contains("<conversation>"));
        if selected == &point {
            assert!(!text.contains("U2") && !text.contains("U3"));
        }
        let next = resumed
            .iter()
            .find_map(|e| match e {
                AgentEvent::NativeForkReady { point, .. } => Some(point),
                _ => None,
            })
            .unwrap();
        assert_eq!(next.source_session_id, child.session_id);
        // An extension may switch a normal Pi conversation, but must not move
        // a native child to an unrelated session during a strict run.
        let switched = send(
            &harness,
            cwd,
            "/probe-new",
            Some(child.session_id.clone()),
            true,
        )
        .await;
        assert!(matches!(
            switched.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Errored,
                ..
            })
        ));
        assert!(
            !switched
                .iter()
                .any(|e| matches!(e, AgentEvent::NativeForkReady { .. }))
        );
        // A disappeared native child cannot be recreated or start a fresh run.
        std::fs::remove_file(child_file).unwrap();
        let (tx, steering) = mpsc::channel(1);
        drop(tx);
        let missing = RunRequest {
            prompt: "must not execute".into(),
            harness: None,
            model: Some("zeron-probe/mock".into()),
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume: Some(child.session_id),
            resume_policy: ResumePolicy::RequireExisting,
            attachments: vec![],
            worktree: None,
            mcp: None,
        };
        assert!(
            harness
                .run(
                    missing,
                    RunControls {
                        realtime: None,
                        turn: Default::default(),
                        execution_lease: None,
                        steering,
                        interrupt: CancellationToken::new(),
                        request_input: Box::new(|_| oneshot::channel().1)
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&source_file).unwrap(), before);
    }
    // The parent's later native turn can stay active while an older fixed entry
    // is branched. The local provider holds the turn until cancellation.
    let (tx, steering) = mpsc::channel(1);
    drop(tx);
    let interrupt = CancellationToken::new();
    let active = RunRequest {
        prompt: "burst hold".into(),
        harness: None,
        model: Some("zeron-probe/mock".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: Some(source),
        resume_policy: ResumePolicy::AllowFresh,
        attachments: vec![],
        worktree: None,
        mcp: None,
    };
    let stream = harness
        .run(
            active,
            RunControls {
                realtime: None,
                turn: Default::default(),
                execution_lease: None,
                steering,
                interrupt: interrupt.clone(),
                request_input: Box::new(|_| oneshot::channel().1),
            },
        )
        .await
        .unwrap();
    wait_probe_lines(&cwd.join("probe-model-calls.jsonl"), 1).await;
    let held = std::fs::read(&source_file).unwrap();
    let child = harness
        .fork_native(
            &point,
            NativeForkControls {
                execution_lease: None,
                interrupt: CancellationToken::new(),
                timeout: Duration::from_secs(30),
                source_idle: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&source_file).unwrap(), held);
    let child_file = files()
        .into_iter()
        .find(|p| p.to_string_lossy().contains(&child.session_id))
        .unwrap();
    let copied = std::fs::read_to_string(child_file).unwrap();
    assert!(copied.contains("U1") && !copied.contains("U2") && !copied.contains("burst hold"));
    interrupt.cancel();
    let events: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), stream.map(Result::unwrap).collect())
            .await
            .unwrap();
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
}

#[tokio::test]
#[ignore = "requires Pi >= 0.85.1 installed; uses only a local mock provider"]
async fn real_pi_steering_bursts_share_the_next_model_call() {
    let (dir, harness) = isolated_pi();
    let cwd = dir.path();
    let (tx, steering) = mpsc::channel(8);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| oneshot::channel().1),
        turn: Default::default(),
    };
    let request = RunRequest {
        prompt: "burst hold".into(),
        harness: None,
        model: Some("zeron-probe/mock".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume_policy: Default::default(),
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
    };
    let mut stream = harness.run(request, controls).await.unwrap();
    let calls = cwd.join("probe-model-calls.jsonl");
    let inputs = cwd.join("probe-inputs.jsonl");
    assert_eq!(
        wait_probe_lines(&calls, 1).await[0],
        serde_json::json!(["burst hold"])
    );
    let burst: Vec<_> = (0..40).map(|i| format!("burst-{}", i / 2)).collect();
    for (i, prompt) in burst.iter().enumerate() {
        tx.send(SteerMessage {
            prompt: prompt.clone(),
            message_id: Some(format!("burst-user-{i}")),
            attachments: Vec::new(),
            config: None,
        })
        .await
        .unwrap();
    }
    assert_eq!(
        wait_probe_lines(&inputs, burst.len()).await,
        burst
            .iter()
            .map(|s| serde_json::json!(s))
            .collect::<Vec<_>>()
    );
    assert_eq!(wait_probe_lines(&calls, 1).await.len(), 1);
    std::fs::write(cwd.join("probe-release-initial"), "").unwrap();
    let snapshot = wait_probe_lines(&calls, 2).await;
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[1], serde_json::json!(burst));
    // Anything arriving after the next call began belongs to the following
    // step, rather than being claimed as part of the already-running call.
    let late = vec!["late-1", "late-2", "late-3"];
    for prompt in &late {
        tx.send(SteerMessage::text(*prompt)).await.unwrap();
    }
    drop(tx);
    wait_probe_lines(&inputs, burst.len() + late.len()).await;
    assert_eq!(wait_probe_lines(&calls, 2).await.len(), 2);
    std::fs::write(cwd.join("probe-release-burst"), "").unwrap();
    let snapshot = wait_probe_lines(&calls, 3).await;
    assert_eq!(snapshot.len(), 3);
    assert_eq!(snapshot[2], serde_json::json!(late));
    std::fs::write(cwd.join("probe-release-late"), "").unwrap();
    let mut confirmed = 0;
    let mut done = vec![];
    let mut text = String::new();
    let mut native = vec![];
    let mut current_assistant = None;
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::SessionStarted {
                    assistant_message_id,
                    ..
                } => current_assistant = Some(assistant_message_id),
                AgentEvent::Steered {
                    next_assistant_message_id,
                    ..
                } => {
                    confirmed += 1;
                    current_assistant = next_assistant_message_id;
                }
                AgentEvent::NativeForkReady {
                    assistant_message_id,
                    point,
                } => {
                    assert_eq!(Some(assistant_message_id), current_assistant);
                    native.push(point);
                }
                AgentEvent::Done { status, .. } => done.push(status),
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(confirmed, burst.len() + late.len());
    assert_eq!(done, vec![DoneStatus::Completed]);
    assert_eq!(
        text,
        format!(
            "MOCK:burst holdMOCK:{}MOCK:{}",
            burst.join("|"),
            late.join("|")
        )
    );
    assert_eq!(wait_probe_lines(&calls, 3).await.len(), 3);
    assert_eq!(
        native.len(),
        3,
        "one native point per actual completed response, not per queued steer"
    );
    let file = std::fs::read_dir(cwd.join("agent/sessions"))
        .unwrap()
        .flat_map(|d| std::fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().path())
        .next()
        .unwrap();
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (point, expected) in native.iter().zip([
        "MOCK:burst hold".to_string(),
        format!("MOCK:{}", burst.join("|")),
        format!("MOCK:{}", late.join("|")),
    ]) {
        let zeron_proto::NativeForkBoundary::PiEntry { entry_id } = &point.boundary else {
            panic!("Pi entry")
        };
        let entry = entries.iter().find(|e| e["id"] == *entry_id).unwrap();
        assert_eq!(entry["message"]["content"][0]["text"], expected);
    }
}
