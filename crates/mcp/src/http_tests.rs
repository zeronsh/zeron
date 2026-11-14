use super::*;

use crate::zeron::Zeron;
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client, methods};

/// What the fake engine saw: `AwaitBackground`'s chat per call, and whether
/// a blocked call was dropped (cancelled).
#[derive(Default)]
struct Engine {
    awaited: Mutex<Vec<String>>,
    dropped: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::AWAIT_BACKGROUND => {
                let chat = params["chatId"].as_str().unwrap_or_default().to_owned();
                lock(&self.awaited).push(chat);
                if params["taskIds"][0] == "block" {
                    let _signal = DropSignal(lock(&self.dropped).take());
                    std::future::pending::<()>().await;
                }
                Ok(RpcReply::Value(json!({ "matched": params["taskIds"] })))
            }
            methods::LIST_PLUGIN_TOOLS => Ok(RpcReply::Value(json!({ "device": { "id": "d" }, "tools": [] }))),
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

fn origin(chat: &str) -> Origin {
    Origin {
        chat_id: Some(chat.into()),
        device_id: Some("d".into()),
        side_chats_only: false,
        waits_on_background: true,
    }
}

async fn server(engine: Arc<Engine>) -> McpHttpServer {
    let factory: ToolsFactory = Arc::new(move |origin: &Origin| {
        let service = zeron_rpc::agent_service(
            engine.clone(),
            origin.chat_id.clone().unwrap_or_default(),
        );
        Arc::new(Tools::new(Arc::new(Zeron::with_client(
            memory_client(service),
            origin.clone(),
        ))))
    });
    McpHttpServer::bind(factory).await.unwrap()
}

/// One raw HTTP/1.1 exchange: (status, head, body as sent — chunked or not).
async fn exchange(port: u16, method: &str, headers: &[(&str, String)], body: &str) -> (u16, String, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut request = format!("{method} /mcp HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut raw))
        .await
        .expect("answered in time")
        .unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head.to_ascii_lowercase(), body.to_owned())
}

fn bearer(token: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {token}"))
}

fn accept() -> (&'static str, String) {
    ("Accept", "application/json, text/event-stream".into())
}

fn legacy_call(id: u64, tool: &str, args: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": tool, "arguments": args } })
        .to_string()
}

fn modern(id: u64, method: &str, mut params: Value) -> String {
    params["_meta"] = json!({
        META_VERSION: "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

fn modern_headers(method: &str, name: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("MCP-Protocol-Version", "2026-07-28".to_owned()),
        ("Mcp-Method", method.to_owned()),
    ];
    if let Some(name) = name {
        headers.push(("Mcp-Name", name.to_owned()));
    }
    headers
}

#[tokio::test]
async fn a_request_without_the_run_s_token_is_refused() {
    let server = server(Arc::default()).await;
    let grant = server.grant(origin("chat-a")).unwrap();
    let port = server.port();
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }).to_string();

    let (status, head, _) = exchange(port, "POST", &[accept()], &init).await;
    assert_eq!(status, 401, "no token");
    assert!(head.contains("www-authenticate: bearer"));
    let (status, ..) = exchange(port, "POST", &[accept(), bearer("not-the-token")], &init).await;
    assert_eq!(status, 401, "wrong token");
    let (status, ..) = exchange(port, "POST", &[accept(), ("Authorization", grant.token().into())], &init).await;
    assert_eq!(status, 401, "token without the Bearer scheme");

    // A browser page (Origin) and a rebound name (Host) never get as far as
    // the token check.
    let with_origin = [accept(), bearer(grant.token()), ("Origin", "https://evil.example".into())];
    assert_eq!(exchange(port, "POST", &with_origin, &init).await.0, 403);
    let rebound = [accept(), bearer(grant.token()), ("Host", format!("evil.example:{port}"))];
    assert_eq!(exchange(port, "POST", &rebound, &init).await.0, 403);

    let (status, head, body) = exchange(port, "POST", &[accept(), bearer(grant.token())], &init).await;
    assert_eq!(status, 200);
    assert!(body.contains("\"protocolVersion\""), "{body}");
    assert!(!head.contains("mcp-session-id"), "stateless: no session is minted");

    // Ended runs' tokens stop working at once.
    let token = grant.token().to_owned();
    drop(grant);
    assert_eq!(server.live_grants(), 0);
    assert_eq!(exchange(port, "POST", &[accept(), bearer(&token)], &init).await.0, 401);
}

#[tokio::test]
async fn get_and_delete_are_not_served() {
    let server = server(Arc::default()).await;
    let grant = server.grant(origin("chat-a")).unwrap();
    for method in ["GET", "DELETE"] {
        let (status, head, _) = exchange(server.port(), method, &[bearer(grant.token())], "").await;
        assert_eq!(status, 405, "{method}");
        assert!(head.contains("allow: post"));
    }
}

#[tokio::test]
async fn one_run_s_token_acts_only_as_its_own_chat() {
    let engine = Arc::<Engine>::default();
    let server = server(engine.clone()).await;
    let a = server.grant(origin("chat-a")).unwrap();
    let b = server.grant(origin("chat-b")).unwrap();
    let call = legacy_call(1, "wait_on_background", json!({ "task_ids": ["t1"] }));
    let (status, _, body) = exchange(server.port(), "POST", &[accept(), bearer(a.token())], &call).await;
    assert_eq!(status, 200);
    assert!(body.contains("waiting on that work"), "{body}");
    let (status, ..) = exchange(server.port(), "POST", &[accept(), bearer(b.token())], &call).await;
    assert_eq!(status, 200);
    assert_eq!(*lock(&engine.awaited), ["chat-a", "chat-b"], "each token speaks for its own run's chat");

    // And underneath the tools: the engine connection a token's calls use
    // refuses what would act on another chat, whatever the params say.
    let client = memory_client(zeron_rpc::agent_service(engine.clone(), "chat-a".into()));
    let refused = client
        .call(methods::AWAIT_BACKGROUND, json!({ "chatId": "chat-b", "taskIds": ["t"], "waiting": true }))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("calling chat only"), "{refused}");
    assert_eq!(lock(&engine.awaited).len(), 2, "never reached the engine");
    // Methods outside the agent scope stay shut, as on the agent IPC token.
    let refused = client.call(methods::STOP_ENGINE, json!({})).await.unwrap_err();
    assert!(refused.to_string().contains("isn't open to agents"), "{refused}");
}

#[tokio::test]
async fn modern_requests_are_stateless_and_header_checked() {
    let server = server(Arc::default()).await;
    let grant = server.grant(origin("chat-a")).unwrap();
    let port = server.port();
    let auth = || vec![accept(), bearer(grant.token())];

    // No handshake: discovery and a call each stand alone.
    let mut headers = auth();
    headers.extend(modern_headers("server/discover", None));
    let (status, _, body) = exchange(port, "POST", &headers, &modern(1, "server/discover", json!({}))).await;
    assert_eq!(status, 200, "{body}");
    let found: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(found["result"]["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(found["result"]["resultType"], "complete");
    assert!(found["result"]["instructions"].as_str().unwrap().contains("wait_on_background"));

    let mut headers = auth();
    headers.extend(modern_headers("tools/list", None));
    let (status, _, body) = exchange(port, "POST", &headers, &modern(2, "tools/list", json!({}))).await;
    assert_eq!(status, 200);
    let listed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(listed["result"]["resultType"], "complete");
    assert_eq!(listed["result"]["cacheScope"], "private");
    assert!(listed["result"]["ttlMs"].is_u64());
    assert_eq!(listed["result"]["_meta"][META_SERVER_INFO]["name"], "zeron");

    // Headers must match the body.
    let call = modern(3, "tools/call", json!({ "name": "wait_on_background", "arguments": { "task_ids": ["t"] } }));
    for (headers, why) in [
        (modern_headers("tools/call", None), "Mcp-Name missing"),
        (modern_headers("tools/call", Some("whoami")), "Mcp-Name differs"),
        (modern_headers("tools/list", Some("wait_on_background")), "Mcp-Method differs"),
        (vec![("Mcp-Method", "tools/call".into()), ("Mcp-Name", "wait_on_background".into())], "no version header"),
    ] {
        let mut all = auth();
        all.extend(headers);
        let (status, _, body) = exchange(port, "POST", &all, &call).await;
        assert_eq!(status, 400, "{why}");
        assert!(body.contains("-32020"), "{why}: {body}");
    }
    // Base64-sentinel names decode before comparing.
    let mut all = auth();
    all.extend(modern_headers("tools/call", Some("=?base64?d2FpdF9vbl9iYWNrZ3JvdW5k?=")));
    let (status, _, body) = exchange(port, "POST", &all, &call).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"resultType\":\"complete\""), "{body}");

    // An unknown revision lists the ones served.
    let mut old = modern(4, "tools/list", json!({}));
    old = old.replace("2026-07-28", "2099-01-01");
    let mut headers = auth();
    headers.extend([("MCP-Protocol-Version", "2099-01-01".into()), ("Mcp-Method", "tools/list".into())]);
    let (status, _, body) = exchange(port, "POST", &headers, &old).await;
    assert_eq!(status, 400);
    let refused: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(refused["error"]["code"], UNSUPPORTED_VERSION);
    assert!(refused["error"]["data"]["supported"].as_array().unwrap().contains(&json!("2026-07-28")));
}

/// Until no call is in flight (the cancelled one was stopped).
async fn gone(server: &McpHttpServer) {
    for _ in 0..500 {
        if server.calls_in_flight() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the call is still in flight");
}

#[tokio::test]
async fn a_closed_stream_cancels_its_call() {
    let engine = Arc::<Engine>::default();
    let server = server(engine.clone()).await;
    let grant = server.grant(origin("chat-a")).unwrap();
    let call = legacy_call(7, "wait_on_background", json!({ "task_ids": ["block"] }));
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port())).await.unwrap();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{call}",
        server.port(),
        grant.token(),
        call.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = [0u8; 256];
    let read = stream.read(&mut head).await.unwrap();
    let head = String::from_utf8_lossy(&head[..read]).to_ascii_lowercase();
    assert!(head.contains("text/event-stream"), "{head}");
    // Wait for the call to reach the engine, then hang up.
    for _ in 0..100 {
        if !lock(&engine.awaited).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(server.calls_in_flight(), 1);
    drop(stream);
    gone(&server).await;
}

#[tokio::test]
async fn a_2025_cancel_notification_stops_its_call() {
    let engine = Arc::<Engine>::default();
    let server = server(engine.clone()).await;
    let grant = server.grant(origin("chat-a")).unwrap();
    let port = server.port();
    let token = grant.token().to_owned();
    let call = legacy_call(9, "wait_on_background", json!({ "task_ids": ["block"] }));
    let pending = tokio::spawn(async move { exchange(port, "POST", &[accept(), bearer(&token)], &call).await });
    for _ in 0..100 {
        if !lock(&engine.awaited).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cancel = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 9 } }).to_string();
    let (status, ..) = exchange(port, "POST", &[accept(), bearer(grant.token())], &cancel).await;
    assert_eq!(status, 202);
    gone(&server).await;
    // The call's own stream ends without an answer.
    let (_, _, body) = pending.await.unwrap();
    assert!(!body.contains("\"result\""), "{body}");
}
