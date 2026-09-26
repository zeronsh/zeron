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
        duration_ms: None,
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
    let copied = target.doc().read_entries().unwrap();
    // The frozen prefix, then the seam: a system entry naming the source.
    assert_eq!(copied[..2], source.doc().read_entries().unwrap()[..2]);
    assert_eq!(copied.len(), 3);
    assert_eq!(copied[2].role, MessageRole::System);
    assert_eq!(copied[2].status, Some(MessageStatus::Complete));
    assert_eq!(
        copied[2].parts,
        vec![zeron_doc::MessagePart::Fork {
            id: "fork:side".into(),
            source_chat_id: "main".into(),
            source_title: "Main conversation".into(),
        }]
    );
    client.call(methods::FORK_SIDE_CHAT, params).await.unwrap();
    assert_eq!(target.doc().read_entries().unwrap().len(), 3);
    core.sessions.set_ipc_port(27699);
    core.sessions
        .dispatch(
            "side",
            HarnessId::Mock,
            RunRequest {
                mcp: None,
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
    // The host stamps its MCP server onto the run: this binary's `zeron
    // mcp`, dialing the served port, identified as the side chat.
    let mcp = request
        .mcp
        .clone()
        .expect("run carries the zeron MCP server");
    assert_eq!(mcp.name, "zeron");
    assert_eq!(mcp.args, ["mcp"]);
    assert_eq!(mcp.env["ZERON_IPC_PORT"], "27699");
    assert_eq!(mcp.env["ZERON_CHAT_ID"], "side");
    assert_eq!(mcp.env["ZERON_DEVICE_ID"], core.device_id);
    assert!(request.prompt.contains("PINEAPPLE"));
    assert!(!request.prompt.contains("unfinished turn"));
    assert_eq!(source.doc().read_entries().unwrap().len(), 4);
    // The fork's first own turn lands after the seam, and the seam itself
    // never reaches the provider as conversation text.
    assert_eq!(
        target.doc().read_entries().unwrap()[3].parts[0],
        MessagePart::Text {
            id: target.doc().read_entries().unwrap()[3].parts[0]
                .id()
                .to_string(),
            text: "What did I ask you to remember?".into()
        }
    );
    assert!(!request.prompt.contains("fork:side"));
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

#[tokio::test]
async fn forking_a_side_chat_can_land_as_a_sibling_under_the_main_chat() {
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
    let main = core.doc_host.open("main").unwrap();
    main.doc()
        .push_message(&message(
            "u1",
            MessageRole::User,
            "hi",
            MessageStatus::Complete,
        ))
        .unwrap();
    main.doc()
        .push_message(&message(
            "a1",
            MessageRole::Assistant,
            "hello",
            MessageStatus::Complete,
        ))
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    // A first-level side chat, then its own fork button: the copy hangs
    // under MAIN (the side chat's parent), not under the side chat.
    let side = client
        .call_as::<zeron_proto::Chat>(
            methods::FORK_SIDE_CHAT,
            serde_json::json!({ "chatId": "side", "sourceChatId": "main" }),
        )
        .await
        .unwrap();
    core.workspace.rename_chat("side", "Side quest").unwrap();
    assert_eq!(side.parent_chat_id.as_deref(), Some("main"));
    // A turn of its own after the seam, so the seam sits inside the frozen
    // prefix the next fork copies.
    let side_doc = core.doc_host.open("side").unwrap();
    side_doc
        .doc()
        .push_message(&message(
            "u2",
            MessageRole::User,
            "more",
            MessageStatus::Complete,
        ))
        .unwrap();
    side_doc
        .doc()
        .push_message(&message(
            "a2",
            MessageRole::Assistant,
            "sure",
            MessageStatus::Complete,
        ))
        .unwrap();
    let sibling = client
        .call_as::<zeron_proto::Chat>(
            methods::FORK_SIDE_CHAT,
            serde_json::json!({
                "chatId": "side-2",
                "sourceChatId": "side",
                "parentChatId": "main",
            }),
        )
        .await
        .unwrap();
    assert_eq!(sibling.parent_chat_id.as_deref(), Some("main"));
    let entries = core
        .doc_host
        .open("side-2")
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    // Lineage survives: the side chat's own seam is copied, then a new one
    // names the side chat (by its title at fork time) as this copy's source.
    let seams: Vec<_> = entries
        .iter()
        .filter_map(|e| match e.parts.first() {
            Some(zeron_doc::MessagePart::Fork { source_title, .. }) => Some(source_title.clone()),
            _ => None,
        })
        .collect();
    // The untitled main chat falls back to the placeholder title.
    assert_eq!(seams, ["New session", "Side quest"]);
    // An empty parent falls back to the source, like an omitted one.
    let nested = client
        .call_as::<zeron_proto::Chat>(
            methods::FORK_SIDE_CHAT,
            serde_json::json!({
                "chatId": "side-3",
                "sourceChatId": "side",
                "parentChatId": "",
            }),
        )
        .await
        .unwrap();
    assert_eq!(nested.parent_chat_id.as_deref(), Some("side"));
}

/// One turn through the engine; returns the request the provider received.
async fn side_turn(
    core: &EngineCore,
    requests: &Arc<Mutex<Vec<RunRequest>>>,
    chat: &str,
    prompt: &str,
    message_id: &str,
) -> RunRequest {
    let before = requests.lock().unwrap().len();
    let resume = core
        .workspace
        .chat(chat)
        .unwrap()
        .unwrap()
        .harness_session_id;
    core.sessions
        .dispatch(
            chat,
            HarnessId::Mock,
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
                resume,
                attachments: vec![],
                worktree: None,
            },
            Some(message_id.into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().unwrap().len() == before
            || core
                .sessions
                .session_status(chat)
                .is_some_and(|s| s.status != zeron_proto::SessionStatus::Idle)
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    requests.lock().unwrap()[before].clone()
}

/// A native command must lead the delivered prompt so the provider routes it
/// (Codex `command_request`, OpenCode commands): no bootstrap wrapper in front
/// of it, and none at all when there is no prior conversation.
#[tokio::test]
async fn native_commands_and_empty_side_chats_skip_the_history_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    for (id, parent) in [
        ("main", None),
        ("plus-review", Some("main")),
        ("plus", Some("main")),
    ] {
        core.workspace
            .create_chat_with_parent(
                id,
                None,
                Some(&core.device_id),
                None,
                Some("/tmp".into()),
                parent.map(str::to_owned),
            )
            .unwrap();
    }
    // A fresh `+` side chat: nothing to bootstrap.
    let request = side_turn(&core, &requests, "plus-review", "/review", "u-review").await;
    assert_eq!(request.prompt, "/review");
    let request = side_turn(&core, &requests, "plus", "hello there", "u-hello").await;
    assert_eq!(request.prompt, "hello there");

    // A fork whose first turn is a command: the command goes out bare, and
    // the copied history rides the first ordinary turn, once.
    let source = core.doc_host.open("main").unwrap();
    for (id, role, text) in [
        ("u1", MessageRole::User, "Remember PINEAPPLE"),
        ("a1", MessageRole::Assistant, "I remember PINEAPPLE"),
    ] {
        source
            .doc()
            .push_message(&message(id, role, text, MessageStatus::Complete))
            .unwrap();
    }
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            methods::FORK_SIDE_CHAT,
            serde_json::json!({ "chatId": "fork", "sourceChatId": "main" }),
        )
        .await
        .unwrap();
    let request = side_turn(&core, &requests, "fork", "  /review tests", "f1").await;
    assert_eq!(request.resume, None);
    assert_eq!(request.prompt, "  /review tests");
    let request = side_turn(&core, &requests, "fork", "What should I remember?", "f2").await;
    assert_eq!(request.resume.as_deref(), Some("side-provider-session"));
    assert!(request.prompt.contains("PINEAPPLE"), "{}", request.prompt);
    assert!(request.prompt.ends_with("What should I remember?"));
    assert!(!request.prompt.contains("/review"), "{}", request.prompt);
    let request = side_turn(&core, &requests, "fork", "And now?", "f3").await;
    assert_eq!(request.prompt, "And now?");
    core.shutdown().await;
}
