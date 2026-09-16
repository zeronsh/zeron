use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{Arc, Mutex};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
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
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "Side answer".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("side-provider-session".into()),
            }),
        ])
        .boxed())
    }
}
fn message(id: &str, role: MessageRole, text: &str, status: MessageStatus) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: format!("{id}-text"),
            text: text.into(),
        }],
        created_at: 1,
        device_id: "device".into(),
        status: Some(status),
        continuation_of: None,
    }
}

#[tokio::test]
async fn fork_is_frozen_durable_idempotent_and_has_an_independent_provider_session() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.workspace
        .create_chat(
            "main",
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    core.workspace
        .rename_chat("main", "Main conversation")
        .unwrap();
    core.workspace
        .set_chat_harness_session("main", "parent-provider-session", "/tmp");
    let source = core.doc_host.open("main").unwrap();
    source
        .doc()
        .push_message(&message(
            "u1",
            MessageRole::User,
            "Remember PINEAPPLE",
            MessageStatus::Complete,
        ))
        .unwrap();
    source
        .doc()
        .push_message(&message(
            "a1",
            MessageRole::Assistant,
            "I remember PINEAPPLE",
            MessageStatus::Complete,
        ))
        .unwrap();
    source
        .doc()
        .push_message(&message(
            "u2",
            MessageRole::User,
            "unfinished turn",
            MessageStatus::Complete,
        ))
        .unwrap();
    source
        .doc()
        .push_message(&message(
            "a2",
            MessageRole::Assistant,
            "still streaming",
            MessageStatus::Streaming,
        ))
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let params = serde_json::json!({ "chatId": "side", "sourceChatId": "main", "targetDeviceId": core.device_id });
    let fork = client
        .call_as::<zeron_proto::Chat>(methods::FORK_SIDE_CHAT, params.clone())
        .await
        .unwrap();
    assert_eq!(fork.parent_chat_id.as_deref(), Some("main"));
    assert_eq!(fork.harness_session_id, None);
    assert_eq!(fork.cwd, Some("/tmp".into()));
    assert!(fork.on_chat2());
    let target = core.doc_host.open("side").unwrap();
    assert_eq!(
        target.doc().read_entries().unwrap(),
        source.doc().read_entries().unwrap()[..2]
    );
    client.call(methods::FORK_SIDE_CHAT, params).await.unwrap();
    assert_eq!(target.doc().read_entries().unwrap().len(), 2);
    core.sessions
        .dispatch(
            "side",
            HarnessId::Mock,
            RunRequest {
                prompt: "What did I ask you to remember?".into(),
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
            },
            Some("side-user".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = requests.lock().unwrap()[0].clone();
    assert_eq!(request.resume, None);
    assert!(request.prompt.contains("PINEAPPLE"));
    assert!(!request.prompt.contains("unfinished turn"));
    assert_eq!(source.doc().read_entries().unwrap().len(), 4);
    assert_eq!(
        target.doc().read_entries().unwrap()[2].parts[0],
        MessagePart::Text {
            id: target.doc().read_entries().unwrap()[2].parts[0]
                .id()
                .to_string(),
            text: "What did I ask you to remember?".into()
        }
    );
    core.shutdown().await;
    drop(client);
    drop(source);
    drop(target);
    drop(core);
    let restarted = EngineCore::assemble(
        dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    assert_eq!(
        restarted
            .workspace
            .chat("side")
            .unwrap()
            .unwrap()
            .parent_chat_id
            .as_deref(),
        Some("main")
    );
    let entries = restarted
        .doc_host
        .open("side")
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert!(entries.iter().any(|e| e.id == "a1"));
    assert!(!entries.iter().any(|e| e.id == "a2"));
    restarted.shutdown().await;
}

#[tokio::test]
async fn cannot_fork_an_empty_chat_or_overwrite_a_main_chat() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    core.workspace
        .create_chat("main", None, Some(&core.device_id), None, None)
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    assert!(
        client
            .call(
                methods::FORK_SIDE_CHAT,
                serde_json::json!({ "chatId": "side", "sourceChatId": "main" })
            )
            .await
            .is_err()
    );
    assert!(core.workspace.chat("side").unwrap().is_none());
    assert!(
        client
            .call(
                methods::FORK_SIDE_CHAT,
                serde_json::json!({ "chatId": "main", "sourceChatId": "main" })
            )
            .await
            .is_err()
    );
}
