//! Oh My Pi's native session file survives an idle process crash through dispatch.
#![cfg(unix)]
use std::{sync::Arc, time::Duration};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::OmpHarness;
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

fn core(dir: &std::path::Path) -> EngineCore {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../harness/tests/fixtures/fake-omp-rpc.py");
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(
        OmpHarness::new()
            .with_executable(fixture)
            .with_agent_dir(dir.join("agent"))
            .with_session_store(dir.join("index"))
            .with_graces(Duration::from_millis(50), Duration::from_millis(100)),
    ));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Omp, None).unwrap()
}

fn request(prompt: &str, cwd: &std::path::Path) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

#[tokio::test]
async fn omp_idle_crash_next_dispatch_resumes_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path());
    let chat = "omp-idle-crash";
    let handle = core.doc_host.open(chat).unwrap();
    for prompt in ["idle-crash", "require-resume"] {
        core.sessions
            .dispatch(chat, HarnessId::Omp, request(prompt, dir.path()), None)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let entries = handle.doc().read_entries().unwrap();
                if entries.iter().any(|entry| entry.parts.iter().any(|part|
                    matches!(part, zeron_doc::MessagePart::Text { text, .. } if text == &format!("reply:{prompt}"))
                )) { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("fixture requires --resume on the next dispatch");
        // Let the fixture exit and the driver remove its live mailbox.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    core.shutdown().await;
}

#[tokio::test]
async fn omp_timed_out_question_resolves_and_finishes_without_user_input() {
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path());
    let chat = "omp-question-timeout";
    let handle = core.doc_host.open(chat).unwrap();
    core.sessions
        .dispatch(chat, HarnessId::Omp, request("/question", dir.path()), None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !core
            .sessions
            .session_status(chat)
            .is_some_and(|s| s.status == zeron_proto::SessionStatus::AwaitingInput)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        loop {
            let entries = handle.doc().read_entries().unwrap();
            if entries.iter().any(|entry| {
                entry.parts.iter().any(|part| {
                    matches!(part, zeron_doc::MessagePart::Input { resolved: true, .. })
                })
            }) && core
                .sessions
                .session_status(chat)
                .is_some_and(|s| s.status == zeron_proto::SessionStatus::Idle)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the expired question must not strand the session");
    core.shutdown().await;
}
