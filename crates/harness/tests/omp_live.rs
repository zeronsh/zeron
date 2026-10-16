//! Real Oh My Pi, an isolated home and agent directory, and a local provider
//! extension (no network/API spend).
#![cfg(unix)]
use futures::StreamExt;
use std::{os::unix::fs::PermissionsExt, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, Harness, OmpHarness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel, UserInputAnswer};

fn isolated_omp() -> (tempfile::TempDir, OmpHarness) {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("agent");
    let extensions = agent.join("extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    std::fs::write(
        agent.join("config.yml"),
        "retry:\n  enabled: false\ncompaction:\n  enabled: false\n",
    )
    .unwrap();
    std::fs::write(
        extensions.join("probe.ts"),
        include_str!("fixtures/omp-rpc-probe.ts"),
    )
    .unwrap();
    // Installed plugins live under ~/.omp, outside the agent dir: isolate HOME.
    let exe = OmpHarness::new()
        .resolve_executable()
        .expect("Oh My Pi CLI installed");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let quote =
        |p: &std::path::Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    let wrapper = dir.path().join("omp-probe");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport HOME={}\nexec {} \"$@\"\n",
            quote(&home),
            quote(&exe)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    // The explicit agent dir also shields the run from OMP_PROFILE.
    let harness = OmpHarness::new()
        .with_executable(wrapper)
        .with_agent_dir(&agent)
        .with_session_store(dir.path().join("index"));
    (dir, harness)
}

fn controls(answer: &'static str) -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer, steering) = mpsc::channel(8);
    let interrupt = CancellationToken::new();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering,
            interrupt: interrupt.clone(),
            request_input: Box::new(move |questions| {
                let (tx, rx) = oneshot::channel();
                tx.send(vec![UserInputAnswer {
                    question_id: questions[0].id.clone(),
                    labels: vec![answer.into()],
                }])
                .unwrap();
                rx
            }),
            turn: Default::default(),
        },
        steer,
        interrupt,
    )
}

fn request(cwd: &std::path::Path, prompt: &str, resume: Option<String>) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: Some("zeron-probe/mock".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume,
        attachments: vec![],
        worktree: None,
        mcp: None,
    }
}

#[tokio::test]
#[ignore = "requires Oh My Pi >= 18.11.0 installed; uses only a local mock provider"]
async fn real_omp_mock_lifecycle() {
    let (dir, harness) = isolated_omp();
    let cwd = dir.path();
    let mut session: Option<String> = None;
    for (prompt, expected) in [
        ("/probe-noop", DoneStatus::Completed),
        ("hello", DoneStatus::Completed),
        ("resume", DoneStatus::Completed),
        ("/probe-input", DoneStatus::Completed),
        ("error", DoneStatus::Errored),
        ("slow", DoneStatus::Interrupted),
        ("steering slow", DoneStatus::Completed),
        ("/probe-new", DoneStatus::Completed),
        ("after new", DoneStatus::Completed),
    ] {
        let (c, steer, interrupt) = controls("local answer");
        let previous = session.clone();
        let mut stream = harness
            .run(request(cwd, prompt, session.clone()), c)
            .await
            .unwrap();
        let mut sender = Some(steer);
        let mut done = 0;
        let mut confirmed = 0;
        let mut errors = vec![];
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::SessionStarted { session_id, .. } => {
                        session = Some(session_id);
                        if prompt == "steering slow" {
                            if let Some(tx) = sender.take() {
                                tx.send(SteerMessage::text("redirect")).await.unwrap();
                            }
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
                    AgentEvent::Error { message } => errors.push(message),
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
                        assert_eq!(status, expected, "{prompt}: {error:?} {text}");
                        done += 1;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("native OMP run must settle");
        assert_eq!(done, 1, "{prompt}: {text}");
        match prompt {
            "hello" | "resume" => assert_eq!(text, format!("MOCK:{prompt}")),
            "/probe-input" => assert!(text.contains("answer:local answer"), "{text}"),
            "steering slow" => {
                assert_eq!(confirmed, 1);
                assert!(text.contains("MOCK:redirect"), "{text}");
            }
            "/probe-new" => assert_ne!(session, previous),
            _ => {}
        }
        if matches!(
            prompt,
            "resume" | "/probe-input" | "error" | "slow" | "steering slow"
        ) {
            assert_eq!(session, previous, "{prompt} resumes the same session");
        }
        // A new session has no file until its first turn, so restarting it
        // starts fresh without claiming lost context.
        assert!(errors.is_empty(), "{prompt}: {errors:?}");
    }
    // A submitted session whose file is gone cannot be restored: the chat
    // continues in a new session and says so instead of failing every message.
    let file = std::fs::read_dir(cwd.join("agent/sessions"))
        .unwrap()
        .flat_map(|project| std::fs::read_dir(project.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| text.contains(session.as_deref().unwrap()))
        })
        .expect("the last session was persisted");
    std::fs::remove_file(file).unwrap();
    let (c, tx, _) = controls("unused");
    drop(tx);
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(30),
        harness
            .run(request(cwd, "after loss", session.clone()), c)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
    )
    .await
    .expect("native OMP run must settle");
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

#[tokio::test]
#[ignore = "requires Oh My Pi >= 18.11.0 installed; uses only a local mock provider"]
async fn real_omp_catalog_lists_extension_models_and_commands() {
    let (dir, harness) = isolated_omp();
    let models = harness.models().await.unwrap();
    assert!(
        models.iter().any(|m| m.id == "zeron-probe/mock"),
        "{models:?}"
    );
    let commands = harness.commands_for(dir.path()).await.unwrap();
    for expected in ["probe-noop", "probe-new", "compact"] {
        assert!(
            commands.iter().any(|c| c.name == expected),
            "{expected}: {commands:?}"
        );
    }
}

/// The shared Pi MCP extension loads in OMP, and its tools reach the model
/// directly: OMP would otherwise defer extension tools behind discovery.
#[tokio::test]
#[ignore = "requires Oh My Pi >= 18.11.0 and node installed; uses only a local mock provider"]
async fn real_omp_offers_zeron_mcp_tools_to_the_model() {
    let (dir, harness) = isolated_omp();
    let cwd = dir.path();
    let mut req = request(cwd, "tools", None);
    req.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: "node".into(),
        args: vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pi-mcp-test.mjs")
                .display()
                .to_string(),
            "server".into(),
        ],
        env: [("ZERON_CHAT_ID".to_string(), "live".to_string())].into(),
    });
    let (c, tx, _) = controls("unused");
    drop(tx);
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(30),
        harness
            .run(req, c)
            .await
            .unwrap()
            .map(Result::unwrap)
            .collect(),
    )
    .await
    .expect("native OMP run must settle");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done { status, .. }
            if *status == DoneStatus::Completed)),
        "{events:?}"
    );
    let tools = wait_probe_lines(&cwd.join("probe-tools.jsonl"), 1).await;
    let offered: Vec<_> = tools[0]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.as_str())
        .collect();
    for expected in ["zeron_whoami", "zeron_send_message", "zeron_wait_for_turn"] {
        assert!(offered.contains(&expected), "{expected}: {offered:?}");
    }
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
    .expect("OMP probe must reach the expected barrier")
}

#[tokio::test]
#[ignore = "requires Oh My Pi >= 18.11.0 installed; uses only a local mock provider"]
async fn real_omp_steering_bursts_share_the_next_model_call() {
    let (dir, harness) = isolated_omp();
    let cwd = dir.path();
    let (c, tx, _) = controls("unused");
    let mut stream = harness
        .run(request(cwd, "burst hold", None), c)
        .await
        .unwrap();
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
}
