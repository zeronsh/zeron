//! A chat can change provider mid-conversation: the new harness never sees the
//! old harness's resume id, is handed the prior transcript once, and the
//! switch is marked in the transcript.

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::methods;

type Seen = Arc<Mutex<Vec<(HarnessId, RunRequest)>>>;

/// A fake provider: records every request it receives and answers with a
/// session id only it could have minted (`<name>-session`).
struct Fake {
    id: HarnessId,
    name: &'static str,
    seen: Seen,
    /// When set, the turn blocks until notified — a turn that is "running".
    gate: Option<Arc<Notify>>,
}

#[async_trait]
impl Harness for Fake {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        self.name
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.seen.lock().unwrap().push((self.id, request));
        let session = format!("{}-session", self.name.to_lowercase());
        let gate = self.gate.clone();
        let name = self.name;
        let id = self.id;
        Ok(futures::stream::once(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            vec![
                Ok(AgentEvent::SessionStarted {
                    harness: id,
                    model: "m".into(),
                    tools: vec![],
                    cwd: "/tmp".into(),
                    session_id: session.clone(),
                    assistant_message_id: format!("{name}-a"),
                }),
                Ok(AgentEvent::TextDelta {
                    text: format!("{name} answer"),
                }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some(session),
                }),
            ]
        })
        .flat_map(futures::stream::iter)
        .boxed())
    }
}

const CLAUDE: HarnessId = HarnessId::ClaudeCode;
const CODEX: HarnessId = HarnessId::Codex;

struct Rig {
    core: EngineCore,
    seen: Seen,
    gate: Arc<Notify>,
    _dir: tempfile::TempDir,
}

fn rig(gated: Option<HarnessId>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let seen: Seen = Arc::default();
    let gate = Arc::new(Notify::new());
    let registry = HarnessRegistry::new();
    for (id, name) in [(CLAUDE, "Claude"), (CODEX, "Codex")] {
        registry.register(Arc::new(Fake {
            id,
            name,
            seen: seen.clone(),
            gate: (gated == Some(id)).then(|| gate.clone()),
        }));
    }
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), CLAUDE, None).unwrap();
    core.workspace
        .create_chat(
            "main",
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    Rig {
        core,
        seen,
        gate,
        _dir: dir,
    }
}

fn run_request(harness: HarnessId, prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(harness),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: vec![],
        worktree: None,
    }
}

impl Rig {
    async fn wait_idle(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.seen.lock().unwrap().len() < count
                || self
                    .core
                    .sessions
                    .session_status("main")
                    .is_some_and(|s| s.status != SessionStatus::Idle)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("turn settles");
    }

    /// One full turn on `harness`; returns the request the provider received.
    async fn turn(&self, harness: HarnessId, prompt: &str, message_id: &str) -> RunRequest {
        let before = self.seen.lock().unwrap().len();
        self.core
            .sessions
            .dispatch(
                "main",
                harness,
                run_request(harness, prompt),
                Some(message_id.into()),
            )
            .await
            .unwrap();
        self.wait_idle(before + 1).await;
        // The row write rides the Done event; let it land.
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.seen.lock().unwrap()[before].1.clone()
    }

    fn row(&self) -> zeron_proto::Chat {
        self.core.workspace.chat("main").unwrap().unwrap()
    }

    fn entries(&self) -> Vec<SessionMessageEntry> {
        self.core
            .doc_host
            .open("main")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
    }

    fn seams(&self) -> Vec<(String, String)> {
        self.entries()
            .iter()
            .flat_map(|entry| entry.parts.iter())
            .filter_map(|part| match part {
                MessagePart::Switch { from, to, .. } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn switching_provider_drops_the_foreign_resume_id_and_replays_the_history_once() {
    let rig = rig(None);

    let first = rig.turn(CLAUDE, "Remember PINEAPPLE", "u1").await;
    assert_eq!(first.resume, None);
    assert_eq!(
        first.prompt, "Remember PINEAPPLE",
        "a plain chat is never wrapped"
    );
    assert_eq!(
        rig.row().harness_session_id.as_deref(),
        Some("claude-session")
    );
    assert_eq!(
        rig.row().harness_session_harness.as_deref(),
        Some("claude-code")
    );

    // Same provider again: resumes its own session, still unwrapped.
    let again = rig.turn(CLAUDE, "and MANGO", "u2").await;
    assert_eq!(again.resume.as_deref(), Some("claude-session"));
    assert_eq!(again.prompt, "and MANGO");
    assert!(rig.seams().is_empty());

    // Switch to Codex: Claude's session id must not reach it.
    let switched = rig.turn(CODEX, "What fruit did I mention?", "u3").await;
    assert_eq!(
        switched.resume, None,
        "a foreign session id is never resumed"
    );
    for needle in [
        "PINEAPPLE",
        "MANGO",
        "Claude answer",
        "taking over",
        "What fruit did I mention?",
    ] {
        assert!(
            switched.prompt.contains(needle),
            "history prompt is missing {needle:?}: {}",
            switched.prompt
        );
    }
    assert_eq!(
        rig.seams(),
        vec![("Claude".to_string(), "Codex".to_string())]
    );
    // The seam names both providers by id too, for their logos.
    let ids: Vec<_> = rig
        .entries()
        .iter()
        .flat_map(|entry| entry.parts.iter())
        .filter_map(|part| match part {
            MessagePart::Switch {
                from_harness,
                to_harness,
                ..
            } => Some((from_harness.clone(), to_harness.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec![(Some("claude-code".to_string()), Some("codex".to_string()))]
    );
    assert_eq!(
        rig.row().harness_session_id.as_deref(),
        Some("codex-session")
    );
    assert_eq!(rig.row().harness_session_harness.as_deref(), Some("codex"));

    // The seam sits above the message that triggered the switch.
    let entries = rig.entries();
    let seam = entries
        .iter()
        .position(|e| {
            e.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Switch { .. }))
        })
        .unwrap();
    assert_eq!(entries[seam].role, MessageRole::System);
    assert_eq!(entries[seam + 1].id, "u3");

    // Codex now holds the history: its next turn resumes and is unwrapped.
    let follow = rig.turn(CODEX, "thanks", "u4").await;
    assert_eq!(follow.resume.as_deref(), Some("codex-session"));
    assert_eq!(
        follow.prompt, "thanks",
        "history is replayed once, not every turn"
    );
    assert_eq!(rig.seams().len(), 1);

    // And back: Claude's old session is stale (Codex has moved the
    // conversation on), so it gets a fresh session carrying everything.
    let back = rig.turn(CLAUDE, "one more thing", "u5").await;
    assert_eq!(back.resume, None);
    assert!(back.prompt.contains("Codex answer"), "{}", back.prompt);
    assert!(back.prompt.contains("What fruit did I mention?"));
    assert_eq!(rig.seams().len(), 2);
    assert_eq!(rig.seams()[1], ("Codex".to_string(), "Claude".to_string()));
}

#[tokio::test]
async fn a_switch_survives_an_engine_restart_and_a_retried_dispatch_writes_one_seam() {
    let dir = tempfile::tempdir().unwrap();
    let seen: Seen = Arc::default();
    let make = |dir: &std::path::Path| {
        let registry = HarnessRegistry::new();
        for (id, name) in [(CLAUDE, "Claude"), (CODEX, "Codex")] {
            registry.register(Arc::new(Fake {
                id,
                name,
                seen: seen.clone(),
                gate: None,
            }));
        }
        EngineCore::assemble(dir, Arc::new(registry), CLAUDE, None).unwrap()
    };

    {
        let core = make(dir.path());
        core.workspace
            .create_chat(
                "main",
                None,
                Some(&core.device_id),
                None,
                Some("/tmp".into()),
            )
            .unwrap();
        core.sessions
            .dispatch(
                "main",
                CLAUDE,
                run_request(CLAUDE, "hello"),
                Some("u1".into()),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while core
                .sessions
                .session_status("main")
                .is_none_or(|s| s.status != SessionStatus::Idle)
                || core
                    .workspace
                    .chat("main")
                    .unwrap()
                    .unwrap()
                    .harness_session_harness
                    .is_none()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        core.shutdown().await;
    }

    // Fresh process, cold caches: only the persisted row knows the owner.
    let core = make(dir.path());
    let before = seen.lock().unwrap().len();
    for id in ["u2", "u2"] {
        // The same message id twice models the startup-crash retry.
        core.sessions
            .dispatch(
                "main",
                CODEX,
                run_request(CODEX, "continue please"),
                Some(id.into()),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while core
                .sessions
                .session_status("main")
                .is_some_and(|s| s.status != SessionStatus::Idle)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    let received = seen.lock().unwrap()[before].1.clone();
    assert_eq!(received.resume, None);
    assert!(received.prompt.contains("hello"), "{}", received.prompt);
    let seams = core
        .doc_host
        .open("main")
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .iter()
        .flat_map(|e| e.parts.clone())
        .filter(|p| matches!(p, MessagePart::Switch { .. }))
        .count();
    assert_eq!(seams, 1, "a retried dispatch does not stack seams");
}

#[tokio::test]
async fn rows_from_before_the_owner_tag_resume_and_are_tagged_from_the_journal() {
    let rig = rig(None);
    rig.turn(CLAUDE, "hello", "u1").await;
    // Simulate a row written by an older build: id present, tag absent.
    rig.core
        .workspace
        .set_chat_harness_session("main", "claude-session", "/tmp", None);
    assert_eq!(rig.row().harness_session_harness, None);

    // Still Claude: the journal confirms the owner, so it resumes.
    let same = rig.turn(CLAUDE, "still here", "u2").await;
    assert_eq!(same.resume.as_deref(), Some("claude-session"));
    assert!(rig.seams().is_empty());
}

#[tokio::test]
async fn replayed_history_is_capped_to_the_newest_messages() {
    let rig = rig(None);
    let doc = rig.core.doc_host.open("main").unwrap();
    let big = "x".repeat(7_000);
    for i in 0..40 {
        let (id, role) = if i % 2 == 0 {
            (format!("u{i}"), MessageRole::User)
        } else {
            (format!("a{i}"), MessageRole::Assistant)
        };
        doc.doc()
            .push_message(&SessionMessageEntry {
                duration_ms: None,
                id: id.clone(),
                role,
                parts: vec![MessagePart::Text {
                    id: format!("{id}-t"),
                    text: format!("msg-{i} {big}"),
                }],
                created_at: i as i64 + 1,
                device_id: "device".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
            })
            .unwrap();
    }
    // Establish Claude as the owner without adding to the transcript.
    rig.core
        .workspace
        .set_chat_harness_session("main", "claude-session", "/tmp", Some(CLAUDE));

    let received = rig.turn(CODEX, "summarize", "final").await;
    assert!(
        received.prompt.len() < 100_000,
        "prompt is {} bytes",
        received.prompt.len()
    );
    assert!(
        received.prompt.contains("msg-39"),
        "newest message survives"
    );
    assert!(
        !received.prompt.contains("msg-0 "),
        "oldest message is dropped"
    );
    assert!(received.prompt.contains("earlier messages omitted"));
}

#[tokio::test]
async fn provider_cannot_change_while_a_turn_is_running() {
    let rig = rig(Some(CLAUDE));
    rig.core
        .workspace
        .set_chat_config(
            "main",
            &ChatConfig {
                harness: CLAUDE,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            },
        )
        .unwrap();
    rig.core
        .sessions
        .dispatch(
            "main",
            CLAUDE,
            run_request(CLAUDE, "long task"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.core.sessions.turn_in_flight("main") {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    let client = zeron_rpc::memory_client(rig.core.rpc_service());
    let to = |harness: HarnessId| {
        serde_json::json!({
            "op": "setChatConfig",
            "chatId": "main",
            "config": ChatConfig {
                harness,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            },
        })
    };
    let refused = client.call(methods::MUTATE, to(CODEX)).await;
    let error = refused
        .expect_err("switching mid-turn is refused")
        .to_string();
    assert!(error.contains("finish"), "{error}");
    assert_eq!(rig.row().config.unwrap().harness, CLAUDE);

    // Same-provider config edits (model, reasoning) stay allowed mid-turn.
    client.call(methods::MUTATE, to(CLAUDE)).await.unwrap();

    // Once the turn ends the switch goes through.
    rig.gate.notify_one();
    rig.wait_idle(1).await;
    client.call(methods::MUTATE, to(CODEX)).await.unwrap();
    assert_eq!(rig.row().config.unwrap().harness, CODEX);
}
