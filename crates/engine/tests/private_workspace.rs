//! Real private hub, two independently persisted agent servers, and a client.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::json;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use zeron_doc::SessionCommandPayload;
use zeron_engine::{Engine, EngineConfig, EngineRuntime, HarnessId, WorkspaceScope};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_private::{NodeRole, PrivateConfig};
use zeron_proto::{AgentEvent, DoneStatus, Model, ReasoningLevel, RunRequest, SteeringMode};
use zeron_rpc::{RpcService, memory_client, methods};

struct CountingHarness(Arc<AtomicUsize>);
#[async_trait]
impl Harness for CountingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Private fixture"
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
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "fixture".into(),
                tools: vec![],
                cwd: request.cwd,
                session_id: "fixture".into(),
                assistant_message_id: uuid::Uuid::new_v4().to_string(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!("Private reply: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("fixture".into()),
            }),
        ])
        .boxed())
    }
}

fn engine_config(dir: &Path, cloud: &str) -> EngineConfig {
    EngineConfig {
        data_dir: dir.into(),
        edge_url: cloud.into(),
        edge_token: Some("must-not-leave-private".into()),
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: Some("must-not-use-workos".into()),
    }
}
async fn start(dir: &Path, cloud: &str, count: Arc<AtomicUsize>) -> EngineRuntime {
    let config = engine_config(dir, cloud);
    let auth = Engine::build_auth(&config).await;
    assert!(auth.is_private());
    assert_eq!(
        auth.access_token().await,
        Err(zeron_rpc::TokenError::SignedOut)
    );
    assert!(!auth.workos_enabled());
    assert_eq!(
        Engine::initial_workspace_scope(&auth),
        WorkspaceScope::Private
    );
    let profile = Engine::resolve_profile(&config, &auth, WorkspaceScope::Private)
        .unwrap()
        .unwrap();
    assert!(
        profile
            .store_root()
            .starts_with(dir.join("profiles/private"))
    );
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();
    runtime
        .core()
        .registry
        .register(Arc::new(CountingHarness(count)));
    runtime
}
async fn wait(mut f: impl FnMut() -> bool, what: &str) {
    tokio::time::timeout(Duration::from_secs(25), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timed out: {what}"));
}
async fn enroll(host: &EngineRuntime, dir: &Path, id: &str, role: NodeRole, url: &str) {
    std::fs::write(dir.join("device-id"), id).unwrap();
    let client = memory_client(host.core().rpc_service());
    let invite = client
        .call(methods::CREATE_PRIVATE_INVITATION, json!({"role":role}))
        .await
        .unwrap();
    PrivateConfig::join(dir, url, invite["code"].as_str().unwrap(), id, id, role)
        .await
        .unwrap();
}
fn request(text: &str) -> RunRequest {
    RunRequest {
        prompt: text.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: None,
    }
}

fn run(runtime: &EngineRuntime, chat: &str, text: &str) -> String {
    runtime
        .core()
        .doc_host
        .queue_command(
            chat,
            SessionCommandPayload::Run {
                request: request(text),
                message_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_runtime_sync_routing_offline_retry_revocation_and_no_cloud_egress() {
    let trap = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cloud = format!("http://{}", trap.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let trap_task = tokio::spawn(async move {
        while let Ok((_, _)) = trap.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let c_dir = tempfile::tempdir().unwrap();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let url = format!("http://127.0.0.1:{port}");
    std::fs::write(a_dir.path().join("device-id"), "server-a").unwrap();
    let mut private = PrivateConfig::create(
        a_dir.path(),
        "Private fixture",
        "server-a",
        "Server A",
        NodeRole::Server,
        &url,
    )
    .unwrap();
    private.listen_port = port;
    private.save(a_dir.path()).unwrap();
    let a_runs = Arc::new(AtomicUsize::new(0));
    let b_runs = Arc::new(AtomicUsize::new(0));
    let c_runs = Arc::new(AtomicUsize::new(0));
    let a = start(a_dir.path(), &cloud, a_runs.clone()).await;
    enroll(&a, b_dir.path(), "server-b", NodeRole::Server, &url).await;
    enroll(&a, c_dir.path(), "client-c", NodeRole::Client, &url).await;
    let b = start(b_dir.path(), &cloud, b_runs.clone()).await;
    let c = start(c_dir.path(), &cloud, c_runs.clone()).await;
    wait(
        || {
            [&a, &b, &c]
                .iter()
                .all(|r| r.core().workspace.read_devices().unwrap().len() == 3)
        },
        "three device rows",
    )
    .await;
    let devices = c.core().workspace.read_devices().unwrap();
    assert_eq!(
        devices
            .iter()
            .find(|d| d.id == "client-c")
            .unwrap()
            .role
            .as_deref(),
        Some("client")
    );
    for (chat, host) in [("chat-a", "server-a"), ("chat-b", "server-b")] {
        c.core()
            .workspace
            .create_chat(chat, None, Some(host), None, Some("/tmp".into()))
            .unwrap();
        c.core()
            .workspace
            .rename_chat(chat, "Private fixture")
            .unwrap();
    }
    wait(
        || {
            a.core().workspace.chat("chat-b").unwrap().is_some()
                && b.core().workspace.chat("chat-a").unwrap().is_some()
        },
        "chat metadata",
    )
    .await;
    let command = run(&c, "chat-a", "one");
    wait(
        || a_runs.load(Ordering::SeqCst) == 1 && !a.core().sessions.any_active(),
        "run on server A",
    )
    .await;
    assert_eq!(b_runs.load(Ordering::SeqCst), 0);
    assert_eq!(c_runs.load(Ordering::SeqCst), 0);
    wait(
        || {
            c.core()
                .doc_host
                .open("chat-a")
                .unwrap()
                .doc()
                .read_entries()
                .unwrap()
                .iter()
                .any(|e| {
                    serde_json::to_string(e)
                        .unwrap()
                        .contains("Private reply: one")
                })
        },
        "live transcript at client",
    )
    .await;
    let peer = c.core().dial_device("server-a").await.unwrap();
    assert!(
        peer.call(methods::PRIVATE_STATUS, json!({})).await.is_err(),
        "relay must not expose pairing administration"
    );
    let entry = c
        .core()
        .doc_host
        .open("chat-a")
        .unwrap()
        .doc()
        .read_commands()
        .unwrap()
        .into_iter()
        .find(|e| e.id == command)
        .unwrap();
    peer.call(
        methods::RELAY_COMMAND,
        json!({"chatId":"chat-a","entry":entry}),
    )
    .await
    .unwrap();
    assert_eq!(
        a_runs.load(Ordering::SeqCst),
        1,
        "same command never executes twice"
    );
    let client_control = memory_client(c.core().rpc_service());
    client_control
        .call(
            methods::SET_PRIVATE_ACCESS_ENABLED,
            json!({"enabled":false}),
        )
        .await
        .unwrap();
    wait(|| !c.core().workspace.connected(), "client access disabled").await;
    client_control
        .call(methods::SET_PRIVATE_ACCESS_ENABLED, json!({"enabled":true}))
        .await
        .unwrap();
    wait(
        || c.core().workspace.connected(),
        "client access enabled again",
    )
    .await;
    run(&c, "chat-a", "after reconnect");
    wait(
        || a_runs.load(Ordering::SeqCst) == 2 && !a.core().sessions.any_active(),
        "run after re-enabling private access",
    )
    .await;
    wait(
        || {
            c.core()
                .doc_host
                .open("chat-a")
                .unwrap()
                .doc()
                .read_entries()
                .unwrap()
                .iter()
                .any(|entry| {
                    serde_json::to_string(entry)
                        .unwrap()
                        .contains("Private reply: after reconnect")
                })
        },
        "open transcript after re-enabling access",
    )
    .await;
    let peer = c.core().dial_device("server-a").await.unwrap();
    if let SessionCommandPayload::Run { request, .. } = entry.payload {
        assert!(
            c.core()
                .sessions
                .dispatch("cannot-run-here", HarnessId::Mock, request, None)
                .await
                .is_err(),
            "client cannot dispatch through the engine either"
        );
    }
    b.shutdown().await;
    drop(b);
    run(&c, "chat-b", "while offline");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(b_runs.load(Ordering::SeqCst), 0);
    let b = start(b_dir.path(), &cloud, b_runs.clone()).await;
    wait(
        || b_runs.load(Ordering::SeqCst) == 1 && !b.core().sessions.any_active(),
        "offline command after server restart",
    )
    .await;
    assert_eq!(a_runs.load(Ordering::SeqCst), 2);
    assert_eq!(c_runs.load(Ordering::SeqCst), 0);
    let admin = memory_client(a.core().rpc_service());
    admin
        .call(methods::REVOKE_PRIVATE_NODE, json!({"deviceId":"client-c"}))
        .await
        .unwrap();
    wait(
        || !c.core().workspace.connected(),
        "revoked client disconnected",
    )
    .await;
    assert!(peer.call(methods::LIST_HARNESSES, json!({})).await.is_err());
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "private mode reached the cloud endpoint"
    );
    c.shutdown().await;
    b.shutdown().await;
    a.shutdown().await;
    trap_task.abort();
}

#[tokio::test]
async fn malformed_private_config_never_falls_back_to_cloud_or_local() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(PrivateConfig::path(dir.path()), "not json").unwrap();
    let config = engine_config(dir.path(), "https://edge.zeron.sh");
    let auth = Engine::build_auth(&config).await;
    assert_eq!(
        Engine::initial_workspace_scope(&auth),
        WorkspaceScope::Private
    );
    assert!(Engine::resolve_profile(&config, &auth, WorkspaceScope::Private).is_err());
    assert_eq!(
        auth.access_token().await,
        Err(zeron_rpc::TokenError::SignedOut)
    );
}

#[tokio::test]
async fn cancelled_pairing_resumes_the_original_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub_url = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(tokio::sync::Notify::new());
    let signal = accepted.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        signal.notify_one();
        std::future::pending::<()>().await;
        drop(stream);
    });
    let mut config = engine_config(dir.path(), "http://127.0.0.1:1");
    config.edge_token = None;
    let auth = Engine::build_auth(&config).await;
    let profile = Engine::resolve_profile(&config, &auth, WorkspaceScope::Local)
        .unwrap()
        .unwrap();
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    runtime
        .core()
        .registry
        .register(Arc::new(CountingHarness(runs.clone())));
    let rpc = runtime.core().rpc_service();
    let joining = tokio::spawn(async move {
        rpc.handle(
            methods::JOIN_PRIVATE_WORKSPACE,
            json!({"hubUrl":hub_url,"code":"123456","name":"Fixture","role":"server"}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), accepted.notified())
        .await
        .unwrap();
    assert!(
        runtime
            .core()
            .sessions
            .dispatch(
                "frozen",
                HarnessId::Mock,
                request("during transition"),
                None
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .core()
            .sessions
            .steer("frozen", "during transition", None)
            .await
            .is_err()
    );
    assert!(runtime.core().terminals.open("/tmp", 80, 24).is_err());
    joining.abort();
    assert!(joining.await.is_err());
    runtime
        .core()
        .sessions
        .dispatch(
            "resumed",
            HarnessId::Mock,
            request("after cancellation"),
            None,
        )
        .await
        .unwrap();
    wait(
        || runs.load(Ordering::SeqCst) == 1 && !runtime.core().sessions.any_active(),
        "execution resumed after cancelled pairing",
    )
    .await;
    assert!(PrivateConfig::load(dir.path()).unwrap().is_none());
    runtime.shutdown().await;
    server.abort();
}
