//! M4b integration: `targetDeviceId` routing — engine A forwards device-addressed RPCs
//! to engine B through B's device-room relay (host relay on B, link cache on A), with a
//! minimal in-memory device-room standing in for the edge DO (route client→host with
//! `from` stamped, host→client by `to`).

// tungstenite's `accept_hdr_async` callback signature fixes the Err type as a full
// `Response` — its size is not ours to shrink.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};

use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
use zeron_engine::{
    BranchHeadContext, ChangeRequestError, CheckoutChangeRequestLookup, CheckoutChangeRequests,
    CheckoutSourceContext, EngineCore, HarnessRegistry,
};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChangeRequestState, ChangeRequestSummary, DoneStatus, HarnessId, Model,
    ReasoningLevel, RunRequest, SandboxLevel, SteeringMode,
};
use zeron_rpc::{
    DeviceFrameHeader, HostRelay, HostRelayConfig, LinkCache, LinkCacheConfig, RpcError, RpcReply,
    RpcService, StaticToken, decode_device_frame, encode_device_frame, methods,
};

// ---------------------------------------------------------------------------
// Minimal in-memory device room (route-only subset of the DO semantics)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RelayState {
    host: Option<mpsc::UnboundedSender<Vec<u8>>>,
    clients: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
}

async fn fake_device_room() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let url = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let state = Arc::new(Mutex::new(RelayState::default()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |req: &WsRequest, res: WsResponse| {
                        uri = req.uri().to_string();
                        Ok(res)
                    },
                )
                .await
                else {
                    return;
                };
                let query = uri.split_once('?').map(|(_, q)| q).unwrap_or("");
                let is_host = query.contains("role=host");
                let conn_id = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("connId="))
                    .unwrap_or("anon")
                    .to_string();
                let (mut sink, mut ws_stream) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                {
                    let mut st = state.lock().expect("lock");
                    if is_host {
                        st.host = Some(tx);
                    } else {
                        st.clients.insert(conn_id.clone(), tx);
                    }
                }
                let writer = tokio::spawn(async move {
                    while let Some(bytes) = rx.recv().await {
                        if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = ws_stream.next().await {
                    let WsMessage::Binary(bytes) = message else {
                        continue;
                    };
                    let Ok((header, payload)) = decode_device_frame(&bytes) else {
                        break;
                    };
                    let st = state.lock().expect("lock");
                    if is_host {
                        let Some(to) = header.to else { continue };
                        if let Some(client) = st.clients.get(&to) {
                            let stripped = DeviceFrameHeader::new(header.s, header.k);
                            let _ = client
                                .send(encode_device_frame(&stripped, &payload).expect("encode"));
                        }
                    } else if let Some(host) = &st.host {
                        let mut routed = DeviceFrameHeader::new(header.s, header.k);
                        routed.from = Some(conn_id.clone());
                        let _ = host.send(encode_device_frame(&routed, &payload).expect("encode"));
                    }
                }
                writer.abort();
            });
        }
    });
    (url, task)
}

// ---------------------------------------------------------------------------
// Engine fixtures
// ---------------------------------------------------------------------------

/// Instant mock harness so a forwarded QueueCommand fully executes on the target.
struct InstantHarness(HarnessId);

#[async_trait]
impl Harness for InstantHarness {
    fn id(&self) -> HarnessId {
        self.0
    }
    fn display_name(&self) -> &str {
        "Instant"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([
            Ok(AgentEvent::SessionStarted {
                harness: self.0,
                model: "instant-1".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "hs-1".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: "remote reply".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("hs-1".into()),
            }),
        ])
        .boxed())
    }
}

fn registry() -> Arc<HarnessRegistry> {
    registry_for(HarnessId::Mock)
}

fn registry_for(harness: HarnessId) -> Arc<HarnessRegistry> {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(InstantHarness(harness)));
    Arc::new(registry)
}

fn assemble(dir: &std::path::Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("create data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("write device id");
    EngineCore::assemble(dir, registry(), HarnessId::Mock, None).expect("engine assembles")
}

async fn git(cwd: &std::path::Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn init_workspace_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path.join("src")).expect("repo tree");
    git(path, &["init", "-b", "main"]).await;
    std::fs::write(
        path.join("src/remote.rs"),
        "pub const REMOTE: bool = true;\n",
    )
    .expect("remote source");
    git(path, &["add", "."]).await;
    git(path, &["commit", "-m", "initial"]).await;
}

struct StaticChangeRequestLookup {
    source: CheckoutSourceContext,
    summary: ChangeRequestSummary,
    resolves: AtomicUsize,
}

/// Models a host from before the checkout change-request watch existed.
struct LegacyChangeRequestService {
    list_harnesses_calls: AtomicUsize,
}

#[async_trait]
impl RpcService for LegacyChangeRequestService {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::LIST_HARNESSES => {
                self.list_harnesses_calls.fetch_add(1, Ordering::AcqRel);
                RpcReply::value(&serde_json::json!([]))
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

#[async_trait]
impl CheckoutChangeRequestLookup for StaticChangeRequestLookup {
    async fn inspect_checkout(
        &self,
        _cwd: &std::path::Path,
    ) -> Result<CheckoutSourceContext, ChangeRequestError> {
        Ok(self.source.clone())
    }

    async fn resolve_github_source(
        &self,
        _source: &CheckoutSourceContext,
    ) -> Result<Option<ChangeRequestSummary>, ChangeRequestError> {
        self.resolves.fetch_add(1, Ordering::AcqRel);
        Ok(Some(self.summary.clone()))
    }
}

fn init_git_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).expect("create git fixture");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(path)
        .status()
        .expect("spawn git init");
    assert!(status.success(), "git init failed");
}

fn change_request_lookup(root: &std::path::Path) -> Arc<StaticChangeRequestLookup> {
    Arc::new(StaticChangeRequestLookup {
        source: CheckoutSourceContext {
            checkout_root: root.to_owned(),
            branch: BranchHeadContext::resolve(
                "feature/status",
                Some("origin/feature/status"),
                Some("origin"),
                Some("https://github.com/acme/zeron.git"),
            ),
            default_branch: Some("main".into()),
        },
        summary: ChangeRequestSummary {
            provider: "github".into(),
            number: 90,
            title: "Stream checkout pull request".into(),
            url: "https://github.com/acme/zeron/pull/90".into(),
            state: ChangeRequestState::Open,
            base_ref: "main".into(),
            head_ref: "feature/status".into(),
            signals: None,
        },
        resolves: AtomicUsize::new(0),
    })
}

/// Exercise the device-room wire format used by the native iOS relay client,
/// without routing through another desktop engine first.
async fn simulated_ios_change_request(
    relay_url: &str,
    conn_id: &str,
    cwd: &std::path::Path,
) -> serde_json::Value {
    let ws_url = relay_url.replacen("http://", "ws://", 1);
    let url = format!("{ws_url}/device/device-b/ws?role=client&connId={conn_id}&token=ios-token");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("iOS relay client connects");
    let request = serde_json::json!({
        "id": 7,
        "method": methods::WATCH_CHECKOUT_CHANGE_REQUEST,
        "params": { "cwd": cwd },
    });
    let header = DeviceFrameHeader::new("rpc", "rpc");
    let frame = encode_device_frame(&header, request.to_string().as_bytes()).expect("encode RPC");
    socket
        .send(WsMessage::Binary(frame))
        .await
        .expect("send iOS subscription");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("iOS item before timeout")
            .expect("iOS relay remains connected")
            .expect("valid iOS relay frame");
        let WsMessage::Binary(bytes) = message else {
            continue;
        };
        let (header, payload) = decode_device_frame(&bytes).expect("decode iOS relay frame");
        assert_eq!(header.k, "rpc");
        let response: serde_json::Value =
            serde_json::from_slice(&payload).expect("decode RPC response");
        assert_eq!(response["id"], 7);
        if let Some(error) = response["err"].as_str() {
            panic!("iOS subscription failed: {error}");
        }
        if let Some(item) = response.get("item") {
            return item.clone();
        }
    }
}

fn assert_public_change_request_payload(item: &serde_json::Value) {
    let mut keys: Vec<_> = item
        .as_object()
        .expect("status object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "branch",
            "changeRequest",
            "checkoutId",
            "cwd",
            "deviceId",
            "updatedAt",
        ]
    );
    let mut summary_keys: Vec<_> = item["changeRequest"]
        .as_object()
        .expect("change request object")
        .keys()
        .map(String::as_str)
        .collect();
    summary_keys.sort_unstable();
    assert_eq!(
        summary_keys,
        [
            "baseRef", "headRef", "number", "provider", "state", "title", "url"
        ]
    );
    let payload = item.to_string();
    assert!(!payload.contains("ios-token"));
    assert!(!payload.to_ascii_lowercase().contains("stderr"));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkout_change_request_stream_matches_locally_and_through_device_routing() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let checkout = dirs.path().join("checkout");
    init_git_repo(&checkout);

    let mut core_b = assemble(&dirs.path().join("b"), "device-b");
    core_b
        .workspace
        .create_space(
            "space-pr",
            "device-b",
            &checkout.to_string_lossy(),
            None,
            true,
        )
        .expect("checkout space");
    let lookup = change_request_lookup(&checkout);
    core_b.change_requests =
        CheckoutChangeRequests::new(core_b.repos.clone(), "device-b", lookup.clone());
    let local_client = zeron_rpc::memory_client(core_b.rpc_service());
    let rejected = match local_client
        .subscribe_checked(
            methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            serde_json::json!({ "cwd": dirs.path().join("not-registered") }),
        )
        .await
    {
        Ok(_) => panic!("unregistered cwd must be rejected"),
        Err(error) => error,
    };
    assert!(rejected.to_string().contains("not a known checkout"));
    let mut local = local_client
        .subscribe_checked(
            methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            serde_json::json!({ "cwd": checkout }),
        )
        .await
        .expect("local change request subscribe");
    let local_frame = local.recv().await.expect("local initial frame");

    let _host = core_b.start_host_relay(&relay_url);
    let core_a = assemble(&dirs.path().join("a"), "device-a");
    let mut link_config =
        LinkCacheConfig::new(relay_url.clone(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core_a.rpc_service());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut remote = loop {
        match client
            .subscribe_checked(
                methods::WATCH_CHECKOUT_CHANGE_REQUEST,
                serde_json::json!({
                    "cwd": checkout,
                    "targetDeviceId": "device-b",
                }),
            )
            .await
        {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let remote_frame = tokio::time::timeout(Duration::from_secs(5), remote.recv())
        .await
        .expect("remote frame before timeout")
        .expect("remote initial frame");
    assert_eq!(remote_frame, local_frame);

    // A second desktop subscription and native mobile clients all consume B's
    // host cache. Neither A nor iOS performs its own GitHub lookup.
    let mut second_desktop = client
        .subscribe_checked(
            methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            serde_json::json!({
                "cwd": checkout,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("second desktop subscription");
    let second_desktop_frame = second_desktop
        .recv()
        .await
        .expect("second desktop initial frame");
    assert_eq!(second_desktop_frame, local_frame);

    let ios_frame = simulated_ios_change_request(&relay_url, "ios-1", &checkout).await;
    assert_eq!(ios_frame, local_frame);
    assert_public_change_request_payload(&ios_frame);

    // Reconnecting creates a new relay peer and receives a fresh initial
    // snapshot from the still-warm host cache.
    let reconnected_ios_frame = simulated_ios_change_request(&relay_url, "ios-2", &checkout).await;
    assert_eq!(reconnected_ios_frame, local_frame);
    assert_eq!(
        lookup.resolves.load(Ordering::Acquire),
        1,
        "all device-boundary subscribers must share one host resolution"
    );

    core_a.disconnect_edge();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), remote.recv())
            .await
            .expect("remote stream closes after link")
            .is_none()
    );

    core_a.shutdown().await;
    core_b.shutdown().await;
}

/// Clients watch a chat's `sourceContext.repoRoot`. It is canonical, so it can
/// differ from the chat's cwd (always on Windows, where it is `\\?\C:\…`). A
/// chat outside every project must still resolve, without a space scan, and
/// a root that is gone must be rejected rather than resolved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkout_change_request_accepts_a_local_chats_source_root() {
    let dirs = tempfile::tempdir().expect("tempdir");
    let checkout = dirs.path().join("checkout");
    init_git_repo(&checkout);
    let app = checkout.join("app");
    std::fs::create_dir_all(&app).expect("app folder");
    let repo_root = std::fs::canonicalize(&checkout).expect("canonical checkout");

    let mut core = assemble(&dirs.path().join("b"), "device-b");
    core.change_requests = CheckoutChangeRequests::new(
        core.repos.clone(),
        "device-b",
        change_request_lookup(&checkout),
    );
    core.workspace
        .create_chat(
            "chat-outside-projects",
            None,
            Some("device-b"),
            None,
            Some(app.to_string_lossy().into_owned()),
        )
        .expect("chat");
    core.workspace
        .set_chat_source_context(
            "chat-outside-projects",
            &zeron_proto::ConversationSourceContext {
                checkout_id: "checkout".into(),
                repo_root: repo_root.to_string_lossy().into_owned(),
                cwd: app.to_string_lossy().into_owned(),
                branch: "feature/status".into(),
                head_sha: None,
                observed_at: chrono::Utc::now(),
            },
        )
        .expect("source context");
    let client = zeron_rpc::memory_client(core.rpc_service());

    let mut watch = client
        .subscribe_checked(
            methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            serde_json::json!({ "cwd": repo_root }),
        )
        .await
        .expect("a local chat's source root is a known checkout");
    tokio::time::timeout(Duration::from_secs(5), watch.recv())
        .await
        .expect("frame before timeout")
        .expect("initial frame");

    std::fs::remove_dir_all(&checkout).expect("remove checkout");
    let rejected = match client
        .subscribe_checked(
            methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            serde_json::json!({ "cwd": repo_root }),
        )
        .await
    {
        Ok(_) => panic!("a removed checkout must be rejected"),
        Err(error) => error,
    };
    assert!(rejected.to_string().contains("not a known checkout"));

    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_remote_change_request_watch_keeps_the_shared_device_link() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let legacy = Arc::new(LegacyChangeRequestService {
        list_harnesses_calls: AtomicUsize::new(0),
    });
    let _legacy_host = HostRelay::spawn(
        HostRelayConfig::new(
            &relay_url,
            "legacy-device",
            Arc::new(StaticToken("test-user".into())),
        ),
        legacy.clone(),
        Arc::new(|_| true),
    );

    let core = assemble(&dirs.path().join("new"), "new-device");
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core.rpc_service());

    // The host can take a moment to attach to the room. Once attached, an old
    // host rejects only the capability added by this version.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    for watch_method in [
        methods::WATCH_CHECKOUT_CHANGE_REQUEST,
        methods::WATCH_WORKSPACE_GIT_STATUS,
    ] {
        loop {
            match client
                .subscribe_checked(
                    watch_method,
                    serde_json::json!({
                        "cwd": "/legacy-checkout",
                        "targetDeviceId": "legacy-device",
                    }),
                )
                .await
            {
                Err(RpcError::UnknownMethod(method)) => {
                    assert_eq!(method, watch_method);
                    break;
                }
                Ok(_) => panic!("legacy host must reject the change-request watch"),
                Err(error) => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "legacy host never came up: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }

    let harnesses = client
        .call(
            methods::LIST_HARNESSES,
            serde_json::json!({ "targetDeviceId": "legacy-device" }),
        )
        .await
        .expect("a capability rejection must not sever the shared link");
    assert_eq!(harnesses, serde_json::json!([]));
    assert_eq!(
        legacy.list_harnesses_calls.load(Ordering::Acquire),
        2,
        "one readiness probe and one forwarded call prove the existing link was reused"
    );

    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_is_computed_on_the_checkout_host_through_the_relay() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().unwrap();
    let root = dirs.path().join("checkout");
    init_git_repo(&root);
    std::fs::create_dir(root.join("new")).unwrap();
    std::fs::write(root.join("new/remote.rs"), "// host only\n").unwrap();
    let host = assemble(&dirs.path().join("host"), "host-device");
    host.workspace
        .create_space(
            "remote-space",
            "host-device",
            &root.to_string_lossy(),
            None,
            true,
        )
        .unwrap();
    host.workspace
        .create_chat("remote-chat", Some("remote-space"), None, None, None)
        .unwrap();
    host.diff_sync.reconcile_now().await;
    let local_client = zeron_rpc::memory_client(host.rpc_service());
    let mut local = local_client
        .subscribe_checked(
            methods::WATCH_WORKSPACE_GIT_STATUS,
            serde_json::json!({"chatId": "remote-chat"}),
        )
        .await
        .unwrap();
    let expected = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = local.recv().await.unwrap();
            if !frame["status"].is_null() {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(expected["status"]["files"][0]["path"], "new/remote.rs");
    assert_eq!(expected["status"]["deviceId"], "host-device");
    assert!(expected["status"].get("patch").is_none());

    let _host_relay = host.start_host_relay(&relay_url);
    let consumer = assemble(&dirs.path().join("consumer"), "consumer-device");
    let mut config = LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    config.probe_timeout = Duration::from_secs(5);
    consumer.set_links(LinkCache::new(config));
    let client = zeron_rpc::memory_client(consumer.rpc_service());
    // The consumer has no corresponding space/chat: resolving locally must fail.
    assert!(
        client
            .subscribe_checked(
                methods::WATCH_WORKSPACE_GIT_STATUS,
                serde_json::json!({"chatId": "remote-chat"})
            )
            .await
            .is_err()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut remote = loop {
        match client
            .subscribe_checked(
                methods::WATCH_WORKSPACE_GIT_STATUS,
                serde_json::json!({"chatId": "remote-chat", "targetDeviceId": "host-device"}),
            )
            .await
        {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay unavailable: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let actual = tokio::time::timeout(Duration::from_secs(5), remote.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
    drop(remote);
    consumer.shutdown().await;
    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_device_id_routes_over_the_relay() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");

    // Engine B hosts its device room on the fake relay.
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    let _host = core_b.start_host_relay(&relay_url);

    // Engine A dials peers through the same relay.
    let core_a = assemble(&dirs.path().join("a"), "device-a");
    let mut link_config =
        LinkCacheConfig::new(relay_url.clone(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));

    // Seed a transcript on B only — proves reads come from B, not A's (empty) doc.
    let handle_b = core_b.doc_host.open("chat-remote").expect("open chat on B");
    handle_b
        .write_user_message("m-b-1", "hello from B", 1_000)
        .expect("write user message");

    let client = zeron_rpc::memory_client(core_a.rpc_service());

    // Our own id in targetDeviceId: handled locally, no forward.
    let local = client
        .call(
            methods::LIST_HARNESSES,
            serde_json::json!({ "targetDeviceId": "device-a" }),
        )
        .await
        .expect("local list");
    assert!(local.is_array());

    // Unary forward: ListHarnesses answered by B through the relay. (The host relay
    // dials with backoff; retry until its session is up.)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let remote = loop {
        match client
            .call(
                methods::LIST_HARNESSES,
                serde_json::json!({ "targetDeviceId": "device-b" }),
            )
            .await
        {
            Ok(value) => break value,
            Err(err) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {err}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    assert!(remote.is_array());

    // Generated media uses the same jail locally and over a targeted peer RPC.
    let generated_root = dirs.path().join("generated_images");
    std::fs::create_dir_all(&generated_root).unwrap();
    let source = generated_root.join("codex.png");
    let payload = b"\x89PNG\r\n\x1a\nremote-generated-image";
    std::fs::write(&source, payload).unwrap();
    let image = core_b
        .uploads
        .import_generated_image(&source, &generated_root, "chat-remote\0image")
        .unwrap();
    let local_b = zeron_rpc::memory_client(core_b.rpc_service());
    let local_image = local_b
        .call(
            methods::READ_ATTACHMENT_CHUNK,
            serde_json::json!({"path": image.path, "offset": 0}),
        )
        .await
        .unwrap();
    let remote_image = client
        .call(
            methods::READ_ATTACHMENT_CHUNK,
            serde_json::json!({"path": image.path, "offset": 0, "targetDeviceId": "device-b"}),
        )
        .await
        .unwrap();
    assert_eq!(remote_image, local_image);
    assert_eq!(remote_image["mimeType"], "image/png");
    assert_eq!(remote_image["done"], true);
    assert!(
        client
            .call(
                methods::READ_ATTACHMENT_CHUNK,
                serde_json::json!({"path": source, "offset": 0, "targetDeviceId": "device-b"})
            )
            .await
            .is_err()
    );

    // A reply's Markdown image reads on the chat's host, within its folder.
    let pictures = dirs.path().join("b-chat-folder");
    std::fs::create_dir_all(&pictures).unwrap();
    std::fs::write(pictures.join("shot.png"), payload).unwrap();
    core_b
        .workspace
        .create_chat(
            "chat-pictures",
            None,
            Some("device-b"),
            None,
            Some(pictures.to_string_lossy().into_owned()),
        )
        .unwrap();
    let params = serde_json::json!({
        "chatId": "chat-pictures",
        "path": pictures.join("shot.png"),
        "offset": 0,
    });
    let local_picture = local_b
        .call(methods::READ_CHAT_IMAGE_CHUNK, params.clone())
        .await
        .unwrap();
    let mut targeted = params.clone();
    targeted["targetDeviceId"] = "device-b".into();
    let remote_picture = client
        .call(methods::READ_CHAT_IMAGE_CHUNK, targeted)
        .await
        .unwrap();
    assert_eq!(remote_picture, local_picture);
    assert_eq!(remote_picture["mimeType"], "image/png");
    // A doesn't host that chat, so it never serves it itself.
    assert!(
        client
            .call(methods::READ_CHAT_IMAGE_CHUNK, params)
            .await
            .is_err()
    );

    // The add-space picker's exact call: browse a folder ON B from A's IPC
    // surface (ListFolders + targetDeviceId, relay-forwarded).
    let browse_dir = dirs.path().join("b-folders");
    std::fs::create_dir_all(browse_dir.join("project-x")).expect("browse fixture");
    let listing = client
        .call(
            methods::LIST_FOLDERS,
            serde_json::json!({
                "path": browse_dir.to_string_lossy(),
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote ListFolders");
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(
        names.contains(&"project-x"),
        "remote folder listing must come from B's filesystem: {names:?}"
    );

    // Worktree preferences and their filesystem validation belong to B.
    let a_settings = core_a.repos.worktree_settings();
    let custom_worktrees = dirs.path().join("b-custom-worktrees");
    let saved = client
        .call(
            methods::SET_WORKTREE_SETTINGS,
            serde_json::json!({
                "targetDeviceId": "device-b",
                "useCustomDirectory": true,
                "customDirectory": custom_worktrees,
            }),
        )
        .await
        .expect("save worktree location on B");
    let remote_settings = client
        .call(
            methods::GET_WORKTREE_SETTINGS,
            serde_json::json!({
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("read worktree location on B");
    assert_eq!(saved, remote_settings);
    assert_eq!(
        core_a.repos.worktree_settings(),
        a_settings,
        "A's preferences are unchanged"
    );
    assert_eq!(
        std::path::PathBuf::from(saved["effectiveDirectory"].as_str().unwrap()),
        custom_worktrees.canonicalize().unwrap()
    );

    // Streaming proxy: WatchDocMessages against B's doc from A's IPC surface.
    let mut stream = client
        .subscribe_scoped(
            methods::WATCH_DOC_MESSAGES,
            serde_json::json!({ "chatId": "chat-remote", "targetDeviceId": "device-b" }),
        )
        .await
        .expect("remote subscribe");
    // The watch emits its current value first ([] if B's publish pass hasn't run yet),
    // then re-emits on every doc change — read until B's entry arrives.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let item = tokio::time::timeout_at(deadline, stream.recv())
            .await
            .expect("remote transcript before timeout")
            .expect("stream alive");
        if item.to_string().contains("hello from B") {
            break;
        }
    }

    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while core_b.doc_host.sync_resources()["retainedBy"]["views"]
            .as_u64()
            .unwrap()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("quiet remote transcript released its view");

    // Unary forward with side effects: QueueCommand lands (and executes) on B.
    let command = serde_json::to_value(SessionCommandPayload::Run {
        request: RunRequest {
            mcp: None,
            prompt: "run remotely".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: "/tmp".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            access: Default::default(),
            plan: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
            extra_mcp: Vec::new(),
        },
        message_id: "m-a-1".into(),
    })
    .expect("serialize command");
    let queued = client
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({
                "chatId": "chat-remote",
                "targetDeviceId": "device-b",
                "command": command,
            }),
        )
        .await
        .expect("queue on B");
    let command_id = queued["commandId"]
        .as_str()
        .expect("command id")
        .to_string();
    let commands = handle_b.doc().read_commands().expect("read B commands");
    assert!(
        commands.iter().any(|c| c.id == command_id),
        "command must live in B's doc"
    );

    // Project Actions are also device-routed and persist only in B's private
    // profile store. A versioned project file only contributes import offers.
    let project_root = dirs.path().join("project-on-b");
    std::fs::create_dir_all(&project_root).expect("project root on B");
    git(&project_root, &["init", "-b", "main"]).await;
    std::fs::write(project_root.join("README.md"), "host B\n").expect("seed repo on B");
    std::fs::write(
        project_root.join("zeron.json"),
        r#"{"actions":[{"name":"Lint","command":"pnpm lint","icon":"lint"}]}"#,
    )
    .expect("project file");
    git(&project_root, &["add", "."]).await;
    git(&project_root, &["commit", "-m", "seed"]).await;
    core_b
        .workspace
        .create_space(
            "space-actions",
            "device-b",
            &project_root.to_string_lossy(),
            None,
            true,
        )
        .expect("space row on B");
    let listed = client
        .call(
            methods::LIST_PROJECT_ACTIONS,
            serde_json::json!({
                "spaceId": "space-actions",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("list remote Actions");
    assert_eq!(listed["actions"].as_array().unwrap().len(), 0);
    assert_eq!(listed["importableActions"].as_array().unwrap().len(), 1);

    let saved = client
        .call(
            methods::UPSERT_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "space-actions",
                "targetDeviceId": "device-b",
                "action": {
                    "name": "Lint",
                    "command": "printf 'remote-action\\n' > action-marker; if [ -n \"$ZERON_WORKTREE_PATH\" ]; then printf 'ROOT=%s\\nWT=%s\\nCWD=%s\\n' \"$ZERON_PROJECT_ROOT\" \"$ZERON_WORKTREE_PATH\" \"$PWD\" > setup-marker; fi; printf 'remote-action\\n'",
                    "icon": "lint",
                    "runOnWorktreeCreate": true,
                },
            }),
        )
        .await
        .expect("save remote Action");
    let action_id = saved["actions"][0]["id"]
        .as_str()
        .expect("normalized action id")
        .to_string();
    assert!(saved["importableActions"].as_array().unwrap().is_empty());
    assert_eq!(
        core_b
            .project_actions
            .actions("space-actions", &project_root)
            .expect("B store")
            .len(),
        1
    );
    assert!(
        core_a
            .project_actions
            .actions("space-actions", &project_root)
            .expect("A store")
            .is_empty(),
        "the forwarding engine must not persist the command"
    );

    core_b
        .workspace
        .create_chat("chat-actions", Some("space-actions"), None, None, None)
        .expect("Action chat on B");
    let run = client
        .call(
            methods::RUN_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "space-actions",
                "chatId": "chat-actions",
                "actionId": action_id,
                "cols": 80,
                "rows": 24,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("run remote Action");
    let action_terminal = run["terminal"]["id"]
        .as_str()
        .expect("Action terminal id")
        .to_string();
    let mut action_stream = client
        .subscribe(
            methods::SUBSCRIBE_TERMINAL,
            serde_json::json!({
                "terminalId": action_terminal,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("subscribe remote Action");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let item = tokio::time::timeout_at(deadline, action_stream.recv())
            .await
            .expect("remote Action output before timeout")
            .expect("Action stream alive");
        if item["type"] == "data" {
            let output = BASE64
                .decode(item["data"].as_str().expect("Action data"))
                .expect("Action base64");
            if String::from_utf8_lossy(&output).contains("remote-action")
                && project_root.join("action-marker").exists()
            {
                break;
            }
        }
    }
    assert_eq!(
        std::fs::read_to_string(project_root.join("action-marker")).expect("marker on B"),
        "remote-action\n"
    );
    assert!(
        !dirs.path().join("a").join("action-marker").exists(),
        "remote execution must not fall back to A's filesystem"
    );
    client
        .call(
            methods::CLOSE_TERMINAL,
            serde_json::json!({
                "terminalId": action_terminal,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("close remote Action terminal");

    // New worktree setup runs entirely on B and returns an already-open PTY.
    // Subscribe after a delay to prove the early output is recovered by replay.
    let setup_outcome = client
        .call(
            methods::CREATE_WORKTREE,
            serde_json::json!({
                "repoPath": project_root,
                "branch": "main",
                "spaceId": "space-actions",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("create remote worktree with setup");
    assert!(setup_outcome.get("setupError").is_none());
    let setup_terminal = setup_outcome["setupAction"]["terminal"]["id"]
        .as_str()
        .expect("remote setup terminal")
        .to_string();
    let setup_worktree = std::path::PathBuf::from(
        setup_outcome["path"]
            .as_str()
            .expect("remote setup worktree"),
    );
    assert!(setup_worktree.starts_with(custom_worktrees.canonicalize().unwrap()));
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut setup_stream = client
        .subscribe(
            methods::SUBSCRIBE_TERMINAL,
            serde_json::json!({
                "terminalId": setup_terminal,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("subscribe remote setup replay");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let item = tokio::time::timeout_at(deadline, setup_stream.recv())
            .await
            .expect("remote setup replay before timeout")
            .expect("setup stream alive");
        if item["type"] == "data" {
            let output = BASE64
                .decode(item["data"].as_str().expect("setup data"))
                .expect("setup base64");
            if String::from_utf8_lossy(&output).contains("remote-action")
                && setup_worktree.join("setup-marker").exists()
            {
                break;
            }
        }
    }
    let canonical_project = std::fs::canonicalize(&project_root).expect("canonical B project");
    let canonical_worktree = std::fs::canonicalize(&setup_worktree).expect("canonical B worktree");
    let setup_marker =
        std::fs::read_to_string(setup_worktree.join("setup-marker")).expect("setup marker on B");
    assert!(setup_marker.contains(&format!("ROOT={}", canonical_project.display())));
    assert!(setup_marker.contains(&format!("WT={}", canonical_worktree.display())));
    assert!(setup_marker.contains(&format!("CWD={}", canonical_worktree.display())));
    assert!(!dirs.path().join("a").join("setup-marker").exists());
    client
        .call(
            methods::CLOSE_TERMINAL,
            serde_json::json!({
                "terminalId": setup_terminal,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("close remote setup terminal");
    client
        .call(
            methods::DELETE_WORKTREE,
            serde_json::json!({
                "repoPath": project_root,
                "worktreePath": setup_worktree,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("delete remote setup worktree");

    let deleted = client
        .call(
            methods::DELETE_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "space-actions",
                "actionId": action_id,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("delete remote Action");
    assert!(deleted["actions"].as_array().unwrap().is_empty());

    let moved_root = dirs.path().join("project-moved-on-b");
    std::fs::create_dir_all(&moved_root).expect("moved project root");
    let mut moved_space = core_b
        .workspace
        .space("space-actions")
        .expect("read space")
        .expect("space exists");
    moved_space.path = moved_root.to_string_lossy().to_string();
    core_b
        .workspace
        .import_space_row(&moved_space)
        .expect("replace space root");
    let changed_root = client
        .call(
            methods::LIST_PROJECT_ACTIONS,
            serde_json::json!({
                "spaceId": "space-actions",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect_err("changed project root rejected by B");
    assert!(
        changed_root
            .to_string()
            .contains("identity no longer matches")
    );

    let missing = client
        .call(
            methods::LIST_PROJECT_ACTIONS,
            serde_json::json!({
                "spaceId": "missing",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect_err("missing space rejected by B");
    assert!(missing.to_string().contains("Project space not found"));
    core_b
        .workspace
        .create_space(
            "space-wrong-owner",
            "device-a",
            &project_root.to_string_lossy(),
            None,
            true,
        )
        .expect("foreign-owned row on B");
    let wrong_owner = client
        .call(
            methods::LIST_PROJECT_ACTIONS,
            serde_json::json!({
                "spaceId": "space-wrong-owner",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect_err("foreign-owned space rejected by B");
    assert!(
        wrong_owner
            .to_string()
            .contains("belongs to another device")
    );

    core_a.shutdown().await;
    core_b.shutdown().await;
}

/// M5: terminals are device-addressable — OpenTerminal/WriteTerminal forward as
/// unary calls and SubscribeTerminal proxies its stream through the relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_stream_proxies_over_the_relay() {
    exercise_terminal_relay(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projectless_terminal_stream_proxies_over_the_relay() {
    exercise_terminal_relay(true).await;
}

async fn exercise_terminal_relay(projectless: bool) {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let cwd = dirs.path().join("work");
    std::fs::create_dir_all(&cwd).expect("cwd");

    // Engine B hosts its device room and resolves the terminal cwd from its row.
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    if !projectless {
        core_b
            .workspace
            .create_space(
                "space-term",
                "device-b",
                &cwd.to_string_lossy(),
                None,
                false,
            )
            .expect("space row on B");
    }
    core_b
        .workspace
        .create_chat(
            "chat-term",
            if projectless {
                None
            } else {
                Some("space-term")
            },
            Some("device-b"),
            None,
            None,
        )
        .expect("chat row on B");
    let _host = core_b.start_host_relay(&relay_url);

    let core_a = assemble(&dirs.path().join("a"), "device-a");
    // A conflicting local row makes accidental local dispatch observable even
    // though both engines in this test share the process's home directory.
    core_a
        .workspace
        .create_chat(
            "chat-term",
            None,
            Some("device-b"),
            None,
            Some(
                dirs.path()
                    .join("missing-on-a")
                    .to_string_lossy()
                    .into_owned(),
            ),
        )
        .expect("decoy chat row on A");
    let mut link_config =
        LinkCacheConfig::new(relay_url.clone(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core_a.rpc_service());

    // OpenTerminal forwards to B once the relay session is up.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let session = loop {
        match client
            .call(
                methods::OPEN_TERMINAL,
                serde_json::json!({
                    "chatId": "chat-term",
                    "cols": 80,
                    "rows": 24,
                    "targetDeviceId": "device-b",
                }),
            )
            .await
        {
            Ok(session) => break session,
            Err(err) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {err}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let terminal_id = session["id"].as_str().expect("terminal id").to_string();
    let expected_cwd = if projectless {
        // B's own scratch folder for the chat, created when the terminal opened.
        let chat = core_b.workspace.chat("chat-term").unwrap().unwrap();
        let chat_cwd = chat.cwd.clone().unwrap();
        assert!(
            zeron_proto::projectless::is_scratch_cwd(&chat_cwd),
            "{chat_cwd}"
        );
        assert_eq!(chat.space_id, None);
        assert!(core_b.workspace.read_spaces().unwrap().is_empty());
        let name = chat_cwd.rsplit('/').next().unwrap();
        dirs.path()
            .join("b")
            .join("scratch")
            .join(name)
            .to_string_lossy()
            .into_owned()
    } else {
        cwd.to_string_lossy().into_owned()
    };
    assert_eq!(
        session["cwd"].as_str(),
        Some(expected_cwd.as_str()),
        "cwd from B's chat row"
    );

    // SubscribeTerminal: the stream is proxied item-by-item through the relay.
    let mut stream = client
        .subscribe(
            methods::SUBSCRIBE_TERMINAL,
            serde_json::json!({ "terminalId": terminal_id, "targetDeviceId": "device-b" }),
        )
        .await
        .expect("remote subscribe");
    client
        .call(
            methods::WRITE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "data": BASE64.encode("echo r3lay-$((20+2))\n"),
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote write");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut transcript = Vec::new();
    loop {
        let item = tokio::time::timeout_at(deadline, stream.recv())
            .await
            .expect("proxied terminal output before timeout")
            .expect("stream alive");
        if item["type"] == "data" {
            let bytes = BASE64
                .decode(item["data"].as_str().expect("data"))
                .expect("valid base64");
            transcript.extend(bytes);
        }
        if String::from_utf8_lossy(&transcript).contains("r3lay-22") {
            break;
        }
    }

    client
        .call(
            methods::RESIZE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "cols": 132,
                "rows": 40,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote resize");
    client
        .call(
            methods::CLOSE_TERMINAL,
            serde_json::json!({ "terminalId": terminal_id, "targetDeviceId": "device-b" }),
        )
        .await
        .expect("remote close");
    client
        .call(
            methods::WRITE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "data": BASE64.encode("x"),
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect_err("closed remote terminal rejects writes");

    core_a.shutdown().await;
    core_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_file_surface_proxies_over_the_relay() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let repo_b = dirs.path().join("repo-b");
    init_workspace_repo(&repo_b).await;

    let core_b = assemble(&dirs.path().join("b-files"), "device-b");
    core_b
        .workspace
        .create_space(
            "space-files",
            "device-b",
            &repo_b.to_string_lossy(),
            None,
            true,
        )
        .expect("space on B");
    core_b
        .workspace
        .create_chat("chat-files", Some("space-files"), None, None, None)
        .expect("chat on B");
    let _host = core_b.start_host_relay(&relay_url);

    let core_a = assemble(&dirs.path().join("a-files"), "device-a");
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core_a.rpc_service());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let listing = loop {
        match client
            .call(
                methods::LIST_WORKSPACE_DIRECTORY,
                serde_json::json!({
                    "chatId": "chat-files",
                    "targetDeviceId": "device-b",
                }),
            )
            .await
        {
            Ok(value) => break value,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    assert!(
        listing["entries"]
            .as_array()
            .is_some_and(|entries| { entries.iter().any(|entry| entry["path"] == "src") })
    );

    let matches = client
        .call(
            methods::SEARCH_WORKSPACE_FILES,
            serde_json::json!({
                "chatId": "chat-files",
                "query": "remote",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote search");
    assert_eq!(matches[0]["path"], "src/remote.rs");

    let warm = client
        .call(
            methods::WARM_WORKSPACE_SEARCH,
            serde_json::json!({
                "chatId": "chat-files",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote search warm-up");
    assert!(matches!(warm["state"].as_str(), Some("building" | "ready")));

    let content = client
        .call(
            methods::SEARCH_WORKSPACE_CONTENT,
            serde_json::json!({
                "chatId": "chat-files",
                "query": "REMOTE",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote content search");
    assert!(
        content["matches"]
            .as_array()
            .is_some_and(|matches| matches.iter().any(|m| m["path"] == "src/remote.rs"))
            || content["indexing"] == true
    );

    let read = client
        .call(
            methods::READ_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files",
                "path": "src/remote.rs",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote read");
    assert!(
        read["text"]
            .as_str()
            .is_some_and(|text| text.contains("REMOTE"))
    );
    let hash = read["contentHash"].as_str().expect("content hash");
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>"#;
    std::fs::write(repo_b.join("remote-image.svg"), svg).unwrap();
    let image = client
        .call(
            methods::READ_WORKSPACE_IMAGE,
            serde_json::json!({
                "chatId": "chat-files", "path": "remote-image.svg", "targetDeviceId": "device-b",
                "expectedCheckoutId": read["checkoutId"], "offset": 0,
            }),
        )
        .await
        .expect("remote workspace image");
    assert_eq!(image["mimeType"], "image/svg+xml");
    assert_eq!(image["size"], svg.len());
    assert_eq!(image["done"], true);
    assert!(client.call(methods::READ_WORKSPACE_IMAGE, serde_json::json!({
        "chatId": "chat-files", "path": "remote-image.svg", "targetDeviceId": "device-b",
        "expectedCheckoutId": "wrong-checkout", "offset": 0,
    })).await.is_err());

    // The caller has no workspace for this chat: only the owner can read it.
    assert!(
        client
            .call(
                methods::READ_WORKSPACE_IMAGE,
                serde_json::json!({
                    "chatId": "chat-files", "path": "remote-image.svg",
                    "expectedCheckoutId": read["checkoutId"], "offset": 0,
                })
            )
            .await
            .is_err()
    );

    let large_svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><!--{}--><rect width="20" height="20" fill="red"/></svg>"#,
        " ".repeat(zeron_proto::WORKSPACE_IMAGE_CHUNK_BYTES)
    );
    std::fs::write(repo_b.join("remote-image.svg"), &large_svg).unwrap();
    let image_request = serde_json::json!({
        "chatId": "chat-files", "path": "remote-image.svg", "targetDeviceId": "device-b",
        "expectedCheckoutId": read["checkoutId"], "offset": 0,
    });
    let first = client
        .call(methods::READ_WORKSPACE_IMAGE, image_request.clone())
        .await
        .unwrap();
    assert_eq!(first["checkoutId"], read["checkoutId"]);
    assert_eq!(first["done"], false);
    let mut continuation = image_request;
    continuation["offset"] = first["nextOffset"].clone();
    continuation["expectedContentHash"] = first["contentHash"].clone();
    let last = client
        .call(methods::READ_WORKSPACE_IMAGE, continuation.clone())
        .await
        .unwrap();
    assert_eq!(last["contentHash"], first["contentHash"]);
    assert_eq!(last["done"], true);
    {
        use base64::Engine as _;
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(first["data"].as_str().unwrap())
            .unwrap();
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(last["data"].as_str().unwrap())
                .unwrap(),
        );
        assert_eq!(bytes, large_svg.as_bytes());
    }
    // Same length, different content: a continuation from the old generation
    // must fail on the owning device, even though the routing is still valid.
    std::fs::write(
        repo_b.join("remote-image.svg"),
        large_svg.replace("red", "tan"),
    )
    .unwrap();
    assert!(
        client
            .call(methods::READ_WORKSPACE_IMAGE, continuation.clone())
            .await
            .is_err()
    );
    continuation["expectedCheckoutId"] = "obsolete-checkout".into();
    assert!(
        client
            .call(methods::READ_WORKSPACE_IMAGE, continuation)
            .await
            .is_err()
    );

    let written = client
        .call(
            methods::WRITE_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files",
                "path": "src/remote.rs",
                "text": "pub const REMOTE: bool = false;\n",
                "expectedCheckoutId": read["checkoutId"],
                "expectedContentHash": hash,
                "encoding": "utf8",
                "lineEnding": "lf",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote write");
    assert_eq!(written["status"], "written");
    assert!(
        std::fs::read_to_string(repo_b.join("src/remote.rs"))
            .unwrap()
            .contains("false")
    );

    let mut stream = client
        .subscribe(
            methods::WATCH_WORKSPACE_FILES,
            serde_json::json!({
                "chatId": "chat-files",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote watch");
    let baseline = tokio::time::timeout(Duration::from_secs(5), stream.recv())
        .await
        .expect("baseline timeout")
        .expect("watch alive");
    assert_eq!(baseline["resyncRequired"], true);
    std::fs::write(repo_b.join("remote-created.txt"), "created\n").expect("external write on B");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let item = tokio::time::timeout_at(deadline, stream.recv())
            .await
            .expect("remote watch timeout")
            .expect("remote watch alive");
        if item["changes"].as_array().is_some_and(|changes| {
            changes
                .iter()
                .any(|change| change["path"] == "remote-created.txt")
        }) {
            break;
        }
    }
    drop(stream);

    core_a.shutdown().await;
    core_b.shutdown().await;
}

#[tokio::test]
async fn remote_target_without_links_fails_clearly() {
    let dirs = tempfile::tempdir().expect("tempdir");
    let core = assemble(&dirs.path().join("solo"), "device-solo");
    let client = zeron_rpc::memory_client(core.rpc_service());
    for (method, params) in [
        (
            methods::LIST_PROJECT_ACTIONS,
            serde_json::json!({
                "spaceId": "remote-space",
                "targetDeviceId": "device-elsewhere",
            }),
        ),
        (
            methods::UPSERT_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "remote-space",
                "action": { "name": "Build", "command": "make", "icon": "build" },
                "targetDeviceId": "device-elsewhere",
            }),
        ),
        (
            methods::RUN_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "remote-space",
                "chatId": "remote-chat",
                "actionId": "build",
                "cols": 80,
                "rows": 24,
                "targetDeviceId": "device-elsewhere",
            }),
        ),
    ] {
        let err = client
            .call(method, params)
            .await
            .expect_err("offline Action forward must fail");
        assert!(
            err.to_string().contains("remote routing unavailable"),
            "{method}: {err}"
        );
    }
    let mut offline_stream = client
        .subscribe(
            methods::SUBSCRIBE_TERMINAL,
            serde_json::json!({
                "terminalId": "remote-terminal",
                "targetDeviceId": "device-elsewhere",
            }),
        )
        .await
        .expect("stream request is accepted before the server reports routing failure");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), offline_stream.recv())
            .await
            .expect("offline stream closes promptly")
            .is_none(),
        "offline subscribe must not produce local terminal output"
    );
    let err = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({
                "chatId": "missing-local-chat",
                "targetDeviceId": "device-elsewhere",
            }),
        )
        .await
        .expect_err("workspace call must not fall back locally");
    assert!(
        err.to_string().contains("remote routing unavailable"),
        "got: {err}"
    );
    core.shutdown().await;
}

/// A screenshot alone, sent from another device while the host's turn runs:
/// its bytes go up to the host through the relay, then that device's own
/// engine writes the queue row (`QueueMessage` isn't routed, the row syncs).
/// It used to refuse there, the files already on the host, and the app said
/// "Your message didn't send".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_with_no_words_queued_on_another_device_reach_the_host() {
    const CHAT: &str = "chat-files-alone";
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    let core_a = assemble(&dirs.path().join("a"), "device-a");
    for core in [&core_a, &core_b] {
        core.workspace
            .create_chat(CHAT, None, Some("device-b"), None, None)
            .expect("create chat");
        core.workspace
            .rename_chat(CHAT, "Pre-titled")
            .expect("avoid auto-title run");
    }
    let _host = core_b.start_host_relay(&relay_url);
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core_a.rpc_service());

    // The bytes, up to the host through the relay, as the app's `upload`.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let sent = client
            .call(
                methods::UPLOAD_CHUNK,
                serde_json::json!({
                    "uploadId": "up-1",
                    "seq": 0,
                    "data": BASE64.encode(b"\x89PNG\r\n\x1a\n"),
                    "targetDeviceId": "device-b",
                }),
            )
            .await;
        match sent {
            Ok(_) => break,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    let committed = client
        .call(
            methods::UPLOAD_COMMIT,
            serde_json::json!({
                "uploadId": "up-1",
                "fileName": "shot.png",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("the upload lands on the host");
    let on_host = committed["path"]
        .as_str()
        .expect("its path there")
        .to_owned();

    // Then the row, on this device's engine: the call that said no.
    client
        .call(
            methods::QUEUE_MESSAGE,
            serde_json::json!({
                "chatId": CHAT,
                "text": "",
                "attachments": [on_host.clone()],
                "holdForTurnEnd": true,
            }),
        )
        .await
        .expect("files with no words are queued");

    // The row as it syncs to the host (no edge here: put there by hand).
    let rows = core_a
        .doc_host
        .open(CHAT)
        .expect("A chat")
        .doc()
        .read_queue()
        .expect("A queue");
    assert_eq!(rows.len(), 1, "the row survives being read back");
    let on_b = core_b.doc_host.open(CHAT).expect("B chat");
    on_b.doc().push_queued(&rows[0]).expect("row reaches B");

    // Sent from here, or by the host's own drain, which may have it first:
    // what counts is what the host then runs.
    client
        .call(
            methods::SEND_QUEUED_MESSAGE_NOW,
            serde_json::json!({
                "chatId": CHAT,
                "id": rows[0].id,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("send-now reaches B");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let prompt = on_b
            .doc()
            .read_entries()
            .expect("B transcript")
            .into_iter()
            .find(|e| e.role == zeron_doc::MessageRole::User)
            .map(|e| format!("{:?}", e.parts));
        if let Some(prompt) = prompt {
            assert!(
                prompt.contains("See the attached image(s)."),
                "got: {prompt}"
            );
            assert!(prompt.contains(&on_host), "got: {prompt}");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "B never sent it");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    core_a.shutdown().await;
    core_b.shutdown().await;
}

/// A run sent from another device (or another Windows user's engine on the
/// same PC) names its picture by the sender's own uploads path. Its bytes
/// went to the host too, under the same upload id: the host's agent must get
/// the host's copy, never `C:\Users\ADMIN\…` from a chat running as another
/// user (2026-11-07, "the AI can't read the attached images").
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_from_another_device_names_the_hosts_copy_of_its_upload() {
    const CHAT: &str = "chat-foreign-path";
    const THEIRS: &str = r"C:\Users\ADMIN\AppData\Local\Zeron\uploads\2fbd2b3b-image.png";
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    let core_a = assemble(&dirs.path().join("a"), "device-a");
    for core in [&core_a, &core_b] {
        core.workspace
            .create_chat(CHAT, None, Some("device-b"), None, None)
            .expect("create chat");
        core.workspace
            .rename_chat(CHAT, "Pre-titled")
            .expect("avoid auto-title run");
    }
    let _host = core_b.start_host_relay(&relay_url);
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core_a.rpc_service());

    // The bytes reach the host through the relay.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let sent = client
            .call(
                methods::UPLOAD_CHUNK,
                serde_json::json!({
                    "uploadId": "2fbd2b3b-9c1e",
                    "seq": 0,
                    "data": BASE64.encode(b"\x89PNG\r\n\x1a\n"),
                    "targetDeviceId": "device-b",
                }),
            )
            .await;
        match sent {
            Ok(_) => break,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    let committed = client
        .call(
            methods::UPLOAD_COMMIT,
            serde_json::json!({
                "uploadId": "2fbd2b3b-9c1e",
                "fileName": "image.png",
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("the upload lands on the host");
    let on_host = committed["path"]
        .as_str()
        .expect("its path there")
        .to_owned();

    // The run, as the sender wrote it, arrives over the relay.
    let prompt = format!(
        "Can u double check this?\n\nAttached images (local files — open them to view):\n- {THEIRS}"
    );
    let entry = SessionCommandEntry {
        id: "cmd-foreign-1".into(),
        payload: SessionCommandPayload::Run {
            request: RunRequest {
                mcp: None,
                prompt,
                harness: None,
                model: None,
                reasoning: None,
                model_options: serde_json::Map::new(),
                cwd: "~".into(),
                sandbox: SandboxLevel::WorkspaceWrite,
                auto_approve: true,
                access: Default::default(),
                plan: false,
                attachments: vec![THEIRS.into()],
                worktree: None,
                resume: None,
                env: Default::default(),
                extra_mcp: Vec::new(),
            },
            message_id: "m-foreign-1".into(),
        },
        issued_by: "device-a".into(),
        issued_at: chrono::Utc::now().timestamp_millis(),
        based_on: None,
        expires_at: None,
        status: SessionCommandStatus::Pending,
        resolution: None,
    };
    core_b
        .doc_host
        .ingest_relayed_command(CHAT, entry)
        .await
        .expect("B takes the relayed run");

    let on_b = core_b.doc_host.open(CHAT).expect("B chat");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let said = loop {
        let user = on_b
            .doc()
            .read_entries()
            .expect("B transcript")
            .into_iter()
            .find(|e| e.id == "m-foreign-1")
            .map(|e| format!("{:?}", e.parts));
        if let Some(said) = user {
            break said;
        }
        assert!(tokio::time::Instant::now() < deadline, "B never ran it");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        said.contains(&on_host.replace('\\', "\\\\")) || said.contains(&on_host),
        "got: {said}"
    );
    assert!(
        !said.contains("ADMIN"),
        "the sender's path reached the agent: {said}"
    );
    core_a.shutdown().await;
    core_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_watch_and_single_consumption_route_to_the_remote_chat_host() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");

    // Seed B's session doc while the workspace temporarily names A as host.
    // That keeps B's automatic drain from consuming the fixture before the
    // remote client has subscribed to it.
    let core_b = assemble(&dirs.path().join("b"), "device-b");
    core_b
        .workspace
        .create_chat("chat-queue-remote", None, Some("device-a"), None, None)
        .expect("create remote queue chat");
    core_b
        .workspace
        .rename_chat("chat-queue-remote", "Pre-titled")
        .expect("avoid auto-title run");
    let first_id = core_b
        .doc_host
        .queue_message_with_behavior(
            "chat-queue-remote",
            "first from the shared queue",
            Vec::new(),
            true,
        )
        .expect("seed held queue row");
    let _host = core_b.start_host_relay(&relay_url);

    let core_a = assemble(&dirs.path().join("a"), "device-a");
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core_a.set_links(LinkCache::new(link_config));
    let remote_client = zeron_rpc::memory_client(core_a.rpc_service());
    let local_client = zeron_rpc::memory_client(core_b.rpc_service());

    // The opening stream frame is the authoritative whole-list snapshot a
    // remote Desktop/iOS client uses to repair or initialize its queue.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut queue_stream = loop {
        match remote_client
            .subscribe_checked(
                methods::WATCH_QUEUE,
                serde_json::json!({
                    "chatId": "chat-queue-remote",
                    "targetDeviceId": "device-b",
                }),
            )
            .await
        {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "relay never came up: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let initial = tokio::time::timeout(Duration::from_secs(5), queue_stream.recv())
        .await
        .expect("remote queue frame before timeout")
        .expect("remote queue stream alive");
    assert_eq!(initial["items"][0]["id"], first_id);
    assert_eq!(initial["items"][0]["holdForTurnEnd"], true);

    // Re-home the chat to B without touching the session doc, then execute the
    // row explicitly through A. Only B may take it or run the harness.
    core_b
        .workspace
        .set_chat_host("chat-queue-remote", "device-b")
        .expect("re-home chat to B");
    let sent = remote_client
        .call(
            methods::SEND_QUEUED_MESSAGE_NOW,
            serde_json::json!({
                "chatId": "chat-queue-remote",
                "id": first_id,
                "targetDeviceId": "device-b",
            }),
        )
        .await
        .expect("remote send-now reaches B");
    assert_eq!(sent["sent"], true);
    assert!(
        core_b
            .doc_host
            .open("chat-queue-remote")
            .expect("B chat")
            .doc()
            .read_queue()
            .expect("B queue")
            .is_empty()
    );
    assert!(
        core_a
            .doc_host
            .open("chat-queue-remote")
            .expect("A chat")
            .doc()
            .read_entries()
            .expect("A transcript")
            .is_empty(),
        "the forwarding engine must not execute the queued prompt"
    );

    // Two clients racing the same synchronized row still produce exactly one
    // consumer. The loser gets `sent: false` and can reconcile its projection.
    core_b
        .workspace
        .set_chat_host("chat-queue-remote", "device-a")
        .expect("pause B drain");
    let second_id = core_b
        .doc_host
        .queue_message_with_behavior("chat-queue-remote", "consume me once", Vec::new(), true)
        .expect("seed contested row");
    // Let the doc-change drain observe the temporary non-host assignment
    // before flipping the workspace row back. Otherwise the scheduler can
    // process the queue commit only after the flip and legitimately drain it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        core_b
            .doc_host
            .open("chat-queue-remote")
            .expect("B chat")
            .doc()
            .read_queue()
            .expect("contested queue")
            .len(),
        1
    );
    core_b
        .workspace
        .set_chat_host("chat-queue-remote", "device-b")
        .expect("restore B host");
    let remote_params = serde_json::json!({
        "chatId": "chat-queue-remote",
        "id": second_id,
        "targetDeviceId": "device-b",
    });
    let local_params = serde_json::json!({
        "chatId": "chat-queue-remote",
        "id": second_id,
    });
    let (remote_result, local_result) = tokio::join!(
        remote_client.call(methods::SEND_QUEUED_MESSAGE_NOW, remote_params),
        local_client.call(methods::SEND_QUEUED_MESSAGE_NOW, local_params),
    );
    let acknowledgements = [
        remote_result.expect("remote contender")["sent"] == true,
        local_result.expect("local contender")["sent"] == true,
    ];
    assert_eq!(
        acknowledgements.into_iter().filter(|sent| *sent).count(),
        1,
        "the host must atomically take a queue row once"
    );

    core_a.shutdown().await;
    core_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_entry_mutations_are_forwarded_to_the_owning_plain_folder() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().unwrap();
    let host = assemble(&dirs.path().join("host"), "mutation-host");
    let _host_relay = host.start_host_relay(&relay_url);
    let viewer = assemble(&dirs.path().join("viewer"), "mutation-viewer");
    let mut config = LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    config.probe_timeout = Duration::from_secs(5);
    viewer.set_links(LinkCache::new(config));
    let folder = dirs.path().join("plain-folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("original.txt"), "host only").unwrap();
    host.workspace
        .create_space(
            "mutation-space",
            "mutation-host",
            &folder.to_string_lossy(),
            None,
            false,
        )
        .unwrap();
    let client = zeron_rpc::memory_client(viewer.rpc_service());
    let page = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({"spaceId":"mutation-space","targetDeviceId":"mutation-host"}),
        )
        .await
        .unwrap();
    let entry = &page["entries"][0];
    assert!(page["checkoutId"].as_str().unwrap().starts_with("folder-"));
    let moved=client.call(methods::MOVE_WORKSPACE_ENTRY,serde_json::json!({
        "spaceId":"mutation-space","targetDeviceId":"mutation-host","operationId":"remote-move",
        "expectedCheckoutId":page["checkoutId"],"sourcePath":"original.txt","destinationPath":"renamed.txt",
        "expectedSourceRevision":entry["mutationRevision"],"expectedKind":"file"
    })).await.unwrap();
    assert_eq!(moved["status"], "applied");
    assert_eq!(
        std::fs::read_to_string(folder.join("renamed.txt")).unwrap(),
        "host only"
    );
    let deleted=client.call(methods::DELETE_WORKSPACE_ENTRY,serde_json::json!({
        "spaceId":"mutation-space","targetDeviceId":"mutation-host","operationId":"remote-delete",
        "expectedCheckoutId":page["checkoutId"],"path":"renamed.txt",
        "expectedSourceRevision":moved["entry"]["mutationRevision"],"expectedKind":"file","recursive":false
    })).await.unwrap();
    assert_eq!(deleted["status"], "applied");
    assert!(!folder.join("renamed.txt").exists());
    // Made on the host, from the viewer.
    let created=client.call(methods::CREATE_WORKSPACE_ENTRY,serde_json::json!({
        "spaceId":"mutation-space","targetDeviceId":"mutation-host","operationId":"remote-create",
        "expectedCheckoutId":page["checkoutId"],"path":"made.txt","kind":"file"
    })).await.unwrap();
    assert_eq!(created["status"], "applied");
    assert_eq!(std::fs::read(folder.join("made.txt")).unwrap(), b"");
    viewer.shutdown().await;
    host.shutdown().await;
}

/// Full MCP discovery/create/converse flow over two real engines, a shared
/// registry and the same device relay used by the routing tests above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_standalone_session_executes_on_the_selected_device() {
    use serde_json::json;
    use zeron_mcp::{Origin, Tools, Zeron};

    let (relay_url, relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().unwrap();
    let a = assemble(&dirs.path().join("a"), "device-a");
    // Mock is deliberately disabled in production catalogs. Offer a scripted
    // Codex adapter on B so MCP exercises normal availability validation.
    let remote_dir = dirs.path().join("b");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("device-id"), "device-b").unwrap();
    let profile = zeron_engine::EngineProfile::development(&remote_dir, "dev-org", "dev-user");
    let remote_store = zeron_sync::DocsStore::open(profile.store_root()).unwrap();
    let b = EngineCore::assemble_with_profile(
        profile,
        registry_for(HarnessId::Codex),
        HarnessId::Codex,
        None,
    )
    .unwrap();
    let registry = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    a.workspace.connect_registry_url(&registry.url());
    b.workspace.connect_registry_url(&registry.url());
    let host = b.start_host_relay(&relay_url);
    let mut config = LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    config.probe_timeout = Duration::from_secs(5);
    a.set_links(LinkCache::new(config));
    let client = zeron_rpc::memory_client(a.rpc_service());
    let tools = Tools::new(Arc::new(Zeron::with_client(
        client,
        Origin {
            chat_id: Some("coordinator".into()),
            device_id: Some("device-a".into()),
            ..Default::default()
        },
    )));
    a.workspace
        .create_chat("coordinator", None, Some("device-a"), None, None)
        .unwrap();
    let folder = dirs.path().join("remote-project");
    std::fs::create_dir_all(&folder).unwrap();
    b.workspace
        .create_space(
            "remote-project",
            "device-b",
            folder.to_str().unwrap(),
            None,
            false,
        )
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let devices = tools.call("list_devices", json!({})).await.unwrap();
        let projects = tools
            .call("list_projects", json!({"device":"device-b"}))
            .await;
        if devices["devices"].as_array().unwrap().len() == 2
            && projects
                .as_ref()
                .is_ok_and(|p| p["projects"].as_array().unwrap().len() == 1)
            && tools
                .call("list_harnesses", json!({"device":"device-b"}))
                .await
                .is_ok()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "discovery did not converge"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let projects = tools
        .call("list_projects", json!({"device":"device-b"}))
        .await
        .unwrap();
    let project = projects["projects"][0]["id"].as_str().unwrap();
    tools
        .call(
            "list_models",
            json!({"device":"device-b", "harness":"codex"}),
        )
        .await
        .unwrap();
    let created = tools
        .call(
            "create_chat",
            json!({
                "kind":"chat", "device":"device-b", "project":project,
                "harness":"codex", "title":"Remote MCP session", "prompt":"execute on B",
                "wait":true, "timeout_secs":30
            }),
        )
        .await
        .unwrap();
    let chat_id = created["chatId"].as_str().unwrap();
    assert_eq!(created["kind"], "chat");
    assert!(created["parentChatId"].is_null());
    assert_eq!(created["deviceId"], "device-b");
    assert_eq!(created["turn"]["outcome"], "completed", "{created}");
    assert_eq!(created["turn"]["replies"][0]["text"], "remote reply");
    assert_eq!(created["turn"]["replies"][0]["deviceId"], "device-b");
    let chat = b.workspace.chat(chat_id).unwrap().unwrap();
    assert_eq!(chat.space_id.as_deref(), Some(project));
    assert!(
        chat.parent_chat_id.is_none(),
        "eligible for Sessions on every client"
    );
    assert!(
        a.sessions.session_status(chat_id).is_none(),
        "A must not execute the prompt"
    );
    assert!(
        b.sessions.session_status(chat_id).is_some(),
        "B actually ran the harness"
    );
    let read = tools
        .call("read_chat", json!({"chat":chat_id}))
        .await
        .unwrap();
    assert!(
        read["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "assistant"
                && m["text"] == "remote reply"
                && m["deviceId"] == "device-b")
    );
    let sent = tools
        .call(
            "send_message",
            json!({
                "chat":chat_id, "text":"a second remote turn", "wait":true, "timeout_secs":30
            }),
        )
        .await
        .unwrap();
    assert_eq!(sent["turn"]["outcome"], "completed", "{sent}");
    assert_eq!(
        sent["turn"]["replies"].as_array().unwrap().len(),
        1,
        "only the new turn"
    );
    assert_eq!(sent["turn"]["replies"][0]["deviceId"], "device-b");
    let waited = tools
        .call("wait_for_turn", json!({"chat":chat_id,"timeout_secs":2}))
        .await
        .unwrap();
    assert_eq!(waited["turn"]["outcome"], "completed");
    let interrupted = tools
        .call("interrupt_chat", json!({"chat":chat_id}))
        .await
        .unwrap();
    let interrupt_id = interrupted["commandId"].as_str().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        // Peer-relay delivery marks the processed-command ledger directly;
        // the command row itself arrives separately through session-doc sync.
        if remote_store.is_processed(interrupt_id).unwrap() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "interrupt must reach B's command ledger"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    a.shutdown().await;
    b.shutdown().await;
    drop(host);
    relay.abort();
}

/// A device answering `UpdateStatus` with one report and then staying quiet, as an
/// engine does between checks.
struct OneStatus(&'static str);

#[async_trait]
impl RpcService for OneStatus {
    async fn handle(
        &self,
        method: &str,
        _params: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, RpcError> {
        if method == methods::UPDATE_STATUS {
            let item = serde_json::json!({ "currentVersion": self.0 });
            return Ok(zeron_rpc::RpcReply::Stream(
                futures::stream::once(async move { item })
                    .chain(futures::stream::pending())
                    .boxed(),
            ));
        }
        zeron_rpc::RpcReply::value(&serde_json::json!({}))
    }
}

/// Updating a device takes it away for a moment. Whoever asked for its status, on
/// another device, goes on hearing about it on the same subscription.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_status_follows_another_device_through_its_restart() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let host = |version: &'static str| {
        HostRelay::spawn(
            HostRelayConfig::new(
                &relay_url,
                "other-device",
                Arc::new(StaticToken("test-user".into())),
            ),
            Arc::new(OneStatus(version)),
            Arc::new(|_| true),
        )
    };
    let before = host("1.0.0");

    let core = assemble(&dirs.path().join("asker"), "asking-device");
    let mut link_config =
        LinkCacheConfig::new(relay_url.clone(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(5);
    core.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core.rpc_service());

    let params = serde_json::json!({ "targetDeviceId": "other-device" });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut updates = loop {
        match client
            .subscribe_checked(methods::UPDATE_STATUS, params.clone())
            .await
        {
            Ok(updates) => break updates,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "never reached it: {error}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    let item = tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("the first report")
        .expect("the subscription");
    assert_eq!(item["currentVersion"], "1.0.0");

    drop(before);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _after = host("2.0.0");

    // The test room doesn't say a host went, as a device dying without a word
    // doesn't: this is the follow asking again.
    let item = tokio::time::timeout(Duration::from_secs(90), updates.recv())
        .await
        .expect("a report after the restart")
        .expect("the same subscription must still be open");
    assert_eq!(item["currentVersion"], "2.0.0");
}

/// Asking a device that isn't there says so plainly, and nothing has started.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updating_a_device_that_cannot_be_reached_says_so_plainly() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let core = assemble(&dirs.path().join("asker"), "asking-device");
    let mut link_config =
        LinkCacheConfig::new(relay_url, Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(2);
    core.set_links(LinkCache::new(link_config));
    let client = zeron_rpc::memory_client(core.rpc_service());
    let error = client
        .call(
            methods::APPLY_UPDATE,
            serde_json::json!({ "targetDeviceId": "not-here" }),
        )
        .await
        .expect_err("nothing can answer");
    assert!(error.to_string().contains("can't be reached"), "{error}");
    assert!(error.to_string().contains("didn't start"), "{error}");
}

// ---------------------------------------------------------------------------
// DeviceStats: another device's machine stats, through its relay
// ---------------------------------------------------------------------------

/// A device on the account, as the workspace doc lists it.
fn device_row(id: &str, version: &str, seen_hours_ago: i64) -> zeron_proto::Device {
    zeron_proto::Device {
        id: id.into(),
        name: format!("{id}-name"),
        platform: "linux".into(),
        last_seen_at: Some(chrono::Utc::now() - chrono::Duration::hours(seen_hours_ago)),
        created_at: None,
        version: Some(version.into()),
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    }
}

/// A peer that answers the link's readiness probe, then either knows
/// nothing of `DeviceStats` (an older Zeron) or never answers it.
struct StatsPeer {
    silent: bool,
}

#[async_trait]
impl RpcService for StatsPeer {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::LIST_HARNESSES => RpcReply::value(&serde_json::json!([])),
            methods::DEVICE_STATS if self.silent => futures::future::pending().await,
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

fn spawn_stats_peer(relay_url: &str, device: &str, silent: bool) -> HostRelay {
    HostRelay::spawn(
        HostRelayConfig::new(relay_url, device, Arc::new(StaticToken("test-user".into()))),
        Arc::new(StatsPeer { silent }),
        Arc::new(|_| true),
    )
}

/// Until the peer's host has joined its room: a forwarded call that it
/// answers (an empty harness catalog).
async fn wait_for_link(client: &zeron_rpc::RpcClient, device: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let asked = client
            .call(methods::LIST_HARNESSES, serde_json::json!({ "targetDeviceId": device }))
            .await;
        if asked.is_ok() {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{device} never joined: {asked:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn asker(dir: &std::path::Path, relay_url: &str) -> EngineCore {
    let core = assemble(dir, "asking-device");
    let mut link_config =
        LinkCacheConfig::new(relay_url.to_string(), Arc::new(StaticToken("test-user".into())));
    link_config.probe_timeout = Duration::from_secs(2);
    core.set_links(LinkCache::new(link_config));
    core
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn device_stats_come_from_the_named_device_through_its_relay() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let a = asker(&dirs.path().join("a"), &relay_url);
    let b = assemble(&dirs.path().join("b"), "device-b");
    let _host = b.start_host_relay(&relay_url);
    a.workspace.upsert_device_row(&device_row("device-b", env!("CARGO_PKG_VERSION"), 0));
    let client = zeron_rpc::memory_client(a.rpc_service());

    // This device's own report, without a target or with its own id.
    let local = client.call(methods::DEVICE_STATS, serde_json::json!({})).await.unwrap();
    assert_eq!(local["zeron"]["engineVersion"], env!("CARGO_PKG_VERSION"));
    let own = client
        .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "asking-device" }))
        .await
        .unwrap();
    assert_eq!(own["zeron"], local["zeron"]);

    // Device B's, once its host is in the room.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let remote = loop {
        match client
            .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "device-b" }))
            .await
        {
            Ok(remote) => break remote,
            Err(error) => {
                assert!(tokio::time::Instant::now() < deadline, "device-b never answered: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    assert_eq!(remote["zeron"]["engineVersion"], env!("CARGO_PKG_VERSION"));
    assert!(remote["storage"]["volumes"].is_array(), "{remote}");
    assert!(remote["storage"]["zeronData"]["totalBytes"].as_u64().is_some_and(|t| t > 0), "{remote}");
    // No folder of B's own makes it out, only mount points.
    assert!(!remote.to_string().contains(&*dirs.path().to_string_lossy()), "{remote}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_stats_refuse_a_device_that_isnt_the_accounts() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let a = asker(&dirs.path().join("a"), &relay_url);
    // Someone else's device may well be in a room; this account doesn't list it.
    let _stranger = spawn_stats_peer(&relay_url, "stranger", false);
    let client = zeron_rpc::memory_client(a.rpc_service());
    let started = std::time::Instant::now();
    let error = client
        .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "stranger" }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("isn't one of this account's devices"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1), "refused without asking");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_stats_of_an_offline_device_say_so_within_the_deadline() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let a = asker(&dirs.path().join("a"), &relay_url);
    // Listed on the account, nothing in its room.
    a.workspace.upsert_device_row(&device_row("sleeper", "0.2.139", 30));
    let client = zeron_rpc::memory_client(a.rpc_service());
    let started = std::time::Instant::now();
    let error = client
        .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "sleeper" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(started.elapsed() < Duration::from_secs(7), "took {:?}", started.elapsed());
    assert!(error.starts_with("sleeper-name"), "{error}");
    assert!(error.contains("offline") && error.contains("last seen") && error.contains("1 day ago"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_stats_of_an_older_zeron_say_it_is_too_old() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let a = asker(&dirs.path().join("a"), &relay_url);
    a.workspace.upsert_device_row(&device_row("old-device", "0.2.100", 0));
    let _peer = spawn_stats_peer(&relay_url, "old-device", false);
    let client = zeron_rpc::memory_client(a.rpc_service());
    wait_for_link(&client, "old-device").await;
    let error = client
        .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "old-device" }))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "old-device-name's Zeron (0.2.100) is too old to report stats. Update Zeron there.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_stats_of_a_device_that_never_answers_stop_at_the_deadline() {
    let (relay_url, _relay) = fake_device_room().await;
    let dirs = tempfile::tempdir().expect("tempdir");
    let a = asker(&dirs.path().join("a"), &relay_url);
    a.workspace.upsert_device_row(&device_row("stuck", "0.2.139", 0));
    let _peer = spawn_stats_peer(&relay_url, "stuck", true);
    let client = zeron_rpc::memory_client(a.rpc_service());
    wait_for_link(&client, "stuck").await;
    let started = std::time::Instant::now();
    let error = client
        .call(methods::DEVICE_STATS, serde_json::json!({ "targetDeviceId": "stuck" }))
        .await
        .unwrap_err()
        .to_string();
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(4900) && took < Duration::from_secs(7), "took {took:?}");
    assert!(error.contains("stuck-name didn't answer within 5 s") && error.contains("last seen"), "{error}");
}
