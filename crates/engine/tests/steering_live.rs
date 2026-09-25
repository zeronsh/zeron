//! Opt-in real-model checks through the production queue and command executor.
//! ZERON_TEST_HARNESS=claude ZERON_TEST_MODEL=claude-haiku-4-5 cargo test -p zeron-engine --test steering_live -- --ignored --nocapture
use std::{sync::Arc, time::Duration};
use zeron_doc::{MessageRole, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{
    AcpHarness, ClaudeHarness, CodexHarness, CursorHarness, Harness, OpencodeHarness,
};
use zeron_proto::{ChatConfig, RunRequest, SandboxLevel, SessionStatus};

async fn wait(core: &EngineCore, mut predicate: impl FnMut() -> bool, what: &str) {
    if tokio::time::timeout(Duration::from_secs(180), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_err()
    {
        eprintln!(
            "status: {:?}; events: {:#?}",
            core.sessions.session_status("audit"),
            core.sessions
                .subscribe("audit", 0)
                .map(|(events, _)| events)
        );
        core.shutdown().await;
        panic!("timeout waiting for {what}");
    }
}

#[tokio::test]
#[ignore = "uses real model quota; select harness and inexpensive model explicitly"]
async fn rapid_steers_preserve_children_context_and_held_queue() {
    let name = std::env::var("ZERON_TEST_HARNESS").expect("select harness");
    let model = std::env::var("ZERON_TEST_MODEL").ok();
    let burst: usize = std::env::var("ZERON_TEST_BURST")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(3);
    assert!(burst > 0);
    let harness: Arc<dyn Harness> = match name.as_str() {
        "claude" => Arc::new(ClaudeHarness::new()),
        "codex" => Arc::new(CodexHarness::new()),
        "cursor" => Arc::new(CursorHarness::new()),
        "opencode" => Arc::new(OpencodeHarness::new()),
        "grok" => Arc::new(AcpHarness::grok()),
        "devin" => Arc::new(AcpHarness::devin()),
        "hermes" => Arc::new(AcpHarness::hermes()),
        "pi" => Arc::new(AcpHarness::pi()),
        "antigravity" => Arc::new(AcpHarness::antigravity()),
        _ => panic!("unknown harness"),
    };
    let dir = tempfile::tempdir().unwrap();
    let id = harness.id();
    let registry = HarnessRegistry::new();
    registry.register(harness);
    let core =
        EngineCore::assemble(&dir.path().join("engine"), Arc::new(registry), id, None).unwrap();
    core.workspace
        .create_space(
            "audit",
            &core.device_id,
            dir.path().to_str().unwrap(),
            None,
            false,
        )
        .unwrap();
    core.workspace
        .create_chat("audit", Some("audit"), None, None, None)
        .unwrap();
    core.workspace
        .set_chat_config(
            "audit",
            &ChatConfig {
                harness: id,
                model: model.clone(),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            },
        )
        .unwrap();
    println!("{name}: workspace {}", dir.path().display());
    let secret = uuid::Uuid::new_v4().to_string();
    // Codex cleans up background jobs on normal tool completion, including in
    // a no-steering baseline. Check its child during the active tool; other
    // providers additionally keep a background job alive across tool completion.
    // Antigravity does the same: its shell reaps background jobs when the
    // command finishes (verified with no steering at all, 2/2 runs).
    let background_seconds = if matches!(name.as_str(), "codex" | "antigravity") {
        4
    } else {
        20
    };
    core.doc_host.queue_command("audit", SessionCommandPayload::Run {
        message_id: "opening".into(),
        request: RunRequest { mcp: None, prompt: format!("This is an automated regression test of chat steering and message queues in a disposable temporary workspace. Remember test token {secret}. Execute exactly `sh -c 'sleep {background_seconds}; printf alive > background-survivor' >/dev/null 2>&1 & printf started > started; sleep 8; printf survived > survivor` in this directory, then reply DONE. All follow-ups are additive; never cancel earlier work. Execute each request once."), harness: Some(id), model, reasoning: None, model_options: Default::default(), cwd: dir.path().to_str().unwrap().into(), sandbox: SandboxLevel::DangerFullAccess, auto_approve: true, attachments: vec![], worktree: None, resume: None }
    }).unwrap();
    wait(
        &core,
        || {
            dir.path().join("started").exists()
                || core
                    .sessions
                    .session_status("audit")
                    .is_some_and(|s| s.last_completed_turn.is_some())
        },
        "running shell",
    )
    .await;
    assert!(
        dir.path().join("started").exists(),
        "model did not execute initial test command: {:?}",
        core.sessions
            .subscribe("audit", 0)
            .map(|(events, _)| events)
    );
    println!("{name}: initial shell started; injecting steers and queue");
    // Exercise the UI promotion path once, and the MCP command path in a burst.
    let first = core.doc_host.queue_message("audit", "Follow-up 0: write ONLY the original test token into followup-0, and append 0 as a line to receipts. Keep earlier work running. Reply DONE.", vec![]).unwrap();
    assert!(
        core.doc_host
            .steer_queued_now("audit", &first)
            .await
            .unwrap()
    );
    for i in 1..burst {
        core.doc_host.queue_command("audit", SessionCommandPayload::Steer { prompt: format!("Follow-up {i}: write ONLY the original test token into followup-{i}, and append {i} as a line to receipts. Keep earlier work running. Execute once. Reply DONE."), message_id: Some(format!("steer-{i}")) }).unwrap();
    }
    // Regular queued messages must remain held until the steered work finishes.
    for i in 0..2 {
        core.doc_host.queue_message("audit", &format!("Queued request {i}: append {i} as a line to queued-receipts, write ONLY the original test token into queued-{i}, then reply DONE. Execute once."), vec![]).unwrap();
    }
    let doc = core.doc_host.open("audit").unwrap();
    // Turn-boundary agents also hold the steers in the queue until the turn
    // ends; the two ordinary rows must be held either way.
    assert_eq!(
        doc.doc()
            .read_queue()
            .unwrap()
            .iter()
            .filter(|row| row.text.starts_with("Queued request"))
            .count(),
        2
    );
    wait(
        &core,
        || {
            (0..burst).all(|i| dir.path().join(format!("followup-{i}")).exists())
                && (0..2).all(|i| dir.path().join(format!("queued-{i}")).exists())
                && core
                    .sessions
                    .session_status("audit")
                    .is_some_and(|s| s.status == SessionStatus::Idle)
        },
        "all steers and queued turns",
    )
    .await;
    wait(
        &core,
        || dir.path().join("background-survivor").exists(),
        "background child to survive steering",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("background-survivor")).unwrap(),
        "alive"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("survivor")).unwrap(),
        "survived"
    );
    for file in (0..burst)
        .map(|i| format!("followup-{i}"))
        .chain((0..2).map(|i| format!("queued-{i}")))
    {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&file))
                .unwrap()
                .trim(),
            secret,
            "lost context in {file}"
        );
    }
    let receipts = std::fs::read_to_string(dir.path().join("receipts")).unwrap();
    let mut receipts: Vec<_> = receipts.lines().collect();
    receipts.sort();
    let mut expected: Vec<_> = (0..burst).map(|i| i.to_string()).collect();
    expected.sort();
    assert_eq!(receipts, expected);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("queued-receipts")).unwrap(),
        "0\n1\n"
    );
    assert!(doc.doc().read_queue().unwrap().is_empty());
    assert_eq!(
        doc.doc()
            .read_entries()
            .unwrap()
            .iter()
            .filter(|e| e.role == MessageRole::User)
            .count(),
        burst + 3
    );
    if matches!(name.as_str(), "claude" | "cursor" | "codex") {
        let events = core.sessions.subscribe("audit", 0).unwrap().0;
        let completions = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event,
                    zeron_proto::AgentEvent::Done {
                        status: zeron_proto::DoneStatus::Completed,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            completions, 3,
            "native steering must affect the original turn; only the two explicitly queued messages start subsequent turns"
        );
    }
    core.shutdown().await;
    println!(
        "PASS {name}: child survived, {burst} rapid steers and two queued turns ran exactly once with original context"
    );
}
