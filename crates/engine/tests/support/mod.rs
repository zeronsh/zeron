//! Shared scaffolding for the goal-mode and child-ask integration tests: a
//! scriptable harness (each run is handed a sender it feeds events to, so a
//! test can gate it, interrupt it, or make it call back into the engine) and
//! a few polling helpers.
#![allow(dead_code)] // each test binary uses a different subset

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

pub type Out = UnboundedSender<AgentEvent>;
/// `(call index, request, event sender, controls)`. Must not block: spawn
/// a task for anything that waits.
pub type Handler = Arc<dyn Fn(usize, RunRequest, Out, RunControls) + Send + Sync>;

pub struct Scripted {
    pub runs: Arc<Mutex<Vec<RunRequest>>>,
    pub handler: Handler,
}

#[async_trait]
impl Harness for Scripted {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let index = {
            let mut runs = self.runs.lock().unwrap();
            runs.push(request.clone());
            runs.len() - 1
        };
        let (tx, rx) = unbounded_channel::<AgentEvent>();
        (self.handler)(index, request, tx, controls);
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        })
        .boxed())
    }
}

/// One finished turn: text, optional token usage, `Done{Completed}`.
pub fn text_turn(out: &Out, text: &str, tokens: Option<(u64, u64)>) {
    let _ = out.send(AgentEvent::TextDelta { text: text.into() });
    if let Some((input_tokens, output_tokens)) = tokens {
        let _ = out.send(AgentEvent::Usage {
            input_tokens,
            output_tokens,
        });
    }
    let _ = out.send(AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: None,
    });
}

pub fn tool_call(out: &Out, id: &str, call: zeron_proto::ToolCall) {
    let _ = out.send(AgentEvent::ToolCall {
        id: id.into(),
        call,
    });
    let _ = out.send(AgentEvent::ToolResult {
        id: id.into(),
        is_error: false,
        output: None,
        diff: None,
    });
}

pub fn interrupted(out: &Out) {
    let _ = out.send(AgentEvent::Done {
        status: DoneStatus::Interrupted,
        result: None,
        error: None,
        session_id: None,
    });
}

pub struct Env {
    pub core: EngineCore,
    pub runs: Arc<Mutex<Vec<RunRequest>>>,
}

pub fn assemble(dir: &std::path::Path, runs: Arc<Mutex<Vec<RunRequest>>>, handler: Handler) -> Env {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Scripted {
        runs: runs.clone(),
        handler,
    }));
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    Env { core, runs }
}

impl Env {
    pub fn prompts(&self) -> Vec<String> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.prompt.clone())
            .collect()
    }

    /// A titled chat in a space on this device (the titler stays out of the
    /// recorded runs).
    pub fn chat(&self, id: &str, sandbox: SandboxLevel) {
        let space = "space-main".to_string();
        self.core
            .workspace
            .create_space(&space, &self.core.device_id, "/tmp", None, false)
            .unwrap();
        self.core
            .workspace
            .create_chat(
                id,
                Some(&space),
                None,
                Some(ChatConfig {
                    preset: None,
                    policy: Default::default(),
                    harness: HarnessId::Mock,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox,
                }),
                None,
            )
            .unwrap();
        self.core.workspace.rename_chat(id, "Titled").unwrap();
    }

    pub fn entries(&self, chat: &str) -> Vec<SessionMessageEntry> {
        self.core
            .doc_host
            .open(chat)
            .ok()
            .and_then(|h| h.doc().read_entries().ok())
            .unwrap_or_default()
    }

    pub fn goal(&self, chat: &str) -> Option<zeron_proto::Goal> {
        self.core.doc_host.goal(chat)
    }

    pub fn goal_command(&self, chat: &str, command: zeron_proto::GoalCommand) {
        self.core
            .doc_host
            .queue_command(chat, zeron_doc::SessionCommandPayload::Goal { command })
            .expect("queue goal command");
    }

    pub fn set_goal(&self, chat: &str, objective: &str) {
        self.goal_command(
            chat,
            zeron_proto::GoalCommand::Set {
                objective: objective.into(),
                limits: Default::default(),
                replace: false,
            },
        );
    }

    pub fn user_text(&self, chat: &str) -> Vec<(String, Option<zeron_proto::MessageOrigin>)> {
        self.entries(chat)
            .into_iter()
            .filter(|e| e.role == MessageRole::User)
            .map(|e| {
                let text = e
                    .parts
                    .iter()
                    .find_map(|p| match p {
                        MessagePart::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                (text, e.origin)
            })
            .collect()
    }

    pub fn complete_turns(&self, chat: &str) -> usize {
        self.entries(chat)
            .iter()
            .filter(|e| {
                e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete)
            })
            .count()
    }
}

pub async fn wait_for<F: FnMut() -> bool>(predicate: F, what: &str) {
    wait_for_within(predicate, what, Duration::from_secs(15)).await;
}

pub async fn wait_for_within<F: FnMut() -> bool>(mut predicate: F, what: &str, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

/// Assert `predicate` stays false for `for_` (a negative check that has to
/// wait to mean anything).
pub async fn stays_false<F: FnMut() -> bool>(mut predicate: F, for_: Duration, what: &str) {
    let until = tokio::time::Instant::now() + for_;
    while tokio::time::Instant::now() < until {
        assert!(!predicate(), "{what}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}
