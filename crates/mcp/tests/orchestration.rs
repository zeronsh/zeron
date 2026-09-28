//! Spawn → wait → output against a real `EngineCore` (MockHarness scripted
//! replies, in-memory RPC transport). The MCP tool calls are the same
//! implementation the `zeron chat` commands dispatch into, so this proves the
//! whole path: chat row → queued run → session settle → transcript read-back.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::json;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::mock::MockHarness;
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_mcp::{Origin, Tools, Zeron};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

fn script(reply: &str) -> Vec<AgentEvent> {
    vec![
        AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: "/tmp".into(),
            session_id: "hs-1".into(),
            assistant_message_id: "a-1".into(),
        },
        AgentEvent::TextDelta { text: reply.into() },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("hs-1".into()),
        },
    ]
}

/// The mock harness is never catalog-enabled (production pickers hide it),
/// so tests replay its script under the `claude-code` id.
struct FakeClaude {
    inner: MockHarness,
}

#[async_trait]
impl Harness for FakeClaude {
    fn id(&self) -> HarnessId {
        HarnessId::ClaudeCode
    }
    fn display_name(&self) -> &str {
        "Claude Code"
    }
    fn supports_steering(&self) -> bool {
        self.inner.supports_steering()
    }
    fn steering_mode(&self) -> SteeringMode {
        self.inner.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        self.inner.reasoning_levels()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.inner.models().await
    }
    async fn run_title(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.inner.run_title(request, controls).await
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.inner.run(request, controls).await
    }
}

fn engine(dir: &std::path::Path) -> (EngineCore, Arc<Tools>) {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(FakeClaude {
        inner: MockHarness {
            script: script("pong"),
        },
    }));
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::ClaudeCode, None)
        .expect("engine core assembles");
    let client = zeron_rpc::memory_client(core.rpc_service());
    let tools = Tools::new(Arc::new(Zeron::with_client(client, Origin::default())));
    (core, Arc::new(tools))
}

#[tokio::test]
async fn spawn_wait_and_output_against_a_live_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (core, root) = engine(dir.path());

    // A top-level parent the agent speaks from.
    let parent = root
        .call(
            "create_chat",
            json!({ "prompt": "Reply with OK", "wait": true, "timeout_secs": 30 }),
        )
        .await
        .expect("parent spawn");
    let parent_id = parent["chatId"].as_str().unwrap().to_owned();
    assert_eq!(
        parent["turn"]["replies"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["text"],
        "pong"
    );

    // The parent's agent spawns a child; it is attributed and waits.
    let agent = Tools::new(Arc::new(Zeron::with_client(
        zeron_rpc::memory_client(core.rpc_service()),
        Origin {
            chat_id: Some(parent_id.clone()),
            device_id: None,
        },
    )));
    let child = agent
        .call(
            "create_chat",
            json!({ "prompt": "Reply with the word pong", "wait": true,
                    "timeout_secs": 30, "title": "Kid" }),
        )
        .await
        .expect("child spawn");
    let child_id = child["chatId"].as_str().unwrap().to_owned();
    assert_eq!(child["parentChatId"], parent_id);
    assert_eq!(child["spawnedByAgent"], true);
    assert_eq!(
        child["turn"]["replies"].as_array().unwrap().last().unwrap()["text"],
        "pong"
    );

    // Already-settled: wait returns immediately with the same reply.
    let settled = agent
        .call(
            "wait_for_turn",
            json!({ "chat": child_id, "timeout_secs": 10 }),
        )
        .await
        .expect("settled wait");
    assert_eq!(
        settled["turn"]["replies"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["text"],
        "pong"
    );

    // Output reads the last assistant reply of the settled turn.
    let out = agent
        .call("chat_output", json!({ "chat": child_id }))
        .await
        .expect("chat_output");
    assert_eq!(out["reply"], "pong");

    // An unattributed follow-up still lands (the parent follows up).
    let sent = agent
        .call(
            "send_message",
            json!({ "chat": child_id, "text": "again", "wait": true, "timeout_secs": 30 }),
        )
        .await
        .expect("send_message");
    assert_eq!(
        sent["turn"]["replies"].as_array().unwrap().last().unwrap()["text"],
        "pong"
    );
}
