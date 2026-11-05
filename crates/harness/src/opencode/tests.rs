use super::*;

#[derive(Clone, Copy)]
enum NativeCommandReply {
    Http404,
    Disconnect,
    DelayedHttp404,
}

/// Real HTTP/SSE transport with explicitly ordered turn events. No provider or
/// installed CLI is involved, so duplicate completion frames are reproducible.
struct TurnWire {
    bus: mpsc::UnboundedSender<Value>,
    posts: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    requests: mpsc::UnboundedReceiver<String>,
    events: mpsc::Receiver<Result<AgentEvent, HarnessError>>,
    interrupt: tokio_util::sync::CancellationToken,
    turn: crate::TurnControl,
    steering: Option<mpsc::Sender<crate::SteerMessage>>,
    polls: Arc<std::sync::atomic::AtomicUsize>,
    /// SSE connections accepted (a reconnect adds one).
    bus_connections: Arc<std::sync::atomic::AtomicUsize>,
    /// Pending-request list GETs answered.
    listings: Arc<std::sync::atomic::AtomicUsize>,
    /// The `pending` lists served to a reconnect sweep.
    pending: Arc<std::sync::Mutex<Value>>,
    /// Every question put to the person, in order.
    asked: Arc<std::sync::Mutex<Vec<UserInputQuestion>>>,
    command_failure_release: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
    run: tokio::task::JoinHandle<()>,
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
        Self::start_config(queued, v2, auto_approve, answer, "2.0.3", json!({}), false).await
    }

    async fn start_config(
        queued: bool,
        v2: bool,
        auto_approve: bool,
        answer: Option<bool>,
        version: &'static str,
        overrides: Value,
        command_failure: bool,
    ) -> Self {
        Self::start_fixture(
            queued,
            v2,
            auto_approve,
            answer,
            version,
            overrides,
            command_failure,
            None,
        )
        .await
    }

    async fn start_native_command(queued: bool, reply: NativeCommandReply) -> Self {
        Self::start_fixture(
            queued,
            false,
            true,
            None,
            "2.0.3",
            json!({}),
            false,
            Some(reply),
        )
        .await
    }

    async fn start_fixture(
        queued: bool,
        v2: bool,
        auto_approve: bool,
        answer: Option<bool>,
        version: &'static str,
        overrides: Value,
        command_failure: bool,
        native_command_reply: Option<NativeCommandReply>,
    ) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (bus, bus_rx) = mpsc::unbounded_channel::<Value>();
        let bus_rx = Arc::new(tokio::sync::Mutex::new(bus_rx));
        let bus_connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connected = bus_connections.clone();
        let (superseded, _) = tokio::sync::watch::channel(0_usize);
        let superseded = Arc::new(superseded);
        let pending: Arc<std::sync::Mutex<Value>> =
            Arc::new(std::sync::Mutex::new(overrides["pending"].clone()));
        let listed = pending.clone();
        let sweeps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listings = sweeps.clone();
        let (request_tx, requests) = mpsc::unbounded_channel();
        let posts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = posts.clone();
        let hold_prompt = overrides["holdPrompt"].as_bool().unwrap_or(false);
        let drop_prompt = overrides["dropPrompt"].as_bool().unwrap_or(false);
        let message_exists = overrides["messageExists"].as_bool().unwrap_or(false);
        let fail_replies = overrides["failReplies"].as_bool().unwrap_or(false);
        let busy_polls = overrides["busyPolls"].as_u64().unwrap_or(0) as usize;
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_polls = polls.clone();
        let (command_failure_release, command_failure_wait) = tokio::sync::oneshot::channel();
        let command_failure_wait = Arc::new(tokio::sync::Mutex::new(Some(command_failure_wait)));
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let bus_rx = bus_rx.clone();
                let request_tx = request_tx.clone();
                let recorded = recorded.clone();
                let polls = server_polls.clone();
                let command_failure_wait = command_failure_wait.clone();
                let connected = connected.clone();
                let superseded = superseded.clone();
                let listed = listed.clone();
                let listed_count = sweeps.clone();
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
                        // A reconnect supersedes the previous stream before
                        // it consumes another event; `"close"` ends one.
                        let generation = connected.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        superseded.send_replace(generation);
                        let mut superseded = superseded.subscribe();
                        let mut events = bus_rx.lock().await;
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: connected\n\n").await.unwrap();
                        loop {
                            tokio::select! {
                                event = events.recv() => {
                                    let Some(event) = event else { return };
                                    if event == "close" { return; }
                                    if socket.write_all(format!("data: {event}\n\n").as_bytes()).await.is_err() { return; }
                                }
                                _ = superseded.changed() => return,
                            }
                        }
                    }
                    if command_failure && is_post && path.ends_with("/command") {
                        socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nbad command!").await.unwrap();
                        return;
                    }
                    if hold_prompt && (path.ends_with("/prompt_async") || path.ends_with("/prompt")) {
                        let _ = request_tx.send(path);
                        std::future::pending::<()>().await;
                        return;
                    }
                    // The prompt is read, then the connection drops before
                    // any answer: the client cannot tell whether it landed.
                    if drop_prompt && (path.ends_with("/prompt_async") || path.ends_with("/prompt")) {
                        let _ = request_tx.send(path);
                        return;
                    }
                    // The stored prompt, found under the id it was posted with.
                    if message_exists && !is_post && let Some(id) = path.strip_prefix("/session/fixture/message/") {
                        let body = json!({"info": {"id": id, "role": "user"}}).to_string();
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                        return;
                    }
                    if path == "/session/status" || path == "/api/session/active" {
                        let count = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let body = if count < busy_polls { r#"{"fixture":{"type":"busy"}}"# } else { r#"{"fixture":{"type":"idle"}}"# };
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                        return;
                    }
                    let health = json!({"version": version}).to_string();
                    if native_command_reply.is_some() && is_post && path == "/session/fixture/command" {
                        let _ = request_tx.send(path.clone());
                        match native_command_reply.expect("native command fixture configured") {
                            NativeCommandReply::Disconnect => return,
                            NativeCommandReply::DelayedHttp404 => {
                                if let Some(wait) = command_failure_wait.lock().await.take() {
                                    let _ = wait.await;
                                }
                            }
                            NativeCommandReply::Http404 => {}
                        }
                        let body = r#"{"error":"command removed"}"#;
                        socket
                            .write_all(
                                format!(
                                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                )
                                .as_bytes(),
                            )
                            .await
                            .unwrap();
                        return;
                    }
                    if fail_replies && is_post && path.contains("/permission") {
                        let body = r#"{"name":"UnknownError"}"#;
                        socket.write_all(format!("HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                        return;
                    }
                    // Pending-request lists (`pending` override: per kind).
                    let route = path.split('?').next().unwrap_or_default();
                    let kind = match route {
                        "/permission" if !v2 => Some("permission"),
                        "/question" if !v2 => Some("question"),
                        _ if v2 && route.ends_with("/permission") => Some("permission"),
                        _ if v2 && route.ends_with("/form") => Some("form"),
                        _ => None,
                    };
                    if !is_post && let Some(kind) = kind {
                        let list = listed.lock().unwrap()[kind].as_array().cloned().unwrap_or_default();
                        let body = if v2 { json!({"data": list}) } else { json!(list) }.to_string();
                        listed_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                        return;
                    }
                    let missing_resume = path == "/session/missing" || path == "/api/session/missing";
                    let (status, body) = if missing_resume {
                        ("404 Not Found", r#"{"error":"missing"}"#)
                    } else if v2 {
                        match path.as_str() {
                            "/api/health" => ("200 OK", health.as_str()),
                            "/api/session" => ("200 OK", r#"{"data":{"id":"fixture"}}"#),
                            "/api/command" => ("200 OK", r#"{"data":[]}"#),
                            // Non-empty: the catalog-sync retry loop must not stall tests.
                            "/api/model" => ("200 OK", r#"{"data":[{"providerID":"opencode","id":"muse","name":"Muse","limit":{"context":1000},"variants":[{"id":"low"}],"enabled":true},{"providerID":"opencode","id":"long-context","name":"Long Context","limit":{"context":2000},"enabled":true}]}"#),
                            _ => ("200 OK", "{}"),
                        }
                    } else {
                        match path.as_str() {
                            "/global/health" => ("200 OK", r#"{"healthy":true,"version":"1.18.31"}"#),
                            "/session" => ("200 OK", r#"{"id":"fixture"}"#),
                            "/command" => (
                                "200 OK",
                                if native_command_reply.is_some() {
                                    r#"[{"name":"project-review","description":"Review"}]"#
                                } else {
                                    "[]"
                                },
                            ),
                            _ => ("200 OK", "{}"),
                        }
                    };
                    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    if path.ends_with("/prompt_async")
                        || path.ends_with("/prompt")
                        || path.ends_with("/abort")
                        || path.ends_with("/interrupt")
                        || path == "/api/model"
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
                    attachments: Vec::new(),
                    config: None,
                })
                .await
                .unwrap();
        }
        let retained_steering = overrides["keepSteering"]
            .as_bool()
            .unwrap_or(false)
            .then_some(steer_tx);
        let interrupt = tokio_util::sync::CancellationToken::new();
        let turn = crate::TurnControl::default();
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let questions_asked = asked.clone();
        let mut request = json!({"prompt": if native_command_reply.is_some() { "/project-review" } else { "first" }, "cwd":"", "sandbox":"workspace-write", "autoApprove": auto_approve, "model": if v2 { Some("opencode/muse") } else { None }, "reasoning": "low"});
        request
            .as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        let run = tokio::spawn(run_session(Session {
            server: Server::attached(base),
            event_tx,
            controls: RunControls {
                realtime: None,
                execution_lease: None,
                request_input: Box::new(move |questions| {
                    let answer = answer.expect("fixture must not ask for input");
                    questions_asked.lock().unwrap().extend(questions.clone());
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(
                        questions
                            .into_iter()
                            .map(|q| UserInputAnswer {
                                question_id: q.id,
                                labels: vec![if answer { "Yes" } else { "No" }.into()],
                            })
                            .collect(),
                    );
                    rx
                }),
                steering,
                interrupt: interrupt.clone(),
                turn: turn.clone(),
            },
            request: serde_json::from_value(request).unwrap(),
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_millis(50),
            // A test stepping a paused clock past the bus's silence bound
            // would otherwise trip the wedged-server probe, whose answer
            // (real loopback IO) the stepped clock outruns.
            bus_timeouts: if overrides["noBusWatchdog"].as_bool() == Some(true) {
                let never = Duration::from_secs(24 * 3600);
                BusTimeouts {
                    silence: never,
                    probe: never,
                    wedged: never,
                }
            } else {
                BusTimeouts::default()
            },
            known_commands: Some(vec![SlashCommand {
                name: if native_command_reply.is_some() {
                    "project-review"
                } else {
                    "test"
                }
                .into(),
                description: String::new(),
                input_hint: None,
            }]),
            initial_native_command_selected: native_command_reply.is_some(),
        }));
        Self {
            bus,
            posts,
            requests,
            events,
            interrupt,
            turn,
            steering: retained_steering,
            polls,
            bus_connections,
            listings,
            pending,
            asked,
            command_failure_release: matches!(
                native_command_reply,
                Some(NativeCommandReply::DelayedHttp404)
            )
            .then_some(command_failure_release),
            server,
            run,
        }
    }

    /// Requests whose relative order the fixture's concurrent HTTP handlers
    /// do not preserve.
    async fn requests_any_order(&mut self, suffixes: &[&str]) {
        let mut seen = Vec::new();
        for _ in suffixes {
            seen.push(
                tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        for suffix in suffixes {
            assert!(
                seen.iter().any(|path| path.ends_with(suffix)),
                "missing {suffix}: {seen:?}"
            );
        }
    }

    async fn request(&mut self, suffix: &str) {
        let path = tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(path.ends_with(suffix), "unexpected request: {path}");
    }

    async fn posted(&self, suffix: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some((_, body)) = self
                    .posts
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(path, _)| path.ends_with(suffix))
                {
                    return body.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("expected HTTP reply")
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
async fn completed_turn_keeps_mailbox_alive_for_the_next_queued_request() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.21",
        json!({ "keepSteering": true }),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.status("idle");
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    assert!(!wire.run.is_finished());
    wire.steering
        .as_ref()
        .unwrap()
        .send(crate::SteerMessage {
            prompt: "after completion".into(),
            message_id: Some("second".into()),
            attachments: Vec::new(),
            config: None,
        })
        .await
        .unwrap();
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.status("idle");
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    assert!(!wire.run.is_finished());
    drop(wire.steering.take());
    tokio::time::timeout(Duration::from_secs(5), &mut wire.run)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn queued_turn_ignores_previous_turn_duplicate_idle() {
    for status_first in [true, false] {
        let mut wire = TurnWire::start(true).await;
        // The queued steer preempts the generation at once.
        wire.requests_any_order(&["/prompt_async", "/abort"]).await;
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
async fn native_command_http_failures_settle_the_current_turn() {
    for (reply, expected_status) in [
        (NativeCommandReply::Http404, Some("404 Not Found")),
        (NativeCommandReply::Disconnect, None),
    ] {
        let mut wire = TurnWire::start_native_command(false, reply).await;
        wire.request("/command").await;
        let (status, error) = tokio::time::timeout(Duration::from_secs(5), async {
            let mut surfaced = None;
            loop {
                match wire.events.recv().await.unwrap().unwrap() {
                    AgentEvent::Error { message } => surfaced = Some(message),
                    AgentEvent::Done { status, error, .. } => {
                        return (status, error.or(surfaced));
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(status, DoneStatus::Errored);
        let error = error.expect("native command HTTP failure is surfaced");
        assert!(error.contains("opencode POST /session/fixture/command:"));
        if let Some(expected_status) = expected_status {
            assert!(error.contains(expected_status), "{error}");
        } else {
            assert!(!error.contains("404 Not Found"), "{error}");
        }
        assert!(
            !wire
                .posts
                .lock()
                .unwrap()
                .iter()
                .any(|(path, _)| path.ends_with("/prompt_async")),
            "failed native command must not fall back to an ordinary prompt"
        );
    }
}

#[tokio::test]
async fn late_native_command_failure_does_not_poison_the_queued_turn() {
    let mut wire = TurnWire::start_native_command(true, NativeCommandReply::DelayedHttp404).await;
    // The queued steer preempts the generation at once.
    wire.requests_any_order(&["/command", "/abort"]).await;
    wire.status("busy");
    wire.status("idle");
    wire.request("/prompt_async").await;

    // The synchronous command endpoint returns only after the event bus has
    // settled its turn and the queued ordinary prompt owns a new generation.
    wire.command_failure_release
        .take()
        .unwrap()
        .send(())
        .unwrap();
    tokio::task::yield_now().await;

    wire.status("busy");
    wire.bus.send(json!({"type":"message.updated", "properties":{"info":{"id":"answer", "sessionID":"fixture", "role":"assistant"}}})).unwrap();
    wire.bus.send(json!({"type":"message.part.updated", "properties":{"part":{"id":"text", "messageID":"answer", "sessionID":"fixture", "type":"text", "text":"SECOND_OK"}}})).unwrap();
    wire.status("idle");
    let (status, text) = wire.done().await;
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(text, "SECOND_OK");
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
    // Each step reports its own tokens; `session.usage.updated` follows with
    // the cumulative session totals, which must not read as occupancy.
    let step = |wire: &TurnWire, message: &str, model: &str, tokens: Value, cumulative: Value| {
        wire.v2(
            "session.step.started",
            json!({
                "sessionID": "fixture",
                "assistantMessageID": message,
                "model": {"id": model, "providerID": "opencode"},
            }),
        );
        wire.v2(
            "session.step.ended",
            json!({
                "sessionID": "fixture", "assistantMessageID": message,
                "finish": "stop", "cost": 0, "tokens": tokens,
            }),
        );
        wire.v2(
            "session.usage.updated",
            json!({"sessionID": "fixture", "cost": 0, "tokens": cumulative}),
        );
    };
    wire.v2(
        "session.step.ended",
        json!({
            "sessionID": "fixture", "assistantMessageID": "msg_a",
            "finish": "stop", "cost": 0,
            "tokens": {"input": 10, "output": 2, "reasoning": 0,
                       "cache": {"read": 0, "write": 0}}
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
    // A model switch resolves the new model's advertised window.
    step(
        &wire,
        "msg_b",
        "long-context",
        json!({"input": 20, "output": 3, "reasoning": 0, "cache": {"read": 5, "write": 0}}),
        json!({"input": 30, "output": 5, "reasoning": 0, "cache": {"read": 5, "write": 0}}),
    );
    // A step without reported usage is not an empty context.
    step(
        &wire,
        "msg_c",
        "long-context",
        json!({"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}),
        json!({"input": 30, "output": 5, "reasoning": 0, "cache": {"read": 5, "write": 0}}),
    );
    // After compaction the next prompt is smaller; the totals keep climbing.
    step(
        &wire,
        "msg_d",
        "long-context",
        json!({"input": 4, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}}),
        json!({"input": 34, "output": 6, "reasoning": 0, "cache": {"read": 5, "write": 0}}),
    );
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID": "fixture"}),
    );

    let (status, text, usage, context_usage) =
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut text = String::new();
            let mut usage = None;
            let mut context_usage = Vec::new();
            loop {
                match wire.events.recv().await.unwrap().unwrap() {
                    AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                    AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        usage = Some((input_tokens, output_tokens));
                    }
                    AgentEvent::ContextUsage { tokens, window } => {
                        context_usage.push((tokens, window));
                    }
                    AgentEvent::Done { status, .. } => {
                        return (status, text, usage, context_usage);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(status, DoneStatus::Completed);
    assert_eq!(text, "PONG");
    assert_eq!(usage, Some((4, 1)));
    assert_eq!(
        context_usage,
        vec![
            (Some(12), Some(1000)),
            (Some(28), Some(2000)),
            (Some(5), Some(2000)),
        ]
    );
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

async fn read_http_request_headers(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(socket.read_u8().await.unwrap());
        assert!(headers.len() <= 8192, "unexpectedly large request headers");
    }
}

/// The loopback `opencode serve` gets our Basic-auth password on every call,
/// so a system/env proxy must never see that traffic.
#[test]
fn http_client_never_proxies_the_loopback_server() {
    const NAME: &str = "opencode::tests::http_client_never_proxies_the_loopback_server";
    const CHILD: &str = "ZERON_OPENCODE_PROXY_PROBE";
    if std::env::var_os(CHILD).is_none() {
        // reqwest reads the proxy from the environment, and mutating it here
        // would race sibling tests, so run the body in a child process.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--test-threads=1"])
            .env(CHILD, "1")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("http_proxy", "http://127.0.0.1:1")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "child probe failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/global/health", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request_headers(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        // Control: a default client does route loopback through the dead
        // proxy, so this test cannot pass vacuously.
        assert!(reqwest::Client::new().get(&url).send().await.is_err());
        let response = http_client()
            .get(&url)
            .send()
            .await
            .expect("opencode client must reach loopback directly");
        assert!(response.status().is_success());
    });
}

#[tokio::test]
async fn catalog_decodes_fragmented_http_without_retaining_unused_fields() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = json!({
        "all": [{"id":"test", "models":{"model":{"name":"模型", "variants":{"high":{"unused":"x".repeat(128 * 1024)}}}}}],
        "connected":["test"]
    }).to_string();
    let expected: ProviderCatalog = serde_json::from_str(&body).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request_headers(&mut socket).await;
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
        read_http_request_headers(&mut socket).await;
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
        Some("msg_1"),
    );
    assert_eq!(body["messageID"], "msg_1");
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
    let mut turn = TurnState::begin(None, false);
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

/// A running tool that takes no arguments is open too: steering preempts
/// only when no tool is open, so an unseen one would be aborted.
#[test]
fn a_running_tool_without_arguments_opens_before_it_completes() {
    let mut feed = feed_with_assistant("msg_a");
    let pending = json!({
        "id": "prt_w", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "tool", "tool": "slow_slow_wait", "callID": "call-w",
        "state": {"status": "pending", "input": {}},
    });
    assert!(part_snapshot_events(&mut feed, &pending, true, None).is_empty());
    let running = json!({
        "id": "prt_w", "messageID": "msg_a", "sessionID": "ses_1",
        "type": "tool", "tool": "slow_slow_wait", "callID": "call-w",
        "state": {"status": "running", "input": {}},
    });
    let events = part_snapshot_events(&mut feed, &running, true, None);
    assert!(
        matches!(events.as_slice(), [AgentEvent::ToolCall { id, .. }] if id == "call-w"),
        "{events:?}"
    );
}

#[test]
fn task_spawn_registers_child_by_metadata_and_completion_settles() {
    for name in ["task", "subagent"] {
        let mut feed = feed_with_assistant("msg_a");
        let mut children = HashMap::new();
        let mut pending = VecDeque::new();
        let mut unbound = HashMap::new();
        let running = json!({
            "id": "prt_task", "messageID": "msg_a", "sessionID": "ses_parent",
            "type": "tool", "tool": name,
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
            "type": "tool", "tool": name,
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
    let pairs = map_questions(&props);
    assert_eq!(pairs.len(), 1);
    let (key, question) = &pairs[0];
    // A key-less 1.x question falls back to the positional form key.
    assert_eq!(key, "q0");
    // Panel ids are minted per question, never taken from the wire.
    assert_ne!(question.id, "q0");
    assert_ne!(question.id, map_questions(&props)[0].1.id);
    assert_eq!(question.header, "Color");
    assert_eq!(question.question, "Which color?");
    assert_eq!(question.options, vec!["Red", "Blue"]);
    assert!(question.multi_select);
}

/// A live 2.0.24 `form.created` frame (the `question` tool's transport)
/// folds into the same bus payload the 1.x question path consumes.
#[test]
fn v2_question_forms_fold_into_the_input_panel_shape() {
    let mut tools = HashMap::new();
    let out = normalize_v2_frame(
        json!({"id":"evt_form","type":"form.created","data":{"form":{
        "id":"frm_116e46801001",
        "sessionID":"ses_1",
        "title":"Questions",
        "metadata":{"kind":"question","tool":{"messageID":"msg_1","id":"call_1"}},
        "fields":[
            {"key":"q0","title":"Color","description":"Which color?","type":"string",
             "options":[{"value":"Red","label":"Red","description":"warm"},
                        {"value":"Blue","label":"Blue","description":"cool"}],
             "custom":true},
            {"key":"q1","title":"Toppings","description":"Pick some","type":"multiselect",
             "options":[{"value":"A","label":"A"},{"value":"B","label":"B"}],"custom":true}
        ]}}}),
        &mut tools,
    );
    assert_eq!(
        out,
        vec![json!({"type":"question.asked","properties":{
        "id":"frm_116e46801001","sessionID":"ses_1","form":true,
        "questions":[
            {"key":"q0","header":"Color","question":"Which color?","multiple":false,
             "options":[{"label":"Red"},{"label":"Blue"}]},
            {"key":"q1","header":"Toppings","question":"Pick some","multiple":true,
             "options":[{"label":"A"},{"label":"B"}]}
        ]}})]
    );
    // Forms from other features (no question metadata) never reach the panel.
    assert!(
        normalize_v2_frame(
            json!({"id":"evt_other","type":"form.created","data":{"form":{
                "id":"frm_2","sessionID":"ses_1","title":"MCP",
                "fields":[{"key":"url","title":"URL","type":"string"}]}}}),
            &mut tools,
        )
        .is_empty()
    );
}

/// Form answers key by field: one label for a single-select field, the label
/// array for a multiselect.
#[test]
fn form_answers_key_by_field_with_multiselect_arrays() {
    let pairs = map_questions(&json!({"questions":[
        {"key":"q0","question":"Which color?","header":"Color","options":[],"multiple":false},
        {"key":"q1","question":"Pick some","header":"Toppings","options":[],"multiple":true}
    ]}));
    let answers = vec![
        UserInputAnswer {
            question_id: pairs[0].1.id.clone(),
            labels: vec!["Red".into()],
        },
        UserInputAnswer {
            question_id: pairs[1].1.id.clone(),
            labels: vec!["A".into(), "B".into()],
        },
    ];
    assert_eq!(
        form_answer_body(&pairs, &answers),
        json!({"answer":{"q0":"Red","q1":["A","B"]}})
    );
    // An unanswered single-select degrades to an empty string, never an array.
    assert_eq!(
        form_answer_body(&pairs, &[]),
        json!({"answer":{"q0":"","q1":[]}})
    );
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
            {"content": "step two", "status": "in_progress"},
            {"content": "step three", "status": "pending"},
            {"content": "step four", "status": "cancelled"},
        ]}),
    );
    assert_eq!(
        call,
        ToolCall::Todo {
            items: vec![
                TodoItem::new("step one", TodoStatus::Completed),
                TodoItem::new("step two", TodoStatus::InProgress),
                TodoItem::new("step three", TodoStatus::Pending),
                TodoItem::new("step four", TodoStatus::Pending),
            ]
        }
    );
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
fn canonical_command_removed_after_discovery_is_not_downgraded_to_prompt_text() {
    use zeron_proto::invocation::{Invocation, harness_prompt};

    let discovered = commands_from_wire(&json!([{
        "name": "project-review",
        "description": "Review this project"
    }]));
    let canonical = Invocation::Command {
        name: discovered[0].name.clone(),
    }
    .link();
    assert!(selected_native_command(&canonical, HarnessId::Opencode));
    let delivered = harness_prompt(&canonical, HarnessId::Opencode);

    // The project-scoped catalog changed after composer discovery. A
    // canonical selection retains command intent and fails explicitly.
    let live = commands_from_wire(&json!([]));
    let error = native_command_request(&delivered, &live, true).unwrap_err();
    assert_eq!(
        error.to_string(),
        "harness protocol error: The selected OpenCode command /project-review is no longer available in this project"
    );

    // Identical raw slash text was never a composer selection, so it keeps
    // the historical ordinary-prompt fallback.
    assert!(
        native_command_request(&delivered, &live, false)
            .unwrap()
            .is_none()
    );
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
    // A step's own usage reaches the engine as an assistant message.updated.
    let out = normalize_v2_frame(
        json!({"id":"evt_9","type":"session.step.ended","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a","finish":"stop","cost":0,
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
    // Cumulative session totals are spend, not occupancy; malformed step
    // usage is dropped.
    for data in [
        json!({"id":"evt_10","type":"session.usage.updated","data":{
            "sessionID":"ses_1","cost":0,"tokens":{"input":10,"output":2}}}),
        json!({"id":"evt_11","type":"session.step.ended","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a","tokens":"12"}}),
        json!({"id":"evt_11","type":"session.step.ended","data":{
            "sessionID":"ses_1","assistantMessageID":"msg_a"}}),
    ] {
        assert_eq!(normalize_v2_frame(data, &mut tools), Vec::<Value>::new());
    }
    // step.ended omits model identity; the session's latest step supplies
    // it so the context window resolves downstream.
    let mut session_models = HashMap::new();
    let step = normalize_v2_frame_with_session_models(
        json!({"id":"evt_12","type":"session.step.started","data":{
            "sessionID":"ses_2","assistantMessageID":"msg_b",
            "model":{"id":"long-context","providerID":"opencode"}}}),
        &mut tools,
        &mut session_models,
    );
    assert_eq!(step.len(), 1);
    let out = normalize_v2_frame_with_session_models(
        json!({"id":"evt_13","type":"session.step.ended","data":{
            "sessionID":"ses_2","assistantMessageID":"msg_b","finish":"stop","cost":0,
            "tokens":{"input":1,"output":2}}}),
        &mut tools,
        &mut session_models,
    );
    assert_eq!(
        out,
        vec![json!({"type":"message.updated","properties":{
            "info":{"sessionID":"ses_2","id":"usage","role":"assistant",
                    "tokens":{"input":1,"output":2},
                    "providerID":"opencode","modelID":"long-context"}}})]
    );
    // A malformed model leaves the cached one in place.
    for model in [
        json!(null),
        json!("opencode/muse"),
        json!({"id":"", "providerID":"opencode"}),
    ] {
        normalize_v2_frame_with_session_models(
            json!({"id":"evt_14","type":"session.step.started","data":{
                "sessionID":"ses_2","assistantMessageID":"msg_c","model":model}}),
            &mut tools,
            &mut session_models,
        );
    }
    assert_eq!(
        session_models.get("ses_2").map(|m| m.model_id.as_str()),
        Some("long-context")
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
    let body = prompt_body_v2("hello", &["/tmp/shot.png".to_owned()], Some("msg_1"));
    assert_eq!(body["text"], "hello");
    assert_eq!(body["id"], "msg_1");
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
        let mut wire = TurnWire::start_policy(false, v2, false, None).await;
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
async fn permissions_always_approve_without_user_input() {
    for version in ["2.0.0", "2.0.3", "2.0.4", "2.0.11"] {
        for auto_approve in [false, true] {
            // No input callback is available: any permission prompt fails the fixture.
            let mut wire =
                TurnWire::start_config(false, true, auto_approve, None, version, json!({}), false)
                    .await;
            wire.request("/api/model").await;
            wire.request("/prompt").await;
            let key = if version == "2.0.0" || version == "2.0.3" {
                "reply"
            } else {
                "decision"
            };
            for id in ["first", "second", "third"] {
                wire.v2("permission.asked", json!({"id":id, "sessionID":"fixture"}));
                let body = wire.posted(&format!("/permission/{id}/reply")).await;
                assert_eq!(body, json!({key: "once"}));
            }
        }
    }
}

#[tokio::test]
async fn permissions_do_not_auto_answer_agent_questions() {
    let mut wire = TurnWire::start_policy(false, false, false, Some(false)).await;
    wire.request("/prompt_async").await;
    wire.bus.send(json!({"type": "question.asked", "properties": {
        "id": "question", "sessionID": "fixture", "questions": [{
            "header": "Choice", "question": "Continue?", "options": [{"label": "No"}, {"label": "Yes"}]
        }]
    }})).unwrap();
    let body = wire.posted("/question/question/reply").await;
    assert_eq!(body, json!({"answers": [["No"]]}));
}

/// Waits for the fixture's first prompt, on either wire.
async fn first_prompt(wire: &mut TurnWire, v2: bool) {
    if v2 {
        wire.request("/api/model").await;
    }
    wire.request(if v2 { "/prompt" } else { "/prompt_async" })
        .await;
}

/// An edit then a command, as 1.x asks them (`permission`, `patterns`,
/// `metadata`; observed live, 1.19.4) or 2.x (`action`, `resources`).
fn ask_edit_and_command(wire: &TurnWire, v2: bool) {
    for (id, permission, pattern, metadata) in [
        (
            "per_edit",
            "edit",
            "src/main.rs",
            json!({"filepath": "/work/src/main.rs"}),
        ),
        (
            "per_bash",
            "bash",
            "cargo test",
            json!({"command": "cargo test"}),
        ),
    ] {
        if v2 {
            wire.v2(
                "permission.asked",
                json!({"id": id, "sessionID": "fixture", "action": permission,
                       "resources": [pattern], "metadata": metadata}),
            );
        } else {
            wire.bus
                .send(json!({"type": "permission.asked", "properties": {
                    "id": id, "sessionID": "fixture", "permission": permission,
                    "patterns": [pattern], "metadata": metadata, "always": ["*"]
                }}))
                .unwrap();
        }
    }
}

fn asked(wire: &TurnWire) -> Vec<String> {
    wire.asked
        .lock()
        .unwrap()
        .iter()
        .map(|q| q.question.clone())
        .collect()
}

#[tokio::test]
async fn supervised_asks_before_every_permission_and_rejects_on_no() {
    for v2 in [false, true] {
        for (answer, reply) in [(true, "once"), (false, "reject")] {
            let overrides = json!({"access": "supervised"});
            let mut wire =
                TurnWire::start_config(false, v2, false, Some(answer), "2.0.3", overrides, false)
                    .await;
            first_prompt(&mut wire, v2).await;
            ask_edit_and_command(&wire, v2);
            for id in ["per_edit", "per_bash"] {
                let body = wire.posted(&format!("/permission/{id}/reply")).await;
                assert_eq!(body, json!({"reply": reply}), "{id} (v2: {v2})");
            }
            assert_eq!(
                asked(&wire),
                [
                    "OpenCode wants to modify /work/src/main.rs. Allow it?",
                    "OpenCode wants to run `cargo test`. Allow it?",
                ]
            );
        }
    }
}

#[tokio::test]
async fn auto_accept_edits_approves_edits_and_asks_before_commands() {
    for v2 in [false, true] {
        let overrides = json!({"access": "auto-accept-edits"});
        let mut wire =
            TurnWire::start_config(false, v2, false, Some(false), "2.0.3", overrides, false).await;
        first_prompt(&mut wire, v2).await;
        ask_edit_and_command(&wire, v2);
        let edit = wire.posted("/permission/per_edit/reply").await;
        let bash = wire.posted("/permission/per_bash/reply").await;
        assert_eq!(edit, json!({"reply": "once"}));
        assert_eq!(bash, json!({"reply": "reject"}));
        assert_eq!(
            asked(&wire),
            ["OpenCode wants to run `cargo test`. Allow it?"]
        );
    }
}

/// Full access, or `auto_approve` whatever the mode, approves without
/// asking: the fixture has no answer, so a question would fail it.
#[tokio::test]
async fn full_access_approves_every_permission_without_asking() {
    for (auto_approve, overrides) in [
        (false, json!({})),
        (false, json!({"access": "full-access"})),
        (true, json!({"access": "supervised"})),
    ] {
        let mut wire =
            TurnWire::start_config(false, false, auto_approve, None, "2.0.3", overrides, false)
                .await;
        first_prompt(&mut wire, false).await;
        ask_edit_and_command(&wire, false);
        for id in ["per_edit", "per_bash"] {
            let body = wire.posted(&format!("/permission/{id}/reply")).await;
            assert_eq!(body, json!({"reply": "once"}));
        }
        assert!(asked(&wire).is_empty());
    }
}

#[test]
fn permission_questions_name_what_is_asked() {
    let question = |props: Value, dir: Option<&str>| permission_question(&props, dir).question;
    assert_eq!(
        question(
            json!({"permission": "write", "patterns": ["src/a.rs"],
                   "metadata": {"filepath": "/work/src/a.rs"}}),
            Some("/work"),
        ),
        "OpenCode wants to modify src/a.rs. Allow it?"
    );
    // apply_patch: several files, no single filepath.
    assert_eq!(
        question(
            json!({"permission": "edit", "patterns": ["a.rs", "b.rs"]}),
            None
        ),
        "OpenCode wants to modify a.rs, b.rs. Allow it?"
    );
    assert_eq!(
        question(json!({"action": "bash", "resources": ["git push"]}), None),
        "OpenCode wants to run `git push`. Allow it?"
    );
    assert_eq!(
        question(
            json!({"permission": "external_directory", "patterns": ["/etc/*"],
                   "metadata": {"filepath": "/etc/hosts", "parentDir": "/etc"}}),
            Some("/work"),
        ),
        "OpenCode wants to access /etc outside the project. Allow it?"
    );
    assert_eq!(
        question(
            json!({"permission": "webfetch", "patterns": ["https://example.test"]}),
            None,
        ),
        "OpenCode wants to use webfetch (https://example.test). Allow it?"
    );
}

/// A steer adopting `plan`, as the engine sends a changed configuration.
fn plan_config(plan: bool) -> crate::SteerMessage {
    let config: RunRequest = serde_json::from_value(json!({
        "prompt": "", "cwd": "", "sandbox": "workspace-write", "plan": plan
    }))
    .unwrap();
    crate::SteerMessage {
        prompt: format!("plan {plan}"),
        message_id: None,
        attachments: Vec::new(),
        config: Some(Box::new(config)),
    }
}

/// 1.x names the plan agent on each prompt in plan mode and none outside
/// it (the server's default agent then runs, verified live on 1.19.4).
#[tokio::test]
async fn plan_mode_names_the_plan_agent_on_each_v1_prompt() {
    let overrides = json!({"plan": true, "keepSteering": true});
    let mut wire =
        TurnWire::start_config(false, false, true, None, "2.0.3", overrides, false).await;
    wire.request("/prompt_async").await;
    for plan in [false, true] {
        wire.status("busy");
        wire.status("idle");
        wire.idle();
        assert_eq!(wire.done().await.0, DoneStatus::Completed);
        let steering = wire.steering.as_ref().unwrap();
        steering.send(plan_config(plan)).await.unwrap();
        wire.request("/prompt_async").await;
    }
    let agents: Vec<Value> = wire
        .posts
        .lock()
        .unwrap()
        .iter()
        .filter(|(path, _)| path.ends_with("/prompt_async"))
        .map(|(_, body)| body.get("agent").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(agents, [json!("plan"), Value::Null, json!("plan")]);
}

/// 2.x holds the agent on the session: plan mode creates it on the plan
/// agent, and leaving or re-entering plan mode switches it before the prompt.
#[tokio::test]
async fn plan_mode_sets_the_v2_session_agent() {
    let overrides = json!({"plan": true, "keepSteering": true});
    let mut wire = TurnWire::start_config(false, true, true, None, "2.0.3", overrides, false).await;
    first_prompt(&mut wire, true).await;
    for plan in [false, true] {
        wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
        wire.v2(
            "session.execution.succeeded",
            json!({"sessionID": "fixture"}),
        );
        assert_eq!(wire.done().await.0, DoneStatus::Completed);
        let steering = wire.steering.as_ref().unwrap();
        steering.send(plan_config(plan)).await.unwrap();
        // Adopting a configuration re-reads the catalog first.
        first_prompt(&mut wire, true).await;
    }
    let posts = wire.posts.lock().unwrap();
    let created = posts
        .iter()
        .find(|(path, _)| path == "/api/session")
        .unwrap();
    assert_eq!(created.1["agent"], "plan");
    let switched: Vec<&Value> = posts
        .iter()
        .filter(|(path, _)| path == "/api/session/fixture/agent")
        .map(|(_, body)| body)
        .collect();
    assert_eq!(
        switched,
        [&json!({"agent": "build"}), &json!({"agent": "plan"})]
    );
    assert!(
        posts
            .iter()
            .filter(|(path, _)| path.ends_with("/prompt"))
            .all(|(_, body)| body.get("agent").is_none())
    );
}

#[test]
fn access_changes_restart_the_server_and_plan_changes_do_not() {
    let harness = OpencodeHarness::new();
    assert_eq!(harness.access_modes(), AccessMode::ALL);
    assert!(harness.supports_plan());
    let request = |access: &str, auto_approve: bool, plan: bool| -> RunRequest {
        serde_json::from_value(json!({
            "prompt": "", "cwd": "/work", "sandbox": "workspace-write",
            "access": access, "autoApprove": auto_approve, "plan": plan
        }))
        .unwrap()
    };
    let full = request("full-access", false, false);
    assert!(!harness.reconfigures_in_place(&full, &request("supervised", false, false)));
    assert!(harness.reconfigures_in_place(&full, &request("full-access", false, true)));
    // auto_approve is full access whatever the mode says.
    assert!(harness.reconfigures_in_place(&full, &request("supervised", true, false)));
}

#[test]
fn access_config_asks_through_opencode_permission_rules() {
    // 1.x: a permission object, merged into the inherited inline layer; a
    // `deny` set there is kept, the one-word form spelled out.
    let v1 = |inherited: Option<&str>, access| -> Value {
        serde_json::from_str(&access_config(inherited, access, Protocol::V1).unwrap()).unwrap()
    };
    // Everything is asked but looking around (and, short of supervised,
    // file changes); the catch-all leads so the named rules win.
    let expected = |edit: &str, overrides: Value| -> Value {
        let mut rules = json!({"*": "ask", "edit": edit, "bash": "ask", "webfetch": "ask",
                               "websearch": "ask"});
        for tool in UNASKED_PERMISSIONS {
            rules[tool] = json!("allow");
        }
        for (name, rule) in overrides.as_object().unwrap() {
            rules[name] = rule.clone();
        }
        rules
    };
    let edits = v1(Some(r#"{"model": "keep"}"#), AccessMode::AutoAcceptEdits);
    assert_eq!(edits["model"], "keep");
    assert_eq!(edits["permission"], expected("allow", json!({})));
    let order: Vec<&String> = edits["permission"].as_object().unwrap().keys().collect();
    assert_eq!(order[0], "*");
    assert!(edits.get("agent").is_none());
    let supervised = v1(
        Some(r#"{"permission": {"bash": "deny", "read": "allow"}}"#),
        AccessMode::Supervised,
    );
    assert_eq!(
        supervised["permission"],
        expected("ask", json!({"bash": "deny"}))
    );
    // The plan agent keeps its edit denial and asks before its plan file.
    assert_eq!(
        supervised["agent"]["plan"]["permission"]["edit"],
        json!({"*": "deny", "*opencode/plans/*.md": "ask"})
    );
    let rules = supervised["agent"]["plan"]["permission"]["edit"]
        .as_object()
        .unwrap();
    assert_eq!(rules.keys().collect::<Vec<_>>(), ["*", PLAN_FILES]);
    // An inherited allow-everything is overridden; a deny-everything kept.
    assert_eq!(
        v1(
            Some(r#"{"permission": "allow"}"#),
            AccessMode::AutoAcceptEdits
        )["permission"],
        expected("allow", json!({}))
    );
    assert_eq!(
        v1(Some(r#"{"permission": "deny"}"#), AccessMode::Supervised)["permission"]["*"],
        "deny"
    );
    // 2.x: ordered rules appended after the inherited ones.
    let v2: Value = serde_json::from_str(
        &access_config(
            Some(r#"{"permissions": [{"action": "read", "resource": "*", "effect": "allow"}]}"#),
            AccessMode::Supervised,
            Protocol::V2,
        )
        .unwrap(),
    )
    .unwrap();
    let rule = |action: &str, resource: &str, effect: &str| -> Value {
        json!({"action": action, "resource": resource, "effect": effect})
    };
    assert_eq!(
        v2["permissions"],
        json!([
            rule("read", "*", "allow"),
            rule("bash", "*", "ask"),
            rule("webfetch", "*", "ask"),
            rule("websearch", "*", "ask"),
            rule("edit", "*", "ask"),
        ])
    );
    assert_eq!(
        v2["agents"]["plan"]["permissions"],
        json!([rule("edit", "*", "deny"), rule("edit", PLAN_FILES, "ask")])
    );
    for invalid in [r#"{"permission": 1}"#, r#"{"agent": []}"#] {
        assert!(access_config(Some(invalid), AccessMode::Supervised, Protocol::V1).is_err());
    }
    assert!(
        access_config(
            Some(r#"{"permissions": {}}"#),
            AccessMode::Supervised,
            Protocol::V2
        )
        .is_err()
    );
}

/// 2.x (2.0.24+) answers question forms, keyed by field, on the session's
/// form route — and the harness emits NO `InputRequested`/`InputResolved` of
/// its own (the engine's input bridge owns that lifecycle; a twin under
/// opencode's id folded an unanswerable duplicate chip into the doc).
#[tokio::test]
async fn v2_question_forms_answer_through_the_form_route() {
    let mut wire = TurnWire::start_policy(false, true, false, Some(true)).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2(
        "form.created",
        json!({"form":{
        "id":"frm_1","sessionID":"fixture","title":"Questions",
        "metadata":{"kind":"question","tool":{"messageID":"msg_1","id":"call_1"}},
        "fields":[
            {"key":"q0","title":"Color","description":"Which color?","type":"string",
             "options":[{"value":"Red","label":"Red","description":"warm"}],"custom":true},
            {"key":"q1","title":"Toppings","description":"Pick some","type":"multiselect",
             "options":[{"value":"A","label":"A"}],"custom":true}
        ]}}),
    );
    let body = wire.posted("/api/session/fixture/form/frm_1/reply").await;
    assert_eq!(body, json!({"answer":{"q0":"Yes","q1":["Yes"]}}));
    // Let the pre-fix twin (if any) land, then prove the stream carries none.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut twins = Vec::new();
    while let Ok(event) = wire.events.try_recv() {
        let event = event.unwrap();
        if matches!(
            event,
            AgentEvent::InputRequested { .. } | AgentEvent::InputResolved { .. }
        ) {
            twins.push(event);
        }
    }
    assert!(
        twins.is_empty(),
        "harness must not emit input lifecycle events itself: {twins:?}"
    );
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
    let mut prompt = posts
        .iter()
        .find(|(p, _)| p == "/api/session/fixture/prompt")
        .unwrap()
        .1
        .clone();
    // A client-minted id makes a retried POST idempotent.
    let id = prompt["id"].take();
    assert!(id.as_str().is_some_and(|id| id.starts_with("msg_")), "{id}");
    prompt.as_object_mut().unwrap().remove("id");
    assert_eq!(prompt, json!({"text":"first","files":[]}));
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
async fn v2_session_model_overflow_drops_the_cache_instead_of_failing_the_run() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
    for i in 0..=MAX_V2_SESSION_MODELS {
        wire.v2(
            "session.step.started",
            json!({
                "sessionID": format!("other-{i}"),
                "assistantMessageID": "m",
                "model": {"id": "muse", "providerID": "opencode"},
            }),
        );
    }
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID": "fixture"}),
    );
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
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

#[test]
fn server_version_parsing() {
    for (raw, expected) in [
        ("2.0.4", Some((2, 0, 4))),
        ("opencode v2.0.11", Some((2, 0, 11))),
        ("v2.0.11-beta+build", Some((2, 0, 11))),
        (" 1.18.21 ", Some((1, 18, 21))),
        ("3.1.0", Some((3, 1, 0))),
        ("2.0", None),
        ("", None),
        ("unknown", None),
    ] {
        let version = ServerVersion::parse(raw);
        assert_eq!(version.raw, raw);
        assert_eq!(version.number, expected, "{raw}");
    }
}

#[tokio::test]
async fn detection_routes_and_authentication() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (route, body, status, expected) in [
        (
            "/api/info",
            r#"{"version":"2.0.11"}"#,
            200,
            Some(Protocol::V2),
        ),
        (
            "/api/status",
            r#"{"data":{"version":"2.0.4"}}"#,
            201,
            Some(Protocol::V2),
        ),
        (
            "/api/health",
            r#"{"healthy":true,"version":"2.0.3"}"#,
            200,
            Some(Protocol::V2),
        ),
        (
            "/global/health",
            r#"{"version":"1.18.21"}"#,
            200,
            Some(Protocol::V1),
        ),
        ("/api/info", r#"{"version":" "}"#, 200, None),
        ("/api/info", r#"{"healthy":true}"#, 200, None),
        ("/api/info", "<html>web UI</html>", 200, None),
        ("/api/info", "{}", 401, None),
        ("/api/status", "{}", 403, None),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Server::attached(format!("http://{}", listener.local_addr().unwrap()));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                let header = String::from_utf8_lossy(&bytes[..n]);
                let path = header.split_whitespace().nth(1).unwrap();
                recorded.lock().unwrap().push(path.to_owned());
                let (code, text) = if path == route {
                    (status, body)
                } else {
                    (404, "{}")
                };
                socket.write_all(format!("HTTP/1.1 {code} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).as_bytes()).await.unwrap();
            }
        });
        let result = Protocol::detect(&server).await;
        task.abort();
        if status == 401 || status == 403 {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("authentication rejected")
            );
        } else {
            assert_eq!(result.unwrap(), expected);
            assert_eq!(server.version.get().is_some(), expected.is_some());
        }
        let seen = seen.lock().unwrap();
        let order = ["/api/info", "/api/status", "/api/health", "/global/health"];
        assert_eq!(*seen, order[..seen.len()]);
    }
}

#[test]
fn v2_command_bodies_follow_server_version() {
    for version in ["2.0.2", "2.0.3", "unknown"] {
        assert_eq!(
            command_body_v2(Some(&ServerVersion::parse(version)), "test", "args", &[]),
            json!({"command":"test","text":"args"})
        );
    }
    for version in ["2.0.4", "v2.0.11", "3.0.0"] {
        let version = ServerVersion::parse(version);
        assert_eq!(
            command_body_v2(Some(&version), "test", "args", &[]),
            json!({"name":"test","text":"args"})
        );
        let attachments = vec!["/workspace/image.png".into()];
        assert_eq!(
            command_body_v2(Some(&version), "test", "args", &attachments)["files"],
            prompt_body_v2("args", &attachments, None)["files"]
        );
    }
}

#[tokio::test]
async fn command_http_failure_errors_turn_without_watchdog() {
    for (v2, version, key) in [
        (false, "1.18.21", "command"),
        (true, "2.0.3", "command"),
        (true, "2.0.11", "name"),
    ] {
        let mut wire = TurnWire::start_config(
            false,
            v2,
            true,
            None,
            version,
            json!({"prompt":"/test args"}),
            true,
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = wire.events.recv().await {
                if let AgentEvent::Done { status, error, .. } = event.unwrap() {
                    assert_eq!(status, DoneStatus::Errored);
                    return error.unwrap();
                }
            }
            panic!("missing Done");
        })
        .await
        .unwrap();
        assert!(error.contains("400"), "{error}");
        assert!(error.contains("bad command!"), "{error}");
        let posts = wire.posts.lock().unwrap();
        let (_, body) = posts.iter().find(|(p, _)| p.ends_with("/command")).unwrap();
        assert_eq!(body[key], "test");
        assert_eq!(body[if v2 { "text" } else { "arguments" }], "args");
    }
}

#[test]
fn agent_model_option_filters_and_preserves_ids() {
    let option = agent_option(&json!({"data":[
        {"id":"build-id","name":"Build","mode":"primary","hidden":false},
        {"id":"all-id","name":"All","mode":"all"},
        {"id":"hidden","name":"Hidden","mode":"primary","hidden":true},
        {"id":"sub","name":"Sub","mode":"subagent"}
    ]}));
    assert_eq!(option.id, "agent");
    assert_eq!(option.label, "Agent");
    assert_eq!(option.default_choice, "");
    assert_eq!(
        option
            .choices
            .iter()
            .map(|c| (c.id.as_str(), c.label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("", "Server default"),
            ("build-id", "Build"),
            ("all-id", "All")
        ]
    );
}

#[tokio::test]
async fn agent_selection_on_create_and_resume() {
    for v2 in [false, true] {
        for resume in [false, true] {
            for agent in [json!("build-id"), json!(""), json!(true)] {
                let mut overrides = json!({"modelOptions":{"agent":agent}});
                if resume {
                    overrides["resume"] = json!("fixture");
                }
                let mut wire =
                    TurnWire::start_config(false, v2, true, None, "2.0.11", overrides, false).await;
                if v2 {
                    wire.request("/api/model").await;
                }
                wire.request(if v2 { "/prompt" } else { "/prompt_async" })
                    .await;
                let posts = wire.posts.lock().unwrap();
                let selection = posts.iter().find(|(p, _)| {
                    if resume {
                        p.ends_with("/agent")
                    } else {
                        p.ends_with("/session")
                    }
                });
                let expected = if v2 && agent == "build-id" {
                    json!("build-id")
                } else {
                    Value::Null
                };
                assert_eq!(
                    selection
                        .map(|(_, b)| b["agent"].clone())
                        .unwrap_or(Value::Null),
                    expected
                );
                if resume && v2 && agent == "build-id" {
                    assert_eq!(
                        posts[0],
                        (
                            "/api/session/fixture/agent".into(),
                            json!({"agent":"build-id"})
                        )
                    );
                }
            }
        }
    }
}

#[test]
fn v2_status_retry_and_progress_vocabulary() {
    let mut names = HashMap::new();
    for status in [
        json!({"type":"busy"}),
        json!({"type":"idle"}),
        json!({"type":"retry","attempt":3,"message":"overloaded","next":123}),
    ] {
        let data = json!({"sessionID":"s", "status":status});
        assert_eq!(
            normalize_v2_frame(json!({"type":"session.status", "data":data}), &mut names),
            vec![json!({"type":"session.status","properties":data})]
        );
    }
    let retry = normalize_v2_frame(
        json!({"type":"session.retry.scheduled","data":{"sessionID":"s","assistantMessageID":"m","attempt":3,"at":123,"error":{"type":"provider.api","message":"overloaded"}}}),
        &mut names,
    );
    assert_eq!(
        retry[0],
        json!({"type":"session.status","properties":{"sessionID":"s","status":{"type":"retry","attempt":3,"next":123,"message":"overloaded"}}})
    );
    normalize_v2_frame(
        json!({"type":"session.tool.input.started","data":{"sessionID":"s","assistantMessageID":"m","id":"tool","name":"task"}}),
        &mut names,
    );
    let progress = normalize_v2_frame(
        json!({"type":"session.tool.progress","data":{"sessionID":"s","assistantMessageID":"m","id":"tool","metadata":{"sessionId":"child"}}}),
        &mut names,
    );
    assert_eq!(
        progress[0]["properties"]["part"]["state"],
        json!({"status":"running","metadata":{"sessionId":"child"}})
    );
    assert_eq!(progress[0]["properties"]["part"]["tool"], "task");
    assert!(
        normalize_v2_frame(
            json!({"type":"session.future.event","data":{"sessionID":"s"}}),
            &mut names
        )
        .is_empty()
    );
}

#[tokio::test]
async fn v2_discovery_settles_and_caches_agents_with_overlapping_models() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let harness =
        OpencodeHarness::new().with_base_url(format!("http://{}", listener.local_addr().unwrap()));
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            let path = request.split_whitespace().nth(1).unwrap();
            let body = {
                let mut calls = recorded.lock().unwrap();
                calls.push(path.to_owned());
                match path {
                    "/api/info" => json!({"version":"2.0.11"}),
                    "/api/model" if calls.iter().filter(|p| p.as_str() == path).count() == 1 => json!({"data":[]}),
                    "/api/model" => json!({"data":[
                        {"providerID":"mock","id":"a","name":"A","enabled":true},
                        {"providerID":"mock","id":"b","name":"B","enabled":true}
                    ]}),
                    "/api/agent" => json!({"data":[{"id":"agent-id","name":"Agent name","mode":"all","hidden":false}]}),
                    _ => json!({"data":[]}),
                }
            }.to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let (first, overlapping) = tokio::join!(harness.models(), harness.models());
    let first = first.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(overlapping.unwrap()).unwrap()
    );
    for model in &first {
        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].id, "agent");
        assert_eq!(model.options[0].choices[1].id, "agent-id");
    }
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == "/api/model")
            .count(),
        2,
        "empty catalog must settle"
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == "/api/agent")
            .count(),
        1,
        "overlapping callers share agents with models"
    );
    harness.models().await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == "/api/agent")
            .count(),
        2,
        "later discovery refreshes agents too"
    );
    task.abort();
    let _ = task.await;
    let retained = harness.model_catalog(true).await.unwrap();
    assert_eq!(retained.source, "cache");
    assert_eq!(
        retained.models, first,
        "offline refresh retains models and agent options"
    );
}

#[tokio::test]
async fn v2_scheduled_retries_reach_existing_retry_abort() {
    let mut wire = TurnWire::start_proto(false, true).await;
    wire.request("/api/model").await;
    wire.request("/prompt").await;
    wire.v2("session.retry.scheduled", json!({"sessionID":"fixture","assistantMessageID":"m","attempt":RETRY_ABORT_ATTEMPT,"at":123,"error":{"type":"provider.api","message":"overloaded"}}));
    wire.request("/interrupt").await;
    wire.v2(
        "session.execution.succeeded",
        json!({"sessionID":"fixture"}),
    );
    assert_eq!(wire.done().await.0, DoneStatus::Errored);
}

#[tokio::test]
async fn stalled_prompt_post_does_not_block_bus_completion() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.31",
        json!({"holdPrompt": true}),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.status("idle");
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    assert_eq!(wire.polls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stalled_prompt_post_has_a_bounded_timeout() {
    for v2 in [false, true] {
        let mut wire = TurnWire::start_config(
            false,
            v2,
            true,
            None,
            "2.0.11",
            json!({"holdPrompt": true, "noBusWatchdog": true}),
            false,
        )
        .await;
        if v2 {
            wire.request("/api/model").await;
        }
        wire.request(if v2 { "/prompt" } else { "/prompt_async" })
            .await;
        tokio::task::yield_now().await;
        // The server never shows the prompt (not running, no message under
        // its id): one retry, then the turn fails instead of spinning. The
        // paused clock is stepped by hand: a timer of the test's own would
        // fire early while loopback IO is in flight. Each step gives that IO
        // a moment of real time (a blocking task holds the paused clock
        // still), or the stepped clock times out a request the fake has not
        // even read yet.
        tokio::time::pause();
        let mut status = None;
        for _ in 0..200 {
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(5)))
                .await
                .unwrap();
            while let Ok(event) = wire.events.try_recv() {
                if let AgentEvent::Done { status: done, .. } = event.unwrap() {
                    status = Some(done);
                }
            }
            if status.is_some() {
                break;
            }
        }
        tokio::time::resume();
        assert_eq!(status, Some(DoneStatus::Errored));
        let prompts: Vec<Value> = wire
            .posts
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("/prompt") || p.ends_with("/prompt_async"))
            .map(|(_, body)| body[if v2 { "id" } else { "messageID" }].clone())
            .collect();
        assert_eq!(prompts.len(), 2, "{prompts:?}");
        assert_eq!(prompts[0], prompts[1], "a retry reuses the message id");
    }
}

/// A prompt POST that dies without an answer, for a turn the bus shows
/// running, was taken by the server: the turn lives on and completes.
#[tokio::test]
async fn unanswered_prompt_post_of_a_running_turn_keeps_the_turn() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.31",
        json!({"dropPrompt": true}),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    // The retry fails the same way; the bus still shows the turn running.
    wire.request("/prompt_async").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    wire.bus.send(json!({"type":"message.updated", "properties":{"info":{"id":"answer", "sessionID":"fixture", "role":"assistant"}}})).unwrap();
    wire.bus.send(json!({"type":"message.part.updated", "properties":{"part":{"id":"text", "messageID":"answer", "sessionID":"fixture", "type":"text", "text":"STILL_HERE"}}})).unwrap();
    wire.status("idle");
    wire.idle();
    assert_eq!(
        wire.done().await,
        (DoneStatus::Completed, "STILL_HERE".to_owned())
    );
}

/// A prompt POST that dies without an answer is not posted again when the
/// session already holds a message under the id it was sent with.
#[tokio::test]
async fn unanswered_prompt_found_by_its_message_id_is_not_reposted() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.31",
        json!({"dropPrompt": true, "messageExists": true}),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    wire.status("busy");
    wire.status("idle");
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    let prompts = wire
        .posts
        .lock()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.ends_with("/prompt_async"))
        .count();
    assert_eq!(prompts, 1);
}

/// A prompt whose POST died unanswered is posted again only while its turn
/// is live: after a stop or an interrupt, the retry never goes out (it
/// would start the stopped turn over on the server).
#[tokio::test]
async fn a_stopped_prompt_is_never_posted_again() {
    for interrupt in [false, true] {
        let mut wire = TurnWire::start_config(
            false,
            false,
            true,
            None,
            "1.18.31",
            json!({"dropPrompt": true, "keepSteering": true}),
            false,
        )
        .await;
        wire.request("/prompt_async").await;
        if interrupt {
            wire.interrupt.cancel();
        } else {
            wire.turn.stop_turn();
        }
        wire.request("/abort").await;
        wire.status("idle");
        wire.idle();
        assert_eq!(wire.done().await.0, DoneStatus::Interrupted);
        // Past the retry delay and its landed check.
        tokio::time::sleep(PROMPT_RETRY_DELAY * 3).await;
        let prompts = wire
            .posts
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("/prompt_async"))
            .count();
        assert_eq!(prompts, 1, "interrupt: {interrupt}");
        assert!(wire.events.try_recv().is_err(), "interrupt: {interrupt}");
    }
}

#[tokio::test]
async fn ambiguous_idle_polls_status_with_backoff_until_idle() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.31",
        json!({"busyPolls": 2}),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.idle();
    let start = tokio::time::Instant::now();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    assert_eq!(wire.polls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(start.elapsed() >= Duration::from_millis(650));
}

#[tokio::test]
async fn abort_ignores_late_bus_text_and_usage() {
    let mut wire = TurnWire::start(false).await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.interrupt.cancel();
    wire.request("/abort").await;
    wire.bus.send(json!({"type":"message.updated", "properties":{"info":{"id":"late", "sessionID":"fixture", "role":"assistant", "tokens":{"input":999,"output":999}}}})).unwrap();
    wire.bus.send(json!({"type":"message.part.updated", "properties":{"part":{"id":"text", "messageID":"late", "sessionID":"fixture", "type":"text", "text":"LATE"}}})).unwrap();
    wire.idle();
    let mut dones = 0;
    while let Some(event) = wire.events.recv().await {
        match event.unwrap() {
            AgentEvent::TextDelta { .. } | AgentEvent::Usage { .. } => {
                panic!("late content after abort")
            }
            AgentEvent::Done { status, .. } => {
                assert_eq!(status, DoneStatus::Interrupted);
                dones += 1;
            }
            _ => {}
        }
    }
    assert_eq!(dones, 1);
}

#[tokio::test]
async fn idle_without_busy_resolves_through_status_poll() {
    let mut wire = TurnWire::start(false).await;
    wire.request("/prompt_async").await;
    wire.idle();
    assert_eq!(wire.done().await.0, DoneStatus::Completed);
    assert_eq!(wire.polls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn v2_spawn_names_bind_child_traffic_to_the_parent_chip() {
    for name in ["task", "subagent"] {
        let mut wire = TurnWire::start_proto(false, true).await;
        wire.request("/api/model").await;
        wire.request("/prompt").await;
        wire.v2("session.execution.started", json!({"sessionID":"fixture"}));
        wire.v2(
            "session.step.started",
            json!({"sessionID":"fixture","assistantMessageID":"parent-message"}),
        );
        wire.v2("session.tool.input.started", json!({"sessionID":"fixture","assistantMessageID":"parent-message","id":"spawn","name":name}));
        wire.v2("session.tool.called", json!({"sessionID":"fixture","assistantMessageID":"parent-message","id":"spawn","input":{"description":"Inspect project","prompt":"inspect"}}));
        wire.v2(
            "session.created",
            json!({"sessionID":"child","parentID":"fixture","title":"Inspect project"}),
        );
        wire.v2(
            "session.step.started",
            json!({"sessionID":"child","assistantMessageID":"child-message"}),
        );
        wire.v2("session.text.delta", json!({"sessionID":"child","assistantMessageID":"child-message","ordinal":0,"delta":"child answer"}));
        wire.v2("session.execution.succeeded", json!({"sessionID":"child"}));
        wire.v2("session.tool.success", json!({"sessionID":"fixture","assistantMessageID":"parent-message","id":"spawn","content":[]}));
        wire.v2(
            "session.execution.succeeded",
            json!({"sessionID":"fixture"}),
        );
        let mut calls = Vec::new();
        let mut child_events = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = wire.events.recv().await {
                match event.unwrap() {
                    AgentEvent::ToolCall { id, call } => calls.push((id, call)),
                    AgentEvent::Subagent {
                        parent_tool_use_id,
                        event,
                    } => child_events.push((parent_tool_use_id, event)),
                    AgentEvent::Done { status, .. } => {
                        assert_eq!(status, DoneStatus::Completed);
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(&calls[0].1, ToolCall::Unknown { name, .. } if name == "Agent: Inspect project")
        );
        assert!(child_events.iter().any(|(id, event)| id == &calls[0].0 && matches!(event.as_ref(), AgentEvent::TextDelta { text } if text == "child answer")), "{name}: {child_events:?}");
    }
}

#[test]
fn native_skill_catalog_rejects_unrepresentable_commands() {
    use zeron_proto::invocation::{Invocation, Skill, invocation_links};
    let mut skills = vec![Skill {
        name: "review[ui]".into(),
        path: "/repo/é skill/SKILL.md".into(),
        description: String::new(),
        enabled: true,
        command: None,
    }];
    let mut commands = vec![];
    for name in [
        "",
        "two words",
        " padded",
        "padded ",
        "line\nbreak",
        "tab\tname",
        "nul\0name",
        "non\u{a0}breaking",
        "review[ui]",
        "review/extra",
    ] {
        commands.push(json!({"name":name,"source":"skill"}));
    }
    for name in ["review", "审查-é:ui.v2_test"] {
        commands.push(json!({"name":name,"source":"skill"}));
    }
    merge_skill_commands(&mut skills, &json!(commands));
    assert_eq!(skills.len(), 3);
    assert!(
        skills[0].command.is_none(),
        "an invalid command must not poison a valid file skill"
    );
    assert_eq!(skills[2].path, "opencode-skill:审查-é:ui.v2_test");
    for skill in skills {
        let invocation = Invocation::Skill {
            name: skill.name,
            path: skill.path,
            command: skill.command,
        };
        assert_eq!(invocation_links(&invocation.link())[0].1, invocation);
    }
}

fn sse_feed(chunks: &[&[u8]]) -> (Vec<String>, u64) {
    let mut decoder = SseDecoder::default();
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.feed(chunk));
    }
    (events, decoder.frames)
}

#[test]
fn sse_decoder_frames_every_line_ending_even_split_across_chunks() {
    for chunks in [
        &[b"data: a\n\ndata: b\n\n".as_slice()][..],
        &[b"data: a\r\n\r\ndata: b\r\n\r\n"],
        &[b"data: a\r\rdata: b\r\r"],
        // A CR ends one chunk and its LF opens the next: one line ending.
        &[b"data: a\r", b"\n\r", b"\ndata: b\r\n", b"\r\n"],
        // A lone CR at a chunk's end is a complete line ending too.
        &[b"data: a\r", b"\r", b"data: b\n", b"\n"],
        &[b"da", b"ta:", b" a\n", b"\n", b"data: b", b"\n\n"],
    ] {
        assert_eq!(
            sse_feed(chunks),
            (vec!["a".into(), "b".into()], 2),
            "{chunks:?}"
        );
    }
}

#[test]
fn sse_decoder_joins_data_lines_and_skips_comments_and_other_fields() {
    let (events, frames) = sse_feed(&[
        b"\xEF\xBB\xBF: connected\n\n",
        b"event: message\nid: 7\nretry: 10\ndata: {\"a\":\ndata:1}\n\n",
        b": heartbeat\n\n",
        b"data\n\n",
    ]);
    assert_eq!(events, vec!["{\"a\":\n1}".to_owned(), String::new()]);
    // Comment-only blocks (the 2.x heartbeat) count as frames.
    assert_eq!(frames, 4);
    // An unterminated event is not dispatched.
    assert_eq!(sse_feed(&[b"data: partial\n"]).0, Vec::<String>::new());
}

#[test]
fn sse_decoder_replaces_invalid_utf8_instead_of_dropping_the_event() {
    let (events, _) = sse_feed(&[
        b"data: {\"text\":\"a\xFFb\"}\n",
        b"\n",
        b"data: {\"ok\":\xE2\x9C",
        b"\x93}\n\n",
    ]);
    assert_eq!(events[0], "{\"text\":\"a\u{FFFD}b\"}");
    // A character split across chunks still decodes whole.
    assert_eq!(events[1], "{\"ok\":\u{2713}}");
    assert!(serde_json::from_str::<Value>(&events[0]).is_ok());
}

#[test]
fn sse_decoder_scans_a_large_frame_once() {
    let payload = format!("data: {{\"text\":\"{}\"}}\n\n", "x".repeat(8 << 20));
    let mut decoder = SseDecoder::default();
    let start = std::time::Instant::now();
    let mut events = Vec::new();
    for chunk in payload.as_bytes().chunks(4096) {
        events.extend(decoder.feed(chunk));
    }
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].len(), (8 << 20) + 11);
    // Rescanning the buffer per chunk took minutes at this size.
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

/// One SSE response per accepted connection, from `bodies` in turn; each
/// connection is held open after its body for `hold`.
async fn sse_server(bodies: Vec<&'static [u8]>, hold: Duration) -> String {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for body in bodies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
                .await;
            let _ = socket.write_all(body).await;
            tokio::spawn(async move {
                tokio::time::sleep(hold).await;
                drop(socket);
            });
        }
    });
    base
}

#[tokio::test]
async fn stream_bus_counts_only_framed_streams_and_abandons_silent_ones() {
    let base = sse_server(
        vec![b"", b"data: {\"payload\":{\"type\":\"x\"}}\n\n"],
        Duration::from_secs(30),
    )
    .await;
    let (tx, mut rx) = mpsc::channel(8);
    let mut tools = HashMap::new();
    let mut models = HashMap::new();
    // Accepted, then nothing: not a healthy stream (the reconnect budget
    // must keep counting), and never announced.
    let resp = reqwest::get(format!("{base}/global/event")).await.unwrap();
    let silence = Duration::from_millis(300);
    let before = tokio::time::Instant::now();
    let mut last_frame = before;
    let streamed = stream_bus(
        &tx,
        resp,
        Protocol::V1,
        silence,
        &mut last_frame,
        &mut tools,
        &mut models,
    );
    assert!(!streamed.await);
    assert!(rx.try_recv().is_err());
    assert_eq!(last_frame, before, "no frame, no stamp");
    // A frame, then silence past the bound: abandoned for a reconnect.
    let resp = reqwest::get(format!("{base}/global/event")).await.unwrap();
    let start = tokio::time::Instant::now();
    let streamed = stream_bus(
        &tx,
        resp,
        Protocol::V1,
        silence,
        &mut last_frame,
        &mut tools,
        &mut models,
    );
    assert!(streamed.await);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(last_frame >= start && last_frame + silence <= tokio::time::Instant::now());
    assert!(matches!(rx.try_recv(), Ok(BusMsg::Connected)));
    assert!(matches!(rx.try_recv(), Ok(BusMsg::Event(_))));
}

#[test]
fn attachment_urls_survive_reserved_characters_and_windows_paths() {
    for (path, url) in [
        ("/tmp/shot.png", "file:///tmp/shot.png"),
        ("/tmp/a b#c?%.png", "file:///tmp/a%20b%23c%3F%25.png"),
        ("/tmp/Zoë/shot 1.png", "file:///tmp/Zo%C3%AB/shot%201.png"),
        (
            r"C:\Users\Zoë\shot 1#.png",
            "file:///C:/Users/Zo%C3%AB/shot%201%23.png",
        ),
        ("c:/x/y.png", "file:///c:/x/y.png"),
        (
            r"\\server\share\dir\f g.png",
            "file://server/share/dir/f%20g.png",
        ),
        (r"\\?\C:\a\b.png", "file:///C:/a/b.png"),
        (r"\\?\UNC\server\share\f.png", "file://server/share/f.png"),
    ] {
        assert_eq!(file_url(path), url, "{path}");
    }
    let body = prompt_body("x", None, None, &[r"C:\Users\me\a b.png".to_owned()], None);
    assert_eq!(body["parts"][1]["url"], "file:///C:/Users/me/a%20b.png");
    let body = prompt_body_v2("x", &["/tmp/a#b.png".to_owned()], None);
    assert_eq!(body["files"][0]["uri"], "file:///tmp/a%23b.png");
    assert_eq!(body["files"][0]["name"], "a#b.png");
}

#[test]
fn windows_directories_encode_like_encode_uri_component() {
    // encodeURIComponent output, except `/` (which decodes the same).
    assert_eq!(
        encode_directory(r"C:\Users\Zoë\my proj"),
        "C%3A%5CUsers%5CZo%C3%AB%5Cmy%20proj"
    );
    assert_eq!(encode_directory(r"\\srv\share"), "%5C%5Csrv%5Cshare");
}

#[test]
fn prompt_message_ids_sort_like_the_servers() {
    let ids: Vec<String> = (0..64).map(|_| prompt_message_id()).collect();
    for id in &ids {
        assert_eq!(id.len(), 30, "{id}");
        assert!(id.starts_with("msg_"), "{id}");
        assert!(id[4..16].bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        assert!(id[16..].bytes().all(|b| b.is_ascii_alphanumeric()), "{id}");
    }
    // The time prefix strictly ascends, within a millisecond too.
    for pair in ids.windows(2) {
        assert!(pair[0][..16] < pair[1][..16], "{pair:?}");
    }
    // 1.x mints the same shape: 12 hex digits of ms*4096+counter.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let stamp = u64::from_str_radix(&ids[63][4..16], 16).unwrap();
    assert!((stamp >> 12).abs_diff(now & 0xF_FFFF_FFFF) < 10_000);
}

/// The bus never replays: requests asked while it reconnected are listed
/// and answered (permissions approved, questions put to the user), and
/// ones the live bus already delivered are not answered twice.
#[tokio::test]
async fn reconnect_sweep_answers_requests_asked_in_the_gap() {
    for v2 in [false, true] {
        let pending = if v2 {
            json!({
                "permission": [
                    {"id": "per_live", "sessionID": "fixture"},
                    {"id": "per_gap", "sessionID": "fixture"},
                    {"id": "per_foreign", "sessionID": "foreign"},
                ],
                "form": [{
                    "id": "frm_gap", "sessionID": "fixture", "title": "Questions",
                    "metadata": {"kind": "question"},
                    "fields": [{"key": "q0", "title": "Color", "description": "Which?",
                                "type": "string", "options": [{"value": "Yes", "label": "Yes"}]}],
                }],
            })
        } else {
            json!({
                "permission": [
                    {"id": "per_live", "sessionID": "fixture"},
                    {"id": "per_gap", "sessionID": "fixture"},
                    {"id": "per_foreign", "sessionID": "foreign"},
                ],
                "question": [{
                    "id": "que_gap", "sessionID": "fixture",
                    "questions": [{"header": "Choice", "question": "Continue?",
                                   "options": [{"label": "Yes"}]}],
                }],
            })
        };
        let mut wire = TurnWire::start_config(
            false,
            v2,
            true,
            Some(true),
            "2.0.11",
            json!({"pending": pending}),
            false,
        )
        .await;
        if v2 {
            wire.request("/api/model").await;
        }
        wire.request(if v2 { "/prompt" } else { "/prompt_async" })
            .await;
        if v2 {
            wire.v2("session.execution.started", json!({"sessionID": "fixture"}));
            wire.v2(
                "permission.asked",
                json!({"id": "per_live", "sessionID": "fixture"}),
            );
        } else {
            wire.status("busy");
            wire.bus.send(json!({"type": "permission.asked", "properties": {"id": "per_live", "sessionID": "fixture"}})).unwrap();
        }
        let live = if v2 {
            "/api/session/fixture/permission/per_live/reply"
        } else {
            "/permission/per_live/reply"
        };
        wire.posted(live).await;
        // The stream drops; the reconnect sweeps what was asked meanwhile.
        wire.bus.send(json!("close")).unwrap();
        let gap = if v2 {
            "/api/session/fixture/permission/per_gap/reply"
        } else {
            "/permission/per_gap/reply"
        };
        let answer_route = if v2 {
            "/api/session/fixture/form/frm_gap/reply"
        } else {
            "/question/que_gap/reply"
        };
        let approval = wire.posted(gap).await;
        let answer = wire.posted(answer_route).await;
        assert_eq!(
            approval,
            if v2 {
                json!({"decision": "once"})
            } else {
                json!({"reply": "once"})
            }
        );
        assert_eq!(
            answer,
            if v2 {
                json!({"answer": {"q0": "Yes"}})
            } else {
                json!({"answers": [["Yes"]]})
            }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            wire.bus_connections
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert!(wire.listings.load(std::sync::atomic::Ordering::SeqCst) >= 2);
        let posts = wire.posts.lock().unwrap();
        let replies = |suffix: &str| posts.iter().filter(|(p, _)| p.ends_with(suffix)).count();
        assert_eq!(
            replies(live),
            1,
            "a request the live bus delivered is answered once"
        );
        assert_eq!(replies(gap), 1);
        assert!(!posts.iter().any(|(p, _)| p.contains("per_foreign")));
        drop(posts);
        let _ = wire.pending.lock().unwrap().take();
    }
}

/// A permission approval that cannot be delivered leaves the agent waiting
/// forever: after its retries the turn ends, visibly.
#[tokio::test]
async fn undeliverable_permission_reply_ends_the_turn_with_an_error() {
    let mut wire = TurnWire::start_config(
        false,
        false,
        true,
        None,
        "1.18.31",
        json!({"failReplies": true}),
        false,
    )
    .await;
    wire.request("/prompt_async").await;
    wire.status("busy");
    wire.bus
        .send(json!({"type": "permission.asked", "properties": {"id": "per_1", "sessionID": "fixture"}}))
        .unwrap();
    let (status, error, surfaced) = tokio::time::timeout(Duration::from_secs(15), async {
        let mut surfaced = None;
        loop {
            match wire.events.recv().await.unwrap().unwrap() {
                AgentEvent::Error { message } => surfaced = Some(message),
                AgentEvent::Done { status, error, .. } => return (status, error, surfaced),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(status, DoneStatus::Errored);
    let error = error.unwrap();
    assert!(error.contains("Could not approve"), "{error}");
    assert!(error.contains("500"), "{error}");
    assert_eq!(surfaced.as_deref(), Some(error.as_str()));
    let posts = wire.posts.lock().unwrap();
    let attempts = posts
        .iter()
        .filter(|(p, _)| p == "/permission/per_1/reply")
        .count();
    assert_eq!(attempts, REPLY_ATTEMPTS as usize);
    assert!(posts.iter().any(|(p, _)| p.ends_with("/abort")));
}

/// A server that dies mid-turn is noticed within the process poll, with its
/// own stderr, not after the bus burns its reconnect budget.
#[cfg(unix)]
#[tokio::test]
async fn a_server_crash_mid_turn_settles_promptly_with_its_stderr() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let exe = fixture.path().join("opencode");
    let script = r#"#!/usr/bin/env node
const http = require('node:http');
if (process.argv.includes('--version')) { console.log('1.18.31'); process.exit(0); }
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => {
  const json = (body, status = 200) => {
    res.writeHead(status, {'content-type': 'application/json'}); res.end(JSON.stringify(body));
  };
  const path = req.url.split('?')[0];
  if (path === '/global/health') return json({healthy: true, version: '1.18.31'});
  if (path === '/global/event') {
    res.writeHead(200, {'content-type': 'text/event-stream'});
    res.write('data: {"payload":{"type":"server.connected","properties":{}}}\n\n');
    return;
  }
  if (path === '/session' && req.method === 'POST') return json({id: 'ses_crash'});
  if (path === '/provider') return json({all: [], connected: []});
  if (path === '/command') return json([]);
  if (path.endsWith('/prompt_async')) {
    res.writeHead(204); res.end();
    setTimeout(() => { console.error('fatal: provider bootstrap exploded'); process.exit(7); }, 100);
    return;
  }
  json({});
}).listen(port, '127.0.0.1', () => console.log(`opencode server listening on http://127.0.0.1:${port}`));
"#;
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (steer_tx, steering) = mpsc::channel(1);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        steering,
        interrupt: tokio_util::sync::CancellationToken::new(),
        turn: Default::default(),
    };
    let request: RunRequest = serde_json::from_value(json!({
        "prompt": "hi", "cwd": fixture.path().to_str().unwrap(),
        "sandbox": "workspace-write", "autoApprove": true,
    }))
    .unwrap();
    let harness = OpencodeHarness::new()
        .with_executable(&exe)
        .with_graces(Duration::from_millis(100), Duration::from_millis(100));
    let mut stream = harness.run(request, controls).await.unwrap();
    let start = tokio::time::Instant::now();
    let (error, done) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut error = None;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Error { message } => error = Some(message),
                AgentEvent::Done {
                    status,
                    error: done,
                    ..
                } => return (error, (status, done)),
                _ => {}
            }
        }
        panic!("stream ended without Done");
    })
    .await
    .unwrap();
    // The bus alone would take 40 reconnects (10s+, far longer on Windows).
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(done.0, DoneStatus::Errored);
    let message = done.1.unwrap();
    assert!(message.contains("exited unexpectedly"), "{message}");
    assert!(message.contains("provider bootstrap exploded"), "{message}");
    assert_eq!(error.as_deref(), Some(message.as_str()));
    drop(steer_tx);
}

/// The losers of a first-start migration race, as 1.18.21 prints them: the
/// statement rides the lines after "Failed query:". Other failed queries,
/// and DDL outside one, are not transient.
#[test]
fn a_lost_migration_race_is_transient() {
    let raced = "\x1b[91m\x1b[1mError: \x1b[0mUnexpected error\n\nFailed query: \n        CREATE TABLE `workspace` (\n          `id` text PRIMARY KEY\n        );\n      \nparams: ";
    let mut race = MigrationRace::default();
    assert!(raced.lines().any(|line| race.feed(line)));
    let mut race = MigrationRace::default();
    assert!(
        race.feed("Failed query: CREATE INDEX `session_project_idx` ON `session` (`project_id`);")
    );
    for unrelated in [
        "Failed query: \n  INSERT INTO `session` VALUES (?)\nparams: 1",
        "Failed query: \nparams: \nCREATE TABLE `x` (",
        "CREATE TABLE `x` (",
        "Error: Unexpected error",
    ] {
        let mut race = MigrationRace::default();
        assert!(
            !unrelated.lines().any(|line| race.feed(line)),
            "{unrelated}"
        );
    }
}

/// A server that lost the migration race is relaunched, not reported.
#[cfg(unix)]
#[tokio::test]
async fn startup_retries_a_lost_migration_race() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let exe = fixture.path().join("opencode");
    let script = r#"#!/usr/bin/env node
const fs = require('node:fs');
const http = require('node:http');
if (process.argv.includes('--version')) { console.log('1.18.21'); process.exit(0); }
const raced = require('node:path').join(__dirname, 'raced');
if (!fs.existsSync(raced)) {
  fs.writeFileSync(raced, '');
  console.error('Error: Unexpected error\n\nFailed query: \n        CREATE TABLE `workspace` (\n          `id` text PRIMARY KEY\n        );\n      \nparams: ');
  process.exit(1);
}
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => {
  res.setHeader('content-type', 'application/json');
  res.end(JSON.stringify({version: '1.18.21'}));
}).listen(port, '127.0.0.1', () => console.log(`opencode server listening on http://127.0.0.1:${port}`));
"#;
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut server = Server::spawn(&exe, fixture.path().to_str(), Duration::from_secs(5), None)
        .await
        .unwrap();
    server.shutdown(Duration::from_millis(100)).await;
    assert!(fixture.path().join("raced").exists());
}

/// A scripted 1.x `opencode serve` with the routes a run needs. `on_prompt`
/// and `on_abort` are JavaScript run with the request's `res` and an
/// `emit(payload)` that writes one bus frame.
#[cfg(unix)]
fn scripted_opencode(dir: &std::path::Path, on_prompt: &str, on_abort: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("opencode");
    let script = format!(
        r#"#!/usr/bin/env node
const http = require('node:http');
if (process.argv.includes('--version')) {{ console.log('1.18.31'); process.exit(0); }}
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
let bus = null;
const emit = (payload) => bus && bus.write(`data: ${{JSON.stringify({{payload}})}}\n\n`);
http.createServer((req, res) => {{
  const json = (body, status = 200) => {{
    res.writeHead(status, {{'content-type': 'application/json'}}); res.end(JSON.stringify(body));
  }};
  const path = req.url.split('?')[0];
  if (path === '/global/health') return json({{healthy: true, version: '1.18.31'}});
  if (path === '/global/event') {{
    res.writeHead(200, {{'content-type': 'text/event-stream'}});
    bus = res;
    emit({{type: 'server.connected', properties: {{}}}});
    return;
  }}
  if (path === '/session' && req.method === 'POST') return json({{id: 'ses_script'}});
  if (path === '/provider') return json({{all: [], connected: []}});
  if (path === '/command') return json([]);
  if (path === '/session/status') return json({{}});
  if (path.endsWith('/prompt_async')) {{ {on_prompt} return; }}
  if (path.endsWith('/abort')) {{ {on_abort} return; }}
  json({{}});
}}).listen(port, '127.0.0.1', () => console.log(`opencode server listening on http://127.0.0.1:${{port}}`));
"#
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    exe
}

/// Run `exe` on a one-prompt request; the steering sender keeps the
/// mailbox open between turns.
#[cfg(unix)]
async fn scripted_run(
    exe: &std::path::Path,
    cwd: &std::path::Path,
) -> (
    BoxStream<'static, Result<AgentEvent, HarnessError>>,
    crate::TurnControl,
    mpsc::Sender<crate::SteerMessage>,
) {
    let (steer_tx, steering) = mpsc::channel(1);
    let turn = crate::TurnControl::default();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        steering,
        interrupt: tokio_util::sync::CancellationToken::new(),
        turn: turn.clone(),
    };
    let request: RunRequest = serde_json::from_value(json!({
        "prompt": "hi", "cwd": cwd.to_str().unwrap(),
        "sandbox": "workspace-write", "autoApprove": true,
    }))
    .unwrap();
    let harness = OpencodeHarness::new()
        .with_executable(exe)
        .with_graces(Duration::from_secs(5), Duration::from_millis(100));
    (
        harness.run(request, controls).await.unwrap(),
        turn,
        steer_tx,
    )
}

/// Events up to and including the next Done.
#[cfg(unix)]
async fn until_done(
    stream: &mut BoxStream<'static, Result<AgentEvent, HarnessError>>,
) -> Vec<AgentEvent> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                return events;
            }
        }
        panic!("stream ended without Done: {events:?}");
    })
    .await
    .unwrap()
}

/// A server that dies while a stop is settling ends the turn the user
/// stopped as stopped, not as a failure.
#[cfg(unix)]
#[tokio::test]
async fn a_crash_during_a_stop_settles_interrupted() {
    let fixture = tempfile::tempdir().unwrap();
    let exe = scripted_opencode(
        fixture.path(),
        "res.writeHead(204); res.end(); \
         emit({type: 'session.status', properties: {sessionID: 'ses_script', status: {type: 'busy'}}});",
        "json(true); \
         setTimeout(() => { console.error('fatal: abort exploded'); process.exit(7); }, 50);",
    );
    let (mut stream, turn, _steer) = scripted_run(&exe, fixture.path()).await;
    // The turn is running once the prompt went out.
    tokio::time::sleep(Duration::from_millis(500)).await;
    turn.stop_turn();
    let events = until_done(&mut stream).await;
    let Some(AgentEvent::Done { status, error, .. }) = events.last() else {
        unreachable!()
    };
    assert_eq!(*status, DoneStatus::Interrupted, "{events:?}");
    assert_eq!(*error, None);
}

/// A server that dies between turns ends the run with one Errored Done
/// carrying its crash (the turn already completed; no error chip of its
/// own), so the next message starts a fresh server.
#[cfg(unix)]
#[tokio::test]
async fn a_crash_between_turns_reports_the_crash_once() {
    let fixture = tempfile::tempdir().unwrap();
    let exe = scripted_opencode(
        fixture.path(),
        "res.writeHead(204); res.end(); \
         const status = (type) => ({type: 'session.status', properties: {sessionID: 'ses_script', status: {type}}}); \
         emit(status('busy')); emit(status('idle')); \
         emit({type: 'session.idle', properties: {sessionID: 'ses_script'}}); \
         setTimeout(() => { console.error('fatal: idle exploded'); process.exit(7); }, 300);",
        "json(true);",
    );
    let (mut stream, _turn, _steer) = scripted_run(&exe, fixture.path()).await;
    let first = until_done(&mut stream).await;
    assert!(
        matches!(
            first.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            })
        ),
        "{first:?}"
    );
    let crash = until_done(&mut stream).await;
    assert_eq!(crash.len(), 1, "{crash:?}");
    let Some(AgentEvent::Done { status, error, .. }) = crash.last() else {
        unreachable!()
    };
    assert_eq!(*status, DoneStatus::Errored);
    let error = error.as_deref().unwrap();
    assert!(error.contains("idle exploded"), "{error}");
    let rest = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    assert!(matches!(rest, Ok(None)), "{rest:?}");
}

/// A 1.x server that serves a run until its prompt lands, then wedges: the
/// open bus goes quiet and every later connection is accepted (as a stopped
/// process's would be) but never answered — health checks too, unless
/// `answer_health`. Also returns the count of bus subscriptions.
async fn wedging_server(answer_health: bool) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let wedged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let subscriptions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let subscribed = subscriptions.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let wedged = wedged.clone();
            let subscribed = subscribed.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let header = String::from_utf8_lossy(&request);
                let path = header.split_whitespace().nth(1).unwrap_or_default();
                let path = path.split('?').next().unwrap_or_default().to_owned();
                let health = path == "/global/health";
                if path == "/global/event" {
                    subscribed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                if wedged.load(std::sync::atomic::Ordering::SeqCst) && !(health && answer_health) {
                    // Held open, never answered.
                    let _ = socket.read(&mut buf).await;
                    std::future::pending::<()>().await;
                }
                if path == "/global/event" {
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: connected\n\n")
                        .await;
                    std::future::pending::<()>().await;
                }
                let body = match path.as_str() {
                    "/global/health" => r#"{"healthy":true,"version":"1.18.31"}"#,
                    "/session" => r#"{"id":"fixture"}"#,
                    _ => "{}",
                };
                if path.ends_with("/prompt_async") {
                    wedged.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    (base, subscriptions)
}

/// Run one prompt against `base` with the given bus bounds; returns the
/// error chip and the Done once it settles.
async fn wedged_run(base: String, timeouts: BusTimeouts) -> (Option<String>, DoneStatus, String) {
    let (event_tx, mut events) = mpsc::channel(64);
    let (_steer_tx, steering) = mpsc::channel(1);
    let request =
        json!({"prompt": "first", "cwd": "", "sandbox": "workspace-write", "autoApprove": true});
    tokio::spawn(run_session(Session {
        server: Server::attached(base),
        event_tx,
        controls: RunControls {
            realtime: None,
            execution_lease: None,
            request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
            steering,
            interrupt: tokio_util::sync::CancellationToken::new(),
            turn: Default::default(),
        },
        request: serde_json::from_value(request).unwrap(),
        interrupt_grace: Duration::from_secs(2),
        kill_grace: Duration::from_millis(50),
        bus_timeouts: timeouts,
        known_commands: Some(Vec::new()),
        initial_native_command_selected: false,
    }));
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut chip = None;
        loop {
            match events.recv().await.unwrap().unwrap() {
                AgentEvent::Error { message } => chip = Some(message),
                AgentEvent::Done { status, error, .. } => {
                    return (chip, status, error.unwrap_or_default());
                }
                _ => {}
            }
        }
    })
    .await
    .expect("a wedged server must end the turn")
}

/// A server that stops serving without exiting (stopped, hung) still
/// accepts connections, so reconnects alone would keep the turn Working
/// for half an hour. One silence and an unanswered health check end it.
#[tokio::test]
async fn a_wedged_server_fails_the_turn_after_one_silence() {
    let (base, subscriptions) = wedging_server(false).await;
    let start = tokio::time::Instant::now();
    let (chip, status, error) = wedged_run(
        base,
        BusTimeouts {
            silence: Duration::from_millis(300),
            probe: Duration::from_millis(300),
            wedged: Duration::from_secs(60),
        },
    )
    .await;
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.contains("stopped responding"), "{error}");
    assert!(error.contains("health check"), "{error}");
    assert_eq!(chip.as_deref(), Some(error.as_str()));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    // Checked on the live stream's silence, before any reconnect.
    assert_eq!(subscriptions.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A server that still answers health checks but whose bus never frames
/// again is given up on once the wedge budget passes.
#[tokio::test]
async fn a_frameless_bus_fails_the_turn_past_the_wedge_budget() {
    let (base, _) = wedging_server(true).await;
    let start = tokio::time::Instant::now();
    let (_, status, error) = wedged_run(
        base,
        BusTimeouts {
            silence: Duration::from_millis(200),
            probe: Duration::from_secs(1),
            wedged: Duration::from_millis(1200),
        },
    )
    .await;
    assert_eq!(status, DoneStatus::Errored);
    assert!(error.contains("stopped responding"), "{error}");
    assert!(error.contains("no event for 1s"), "{error}");
    assert!(start.elapsed() >= Duration::from_millis(1200));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

/// The live case: a SIGSTOPped `opencode serve`. The turn fails fast and the
/// stopped process is torn down with the run.
#[cfg(unix)]
#[tokio::test]
async fn a_stopped_server_process_fails_the_turn_and_is_torn_down() {
    let fixture = tempfile::tempdir().unwrap();
    let exe = scripted_opencode(
        fixture.path(),
        "res.writeHead(204); res.end(); \
         require('node:fs').writeFileSync(require('node:path').join(__dirname, 'pid'), String(process.pid)); \
         setTimeout(() => process.kill(process.pid, 'SIGSTOP'), 50);",
        "json(true);",
    );
    let (steer_tx, steering) = mpsc::channel(1);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        steering,
        interrupt: tokio_util::sync::CancellationToken::new(),
        turn: Default::default(),
    };
    let request: RunRequest = serde_json::from_value(json!({
        "prompt": "hi", "cwd": fixture.path().to_str().unwrap(),
        "sandbox": "workspace-write", "autoApprove": true,
    }))
    .unwrap();
    let harness = OpencodeHarness::new()
        .with_executable(&exe)
        .with_graces(Duration::from_millis(100), Duration::from_millis(100))
        .with_bus_timeouts(
            Duration::from_millis(500),
            Duration::from_millis(500),
            Duration::from_secs(60),
        );
    let mut stream = harness.run(request, controls).await.unwrap();
    let start = tokio::time::Instant::now();
    let events = until_done(&mut stream).await;
    let Some(AgentEvent::Done { status, error, .. }) = events.last() else {
        unreachable!()
    };
    assert_eq!(*status, DoneStatus::Errored);
    let error = error.as_deref().unwrap();
    assert!(error.contains("stopped responding"), "{error}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    // The run ends only once its server is gone.
    let rest = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
    assert!(matches!(rest, Ok(None)), "{rest:?}");
    let pid = std::fs::read_to_string(fixture.path().join("pid")).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the stopped server {pid} outlived its run");
    drop(steer_tx);
}
