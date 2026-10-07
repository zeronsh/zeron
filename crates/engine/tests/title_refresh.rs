//! A long chat's generated title is re-checked every fourth prompt through the
//! real dispatch path, and a title a person typed is left alone.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::mock::MockHarness;
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

const CHAT: &str = "chat-refresh-e2e";

/// Mock coding turns that finish at once; title runs answer from a queue.
struct Titler {
    coding: MockHarness,
    titles: Mutex<VecDeque<String>>,
    title_calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl Harness for Titler {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Titler"
    }
    fn supports_steering(&self) -> bool {
        self.coding.supports_steering()
    }
    fn steering_mode(&self) -> SteeringMode {
        self.coding.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(Vec::new())
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.coding.run(request, controls).await
    }
    async fn run_title(
        &self,
        _: RunRequest,
        _: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        *self.title_calls.lock().unwrap() += 1;
        let reply = self.titles.lock().unwrap().pop_front().unwrap_or_default();
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta { text: reply }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }),
        ])
        .boxed())
    }
}

fn engine(dir: &std::path::Path, title_replies: &[&str]) -> (EngineCore, Arc<Titler>) {
    let titler = Arc::new(Titler {
        coding: MockHarness {
            script: vec![AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }],
        },
        titles: Mutex::new(title_replies.iter().map(|t| (*t).to_owned()).collect()),
        title_calls: Mutex::new(0),
    });
    let registry = HarnessRegistry::new();
    registry.register(titler.clone());
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine assembles");
    core.workspace
        .create_space(
            "space",
            &core.device_id,
            &dir.to_string_lossy(),
            None,
            false,
        )
        .expect("space");
    core.workspace
        .create_chat(CHAT, Some("space"), None, None, None)
        .expect("chat");
    (core, titler)
}

async fn send(core: &EngineCore, dir: &std::path::Path, prompt: &str) {
    let request = RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: dir.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    };
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, request, None)
        .await
        .expect("dispatch");
    // Let the mock turn finish before the next prompt arrives.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

async fn wait_for_title(core: &EngineCore, wanted: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let title = core.workspace.chat(CHAT).unwrap().and_then(|c| c.title);
        if title.as_deref() == Some(wanted) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for title {wanted:?}, have {title:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_fourth_prompt_refreshes_a_generated_title() {
    let dir = tempfile::tempdir().unwrap();
    let (core, titler) = engine(dir.path(), &["Fix Login Flow", "Ship Dark Mode"]);

    send(&core, dir.path(), "fix the login flow").await;
    wait_for_title(&core, "Fix Login Flow").await;
    for prompt in ["now the password reset", "and its email copy"] {
        send(&core, dir.path(), prompt).await;
    }
    // Three prompts in: the title has not been re-checked yet.
    assert_eq!(*titler.title_calls.lock().unwrap(), 1);
    assert_eq!(
        core.workspace.chat(CHAT).unwrap().unwrap().title.as_deref(),
        Some("Fix Login Flow")
    );

    send(&core, dir.path(), "actually, let's add dark mode").await;
    wait_for_title(&core, "Ship Dark Mode").await;
    assert_eq!(*titler.title_calls.lock().unwrap(), 2);
    core.shutdown().await;
}

#[tokio::test]
async fn a_title_a_person_typed_is_never_refreshed() {
    let dir = tempfile::tempdir().unwrap();
    let (core, titler) = engine(dir.path(), &["Unwanted"]);
    core.workspace.rename_chat(CHAT, "My Own Name").unwrap();

    for prompt in ["one", "two", "three", "four", "five"] {
        send(&core, dir.path(), prompt).await;
    }
    assert_eq!(*titler.title_calls.lock().unwrap(), 0);
    assert_eq!(
        core.workspace.chat(CHAT).unwrap().unwrap().title.as_deref(),
        Some("My Own Name")
    );
    core.shutdown().await;
}
