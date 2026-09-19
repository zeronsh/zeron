use super::*;

#[test]
fn attached_servers_are_limited_to_loopback() {
    for address in [
        "http://localhost:4096",
        "http://127.0.0.1:4096",
        "http://127.42.0.8:4096",
        "http://[::1]:4096",
    ] {
        assert!(opencode_url_is_loopback(
            &reqwest::Url::parse(address).unwrap()
        ));
    }
    for address in ["http://192.168.1.42:4096", "https://opencode.example.com"] {
        assert!(!opencode_url_is_loopback(
            &reqwest::Url::parse(address).unwrap()
        ));
    }
}

#[test]
fn connection_url_validation_preserves_proxy_paths_and_rejects_invalid_addresses() {
    let url = parse_opencode_url(" https://LOCALHOST:49374/opencode/ ").unwrap();
    assert_eq!(url.as_str(), "https://localhost:49374/opencode/");
    assert!(parse_opencode_url("http://[::1]").is_ok());
    for address in [
        "file:///opencode",
        "http://localhost:0",
        "http://localhost:65536",
        "http://user:secret@localhost",
        "http://localhost?directory=/project",
        "http://localhost#fragment",
    ] {
        assert!(parse_opencode_url(address).is_err(), "{address}");
    }
}

/// Real HTTP/SSE transport with explicitly ordered turn events. No provider or
/// installed CLI is involved, so duplicate completion frames are reproducible.
struct TurnWire {
    base: String,
    bus: mpsc::UnboundedSender<Value>,
    posts: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    requests: mpsc::UnboundedReceiver<String>,
    events: mpsc::Receiver<Result<AgentEvent, HarnessError>>,
    interrupt: tokio_util::sync::CancellationToken,
    server: tokio::task::JoinHandle<()>,
    run: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct WireCase<'a> {
    queued: bool,
    v2: bool,
    modern: bool,
    auto_approve: bool,
    answer: Option<bool>,
    agent: Option<&'a str>,
    cwd: &'a str,
    resume: bool,
    fail_agent: bool,
    command: bool,
    fail_command: bool,
}

impl Drop for TurnWire {
    fn drop(&mut self) {
        self.interrupt.cancel();
        self.run.abort();
        self.server.abort();
    }
}

impl TurnWire {
    async fn start(queued: bool) -> Self {
        Self::start_proto(queued, false).await
    }

    /// `v2` serves the 2.x wire (`/api/*` routes, `{data}` wrappers,
    /// version-bearing `/api/health`); otherwise the 1.18 one.
    async fn start_proto(queued: bool, v2: bool) -> Self {
        Self::start_policy(queued, v2, true, None).await
    }

    async fn start_policy(
        queued: bool,
        v2: bool,
        auto_approve: bool,
        answer: Option<bool>,
    ) -> Self {
        Self::start_agent(queued, v2, auto_approve, answer, None).await
    }

    async fn start_agent(
        queued: bool,
        v2: bool,
        auto_approve: bool,
        answer: Option<bool>,
        selected_agent: Option<&str>,
    ) -> Self {
        Self::start_case(WireCase {
            queued,
            v2,
            auto_approve,
            answer,
            agent: selected_agent,
            ..Default::default()
        })
        .await
    }

    async fn start_case(case: WireCase<'_>) -> Self {
        let WireCase {
            queued,
            v2,
            modern,
            auto_approve,
            answer,
            agent: selected_agent,
            cwd,
            resume,
            fail_agent,
            command,
            fail_command,
        } = case;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (bus, bus_rx) = mpsc::unbounded_channel::<Value>();
        let bus_rx = Arc::new(tokio::sync::Mutex::new(Some(bus_rx)));
        let (request_tx, requests) = mpsc::unbounded_channel();
        let posts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = posts.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let bus_rx = bus_rx.clone();
                let request_tx = request_tx.clone();
                let recorded = recorded.clone();
                connections.spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0; 4096];
                    let header_end = loop {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 { return; }
                        request.extend_from_slice(&buf[..n]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let header = String::from_utf8_lossy(&request[..header_end]);
                    let is_post = header.starts_with("POST ");
                    let path = header.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
                    let route_path = path.split('?').next().unwrap_or(&path);
                    let length = header.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap_or(0);
                    while request.len() < header_end + length {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 { return; }
                        request.extend_from_slice(&buf[..n]);
                    }
                    if is_post { recorded.lock().unwrap().push((path.clone(), serde_json::from_slice(&request[header_end..header_end+length]).unwrap_or(Value::Null))); }
                    if path == "/global/event" || path == "/api/event" {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: connected\n\n").await.unwrap();
                        let mut events = bus_rx.lock().await.take().unwrap();
                        while let Some(event) = events.recv().await {
                            if socket.write_all(format!("data: {event}\n\n").as_bytes()).await.is_err() { break; }
                        }
                        return;
                    }
                    let command_body = (is_post && v2 && route_path.ends_with("/command"))
                        .then(|| serde_json::from_slice::<Value>(&request[header_end..header_end+length]).unwrap_or(Value::Null));
                    // Match OpenCode 2.0.7's actual SessionCommandInput. Keep
                    // this validation in the fake server so a wrong client
                    // schema receives the same immediate 400 as the real one.
                    let command_schema_rejected = command_body.as_ref().is_some_and(|body| {
                        body.get("name").and_then(Value::as_str).is_none()
                            || body.get("text").and_then(Value::as_str).is_none()
                            || !body.get("files").is_some_and(Value::is_array)
                            || body.get("command").is_some()
                            || body.get("arguments").is_some()
                    });
                    let command_failed = is_post && route_path.ends_with("/command")
                        && (fail_command || command_schema_rejected);
                    let status = if fail_agent && route_path == "/api/session/fixture/agent" { "500 Internal Server Error" } else if command_failed { "400 Bad Request" } else { "200 OK" };
                    let body = if command_failed {
                        r#"{"message":"invalid command payload"}"#.to_owned()
                    } else if v2 && modern && route_path == "/api/model" {
                        let directory = reqwest::Url::parse(&format!("http://local{path}")).unwrap()
                            .query_pairs().find(|(key, _)| key == "directory")
                            .map(|(_, value)| value.into_owned()).unwrap_or_default();
                        let project = if directory == "/one" { "one" } else if directory == "/two" { "two" } else { "default" };
                        json!({"data":[
                            {"providerID":"opencode","id":"muse","name":"Muse","enabled":true},
                            {"providerID":"custom","id":format!("{project}/model"),"name":project,"enabled":true}
                        ]}).to_string()
                    } else if v2 {
                        match route_path {
                            "/api/info" if modern => r#"{"version":"2.0.7"}"#,
                            "/api/health" => r#"{"healthy":true,"version":"2.0.3"}"#,
                            "/api/session" => r#"{"data":{"id":"fixture"}}"#,
                            "/api/session/fixture" => r#"{"data":{"id":"fixture"}}"#,
                            "/api/command" if command => r#"{"data":[{"name":"next-task","description":"Start the next task"}]}"#,
                            "/api/command" => r#"{"data":[]}"#,
                            "/api/agent" => r#"{"data":[{"id":"build","mode":"primary","hidden":false},{"id":"team/review","name":"Review","mode":"all","hidden":false},{"id":"explore","mode":"subagent"},{"id":"title","mode":"primary","hidden":true}]}"#,
                            // Non-empty: the catalog-sync retry loop must not stall tests.
                            "/api/model" => r#"{"data":[{"providerID":"opencode","id":"muse","name":"Muse","limit":{"context":1000},"variants":[{"id":"low"}],"enabled":true}]}"#,
                            _ => "{}",
                        }.to_owned()
                    } else {
                        match route_path {
                            "/global/health" => r#"{"healthy":true,"version":"1.18.31"}"#,
                            "/session" => r#"{"id":"fixture"}"#,
                            "/command" if command => r#"[{"name":"next-task","description":"Start the next task"}]"#,
                            "/command" => "[]",
                            _ => "{}",
                        }.to_owned()
                    };
                    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    if path.ends_with("/prompt_async")
                        || path.ends_with("/prompt")
                        || path.ends_with("/abort")
                        || path.ends_with("/interrupt")
                        || (is_post && path.ends_with("/command"))
                        || route_path == "/api/model"
                    {
                        let _ = request_tx.send(path);
                    }
                });
            }
        });
        let (event_tx, events) = mpsc::channel(64);
        let (steer_tx, steering) = mpsc::channel(4);
        if queued {
            steer_tx
                .send(crate::SteerMessage {
                    prompt: "second".into(),
                    message_id: None,
                })
                .await
                .unwrap();
        }
        drop(steer_tx);
        let interrupt = tokio_util::sync::CancellationToken::new();
        let attached = Server::attached(&OpencodeConnection {
            base_url: base.clone(),
            username: String::new(),
            password: None,
        })
        .unwrap();
        let resume_token = resume.then(|| attached.public_session_id("fixture"));
        let run = tokio::spawn(run_session(Session {
            server: attached,
            event_tx,
            controls: RunControls {
                request_input: Box::new(move |questions| {
                    let answer = answer.expect("fixture must not ask for input");
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(questions.into_iter().map(|q| UserInputAnswer {
                        question_id: q.id, labels: vec![if answer { "Yes" } else { "No" }.into()],
                    }).collect());
                    rx
                }),
                steering,
                interrupt: interrupt.clone(),
            },
            request: serde_json::from_value(
                json!({"prompt":if command { "/next-task focus" } else { "first" }, "cwd":cwd, "sandbox":"workspace-write", "autoApprove": auto_approve, "model": if v2 { Some("opencode/muse") } else { None }, "reasoning": "low", "agent": selected_agent, "resume": resume_token}),
            )
            .unwrap(),
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_millis(50),
        }));
        Self {
            base,
            bus,
            posts,
            requests,
            events,
            interrupt,
            server,
            run,
        }
    }

    async fn request(&mut self, suffix: &str) {
        let path = tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(path.ends_with(suffix), "unexpected request: {path}");
    }

    fn status(&self, status: &str) {
        self.bus.send(json!({"type":"session.status", "properties":{"sessionID":"fixture", "status":{"type":status}}})).unwrap();
    }

    fn idle(&self) {
        self.bus
            .send(json!({"type":"session.idle", "properties":{"sessionID":"fixture"}}))
            .unwrap();
    }

    /// Push one raw 2.x `/api/event` frame (normalized in the real
    /// stream_bus, not here).
    fn v2(&self, kind: &str, data: Value) {
        self.bus
            .send(json!({"id": format!("evt_{kind}"), "type": kind, "data": data}))
            .unwrap();
    }

    /// Push a current 2.x `/api/event` frame used by OpenCode 2.0.7+.
    fn v2_current(&self, kind: &str, properties: Value) {
        self.bus
            .send(json!({"id": format!("evt_{kind}"), "type": kind, "data": properties}))
            .unwrap();
    }

    async fn done(&mut self) -> (DoneStatus, String) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut text = String::new();
            loop {
                match self.events.recv().await.unwrap().unwrap() {
                    AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                    AgentEvent::Done { status, .. } => return (status, text),
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn queued_turn_ignores_previous_turn_duplicate_idle() {
    for status_first in [true, false] {
        let mut wire = TurnWire::start(true).await;
        wire.request("/prompt_async").await;
        wire.status("busy");
        // Both completion encodings belong to the first turn. The first frame
        // submits the queued prompt; the second must not finish that new turn.
        if status_first {
            wire.status("idle");
            wire.idle();
        } else {
            wire.idle();
            wire.status("idle");
        }
        wire.request("/prompt_async").await;
        wire.status("busy");
        wire.bus.send(json!({"type":"message.updated", "properties":{"info":{"id":"answer", "sessionID":"fixture", "role":"assistant"}}})).unwrap();
        wire.bus.send(json!({"type":"message.part.updated", "properties":{"part":{"id":"text", "messageID":"answer", "sessionID":"fixture", "type":"text", "text":"SECOND_OK"}}})).unwrap();
        wire.status("idle");
        let (status, text) = wire.done().await;
        assert_eq!(status, DoneStatus::Completed);
        assert_eq!(
            text, "SECOND_OK",
            "queued turn was completed before its response"
        );
    }
}

#[tokio::test]
async fn v2_wire_streams_text_and_settles_on_execution_success() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
    wire.v2(
        "session.step.started",
        json!({
            "sessionID": "fixture",
            "assistantMessageID": "msg_a",
            "model": {"id": "muse", "providerID": "opencode", "variant": "low"},
        }),
    );
    wire.v2(
        "session.text.started",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a", "ordinal": 0
        }),
    );
    wire.v2(
        "session.text.delta",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a", "ordinal": 0,
            "delta": "PONG"
        }),
    );
    wire.v2(
        "session.text.ended",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a", "ordinal": 0,
            "text": "PONG"
        }),
    );
    wire.v2(
        "session.usage.updated",
        json!({
            "sessionID": "fixture", "cost": 0,
            "tokens": {"input": 10, "output": 2, "reasoning": 0,
                       "cache": {"read": 0, "write": 0}}
        }),
    );
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID": "fixture"}),
    );

    let (status, text, usage) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut text = String::new();
        let mut usage = None;
        loop {
            match wire.events.recv().await.unwrap().unwrap() {
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    usage = Some((input_tokens, output_tokens));
                }
                AgentEvent::Done { status, .. } => return (status, text, usage),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(text, "PONG");
    assert_eq!(usage, Some((10, 2)));
}

#[tokio::test]
async fn current_v2_wire_streams_text_and_settles_without_false_stall() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2_current(
        "session.status",
        json!({"sessionID": "fixture", "status": {"type": "busy"}}),
    );
    wire.v2_current(
        "session.next.step.started",
        json!({
            "timestamp": 1,
            "sessionID": "fixture",
            "assistantMessageID": "msg_current",
            "agent": "plan",
            "model": {"id": "muse", "providerID": "opencode"},
        }),
    );
    wire.v2_current(
        "session.next.text.delta",
        json!({
            "timestamp": 2,
            "sessionID": "fixture",
            "assistantMessageID": "msg_current",
            "textID": "text_current",
            "delta": "CURRENT_OK",
        }),
    );
    wire.v2_current("session.idle", json!({"sessionID": "fixture"}));

    let (status, text) = wire.done().await;
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(text, "CURRENT_OK");
}

#[tokio::test]
async fn current_v2_provider_error_settles_immediately_instead_of_stalling() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2_current(
        "session.error",
        json!({
            "sessionID": "fixture",
            "error": {
                "name": "ProviderAuthError",
                "data": {"providerID": "anthropic", "message": "unauthorized"},
            },
        }),
    );
    wire.v2_current("session.idle", json!({"sessionID": "fixture"}));

    let (status, text) = wire.done().await;
    assert_eq!(status, DoneStatus::Errored);
    assert!(text.is_empty());
}

#[tokio::test]
async fn current_v2_command_uses_exact_opencode_2_0_7_schema() {
    let mut wire = TurnWire::start_case(WireCase {
        v2: true,
        command: true,
        ..Default::default()
    })
    .await;
    wire.request("/api/model").await;
    wire.request("/api/session/fixture/command").await;

    let body = wire
        .posts
        .lock()
        .unwrap()
        .iter()
        .find(|(path, _)| path.ends_with("/command"))
        .map(|(_, body)| body.clone())
        .expect("command request was recorded");
    assert_eq!(
        body,
        json!({"name":"next-task", "text":"focus", "files":[]})
    );

    wire.v2_current(
        "session.status",
        json!({"sessionID":"fixture", "status":{"type":"busy"}}),
    );
    wire.v2_current("session.idle", json!({"sessionID":"fixture"}));
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
}

#[tokio::test]
async fn rejected_v2_command_reports_http_error_without_stalling() {
    let mut wire = TurnWire::start_case(WireCase {
        v2: true,
        command: true,
        fail_command: true,
        ..Default::default()
    })
    .await;
    wire.request("/api/model").await;
    wire.request("/api/session/fixture/command").await;

    let (status, text) = wire.done().await;
    assert_eq!(status, DoneStatus::Errored);
    assert!(text.is_empty());
}

#[tokio::test]
async fn server_agent_changes_reach_the_run_on_both_protocols() {
    for v2 in [false, true] {
        let mut wire = TurnWire::start_proto(false, v2).await;
        if v2 {
            wire.request("/api/model").await;
            wire.request("/prompt").await;
            wire.v2_current(
                "session.agent.selected",
                json!({
                    "sessionID": "fixture", "messageID": "switch",
                    "timestamp": 1, "agent": "plan"
                }),
            );
        } else {
            wire.request("/prompt_async").await;
            wire.bus
                .send(json!({"type":"message.updated", "properties":{"info":{
                    "id":"answer", "sessionID":"fixture", "role":"assistant", "agent":"plan"
                }}}))
                .unwrap();
        }
        let changed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let AgentEvent::AgentChanged { agent } =
                    wire.events.recv().await.unwrap().unwrap()
                {
                    return agent;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(changed, "plan");
    }
}

#[tokio::test]
async fn v2_wire_tool_frames_open_and_resolve_chips() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
    wire.v2(
        "session.step.started",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a"
        }),
    );
    wire.v2(
        "session.tool.input.started",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a",
            "id": "call_1", "name": "read"
        }),
    );
    wire.v2(
        "session.tool.called",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a",
            "id": "call_1", "input": {"path": "/tmp/oc2x-probe/note.txt"}
        }),
    );
    wire.v2(
        "session.tool.success",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a",
            "id": "call_1",
            "content": [{"type": "text", "text": "1: The secret word is BANANA42"}]
        }),
    );
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID": "fixture"}),
    );

    let (status, calls, results) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut calls = Vec::new();
        let mut results = Vec::new();
        loop {
            match wire.events.recv().await.unwrap().unwrap() {
                AgentEvent::ToolCall { id, call } => calls.push((id, call)),
                AgentEvent::ToolResult { id, output, .. } => results.push((id, output)),
                AgentEvent::Done { status, .. } => return (status, calls, results),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(
        calls,
        vec![(
            "fixture:msg_a:call_1".to_owned(),
            ToolCall::ReadFile {
                path: "/tmp/oc2x-probe/note.txt".to_owned()
            }
        )]
    );
    assert_eq!(
        results,
        vec![(
            "fixture:msg_a:call_1".to_owned(),
            Some("1: The secret word is BANANA42".to_owned())
        )]
    );
}

#[tokio::test]
async fn v2_execution_failure_and_interrupt_settle_the_turn() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
    wire.v2(
        "session.execution.failed",
        json!({"sessionID": "fixture", "error": {"type": "provider.auth", "message": ""}}),
    );
    let (status, error) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut error = None;
        loop {
            match wire.events.recv().await.unwrap().unwrap() {
                AgentEvent::Error { message } => error = Some(message),
                AgentEvent::Done {
                    status, error: e, ..
                } => return (status, e.or(error)),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.unwrap().contains("provider.auth"));

    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
    wire.interrupt.cancel();
    wire.request("/interrupt").await;
    wire.v2(
        "session.execution.interrupted",
        json!({"sessionID": "fixture", "reason": "user"}),
    );
    assert_eq!(wire.done().await.0, DoneStatus::Interrupted);
}

#[tokio::test]
async fn catalog_decodes_fragmented_http_without_retaining_unused_fields() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = json!({
        "all": [{"id":"test", "models":{"model":{"name":"模型", "variants":{"high":{"unused":"x".repeat(128 * 1024)}}}}}],
        "connected":["test"]
    }).to_string();
    let expected: ProviderCatalog = serde_json::from_str(&body).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.read(&mut [0; 4096]).await.unwrap();
        socket
            .write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
            )
            .await
            .unwrap();
        for chunk in body.as_bytes().chunks(257) {
            socket.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    let response = reqwest::get(format!("http://{address}")).await.unwrap();
    let catalog: ProviderCatalog = decode_json_response(response).await.unwrap();
    assert_eq!(
        models_from_providers(&catalog),
        models_from_providers(&expected)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_catalog_decode_releases_a_stalled_http_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.read(&mut [0; 4096]).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n{")
            .await
            .unwrap();
        let result = socket.read(&mut [0; 1]).await;
        let _ = closed_tx.send(result);
    });
    let response = reqwest::get(format!("http://{address}")).await.unwrap();
    let decode = tokio::spawn(decode_json_response::<ProviderCatalog>(response));
    tokio::time::sleep(Duration::from_millis(20)).await;
    decode.abort();
    assert!(decode.await.unwrap_err().is_cancelled());
    let result = tokio::time::timeout(Duration::from_secs(2), closed_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Ok(0) | Err(_)),
        "cancel must close the body reader"
    );
    server.await.unwrap();
}
use serde_json::json;

#[test]
fn provider_discovery_ignores_metadata_and_accepts_null_optional_fields() {
    let catalog: ProviderCatalog = serde_json::from_str(
        r#"{
        "all": [{
            "id": "local", "name": null,
            "models": {
                "small": {"name": null, "variants": null},
                "thinking": {
                    "variants": {"high": {"nested": [{"unused": "configuration"}]}},
                    "capabilities": {"large": [1, 2, 3]},
                    "cost": {"input": 1}, "limit": {"context": 200000}
                }
            },
            "options": {"unused": [true, false, null]}
        }, {"id": "empty", "models": null}],
        "connected": null,
        "default": {"unused": "model"}
    }"#,
    )
    .unwrap();
    let models = models_from_providers(&catalog);
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "local/small");
    assert_eq!(models[0].description.as_deref(), Some("local"));
    assert!(models[0].reasoning_levels.is_empty());
    assert_eq!(models[1].reasoning_levels, vec![ReasoningLevel::High]);
    assert_eq!(
        pick_variant(&catalog, "local", "thinking", Some(ReasoningLevel::High)).as_deref(),
        Some("high")
    );
}

#[test]
fn models_map_provider_catalog_with_variant_ladders() {
    let providers: ProviderCatalog = serde_json::from_value(json!({
        "all": [
            {
                "id": "anthropic",
                "name": "Anthropic",
                "models": {
                    "claude-opus-5": {
                        "name": "Claude Opus 5",
                        "variants": {"low": {}, "medium": {}, "high": {}, "max": {}},
                    },
                    "claude-haiku-4-5": {"name": "Claude Haiku 4.5"},
                }
            },
            {
                "id": "opencode",
                "name": "OpenCode Zen",
                "models": {"big-pickle": {"name": "Big Pickle"}}
            }
        ],
        "default": {},
        "connected": ["anthropic"],
    }))
    .unwrap();
    let models = models_from_providers(&providers);
    // `connected` filters: the full catalog is 194 providers / 7k models of
    // which the user can run almost none (v0.2.21 field report).
    assert_eq!(models.len(), 2);
    let opus = models
        .iter()
        .find(|m| m.id == "anthropic/claude-opus-5")
        .expect("opus");
    assert_eq!(opus.label, "Claude Opus 5");
    assert_eq!(opus.description.as_deref(), Some("Anthropic"));
    assert_eq!(
        opus.reasoning_levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Max
        ]
    );
    let haiku = models
        .iter()
        .find(|m| m.id == "anthropic/claude-haiku-4-5")
        .expect("haiku");
    assert!(haiku.reasoning_levels.is_empty());
    assert!(
        !models.iter().any(|m| m.id == "opencode/big-pickle"),
        "unconnected providers stay out of the picker"
    );
}

#[test]
fn missing_connected_list_falls_back_to_the_full_catalog() {
    let providers: ProviderCatalog = serde_json::from_value(json!({
        "all": [
            {"id": "a", "models": {"m1": {}}},
            {"id": "b", "models": {"m2": {}}},
        ],
    }))
    .unwrap();
    assert_eq!(models_from_providers(&providers).len(), 2);
    let providers: ProviderCatalog = serde_json::from_value(json!({
        "all": [
            {"id": "a", "models": {"m1": {}}},
            {"id": "b", "models": {"m2": {}}},
        ],
        "connected": [],
    }))
    .unwrap();
    assert_eq!(models_from_providers(&providers).len(), 2);
}

#[test]
fn variants_only_ride_models_that_advertise_them() {
    let providers: ProviderCatalog = serde_json::from_value(json!({
        "all": [{
            "id": "anthropic",
            "models": {
                "opus": {"variants": {"high": {}, "max": {}}},
                "haiku": {},
            }
        }]
    }))
    .unwrap();
    assert_eq!(
        pick_variant(&providers, "anthropic", "opus", Some(ReasoningLevel::High)).as_deref(),
        Some("high")
    );
    // XHigh clamps down the candidate ladder to an advertised id.
    assert_eq!(
        pick_variant(&providers, "anthropic", "opus", Some(ReasoningLevel::XHigh)).as_deref(),
        Some("high")
    );
    assert_eq!(
        pick_variant(&providers, "anthropic", "haiku", Some(ReasoningLevel::High)),
        None
    );
    assert_eq!(pick_variant(&providers, "anthropic", "opus", None), None);
    assert_eq!(
        pick_variant(&providers, "missing", "opus", Some(ReasoningLevel::Low)),
        None
    );
}

#[test]
fn prompt_body_carries_model_variant_and_attachments() {
    let body = prompt_body(
        "hello",
        Some(("anthropic", "claude-opus-5")),
        Some("high"),
        &["/tmp/shot.png".to_owned()],
    );
    assert_eq!(body["model"]["providerID"], "anthropic");
    assert_eq!(body["model"]["modelID"], "claude-opus-5");
    assert_eq!(body["variant"], "high");
    assert_eq!(body["parts"][0]["type"], "text");
    assert_eq!(body["parts"][0]["text"], "hello");
    assert_eq!(body["parts"][1]["type"], "file");
    assert_eq!(body["parts"][1]["mime"], "image/png");
    assert_eq!(body["parts"][1]["url"], "file:///tmp/shot.png");
}

fn feed_with_assistant(message: &str) -> SessionFeed {
    let mut feed = SessionFeed::default();
    feed.assistant_messages.insert(message.into(), true);
    feed
}

#[test]
fn reasoning_parts_stream_as_reasoning_deltas() {
    let mut feed = feed_with_assistant("msg_a");
    // Opening snapshot: empty reasoning part fixes the kind.
    let open = json!({
        "id": "prt_r", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "reasoning", "text": "",
    });
    assert!(part_snapshot_events(&mut feed, &open, true, None).is_empty());
    // Deltas append as ReasoningDelta, not text.
    let props = json!({"sessionID": "ses_1", "messageID": "msg_a", "partID": "prt_r"});
    let events = part_delta_events(&mut feed, &props, "prt_r", "thinking hard");
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ReasoningDelta { text }] if text == "thinking hard"
    ));
    // The closing full snapshot re-sends everything: dedup emits nothing.
    let close = json!({
        "id": "prt_r", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "reasoning", "text": "thinking hard",
    });
    assert!(part_snapshot_events(&mut feed, &close, true, None).is_empty());
    // A longer snapshot emits only the suffix.
    let more = json!({
        "id": "prt_r", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "reasoning", "text": "thinking hard about it",
    });
    let events = part_snapshot_events(&mut feed, &more, true, None);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ReasoningDelta { text }] if text == " about it"
    ));
}

#[test]
fn reasoning_ahead_of_its_message_role_is_held_and_replayed() {
    let mut feed = SessionFeed::default();
    let part = json!({
        "id": "prt_r", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "reasoning", "text": "early thought",
    });
    assert!(part_snapshot_events(&mut feed, &part, true, None).is_empty());
    assert_eq!(feed.pending_parts.len(), 1);
    // The role lands; replay drains the held part.
    feed.assistant_messages.insert("msg_a".into(), true);
    let mut turn = TurnState::begin(None);
    let events = replay_pending(&mut feed, "msg_a", true, &mut turn);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ReasoningDelta { text }] if text == "early thought"
    ));
    assert!(turn.saw_content);
}

#[test]
fn main_feed_user_text_is_the_prompt_echo_and_never_renders() {
    let mut feed = SessionFeed::default();
    feed.assistant_messages.insert("msg_u".into(), false);
    let part = json!({
        "id": "prt_u", "messageID": "msg_u", "sessionID": "ses_1",
        "type": "text", "text": "the prompt",
    });
    assert!(part_snapshot_events(&mut feed, &part, true, None).is_empty());
    // On a CHILD feed the same shape is the message INTO the child.
    let mut child = SessionFeed::default();
    child.assistant_messages.insert("msg_u".into(), false);
    let events = part_snapshot_events(&mut child, &part, false, None);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::UserMessage { text }] if text == "the prompt"
    ));
    // Re-delivered snapshots don't double the entry.
    assert!(part_snapshot_events(&mut child, &part, false, None).is_empty());
}

#[test]
fn tool_parts_open_and_resolve_once() {
    let mut feed = feed_with_assistant("msg_a");
    let running = json!({
        "id": "prt_t", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "tool", "tool": "bash", "callID": "call-1",
        "state": {"status": "running", "input": {"command": "echo ok"}},
    });
    let events = part_snapshot_events(&mut feed, &running, true, None);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ToolCall { id, call: ToolCall::Exec { command } }]
            if id == "call-1" && command == "echo ok"
    ));
    assert!(part_snapshot_events(&mut feed, &running, true, None).is_empty());
    let done = json!({
        "id": "prt_t", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "tool", "tool": "bash", "callID": "call-1",
        "state": {"status": "completed", "input": {"command": "echo ok"}, "output": "ok\n"},
    });
    let events = part_snapshot_events(&mut feed, &done, true, None);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ToolResult { id, is_error: false, output: Some(o), .. }]
            if id == "call-1" && o == "ok\n"
    ));
}

#[test]
fn task_spawn_registers_child_by_metadata_and_completion_settles() {
    let mut feed = feed_with_assistant("msg_a");
    let mut children = HashMap::new();
    let mut pending = VecDeque::new();
    let mut unbound = HashMap::new();
    let running = json!({
        "id": "prt_task", "messageID": "msg_a", "sessionID": "ses_parent",
        "type": "tool", "tool": "task",
        "state": {
            "status": "running",
            "input": {"description": "Scan crates", "prompt": "scan", "subagent_type": "general"},
            "metadata": {"sessionId": "ses_child", "parentSessionId": "ses_parent"},
        },
    });
    let events = part_snapshot_events(
        &mut feed,
        &running,
        true,
        Some((&mut children, &mut pending, &mut unbound)),
    );
    // Genus-gated spawn naming, keyed by the PART id.
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }]
            if id == "prt_task" && name == "Agent: Scan crates"
    ));
    let child = children.get("ses_child").expect("bound child");
    assert_eq!(child.parent_tool_use_id, "prt_task");

    let completed = json!({
        "id": "prt_task", "messageID": "msg_a", "sessionID": "ses_parent",
        "type": "tool", "tool": "task",
        "state": {
            "status": "completed",
            "input": {"description": "Scan crates"},
            "output": "<task_result>done</task_result>",
            "metadata": {"sessionId": "ses_child"},
        },
    });
    assert_eq!(
        task_completion(&completed),
        Some(("ses_child".to_owned(), false))
    );
}

#[test]
fn child_binding_falls_back_to_title_match() {
    let mut children = HashMap::new();
    let mut pending = VecDeque::new();
    pending.push_back(PendingSpawn {
        tool_part_id: "prt_1".into(),
        description: "Scan crates".into(),
    });
    pending.push_back(PendingSpawn {
        tool_part_id: "prt_2".into(),
        description: "Write docs".into(),
    });
    assert!(bind_child(
        &mut children,
        &mut pending,
        "ses_b",
        "Write docs (@general subagent)"
    ));
    assert_eq!(children.get("ses_b").unwrap().parent_tool_use_id, "prt_2");
    assert_eq!(pending.len(), 1);
    // Unmatched title binds FIFO.
    assert!(bind_child(&mut children, &mut pending, "ses_a", "mystery"));
    assert_eq!(children.get("ses_a").unwrap().parent_tool_use_id, "prt_1");
    // Nothing pending: no bind.
    assert!(!bind_child(
        &mut children,
        &mut pending,
        "ses_c",
        "anything"
    ));
}

#[test]
fn questions_map_to_input_panel_shape() {
    let props = json!({
        "id": "que_1",
        "sessionID": "ses_1",
        "questions": [{
            "question": "Which color?",
            "header": "Color",
            "options": [
                {"label": "Red", "description": "warm"},
                {"label": "Blue", "description": "cool"},
            ],
            "multiple": true,
        }],
    });
    let questions = map_questions(&props);
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].id, "q0");
    assert_eq!(questions[0].header, "Color");
    assert_eq!(questions[0].question, "Which color?");
    assert_eq!(questions[0].options, vec!["Red", "Blue"]);
    assert!(questions[0].multi_select);
}

#[test]
fn tool_names_type_the_common_calls() {
    let call = oc_tool_call("bash", &json!({"command": "ls -la"}));
    assert_eq!(
        call,
        ToolCall::Exec {
            command: "ls -la".into()
        }
    );
    let call = oc_tool_call(
        "edit",
        &json!({"filePath": "/w/a.rs", "oldString": "a", "newString": "b"}),
    );
    assert_eq!(
        call,
        ToolCall::EditFile {
            path: "/w/a.rs".into(),
            old_string: Some("a".into()),
            new_string: Some("b".into()),
        }
    );
    let call = oc_tool_call("task", &json!({"description": "Scan crates"}));
    assert!(matches!(&call, ToolCall::Unknown { name, .. } if name == "Agent: Scan crates"));
    assert!(call.is_subagent_spawn());
    let call = oc_tool_call(
        "todowrite",
        &json!({"todos": [
            {"content": "step one", "status": "completed"},
            {"content": "step two", "status": "pending"},
        ]}),
    );
    assert!(matches!(
        &call,
        ToolCall::Todo { items } if items.len() == 2 && items[0].done && !items[1].done
    ));
    let call = oc_tool_call("mystery", &json!({"x": 1}));
    assert!(matches!(&call, ToolCall::Unknown { name, input: Some(_) } if name == "mystery"));
    assert!(!call.is_subagent_spawn());
}

#[test]
fn commands_map_from_wire() {
    let wire = json!([
        {"name": "init", "description": "Create AGENTS.md"},
        {"name": "share"},
        {"description": "nameless is dropped"},
    ]);
    let commands = commands_from_wire(&wire);
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].name, "init");
    assert_eq!(commands[0].description, "Create AGENTS.md");
    assert_eq!(commands[1].name, "share");
}

#[test]
fn stall_env_and_startup_env_parse() {
    // Defaults (no env in test runner): bounded stall, 300s startup.
    assert_eq!(stall_bound(), Some(DEFAULT_STALL_BOUND));
    assert_eq!(startup_timeout(), DEFAULT_STARTUP_TIMEOUT);
}

#[test]
fn directory_header_percent_encodes() {
    assert_eq!(
        encode_directory("/home/u/my project"),
        "/home/u/my%20project"
    );
    assert_eq!(encode_directory("/plain/path"), "/plain/path");
}

#[test]
fn v2_frames_normalize_to_v1_payloads() {
    let mut tools = HashMap::new();
    // Lifecycle: busy on start, idle on the terminal frames.
    let out = normalize_v2_frame(
        json!({"id":"evt_1","type":"session.execution.started","data":{"sessionID":"ses_1"}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"session.status","properties":{
            "sessionID":"ses_1","status":{"type":"busy"}}})]
    );
    for kind in [
        "session.execution.succeeded",
        "session.execution.interrupted",
    ] {
        let out = normalize_v2_frame(
            json!({"id":"evt_2","type":kind,"data":{"sessionID":"ses_1"}}),
            &mut tools,
        );
        assert_eq!(
            out,
            vec![
                json!({"type":if kind == "session.execution.interrupted" { "session.interrupted" } else { "session.idle" },"properties":{"sessionID":"ses_1"}})
            ]
        );
    }
    // Terminal failure = error chip + idle (both shapes captured live).
    let out = normalize_v2_frame(
        json!({"id":"evt_3","type":"session.execution.failed","data":{
            "sessionID":"ses_1","error":{"type":"provider.auth","message":""}}}),
        &mut tools,
    );
    assert_eq!(out.len(), 2);
    // Empty message (live shape) falls back to the error type.
    assert_eq!(
        out[0],
        json!({"type":"session.error","properties":{
            "sessionID":"ses_1",
            "error":{"name":"provider.auth","data":{"message":"provider.auth"}}}}),
    );
    // The interrupt's step-level echo is NOT a provider error.
    assert!(
        normalize_v2_frame(
            json!({"id":"evt_4","type":"session.step.failed","data":{
            "sessionID":"ses_1","error":{"type":"aborted","message":"Step interrupted"}}}),
            &mut tools,
        )
        .is_empty()
    );
    // Streaming: the step registers its assistant message, then text flows
    // as a part delta keyed by message + kind + ordinal.
    let out = normalize_v2_frame(
        json!({"id":"evt_5","type":"session.step.started","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a"}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.updated","properties":{
            "info":{"sessionID":"ses_1","id":"msg_a","role":"assistant"}}})]
    );
    let out = normalize_v2_frame(
        json!({"id":"evt_agent","type":"session.next.agent.switched","data":{
            "sessionID":"ses_1","messageID":"msg_switch","timestamp":1,"agent":"plan"}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"session.agent.changed","properties":{
            "sessionID":"ses_1","agent":"plan"}})]
    );
    let out = normalize_v2_frame(
        json!({"id":"evt_6","type":"session.text.delta","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a","ordinal":0,"delta":"hi"}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.part.delta","properties":{
            "sessionID":"ses_1","messageID":"msg_a","partID":"msg_a:t0",
            "field":"text","delta":"hi"}})]
    );
    // OpenCode 2.0.7+ `/api/event` keeps `data` but prefixes streaming names
    // with `session.next.` and uses explicit part ids.
    let out = normalize_v2_frame(
        json!({"id":"evt_current_step","type":"session.next.step.started","data":{
                "sessionID":"ses_1","assistantMessageID":"msg_current",
                "timestamp":1,"agent":"plan",
                "model":{"id":"muse","providerID":"opencode"}}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.updated","properties":{"info":{
            "sessionID":"ses_1","id":"msg_current","role":"assistant","agent":"plan",
            "model":{"id":"muse","providerID":"opencode"}}}})]
    );
    let out = normalize_v2_frame(
        json!({"id":"evt_current_text","type":"session.next.text.delta","data":{
                "sessionID":"ses_1","assistantMessageID":"msg_current",
                "timestamp":2,"textID":"txt_exact","delta":"current"}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.part.delta","properties":{
            "sessionID":"ses_1","messageID":"msg_current","partID":"txt_exact",
            "field":"text","delta":"current"}})]
    );
    let current_idle = json!({"type":"session.idle","properties":{"sessionID":"ses_1"}});
    assert_eq!(
        normalize_v2_frame(
            json!({"id":"evt_idle","type":"session.idle","data":{"sessionID":"ses_1"}}),
            &mut tools,
        ),
        vec![current_idle]
    );
    let current_error = json!({"type":"session.error","properties":{
        "sessionID":"ses_1","error":{"name":"ProviderAuthError","data":{
            "providerID":"anthropic","message":"unauthorized"}}}});
    assert_eq!(
        normalize_v2_frame(
            json!({"id":"evt_error","type":"session.error","data":{
                "sessionID":"ses_1","error":{"name":"ProviderAuthError","data":{
                    "providerID":"anthropic","message":"unauthorized"}}}}),
            &mut tools,
        ),
        vec![current_error]
    );
    // The global stream's wrapped form remains accepted too.
    let global_status = json!({"id":"evt_status","type":"session.status","properties":{
        "sessionID":"ses_1","status":{"type":"busy"}}});
    assert_eq!(
        normalize_v2_frame(
            json!({"directory":"/w","payload":global_status.clone()}),
            &mut tools,
        ),
        vec![global_status]
    );
    // A tool: the NAME rides input.started; later frames carry only ids.
    normalize_v2_frame(
        json!({"id":"evt_7","type":"session.tool.input.started","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a","id":"call_1","name":"read"}}),
        &mut tools,
    );
    let out = normalize_v2_frame(
        json!({"id":"evt_8","type":"session.tool.called","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a","id":"call_1",
            "input":{"path":"/tmp/x"}}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.part.updated","properties":{"part":{
            "sessionID":"ses_1","messageID":"msg_a","id":"ses_1:msg_a:call_1","callID":"ses_1:msg_a:call_1",
            "type":"tool","tool":"read",
            "state":{"status":"running","input":{"path":"/tmp/x"}}}}})]
    );
    // Usage totals reach the engine as an assistant message.updated.
    let out = normalize_v2_frame(
        json!({"id":"evt_9","type":"session.usage.updated","data":{
            "sessionID":"ses_1","cost":0,
            "tokens":{"input":10,"output":2,"reasoning":0,"cache":{"read":0,"write":0}}}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.updated","properties":{
            "info":{"sessionID":"ses_1","id":"usage","role":"assistant",
                    "tokens":{"input":10,"output":2,"reasoning":0,
                              "cache":{"read":0,"write":0}}}}})]
    );
    // The permission ask keeps its 1.x name on 2.x (observed live when a
    // tool reaches outside the workspace); the auto-approver replies.
    let out = normalize_v2_frame(
        json!({"id":"evt_11","type":"permission.asked","data":{
            "id":"per_1","sessionID":"ses_1","action":"external_directory",
            "resources":["/tmp/*"]}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"permission.asked","properties":{
            "id":"per_1","sessionID":"ses_1","action":"external_directory","resources":["/tmp/*"]}})]
    );
    // Boilerplate frames (catalog sync etc.) drop.
    assert!(
        normalize_v2_frame(
            json!({"id":"evt_10","type":"catalog.updated","data":{}}),
            &mut tools,
        )
        .is_empty()
    );
}

#[test]
fn effective_agent_changes_are_trimmed_and_deduplicated() {
    let mut current = Some("build".to_owned());
    assert!(agent_change(&mut current, Some("build")).is_none());
    assert!(agent_change(&mut current, Some("  ")).is_none());
    assert_eq!(
        agent_change(&mut current, Some(" plan ")),
        Some(AgentEvent::AgentChanged {
            agent: "plan".to_owned()
        })
    );
    assert_eq!(current.as_deref(), Some("plan"));
}

#[tokio::test]
async fn v2_command_selections_update_agent_model_and_reasoning() {
    let mut wire = TurnWire::start_case(WireCase {
        v2: true,
        modern: true,
        command: true,
        ..Default::default()
    })
    .await;
    wire.request("/api/model").await;
    wire.request("/command").await;

    wire.v2_current(
        "session.status",
        json!({"sessionID":"fixture","status":{"type":"busy"}}),
    );

    // These are the durable selection events in OpenCode 2.0.9. A command
    // can switch settings without starting a provider step at all.
    for session in ["unrelated", "fixture"] {
        let ours = session == "fixture";
        wire.v2_current(
            "session.agent.selected",
            json!({"sessionID":session,"agent":if ours { "plan" } else { "other-agent" },"previous":"build"}),
        );
        wire.v2_current(
            "session.model.selected",
            json!({"sessionID":session,"model":{
                "providerID":"custom","id":if ours { "review/model" } else { "other-model" },"variant":"xhigh"
            }}),
        );
    }
    // A repeated selection/step snapshot must not rewrite client settings.
    wire.v2_current(
        "session.step.started",
        json!({"sessionID":"fixture","assistantMessageID":"msg_review","agent":"plan",
            "model":{"providerID":"custom","id":"review/model","variant":"xhigh"}}),
    );
    // A variant-only change matters even when the model id stays the same.
    wire.v2_current(
        "session.model.selected",
        json!({"sessionID":"fixture","model":{
            "providerID":"custom","id":"review/model","variant":"low"
        }}),
    );
    // A missing variant clears the previous thinking level.
    wire.v2_current(
        "session.model.selected",
        json!({"sessionID":"fixture","model":{"providerID":"custom","id":"fast"}}),
    );
    wire.v2_current("session.idle", json!({"sessionID":"fixture"}));

    let changes = tokio::time::timeout(Duration::from_secs(5), async {
        let mut changes = Vec::new();
        while let Some(event) = wire.events.recv().await {
            match event.unwrap() {
                event @ (AgentEvent::AgentChanged { .. } | AgentEvent::ModelChanged { .. }) => {
                    changes.push(event);
                }
                AgentEvent::Done { status, .. } => {
                    assert_eq!(status, DoneStatus::Completed);
                    return changes;
                }
                _ => {}
            }
        }
        panic!("run ended without Done");
    })
    .await
    .unwrap();
    assert_eq!(
        changes,
        vec![
            AgentEvent::AgentChanged {
                agent: "plan".into()
            },
            AgentEvent::ModelChanged {
                model: "custom/review/model".into(),
                reasoning: Some(ReasoningLevel::XHigh),
            },
            AgentEvent::ModelChanged {
                model: "custom/review/model".into(),
                reasoning: Some(ReasoningLevel::Low),
            },
            AgentEvent::ModelChanged {
                model: "custom/fast".into(),
                reasoning: None
            },
        ]
    );
}

#[tokio::test]
async fn v2_step_snapshot_reports_effective_model_and_thinking_level() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2_current(
        "session.next.step.started",
        json!({"sessionID":"fixture","assistantMessageID":"msg_review","agent":"plan",
            "model":{"providerID":"custom","id":"review","variant":"high"}}),
    );
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = wire.events.recv().await {
            if let event @ AgentEvent::ModelChanged { .. } = event.unwrap() {
                return event;
            }
        }
        panic!("no model change");
    })
    .await
    .unwrap();
    assert_eq!(
        changed,
        AgentEvent::ModelChanged {
            model: "custom/review".into(),
            reasoning: Some(ReasoningLevel::High),
        }
    );
}

#[test]
fn v2_model_list_folds_into_provider_catalog() {
    let list: V2ModelList = serde_json::from_value(json!({
        "location": {"directory": "/w"},
        "data": [
            {"providerID": "opencode", "id": "muse-spark", "name": "Muse Spark",
             "limit": {"context": 1000000},
             // Live shape: variants are `{id, settings}` objects.
             "variants": [{"id": "low", "settings": {"x": 1}}, {"id": "high"}],
             "enabled": true},
            {"providerID": "opencode", "id": "plain", "enabled": true},
            {"providerID": "dead", "id": "off", "enabled": false},
        ]
    }))
    .unwrap();
    let catalog = catalog_from_v2_models(list.data);
    let models = models_from_providers(&catalog);
    assert_eq!(models.len(), 2);
    let muse = models
        .iter()
        .find(|m| m.id == "opencode/muse-spark")
        .unwrap();
    assert_eq!(muse.label, "Muse Spark");
    assert_eq!(
        muse.reasoning_levels,
        vec![ReasoningLevel::Low, ReasoningLevel::High]
    );
    // The folded catalog feeds variant picking exactly like 1.x's.
    assert_eq!(
        pick_variant(
            &catalog,
            "opencode",
            "muse-spark",
            Some(ReasoningLevel::High)
        )
        .as_deref(),
        Some("high")
    );
    assert!(models.iter().all(|m| m.id != "dead/off"));
}

#[test]
fn prompt_body_v2_carries_text_and_files_only() {
    let body = prompt_body_v2("hello", &["/tmp/shot.png".to_owned()]);
    assert_eq!(body["text"], "hello");
    assert_eq!(body["files"][0]["uri"], "file:///tmp/shot.png");
    assert_eq!(body["files"][0]["name"], "shot.png");
    assert!(
        body.get("model").is_none(),
        "model rides the session on 2.x"
    );
    assert!(body.get("parts").is_none());
}

#[tokio::test]
async fn permissions_stay_session_scoped_and_never_persist_grants() {
    for v2 in [false, true] {
        let mut wire = TurnWire::start_proto(false, v2).await;
        if v2 {
            wire.request("/api/model").await;
        }
        wire.request(if v2 { "/prompt" } else { "/prompt_async" })
            .await;
        let ask = |owner: Option<&str>, id: &str| {
            let mut data =
                json!({"id":id, "action":"external_directory", "resources":["/private/*"]});
            if let Some(owner) = owner {
                data["sessionID"] = json!(owner);
            }
            if v2 {
                json!({"type":"permission.asked", "data":data})
            } else {
                json!({"type":"permission.asked", "properties":data})
            }
        };
        wire.bus.send(ask(Some("foreign"), "foreign")).unwrap();
        wire.bus.send(ask(None, "missing")).unwrap();
        // Establish an owned child before its first permission request.
        wire.bus.send(if v2 {
            json!({"type":"session.created","data":{"sessionID":"child","parentID":"fixture"}})
        } else {
            json!({"type":"session.created","properties":{"info":{"id":"child","parentID":"fixture"}}})
        }).unwrap();
        wire.bus.send(ask(Some("child"), "child")).unwrap();
        wire.bus.send(ask(Some("fixture"), "own")).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if wire
                    .posts
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(p, _)| p.contains("permission"))
                    .count()
                    >= 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let posts = wire.posts.lock().unwrap();
        let approvals: Vec<_> = posts
            .iter()
            .filter(|(p, _)| p.contains("permission"))
            .collect();
        assert_eq!(
            approvals.len(),
            2,
            "foreign or ownerless permission was answered"
        );
        for (path, body) in approvals {
            assert_eq!(body["reply"], "once");
            assert!(!path.contains("foreign") && !path.contains("missing"));
        }
    }
}

#[tokio::test]
async fn permissions_without_auto_approve_require_an_explicit_answer() {
    for accept in [false, true] {
        let mut wire = TurnWire::start_policy(false, true, false, Some(accept)).await;
        wire.request("/api/model").await;
        wire.request("/prompt").await;
        wire.v2(
            "permission.asked",
            json!({"id":"approval", "sessionID":"fixture"}),
        );
        let body = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some((_, body)) = wire
                    .posts
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(p, _)| p.contains("permission"))
                {
                    break body.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(body["reply"], if accept { "once" } else { "reject" });
    }
}

#[test]
fn v2_tools_are_scoped_and_retired_and_real_failures_are_visible() {
    let mut tools = HashMap::new();
    let event = |kind, session, message, name| {
        json!({"type":kind,"data":{
            "sessionID":session,"assistantMessageID":message,"id":"same","name":name,
            "error":{"type":"permission.denied","message":"Denied"}
        }})
    };
    for (session, message, name) in [
        ("a", "m1", "read"),
        ("b", "m1", "bash"),
        ("a", "m2", "write"),
    ] {
        normalize_v2_frame(
            event("session.tool.input.started", session, message, name),
            &mut tools,
        );
    }
    for (session, message, name) in [
        ("a", "m1", "read"),
        ("b", "m1", "bash"),
        ("a", "m2", "write"),
    ] {
        let out = normalize_v2_frame(
            event("session.tool.failed", session, message, ""),
            &mut tools,
        );
        assert_eq!(out[0].pointer("/properties/part/tool").unwrap(), name);
        assert_eq!(
            out[0].pointer("/properties/part/state/status").unwrap(),
            "error"
        );
        assert_eq!(
            out[0].pointer("/properties/part/callID").unwrap(),
            &json!(format!("{session}:{message}:same"))
        );
    }
    assert!(tools.is_empty());
    normalize_v2_frame(
        event("session.tool.input.started", "a", "m1", "read"),
        &mut tools,
    );
    normalize_v2_frame(
        event("session.execution.interrupted", "a", "m1", ""),
        &mut tools,
    );
    assert!(tools.is_empty());
}

#[tokio::test]
async fn v2_reasoning_deltas_and_failed_tools_reach_the_feed() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID":"fixture"}));
    wire.v2(
        "session.step.started",
        json!({"sessionID":"fixture","assistantMessageID":"m"}),
    );
    wire.v2(
        "session.reasoning.started",
        json!({"sessionID":"fixture","assistantMessageID":"m","ordinal":0}),
    );
    wire.v2(
        "session.reasoning.delta",
        json!({"sessionID":"fixture","assistantMessageID":"m","ordinal":0,"delta":"Thinking"}),
    );
    wire.v2(
        "session.tool.input.started",
        json!({"sessionID":"fixture","assistantMessageID":"m","id":"c","name":"read"}),
    );
    wire.v2("session.tool.failed",json!({"sessionID":"fixture","assistantMessageID":"m","id":"c","error":{"message":"Denied"}}));
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID":"fixture"}),
    );
    let mut reasoning = String::new();
    let mut failed = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(3), wire.events.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            AgentEvent::ReasoningDelta { text } => reasoning.push_str(&text),
            AgentEvent::ToolResult { is_error, .. } => failed |= is_error,
            AgentEvent::Done { .. } => break,
            _ => {}
        }
    }
    assert_eq!(reasoning, "Thinking");
    assert!(failed);
}

#[tokio::test]
async fn prompt_error_and_interrupt_before_busy_still_settle() {
    let mut wire = TurnWire::start(false).await;
    wire.request("/prompt_async").await;
    wire.bus.send(json!({"type":"session.error", "properties":{"sessionID":"fixture", "error":{"name":"ProviderError", "data":{"message":"bad model"}}}})).unwrap();
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Errored);

    let mut wire = TurnWire::start(false).await;
    wire.request("/prompt_async").await;
    wire.interrupt.cancel();
    wire.request("/abort").await;
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Interrupted);
}

#[tokio::test]
async fn v2_model_selection_and_prompt_use_the_documented_bodies() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    let posts = wire.posts.lock().unwrap();
    assert_eq!(
        posts
            .iter()
            .find(|(p, _)| p == "/api/session/fixture/model")
            .unwrap()
            .1,
        json!({"model":{"providerID":"opencode","id":"muse","variant":"low"}})
    );
    assert_eq!(
        posts
            .iter()
            .find(|(p, _)| p == "/api/session/fixture/prompt")
            .unwrap()
            .1,
        json!({"text":"first","files":[]})
    );
}

#[tokio::test]
async fn v2_agent_selection_is_validated_and_sent_before_prompt() {
    let mut wire = TurnWire::start_agent(false, true, true, None, Some("team/review")).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    let posts = wire.posts.lock().unwrap();
    let create = posts
        .iter()
        .position(|(path, _)| path == "/api/session")
        .unwrap();
    let prompt = posts
        .iter()
        .position(|(path, _)| path == "/api/session/fixture/prompt")
        .unwrap();
    assert!(create < prompt);
    assert_eq!(posts[create].1["agent"], "team/review");
    assert!(posts[prompt].1.get("agents").is_none());
}

#[tokio::test]
async fn v2_agents_filter_hidden_and_subagent_rows() {
    let wire = TurnWire::start_proto(false, true).await;
    let agents = OpencodeHarness::new()
        .with_base_url(wire.base.clone())
        .agents(None)
        .await
        .unwrap();
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>(),
        ["build", "team/review"]
    );
    drop(wire);
    let mut wire = TurnWire::start_agent(false, true, true, None, Some("explore")).await;
    assert!(matches!(wire.done().await.0, DoneStatus::Errored));
    assert!(wire.posts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn modern_catalog_uses_directory_query_and_keeps_projects_separate() {
    let wire = TurnWire::start_case(WireCase {
        v2: true,
        modern: true,
        auto_approve: true,
        cwd: "/one",
        ..Default::default()
    })
    .await;
    let harness = OpencodeHarness::new().with_base_url(wire.base.clone());
    assert_eq!(harness.test_connection().await.unwrap().version, "2.0.7");
    let one = harness.models_for_directory(Some("/one")).await.unwrap();
    let two = harness.models_for_directory(Some("/two")).await.unwrap();
    assert!(one.iter().any(|model| model.id == "custom/one/model"));
    assert!(two.iter().any(|model| model.id == "custom/two/model"));
    assert!(!two.iter().any(|model| model.id == "custom/one/model"));
    assert_eq!(
        harness.models_for_directory(Some("/one")).await.unwrap(),
        one
    );
}

#[tokio::test]
async fn resumed_agent_switch_precedes_prompt_and_failure_blocks_prompt() {
    let mut wire = TurnWire::start_case(WireCase {
        v2: true,
        modern: true,
        auto_approve: true,
        agent: Some("team/review"),
        resume: true,
        ..Default::default()
    })
    .await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    let posts = wire.posts.lock().unwrap();
    let switch = posts
        .iter()
        .position(|(path, _)| path == "/api/session/fixture/agent")
        .unwrap();
    let prompt = posts
        .iter()
        .position(|(path, _)| path == "/api/session/fixture/prompt")
        .unwrap();
    assert!(switch < prompt);
    assert_eq!(posts[switch].1, json!({"agent":"team/review"}));
    assert!(posts.iter().all(|(path, _)| path != "/api/session"));
    drop(posts);
    drop(wire);

    let mut failed = TurnWire::start_case(WireCase {
        v2: true,
        modern: true,
        auto_approve: true,
        agent: Some("team/review"),
        resume: true,
        fail_agent: true,
        ..Default::default()
    })
    .await;
    assert_eq!(failed.done().await.0, DoneStatus::Errored);
    let posts = failed.posts.lock().unwrap();
    assert!(
        posts
            .iter()
            .any(|(path, _)| path == "/api/session/fixture/agent")
    );
    assert!(
        posts
            .iter()
            .all(|(path, _)| path != "/api/session/fixture/prompt" && path != "/api/session")
    );
}

#[test]
fn attached_resume_tokens_are_server_bound_and_credential_free() {
    let first = Server::attached(&OpencodeConnection {
        base_url: "http://localhost:4096/opencode/".into(),
        username: "alice".into(),
        password: Some("secret".into()),
    })
    .unwrap();
    let same = Server::attached(&OpencodeConnection {
        base_url: "http://localhost:4096/opencode".into(),
        username: "bob".into(),
        password: Some("different".into()),
    })
    .unwrap();
    let other = Server::attached(&OpencodeConnection {
        base_url: "http://localhost:4096/other".into(),
        username: "alice".into(),
        password: None,
    })
    .unwrap();
    let token = first.public_session_id("ses_1");
    assert!(!token.contains("secret"));
    assert_eq!(same.resume_id(&token).unwrap(), "ses_1");
    assert!(other.resume_id(&token).is_err());
    assert!(first.resume_id("ses_1").is_err());
}

#[tokio::test]
async fn v2_pending_tool_overflow_fails_the_run_instead_of_growing_forever() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    for i in 0..=MAX_PENDING_V2_TOOLS {
        wire.v2("session.tool.input.started",json!({"sessionID":"other","assistantMessageID":"m","id":format!("c{i}"),"name":"read"}));
    }
    assert_eq!(wire.done().await.0, DoneStatus::Errored);
}

#[tokio::test]
async fn v2_external_interrupt_is_not_reported_as_success() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2(
        "session.execution.interrupted",
        json!({"sessionID":"fixture","reason":"shutdown"}),
    );
    assert_eq!(wire.done().await.0, DoneStatus::Interrupted);
}

#[tokio::test]
async fn v2_recovered_step_failure_does_not_poison_successful_execution() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID":"fixture"}));
    wire.v2(
        "session.step.failed",
        json!({"sessionID":"fixture","error":{"type":"provider.rate_limit","message":"Retrying"}}),
    );
    wire.v2(
        "session.step.started",
        json!({"sessionID":"fixture","assistantMessageID":"recovered"}),
    );
    wire.v2("session.text.ended",json!({"sessionID":"fixture","assistantMessageID":"recovered","ordinal":0,"text":"Recovered"}));
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID":"fixture"}),
    );
    let (status, text) = wire.done().await;
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(text, "Recovered");
}
