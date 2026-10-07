//! Deleted chats must retire persistent providers, child work, documents, and
//! session metadata, including tombstones received over the registry protocol.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use tokio::sync::mpsc;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::{memory_client, methods};

struct Live(Arc<AtomicUsize>);
impl Live {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PersistentHarness {
    live: Arc<AtomicUsize>,
    children: Arc<AtomicUsize>,
    parked: bool,
}
#[async_trait]
impl Harness for PersistentHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Deletion fixture"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
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
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let live = Live::new(self.live.clone());
        let child = Live::new(self.children.clone());
        let parked = self.parked;
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            // On Unix this also owns a real process; cancellation must reap it.
            #[cfg(unix)]
            let mut process = tokio::process::Command::new("sleep")
                .arg("60")
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            tx.send(Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "fixture".into(),
                tools: vec![],
                cwd: request.cwd,
                session_id: "persistent-session".into(),
                assistant_message_id: "reply".into(),
            }))
            .await
            .unwrap();
            tx.send(Ok(AgentEvent::TextDelta {
                text: "x".repeat(256 * 1024),
            }))
            .await
            .unwrap();
            tx.send(Ok(AgentEvent::ToolCall {
                id: "spawn".into(),
                call: zeron_proto::ToolCall::Unknown {
                    name: "Agent: child".into(),
                    input: Some(serde_json::json!({"prompt": "child work"})),
                },
            }))
            .await
            .unwrap();
            tx.send(Ok(AgentEvent::ToolResult {
                id: "spawn".into(),
                is_error: false,
                output: None,
                diff: None,
            }))
            .await
            .unwrap();
            tx.send(Ok(AgentEvent::Subagent {
                parent_tool_use_id: "spawn".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "child data".repeat(1024),
                }),
            }))
            .await
            .unwrap();
            if parked {
                tx.send(Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("persistent-session".into()),
                }))
                .await
                .unwrap();
            }
            controls.interrupt.cancelled().await;
            #[cfg(unix)]
            {
                process.kill().await.unwrap();
                process.wait().await.unwrap();
            }
            drop(child);
            drop(live);
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn request() -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: "go".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: std::env::temp_dir().to_string_lossy().into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: None,
    }
}

async fn wait(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("deletion lifecycle did not converge");
}

fn engine(dir: &std::path::Path, parked: bool) -> (EngineCore, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let live = Arc::new(AtomicUsize::new(0));
    let children = Arc::new(AtomicUsize::new(0));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(PersistentHarness {
        live: live.clone(),
        children: children.clone(),
        parked,
    }));
    (
        EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).unwrap(),
        live,
        children,
    )
}

async fn create(core: &EngineCore, id: &str, space: Option<&str>) {
    memory_client(core.rpc_service())
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": id, "deviceId": core.device_id,
                "spaceId": space, "cwd": std::env::temp_dir(),
            }),
        )
        .await
        .unwrap();
}

async fn start(core: &EngineCore, id: &str, parked: bool) {
    core.sessions
        .dispatch(id, HarnessId::Mock, request(), None)
        .await
        .unwrap();
    wait(|| {
        core.sessions.session_status(id).is_some_and(|s| {
            s.status
                == if parked {
                    SessionStatus::Idle
                } else {
                    SessionStatus::Working
                }
        }) && core
            .doc_host
            .open(id)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .iter()
            .any(|e| e.role == zeron_doc::MessageRole::Assistant && !e.parts.is_empty())
    })
    .await;
}

async fn assert_deleted(core: &EngineCore, id: &str) {
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        while core.sessions.session_status(id).is_some() || core.sessions.last_request(id).is_some()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "deleted chat {id} retained status {:?} or request: {}",
        core.sessions.session_status(id),
        core.sessions.last_request(id).is_some()
    );
    // A straggling last-message write previously claimed a fresh row here.
    core.workspace.note_message(id, "late provider output");
    assert!(core.workspace.chat(id).unwrap().is_none());
    assert!(core.doc_host.open(id).is_err());
    assert!(
        core.doc_host.open(&format!("{id}--sub--spawn")).is_err(),
        "deleted child doc reopened"
    );
    assert!(
        core.sessions
            .dispatch(id, HarnessId::Mock, request(), None)
            .await
            .is_err()
    );
}

async fn local_delete(parked: bool, space: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (core, live, children) = engine(dir.path(), parked);
    let client = memory_client(core.rpc_service());
    if space {
        core.workspace
            .create_space(
                "project",
                &core.device_id,
                &std::env::temp_dir().to_string_lossy(),
                None,
                false,
            )
            .unwrap();
    }
    for i in 0..8 {
        let id = format!("delete-{i}");
        create(&core, &id, space.then_some("project")).await;
        start(&core, &id, parked).await;
    }
    create(&core, "survivor", None).await;
    start(&core, "survivor", parked).await;
    wait(|| live.load(Ordering::SeqCst) == 9).await;
    for i in 0..8 {
        let child = format!("delete-{i}--sub--spawn");
        wait(|| {
            !core
                .doc_host
                .open(&child)
                .unwrap()
                .doc()
                .read_entries()
                .unwrap()
                .is_empty()
        })
        .await;
    }
    if space {
        client
            .call(
                methods::MUTATE,
                serde_json::json!({ "op": "deleteSpace", "spaceId": "project" }),
            )
            .await
            .unwrap();
    } else {
        for i in 0..8 {
            client
                .call(
                    methods::MUTATE,
                    serde_json::json!({ "op": "deleteChat", "chatId": format!("delete-{i}") }),
                )
                .await
                .unwrap();
        }
    }
    wait(|| live.load(Ordering::SeqCst) == 1 && children.load(Ordering::SeqCst) == 1).await;
    for i in 0..8 {
        assert_deleted(&core, &format!("delete-{i}")).await;
    }
    assert!(core.sessions.session_status("survivor").is_some());
    core.shutdown().await;
    wait(|| live.load(Ordering::SeqCst) == 0 && children.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_eight_parked_chats_reaps_providers_and_children() {
    local_delete(true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_active_chats_cancels_without_touching_other_chats() {
    local_delete(false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_deletion_uses_the_same_complete_teardown() {
    local_delete(true, true).await;
}

async fn remote_delete(visible: bool) {
    let dir = tempfile::tempdir().unwrap();
    let viewer_dir = tempfile::tempdir().unwrap();
    let (host, live, children) = engine(dir.path(), true);
    let (viewer, _, _) = engine(viewer_dir.path(), true);
    let server = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    host.workspace.connect_registry_url(&server.url());
    viewer.workspace.connect_registry_url(&server.url());
    wait(|| host.workspace.registry_synced() && viewer.workspace.registry_synced()).await;
    if visible {
        create(&host, "remote-delete", None).await;
        wait(|| viewer.workspace.chat("remote-delete").unwrap().is_some()).await;
    }
    // An absent row must allow the remote-create/nudge race. An explicit
    // remote tombstone must end the runtime after its implicit host claim.
    start(&host, "remote-delete", true).await;
    assert_eq!(live.load(Ordering::SeqCst), 1);
    memory_client(viewer.rpc_service())
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": "remote-delete" }),
        )
        .await
        .unwrap();
    wait(|| live.load(Ordering::SeqCst) == 0 && children.load(Ordering::SeqCst) == 0).await;
    assert_deleted(&host, "remote-delete").await;
    wait(|| {
        viewer
            .workspace
            .watch_session_rows()
            .borrow()
            .iter()
            .all(|s| s.chat_id != "remote-delete")
    })
    .await;
    host.shutdown().await;
    viewer.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synced_chat_deletion_stops_its_hosted_provider() {
    remote_delete(true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_before_receiving_a_chat_row_can_later_be_deleted_remotely() {
    remote_delete(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_closes_deleted_journals_and_recovers_other_chats() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _, _) = engine(dir.path(), true);
    create(&core, "deleted-journal", None).await;
    core.workspace.delete_chat("deleted-journal").unwrap();
    core.shutdown().await;
    drop(core);
    let journal =
        zeron_engine::RunJournal::open(dir.path().join("orgs/dev-org/dev-user/journals")).unwrap();
    for id in ["deleted-journal", "surviving-journal"] {
        journal
            .append(
                id,
                &AgentEvent::TextDelta {
                    text: "unfinished".into(),
                },
            )
            .unwrap();
    }
    let (restarted, live, _) = engine(dir.path(), true);
    for id in ["deleted-journal", "surviving-journal"] {
        assert!(matches!(
            journal.last_event(id).unwrap(),
            Some((_, AgentEvent::Done { .. }))
        ));
    }
    assert!(restarted.doc_host.open("deleted-journal").is_err());
    assert!(restarted.doc_host.open("surviving-journal").is_ok());
    assert_eq!(live.load(Ordering::SeqCst), 0);
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_tombstone_releases_an_open_doc_without_a_visible_chat_row() {
    let host_dir = tempfile::tempdir().unwrap();
    let viewer_dir = tempfile::tempdir().unwrap();
    let (host, _, _) = engine(host_dir.path(), true);
    let (viewer, _, _) = engine(viewer_dir.path(), true);
    let server = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    host.workspace.connect_registry_url(&server.url());
    viewer.workspace.connect_registry_url(&server.url());
    wait(|| host.workspace.registry_synced() && viewer.workspace.registry_synced()).await;
    let handle = host.doc_host.open_local("unseen-doc").unwrap();
    let weak = Arc::downgrade(&handle);
    drop(handle);
    assert!(host.workspace.chat("unseen-doc").unwrap().is_none());
    viewer.workspace.delete_chat("unseen-doc").unwrap();
    wait(|| host.doc_host.open_local("unseen-doc").is_err()).await;
    wait(|| weak.upgrade().is_none()).await;
    assert!(host.workspace.chat("unseen-doc").unwrap().is_none());
    host.shutdown().await;
    viewer.shutdown().await;
}
