//! Opt-in real-model checks of transcript order for every way a message can
//! arrive while a turn is running: a plain send (the composer's Send before
//! its busy state lands, or any remote client), the composer queue, and
//! explicit steering (queue-row Steer / Send next, MCP `send_message` steer).
//!
//! ZERON_TEST_HARNESS=pi cargo test -p zeron-engine --test queue_order_live -- --ignored --nocapture
//! ZERON_TEST_MODEL optionally pins the model; ZERON_TEST_SCENARIO runs one of
//! `sends`, `queue`, `steer`.
use std::{sync::Arc, time::Duration};
use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{
    AcpHarness, ClaudeHarness, CodexHarness, CursorHarness, Harness, OpencodeHarness,
};
use zeron_proto::{ChatConfig, HarnessId, RunRequest, SandboxLevel, SessionStatus};

const CHAT: &str = "order";

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
            let errors: Vec<_> = e
                .parts
                .iter()
                .filter_map(|p| match p {
                    MessagePart::Error { message, .. } => Some(message.as_str()),
                    _ => None,
                })
                .collect();
            format!(
                "  {:?} {:?}{}",
                e.role,
                text(e).chars().take(120).collect::<String>(),
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

struct Rig {
    core: EngineCore,
    dir: tempfile::TempDir,
    id: HarnessId,
    model: Option<String>,
    mid_turn: bool,
}

impl Rig {
    async fn new(name: &str) -> Self {
        let harness = harness(name);
        let id = harness.id();
        let dir = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.register(harness);
        let core =
            EngineCore::assemble(&dir.path().join("engine"), Arc::new(registry), id, None).unwrap();
        let model = std::env::var("ZERON_TEST_MODEL").ok();
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
        let mid_turn = core.sessions.steers_mid_turn(id);
        Self {
            core,
            dir,
            id,
            model,
            mid_turn,
        }
    }

    fn request(&self, prompt: String) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt,
            harness: Some(self.id),
            model: self.model.clone(),
            reasoning: None,
            model_options: Default::default(),
            cwd: self.dir.path().to_str().unwrap().into(),
            sandbox: SandboxLevel::DangerFullAccess,
            auto_approve: true,
            attachments: vec![],
            worktree: None,
            resume: None,
        }
    }

    fn run(&self, message_id: &str, prompt: String) {
        self.core
            .doc_host
            .queue_command(
                CHAT,
                SessionCommandPayload::Run {
                    message_id: message_id.into(),
                    request: self.request(prompt),
                },
            )
            .unwrap();
    }

    fn entries(&self) -> Vec<SessionMessageEntry> {
        self.core
            .doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
    }

    fn status(&self) -> Option<SessionStatus> {
        self.core.sessions.session_status(CHAT).map(|s| s.status)
    }

    async fn wait(&self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let ok = tokio::time::timeout(Duration::from_secs(300), async {
            while !done(self) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok();
        if !ok {
            let entries = self.entries();
            self.core.shutdown().await;
            panic!(
                "timeout waiting for {what}; status {:?}\n{}",
                self.status(),
                dump(&entries)
            );
        }
    }

    /// Opening turn long enough that later sends land while it runs.
    async fn start_long_turn(&self) {
        self.run(
            "u0",
            "This is an automated ordering test. Use your shell tool to run exactly `sleep 12` \
             in this directory and wait for it to finish, then reply with the token ACK-0. \
             Later messages may ask for more tokens; always include every token requested."
                .into(),
        );
        self.wait("opening turn to start", |r| {
            r.status() == Some(SessionStatus::Working)
        })
        .await;
    }

    fn follow_up(i: usize) -> String {
        format!("Also include the token ACK-{i} in your reply. No tools are needed for this.")
    }

    async fn settle(&self, users: usize, acks: usize) {
        self.wait("every reply and an idle chat", |r| {
            let entries = r.entries();
            let all = entries
                .iter()
                .filter(|e| e.role == MessageRole::Assistant)
                .map(text)
                .collect::<String>();
            entries
                .iter()
                .filter(|e| e.role == MessageRole::User)
                .count()
                == users
                && (0..acks).all(|i| all.contains(&format!("ACK-{i}")))
                && r.status() == Some(SessionStatus::Idle)
                && r.core
                    .doc_host
                    .open(CHAT)
                    .unwrap()
                    .doc()
                    .read_queue()
                    .unwrap()
                    .is_empty()
        })
        .await;
        // Anything still in flight (a duplicate delivery, a late turn) shows
        // up within a few seconds of the apparent settle.
        tokio::time::sleep(Duration::from_secs(8)).await;
    }

    /// Every user message appears exactly once. Its ACK appears after it —
    /// and for turn-boundary agents, where each message is its own turn,
    /// before the next user message too (reply sits under its message).
    fn assert_order(&self, users: usize, strict: bool) {
        let entries = self.entries();
        let report = dump(&entries);
        let user_positions: Vec<(usize, String)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.role == MessageRole::User)
            .map(|(ix, e)| (ix, text(e)))
            .collect();
        assert_eq!(user_positions.len(), users, "user entry count\n{report}");
        assert!(
            self.status() == Some(SessionStatus::Idle),
            "final status {:?}\n{report}",
            self.status()
        );
        for i in 0..users {
            let token = format!("ACK-{i}");
            let (at, ref prompt) = user_positions[i];
            assert!(
                prompt.contains(&token),
                "user message {i} out of order: {prompt:?}\n{report}"
            );
            let next_user = user_positions
                .get(i + 1)
                .map_or(entries.len(), |(ix, _)| *ix);
            let window_end = if strict { next_user } else { entries.len() };
            let answered = entries[at + 1..window_end]
                .iter()
                .any(|e| e.role == MessageRole::Assistant && text(e).contains(&token));
            assert!(
                answered,
                "{token} is not answered {} its message\n{report}",
                if strict { "directly under" } else { "after" }
            );
        }
        let assistant_errors: Vec<_> = entries
            .iter()
            .flat_map(|e| e.parts.iter())
            .filter(|p| matches!(p, MessagePart::Error { .. }))
            .collect();
        assert!(
            assistant_errors.is_empty(),
            "errors in transcript\n{report}"
        );
        println!("transcript:\n{report}");
    }
}

async fn scenario(name: &str, which: &str) {
    let rig = Rig::new(name).await;
    println!("{name}/{which}: mid-turn steering = {}", rig.mid_turn);
    rig.start_long_turn().await;
    match which {
        // Plain sends while the turn runs, back to back.
        "sends" => {
            for i in 1..=3 {
                rig.run(&format!("u{i}"), Rig::follow_up(i));
            }
            rig.settle(4, 4).await;
            rig.assert_order(4, !rig.mid_turn);
        }
        // The composer's Queue mode: held rows, delivered one per turn.
        "queue" => {
            for i in 1..=2 {
                rig.core
                    .doc_host
                    .queue_message(CHAT, &Rig::follow_up(i), vec![])
                    .unwrap();
            }
            rig.settle(3, 3).await;
            rig.assert_order(3, true);
        }
        // Queue-row Steer / Send next, then an MCP steer command.
        "steer" => {
            let row = rig
                .core
                .doc_host
                .queue_message(CHAT, &Rig::follow_up(1), vec![])
                .unwrap();
            assert!(
                rig.core
                    .doc_host
                    .steer_queued_now(CHAT, &row)
                    .await
                    .unwrap()
            );
            rig.core
                .doc_host
                .queue_command(
                    CHAT,
                    SessionCommandPayload::Steer {
                        prompt: Rig::follow_up(2),
                        message_id: Some("steer-2".into()),
                    },
                )
                .unwrap();
            rig.settle(3, 3).await;
            rig.assert_order(3, !rig.mid_turn);
        }
        other => panic!("unknown scenario {other}"),
    }
    rig.core.shutdown().await;
    println!("PASS {name}/{which}");
}

#[tokio::test]
#[ignore = "uses real model quota; select the harness explicitly"]
async fn messages_sent_during_a_turn_keep_transcript_order() {
    let name = std::env::var("ZERON_TEST_HARNESS").expect("select harness");
    let only = std::env::var("ZERON_TEST_SCENARIO").ok();
    for which in ["sends", "queue", "steer"] {
        if only.as_deref().is_none_or(|o| o == which) {
            scenario(&name, which).await;
        }
    }
}
