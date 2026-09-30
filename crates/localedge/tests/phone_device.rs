//! The phone as one regular device (docs/android.md): its viewer shares the
//! phone engine's device id and takes the engine's edge bearer over IPC
//! (`Credentials::Engine`), so the phone is a single device beside a desktop
//! engine on the same edge — each can run sessions on the other.
//!
//! One test function on purpose: `ZERON_IPC_TOKEN` is process environment,
//! set before any other thread exists.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_client::engine::EngineLink;
use zeron_client::events::NullListener;
use zeron_client::{
    Client, ClientConfig, Credentials, MessagePart, MessageRole, NewSession, SendRequest,
    SessionTarget,
};
use zeron_engine::{EdgeBearerRpc, Engine, EngineConfig, EngineRuntime, HarnessId};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};
use zeron_proto::{ChatConfig, SandboxLevel};

const TOKEN: &str = "dev-0123456789abcdef0123456789abcdef";
const IPC_TOKEN: &str = "ipc-fedcba9876543210fedcba9876543210";
const REPLY_MARK: &str = "Streaming pipeline";

/// A `zeron headless` joined to the shared edge as `local`/`local` (what
/// `ZERON_EDGE_URL` + `ZERON_EDGE_TOKEN` + `ZERON_USER_ID=local` configure).
async fn start_engine(dir: &Path, edge_url: &str) -> EngineRuntime {
    let config = EngineConfig {
        data_dir: dir.to_path_buf(),
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: None,
        dev_user_id: None,
    }
    .with_local_edge(edge_url, TOKEN);
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("development profile");
    Engine::assemble_runtime(&config, auth, profile)
        .await
        .expect("engine runtime assembles")
}

/// The engine's IPC as `zeron headless` serves it with `ZERON_IPC_TOKEN`:
/// the engine's RPC plus `EdgeBearer`.
async fn serve_ipc(engine: &EngineRuntime) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let service = EdgeBearerRpc::new(engine.core().rpc_service(), engine.edge_bearer_source());
    tokio::spawn(zeron_rpc::serve_ws_listener(listener, Arc::new(service)));
    url
}

async fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn replies(client: &Client, chat_id: &str) -> usize {
    let Ok(session) = client.open_session(chat_id) else {
        return 0;
    };
    session
        .snapshot()
        .entries
        .iter()
        .filter(|entry| {
            entry.message.role == MessageRole::Assistant
                && entry.message.status == Some(zeron_client::MessageStatus::Complete)
                && entry.message.parts.iter().any(
                    |part| matches!(part, MessagePart::Text { text, .. } if text.contains(REPLY_MARK)),
                )
        })
        .count()
}

fn mock() -> ChatConfig {
    ChatConfig {
        harness: HarnessId::Mock,
        model: Some("mock-1".into()),
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    }
}

async fn run_turn(client: &Client, host: &str, prompt: &str) -> String {
    let chat_id = client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: host.to_owned(),
            },
            config: Some(mock()),
            branch: None,
            cwd: None,
            title: None,
        })
        .expect("session created");
    let session = client.open_session(&chat_id).unwrap();
    session.set_view_attached(true);
    session.send(SendRequest::text(prompt)).unwrap();
    wait_until(
        &format!("reply from {host}"),
        Duration::from_secs(60),
        || replies(client, &chat_id) >= 1,
    )
    .await;
    chat_id
}

#[test]
fn the_phone_is_one_device_beside_a_desktop() {
    // SAFETY: set before the runtime (or any other thread) starts.
    unsafe { std::env::set_var("ZERON_IPC_TOKEN", IPC_TOKEN) };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(scenario());
}

async fn scenario() {
    let root = tempfile::tempdir().unwrap();
    let edge = LocalEdge::start(LocalEdgeConfig::loopback(
        root.path().join("edge"),
        0,
        TOKEN,
    ))
    .await
    .unwrap();
    let phone_engine = start_engine(&root.path().join("phone"), &edge.url()).await;
    let desktop = start_engine(&root.path().join("desktop"), &edge.url()).await;
    let phone_id = phone_engine.core().device_id.clone();
    let desktop_id = desktop.core().device_id.clone();
    let ipc = serve_ipc(&phone_engine).await;

    // The app's boot: the engine says who the device is, where it syncs and
    // as whom; the viewer takes all three and never holds a secret itself.
    let link = EngineLink::new(&ipc, Some(IPC_TOKEN.into()));
    let identity = link.info().await.unwrap();
    assert_eq!(identity.device_id, phone_id);
    let edge_info = link.edge_bearer().await.unwrap();
    assert_eq!(edge_info.edge_url, edge.url());
    assert_eq!(edge_info.bearer.as_deref(), Some(TOKEN));
    let mut config = ClientConfig::new(&edge_info.edge_url, root.path().join("viewer"));
    config.device_id = identity.device_id.clone();
    config.device_name = "Pixel".into();
    config.platform = "android".into();
    let viewer = Client::new(
        config,
        Credentials::Engine {
            ipc_url: ipc.clone(),
            ipc_token: Some(IPC_TOKEN.into()),
            user_id: edge_info.user_id.clone(),
            org_id: edge_info.org_id.clone(),
        },
        Arc::new(NullListener),
    )
    .unwrap();

    // One device: the phone's own row is "this device" and a host; the
    // desktop is a host beside it, online through its engine's presence.
    wait_until("both hosts on the phone", Duration::from_secs(20), || {
        let ws = viewer.workspace();
        let phone = ws.device(&phone_id);
        let desktop = ws.device(&desktop_id);
        phone.is_some_and(|d| d.is_self && d.is_execution_host)
            && desktop.is_some_and(|d| d.is_execution_host && d.online && !d.is_self)
    })
    .await;
    assert_eq!(viewer.workspace().execution_devices().len(), 2);

    // The phone starts a session on the desktop, and one on itself: the
    // command the viewer stamps with the phone's id is executed by the
    // phone's engine (the same device id writes and hosts).
    let on_desktop = run_turn(&viewer, &desktop_id, "run on the desktop").await;
    let on_phone = run_turn(&viewer, &phone_id, "run on the phone").await;
    let hosted = |engine: &EngineRuntime, chat: &str| {
        engine
            .core()
            .workspace
            .read_chats()
            .unwrap()
            .iter()
            .any(|c| c.id == chat && c.device_id == engine.core().device_id)
    };
    assert!(hosted(&desktop, &on_desktop));
    assert!(hosted(&phone_engine, &on_phone));

    // The desktop sees the phone as a device that runs sessions, and its
    // chat — the desktop can start work there like on any other device.
    wait_until("the phone on the desktop", Duration::from_secs(20), || {
        desktop
            .core()
            .workspace
            .read_devices()
            .unwrap_or_default()
            .iter()
            .any(|d| d.id == phone_id && d.is_execution_host())
    })
    .await;
    wait_until(
        "the phone's chat on the desktop",
        Duration::from_secs(20),
        || {
            desktop
                .core()
                .workspace
                .read_chats()
                .unwrap()
                .iter()
                .any(|c| c.id == on_phone && c.device_id == phone_id)
        },
    )
    .await;

    viewer.shutdown();
    phone_engine.shutdown().await;
    desktop.shutdown().await;
    edge.shutdown().await;
}
