//! Loopback realtime socket. No device, no mint host, and no session file.
#![allow(clippy::result_large_err)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::{Sink, SinkExt, Stream, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::{
    VoiceEvent, VoicePhase, VoiceRejection, VoiceRole, VoiceSnapshot, VoiceTranscript, VoiceWork,
};

use crate::audio::NullDuplex;
use crate::{Call, Credentials, Minted, Speech, ToolExec, ToolSpec, TranscriptSink, Tune, run};

const SECRET: &str = "test-secret";
const SAMPLES: [i16; 3] = [1, -2, 300];

struct TestCreds {
    port: u16,
    mints: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Credentials for TestCreds {
    async fn mint(&self) -> Result<Minted, VoiceRejection> {
        self.mints.fetch_add(1, Ordering::SeqCst);
        Ok(Minted {
            client_secret: SECRET.into(),
            model: "test".into(),
            websocket_url: format!("ws://127.0.0.1:{}/v1/realtime", self.port),
        })
    }
}

#[derive(Default)]
struct RecTool {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait::async_trait]
impl ToolExec for RecTool {
    async fn call(&self, name: &str, arguments: Value) -> String {
        self.calls
            .lock()
            .expect("calls")
            .push((name.to_string(), arguments));
        "looked-up".into()
    }
}

struct NoSink;

#[async_trait::async_trait]
impl TranscriptSink for NoSink {
    async fn commit(&self, _transcript: &VoiceTranscript) {}
}

struct Running {
    task: tokio::task::JoinHandle<Result<(), VoiceRejection>>,
    cancel: CancellationToken,
    events: Arc<Mutex<Vec<VoiceEvent>>>,
    pcm: Arc<Mutex<Vec<i16>>>,
    mints: Arc<AtomicUsize>,
    tool: Arc<RecTool>,
    _mute: watch::Sender<bool>,
    tune: watch::Sender<Tune>,
}

struct Seen {
    uri: String,
    query: String,
    protocol: String,
}

fn start_call(port: u16) -> Running {
    let mints = Arc::new(AtomicUsize::new(0));
    let creds = TestCreds {
        port,
        mints: Arc::clone(&mints),
    };
    let duplex = NullDuplex::default();
    let pcm = Arc::clone(&duplex.pcm);
    let tool = Arc::new(RecTool::default());
    let cancel = CancellationToken::new();
    let (mute_tx, mute_rx) = watch::channel(false);
    let (tune_tx, tune_rx) = watch::channel(Tune {
        instructions: "be brief".into(),
        speech: Speech::default(),
        input: None,
    });
    let (tx, mut rx) = mpsc::channel(256);
    let events = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            store.lock().expect("events").push(event);
        }
    });
    let task = tokio::spawn(run(
        creds,
        duplex,
        tool.clone(),
        Arc::new(NoSink),
        Call {
            instructions: "be brief".into(),
            tools: vec![ToolSpec {
                name: "lookup".into(),
                description: "Look up a thing".into(),
                parameters: json!({"type": "object"}),
            }],
            chat_id: "chat-1".into(),
            speech: Speech::default(),
        },
        cancel.clone(),
        mute_rx,
        tune_rx,
        tx,
    ));
    Running {
        task,
        cancel,
        events,
        pcm,
        mints,
        tool,
        _mute: mute_tx,
        tune: tune_tx,
    }
}

async fn bind() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.expect("bind")
}

async fn accept_one(listener: &TcpListener) -> (WebSocketStream<TcpStream>, Seen) {
    // Something on this machine probes new listeners with `HEAD /`. Skip those.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "accept timed out");
        let (socket, _) = timeout(remaining, listener.accept())
            .await
            .expect("accept timed out")
            .expect("accept");
        socket.set_nodelay(true).expect("nodelay");
        let slot = Arc::new(Mutex::new(None));
        let write_slot = Arc::clone(&slot);
        // The client rejects the handshake unless the subprotocol is echoed verbatim.
        let handshake = tokio_tungstenite::accept_hdr_async(
            socket,
            move |request: &Request, mut response: Response| {
                let protocol = request
                    .headers()
                    .get("Sec-WebSocket-Protocol")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if let Ok(value) = HeaderValue::from_str(&protocol) {
                    response
                        .headers_mut()
                        .insert("Sec-WebSocket-Protocol", value);
                }
                *write_slot.lock().expect("seen") = Some(Seen {
                    uri: request.uri().to_string(),
                    query: request.uri().query().unwrap_or("").to_string(),
                    protocol,
                });
                Ok::<_, ErrorResponse>(response)
            },
        )
        .await;
        let ws = match handshake {
            Ok(ws) => ws,
            Err(WsError::Protocol(ProtocolError::WrongHttpMethod)) => continue,
            Err(err) => panic!("handshake: {err}"),
        };
        let seen = slot.lock().expect("seen").take().expect("request");
        return (ws, seen);
    }
}

fn assert_handshake(seen: &Seen, conversation: Option<&str>) {
    assert_eq!(seen.protocol, format!("xai-client-secret.{SECRET}"));
    assert!(!seen.uri.contains(SECRET), "{}", seen.uri);
    assert!(!seen.query.contains(SECRET), "{}", seen.query);
    assert!(seen.uri.contains("/v1/realtime"), "{}", seen.uri);
    assert!(
        seen.query.split('&').any(|pair| pair == "model=test"),
        "{}",
        seen.query
    );
    let id = seen
        .query
        .split('&')
        .find_map(|pair| pair.strip_prefix("conversation_id="));
    assert_eq!(id, conversation, "{}", seen.query);
}

fn assert_session_update(text: &str) {
    let value: Value = serde_json::from_str(text).expect("session.update");
    assert_eq!(value["type"], "session.update");
    assert_eq!(value["session"]["instructions"], "be brief");
    assert_eq!(value["session"]["tools"][0]["type"], "function");
    assert_eq!(value["session"]["tools"][0]["name"], "lookup");
    assert_eq!(value["session"]["turn_detection"]["type"], "server_vad");
    assert_eq!(value["session"]["resumption"]["enabled"], true);
    assert_eq!(
        value["session"]["audio"]["output"]["speed"].as_f64(),
        Some(1.0)
    );
    assert!(value["session"].get("voice").is_none());
}

fn assert_live(snapshot: &VoiceSnapshot) {
    assert_eq!(snapshot.chat_id, "chat-1");
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.phase, VoicePhase::Active);
    assert_eq!(snapshot.work, VoiceWork::Idle);
    assert!(!snapshot.muted);
    assert!(!snapshot.playing);
    assert!(snapshot.voice.is_none());
    assert!(snapshot.voices.is_empty());
}

fn assert_still_open(events: &[VoiceEvent]) {
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, VoiceEvent::Closed { .. }))
    );
}

async fn next_text<R>(read: &mut R) -> String
where
    R: Stream<Item = Result<Message, WsError>> + Unpin,
{
    timeout(Duration::from_secs(4), async {
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => return text,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected text, got {other:?}"),
            }
        }
    })
    .await
    .expect("text timed out")
}

async fn send_json<W>(write: &mut W, value: Value)
where
    W: Sink<Message, Error = WsError> + Unpin,
{
    write
        .send(Message::Text(value.to_string()))
        .await
        .expect("send");
}

fn pcm_b64(samples: &[i16]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn until<F>(label: &str, mut ready: F)
where
    F: FnMut() -> bool,
{
    timeout(Duration::from_secs(4), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}

async fn finish(call: Running, mints: usize) -> Vec<VoiceEvent> {
    call.cancel.cancel();
    let result = timeout(Duration::from_secs(3), call.task)
        .await
        .expect("run hung")
        .expect("run panicked");
    assert_eq!(result, Ok(()));
    assert_eq!(call.mints.load(Ordering::SeqCst), mints);
    let events = call.events.lock().expect("events").clone();
    assert_still_open(&events);
    events
}

#[tokio::test]
async fn redial_resumes_with_the_conversation_and_cancel_does_not_remint() {
    timeout(Duration::from_secs(8), redial_body())
        .await
        .expect("redial test timed out");
}

async fn redial_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);

    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    send_json(
        &mut write,
        json!({
            "type": "conversation.created",
            "conversation": {"id": "conv-1"}
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio_transcript.delta",
            "item_id": "asst-1",
            "delta": "Hel"
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio_transcript.delta",
            "item_id": "asst-1",
            "delta": "lo"
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&SAMPLES)
        }),
    )
    .await;
    let events = &call.events;
    let pcm = &call.pcm;
    until("partials and speaker pcm", || {
        let texts: Vec<String> = events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match event {
                VoiceEvent::Partial { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        texts == ["Hel", "lo"] && pcm.lock().expect("pcm").as_slice() == SAMPLES
    })
    .await;
    write.send(Message::Close(None)).await.expect("close");
    drop(write);
    drop(read);

    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, Some("conv-1"));
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    until("snapshot after redial", || {
        events
            .lock()
            .expect("events")
            .iter()
            .filter(|event| matches!(event, VoiceEvent::Snapshot { .. }))
            .count()
            >= 2
    })
    .await;
    // Keep this socket open across cancel. A close is another redial, and
    // select can prefer it when both the close and the cancel are ready.
    let events = finish(call, 2).await;
    drop(write);
    drop(read);

    let mut ids = Vec::new();
    let mut partials = Vec::new();
    for event in &events {
        match event {
            VoiceEvent::Snapshot { snapshot } => {
                assert_live(snapshot);
                ids.push(snapshot.session_id.clone());
            }
            VoiceEvent::Partial {
                generation,
                item_id,
                text,
            } => {
                assert_eq!(*generation, 1);
                assert_eq!(item_id.as_deref(), Some("asst-1"));
                partials.push(text.clone());
            }
            _ => {}
        }
    }
    assert!(ids.len() >= 2, "{ids:?}");
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(partials, ["Hel", "lo"]);
}

#[tokio::test]
async fn function_call_sends_output_then_response_create() {
    timeout(Duration::from_secs(8), function_body())
        .await
        .expect("function test timed out");
}

async fn function_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    write
        .send(Message::Ping(b"abc".to_vec()))
        .await
        .expect("ping");
    timeout(Duration::from_secs(3), async {
        loop {
            match read.next().await {
                Some(Ok(Message::Pong(payload))) if payload.as_slice() == b"abc" => return,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected pong, got {other:?}"),
            }
        }
    })
    .await
    .expect("pong");
    send_json(
        &mut write,
        json!({
            "type": "response.function_call_arguments.done",
            "name": "lookup",
            "call_id": "call-1",
            "arguments": "{\"q\":1}"
        }),
    )
    .await;
    let output: Value = serde_json::from_str(&next_text(&mut read).await).expect("output");
    assert_eq!(output["type"], "conversation.item.create");
    assert_eq!(output["item"]["type"], "function_call_output");
    assert_eq!(output["item"]["call_id"], "call-1");
    assert_eq!(output["item"]["output"], "looked-up");
    let follow: Value = serde_json::from_str(&next_text(&mut read).await).expect("create");
    assert_eq!(follow["type"], "response.create");
    let calls = call.tool.calls.lock().expect("calls").clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "lookup");
    assert_eq!(calls[0].1["q"], 1);
    let _events = finish(call, 1).await;
    drop(write);
    drop(read);
}

#[tokio::test]
async fn resume_failure_seeds_history_without_response_create() {
    timeout(Duration::from_secs(10), resume_body())
        .await
        .expect("resume test timed out");
}

async fn resume_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);

    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    send_json(
        &mut write,
        json!({
            "type": "conversation.created",
            "conversation": {"id": "conv-9"}
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "conversation.item.input_audio_transcription.completed",
            "item_id": "user-1",
            "transcript": "hello user"
        }),
    )
    .await;
    let events = &call.events;
    until("user final", || {
        events.lock().expect("events").iter().any(|event| {
            matches!(
                event,
                VoiceEvent::Final { transcript }
                    if transcript.role == VoiceRole::User && transcript.text == "hello user"
            )
        })
    })
    .await;
    write.send(Message::Close(None)).await.expect("close");
    drop(write);
    drop(read);

    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, Some("conv-9"));
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(
        &mut write,
        json!({
            "type": "error",
            "error": {"message": "Unknown conversation"}
        }),
    )
    .await;
    // The client leaves this socket once, then seeds on the next dial.
    timeout(Duration::from_secs(4), async {
        while read.next().await.is_some() {}
    })
    .await
    .expect("client stayed after the resume error");
    drop(write);
    drop(read);

    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    let seed: Value = serde_json::from_str(&next_text(&mut read).await).expect("seed");
    assert_eq!(seed["type"], "conversation.item.create");
    assert_eq!(seed["item"]["role"], "user");
    assert_eq!(seed["item"]["content"][0]["type"], "input_text");
    assert_eq!(seed["item"]["content"][0]["text"], "hello user");
    assert!(!seed.to_string().contains("response.create"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    match timeout(Duration::from_millis(50), read.next()).await {
        Err(_) => {}
        Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
        Ok(other) => panic!("seed batch sent an extra frame: {other:?}"),
    }
    let events = finish(call, 3).await;
    drop(write);
    drop(read);
    let mut ids = Vec::new();
    for event in &events {
        if let VoiceEvent::Snapshot { snapshot } = event {
            assert_live(snapshot);
            ids.push(snapshot.session_id.clone());
        }
    }
    assert!(ids.len() >= 2, "{ids:?}");
    assert!(ids.iter().all(|id| id == &ids[0]));
}

#[tokio::test]
async fn established_drops_keep_a_fresh_redial_budget() {
    timeout(Duration::from_secs(8), established_drops_body())
        .await
        .expect("established redial test timed out");
}

async fn established_drops_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    for n in 0..4 {
        let (ws, seen) = accept_one(&listener).await;
        assert_handshake(&seen, None);
        let (mut write, mut read) = ws.split();
        assert_session_update(&next_text(&mut read).await);
        send_json(&mut write, json!({"type": "session.updated"})).await;
        let events = &call.events;
        let want = n + 1;
        until("snapshot", || {
            events
                .lock()
                .expect("events")
                .iter()
                .filter(|event| matches!(event, VoiceEvent::Snapshot { .. }))
                .count()
                >= want
        })
        .await;
        write.send(Message::Close(None)).await.expect("close");
        drop(write);
        drop(read);
    }
    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    let events = &call.events;
    until("fifth snapshot", || {
        events
            .lock()
            .expect("events")
            .iter()
            .filter(|event| matches!(event, VoiceEvent::Snapshot { .. }))
            .count()
            >= 5
    })
    .await;
    let events = finish(call, 5).await;
    drop(write);
    drop(read);
    let mut ids = Vec::new();
    for event in &events {
        if let VoiceEvent::Snapshot { snapshot } = event {
            assert_live(snapshot);
            ids.push(snapshot.session_id.clone());
        }
    }
    assert!(ids.len() >= 5, "{ids:?}");
    assert!(ids.iter().all(|id| id == &ids[0]));
}

#[tokio::test]
async fn three_failed_dials_end_the_call() {
    timeout(Duration::from_secs(8), failed_dials_body())
        .await
        .expect("failed dial test timed out");
}

async fn failed_dials_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    for _ in 0..4 {
        let (ws, seen) = accept_one(&listener).await;
        assert_handshake(&seen, None);
        let (mut write, mut read) = ws.split();
        assert_session_update(&next_text(&mut read).await);
        write.send(Message::Close(None)).await.expect("close");
        drop(write);
        drop(read);
    }
    let result = timeout(Duration::from_secs(3), call.task)
        .await
        .expect("run hung")
        .expect("run panicked");
    assert_eq!(result, Err(VoiceRejection::Protocol));
    assert_eq!(call.mints.load(Ordering::SeqCst), 4);
    assert_still_open(&call.events.lock().expect("events"));
}

#[tokio::test]
async fn speaking_before_a_reply_does_not_cancel() {
    timeout(Duration::from_secs(8), idle_speech_body())
        .await
        .expect("idle speech test timed out");
}

async fn idle_speech_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    send_json(
        &mut write,
        json!({"type": "input_audio_buffer.speech_started"}),
    )
    .await;
    assert!(
        text_within(&mut read, Duration::from_millis(200))
            .await
            .is_none(),
        "an idle turn must not cancel"
    );
    send_json(
        &mut write,
        json!({"type": "input_audio_buffer.speech_stopped"}),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&SAMPLES)
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio_transcript.delta",
            "item_id": "asst-idle",
            "delta": "hi"
        }),
    )
    .await;
    let pcm = &call.pcm;
    let events = &call.events;
    until("audio after the caller stops", || {
        pcm.lock().expect("pcm").as_slice() == SAMPLES
            && events
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(event, VoiceEvent::Partial { text, .. } if text == "hi"))
    })
    .await;
    let _events = finish(call, 1).await;
    drop(write);
    drop(read);
}

#[tokio::test]
async fn an_interrupt_cancels_the_open_reply_and_drops_the_rest() {
    timeout(Duration::from_secs(8), interrupt_body())
        .await
        .expect("interrupt test timed out");
}

async fn interrupt_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;
    send_json(&mut write, json!({"type": "response.created"})).await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&SAMPLES)
        }),
    )
    .await;
    let pcm = Arc::clone(&call.pcm);
    until("first audio", || {
        pcm.lock().expect("pcm").as_slice() == SAMPLES
    })
    .await;
    send_json(
        &mut write,
        json!({"type": "input_audio_buffer.speech_started"}),
    )
    .await;
    let cancel: Value = serde_json::from_str(&next_text(&mut read).await).expect("cancel");
    assert_eq!(cancel["type"], "response.cancel");
    assert!(pcm.lock().expect("pcm").is_empty());
    let rest = [4i16, 5, 6];
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&rest)
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio_transcript.delta",
            "item_id": "asst-cut",
            "delta": "no"
        }),
    )
    .await;
    let events = &call.events;
    until("dropped tail still reports the transcript", || {
        events
            .lock()
            .expect("events")
            .iter()
            .any(|event| matches!(event, VoiceEvent::Partial { text, .. } if text == "no"))
    })
    .await;
    assert!(
        pcm.lock().expect("pcm").is_empty(),
        "audio after the interrupt must not play"
    );
    send_json(&mut write, json!({"type": "response.cancelled"})).await;
    send_json(
        &mut write,
        json!({"type": "input_audio_buffer.speech_stopped"}),
    )
    .await;
    send_json(&mut write, json!({"type": "response.created"})).await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&SAMPLES)
        }),
    )
    .await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio_transcript.delta",
            "item_id": "asst-next",
            "delta": "yes"
        }),
    )
    .await;
    until("the next reply plays", || {
        pcm.lock().expect("pcm").as_slice() == SAMPLES
            && events
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(event, VoiceEvent::Partial { text, .. } if text == "yes"))
    })
    .await;
    let _events = finish(call, 1).await;
    drop(write);
    drop(read);
}

#[tokio::test]
async fn speed_changes_wait_until_the_open_reply_finishes() {
    timeout(Duration::from_secs(8), speed_change_body())
        .await
        .expect("speed change test timed out");
}

async fn speed_change_body() {
    let listener = bind().await;
    let port = listener.local_addr().expect("addr").port();
    let call = start_call(port);
    let (ws, seen) = accept_one(&listener).await;
    assert_handshake(&seen, None);
    let (mut write, mut read) = ws.split();
    assert_session_update(&next_text(&mut read).await);
    send_json(&mut write, json!({"type": "session.updated"})).await;

    call.tune
        .send(Tune {
            instructions: "be brief".into(),
            speech: Speech {
                voice: Some("liora".into()),
                speed: 1.15,
            },
            input: None,
        })
        .expect("tune");
    let idle: Value = serde_json::from_str(&next_text(&mut read).await).expect("idle update");
    assert_eq!(idle["type"], "session.update");
    assert_eq!(idle["session"]["voice"], "liora");
    assert_eq!(idle["session"]["instructions"], "be brief");
    let idle_speed = idle["session"]["audio"]["output"]["speed"]
        .as_f64()
        .expect("speed");
    assert!((idle_speed - 1.15).abs() < 0.001, "{idle_speed}");
    assert!(idle["session"]["tools"][0]["name"] == "lookup");

    send_json(&mut write, json!({"type": "response.created"})).await;
    send_json(
        &mut write,
        json!({
            "type": "response.output_audio.delta",
            "delta": pcm_b64(&SAMPLES)
        }),
    )
    .await;
    let pcm = Arc::clone(&call.pcm);
    until("the open reply is playing", || {
        pcm.lock().expect("pcm").as_slice() == SAMPLES
    })
    .await;
    call.tune
        .send(Tune {
            instructions: "be brief\n\nwarmer".into(),
            speech: Speech {
                voice: Some("ara".into()),
                speed: 1.5,
            },
            input: None,
        })
        .expect("tune");
    assert!(
        text_within(&mut read, Duration::from_millis(250))
            .await
            .is_none(),
        "an open reply keeps its speed until it finishes"
    );
    send_json(&mut write, json!({"type": "response.done"})).await;
    let done: Value = serde_json::from_str(&next_text(&mut read).await).expect("deferred update");
    assert_eq!(done["type"], "session.update");
    assert_eq!(done["session"]["voice"], "ara");
    assert_eq!(done["session"]["instructions"], "be brief\n\nwarmer");
    assert_eq!(
        done["session"]["audio"]["output"]["speed"].as_f64(),
        Some(1.5)
    );
    let _events = finish(call, 1).await;
    drop(write);
    drop(read);
}

async fn text_within<R>(read: &mut R, budget: Duration) -> Option<String>
where
    R: Stream<Item = Result<Message, WsError>> + Unpin,
{
    timeout(budget, async {
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => return Some(text),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(_)) => return None,
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}
