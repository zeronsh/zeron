//! Opt-in real-model check that a steer takes effect IMMEDIATELY, the way
//! Codex's `turn/steer` does: while the agent is still streaming a long
//! answer (a long story), the composer queues a row and the user presses Steer
//! (`QueueMessage` + `SteerQueuedMessageNow`, exactly what the UI sends).
//! The current answer must stop early and the steer must be answered within
//! the same live runtime — not after the long answer finishes.
//!
//! ZERON_TEST_HARNESS=codex cargo test -p zeron-engine --test steer_now_live -- --ignored --nocapture
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{
    AcpHarness, ClaudeHarness, CodexHarness, CursorHarness, Harness, OpencodeHarness,
};
use zeron_proto::{ChatConfig, RunRequest, SandboxLevel, SessionStatus};

const CHAT: &str = "steer-now";

fn harness(name: &str) -> Arc<dyn Harness> {
    match name {
        "claude" => Arc::new(ClaudeHarness::new()),
        "codex" => Arc::new(CodexHarness::new()),
        "cursor" => Arc::new(CursorHarness::new()),
        "opencode" => Arc::new(OpencodeHarness::new()),
        "grok" => Arc::new(AcpHarness::grok()),
        "devin" => Arc::new(AcpHarness::devin()),
        "hermes" => Arc::new(AcpHarness::hermes()),
        "pi" => Arc::new(AcpHarness::pi()),
        "antigravity" => Arc::new(AcpHarness::antigravity()),
        _ => panic!("unknown harness {name}"),
    }
}

fn text(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn dump(entries: &[SessionMessageEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            let t = text(e);
            let errors: Vec<_> = e
                .parts
                .iter()
                .filter_map(|p| match p {
                    MessagePart::Error { message, .. } => Some(message.as_str()),
                    _ => None,
                })
                .collect();
            format!(
                "  {:?} [{} words] {:?}{}",
                e.role,
                t.split_whitespace().count(),
                t.chars().take(100).collect::<String>(),
                if errors.is_empty() {
                    String::new()
                } else {
                    format!(" ERR {errors:?}")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
#[ignore = "uses real model quota; select the harness explicitly"]
async fn steer_now_interrupts_a_streaming_answer() {
    let name = std::env::var("ZERON_TEST_HARNESS").expect("select harness");
    let model = std::env::var("ZERON_TEST_MODEL").ok();
    let bursts: usize = std::env::var("ZERON_TEST_BURST")
        .ok()
        .map(|b| b.parse().unwrap())
        .unwrap_or(1);
    let harness = harness(&name);
    let id = harness.id();
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(harness);
    let core =
        EngineCore::assemble(&dir.path().join("engine"), Arc::new(registry), id, None).unwrap();
    let cwd = dir.path().to_str().unwrap().to_owned();
    core.workspace
        .create_space(CHAT, &core.device_id, &cwd, None, false)
        .unwrap();
    core.workspace
        .create_chat(CHAT, Some(CHAT), None, None, None)
        .unwrap();
    core.workspace
        .set_chat_config(
            CHAT,
            &ChatConfig {
                harness: id,
                model: model.clone(),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            },
        )
        .unwrap();
    let entries = || {
        core.doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
    };
    let status = || core.sessions.session_status(CHAT).map(|s| s.status);
    let assistant_text = |es: &[SessionMessageEntry]| {
        es.iter()
            .filter(|e| e.role == MessageRole::Assistant)
            .map(text)
            .collect::<Vec<_>>()
            .join("\n")
    };

    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                message_id: "story".into(),
                request: RunRequest {
                    mcp: None,
                    prompt: "Write a long, detailed story (about 1500 words) about a lighthouse keeper. Do not use any tools."
                        .into(),
                    harness: Some(id),
                    model,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: cwd.clone(),
                    sandbox: SandboxLevel::DangerFullAccess,
                    auto_approve: true,
                    attachments: vec![],
                    worktree: None,
                    resume: None,
                },
            },
        )
        .unwrap();
    let started = Instant::now();
    // Steer once the story is visibly streaming.
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(240),
            "story never streamed\n{}",
            dump(&entries())
        );
        if assistant_text(&entries()).split_whitespace().count() >= 40 {
            break;
        }
        if status() == Some(SessionStatus::Idle) && started.elapsed() > Duration::from_secs(5) {
            panic!("story finished before steering\n{}", dump(&entries()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let words_at_steer = assistant_text(&entries()).split_whitespace().count();
    let steer_at = Instant::now();
    // ZERON_TEST_CLICK_GAP_MS: the UI flow of queueing every message first
    // (Enter while busy) and then pressing Steer on each row in turn.
    if let Some(gap) = std::env::var("ZERON_TEST_CLICK_GAP_MS")
        .ok()
        .map(|g| Duration::from_millis(g.parse().unwrap()))
    {
        let rows: Vec<String> = (0..bursts)
            .map(|i| {
                core.doc_host
                    .queue_message(
                        CHAT,
                        &if i == 0 {
                            format!("Please pause the story here. Just reply with the word PINEAPPLE{i}.")
                        } else {
                            format!("Also include the word PINEAPPLE{i} in that reply.")
                        },
                        vec![],
                    )
                    .unwrap()
            })
            .collect();
        for row in rows {
            tokio::time::sleep(gap).await;
            assert!(core.doc_host.steer_queued_now(CHAT, &row).await.unwrap());
        }
    }
    for i in (0..bursts).filter(|_| std::env::var("ZERON_TEST_CLICK_GAP_MS").is_err()) {
        let word = format!("PINEAPPLE{i}");
        let row = core
            .doc_host
            .queue_message(
                CHAT,
                &if i == 0 {
                    format!("Please pause the story here. Just reply with the word {word}.")
                } else {
                    format!("Also include the word {word} in that reply.")
                },
                vec![],
            )
            .unwrap();
        assert!(core.doc_host.steer_queued_now(CHAT, &row).await.unwrap());
    }
    let last_word = format!("PINEAPPLE{}", bursts - 1);
    let mut answered_at = None;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let es = entries();
        if answered_at.is_none() && assistant_text(&es).contains(&last_word) {
            answered_at = Some(steer_at.elapsed());
        }
        if answered_at.is_some() && status() == Some(SessionStatus::Idle) {
            break;
        }
        if Instant::now() > deadline {
            panic!("steer never answered\n{}", dump(&es));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let es = entries();
    let story_words = es
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && !text(e).contains("PINEAPPLE"))
        .map(|e| text(e).split_whitespace().count())
        .sum::<usize>();
    let users = es.iter().filter(|e| e.role == MessageRole::User).count();
    println!(
        "RESULT {name}: words_at_steer={words_at_steer} story_words_total={story_words} \
         steer_answered_after={:?} users={users} status={:?}\n{}",
        answered_at.unwrap(),
        status(),
        dump(&es)
    );
    core.shutdown().await;
}
