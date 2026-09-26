//! Opt-in production engine + Cursor SDK checks. Uses real account quota.
//! ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo test -p zeron-engine --test cursor_live -- --ignored --nocapture --test-threads=1
use std::{sync::Arc, time::Duration};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::CursorHarness;
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

const CHAT: &str = "cursor-live-audit";
fn setup(path: &std::path::Path) -> EngineCore {
    assert!(
        std::env::var_os("ZERON_CURSOR_STATE_DIR").is_some(),
        "use isolated Cursor state"
    );
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(CursorHarness::new()));
    let core = EngineCore::assemble(
        &path.join("engine"),
        Arc::new(registry),
        HarnessId::Cursor,
        None,
    )
    .unwrap();
    core.workspace
        .create_space(
            "audit",
            &core.device_id,
            path.to_str().unwrap(),
            None,
            false,
        )
        .unwrap();
    core.workspace
        .create_chat(CHAT, Some("audit"), None, None, None)
        .unwrap();
    core.workspace
        .rename_chat(CHAT, "Cursor live audit")
        .unwrap();
    core
}
fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
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
async fn wait(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    while !predicate() {
        assert!(tokio::time::Instant::now() < deadline, "timeout: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
fn start(core: &EngineCore, cwd: &std::path::Path, prompt: String) {
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                message_id: uuid::Uuid::new_v4().to_string(),
                request: RunRequest {
                    mcp: None,
                    prompt,
                    harness: Some(HarnessId::Cursor),
                    model: Some("muse-spark-1.3".into()),
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: cwd.to_str().unwrap().into(),
                    sandbox: SandboxLevel::DangerFullAccess,
                    auto_approve: true,
                    attachments: vec![],
                    worktree: None,
                    resume: None,
                },
            },
        )
        .unwrap();
}
fn completed(core: &EngineCore) -> usize {
    entries(core)
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn remote_steer_batch_reaches_real_muse_with_every_message() {
    let dir = tempfile::tempdir().unwrap();
    let core = setup(dir.path());
    let tokens: Vec<_> = (0..9)
        .map(|i| format!("ENGINE-{i}-{}", uuid::Uuid::new_v4()))
        .collect();
    start(
        &core,
        dir.path(),
        format!(
            "Remember {}. Run shell command `sleep 5`, then reply only that token. Keep every later user token in conversation history; do not write them to files.",
            tokens[0]
        ),
    );
    wait(
        || {
            entries(&core).iter().any(|e| {
                e.parts
                    .iter()
                    .any(|p| matches!(p, MessagePart::Tool { .. }))
            })
        },
        "first tool",
    )
    .await;
    let handle = core.doc_host.open(CHAT).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for (i, token) in tokens.iter().enumerate().skip(1) {
        handle.doc().queue_command(&zeron_doc::SessionCommandEntry {
            id: format!("remote-{i}"),
            payload: SessionCommandPayload::Steer {prompt: format!("Remember {token}. Reply with the immediately previous user token and this token. Do not use tools."), message_id: Some(format!("message-{i}"))},
            issued_by: "remote-viewer".into(), issued_at: now + i as i64,
            based_on: None, expires_at: None, status: zeron_doc::SessionCommandStatus::Pending, resolution: None,
        }).unwrap();
    }
    core.doc_host.drain_commands(&handle).await;
    wait(
        || completed(&core) == tokens.len(),
        "all remote prompts answered",
    )
    .await;
    let history = entries(&core);
    let users: Vec<_> = history
        .iter()
        .filter(|e| e.role == MessageRole::User)
        .collect();
    let answers: Vec<_> = history
        .iter()
        .filter(|e| e.role == MessageRole::Assistant)
        .collect();
    assert_eq!(users.len(), tokens.len());
    assert_eq!(answers.len(), tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        if !text(answers[i]).contains(token) {
            let events = core.sessions.subscribe(CHAT, 0).unwrap().0;
            eprintln!("missing_answer_index={i} history={history:?} events={events:?}");
        }
        assert!(text(users[i]).contains(token));
        assert!(
            text(answers[i]).contains(token),
            "missing current token in {}",
            text(answers[i])
        );
        if i > 0 {
            assert!(
                text(answers[i]).contains(&tokens[i - 1]),
                "missing previous token in {}",
                text(answers[i])
            );
        }
    }
    assert!(handle.doc().read_queue().unwrap().is_empty());
    println!(
        "real_engine_remote_batch messages={} all_previous_and_current_recalled=true",
        tokens.len()
    );
    core.shutdown().await;
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn send_now_keeps_the_interrupted_user_message() {
    send_now_recall(2).await;
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn send_now_during_startup_keeps_the_interrupted_user_message() {
    let rounds = std::env::var("ZERON_CURSOR_EARLY_ROUNDS")
        .ok()
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(1);
    for round in 0..rounds {
        send_now_recall(1).await;
        println!("startup_interrupt_round={} passed=true", round + 1);
    }
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn send_now_before_sdk_readiness_keeps_the_user_message() {
    send_now_recall(0).await;
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn resumed_startup_interruption_keeps_the_newest_user_message() {
    send_now_recall(3).await;
}

async fn send_now_recall(stage: u8) {
    let dir = tempfile::tempdir().unwrap();
    let core = setup(dir.path());
    if stage == 3 {
        start(
            &core,
            dir.path(),
            "Reply only SEEDED. Do not use tools.".into(),
        );
        wait(|| completed(&core) == 1, "seeded conversation checkpoint").await;
        core.sessions.interrupt(CHAT).await.unwrap();
    }
    let completed_before = completed(&core);
    let users_before = entries(&core)
        .iter()
        .filter(|e| e.role == MessageRole::User)
        .count();
    let token = format!("INTERRUPTED-{}", uuid::Uuid::new_v4());
    start(
        &core,
        dir.path(),
        format!(
            "Remember {token}. Run shell command `sleep 30`, then reply with the token. Do not write it to files."
        ),
    );
    if stage == 0 || stage == 3 {
        wait(
            || core.sessions.turn_in_flight(CHAT),
            "engine dispatch before SDK readiness",
        )
        .await;
    } else if stage == 1 {
        wait(
            || {
                core.sessions
                    .subscribe(CHAT, 0)
                    .ok()
                    .is_some_and(|(events, _)| {
                        events.iter().any(|e| {
                            matches!(e.event, zeron_proto::AgentEvent::SessionStarted { .. })
                        })
                    })
            },
            "shim startup",
        )
        .await;
    } else {
        wait(
            || {
                entries(&core).iter().any(|e| {
                    e.parts
                        .iter()
                        .any(|p| matches!(p, MessagePart::Tool { .. }))
                })
            },
            "interruptible tool",
        )
        .await;
    }
    let id = core.doc_host.queue_message(CHAT, "What exact INTERRUPTED token was in my previous message? Reply only that token. Do not use tools.", vec![]).unwrap();
    assert!(core.doc_host.send_queued_now(CHAT, &id).await.unwrap());
    wait(
        || completed(&core) == completed_before + 1,
        "send-now answer",
    )
    .await;
    let history = entries(&core);
    assert_eq!(
        history
            .iter()
            .filter(|e| e.role == MessageRole::User)
            .count(),
        users_before + 2
    );
    let answer = history
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .unwrap();
    assert!(
        text(answer).contains(&token),
        "interrupted message missing: {}",
        text(answer)
    );
    println!("real_engine_send_now interrupted_user_message_recalled=true");
    core.shutdown().await;
}

#[tokio::test]
#[ignore = "real authenticated Muse Spark; consumes quota"]
async fn startup_steering_burst_retains_all_interrupted_messages() {
    let dir = tempfile::tempdir().unwrap();
    let core = setup(dir.path());
    let tokens: Vec<_> = (0..10)
        .map(|i| format!("RACE-{i}-{}", uuid::Uuid::new_v4()))
        .collect();
    let prompt = |token: &str| {
        format!(
            "Remember {token}. Run shell command `printf once >> forbidden.txt; sleep 30`, then reply with the token. Do not write the token to files."
        )
    };
    start(&core, dir.path(), prompt(&tokens[0]));
    for i in 0..tokens.len() {
        wait(
            || {
                core.sessions
                    .subscribe(CHAT, 0)
                    .unwrap()
                    .0
                    .iter()
                    .filter(|e| matches!(e.event, zeron_proto::AgentEvent::SessionStarted { .. }))
                    .count()
                    == i + 1
            },
            "next startup boundary",
        )
        .await;
        let next = if i + 1 == tokens.len() {
            "List every RACE token from previous user messages, in order. Only list tokens. Do not use tools or execute interrupted requests.".into()
        } else {
            prompt(&tokens[i + 1])
        };
        let id = core.doc_host.queue_message(CHAT, &next, vec![]).unwrap();
        assert!(core.doc_host.send_queued_now(CHAT, &id).await.unwrap());
    }
    wait(
        || !core.sessions.turn_in_flight(CHAT) && completed(&core) > 0,
        "burst recall",
    )
    .await;
    let history = entries(&core);
    let answer = text(
        history
            .iter()
            .rev()
            .find(|e| e.role == MessageRole::Assistant)
            .unwrap(),
    );
    let mut offset = 0;
    for token in &tokens {
        offset += answer[offset..]
            .find(token)
            .unwrap_or_else(|| panic!("lost {token}: {answer}"))
            + token.len();
    }
    assert_eq!(
        history
            .iter()
            .filter(|e| e.role == MessageRole::User)
            .count(),
        tokens.len() + 1
    );
    assert!(
        !dir.path().join("forbidden.txt").exists(),
        "interrupted tools were executed/replayed"
    );
    println!(
        "real_engine_startup_burst interrupted_messages={} all_recalled_in_order=true tool_replays=0",
        tokens.len()
    );
    core.shutdown().await;
}
