//! Pi's native session id survives an idle adapter crash through dispatch.
use std::{sync::Arc, time::Duration};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::AcpHarness;
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

#[tokio::test]
async fn pi_idle_crash_next_dispatch_loads_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../harness/tests/fixtures/fake-pi-acp.sh");
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(
        AcpHarness::pi()
            .with_executable(fixture)
            .with_graces(Duration::from_millis(50), Duration::from_millis(100)),
    ));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Pi, None).unwrap();
    let chat = "pi-idle-crash";
    let handle = core.doc_host.open(chat).unwrap();
    for prompt in ["idle-crash", "require-resume"] {
        let req = RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: dir.path().display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
        };
        core.sessions
            .dispatch(chat, HarnessId::Pi, req, None)
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
        }).await.expect("fixture requires session/load on the next dispatch");
        // Let the fixture exit and the driver remove its live mailbox.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
