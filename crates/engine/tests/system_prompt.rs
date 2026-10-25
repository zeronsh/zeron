//! The device's own system prompt: set over RPC, persisted, and stamped onto
//! every run the host dispatches.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{Arc, Mutex};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_rpc::methods;

struct Capture(Arc<Mutex<Vec<RunRequest>>>);
#[async_trait]
impl Harness for Capture {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Capture"
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
        self.0.lock().unwrap().push(request);
        Ok(futures::stream::iter(vec![Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("provider-session".into()),
        })])
        .boxed())
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: vec![],
        worktree: None,
        env: Default::default(),
        // A sender's value never counts: the host stamps its own.
        system_prompt: Some("from the sender".into()),
    }
}

async fn turn(
    core: &EngineCore,
    requests: &Arc<Mutex<Vec<RunRequest>>>,
    prompt: &str,
) -> RunRequest {
    let before = requests.lock().unwrap().len();
    core.sessions
        .dispatch("chat", HarnessId::Mock, request(prompt), None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().unwrap().len() == before {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    requests.lock().unwrap()[before].clone()
}

#[tokio::test]
async fn the_system_prompt_is_set_over_rpc_and_reaches_every_run() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.workspace
        .create_chat(
            "chat",
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());

    let unset = client
        .call(methods::GET_SYSTEM_PROMPT, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(unset["text"], serde_json::Value::Null);
    assert_eq!(turn(&core, &requests, "one").await.system_prompt, None);

    let set = client
        .call(
            methods::SET_SYSTEM_PROMPT,
            serde_json::json!({ "text": "Answer in French." }),
        )
        .await
        .unwrap();
    assert_eq!(set["text"], "Answer in French.");
    assert_eq!(
        turn(&core, &requests, "two").await.system_prompt.as_deref(),
        Some("Answer in French.")
    );

    // Blank clears it.
    let cleared = client
        .call(
            methods::SET_SYSTEM_PROMPT,
            serde_json::json!({ "text": "  " }),
        )
        .await
        .unwrap();
    assert_eq!(cleared["text"], serde_json::Value::Null);
    assert_eq!(turn(&core, &requests, "three").await.system_prompt, None);

    let too_long = "x".repeat(64 * 1024 + 1);
    assert!(
        client
            .call(
                methods::SET_SYSTEM_PROMPT,
                serde_json::json!({ "text": too_long })
            )
            .await
            .is_err()
    );
}

#[test]
fn the_system_prompt_persists_on_the_device() {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.load_prefs(dir.path());
    registry
        .set_system_prompt(Some("Be brief.".into()))
        .unwrap();
    let reloaded = HarnessRegistry::new();
    reloaded.load_prefs(dir.path());
    assert_eq!(reloaded.system_prompt().as_deref(), Some("Be brief."));
}
