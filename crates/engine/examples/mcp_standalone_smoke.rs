//! Isolated MCP stdio instance for manual discovery/create/converse smoke tests.
//! Run with `cargo run -p zeron-engine --example mcp_standalone_smoke`.
//! Uses a temporary profile and scripted harness, never the user's workspace.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls, mock::MockHarness};
use zeron_mcp::{Origin, Tools, Zeron};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("device-id"), "smoke-device")?;
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(SmokeHarness));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Codex, None)?;
    core.workspace.create_space(
        "smoke-project",
        "smoke-device",
        dir.path().to_str().unwrap(),
        Some("Smoke project".into()),
        false,
    )?;
    core.workspace
        .create_chat("smoke-origin", Some("smoke-project"), None, None, None)?;
    core.workspace.rename_chat("smoke-origin", "Coordinator")?;
    let tools = Tools::new(Arc::new(Zeron::with_client(
        zeron_rpc::memory_client(core.rpc_service()),
        Origin {
            chat_id: Some("smoke-origin".into()),
            device_id: Some("smoke-device".into()),
            ..Default::default()
        },
    )));
    zeron_mcp::serve_stdio(Arc::new(tools)).await?;
    core.shutdown().await;
    Ok(())
}

/// A scripted, installed Codex adapter: the production catalog excludes Mock.
struct SmokeHarness;

#[async_trait]
impl Harness for SmokeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        "Scripted Codex (smoke only)"
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
        Ok(vec![Model {
            id: "smoke-1".into(),
            label: "Smoke 1".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mock = MockHarness {
            script: vec![
                AgentEvent::SessionStarted {
                    harness: HarnessId::Codex,
                    model: "smoke-1".into(),
                    tools: vec![],
                    cwd: request.cwd.clone(),
                    session_id: session_id.clone(),
                    assistant_message_id: uuid::Uuid::new_v4().to_string(),
                },
                AgentEvent::TextDelta {
                    text: "pong".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some(session_id),
                },
            ],
        };
        mock.run(request, controls).await
    }
}
