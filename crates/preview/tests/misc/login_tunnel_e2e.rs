//! A remote sign-in end to end: two preview services paired through an
//! in-process stand-in for the edge's preview room, the requester forwarding
//! its loopback port to the login's callback on the running device. The room
//! is full: it admits only devices joining for a sign-in, as the edge does
//! past its device limit.
use futures::{SinkExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
};
use tokio_tungstenite::tungstenite::Message;
use zeron_preview::{PreviewService, Scope, signaling};

struct Token;
#[async_trait::async_trait]
impl signaling::TokenSource for Token {
    async fn token(&self) -> anyhow::Result<String> {
        Ok("token".into())
    }
}

type Sockets = Arc<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<String>>>>;

/// Relays `signal`s between connected devices, like `PreviewRoom` does, and
/// refuses every device that doesn't join for a sign-in (a full room).
async fn coordinator() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let sockets: Sockets = Default::default();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let sockets = sockets.clone();
            tokio::spawn(async move {
                use tokio_tungstenite::tungstenite::handshake::server::{
                    ErrorResponse, Request, Response,
                };
                let mut device = String::new();
                let admit = |request: &Request, response: Response| {
                    let query = request.uri().query().unwrap_or_default();
                    let mut sign_in = false;
                    for (key, value) in query.split('&').filter_map(|p| p.split_once('=')) {
                        match key {
                            "device" => device = value.to_owned(),
                            "signIn" => sign_in = value == "1",
                            _ => {}
                        }
                    }
                    if !sign_in {
                        let mut refused = ErrorResponse::new(Some("Device limit reached".into()));
                        *refused.status_mut() =
                            tokio_tungstenite::tungstenite::http::StatusCode::TOO_MANY_REQUESTS;
                        return Err(refused);
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
                        if write.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = read.next().await {
                    let Message::Text(text) = message else {
                        continue;
                    };
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
    let service = PreviewService::new(
        dir.join(format!("{device}.json")),
        device.into(),
        device.into(),
    )
    .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_sign_in_reaches_the_running_devices_callback() {
    let dir = tempfile::tempdir().unwrap();
    let edge = coordinator().await;
    // "device-a" < "device-b": exercise both pairing directions.
    for (host_id, requester_id) in [("device-b", "device-a"), ("device-c", "device-d")] {
        let host = service(dir.path(), host_id, &edge).await;
        let requester = service(dir.path(), requester_id, &edge).await;
        // Both are refused, and keep being refused, until a sign-in starts.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The CLI's loopback callback on the running device.
        let callback = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback_port = callback.local_addr().unwrap().port();
        let cli = tokio::spawn(async move {
            let (mut socket, _) = callback.accept().await.unwrap();
            let mut request = vec![0u8; 256];
            let n = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        host.callback_routes().register(
            "login-1",
            callback_port,
            requester_id,
            Duration::from_secs(60),
        );

        // The requester binds a free port of its own (in production the same
        // number; both run on this machine here).
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_port = probe.local_addr().unwrap().port();
        drop(probe);
        requester
            .open_login_tunnel("login-1", host_id, local_port, Duration::from_secs(60))
            .await
            .unwrap();

        let mut browser = TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
        browser
            .write_all(b"GET /callback?code=x HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(40), browser.read_to_string(&mut reply))
            .await
            .expect("callback reply in time")
            .unwrap();
        assert!(
            reply.ends_with("ok"),
            "{host_id}<-{requester_id}: {reply:?}"
        );
        assert!(cli.await.unwrap().starts_with("GET /callback?code=x"));
        requester.close_login_tunnel("login-1");
        host.shutdown().await;
        requester.shutdown().await;
    }
}
