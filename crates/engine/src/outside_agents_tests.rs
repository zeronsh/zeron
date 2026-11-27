use super::*;

use async_trait::async_trait;
use tokio::sync::mpsc;
use zeron_rpc::{RpcService, memory_client};

/// The engine behind the tools: every method an outside agent's call reached.
#[derive(Default)]
struct Engine {
    called: Mutex<Vec<String>>,
}

#[async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        lock(&self.called).push(method.to_owned());
        Err(RpcError::UnknownMethod(method.into()))
    }
}

async fn endpoint(engine: Arc<Engine>) -> Arc<McpHttpServer> {
    let factory: zeron_mcp::http::ToolsFactory = Arc::new(move |origin: &zeron_mcp::Origin| {
        let service = zeron_rpc::agent_service(engine.clone(), origin.chat_id.clone().unwrap_or_default());
        Arc::new(zeron_mcp::Tools::new(Arc::new(zeron_mcp::Zeron::with_client(
            memory_client(service),
            origin.clone(),
        ))))
    });
    Arc::new(McpHttpServer::bind(factory).await.unwrap())
}

/// The next frame the room gets: (stream, payload).
async fn next(rx: &mut mpsc::Receiver<Vec<u8>>) -> (String, Value) {
    let bytes = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a frame in time")
        .expect("the relay is open");
    let (header, payload) = zeron_rpc::decode_device_frame(&bytes).unwrap();
    assert_eq!(header.k, zeron_rpc::MCP_KIND);
    (header.s, serde_json::from_slice(&payload).unwrap())
}

/// One call relayed as `grant`: its head's status, and its body.
async fn call(agents: &OutsideAgents, out: &McpOut, rx: &mut mpsc::Receiver<Vec<u8>>, grant: &str) -> (u64, String) {
    let stream = format!("call-{}", new_id());
    let body = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "list_devices", "arguments": {} } });
    let frame = json!({ "t": "call", "grant": grant, "headers": { "accept": "application/json" }, "body": body.to_string() });
    agents.frame(stream.clone(), frame.to_string().into_bytes(), out.clone());
    let (s, head) = next(rx).await;
    assert_eq!((s.as_str(), head["t"].as_str()), (stream.as_str(), Some("head")));
    let mut text = String::new();
    loop {
        let (s, frame) = next(rx).await;
        assert_eq!(s, stream);
        match frame["t"].as_str() {
            Some("data") => text.push_str(frame["data"].as_str().unwrap()),
            Some("end") => break,
            other => panic!("unexpected frame {other:?}"),
        }
    }
    (head["status"].as_u64().unwrap(), text)
}

#[tokio::test]
async fn only_a_grant_this_device_made_and_still_keeps_runs_a_tool() {
    let engine = Arc::<Engine>::default();
    let server = endpoint(engine.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("outside-agents.json");
    let source = server.clone();
    let agents = OutsideAgents::new(path.clone(), "dev", Arc::new(move || Some(source.clone())));
    let (tx, mut rx) = mpsc::channel(64);
    let out = McpOut::new(tx);
    agents.joined(out.clone());
    assert_eq!(next(&mut rx).await.1, json!({ "t": "grants", "grants": [] }));

    // A grant id the edge made up: nothing runs.
    let (status, _) = call(&agents, &out, &mut rx, "edge-invented").await;
    assert_eq!(status, 401);
    assert!(lock(&engine.called).is_empty());

    // The person allows an ask: the grant id is this device's own.
    let ask = json!({ "t": "ask", "client": "Grok\u{7}", "redirectHost": "grok.com" });
    agents.frame("ask-1".into(), ask.to_string().into_bytes(), out.clone());
    assert_eq!(agents.snapshot().asks[0].client, "Grok");
    agents.resolve("ask-1", true).await.unwrap();
    let (stream, answer) = next(&mut rx).await;
    assert_eq!(stream, "ask-1");
    let grant = answer["grant"].as_str().unwrap().to_owned();
    assert_ne!(grant, "edge-invented");
    assert!(agents.snapshot().asks.is_empty());
    assert_eq!(agents.snapshot().grants[0].id, grant);
    let reopened = OutsideAgents::new(path.clone(), "dev", Arc::new(|| None));
    assert_eq!(reopened.snapshot().grants, agents.snapshot().grants, "kept across restarts");

    let (status, body) = call(&agents, &out, &mut rx, &grant).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"id\":7"), "{body}");
    let dispatched = lock(&engine.called).len();
    assert!(dispatched > 0);

    // Revoked: the edge is told, and the grant runs nothing more.
    agents.revoke(&grant).await.unwrap();
    assert_eq!(next(&mut rx).await.1, json!({ "t": "revoked", "grant": grant }));
    let (status, _) = call(&agents, &out, &mut rx, &grant).await;
    assert_eq!(status, 401);
    assert_eq!(lock(&engine.called).len(), dispatched);
    assert!(OutsideAgents::new(path, "dev", Arc::new(|| None)).snapshot().grants.is_empty());
}

#[tokio::test]
async fn a_declined_ask_keeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let agents = OutsideAgents::new(dir.path().join("outside-agents.json"), "dev", Arc::new(|| None));
    let (tx, mut rx) = mpsc::channel(64);
    let out = McpOut::new(tx);
    agents.joined(out.clone());
    next(&mut rx).await;
    let ask = json!({ "t": "ask", "client": "Grok", "redirectHost": "grok.com" });
    agents.frame("ask-1".into(), ask.to_string().into_bytes(), out.clone());
    agents.resolve("ask-1", false).await.unwrap();
    assert_eq!(next(&mut rx).await, ("ask-1".to_owned(), json!({ "t": "answer", "denied": true })));
    assert_eq!(agents.snapshot(), OutsideAgentsView::default());
    assert!(agents.resolve("ask-1", true).await.is_err(), "answered once");
}
