use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use zeron_doc::*;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, NativeForkControls, NativeForkError, RunControls};
use zeron_proto::*;
use zeron_rpc::methods;

#[derive(Default)]
struct NativeStore {
    provider: Option<HarnessId>,
    forks: AtomicUsize,
    sessions: Mutex<std::collections::HashMap<String, Vec<String>>>,
    requests: Mutex<Vec<RunRequest>>,
    ambiguous: bool,
    reject_once: bool,
    reject_resume: bool,
    unexpected_identity: bool,
    fork_started: Option<Arc<tokio::sync::Notify>>,
    fork_release: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait]
impl Harness for NativeStore {
    fn id(&self) -> HarnessId {
        self.provider.unwrap_or(HarnessId::Mock)
    }
    fn display_name(&self) -> &str {
        "Native store"
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
    async fn native_fork_support(&self, _: &std::path::Path) -> NativeForkAvailability {
        NativeForkAvailability::available()
    }
    async fn fork_native(
        &self,
        point: &NativeForkPoint,
        _: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        let attempt = self.forks.fetch_add(1, Ordering::SeqCst);
        if self.reject_once && attempt == 0 {
            return Err(NativeForkError::Rejected("Helper could not start".into()));
        }
        if self.ambiguous {
            return Err(NativeForkError::Indeterminate("lost provider reply".into()));
        }
        if let Some(started) = &self.fork_started {
            started.notify_one();
        }
        if let Some(release) = &self.fork_release {
            release.notified().await;
        }
        assert_eq!(point.source_session_id, "canonical");
        match &point.boundary {
            NativeForkBoundary::AppServerTurn { turn_id } => assert_eq!(turn_id, "a1"),
            NativeForkBoundary::PiEntry { entry_id } => {
                assert_eq!(self.id(), HarnessId::Pi);
                assert_eq!(entry_id, "native-a1");
            }
            _ => panic!("unexpected boundary"),
        }
        let id = format!("native-child-{}", self.forks.load(Ordering::SeqCst));
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), vec!["U1".into(), "A1".into()]);
        Ok(NativeForkResult {
            session_id: id,
            cwd: point.cwd.clone(),
        })
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let session_id = if self.unexpected_identity {
            Some("wrong-native-session".into())
        } else {
            request.resume.clone()
        };
        self.requests.lock().unwrap().push(request);
        if self.reject_resume {
            return Err(HarnessError::Protocol(
                "Saved native session disappeared".into(),
            ));
        }
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "child answer".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id,
            }),
        ])
        .boxed())
    }
}
fn setup(dir: &std::path::Path, harness: Arc<NativeStore>) -> EngineCore {
    let provider = harness.id();
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness);
    let core = EngineCore::assemble(dir, registry, provider, None).unwrap();
    core.workspace
        .create_chat(
            "main",
            None,
            Some(&core.device_id),
            (provider == HarnessId::Pi).then(|| ChatConfig {
                harness: provider,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            Some("/tmp".into()),
        )
        .unwrap();
    core.workspace
        .set_chat_harness_session("main", "canonical", "/tmp");
    let doc = core.doc_host.open("main").unwrap();
    for (id, role) in [
        ("u1", MessageRole::User),
        ("a1", MessageRole::Assistant),
        ("u2", MessageRole::User),
        ("a2", MessageRole::Assistant),
    ] {
        doc.doc()
            .push_message(&SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![MessagePart::Text {
                    id: format!("text-{id}"),
                    text: id.to_uppercase(),
                }],
                created_at: 1,
                device_id: core.device_id.clone(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
                native_fork_point: None,
            })
            .unwrap();
    }
    doc.doc()
        .set_native_fork_point(
            "a1",
            &NativeForkPoint {
                format_version: 1,
                harness: provider,
                source_device_id: core.device_id.clone(),
                source_session_id: "canonical".into(),
                cwd: "/tmp".into(),
                boundary: if provider == HarnessId::Pi {
                    NativeForkBoundary::PiEntry {
                        entry_id: "native-a1".into(),
                    }
                } else {
                    NativeForkBoundary::AppServerTurn {
                        turn_id: "a1".into(),
                    }
                },
            },
        )
        .unwrap();
    core
}
fn request(core: &EngineCore) -> ForkMessageSideChatRequest {
    ForkMessageSideChatRequest {
        destination: NativeForkDestination::SideChat,
        request_id: "op-one".into(),
        chat_id: "child".into(),
        source_chat_id: "main".into(),
        source_message_id: "a1".into(),
        parent_chat_id: None,
        target_device_id: core.device_id.clone(),
    }
}
#[tokio::test]
async fn native_pi_forks_publish_both_destinations_and_resume_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore {
        provider: Some(HarnessId::Pi),
        ..Default::default()
    });
    let core = setup(dir.path(), harness.clone());
    let client = zeron_rpc::memory_client(core.rpc_service());
    let available: std::collections::HashMap<String, NativeForkAvailability> = client.call_as(methods::GET_NATIVE_FORK_AVAILABILITY,
        serde_json::json!({"sourceChatId":"main","messageIds":["a1"],"targetDeviceId":core.device_id})).await.unwrap();
    assert!(available["a1"].available);
    for (destination, id) in [
        (NativeForkDestination::SideChat, "child"),
        (NativeForkDestination::MainConversation, "pi-main"),
    ] {
        let mut req = request(&core);
        req.destination = destination;
        req.chat_id = id.into();
        req.request_id = format!("pi-{id}");
        let child: Chat = client
            .call_as(
                methods::FORK_MESSAGE_SIDE_CHAT,
                serde_json::to_value(req).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(child.config.as_ref().unwrap().harness, HarnessId::Pi);
        assert_eq!(
            child.parent_chat_id.as_deref(),
            if destination == NativeForkDestination::SideChat {
                Some("main")
            } else {
                None
            }
        );
        let entries = core
            .doc_host
            .open(id)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap();
        assert_eq!(
            entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["u1", "a1", &format!("fork:{id}")]
        );
        assert_eq!(
            entries[1]
                .native_fork_point
                .as_ref()
                .unwrap()
                .source_session_id,
            "canonical"
        );
    }
    assert!(harness.requests.lock().unwrap().is_empty());
    assert_eq!(harness.forks.load(Ordering::SeqCst), 2);
    core.shutdown().await;
    drop(client);
    drop(core);
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let restarted = EngineCore::assemble(dir.path(), registry, HarnessId::Pi, None).unwrap();
    for id in ["child", "pi-main"] {
        let expected = restarted
            .workspace
            .chat(id)
            .unwrap()
            .unwrap()
            .harness_session_id;
        restarted
            .sessions
            .dispatch(id, HarnessId::Pi, send_request("Continue here"), None)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if harness
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r.resume == expected)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    let requests = harness.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for req in requests {
        assert_eq!(req.resume_policy, ResumePolicy::RequireExisting);
        assert_eq!(req.prompt, "Continue here");
    }
    assert_eq!(
        restarted
            .workspace
            .chat("main")
            .unwrap()
            .unwrap()
            .harness_session_id
            .as_deref(),
        Some("canonical")
    );
    restarted.shutdown().await;
}
#[tokio::test]
async fn native_fork_rpc_freezes_exact_prefix_dedupes_and_preserves_canonical_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    let client = zeron_rpc::memory_client(core.rpc_service());
    let params = serde_json::to_value(request(&core)).unwrap();
    let (a, b) = tokio::join!(
        client.call_as::<Chat>(methods::FORK_MESSAGE_SIDE_CHAT, params.clone()),
        client.call_as::<Chat>(methods::FORK_MESSAGE_SIDE_CHAT, params.clone())
    );
    let child = a.unwrap();
    assert_eq!(child, b.unwrap());
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    let entries = core
        .doc_host
        .open("child")
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert_eq!(
        entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["u1", "a1", "fork:child"]
    );
    assert_eq!(
        entries[2].parts,
        vec![MessagePart::Fork {
            id: "fork:child".into(),
            source_chat_id: "main".into(),
            source_title: "Conversation".into(),
        }]
    );
    assert!(harness.requests.lock().unwrap().is_empty());
    assert_eq!(
        harness.sessions.lock().unwrap()[child.harness_session_id.as_ref().unwrap()],
        ["U1", "A1"]
    );
    let mut changed = params.clone();
    changed["sourceMessageId"] = serde_json::json!("a2");
    assert!(
        client
            .call(methods::FORK_MESSAGE_SIDE_CHAT, changed)
            .await
            .is_err()
    );
    let mut inherited = request(&core);
    inherited.request_id = "op-two".into();
    inherited.chat_id = "sibling".into();
    inherited.source_chat_id = "child".into();
    let sibling = core.native_forks.create(inherited).await.unwrap();
    assert_eq!(sibling.parent_chat_id.as_deref(), Some("main"));
    assert_eq!(
        core.doc_host
            .open("sibling")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()[1]
            .native_fork_point,
        entries[1].native_fork_point
    );
    assert_eq!(
        core.doc_host
            .open("main")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .len(),
        4
    );
    core.shutdown().await;
    drop(client);
    drop(core);
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let restarted = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    let replay = zeron_rpc::memory_client(restarted.rpc_service())
        .call_as::<Chat>(methods::FORK_MESSAGE_SIDE_CHAT, params)
        .await
        .unwrap();
    assert_eq!(replay, child);
    assert_eq!(harness.forks.load(Ordering::SeqCst), 2);
    restarted.shutdown().await;
}
#[tokio::test]
async fn native_fork_main_conversation_survives_restart_and_resumes_native_context() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    // A reply from a side chat can also become a root conversation.
    let side = core.native_forks.create(request(&core)).await.unwrap();
    let mut fork = request(&core);
    fork.request_id = "op-main".into();
    fork.chat_id = "new-main".into();
    fork.source_chat_id = side.id.clone();
    fork.parent_chat_id = None;
    fork.destination = NativeForkDestination::MainConversation;
    let client = zeron_rpc::memory_client(core.rpc_service());
    let params = serde_json::to_value(&fork).unwrap();
    let child = client
        .call_as::<Chat>(methods::FORK_MESSAGE_SIDE_CHAT, params.clone())
        .await
        .unwrap();
    assert_eq!(child.parent_chat_id, None);
    assert_eq!(child.device_id, side.device_id);
    assert_eq!(child.cwd, side.cwd);
    assert!(harness.requests.lock().unwrap().is_empty());
    assert_eq!(
        core.doc_host
            .open("new-main")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["u1", "a1", "fork:new-main"]
    );
    // Reusing an operation ID with another destination is not a retry.
    let mut changed = fork.clone();
    changed.destination = NativeForkDestination::SideChat;
    assert!(core.native_forks.create(changed).await.is_err());
    let mut invalid = fork.clone();
    invalid.request_id = "invalid-parent".into();
    invalid.chat_id = "invalid-child".into();
    invalid.parent_chat_id = Some("main".into());
    assert!(
        core.native_forks
            .create(invalid)
            .await
            .unwrap_err()
            .contains("visual parent")
    );
    assert_eq!(harness.forks.load(Ordering::SeqCst), 2);
    core.shutdown().await;
    drop(client);
    drop(core);
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let restarted = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    let replay = zeron_rpc::memory_client(restarted.rpc_service())
        .call_as::<Chat>(methods::FORK_MESSAGE_SIDE_CHAT, params)
        .await
        .unwrap();
    assert_eq!(replay, child);
    assert_eq!(
        restarted
            .workspace
            .chat("new-main")
            .unwrap()
            .unwrap()
            .parent_chat_id,
        None
    );
    restarted
        .sessions
        .dispatch(
            "new-main",
            HarnessId::Mock,
            send_request("Continue here"),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if !harness.requests.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let requests = harness.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].resume, child.harness_session_id);
    assert_eq!(requests[0].resume_policy, ResumePolicy::RequireExisting);
    assert_eq!(requests[0].prompt, "Continue here");
    assert_eq!(harness.forks.load(Ordering::SeqCst), 2);
    assert_eq!(
        restarted
            .workspace
            .chat("main")
            .unwrap()
            .unwrap()
            .harness_session_id
            .as_deref(),
        Some("canonical")
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn native_fork_indeterminate_never_retries_or_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore {
        ambiguous: true,
        ..Default::default()
    });
    let core = setup(dir.path(), harness.clone());
    let request = request(&core);
    for _ in 0..2 {
        assert!(
            core.native_forks
                .create(request.clone())
                .await
                .unwrap_err()
                .contains("indeterminate")
        );
    }
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    assert!(core.workspace.chat("child").unwrap().is_none());
    core.shutdown().await;
}
#[tokio::test]
async fn native_fork_availability_and_host_validation_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    let availability = core
        .native_forks
        .availability(NativeForkAvailabilityRequest {
            source_chat_id: "main".into(),
            message_ids: vec!["a1".into(), "a2".into(), "u1".into()],
            target_device_id: core.device_id.clone(),
        })
        .await
        .unwrap();
    assert!(availability["a1"].available);
    assert!(!availability["a2"].available);
    assert!(!availability["u1"].available);
    let mut bad = request(&core);
    bad.target_device_id = "foreign".into();
    assert!(core.native_forks.create(bad).await.is_err());
    let mut bad = request(&core);
    bad.parent_chat_id = Some("unrelated".into());
    assert!(core.native_forks.create(bad).await.is_err());
    assert_eq!(harness.forks.load(Ordering::SeqCst), 0);
    core.shutdown().await;
}

#[tokio::test]
async fn native_fork_provider_created_recovery_reuses_the_durable_native_id() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    let request = request(&core);
    let child = core.native_forks.create(request.clone()).await.unwrap();
    // Simulate the workspace row not surviving publication; the durable provider
    // result and child snapshot must be sufficient to repair it at startup.
    core.workspace.delete_chat("child").unwrap();
    core.shutdown().await;
    drop(core);
    fn records(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = vec![];
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(records(&path));
            } else if path.parent().unwrap().file_name().unwrap() == "native-forks"
                && path.extension().is_some_and(|s| s == "json")
            {
                out.push(path);
            }
        }
        out
    }
    let path = records(dir.path()).pop().unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["phase"] = serde_json::json!("ProviderCreated");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let restarted = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    assert_eq!(restarted.native_forks.create(request).await.unwrap(), child);
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    assert_eq!(
        restarted
            .doc_host
            .open("child")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .len(),
        3
    );
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["phase"], "Published");
    restarted.shutdown().await;
}

async fn registry_write_failure_recovers(restart_before_retry: bool) {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let mut core = setup(dir.path(), harness.clone());
    let request = request(&core);
    core.workspace.flush().unwrap();
    let store_root = core.uploads.dir().parent().unwrap().to_path_buf();
    let database = rusqlite::Connection::open(store_root.join("docs.sqlite3")).unwrap();
    // Fail only registry writes: the provider result and child document can
    // still become durable, reproducing a failure at the publication boundary.
    database
        .execute_batch(&format!(
            "CREATE TRIGGER fail_native_fork_registry BEFORE INSERT ON snapshots
         WHEN NEW.doc_id = '{REGISTRY_DOC_ID}'
         BEGIN SELECT RAISE(FAIL, 'injected registry write failure'); END;"
        ))
        .unwrap();
    let device_id = core.device_id.clone();
    let persisted_chats = || {
        let bytes: Vec<u8> = database
            .query_row(
                "SELECT bytes FROM snapshots WHERE doc_id = ?1",
                [REGISTRY_DOC_ID],
                |row| row.get(0),
            )
            .unwrap();
        RegistryDoc::from_bytes(&bytes, &device_id)
            .unwrap()
            .read_chats()
            .unwrap()
    };
    let error = core.native_forks.create(request.clone()).await.unwrap_err();
    assert!(error.contains("injected registry write failure"), "{error}");
    assert!(core.workspace.chat("child").unwrap().is_some());
    assert!(!persisted_chats().iter().any(|chat| chat.id == "child"));
    let record_path = std::fs::read_dir(store_root.join("native-forks"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|s| s == "json"))
        .unwrap();
    let record = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap()
    };
    assert_eq!(record()["phase"], "ProviderCreated");
    assert_eq!(record()["child"]["sessionId"], "native-child-1");

    // The existing in-memory row must not let a retry acknowledge the same
    // failed disk write, or discard the recoverable provider result.
    let error = core.native_forks.create(request.clone()).await.unwrap_err();
    assert!(error.contains("injected registry write failure"), "{error}");
    assert_eq!(record()["phase"], "ProviderCreated");
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);

    let reopen = || {
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(harness.clone());
        EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap()
    };
    if restart_before_retry {
        // Keep the fault through shutdown so no final flush can hide the loss.
        core.shutdown().await;
        drop(core);
        assert!(!persisted_chats().iter().any(|chat| chat.id == "child"));
        database
            .execute_batch("DROP TRIGGER fail_native_fork_registry;")
            .unwrap();
        core = reopen();
        assert!(
            core.workspace.chat("child").unwrap().is_some(),
            "Startup must finish publication"
        );
    } else {
        database
            .execute_batch("DROP TRIGGER fail_native_fork_registry;")
            .unwrap();
    }
    let child = core.native_forks.create(request.clone()).await.unwrap();
    assert_eq!(child.harness_session_id.as_deref(), Some("native-child-1"));
    assert_eq!(record()["phase"], "Published");
    let durable_child = persisted_chats()
        .into_iter()
        .find(|chat| chat.id == child.id)
        .expect("The confirmed child must already be on disk");
    assert_eq!(durable_child.harness_session_id, child.harness_session_id);
    assert_eq!(durable_child.parent_chat_id, child.parent_chat_id);
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    core.shutdown().await;
    drop(core);

    let restarted = reopen();
    assert_eq!(
        restarted.workspace.chat("child").unwrap(),
        Some(durable_child)
    );
    assert_eq!(restarted.native_forks.create(request).await.unwrap(), child);
    assert_eq!(
        restarted
            .doc_host
            .open("child")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .len(),
        3,
    );
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    restarted.shutdown().await;
}

#[tokio::test]
async fn native_fork_registry_write_failure_retries_publication_in_memory() {
    registry_write_failure_recovers(false).await;
}

#[tokio::test]
async fn native_fork_registry_write_failure_recovers_after_restart() {
    registry_write_failure_recovers(true).await;
}

fn send_request(prompt: &str) -> RunRequest {
    serde_json::from_value(serde_json::json!({
        "prompt": prompt, "cwd": "/tmp", "sandbox": "workspace-write", "autoApprove": true,
        "resume": "untrusted-parent", "mcp": {"name":"zeron", "command":"parent", "env":{"ZERON_CHAT_ID":"main"}}
    })).unwrap()
}

async fn send_native(core: &EngineCore, harness: &NativeStore, prompt: &str) {
    let before = harness.requests.lock().unwrap().len();
    core.sessions
        .dispatch("child", HarnessId::Mock, send_request(prompt), None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while harness.requests.lock().unwrap().len() == before
            || core
                .sessions
                .session_status("child")
                .is_some_and(|s| s.status != SessionStatus::Idle)
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let captured = harness.requests.lock().unwrap()[before].clone();
    assert_eq!(captured.resume.as_deref(), Some("native-child-1"));
    assert_eq!(captured.resume_policy, ResumePolicy::RequireExisting);
    assert_eq!(captured.prompt, prompt);
    assert_eq!(captured.mcp.unwrap().env["ZERON_CHAT_ID"], "child");
}

#[tokio::test]
async fn native_fork_resume_survives_restart_and_never_wraps_history() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    core.native_forks.create(request(&core)).await.unwrap();
    core.shutdown().await;
    drop(core);
    for prompts in [["/review", "What do you remember?"], ["Continue", "Again"]] {
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(harness.clone());
        let core = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
        core.sessions.set_ipc_port(27699);
        for prompt in prompts {
            send_native(&core, &harness, prompt).await;
        }
        let mut changed = send_request("wrong checkout");
        changed.cwd = "/".into();
        assert!(
            core.sessions
                .dispatch("child", HarnessId::Mock, changed, None)
                .await
                .is_err()
        );
        assert!(
            core.sessions
                .dispatch(
                    "child",
                    HarnessId::Codex,
                    send_request("wrong harness"),
                    None
                )
                .await
                .is_err()
        );
        assert!(
            core.doc_host
                .open("child")
                .unwrap()
                .doc()
                .native_fork_lineage()
                .unwrap()
                .is_some()
        );
        core.shutdown().await;
    }
    assert_eq!(harness.requests.lock().unwrap().len(), 4);
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_fork_resume_failure_cannot_retry_fresh_or_replace_identity() {
    for unexpected_identity in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let harness = Arc::new(NativeStore {
            reject_resume: !unexpected_identity,
            unexpected_identity,
            ..Default::default()
        });
        let core = setup(dir.path(), harness.clone());
        core.native_forks.create(request(&core)).await.unwrap();
        core.sessions
            .dispatch("child", HarnessId::Mock, send_request("Continue"), None)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if core
                    .sessions
                    .session_status("child")
                    .is_some_and(|s| s.status == SessionStatus::Errored)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(harness.requests.lock().unwrap().len(), 1);
        assert_eq!(
            core.workspace
                .chat("child")
                .unwrap()
                .unwrap()
                .harness_session_id
                .as_deref(),
            Some("native-child-1")
        );
        assert!(
            core.doc_host
                .open("child")
                .unwrap()
                .doc()
                .native_fork_lineage()
                .unwrap()
                .is_some()
        );
        core.shutdown().await;
    }
}

#[tokio::test]
async fn native_fork_update_blocks_creation_and_keeps_availability_honest() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    core.registry.begin_update(HarnessId::Mock);
    let availability = core
        .native_forks
        .availability(NativeForkAvailabilityRequest {
            source_chat_id: "main".into(),
            message_ids: vec!["a1".into()],
            target_device_id: core.device_id.clone(),
        })
        .await
        .unwrap();
    assert!(!availability["a1"].available);
    assert!(
        core.native_forks
            .create(request(&core))
            .await
            .unwrap_err()
            .contains("update")
    );
    assert_eq!(harness.forks.load(Ordering::SeqCst), 0);
    core.registry.end_update(HarnessId::Mock);
    core.native_forks.create(request(&core)).await.unwrap();
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn native_fork_prepared_restart_is_indeterminate_and_never_recreates() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore::default());
    let core = setup(dir.path(), harness.clone());
    let request = request(&core);
    core.native_forks.create(request.clone()).await.unwrap();
    core.workspace.delete_chat("child").unwrap();
    core.shutdown().await;
    drop(core);
    fn rewrite(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rewrite(&path);
            } else if path.parent().unwrap().file_name().unwrap() == "native-forks"
                && path.extension().is_some_and(|s| s == "json")
            {
                let mut record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                record["phase"] = serde_json::json!("Prepared");
                record["child"] = serde_json::Value::Null;
                std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        }
    }
    rewrite(dir.path());
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    assert!(
        core.native_forks
            .create(request)
            .await
            .unwrap_err()
            .contains("will not be retried")
    );
    assert!(core.workspace.chat("child").unwrap().is_none());
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn native_fork_definite_rejection_can_retry_the_same_operation() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(NativeStore {
        reject_once: true,
        ..Default::default()
    });
    let core = setup(dir.path(), harness.clone());
    let request = request(&core);
    assert!(core.native_forks.create(request.clone()).await.is_err());
    assert!(core.workspace.chat("child").unwrap().is_none());
    let child = core.native_forks.create(request.clone()).await.unwrap();
    assert_eq!(core.native_forks.create(request).await.unwrap(), child);
    assert_eq!(harness.forks.load(Ordering::SeqCst), 2);
    assert_eq!(harness.sessions.lock().unwrap().len(), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn native_fork_never_overwrites_a_destination_created_during_provider_io() {
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let harness = Arc::new(NativeStore {
        fork_started: Some(started.clone()),
        fork_release: Some(release.clone()),
        ..Default::default()
    });
    let core = setup(dir.path(), harness.clone());
    let request = request(&core);
    let task = tokio::spawn({
        let forks = core.native_forks.clone();
        let request = request.clone();
        async move { forks.create(request).await }
    });
    started.notified().await;
    core.workspace
        .create_chat(
            "child",
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    core.workspace
        .rename_chat("child", "Created by another action")
        .unwrap();
    let foreign = core.workspace.chat("child").unwrap().unwrap();
    release.notify_one();
    assert!(task.await.unwrap().unwrap_err().contains("occupied"));
    assert!(core.native_forks.create(request).await.is_err());
    assert_eq!(core.workspace.chat("child").unwrap().unwrap(), foreign);
    assert!(
        core.doc_host
            .open("child")
            .unwrap()
            .doc()
            .native_fork_lineage()
            .unwrap()
            .is_none()
    );
    assert_eq!(harness.forks.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}
