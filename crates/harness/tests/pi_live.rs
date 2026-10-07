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
            subagent_control: None,
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
            mcp: None,
        };
        let previous_session = session.clone();
        let mut stream = harness.run(request, controls).await.unwrap();
        let mut sender = Some(steer);
        let mut done = 0;
        let mut confirmed = 0;
        let mut text = String::new();
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
        if matches!(prompt, "hello" | "resume") {
            assert_eq!(text, format!("MOCK:{prompt}"));
        }
    }
    // Unsaved extension entries are not equivalent to an empty conversation, so
    // the UUID is not recreated. Pi has no public RPC to restore them; the chat
    // continues in a new session and says so instead of failing every message.
    let (_, steering) = mpsc::channel(1);
    let controls = RunControls {
        subagent_control: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| oneshot::channel().1),
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
#[ignore = "requires Pi >= 0.85.1 installed; uses only a local mock provider"]
async fn real_pi_steering_bursts_share_the_next_model_call() {
    let (dir, harness) = isolated_pi();
    let cwd = dir.path();
    let (tx, steering) = mpsc::channel(8);
    let controls = RunControls {
        subagent_control: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| oneshot::channel().1),
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
        tx.send(SteerMessage {
            prompt: (*prompt).into(),
            message_id: None,
        })
        .await
        .unwrap();
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
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Steered { .. } => confirmed += 1,
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
}
