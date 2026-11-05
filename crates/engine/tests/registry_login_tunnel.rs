//! Signing in to an agent from the ACP registry from another device, end to
//! end: the agent (tests/fixtures/fake-registry-login.py, added on device B
//! as the registry would add it) runs its own sign-in on B, its browser
//! captured; B's login reports the page and publishes the page's loopback
//! callback for device A alone; A forwards the same port over the P2P login
//! tunnel (two preview services, paired through a stand-in for the edge's
//! room); A's "browser" lands the redirect, which reaches the agent on B,
//! and the sign-in finishes. Unix: the recording browser is a shell script.

#![cfg(unix)]

use futures::{SinkExt, StreamExt};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
};
use tokio_tungstenite::tungstenite::Message;
use zeron_engine::{AgentAccounts, AgentAccountsConfig};
use zeron_preview::{PreviewService, Scope, signaling};
use zeron_proto::{AgentLoginStatus, HarnessId, RegistryAgent};

struct Token;
#[async_trait::async_trait]
impl signaling::TokenSource for Token {
    async fn token(&self) -> anyhow::Result<String> {
        Ok("token".into())
    }
}

type Sockets = Arc<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<String>>>>;

/// Relays `signal`s between connected devices, as the edge's preview room.
async fn coordinator() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let sockets: Sockets = Default::default();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let sockets = sockets.clone();
            tokio::spawn(async move {
                use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
                let mut device = String::new();
                let admit = |request: &Request, response: Response| {
                    let query = request.uri().query().unwrap_or_default();
                    for (key, value) in query.split('&').filter_map(|p| p.split_once('=')) {
                        if key == "device" {
                            device = value.to_owned();
                        }
                    }
                    Ok(response)
                };
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(socket, admit).await else {
                    return;
                };
                let (mut write, mut read) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                sockets.lock().await.insert(device.clone(), tx);
                tokio::spawn(async move {
                    while let Some(text) = rx.recv().await {
                        if write.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = read.next().await {
                    let Message::Text(text) = message else { continue };
                    if text.as_str() == "ping" {
                        if let Some(me) = sockets.lock().await.get(&device) {
                            let _ = me.send("pong".into());
                        }
                        continue;
                    }
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "signal" {
                        let to = value["to"].as_str().unwrap().to_owned();
                        let out = serde_json::json!({"type":"signal","from":device,"signal":value["signal"]});
                        if let Some(target) = sockets.lock().await.get(&to) {
                            let _ = target.send(out.to_string());
                        }
                    }
                }
            });
        }
    });
    format!("http://{address}")
}

async fn service(dir: &std::path::Path, device: &str, edge: &str) -> PreviewService {
    let service = PreviewService::new(dir.join(format!("{device}.json")), device.into(), device.into()).unwrap();
    service
        .start(
            Arc::new(Scope::default),
            Some(signaling::Config {
                edge_url: edge.into(),
                org_id: "org".into(),
                tokens: Arc::new(Token),
            }),
        )
        .await;
    service
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-registry-login.py")
}

/// Device B's agents: the fake agent added there, as the registry would add
/// it (one folder for both tests: the environment is the process's).
fn added() -> RegistryAgent {
    static AGENTS: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = AGENTS.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("fake-login")).unwrap();
        let record = serde_json::json!({
            "id": "fake-login", "name": "Fake Login", "version": "1.0.0", "kind": "binary",
            "program": fixture(), "args": [], "env": {}, "verified": true
        });
        std::fs::write(dir.path().join("fake-login/agent.json"), record.to_string()).unwrap();
        // SAFETY: set once, before either test reads it.
        unsafe { std::env::set_var("ZERON_AGENTS_DIR", dir.path()) };
        dir
    });
    let _ = dir;
    let agent = RegistryAgent::new("fake-login").unwrap();
    assert!(zeron_harness::acp_registry::installed(agent));
    agent
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "pass"])
        .status()
        .is_ok_and(|s| s.success())
}

/// B runs the sign-in for A: its page reported, its loopback callback
/// published for A alone; then the browser's redirect lands (over the P2P
/// tunnel from A's loopback, or, `tunnel` off, straight at the port B
/// published, as B's end of the tunnel dials it) and the sign-in finishes.
async fn sign_in(tunnel: bool) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let agent = added();

    let edge = coordinator().await;
    let host = service(dir.path(), "device-b", &edge).await;
    let requester = service(dir.path(), "device-a", &edge).await;
    let accounts = AgentAccounts::with_callback_routes(
        AgentAccountsConfig::isolated(&dir.path().join("accounts")),
        host.callback_routes(),
    );

    // A asks B to sign the agent in: B runs it, the page comes with a poll.
    let start = accounts
        .start_login_for(HarnessId::Registry(agent), Some("device-a"))
        .await
        .expect("the sign-in starts");
    let mut page = None;
    for _ in 0..100 {
        let poll = accounts.poll_login(&start.login_id).await.unwrap();
        assert_eq!(poll.status, AgentLoginStatus::Pending, "{poll:?}");
        if let (Some(url), Some(port)) = (poll.url.clone(), poll.callback_port) {
            page = Some((url, port));
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (url, port) = page.expect("the sign-in's page, with its loopback callback");
    assert!(url.starts_with("https://auth.example.com/authorize"), "{url}");
    // Published for A alone.
    assert!(host.callback_routes().is_registered(&start.login_id));
    assert!(host.callback_routes().port_for(&start.login_id, "device-c").is_err());

    // A forwards a port of its own (the same number in production; both
    // run on this machine here) and its browser lands the redirect.
    let published = host.callback_routes().port_for(&start.login_id, "device-a").unwrap();
    assert_eq!(published, port);
    let local = if tunnel {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = probe.local_addr().unwrap().port();
        drop(probe);
        assert_ne!(local, port);
        requester
            .open_login_tunnel(&start.login_id, "device-b", local, Duration::from_secs(60))
            .await
            .unwrap();
        local
    } else {
        published
    };
    let mut browser = TcpStream::connect(("127.0.0.1", local)).await.unwrap();
    browser
        .write_all(b"GET /callback?code=abc&state=s HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(40), browser.read_to_string(&mut reply))
        .await
        .expect("the callback answers in time")
        .unwrap();
    assert!(reply.ends_with("signed in"), "{reply:?}");

    // The agent finished its sign-in; the login says so.
    let mut done = false;
    for _ in 0..100 {
        let poll = accounts.poll_login(&start.login_id).await.unwrap();
        match poll.status {
            AgentLoginStatus::Done => {
                done = true;
                break;
            }
            AgentLoginStatus::Error => panic!("sign-in failed: {poll:?}"),
            AgentLoginStatus::Pending => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(done, "the sign-in finishes");
    requester.close_login_tunnel(&start.login_id);
    host.shutdown().await;
    requester.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registry_agents_sign_in_publishes_its_callback_for_the_requester_and_finishes() {
    if has_python() {
        sign_in(false).await;
    }
}

/// The whole way, over WebRTC between two preview services on this machine
/// (as `zeron-preview`'s login_tunnel_e2e; a network that refuses loopback
/// WebRTC fails both alike).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registry_agents_sign_in_finishes_through_the_requesters_browser() {
    if has_python() {
        sign_in(true).await;
    }
}
