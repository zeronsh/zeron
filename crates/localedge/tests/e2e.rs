//! Milestone 1 of Android on-device mode (docs/android.md), end to end: a
//! real engine runtime (mock harness) hosting sessions through the local
//! edge, and a `zeron-client` with `Credentials::Local` driving it over the
//! same loopback edge — the production protocols with nothing in between.
//!
//! One test function on purpose: the engine reads `ZERON_DEVICE_PLATFORM`
//! and `ZERON_IPC_TOKEN` from the process environment, which is only safe to
//! set before any other thread exists.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_client::events::NullListener;
use zeron_client::{
    Client, ClientConfig, Credentials, MessagePart, MessageRole, NewSession, SendOutcome,
    SendRequest, SessionTarget,
};
use zeron_engine::{Engine, EngineConfig, EngineRuntime, HarnessId, WorkspaceScope};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};
use zeron_proto::{ChatConfig, SandboxLevel};

const TOKEN: &str = "e2e-0123456789abcdef0123456789abcdef";
const IPC_TOKEN: &str = "ipc-fedcba9876543210fedcba9876543210";
/// The mock harness's scripted reply opens with this heading.
const REPLY_MARK: &str = "Streaming pipeline";

async fn start_edge(dir: &Path, port: u16) -> LocalEdge {
    LocalEdge::start(LocalEdgeConfig::loopback(
        dir.join("local-edge"),
        port,
        TOKEN,
    ))
    .await
    .expect("local edge starts")
}

/// What `zeron headless` does with `ZERON_LOCAL_EDGE_*` set, minus the IPC
/// server: the local-edge config, Development scope, a full runtime.
async fn start_engine(dir: &Path, edge_url: &str) -> EngineRuntime {
    let config = EngineConfig {
        data_dir: dir.to_path_buf(),
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: Some("client_would_be_production".into()),
        dev_user_id: None,
    }
    .with_local_edge(edge_url, TOKEN);
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    assert_eq!(scope, WorkspaceScope::Development, "no WorkOS on the phone");
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("development profile");
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .expect("engine runtime assembles");
    // The store is keyed by the fixed local identity, never the secret.
    assert!(dir.join("orgs/local/local").is_dir());
    runtime
}

/// The Android app's viewer: `Credentials::Local` against the local edge.
fn phone(edge_url: &str, dir: &Path, device_id: &str, token: &str) -> Client {
    let mut config = ClientConfig::new(edge_url, dir);
    config.device_id = device_id.into();
    config.device_name = "Pixel".into();
    config.platform = "android".into();
    Client::new(
        config,
        Credentials::Local {
            token: token.into(),
        },
        Arc::new(NullListener),
    )
    .expect("client starts")
}

async fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Finished assistant replies in a session's transcript.
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

fn user_messages(client: &Client, chat_id: &str) -> Vec<String> {
    let Ok(session) = client.open_session(chat_id) else {
        return Vec::new();
    };
    let snapshot = session.snapshot();
    snapshot.entries[..snapshot.transcript_len]
        .iter()
        .filter(|entry| entry.message.role == MessageRole::User)
        .flat_map(|entry| {
            entry.message.parts.iter().filter_map(|part| match part {
                MessagePart::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
        })
        .collect()
}

#[test]
fn phone_engine_serves_a_local_client_through_the_local_edge() {
    // SAFETY: set before the runtime (or any other thread) starts.
    unsafe {
        std::env::set_var("ZERON_DEVICE_PLATFORM", "android");
        std::env::set_var("ZERON_IPC_TOKEN", IPC_TOKEN);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(scenario());
}

async fn scenario() {
    let root = tempfile::tempdir().unwrap();
    let engine_dir = root.path().join("engine");
    let edge = start_edge(&engine_dir, 0).await;
    let port = edge.addr().port();
    let edge_url = edge.url();
    let engine = start_engine(&engine_dir, &edge_url).await;
    let engine_id = engine.core().device_id.clone();

    // The engine's IPC, served as `zeron headless` serves it: gated on
    // ZERON_IPC_TOKEN, which `connect_ws` (zeron mcp / sync / the desktop
    // viewport) presents from the same environment.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ipc_url = format!("ws://{}", listener.local_addr().unwrap());
    let ipc = tokio::spawn(zeron_rpc::serve_ws_listener(
        listener,
        engine.core().rpc_service(),
    ));
    for token in [None, Some("ipc-0000000000000000000000000000000")] {
        assert!(
            zeron_rpc::connect_ws_with_token(&ipc_url, token)
                .await
                .is_err(),
            "IPC must refuse {token:?}"
        );
    }
    let rpc = zeron_rpc::connect_ws(&ipc_url)
        .await
        .expect("connect_ws presents ZERON_IPC_TOKEN");
    let info: zeron_engine::EngineInfo = serde_json::from_value(
        rpc.call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(info.device_id, engine_id);
    assert_eq!(info.workspace_scope, WorkspaceScope::Development);

    let client = phone(
        &edge_url,
        &root.path().join("phone"),
        "android-viewer",
        TOKEN,
    );

    // The engine's own registry row reaches the phone, platform `android`,
    // and — because an engine wrote it, advertising capabilities — it is an
    // execution host, online through presence.
    wait_until(
        "engine device on the phone",
        Duration::from_secs(20),
        || {
            client
                .workspace()
                .device(&engine_id)
                .is_some_and(|d| d.is_execution_host && d.online)
        },
    )
    .await;
    let device = client.workspace().device(&engine_id).cloned().unwrap();
    assert_eq!(device.platform, "android");
    assert!(
        client
            .workspace()
            .execution_devices()
            .iter()
            .any(|d| d.id == engine_id)
    );

    // Create a session on the phone's engine and run a turn.
    let chat_id = client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: engine_id.clone(),
            },
            config: Some(ChatConfig {
                harness: HarnessId::Mock,
                model: Some("mock-1".into()),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            branch: None,
            cwd: None,
            title: None,
        })
        .expect("the phone's engine can host sessions");
    let session = client.open_session(&chat_id).unwrap();
    session.set_view_attached(true);
    let outcome = session.send(SendRequest::text("hello, engine")).unwrap();
    assert!(
        matches!(outcome, SendOutcome::Started { .. }),
        "{outcome:?}"
    );
    wait_until("assistant reply", Duration::from_secs(60), || {
        replies(&client, &chat_id) >= 1
    })
    .await;
    assert_eq!(user_messages(&client, &chat_id), vec!["hello, engine"]);
    // A peer without the secret gets nothing: its dials are refused, so its
    // workspace stays empty, and the chat's HTTP rows answer 401.
    let intruder = phone(
        &edge_url,
        &root.path().join("intruder"),
        "android-intruder",
        "not-the-token-0123456789abcdef",
    );
    let rows_url = format!("{edge_url}/chat2/{chat_id}/rows?after=0");
    let http = reqwest::Client::new();
    for bearer in [None, Some("not-the-token-0123456789abcdef")] {
        let mut request = http.get(&rows_url);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    let authed = http.get(&rows_url).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(authed.status(), 200);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seen = intruder.workspace();
    assert!(seen.devices.is_empty() && seen.sessions.is_empty());
    intruder.shutdown();

    // The engine's registry saw the chat it now hosts.
    assert!(
        engine
            .core()
            .workspace
            .read_chats()
            .unwrap()
            .iter()
            .any(|c| c.id == chat_id && c.device_id == engine_id)
    );

    // Restart the edge on the same port. Engine and client redial on their
    // own; a second turn round-trips, and a fresh device catches up on the
    // whole history from the edge's disk.
    edge.shutdown().await;
    let edge = start_edge(&engine_dir, port).await;
    wait_until(
        "idle before the second turn",
        Duration::from_secs(30),
        || !session.snapshot().working,
    )
    .await;
    session.send(SendRequest::text("still there?")).unwrap();
    wait_until(
        "reply after the edge restart",
        Duration::from_secs(60),
        || replies(&client, &chat_id) >= 2,
    )
    .await;

    let fresh = phone(
        &edge.url(),
        &root.path().join("second-phone"),
        "android-tablet",
        TOKEN,
    );
    wait_until(
        "fresh device sees the chat",
        Duration::from_secs(20),
        || fresh.workspace().session(&chat_id).is_some(),
    )
    .await;
    fresh
        .open_session(&chat_id)
        .unwrap()
        .set_view_attached(true);
    wait_until("fresh device catches up", Duration::from_secs(30), || {
        replies(&fresh, &chat_id) >= 2
    })
    .await;
    assert_eq!(
        user_messages(&fresh, &chat_id),
        vec!["hello, engine", "still there?"]
    );

    fresh.shutdown();
    client.shutdown();
    ipc.abort();
    engine.shutdown().await;
    edge.shutdown().await;
}
