//! Opt-in real Droid checks. All session/project writes stay in a temp dir;
//! authentication is inherited from FACTORY_API_KEY, never copied or logged.
use futures::StreamExt;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, DroidHarness, Harness, RunControls};
use zeron_proto::{AccessMode, AgentEvent, DoneStatus, RunRequest, UserInputAnswer};

async fn run_live(
    project: &Path,
    factory: &Path,
    plan: bool,
    approve: bool,
    child: bool,
) -> (Vec<AgentEvent>, usize) {
    let executable = std::env::var_os("DROID_LIVE_EXECUTABLE")
        .map(PathBuf::from)
        .expect("Set DROID_LIVE_EXECUTABLE to the installed Droid binary");
    assert!(
        std::env::var_os("FACTORY_API_KEY").is_some(),
        "Set FACTORY_API_KEY for the isolated live session"
    );
    let harness = DroidHarness::new().with_executable(executable);
    let target = project.join("approved.txt");
    let text = Arc::new(Mutex::new(String::new()));
    let observed = text.clone();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let prompt = if child {
        "Use Task to spawn exactly one worker subagent whose task is to Read fixture.txt and report its contents. Wait for it to finish, then reply DONE. Do not inspect other files or modify anything."
    } else if plan {
        "In Spec mode, call ExitSpecMode now with a short Markdown plan to create approved.txt containing exactly APPROVED. Do not inspect files or ask questions. Do not create or edit anything until approval. If approved, use Create to write approved.txt containing APPROVED, then finish. If rejected, stop without any changes."
    } else {
        "Reply with exactly LIVE_DROID_OK. Do not use tools."
    };
    let model = std::env::var("DROID_LIVE_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".into());
    let mut request: RunRequest = serde_json::from_value(serde_json::json!({
        "prompt": prompt, "model": model, "reasoning": "low",
        "cwd": project, "sandbox": "workspace-write", "plan": plan,
    }))
    .unwrap();
    request.access = if approve {
        AccessMode::AutoAcceptEdits
    } else {
        AccessMode::FullAccess
    };
    request.env.insert(
        "FACTORY_HOME_OVERRIDE".into(),
        factory.display().to_string(),
    );
    let (_steer, steering) = mpsc::channel(8);
    let interrupt = CancellationToken::new();
    let mut events = harness
        .run(
            request,
            RunControls {
                realtime: None,
                execution_lease: None,
                steering,
                interrupt: interrupt.clone(),
                turn: Default::default(),
                request_input: Box::new(move |questions| {
                    assert!(plan, "plain turn unexpectedly asked a question");
                    assert_eq!(questions.len(), 1);
                    assert_eq!(questions[0].header, "Approve plan");
                    assert!(!target.exists(), "Droid edited before plan approval");
                    assert!(
                        !observed.lock().unwrap().trim().is_empty(),
                        "plan Markdown must arrive before the question"
                    );
                    count.fetch_add(1, Ordering::SeqCst);
                    let (tx, rx) = oneshot::channel();
                    tx.send(vec![UserInputAnswer {
                        question_id: questions[0].id.clone(),
                        labels: vec![if approve { "Yes" } else { "No" }.into()],
                    }])
                    .unwrap();
                    rx
                }),
            },
        )
        .await
        .unwrap();
    let all = tokio::time::timeout(Duration::from_secs(180), async {
        let mut all = vec![];
        let mut parent_done = false;
        let mut child_done = false;
        loop {
            let event = events
                .next()
                .await
                .expect("live stream ended before terminal")
                .unwrap();
            if let AgentEvent::TextDelta { text: delta } = &event {
                text.lock().unwrap().push_str(delta);
            }
            parent_done |= matches!(event, AgentEvent::Done { .. });
            child_done |= matches!(&event, AgentEvent::Subagent { event, .. } if matches!(event.as_ref(), AgentEvent::Done { .. }));
            all.push(event);
            if parent_done && (!child || child_done) {
                break all;
            }
        }
    })
    .await
    .expect("live Droid turn timed out");
    interrupt.cancel();
    tokio::time::timeout(Duration::from_secs(10), async {
        while events.next().await.is_some() {}
    })
    .await
    .expect("Droid cleanup");
    (all, asked.load(Ordering::SeqCst))
}

fn isolated() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let factory = temp.path().join("factory");
    std::fs::create_dir(&project).unwrap();
    (temp, project, factory)
}

#[tokio::test]
#[ignore = "requires DROID_LIVE_EXECUTABLE and FACTORY_API_KEY; spends tokens"]
async fn droid_live_plain_turn() {
    let (_temp, project, factory) = isolated();
    let (all, asked) = run_live(&project, &factory, false, false, false).await;
    assert_eq!(asked, 0);
    let reply: String = all
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reply.trim(), "LIVE_DROID_OK");
    assert!(matches!(
        all.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
#[ignore = "requires DROID_LIVE_EXECUTABLE and FACTORY_API_KEY; spends tokens and edits only a temp project"]
async fn droid_live_spec_approve_edits() {
    let (_temp, project, factory) = isolated();
    let (all, asked) = run_live(&project, &factory, true, true, false).await;
    assert_eq!(asked, 1);
    assert!(matches!(
        all.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert_eq!(
        std::fs::read_to_string(project.join("approved.txt"))
            .unwrap()
            .trim(),
        "APPROVED"
    );
}

#[tokio::test]
#[ignore = "requires DROID_LIVE_EXECUTABLE and FACTORY_API_KEY; spends tokens"]
async fn droid_live_spec_reject_makes_no_edits() {
    let (_temp, project, factory) = isolated();
    let (all, asked) = run_live(&project, &factory, true, false, false).await;
    assert_eq!(asked, 1, "Full access must still ask for plan approval");
    assert!(matches!(
        all.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    assert_eq!(std::fs::read_dir(project).unwrap().count(), 0);
}

#[tokio::test]
#[ignore = "requires DROID_LIVE_EXECUTABLE and FACTORY_API_KEY; spends tokens"]
async fn droid_live_subagent_transcript() {
    let (_temp, project, factory) = isolated();
    std::fs::write(project.join("fixture.txt"), "LIVE_CHILD_OK\n").unwrap();
    let (all, asked) = run_live(&project, &factory, false, false, true).await;
    assert_eq!(asked, 0);
    let spawn = all
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => Some(id),
            _ => None,
        })
        .expect("Task spawn");
    let child: Vec<_> = all
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if parent_tool_use_id == spawn => Some(event.as_ref()),
            _ => None,
        })
        .collect();
    assert!(matches!(child[0], AgentEvent::UserMessage { .. }));
    assert!(child.iter().any(|event| matches!(event, AgentEvent::ToolCall { id, call: zeron_proto::ToolCall::ReadFile { path } }
        if path.ends_with("fixture.txt") && !id.starts_with("droid-progress-"))));
    assert!(child.iter().any(|event| matches!(event, AgentEvent::ToolResult { output: Some(text), is_error: false, .. } if text.contains("LIVE_CHILD_OK"))));
    assert!(child.iter().any(
        |event| matches!(event, AgentEvent::TextDelta { text } if text.contains("LIVE_CHILD_OK"))
    ));
    assert!(matches!(
        child.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert!(all.iter().any(|event| matches!(
        event,
        AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        }
    )));
}

#[tokio::test]
#[ignore = "requires DROID_LIVE_EXECUTABLE and Droid login or FACTORY_API_KEY; spends tokens"]
async fn droid_live_title_has_no_tools() {
    let temp = tempfile::tempdir().unwrap();
    let executable = std::env::var_os("DROID_LIVE_EXECUTABLE")
        .map(PathBuf::from)
        .expect("Set DROID_LIVE_EXECUTABLE to the installed Droid binary");
    let harness = DroidHarness::new().with_executable(executable);
    harness.model_catalog(false).await.unwrap();
    let model = std::env::var("DROID_LIVE_MODEL").unwrap_or_else(|_| "gpt-6-luna".into());
    let request = serde_json::from_value(serde_json::json!({
        "prompt":"Add a keyboard shortcut for opening the settings panel",
        "model":model,"reasoning":"minimal","cwd":temp.path(),
        "sandbox":"workspace-write"
    }))
    .unwrap();
    let (_steer, steering) = mpsc::channel(8);
    let interrupt = CancellationToken::new();
    let mut events = harness
        .run_title(
            request,
            RunControls {
                realtime: None,
                execution_lease: None,
                steering,
                interrupt: interrupt.clone(),
                turn: Default::default(),
                request_input: Box::new(|_| panic!("title generation must never ask for input")),
            },
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let mut text = String::new();
        let mut completed = false;
        while let Some(event) = events.next().await {
            match event.unwrap() {
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                AgentEvent::Done { status, error, .. } => {
                    assert_eq!(status, DoneStatus::Completed, "{error:?}");
                    assert!(!completed);
                    completed = true;
                }
                AgentEvent::ToolCall { .. }
                | AgentEvent::Error { .. }
                | AgentEvent::InputRequested { .. }
                | AgentEvent::Subagent { .. } => panic!("unexpected title event"),
                _ => {}
            }
        }
        assert!(completed);
        let words = text.split_whitespace().count();
        assert!((3..=5).contains(&words), "expected a short title: {text:?}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        eprintln!("Droid title model={model}: {}", text.trim());
    })
    .await;
    interrupt.cancel();
    result.expect("live Droid title timed out");
}
