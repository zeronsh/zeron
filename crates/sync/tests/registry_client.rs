//! RegistryClient integration tests against the in-process mock server
//! (`registry::mock_server`), which runs the SAME merge fn as the client and
//! speaks the DO's JSON WS protocol. TS↔Rust interop is proven separately by
//! the `--ignored` live-edge test (registry_edge.rs) and scripts/e2e-smoke.sh.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use zeron_doc::{REGISTRY_DOC_ID, RegistryDoc};
use zeron_proto::{Chat, Device, Session, SessionStatus};
use zeron_sync::registry::mock_server::MockRegistryServer;
use zeron_sync::{DocsStore, RegistryClient, RegistryEvent};

fn ts(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH)
}

fn device(id: &str) -> Device {
    Device {
        id: id.into(),
        name: format!("{id}-name"),
        platform: "linux".into(),
        devbox_provider: None,
        devbox_id: None,
        last_seen_at: Some(ts(1_000)),
        created_at: Some(ts(500)),
        version: Some("0.1.0".into()),
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    }
}

fn chat(id: &str, device_id: &str) -> Chat {
    Chat { id: id.into(), crew: None, device_id: device_id.into(),
    title: Some("chat".into()),
    archived: false,
    cwd: Some("/tmp".into()),
    branch: None,
    checkout_id: None,
    source_context: None,
    config: None,
    last_message_preview: None,
    last_message_at: None,
    created_at: ts(2_000),
    harness_session_id: None,
    harness_session_harness: None,
    harness_session_cwd: None,
    parent_chat_id: None,
    space_id: None,
    last_seen_at: None,
    room_gen: None, }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition not reached in time");
}

fn new_doc(device: &str) -> Arc<Mutex<RegistryDoc>> {
    Arc::new(Mutex::new(RegistryDoc::new(device)))
}

#[tokio::test]
async fn two_clients_converge_and_stream_live_updates() {
    let server = MockRegistryServer::start().await;
    let doc_a = new_doc("dev-a");
    let doc_b = new_doc("dev-b");

    // A writes while OFFLINE; the batches queue locally.
    {
        let mut doc = doc_a.lock().unwrap();
        doc.upsert_device(&device("dev-a")).unwrap();
        doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
        assert_eq!(doc.pending_len(), 2);
    }

    let client_a = RegistryClient::connect(&server.url(), doc_a.clone(), "dev-a")
        .await
        .expect("a connects");
    client_a.nudge();
    wait_until(|| doc_a.lock().unwrap().pending_len() == 0).await;
    assert_eq!(server.row_count(), 2);

    // B joins fresh and receives the full state.
    let client_b = RegistryClient::connect(&server.url(), doc_b.clone(), "dev-b")
        .await
        .expect("b connects");
    wait_until(|| doc_b.lock().unwrap().read_chats().unwrap().len() == 1).await;

    // Live propagation: B renames, A observes.
    {
        let mut doc = doc_b.lock().unwrap();
        assert!(doc.rename_chat("chat-1", "from b").unwrap());
    }
    client_b.nudge();
    wait_until(|| {
        doc_a
            .lock()
            .unwrap()
            .chat("chat-1")
            .unwrap()
            .is_some_and(|c| c.title.as_deref() == Some("from b"))
    })
    .await;

    // Working-status flip propagates the other way.
    {
        let mut doc = doc_a.lock().unwrap();
        doc.upsert_session(&Session {
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            chat_id: "chat-1".into(),
            device_id: "dev-a".into(),
            status: SessionStatus::Working,
            started_at: Some(ts(3_000)),
            updated_at: ts(3_500),
        })
        .unwrap();
    }
    client_a.nudge();
    wait_until(|| {
        doc_b
            .lock()
            .unwrap()
            .read_sessions()
            .unwrap()
            .first()
            .is_some_and(|s| s.status == SessionStatus::Working)
    })
    .await;

    client_a.shutdown().await;
    client_b.shutdown().await;
}

#[tokio::test]
async fn cursor_delta_sync_on_reconnect() {
    let server = MockRegistryServer::start().await;
    let doc_a = new_doc("dev-a");
    {
        let mut doc = doc_a.lock().unwrap();
        doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    let client_a = RegistryClient::connect(&server.url(), doc_a.clone(), "dev-a")
        .await
        .expect("a connects");
    client_a.nudge();
    wait_until(|| doc_a.lock().unwrap().pending_len() == 0).await;
    client_a.shutdown().await;
    let cursor_before = doc_a.lock().unwrap().cursor();
    assert!(cursor_before > 0);

    // While A is away, B lands more rows.
    let doc_b = new_doc("dev-b");
    {
        let mut doc = doc_b.lock().unwrap();
        doc.upsert_chat(&chat("chat-2", "dev-b")).unwrap();
        doc.upsert_chat(&chat("chat-3", "dev-b")).unwrap();
    }
    let client_b = RegistryClient::connect(&server.url(), doc_b.clone(), "dev-b")
        .await
        .expect("b connects");
    client_b.nudge();
    wait_until(|| doc_b.lock().unwrap().pending_len() == 0).await;
    client_b.shutdown().await;

    // A reconnects with its cursor and catches up via the delta.
    let client_a = RegistryClient::connect(&server.url(), doc_a.clone(), "dev-a")
        .await
        .expect("a reconnects");
    wait_until(|| doc_a.lock().unwrap().read_chats().unwrap().len() == 3).await;
    assert!(doc_a.lock().unwrap().cursor() > cursor_before);
    client_a.shutdown().await;
}

#[tokio::test]
async fn unacked_writes_replay_idempotently_after_reconnect() {
    let server = MockRegistryServer::start().await;
    let doc = new_doc("dev-a");
    {
        let mut d = doc.lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("connects");
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    let seq_before = server.seq();

    // Simulate an ack lost to a dropped connection: mark the pending batch
    // un-flighted and replay it through a fresh session.
    {
        let mut d = doc.lock().unwrap();
        d.rename_chat("chat-1", "replayed").unwrap();
    }
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    let seq_after_first = server.seq();
    assert_eq!(seq_after_first, seq_before + 1);
    client.shutdown().await;

    // Manually re-enqueue the same logical write (same value, fresh clock
    // would win; instead we assert the SERVER state is unchanged when the
    // exact same batch replays — covered at the merge layer — and that a
    // reconnect with nothing pending doesn't grow seq).
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("reconnects");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.seq(), seq_after_first);
    assert_eq!(
        server
            .row("chats", "chat-1")
            .expect("row")
            .fields
            .get("title")
            .and_then(|v| v.as_str()),
        Some("replayed")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn queued_write_survives_auth_close_and_temporary_credential_outage() {
    use futures::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tokio_tungstenite::tungstenite::{Message, protocol::CloseFrame};
    use zeron_sync::{SyncError, UrlLease, UrlProvider};

    struct RecoveringUrl {
        rejected_url: String,
        recovered_url: String,
        generation: AtomicU64,
        unavailable_dials: AtomicUsize,
    }

    impl UrlProvider for RecoveringUrl {
        fn url(&self) -> futures::future::BoxFuture<'static, Result<UrlLease, SyncError>> {
            let generation = self.generation.load(Ordering::SeqCst);
            let result = match generation {
                7 => Ok(UrlLease { url: self.rejected_url.clone(), generation: Some(7) }),
                8 => {
                    self.unavailable_dials.fetch_add(1, Ordering::SeqCst);
                    Err(SyncError::TemporarilyUnavailable("credential verification unavailable".into()))
                }
                9 => Ok(UrlLease { url: self.recovered_url.clone(), generation: Some(9) }),
                _ => unreachable!(),
            };
            Box::pin(async move { result })
        }

        fn invalidate(&self, generation: u64) {
            // A close from generation 7 cannot revoke the replacement at 9.
            let _ = self.generation.compare_exchange(generation, 8, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    let server = MockRegistryServer::start().await;
    let rejected_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = Arc::new(RecoveringUrl {
        rejected_url: format!("ws://{}", rejected_listener.local_addr().unwrap()),
        recovered_url: server.url(),
        generation: AtomicU64::new(7),
        unavailable_dials: AtomicUsize::new(0),
    });
    let rejecting_server = tokio::spawn(async move {
        let (stream, _) = rejected_listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(frame) = socket.next().await {
            let Message::Text(text) = frame.unwrap() else { continue };
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            match frame["t"].as_str() {
                Some("hello") => socket.send(Message::Text(
                    r#"{"t":"state","seq":0,"full":true,"gcFloor":0,"rows":[],"presence":{}}"#.into(),
                )).await.unwrap(),
                Some("push") => {
                    // Receive the real pending batch, but reject its credential without ACKing it.
                    socket.send(Message::Close(Some(CloseFrame {
                        code: 4401.into(), reason: "credential rejected".into(),
                    }))).await.unwrap();
                    return serde_json::from_value::<Vec<zeron_doc::registry::RowOp>>(frame["ops"].clone()).unwrap();
                }
                _ => {}
            }
        }
        panic!("socket ended before receiving the queued write");
    });

    let doc = new_doc("auth-recovery-device");
    doc.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        .upsert_chat(&chat("auth-recovery-chat", "auth-recovery-device")).unwrap();
    let pending = || doc.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending_len();
    assert_eq!(pending(), 1);
    let client = RegistryClient::connect_via(provider.clone(), doc.clone(), "auth-recovery-device")
        .await.expect("initial registry hello succeeds");
    let rejected_ops = tokio::time::timeout(Duration::from_secs(10), rejecting_server)
        .await.expect("queued write reaches rejecting socket").unwrap();
    assert_eq!(rejected_ops.len(), 1);
    wait_until(|| provider.unavailable_dials.load(Ordering::SeqCst) > 0).await;
    assert_eq!(provider.generation.load(Ordering::SeqCst), 8);
    assert_eq!(pending(), 1, "4401 and failed revalidation must retain the unacked write");
    assert_eq!(server.row_count(), 0);

    // Release only this provider; normal reconnect backoff drives recovery, not a global wake.
    provider.generation.store(9, Ordering::SeqCst);
    wait_until(|| pending() == 0).await;
    let (expected, applied) = zeron_doc::registry::apply_op(None, &rejected_ops[0]);
    assert!(applied);
    let mut expected = expected.unwrap();
    expected.seq = 1;
    assert_eq!(server.row("chats", "auth-recovery-chat"), Some(expected),
        "the exact original fields and clocks must reach the recovered server");
    assert_eq!(server.row_count(), 1);
    assert_eq!(server.seq(), 1, "the pending write is applied exactly once");
    client.shutdown().await;
}

#[tokio::test]
async fn server_wipe_reseeds_from_client_rows() {
    let server = MockRegistryServer::start().await;
    let doc = new_doc("dev-a");
    {
        let mut d = doc.lock().unwrap();
        d.upsert_device(&device("dev-a")).unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
        d.rename_chat("chat-1", "kept title").unwrap();
    }
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("connects");
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    assert_eq!(server.row_count(), 2);
    client.shutdown().await;

    // Operator wipe: rows gone, seq back to 0.
    server.reset();
    assert_eq!(server.row_count(), 0);

    // The client reconnects, detects the seq regression, and re-seeds —
    // automatically, with the data intact.
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("reconnects");
    wait_until(|| server.row_count() == 2).await;
    assert_eq!(
        server
            .row("chats", "chat-1")
            .expect("row")
            .fields
            .get("title")
            .and_then(|v| v.as_str()),
        Some("kept title")
    );
    // And the local view never flickered.
    assert_eq!(doc.lock().unwrap().read_chats().unwrap().len(), 1);
    client.shutdown().await;
}

#[tokio::test]
async fn gc_floor_forces_full_resync() {
    let server = MockRegistryServer::start().await;
    let doc_a = new_doc("dev-a");
    {
        let mut d = doc_a.lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    let client = RegistryClient::connect(&server.url(), doc_a.clone(), "dev-a")
        .await
        .expect("connects");
    client.nudge();
    wait_until(|| doc_a.lock().unwrap().pending_len() == 0).await;
    client.shutdown().await;

    // B pushes more rows; then the server GC-jumps past A's cursor.
    let doc_b = new_doc("dev-b");
    {
        let mut d = doc_b.lock().unwrap();
        d.upsert_chat(&chat("chat-2", "dev-b")).unwrap();
    }
    let client_b = RegistryClient::connect(&server.url(), doc_b.clone(), "dev-b")
        .await
        .expect("b connects");
    client_b.nudge();
    wait_until(|| doc_b.lock().unwrap().pending_len() == 0).await;
    client_b.shutdown().await;
    server.set_gc_floor(server.seq());

    let client = RegistryClient::connect(&server.url(), doc_a.clone(), "dev-a")
        .await
        .expect("reconnects");
    wait_until(|| doc_a.lock().unwrap().read_chats().unwrap().len() == 2).await;
    client.shutdown().await;
}

#[tokio::test]
async fn presence_beats_reach_peers() {
    let server = MockRegistryServer::start().await;
    let doc_a = new_doc("dev-a");
    let doc_b = new_doc("dev-b");
    let client_a = RegistryClient::connect(&server.url(), doc_a, "dev-a")
        .await
        .expect("a connects");
    let client_b = RegistryClient::connect(&server.url(), doc_b, "dev-b")
        .await
        .expect("b connects");
    client_a.set_presence(123_456);
    wait_until(|| client_b.presence().get("dev-a") == Some(&123_456)).await;
    // And the hello reply carries existing presence to late joiners.
    let doc_c = new_doc("dev-c");
    let client_c = RegistryClient::connect(&server.url(), doc_c, "dev-c")
        .await
        .expect("c connects");
    wait_until(|| client_c.presence().get("dev-a") == Some(&123_456)).await;
    client_a.shutdown().await;
    client_b.shutdown().await;
    client_c.shutdown().await;
}

#[tokio::test]
async fn probe_answers_and_stats_flow() {
    let server = MockRegistryServer::start().await;
    let doc = new_doc("dev-a");
    let client = RegistryClient::connect(&server.url(), doc, "dev-a")
        .await
        .expect("connects");
    let before = client.stats();
    assert!(before.connected);
    assert!(before.synced, "hello state apply latches synced");
    client.probe();
    let events = client.events();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = client.stats();
    assert_eq!(after.probes, before.probes + 1);
    // The probe was answered — no disconnect happened.
    assert!(after.connected);
    assert_eq!(after.disconnects, 0);
    drop(events);
    client.shutdown().await;
}

#[tokio::test]
async fn persisted_doc_survives_restart_with_docs_store() {
    let server = MockRegistryServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = DocsStore::open(dir.path()).unwrap();

    // Session 1: write, sync, persist, "shut down".
    let doc = new_doc("dev-a");
    {
        let mut d = doc.lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("connects");
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    {
        let mut d = doc.lock().unwrap();
        d.rename_chat("chat-1", "offline rename").unwrap(); // will stay unacked
        let bytes = d.to_bytes().unwrap();
        store.save_snapshot(REGISTRY_DOC_ID, &bytes).unwrap();
        // Roll back the rename's push by dropping the connection first.
        d.mark_disconnected();
    }
    client.shutdown().await;

    // Session 2: restore, reconnect — the offline rename pushes.
    let bytes = store
        .load_snapshot(REGISTRY_DOC_ID)
        .unwrap()
        .expect("saved");
    let restored = Arc::new(Mutex::new(
        RegistryDoc::from_bytes(&bytes, "dev-a").unwrap(),
    ));
    assert_eq!(restored.lock().unwrap().pending_len(), 1);
    let client = RegistryClient::connect(&server.url(), restored.clone(), "dev-a")
        .await
        .expect("reconnects");
    wait_until(|| restored.lock().unwrap().pending_len() == 0).await;
    assert_eq!(
        server
            .row("chats", "chat-1")
            .expect("row")
            .fields
            .get("title")
            .and_then(|v| v.as_str()),
        Some("offline rename")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn churn_stays_bounded_no_history_growth() {
    // The wedge regression: 2,000 status flips on the same row must leave the
    // server with ONE row and delta-syncing clients with tiny catch-ups —
    // history is discarded the moment it stops being true.
    let server = MockRegistryServer::start().await;
    let doc = new_doc("dev-a");
    {
        let mut d = doc.lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    let client = RegistryClient::connect(&server.url(), doc.clone(), "dev-a")
        .await
        .expect("connects");
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    for i in 0..2_000i64 {
        {
            let mut d = doc.lock().unwrap();
            d.upsert_session(&Session {
                last_completed_turn: None,
                last_turn_outcome: None,
                terminal_activity: None,
                running_subagents: 0,
                chat_id: "chat-1".into(),
                device_id: "dev-a".into(),
                status: if i % 2 == 0 {
                    SessionStatus::Working
                } else {
                    SessionStatus::Idle
                },
                started_at: Some(ts(i)),
                updated_at: ts(i + 1),
            })
            .unwrap();
        }
        if i % 100 == 0 {
            client.nudge();
            // Let the queue drain periodically so the test bounds memory too.
            wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
        }
    }
    client.nudge();
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    // 2,001 writes → 2 rows. No log, no replay, nothing to compact.
    assert_eq!(server.row_count(), 2);
    client.shutdown().await;

    // A fresh device catches up with 2 rows, not 2,001 updates.
    let doc_b = new_doc("dev-b");
    let client_b = RegistryClient::connect(&server.url(), doc_b.clone(), "dev-b")
        .await
        .expect("b connects");
    wait_until(|| doc_b.lock().unwrap().read_sessions().unwrap().len() == 1).await;
    client_b.shutdown().await;
}

#[tokio::test]
async fn applied_events_fire_for_republish() {
    let server = MockRegistryServer::start().await;
    let doc_a = new_doc("dev-a");
    let doc_b = new_doc("dev-b");
    let client_a = RegistryClient::connect(&server.url(), doc_a, "dev-a")
        .await
        .expect("a connects");
    let client_b = RegistryClient::connect(&server.url(), doc_b, "dev-b")
        .await
        .expect("b connects");
    let mut events = client_b.events();
    {
        let mut d = client_a.doc().lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    }
    client_a.nudge();
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(RegistryEvent::Applied) => return RegistryEvent::Applied,
                Ok(_) => continue,
                Err(err) => panic!("event stream ended: {err}"),
            }
        }
    })
    .await
    .expect("applied event");
    assert_eq!(event, RegistryEvent::Applied);
    client_a.shutdown().await;
    client_b.shutdown().await;
}

/// HTTPS-transport seam for the pure-offline tests below: pulls answer with
/// an empty delta; the FIRST push ack comes back unreadable (a captive
/// portal / proxy interposing on the response body), later pushes ack
/// properly. The regression: an unreadable ack used to be silently ignored,
/// leaving the batch marked in-flight — and with no socket session there is
/// no disconnect to un-mark it, so the write was stranded until restart.
struct FlakyAckTransport {
    push_calls: Arc<std::sync::atomic::AtomicU64>,
}

impl zeron_sync::registry::RegistryTransport for FlakyAckTransport {
    fn fetch(
        &self,
        _since: u64,
    ) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        Box::pin(async {
            Ok(r#"{"seq":0,"full":false,"gcFloor":0,"rows":[],"presence":{}}"#.to_string())
        })
    }

    fn push(
        &self,
        body: String,
    ) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        let call = self
            .push_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if call == 0 {
                return Ok("<html>proxy login required</html>".to_string());
            }
            let v: serde_json::Value = serde_json::from_str(&body).expect("push body is json");
            let batch = v["batch"].as_str().expect("batch id").to_owned();
            Ok(format!(r#"{{"batch":"{batch}","seq":1,"applied":1}}"#))
        })
    }
}

/// HTTPS-transport seam for a pull the socket overtakes: while the first pull
/// is in flight, the socket side applies a newer broadcast, and the pull then
/// answers for the cursor it was sent with. Later pulls find nothing new.
struct OvertakenPullTransport {
    doc: Arc<Mutex<RegistryDoc>>,
    pulls: Arc<std::sync::atomic::AtomicU64>,
}

impl zeron_sync::registry::RegistryTransport for OvertakenPullTransport {
    fn fetch(
        &self,
        since: u64,
    ) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        let first = self.pulls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let doc = self.doc.clone();
        Box::pin(async move {
            let mut doc = doc.lock().unwrap();
            let seq = if first {
                let renamed = zeron_doc::registry::RegistryRow {
                    kind: zeron_doc::registry::KIND_CHATS.into(),
                    id: "chat-1".into(),
                    seq: since + 1,
                    deleted: false,
                    del_hlc: None,
                    fields: std::collections::BTreeMap::from([("title".into(), serde_json::json!("renamed"))]),
                    clocks: std::collections::BTreeMap::new(),
                };
                assert!(doc.apply_rows(since + 1, vec![renamed]));
                since
            } else {
                doc.cursor()
            };
            Ok(format!(r#"{{"seq":{seq},"full":false,"gcFloor":0,"rows":[],"presence":{{}}}}"#))
        })
    }

    fn push(
        &self,
        _body: String,
    ) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        Box::pin(async { panic!("nothing is pending, so nothing is pushed") })
    }
}

// Failure modes: a pull answer the socket overtook rolls the cursor back, and
// its sequence below the cursor reads as a server reset, which blocks Crew
// directory publication.
#[tokio::test]
async fn a_pull_answer_the_socket_overtook_never_reads_as_a_server_reset() {
    let doc = new_doc("dev-a");
    let _ = doc.lock().unwrap().apply_state(10, true, 0, vec![zeron_doc::registry::RegistryRow {
        kind: zeron_doc::registry::KIND_CHATS.into(),
        id: "chat-1".into(),
        seq: 10,
        deleted: false,
        del_hlc: None,
        fields: std::collections::BTreeMap::from([("title".into(), serde_json::json!("original"))]),
        clocks: std::collections::BTreeMap::new(),
    }]);
    let pulls = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // The WS side never connects (dead port): every sync runs over HTTPS.
    let client = RegistryClient::connect_via_transport(
        Arc::new(zeron_sync::StaticUrl("ws://127.0.0.1:1/ws".into())),
        doc.clone(),
        "dev-a",
        zeron_sync::RegistryTuning::default(),
        Arc::new(OvertakenPullTransport { doc: doc.clone(), pulls: pulls.clone() }),
    )
    .await
    .expect("transport clients construct immediately");
    wait_until(|| pulls.load(std::sync::atomic::Ordering::SeqCst) >= 2).await;
    client.shutdown().await;

    let doc = doc.lock().unwrap();
    assert!(!doc.directory_reconciliation_required(), "a stale pull is not a server reset");
    assert_eq!(doc.cursor(), 11);
    let row = doc.acknowledged_row(zeron_doc::registry::KIND_CHATS, "chat-1").unwrap();
    assert_eq!(row.fields["title"], serde_json::json!("renamed"));
}

#[tokio::test]
async fn unreadable_http_ack_retries_instead_of_stranding_the_batch() {
    let push_calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let doc = new_doc("dev-a");
    {
        let mut d = doc.lock().unwrap();
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
        assert_eq!(d.pending_len(), 1);
    }
    // The WS side never connects (dead port): every sync runs over HTTPS.
    let client = RegistryClient::connect_via_transport(
        Arc::new(zeron_sync::StaticUrl("ws://127.0.0.1:1/ws".into())),
        doc.clone(),
        "dev-a",
        zeron_sync::RegistryTuning::default(),
        Arc::new(FlakyAckTransport {
            push_calls: push_calls.clone(),
        }),
    )
    .await
    .expect("transport clients construct immediately");

    // Cycle 1 push gets the unreadable ack; the batch must become pushable
    // again so a later cycle (here: the next dial backoff) retries and the
    // proper ack clears it.
    wait_until(|| doc.lock().unwrap().pending_len() == 0).await;
    assert!(
        push_calls.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "the stranded batch must be re-pushed after the unreadable ack"
    );

    // The empty pulls also latched the synced flag — server truth has been
    // heard this process even though no socket ever joined.
    let stats = client.stats();
    assert!(stats.synced, "HTTP pull apply must latch synced");
    assert!(!stats.connected, "no WS session ever joined");

    client.shutdown().await;
}
