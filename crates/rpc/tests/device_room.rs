//! HostRelay + ClientLink end-to-end over an in-memory fake device room.
//!
//! The fake implements the `DeviceRoom` DO's relay semantics (edge/src/device-room.ts):
//! route client frames to the single host socket with `from` stamped; route host frames
//! by `to` (bounce `client_gone` when the target left); host supersede (a new host join
//! closes the predecessor); `client_closed` on client disconnect; `host_closed` broadcast
//! on host disconnect; `host_offline` bounce when a client sends with no host; nudge
//! frames delivered to the host.

// tungstenite's `accept_hdr_async` callback signature fixes the Err type as a full
// `Response` — its size is not ours to shrink.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};
use zeron_proto::AccessToken;

use zeron_rpc::device_room::{
    CLIENT_CLOSED, CLIENT_GONE, HOST_CLOSED, HOST_OFFLINE, NUDGE_KIND, RELAY_KIND,
};
use zeron_rpc::{
    DeviceFrameHeader, DeviceLink, HostRelay, HostRelayConfig, LinkCache, LinkCacheConfig,
    RpcError, RpcReply, RpcService, StaticToken, TokenError, TokenSource, decode_device_frame,
    device_room_ws_url, encode_device_frame, methods,
};

// ---------------------------------------------------------------------------
// Fake relay (the DO semantics, in-memory)
// ---------------------------------------------------------------------------

enum Out {
    Frame(Vec<u8>),
    Close,
    CloseCode(u16),
}

#[derive(Default)]
struct RelayState {
    nudge_acks: Vec<serde_json::Value>,
    reject_next: Option<u16>,
    requests: Vec<String>,
    host: Option<mpsc::UnboundedSender<Out>>,
    clients: HashMap<String, mpsc::UnboundedSender<Out>>,
    /// Zombie-path simulation: the host stays "connected" (no bounce) but
    /// client→host frames vanish — the 2026-08-19 dead edge↔host leg.
    blackhole_host_bound: bool,
}

struct FakeRelay {
    port: u16,
    state: Arc<Mutex<RelayState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeRelay {
    async fn start() -> FakeRelay {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(RelayState::default()));
        let accept_state = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(handle_socket(stream, accept_state.clone()));
            }
        });
        FakeRelay { port, state, task }
    }

    fn edge_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn host_connected(&self) -> bool {
        self.state.lock().expect("lock").host.is_some()
    }

    async fn wait_host_connected(&self) {
        wait_until(|| self.host_connected()).await;
    }

    /// Swallow client→host frames while keeping the host registered — the
    /// zombie relay path (client↔edge healthy, edge↔host dead).
    fn set_blackhole_host_bound(&self, on: bool) {
        self.state.lock().expect("lock").blackhole_host_bound = on;
    }

    /// Deliver a nudge frame to the connected host (the DO's /nudge live path).
    fn nudge(&self, chat_id: &str) {
        self.nudge_receipt(chat_id, None);
    }

    fn nudge_receipt(&self, chat_id: &str, token: Option<&str>) {
        let header = DeviceFrameHeader::new(chat_id, NUDGE_KIND);
        let payload = serde_json::json!({ "chatId": chat_id, "token": token }).to_string();
        let frame = encode_device_frame(&header, payload.as_bytes()).expect("encode nudge");
        let state = self.state.lock().expect("lock");
        state
            .host
            .as_ref()
            .expect("host connected")
            .send(Out::Frame(frame))
            .expect("send");
    }
}

fn relay_error(code: &str) -> Vec<u8> {
    serde_json::json!({ "error": code })
        .to_string()
        .into_bytes()
}

async fn handle_socket(stream: tokio::net::TcpStream, state: Arc<Mutex<RelayState>>) {
    let mut uri = String::new();
    let ws =
        match tokio_tungstenite::accept_hdr_async(stream, |req: &WsRequest, res: WsResponse| {
            uri = req.uri().to_string();
            let mut state = lock(&state);
            state.requests.push(uri.clone());
            if let Some(status) = state.reject_next.take() {
                return Err(tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(status)
                    .body(Some("rejected".into()))
                    .expect("rejection response"));
            }
            Ok(res)
        })
        .await
        {
            Ok(ws) => ws,
            Err(_) => return,
        };
    let query: HashMap<String, String> = uri
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let is_host = query.get("role").map(String::as_str) == Some("host");
    let conn_id = query
        .get("connId")
        .cloned()
        .unwrap_or_else(|| "anon".into());

    let (mut sink, mut ws_stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    {
        let mut st = state.lock().expect("lock");
        if is_host {
            // One live host socket: close any predecessor (backend restart / supersede).
            if let Some(old) = st.host.take() {
                let _ = old.send(Out::Close);
            }
            st.host = Some(tx.clone());
        } else {
            st.clients.insert(conn_id.clone(), tx.clone());
        }
    }

    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Frame(bytes) => {
                    if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                Out::Close => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    break;
                }
                Out::CloseCode(code) => {
                    let _ = sink.send(WsMessage::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: code.into(),
                            reason: "test close".into(),
                        },
                    ))).await;
                    break;
                }
            }
        }
    });

    while let Some(message) = ws_stream.next().await {
        let bytes = match message {
            Ok(WsMessage::Binary(bytes)) => bytes,
            Ok(WsMessage::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let Ok((header, payload)) = decode_device_frame(&bytes) else {
            break;
        };
        let mut st = state.lock().expect("lock");
        if !is_host {
            if st.blackhole_host_bound {
                continue; // zombie path: frame vanishes, no bounce
            }
            match &st.host {
                Some(host) => {
                    let mut routed = DeviceFrameHeader::new(header.s, header.k);
                    routed.from = Some(conn_id.clone());
                    let _ = host.send(Out::Frame(
                        encode_device_frame(&routed, &payload).expect("encode"),
                    ));
                }
                None => {
                    let bounce = DeviceFrameHeader::new(header.s, RELAY_KIND);
                    let _ = tx.send(Out::Frame(
                        encode_device_frame(&bounce, &relay_error(HOST_OFFLINE)).expect("encode"),
                    ));
                }
            }
            continue;
        }
        if header.k == "nudgeAck" {
            st.nudge_acks
                .push(serde_json::from_slice(&payload).unwrap());
            continue;
        }
        // Host frame: route by `to`.
        let Some(to) = header.to else { continue };
        match st.clients.get(&to) {
            Some(client) => {
                let stripped = DeviceFrameHeader::new(header.s, header.k);
                let _ = client.send(Out::Frame(
                    encode_device_frame(&stripped, &payload).expect("encode"),
                ));
            }
            None => {
                let bounce = DeviceFrameHeader::new(header.s, RELAY_KIND).with_to(to);
                let _ = tx.send(Out::Frame(
                    encode_device_frame(&bounce, &relay_error(CLIENT_GONE)).expect("encode"),
                ));
            }
        }
    }

    // Disconnect bookkeeping (mirrors DeviceRoom.webSocketClose).
    {
        let mut st = state.lock().expect("lock");
        if is_host {
            if st.host.as_ref().is_some_and(|h| h.same_channel(&tx)) {
                st.host = None;
            }
            for client in st.clients.values() {
                let header = DeviceFrameHeader::new("", RELAY_KIND);
                let _ = client.send(Out::Frame(
                    encode_device_frame(&header, &relay_error(HOST_CLOSED)).expect("encode"),
                ));
            }
        } else {
            st.clients.remove(&conn_id);
            if let Some(host) = &st.host {
                let mut header = DeviceFrameHeader::new("", RELAY_KIND);
                header.from = Some(conn_id.clone());
                let _ = host.send(Out::Frame(
                    encode_device_frame(&header, &relay_error(CLIENT_CLOSED)).expect("encode"),
                ));
            }
        }
    }
    writer.abort();
}

// ---------------------------------------------------------------------------
// Test service + helpers
// ---------------------------------------------------------------------------

struct TestService {
    label: String,
    active_streams: Arc<AtomicUsize>,
    peer_messages: AtomicUsize,
}

impl TestService {
    fn new(label: &str) -> Arc<Self> {
        Arc::new(Self {
            label: label.into(),
            active_streams: Arc::new(AtomicUsize::new(0)),
            peer_messages: AtomicUsize::new(0),
        })
    }
}

struct StreamGuard(Arc<AtomicUsize>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl RpcService for TestService {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            "DeliverPeerMessage" => {
                self.peer_messages.fetch_add(1, Ordering::SeqCst);
                std::future::pending().await
            }
            methods::LOCAL_DEVICE => Ok(RpcReply::Value(serde_json::json!({"id": self.label}))),
            "Echo" => Ok(RpcReply::Value(
                serde_json::json!({ "host": self.label, "params": params }),
            )),
            "Count" => {
                let n = params.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(RpcReply::Stream(
                    futures::stream::iter((0..n).map(|i| serde_json::json!(i))).boxed(),
                ))
            }
            "Never" => {
                self.active_streams.fetch_add(1, Ordering::SeqCst);
                let guard = StreamGuard(self.active_streams.clone());
                Ok(RpcReply::Stream(
                    futures::stream::poll_fn(move |_| {
                        let _keep = &guard;
                        std::task::Poll::Pending
                    })
                    .boxed(),
                ))
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

async fn wait_until(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached within 5s");
}

fn relay_config(edge_url: &str, retry_ms: u64) -> HostRelayConfig {
    let mut config =
        HostRelayConfig::new(edge_url, "dev-a", Arc::new(StaticToken("test-user".into())));
    config.retry = Duration::from_millis(retry_ms);
    config
}

fn cache(edge_url: &str) -> Arc<LinkCache> {
    let mut config = LinkCacheConfig::new(edge_url, Arc::new(StaticToken("test-user".into())));
    config.cooldown_base = Duration::from_millis(100);
    config.cooldown_max = Duration::from_millis(400);
    config.probe_timeout = Duration::from_millis(1_500);
    LinkCache::new(config)
}

fn noop_nudge() -> zeron_rpc::NudgeHandler {
    Arc::new(|_| true)
}

struct RecoveringToken {
    value: Mutex<Option<AccessToken>>,
    invalidated: Mutex<Vec<u64>>,
    unavailable: AtomicBool,
    changes: tokio::sync::watch::Sender<u64>,
}

impl RecoveringToken {
    fn new(value: Option<&str>) -> Self {
        let (changes, _) = tokio::sync::watch::channel(0);
        Self {
            value: Mutex::new(value.map(|value| AccessToken { value: value.into(), generation: 0 })),
            invalidated: Mutex::new(Vec::new()),
            unavailable: AtomicBool::new(false),
            changes,
        }
    }

    fn replace(&self, value: &str) {
        self.unavailable.store(false, Ordering::SeqCst);
        *self.value.lock().expect("lock") = Some(AccessToken {
            value: value.into(),
            generation: *self.changes.borrow() + 1,
        });
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    fn clear(&self) {
        self.unavailable.store(false, Ordering::SeqCst);
        *self.value.lock().expect("lock") = None;
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    fn fail_temporarily(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
}

#[async_trait]
impl TokenSource for RecoveringToken {
    async fn token(&self) -> Result<AccessToken, TokenError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(TokenError::TemporarilyUnavailable(
                "resolver unavailable".into(),
            ));
        }
        self.value
            .lock()
            .expect("lock")
            .clone()
            .ok_or(TokenError::SignedOut)
    }

    fn invalidate(&self, generation: u64) {
        lock(&self.invalidated).push(generation);
    }

    fn subscribe(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(self.changes.subscribe())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tokio::test]
async fn handshake_rejections_invalidate_only_unauthorized_leases() {
    for status in [401, 403, 429, 503] {
        let relay = FakeRelay::start().await;
        let token = Arc::new(RecoveringToken::new(Some("old-token")));
        let lease = token.token().await.unwrap();
        token.replace("new-token");
        lock(&relay.state).reject_next = Some(status);
        let url = device_room_ws_url(&relay.edge_url(), "dev-a", "client", None, &lease.value);
        assert!(DeviceLink::connect(&url, token.clone(), lease.generation).await.is_err());
        assert_eq!(*lock(&token.invalidated), if status == 401 { vec![0] } else { vec![] });
        assert_eq!(token.token().await.unwrap().generation, 1);
        assert_eq!(lock(&relay.state).requests.len(), 1);
    }
}

#[tokio::test]
async fn host_handshake_rejections_preserve_reconnect_and_classify_auth() {
    for status in [401, 403, 429, 503] {
        let relay = FakeRelay::start().await;
        lock(&relay.state).reject_next = Some(status);
        let token = Arc::new(RecoveringToken::new(Some("old-token")));
        let mut config = HostRelayConfig::new(relay.edge_url(), "dev-a", token.clone());
        config.retry = Duration::from_millis(10);
        let host = HostRelay::spawn(config, TestService::new("host-a"), noop_nudge());
        relay.wait_host_connected().await;
        assert_eq!(*lock(&token.invalidated), if status == 401 { vec![0] } else { vec![] });
        assert_eq!(lock(&relay.state).requests.len(), 2);
        drop(host);
    }
}

#[tokio::test]
async fn host_and_device_closes_reject_the_original_lease_only_for_4401() {
    for code in [4401, 4403, 1013] {
        let relay = FakeRelay::start().await;
        let host_token = Arc::new(RecoveringToken::new(Some("old-host")));
        let host = HostRelay::spawn(
            HostRelayConfig::new(relay.edge_url(), "dev-a", host_token.clone()),
            TestService::new("host-a"), noop_nudge(),
        );
        relay.wait_host_connected().await;
        let token = Arc::new(RecoveringToken::new(Some("old-client")));
        let lease = token.token().await.unwrap();
        let url = device_room_ws_url(&relay.edge_url(), "dev-a", "client", Some("auth-client"), &lease.value);
        let link = DeviceLink::connect(&url, token.clone(), lease.generation).await.unwrap();
        wait_until(|| lock(&relay.state).clients.contains_key("auth-client")).await;
        token.replace("new-client");
        host_token.replace("new-host");
        lock(&relay.state).clients["auth-client"].send(Out::CloseCode(code)).unwrap();
        wait_until(|| link.is_closed()).await;
        assert_eq!(*lock(&token.invalidated), if code == 4401 { vec![0] } else { vec![] });
        lock(&relay.state).host.as_ref().unwrap().send(Out::CloseCode(code)).unwrap();
        if code == 4401 {
            wait_until(|| !lock(&host_token.invalidated).is_empty()).await;
        } else {
            // A subsequent connection proves the old host pump observed the close.
            wait_until(|| lock(&relay.state).requests.len() >= 3).await;
        }
        assert_eq!(*lock(&host_token.invalidated), if code == 4401 { vec![0] } else { vec![] });
        assert_eq!(token.token().await.unwrap().generation, 1);
        assert_eq!(host_token.token().await.unwrap().generation, 1);
        drop(host);
    }
}

fn peer_params() -> serde_json::Value {
    serde_json::json!({"chatId":"target", "command": {
        "sourceChatId":"source", "sourceDeviceId":"source-device", "id":"stable-command"
    }})
}

#[tokio::test]
async fn directional_handshake_rejection_never_replays_admission() {
    for device in [false, true] {
        for status in [401, 403, 429, 503] {
            let relay = FakeRelay::start().await;
            lock(&relay.state).reject_next = Some(status);
            let token = Arc::new(RecoveringToken::new(Some("peer-token")));
            let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
            let result = if device {
                links.peer_device_call("dev-a", "target", None, "DeliverPeerMessage", peer_params()).await
            } else {
                links.peer_call("target", None, "DeliverPeerMessage", peer_params()).await
            };
            assert!(result.is_err());
            assert_eq!(*lock(&token.invalidated), if status == 401 { vec![0] } else { vec![] });
            let state = lock(&relay.state);
            assert_eq!(state.requests.len(), 1, "directional failures must not replay");
            assert!(state.requests[0].starts_with(if device { "/device/dev-a/ws?" } else { "/session/target/peer/ws?" }));
        }
    }
}

#[tokio::test]
async fn directional_close_rejects_original_lease_without_replaying_mutation() {
    for device in [false, true] {
        for code in [4401, 1013] {
            let relay = FakeRelay::start().await;
            let service = TestService::new("host-a");
            let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service.clone(), noop_nudge());
            relay.wait_host_connected().await;
            let token = Arc::new(RecoveringToken::new(Some("old-token")));
            let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
            let call = tokio::spawn(async move {
                if device {
                    links.peer_device_call("dev-a", "target", None, "DeliverPeerMessage", peer_params()).await
                } else {
                    links.peer_call("target", None, "DeliverPeerMessage", peer_params()).await
                }
            });
            wait_until(|| service.peer_messages.load(Ordering::SeqCst) == 1).await;
            token.replace("new-token");
            lock(&relay.state).clients.values().next().unwrap().send(Out::CloseCode(code)).unwrap();
            assert!(tokio::time::timeout(Duration::from_secs(2), call).await.unwrap().unwrap().is_err());
            assert_eq!(*lock(&token.invalidated), if code == 4401 { vec![0] } else { vec![] });
            assert_eq!(service.peer_messages.load(Ordering::SeqCst), 1);
            assert_eq!(lock(&relay.state).requests.len(), 2);
            assert_eq!(token.token().await.unwrap().generation, 1);
        }
    }
}

#[tokio::test]
async fn cached_link_retries_dial_with_fresh_lease_but_never_replays_mutation() {
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service.clone(), noop_nudge());
    relay.wait_host_connected().await;
    lock(&relay.state).reject_next = Some(401);
    let token = Arc::new(RecoveringToken::new(Some("old-token")));
    let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
    let dialing = links.clone();
    let dial = tokio::spawn(async move { dialing.client("dev-a").await });
    wait_until(|| !lock(&token.invalidated).is_empty()).await;
    token.replace("fresh-token");
    let client = tokio::time::timeout(Duration::from_secs(5), dial).await.unwrap().unwrap().unwrap();
    {
        let state = lock(&relay.state);
        assert_eq!(state.requests.len(), 3);
        assert!(state.requests[1].contains("token=old-token"));
        assert!(state.requests[2].contains("token=fresh-token"));
    }
    let call = tokio::spawn(async move { client.call("DeliverPeerMessage", peer_params()).await });
    wait_until(|| service.peer_messages.load(Ordering::SeqCst) == 1).await;
    token.replace("newer-token");
    lock(&relay.state).clients.values().next().unwrap().send(Out::CloseCode(4401)).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), call).await.unwrap().unwrap().is_err());
    assert_eq!(*lock(&token.invalidated), vec![0, 1]);
    assert_eq!(service.peer_messages.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&relay.state).requests.len(), 3);
    let fresh = links.client("dev-a").await.expect("next caller reconnects");
    fresh.call("Echo", serde_json::json!({})).await.unwrap();
    assert!(lock(&relay.state).requests.last().unwrap().contains("token=newer-token"));
}


#[tokio::test]
async fn token_recovery_wakes_a_signed_out_host_relay_immediately() {
    let relay = FakeRelay::start().await;
    let token = Arc::new(RecoveringToken::new(None));
    let mut config = HostRelayConfig::new(relay.edge_url(), "dev-a", token.clone());
    // Without the token signal this test would remain asleep for two minutes.
    config.retry = Duration::from_secs(120);
    let _host = HostRelay::spawn(config, TestService::new("host-a"), noop_nudge());
    tokio::task::yield_now().await;
    assert!(!relay.host_connected());

    token.replace("test-user");

    relay.wait_host_connected().await;
}

#[tokio::test]
async fn sign_out_closes_a_live_host_relay_immediately() {
    let relay = FakeRelay::start().await;
    let token = Arc::new(RecoveringToken::new(Some("test-user")));
    let mut config = HostRelayConfig::new(relay.edge_url(), "dev-a", token.clone());
    // A sign-out signal must close the authenticated socket, not wait for retry.
    config.retry = Duration::from_secs(120);
    let _host = HostRelay::spawn(config, TestService::new("host-a"), noop_nudge());
    relay.wait_host_connected().await;

    token.clear();

    wait_until(|| !relay.host_connected()).await;
}

#[tokio::test]
async fn temporary_token_failure_keeps_live_host_and_peer_connections() {
    let relay = FakeRelay::start().await;
    let token = Arc::new(RecoveringToken::new(Some("test-user")));
    let _host = HostRelay::spawn(
        HostRelayConfig::new(relay.edge_url(), "dev-a", token.clone()),
        TestService::new("host-a"),
        noop_nudge(),
    );
    relay.wait_host_connected().await;
    let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
    let client = links
        .client("dev-a")
        .await
        .expect("initial peer connection");

    token.fail_temporarily();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        relay.host_connected(),
        "a refresh failure is not a sign-out"
    );
    client
        .call("Echo", serde_json::json!({}))
        .await
        .expect("live peer survives");
    links
        .client("dev-a")
        .await
        .expect("cached peer is not revoked");

    token.replace("fresh-token");
    client
        .call("Echo", serde_json::json!({}))
        .await
        .expect("peer survives recovery");
}

#[tokio::test]
async fn token_rotation_does_not_interrupt_a_live_peer_link() {
    let relay = FakeRelay::start().await;
    let _host = HostRelay::spawn(
        relay_config(&relay.edge_url(), 100),
        TestService::new("host-a"),
        noop_nudge(),
    );
    relay.wait_host_connected().await;

    let token = Arc::new(RecoveringToken::new(Some("old-token")));
    let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
    let client = links.client("dev-a").await.expect("client dials");
    token.replace("new-token");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let echoed = client
        .call("Echo", serde_json::json!({ "after": "refresh" }))
        .await
        .expect("live link survives token rotation");
    assert_eq!(echoed["params"]["after"], "refresh");
}

#[tokio::test]
async fn sign_out_closes_cached_peer_links() {
    let relay = FakeRelay::start().await;
    let _host = HostRelay::spawn(
        relay_config(&relay.edge_url(), 100),
        TestService::new("host-a"),
        noop_nudge(),
    );
    relay.wait_host_connected().await;

    let token = Arc::new(RecoveringToken::new(Some("test-user")));
    let links = LinkCache::new(LinkCacheConfig::new(relay.edge_url(), token.clone()));
    let client = links.client("dev-a").await.expect("client dials");
    client
        .call("Echo", serde_json::json!({ "before": "sign-out" }))
        .await
        .expect("link is live before sign-out");

    token.clear();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client
                .call("Echo", serde_json::json!({ "after": "sign-out" }))
                .await
                .is_err()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cached peer link survived sign-out");
    assert!(
        links.client("dev-a").await.is_err(),
        "signed-out cache must not redial"
    );
}

#[tokio::test]
async fn relay_serves_multiple_clients_end_to_end() {
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;

    let links = cache(&relay.edge_url());
    let a = links.client("dev-a").await.expect("client a dials");
    let b = links.client("dev-a").await.expect("client b reuses/dials");

    let echoed = a
        .call("Echo", serde_json::json!({ "who": "a" }))
        .await
        .expect("echo a");
    assert_eq!(echoed["host"], "host-a");
    assert_eq!(echoed["params"]["who"], "a");

    // Streaming through the relay: items arrive in order, stream terminates.
    let mut items = b
        .subscribe("Count", serde_json::json!({ "n": 3 }))
        .await
        .expect("count");
    let mut seen = Vec::new();
    while let Some(v) = items.recv().await {
        seen.push(v);
    }
    assert_eq!(
        seen,
        vec![
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!(2)
        ]
    );

    // Concurrent calls from the same cached link multiplex fine.
    let (x, y) = tokio::join!(
        a.call("Echo", serde_json::json!(1)),
        a.call("Echo", serde_json::json!(2))
    );
    assert_eq!(x.expect("x")["params"], serde_json::json!(1));
    assert_eq!(y.expect("y")["params"], serde_json::json!(2));
}

#[tokio::test]
async fn client_disconnect_tears_down_virtual_conn() {
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let active = service.active_streams.clone();
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;

    let url = device_room_ws_url(&relay.edge_url(), "dev-a", "client", Some("conn-x"), "t");
    let link = DeviceLink::connect(&url, Arc::new(StaticToken("t".into())), 0)
        .await.expect("link connects");
    let client = link.client();
    let _items = client
        .subscribe("Never", serde_json::Value::Null)
        .await
        .expect("subscribe");
    wait_until(|| active.load(Ordering::SeqCst) == 1).await;

    // Dropping the link closes the relay socket → the DO tells the host client_closed →
    // the host drops the virtual conn, aborting the client's server-side streams.
    drop(link);
    wait_until(|| active.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn host_offline_fails_fast_and_cools_down() {
    let relay = FakeRelay::start().await;
    let links = cache(&relay.edge_url());

    // No host connected: the readiness probe is bounced with host_offline → link-down →
    // the dial fails quickly instead of hanging.
    let Err(err) = links.client("dev-a").await else {
        panic!("dial must fail with no host")
    };
    let message = err.to_string();
    assert!(
        message.contains("readiness check"),
        "expected readiness failure, got: {message}"
    );

    // Immediately after, the cooldown makes callers fail fast without redialing.
    let Err(err) = links.client("dev-a").await else {
        panic!("must fail fast while cooling")
    };
    assert!(err.to_string().contains("backing off"), "got: {err}");

    // After the cooldown a host is up — dial succeeds and clears the slate.
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let client = links.client("dev-a").await.expect("dials after cooldown");
    assert_eq!(
        client
            .call("Echo", serde_json::json!({}))
            .await
            .expect("echo")["host"],
        "host-a"
    );
}

/// The data-driven cooldown reset (fresh workspace presence → peer is alive):
/// with a long backoff engaged, `reset_cooldown` lets the next call dial
/// immediately instead of waiting the window out.
#[tokio::test]
async fn presence_reset_clears_cooldown_immediately() {
    let relay = FakeRelay::start().await;
    let mut config =
        LinkCacheConfig::new(relay.edge_url(), Arc::new(StaticToken("test-user".into())));
    // Long enough that only a reset (never elapsed time) can explain success.
    config.cooldown_base = Duration::from_secs(120);
    config.cooldown_max = Duration::from_secs(120);
    config.probe_timeout = Duration::from_millis(1_500);
    let links = LinkCache::new(config);

    // No host: the dial fails and the two-minute cooldown engages.
    assert!(links.client("dev-a").await.is_err(), "no host: dial fails");
    let Err(err) = links.client("dev-a").await else {
        panic!("must fail fast while cooling")
    };
    assert!(err.to_string().contains("backing off"), "got: {err}");

    // Host comes up and its presence heartbeat clears the backoff — the next
    // call dials immediately.
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;
    links.reset_cooldown("dev-a");
    let client = links.client("dev-a").await.expect("dials after reset");
    assert_eq!(
        client
            .call("Echo", serde_json::json!({}))
            .await
            .expect("echo")["host"],
        "host-a"
    );
}

fn long_cooldown_cache(edge_url: &str) -> Arc<LinkCache> {
    let mut config = LinkCacheConfig::new(edge_url, Arc::new(StaticToken("test-user".into())));
    // Long enough that only a reset (never elapsed time) can end it.
    config.cooldown_base = Duration::from_secs(120);
    config.cooldown_max = Duration::from_secs(120);
    config.probe_timeout = Duration::from_millis(1_500);
    LinkCache::new(config)
}

/// A dead peer's own dial opens an edge socket, which is itself a
/// process-wide "online" event. Network recovery must not void that peer's
/// cooldown, or the dead host is redialed every few seconds forever.
#[tokio::test]
async fn network_recovery_keeps_a_dead_peers_cooldown() {
    let relay = FakeRelay::start().await;
    let links = long_cooldown_cache(&relay.edge_url());

    // No host: the edge socket opens but the readiness probe fails.
    assert!(links.client("dev-a").await.is_err(), "no host: dial fails");
    zeron_sync::wake::notify_online();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let Err(err) = links.client("dev-a").await else {
        panic!("a dead peer must stay parked")
    };
    assert!(err.to_string().contains("backing off"), "got: {err}");
}

/// The other half: a dial that never reached the edge belonged to the dead
/// network, so network recovery lets the next call dial at once.
#[tokio::test]
async fn network_recovery_clears_a_network_failure_cooldown() {
    let links = long_cooldown_cache("http://127.0.0.1:1");

    assert!(links.client("dev-a").await.is_err(), "no edge: dial fails");
    let Err(err) = links.client("dev-a").await else {
        panic!("must fail fast while cooling")
    };
    assert!(err.to_string().contains("backing off"), "got: {err}");

    zeron_sync::wake::notify_online();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let Err(err) = links.client("dev-a").await else {
        panic!("there is still no edge")
    };
    assert!(!err.to_string().contains("backing off"), "redialed: {err}");
}

#[tokio::test]
async fn host_supersede_drops_old_links_and_recovers() {
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;

    let links = cache(&relay.edge_url());
    let client = links.client("dev-a").await.expect("dials");
    assert!(client.call("Echo", serde_json::json!({})).await.is_ok());

    // A rogue host joins: the relay closes the HostRelay's socket (supersede) and, when
    // the rogue's socket later closes, clients see host_closed. The HostRelay backs off
    // and reclaims the room; old links are down and the cache re-dials.
    let rogue_url = device_room_ws_url(&relay.edge_url(), "dev-a", "host", None, "t");
    let (rogue_ws, _) = tokio_tungstenite::connect_async(&rogue_url)
        .await
        .expect("rogue joins");
    // Wait for the supersede to land, then let the rogue die: the HostRelay's reconnect
    // supersedes it right back (proved by its socket closing).
    let (_, mut rogue_stream) = rogue_ws.split();
    loop {
        match rogue_stream.next().await {
            Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
            Some(Ok(_)) => {}
        }
    }
    relay.wait_host_connected().await;

    // The pre-supersede link died; in-flight/new calls on it fail rather than hang.
    let err = client
        .call("Echo", serde_json::json!({}))
        .await
        .expect_err("old link dead");
    assert!(
        matches!(err, RpcError::Closed | RpcError::Transport(_)),
        "got: {err}"
    );

    // The cache notices the dead link and re-dials the reclaimed host (allowing for a
    // cooldown window if a re-dial raced the reclaim).
    let recovered = loop {
        match links.client("dev-a").await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(120)).await,
        }
    };
    let echoed = recovered
        .call("Echo", serde_json::json!({}))
        .await
        .expect("echo");
    assert_eq!(echoed["host"], "host-a");
}

#[tokio::test]
async fn nudges_reach_the_host_callback() {
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let on_nudge: zeron_rpc::NudgeHandler = Arc::new(move |chat_id| tx.send(chat_id).is_ok());
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, on_nudge);
    relay.wait_host_connected().await;

    relay.nudge("chat-42");
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("nudge delivered")
        .expect("channel open");
    assert_eq!(got, "chat-42");
}

/// Live-edge variant: run the same host+client path through a real DeviceRoom DO.
/// `ZERON_EDGE_WS=http://127.0.0.1:26640 cargo test -p zeron-rpc -- --ignored live_edge`
/// (dev-mode edge; ZERON_EDGE_TOKEN defaults to a fixed dev user id).
#[tokio::test]
#[ignore = "needs a running edge (set ZERON_EDGE_WS)"]
async fn live_edge_relay_round_trip() {
    let Ok(edge_url) = std::env::var("ZERON_EDGE_WS") else {
        panic!("set ZERON_EDGE_WS to the edge base URL (e.g. http://127.0.0.1:26640)");
    };
    let token = std::env::var("ZERON_EDGE_TOKEN").unwrap_or_else(|_| "relay-live-test".into());
    let device_id = format!("relay-live-{}", uuid::Uuid::new_v4());

    let service = TestService::new("live-host");
    let mut config = HostRelayConfig::new(
        edge_url.clone(),
        device_id.clone(),
        Arc::new(StaticToken(token.clone())),
    );
    config.retry = Duration::from_millis(500);
    let _host = HostRelay::spawn(config, service, noop_nudge());

    let mut cache_config = LinkCacheConfig::new(edge_url, Arc::new(StaticToken(token)));
    cache_config.probe_timeout = Duration::from_secs(5);
    let links = LinkCache::new(cache_config);

    // The host claims the room asynchronously; retry the dial until it answers.
    let client = loop {
        match links.client(&device_id).await {
            Ok(client) => break client,
            Err(err) => {
                eprintln!("dial retry: {err}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };
    let echoed = client
        .call("Echo", serde_json::json!({ "live": true }))
        .await
        .expect("echo");
    assert_eq!(echoed["host"], "live-host");
    assert_eq!(echoed["params"]["live"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn zombie_relay_path_trips_the_echo_deadline() {
    // 2026-08-19 incident shape: the client↔edge leg stays healthy (pongs
    // flow), the edge↔host leg is dead — the link used to sit "open" for
    // minutes, retrying frames into the void, until an unrelated host-session
    // cycle exposed it. The app-level echo must rule the link dead within its
    // deadline instead.
    zeron_rpc::device_room::set_client_liveness_for_tests(
        Duration::from_millis(100),
        Duration::from_millis(600),
    );
    let relay = FakeRelay::start().await;
    let service = TestService::new("host-a");
    let _host = HostRelay::spawn(relay_config(&relay.edge_url(), 100), service, noop_nudge());
    relay.wait_host_connected().await;

    let url = device_room_ws_url(&relay.edge_url(), "dev-a", "client", Some("c-echo"), "tok");
    let link = DeviceLink::connect(&url, Arc::new(StaticToken("tok".into())), 0)
        .await.expect("client link dials");
    // Healthy phase: echoes flow, the deadline never trips.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!link.is_closed(), "echoing link must stay open");

    relay.set_blackhole_host_bound(true);
    wait_until(|| link.is_closed()).await;
    assert_eq!(
        link.closed().borrow().as_deref(),
        Some("host echo silent"),
        "the zombie path must be ruled dead by the echo deadline"
    );
    // Restore the production clocks for the rest of the process's tests.
    zeron_rpc::device_room::set_client_liveness_for_tests(Duration::ZERO, Duration::ZERO);
}

#[tokio::test]
async fn nudge_ack_requires_durable_acceptance_and_echoes_the_exact_token() {
    let relay = FakeRelay::start().await;
    let accept = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let callback: zeron_rpc::NudgeHandler = Arc::new({
        let accept = accept.clone();
        move |id| {
            let _ = tx.send(id);
            accept.load(Ordering::SeqCst)
        }
    });
    let _host = HostRelay::spawn(
        relay_config(&relay.edge_url(), 100),
        TestService::new("host"),
        callback,
    );
    relay.wait_host_connected().await;
    relay.nudge_receipt("chat", Some("receipt-v1"));
    rx.recv().await.unwrap();
    assert!(relay.state.lock().unwrap().nudge_acks.is_empty());
    accept.store(true, Ordering::SeqCst);
    relay.nudge_receipt("chat", Some("receipt-v2"));
    tokio::time::timeout(Duration::from_secs(3), async {
        while relay.state.lock().unwrap().nudge_acks.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        relay.state.lock().unwrap().nudge_acks[0],
        serde_json::json!({"chatId":"chat","token":"receipt-v2"})
    );
}
