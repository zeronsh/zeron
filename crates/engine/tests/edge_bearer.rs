//! `EdgeBearer`: a viewer on the engine's device (the Android app) takes the
//! engine's edge bearer over the token-gated IPC instead of refreshing a
//! WorkOS session of its own. WorkOS refresh tokens rotate, so the engine must
//! stay the only refresher: every refresh presents the latest refresh token.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_client::engine::{EngineLink, REASK_BEFORE_EXPIRY_MS};
use zeron_engine::{Auth, AuthConfig, EdgeBearerRpc, EdgeBearerSource};
use zeron_rpc::{RpcError, RpcReply, RpcService};

const IPC_TOKEN: &str = "ipc-0123456789abcdef0123456789abcdef";

fn jwt(ttl_secs: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let b64 = |v: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
    };
    format!(
        "{}.{}.sig",
        b64(serde_json::json!({ "alg": "none" })),
        b64(serde_json::json!({ "iat": now, "exp": now + ttl_secs, "org_id": "org_1" })),
    )
}

/// `/auth/refresh`: accepts only the latest refresh token `r{n}`, rotates it
/// to `r{n+1}`. The first access token lives 25s — inside the engine's 30s
/// refresh slack, so the next ask refreshes — the rest an hour.
async fn mock_edge(presented: Arc<Mutex<Vec<String>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let presented = presented.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let mut len = 0;
                // Headers, then the (small) JSON body.
                loop {
                    let n = socket.read(&mut buf[len..]).await.unwrap_or(0);
                    len += n;
                    let text = String::from_utf8_lossy(&buf[..len]);
                    if n == 0 || (text.contains("\r\n\r\n") && text.trim_end().ends_with('}')) {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf[..len]).to_string();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("{}");
                let token = serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v["refreshToken"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                let (status, reply) = {
                    let mut presented = presented.lock().unwrap();
                    let expected = format!("r{}", presented.len());
                    presented.push(token.clone());
                    if text.starts_with("POST /auth/refresh") && token == expected {
                        let ttl = if presented.len() == 1 { 25 } else { 3600 };
                        let next = format!("r{}", presented.len());
                        (
                            200,
                            serde_json::json!({ "accessToken": jwt(ttl), "refreshToken": next })
                                .to_string(),
                        )
                    } else {
                        (401, r#"{"error":"invalid_grant"}"#.to_owned())
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    url
}

/// Everything but `EdgeBearer` reaches the wrapped service.
struct Inner;

#[async_trait::async_trait]
impl RpcService for Inner {
    async fn handle(&self, method: &str, _: serde_json::Value) -> Result<RpcReply, RpcError> {
        RpcReply::value(&serde_json::json!({ "inner": method }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_viewer_gets_the_engines_bearer_and_only_the_engine_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("session.json"),
        serde_json::json!({
            "refreshToken": "r0",
            "user": { "id": "user_1", "email": "wing@example.com" },
            "orgId": "org_1",
        })
        .to_string(),
    )
    .unwrap();
    let presented = Arc::new(Mutex::new(Vec::new()));
    let edge = mock_edge(presented.clone()).await;
    let mut config = AuthConfig::new(&edge, dir.path());
    config.workos_client_id = Some("client_test".into());
    let auth = Auth::new(config);
    assert!(auth.state().is_signed_in());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ipc = format!("ws://{}", listener.local_addr().unwrap());
    let service = EdgeBearerRpc::new(
        Arc::new(Inner),
        Some(EdgeBearerSource::new(
            &edge,
            "user_1",
            "org_1",
            auth.clone(),
        )),
    );
    tokio::spawn(zeron_rpc::serve_ws_listener_with_token(
        listener,
        Arc::new(service),
        Some(IPC_TOKEN.into()),
    ));

    // The gate: no bearer for a process without the IPC token.
    assert!(EngineLink::new(&ipc, None).edge_bearer().await.is_err());
    let link = EngineLink::new(&ipc, Some(IPC_TOKEN.into()));
    // Other methods pass through to the engine's own service.
    let passthrough = link
        .call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(passthrough["inner"], zeron_rpc::methods::ENGINE_INFO);

    // First ask: the engine refreshes (nothing cached) and answers with the
    // edge, the identity and the bearer's expiry.
    let first = link.edge_bearer().await.unwrap();
    assert_eq!(first.edge_url, edge);
    assert_eq!(
        (first.user_id.as_str(), first.org_id.as_str()),
        ("user_1", "org_1")
    );
    assert!(first.bearer.is_some());
    let expires = first.expires_at_ms.expect("a WorkOS bearer expires");
    let now = chrono::Utc::now().timestamp_millis();
    assert!(
        (20_000..=26_000).contains(&(expires - now)),
        "{}",
        expires - now
    );

    // A viewer re-asks at `expires - REASK_BEFORE_EXPIRY_MS`: inside the
    // engine's 30s slack, so the engine rotates instead of re-serving it.
    assert!(expires - REASK_BEFORE_EXPIRY_MS - now < 30_000);
    let second = link.edge_bearer().await.unwrap();
    assert_ne!(second.bearer, first.bearer);

    // Many viewers' dials at once: served from the engine's cache.
    let asks = (0..8).map(|_| {
        let link = link.clone();
        tokio::spawn(async move { link.edge_bearer().await.unwrap().bearer })
    });
    for ask in futures::future::join_all(asks).await {
        assert_eq!(ask.unwrap(), second.bearer);
    }
    // Exactly two refreshes, each with the latest rotated refresh token.
    assert_eq!(*presented.lock().unwrap(), vec!["r0", "r1"]);

    // Signed out: a terminal answer, not an error to retry.
    auth.sign_out();
    let gone = link.edge_bearer().await.unwrap();
    assert!(gone.signed_out && gone.bearer.is_none());

    // No edge (a local-only runtime): an error, never a bearer.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(zeron_rpc::serve_ws_listener_with_token(
        listener,
        Arc::new(EdgeBearerRpc::new(Arc::new(Inner), None)),
        Some(IPC_TOKEN.into()),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        EngineLink::new(&local, Some(IPC_TOKEN.into()))
            .edge_bearer()
            .await
            .is_err()
    );
}
