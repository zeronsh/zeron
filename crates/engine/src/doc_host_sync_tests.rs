//! Exercise the dispatcher against slow but live loopback transports.
use super::*;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_sync::chat_frames::{decode, encode, frame_type};

#[derive(Clone, Copy)]
enum Hold {
    Checkpoint,
    Backfill,
    /// The room accepts the socket but answers HELLO only on release: a
    /// slow link's opening join.
    Hello,
}

struct Relay {
    url: String,
    release: watch::Sender<bool>,
    joins: Arc<AtomicUsize>,
    joins_by_chat: Arc<Mutex<HashMap<String, usize>>>,
    rows_requests: Arc<AtomicUsize>,
    received: Arc<Mutex<HashMap<String, loro::LoroDoc>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn relay(hold: Hold) -> Relay {
    let remote = loro::LoroDoc::new();
    remote
        .get_text("slow-checkpoint")
        .insert(0, "received")
        .unwrap();
    remote.commit();
    let checkpoint = remote.export(loro::ExportMode::Snapshot).unwrap();
    let frontier = remote.oplog_vv().encode();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (release, gate) = watch::channel(false);
    let joins = Arc::new(AtomicUsize::new(0));
    let rows_requests = Arc::new(AtomicUsize::new(0));
    let joined = joins.clone();
    let joins_by_chat = Arc::new(Mutex::new(HashMap::new()));
    let joined_by_chat = joins_by_chat.clone();
    let requested = rows_requests.clone();
    let received = Arc::new(Mutex::new(HashMap::<String, loro::LoroDoc>::new()));
    let received_by_peer = received.clone();
    let task = tokio::spawn(async move {
        let mut peers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let checkpoint = checkpoint.clone();
                    let frontier = frontier.clone();
                    let joined = joined.clone();
                    let joined_by_chat = joined_by_chat.clone();
                    let requested = requested.clone();
                    let received = received_by_peer.clone();
                    let mut gate = gate.clone();
                    peers.spawn(async move {
                        // Inspect without consuming the websocket upgrade.
                        let mut buffer = [0; 8192];
                        let (headers, length) = loop {
                            let n = stream.peek(&mut buffer).await.unwrap_or(0);
                            if n == 0 { return }
                            if let Some(end) = buffer[..n].windows(4).position(|v| v == b"\r\n\r\n") {
                                break (String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase(), end + 4);
                            }
                            assert!(n < buffer.len(), "oversized test request");
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        };
                        if !headers.contains("upgrade: websocket") {
                            stream.read_exact(&mut buffer[..length]).await.unwrap();
                            if headers.lines().next().unwrap().contains("/checkpoint") {
                                if matches!(hold, Hold::Checkpoint) {
                                    let _ = gate.wait_for(|released| *released).await;
                                }
                                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", checkpoint.len());
                                if stream.write_all(header.as_bytes()).await.is_ok() {
                                    let _ = stream.write_all(&checkpoint).await;
                                }
                            } else {
                                // Prevent HTTP fallback from masking a stalled WS backfill.
                                let _ = stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            }
                            return;
                        }
                        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                        joined.fetch_add(1, Ordering::SeqCst);
                        let chat = headers.lines().next().unwrap().split('/').nth(2).unwrap().to_owned();
                        *lock(&joined_by_chat).entry(chat.clone()).or_default() += 1;
                        let mut sequence = 0u64;
                        while let Some(Ok(message)) = ws.next().await {
                            if let tokio_tungstenite::tungstenite::Message::Text(text) = &message {
                                if text == "ping" { let _ = ws.send("pong".into()).await; }
                                continue;
                            }
                            let Some(frame) = decode(&message.into_data()) else { continue };
                            if frame.kind == frame_type::HELLO && matches!(hold, Hold::Hello) {
                                let _ = gate.wait_for(|released| *released).await;
                            }
                            let reply = match frame.kind {
                                frame_type::HELLO => encode(frame_type::STATE, &serde_json::json!({
                                    "headSeq": 0, "seqFloor": 0, "checkpointSeq": 0,
                                    "checkpointSize": if matches!(hold, Hold::Checkpoint) {checkpoint.len()} else {0},
                                    "rowCount": 0, "rowBytes": 0,
                                }), &frontier),
                                frame_type::ROWS_REQ => {
                                    requested.fetch_add(1, Ordering::SeqCst);
                                    if matches!(hold, Hold::Backfill) {
                                        let _ = gate.wait_for(|released| *released).await;
                                    }
                                    encode(frame_type::ROWS_DONE, &serde_json::json!({"headSeq": sequence}), &[])
                                }
                                frame_type::PUSH => {
                                    // Observe actual update delivery across rotation, not
                                    // just connection counts or local outbox bookkeeping.
                                    lock(&received).entry(chat.clone()).or_default().import(&frame.payload).unwrap();
                                    sequence += 1;
                                    encode(frame_type::ACK, &serde_json::json!({"batchId": frame.header["batchId"], "seq": sequence, "dup": false}), &[])
                                }
                                frame_type::PROBE => encode(frame_type::PROBE_OK, &serde_json::json!({"headSeq": sequence}), &[]),
                                _ => continue,
                            };
                            if ws.send(reply.into()).await.is_err() { break }
                        }
                    });
                }
                _ = peers.join_next(), if !peers.is_empty() => {}
            }
        }
    });
    Relay {
        url,
        release,
        joins,
        joins_by_chat,
        rows_requests,
        received,
        task,
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dispatcher failed to make progress");
}

fn host(store: Arc<DocsStore>, relay: &Relay) -> DocHost {
    DocHost::new(
        store,
        DocHostConfig {
            device_id: "host".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(&relay.url, "test")),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_checkpoints_survive_idle_grace_with_all_slots_occupied() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Checkpoint).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    for i in 0..ACTIVE_SYNC_CAP {
        let id = format!("slow-{i:02}");
        // Model already-published chats: no outgoing operations pin the client.
        store.initialize_chat_outbox(&id, &[]).unwrap();
        store.schedule_sync_job(&id, "wake").unwrap();
    }
    let host = host(store.clone(), &relay);
    until(|| relay.rows_requests.load(Ordering::SeqCst) == ACTIVE_SYNC_CAP).await;
    assert_eq!(
        store.sync_work_counts().unwrap(),
        (0, ACTIVE_SYNC_CAP as u64)
    );
    // Real time: every checkpoint remains incomplete beyond the 10 s grace.
    tokio::time::sleep(Duration::from_millis(SYNC_IDLE_MS as u64 + 500)).await;
    assert_eq!(
        relay.joins.load(Ordering::SeqCst),
        ACTIVE_SYNC_CAP,
        "idle retirement restarted incomplete checkpoint downloads"
    );
    assert_eq!(
        store.sync_work_counts().unwrap(),
        (0, ACTIVE_SYNC_CAP as u64)
    );
    relay.release.send_replace(true);
    until(|| store.sync_work_counts().unwrap() == (0, 0)).await;
    for i in 0..ACTIVE_SYNC_CAP {
        let bytes = store
            .load_snapshot(&format!("slow-{i:02}"))
            .unwrap()
            .unwrap();
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        assert_eq!(doc.get_text("slow-checkpoint").to_string(), "received");
    }
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_wake_during_stalled_backfill_does_not_block_other_admissions() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.initialize_chat_outbox("stalled", &[]).unwrap();
    let host = host(store, &relay);
    let stalled = host.open("stalled").unwrap();
    until(|| relay.rows_requests.load(Ordering::SeqCst) == 1).await;
    host.enqueue_wakeup("stalled").unwrap();
    // A wake stays on the original client while its first read is stalled.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let other = host.open("unrelated").unwrap();
    until(|| lock(&other.chat2).is_some()).await;
    assert!(other.sync_started.load(Ordering::Acquire));
    assert!(!lock(&stalled.chat2).as_ref().is_some_and(|c| c.caught_up()));
    assert_eq!(lock(&relay.joins_by_chat).get("stalled"), Some(&1));
    assert!(
        host.inner
            .store
            .sync_job_version("stalled", "wake")
            .unwrap()
            .is_some()
    );
    tokio::time::timeout(Duration::from_secs(2), host.shutdown_workers())
        .await
        .expect("host shutdown waited on a peer holding ROWS_DONE");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_connection_closes_while_caller_keeps_document() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Checkpoint).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let host = host(Arc::new(DocsStore::open(dir.path()).unwrap()), &relay);
    let handle = host.open("idle").unwrap();
    until(|| lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    until(|| !host.inner.store.has_pending_chat_updates("idle").unwrap()).await;
    handle
        .last_access
        .store(now_ms() - SYNC_IDLE_MS - 1, Ordering::Release);
    until(|| !handle.sync_started.load(Ordering::Acquire)).await;
    assert!(lock(&handle.chat2).is_none());
    assert!(lock(&host.inner.handles).contains_key("idle"));
    let writer = handle.writer();
    assert_eq!(handle.writers.load(Ordering::Acquire), 1);
    drop(writer);
    assert_eq!(handle.writers.load(Ordering::Acquire), 0);
    host.shutdown_workers().await;
}

/// What a remote chat's composer and transcript read while it opens on a
/// slow link: its room is Connecting and, past the engine's grace, not
/// connected — never delivery-live — until the room answers. The UI calls
/// that syncing, and an outage only once the dial has stalled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_hello_reads_as_connecting_until_the_room_answers() {
    use zeron_proto::ChatSyncState as S;
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Hello).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.initialize_chat_outbox("slow", &[]).unwrap();
    let host = host(store, &relay);
    let _handle = host.open("slow").unwrap();
    let chat = |host: &DocHost| {
        let chat = host
            .compute_connectivity()
            .chats
            .into_iter()
            .find(|chat| chat.chat_id == "slow")
            .expect("an open chat is reported");
        (chat.sync_state, chat.connected, chat.delivery_live)
    };
    until(|| relay.joins.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        chat(&host),
        (S::Connecting, true, false),
        "inside the grace"
    );
    tokio::time::sleep(DEGRADE_GRACE + Duration::from_millis(500)).await;
    assert_eq!(chat(&host), (S::Connecting, false, false));
    relay.release.send_replace(true);
    until(|| chat(&host).2).await;
    assert_eq!(chat(&host), (S::Synced, true, true));
    host.shutdown_workers().await;
}

/// A room holding `head` remote rows whose checkpoint covers the first
/// `checkpoint_seq`. Records `(connection, after)` for every ROWS_REQ.
struct RowsRelay {
    url: String,
    requests: Arc<Mutex<Vec<(usize, u64)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RowsRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn rows_relay(head: u64, checkpoint_seq: u64) -> RowsRelay {
    let remote = loro::LoroDoc::new();
    let text = remote.get_text("log");
    let mut rows = Vec::new();
    let mut checkpoint = (Vec::new(), Vec::new());
    for seq in 1..=head {
        let before = remote.oplog_vv();
        text.insert(text.len_unicode(), &format!("{seq};")).unwrap();
        remote.commit();
        rows.push(remote.export(loro::ExportMode::updates(&before)).unwrap());
        if seq == checkpoint_seq {
            checkpoint = (
                remote.export(loro::ExportMode::Snapshot).unwrap(),
                remote.oplog_vv().encode(),
            );
        }
    }
    let log = Arc::new(Mutex::new(rows));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let connections = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(async move {
        let mut peers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let (checkpoint, frontier) = checkpoint.clone();
                    let log = log.clone();
                    let recorded = recorded.clone();
                    let connections = connections.clone();
                    peers.spawn(async move {
                        let mut buffer = [0; 8192];
                        let (headers, length) = loop {
                            let n = stream.peek(&mut buffer).await.unwrap_or(0);
                            if n == 0 { return }
                            if let Some(end) = buffer[..n].windows(4).position(|v| v == b"\r\n\r\n") {
                                break (String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase(), end + 4);
                            }
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        };
                        if !headers.contains("upgrade: websocket") {
                            stream.read_exact(&mut buffer[..length]).await.unwrap();
                            let response = if headers.lines().next().unwrap().contains("/checkpoint") {
                                let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", checkpoint.len()).into_bytes();
                                response.extend_from_slice(&checkpoint);
                                response
                            } else {
                                // Rows travel over the socket only, so every read is recorded.
                                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                            };
                            let _ = stream.write_all(&response).await;
                            return;
                        }
                        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                        let connection = connections.fetch_add(1, Ordering::SeqCst);
                        while let Some(Ok(message)) = ws.next().await {
                            if let tokio_tungstenite::tungstenite::Message::Text(text) = &message {
                                if text == "ping" { let _ = ws.send("pong".into()).await; }
                                continue;
                            }
                            let Some(frame) = decode(&message.into_data()) else { continue };
                            let head = lock(&log).len() as u64;
                            let replies = match frame.kind {
                                frame_type::HELLO => vec![encode(frame_type::STATE, &serde_json::json!({
                                    "headSeq": head, "seqFloor": 0, "checkpointSeq": checkpoint_seq,
                                    "checkpointSize": checkpoint.len(), "rowCount": head - checkpoint_seq, "rowBytes": 0,
                                }), &frontier)],
                                frame_type::ROWS_REQ => {
                                    let after = frame.header["after"].as_u64().unwrap();
                                    lock(&recorded).push((connection, after));
                                    let log = lock(&log);
                                    let mut frames: Vec<_> = (after + 1..=head).map(|seq| encode(frame_type::ROW,
                                        &serde_json::json!({"seq": seq, "device": "remote", "batchId": format!("row-{seq}")}),
                                        &log[seq as usize - 1])).collect();
                                    frames.push(encode(frame_type::ROWS_DONE, &serde_json::json!({"headSeq": head}), &[]));
                                    frames
                                }
                                frame_type::PUSH => {
                                    let mut log = lock(&log);
                                    log.push(frame.payload.clone());
                                    vec![encode(frame_type::ACK, &serde_json::json!({"batchId": frame.header["batchId"], "seq": log.len(), "dup": false}), &[])]
                                }
                                frame_type::PROBE => vec![encode(frame_type::PROBE_OK, &serde_json::json!({"headSeq": head}), &[])],
                                _ => continue,
                            };
                            for reply in replies {
                                if ws.send(reply.into()).await.is_err() { return }
                            }
                        }
                    });
                }
                _ = peers.join_next(), if !peers.is_empty() => {}
            }
        }
    });
    RowsRelay {
        url,
        requests,
        task,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejoin_after_idle_retirement_resumes_from_its_cursor() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    // A cold reader's first open stamps an unverified born snapshot, so the
    // first join repairs; every later join must trust the cursor it wrote.
    // A rewind to the checkpoint redownloads rows 3..=5 (on a whale chat,
    // most of its log) on every revisit.
    let relay = rows_relay(5, 2).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.initialize_chat_outbox("remote", &[]).unwrap();
    let host = DocHost::new(
        store.clone(),
        DocHostConfig {
            device_id: "host".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(&relay.url, "test")),
        },
    );
    let first_request = |connection| {
        lock(&relay.requests)
            .iter()
            .find(|(c, _)| *c == connection)
            .map(|(_, after)| *after)
    };
    let handle = host.open("remote").unwrap();
    until(|| lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    until(|| {
        store.snapshot_cursor("remote").unwrap() == 5
            && store.snapshot_cursor_verified("remote").unwrap()
    })
    .await;
    assert_eq!(
        first_request(0),
        Some(2),
        "cold join fetches the checkpoint"
    );
    handle
        .last_access
        .store(now_ms() - SYNC_IDLE_MS - 1, Ordering::Release);
    until(|| !handle.sync_started.load(Ordering::Acquire)).await;

    host.open("remote").unwrap();
    until(|| first_request(1).is_some()).await;
    assert_eq!(
        first_request(1),
        Some(5),
        "rejoin rewound to the checkpoint"
    );
    until(|| lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    assert_eq!(store.snapshot_cursor("remote").unwrap(), 5);
    assert_eq!(handle.doc.doc().get_text("log").to_string(), "1;2;3;4;5;");
    host.shutdown_workers().await;
}

fn workspace(store: Arc<DocsStore>) -> WorkspaceHost {
    WorkspaceHost::open(
        store,
        crate::workspace_host::WorkspaceHostConfig {
            device_id: "host".into(),
            device_name: "Test host".into(),
            platform: "linux".into(),
            org_id: "test-org".into(),
            user_id: "test-user".into(),
            edge: None,
            local_only: false,
        },
    )
    .unwrap()
}

fn legacy_chat(workspace: &WorkspaceHost, id: &str, device: &str, generation: Option<u32>) {
    workspace
        .create_chat(id, None, Some(device), None, None)
        .unwrap();
    let mut row = workspace.chat(id).unwrap().unwrap();
    row.room_gen = generation;
    workspace.import_chat_row(&row).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn twelve_foreign_legacy_wakes_do_not_block_interactive_sync() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Checkpoint).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let workspace = workspace(store.clone());
    for i in 0..ACTIVE_SYNC_CAP {
        let id = format!("legacy-{i:02}");
        legacy_chat(
            &workspace,
            &id,
            "another-host",
            if i % 2 == 0 { None } else { Some(1) },
        );
        store.schedule_sync_job(&id, "wake").unwrap();
    }
    let host = host(store.clone(), &relay);
    host.set_workspace(workspace);
    // Reach the old failure state (12 phantom clients) or successful retirement.
    until(|| {
        store.sync_work_counts().unwrap().1 == 0
            || lock(&host.inner.handles)
                .values()
                .filter(|h| h.sync_started.load(Ordering::Acquire))
                .count()
                == ACTIVE_SYNC_CAP
    })
    .await;
    let interactive = host.open("interactive").unwrap();
    let _view = interactive.watch_messages();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !lock(&interactive.chat2)
            .as_ref()
            .is_some_and(|c| c.caught_up())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("foreign legacy wakes occupied every admission slot");
    until(|| store.sync_work_counts().unwrap().1 == 0).await;
    assert!(
        lock(&host.inner.handles)
            .values()
            .filter(|h| h.chat_id.starts_with("legacy-"))
            .all(|h| !h.sync_started.load(Ordering::Acquire)
                && !h.sync_requested.load(Ordering::Acquire))
    );
    assert!(lock(&host.inner.seed_waiting).is_empty());
    for i in 0..ACTIVE_SYNC_CAP {
        host.enqueue_wakeup(&format!("legacy-{i:02}")).unwrap();
    }
    until(|| store.sync_work_counts().unwrap().1 == 0).await;
    assert_eq!(
        relay.joins.load(Ordering::SeqCst),
        1,
        "rejected wakes started network work"
    );
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admissible_wakes_preserve_missing_rows_modern_readers_and_local_epoch() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Checkpoint).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let workspace = workspace(store.clone());
    workspace
        .create_chat("modern-foreign", None, Some("another-host"), None, None)
        .unwrap();
    legacy_chat(&workspace, "epoch-override", "another-host", Some(1));
    let thin = SessionDoc::init("epoch-override").unwrap();
    store
        .save_snapshot_with_cursor(
            "epoch-override",
            &thin.export_snapshot().unwrap(),
            0,
            crate::chat2_host::CHAT2_DOC_EPOCH,
        )
        .unwrap();
    let ids = ["missing-row", "modern-foreign", "epoch-override"];
    for id in ids {
        store.initialize_chat_outbox(id, &[]).unwrap();
        store.schedule_sync_job(id, "wake").unwrap();
    }
    let host = host(store.clone(), &relay);
    host.set_workspace(workspace.clone());
    until(|| relay.rows_requests.load(Ordering::SeqCst) == ids.len()).await;
    for id in ids {
        let handle = lock(&host.inner.handles).get(id).cloned().unwrap();
        assert_eq!(handle.room_gen, 2);
        assert!(handle.sync_started.load(Ordering::Acquire));
        assert!(
            store.sync_job_version(id, "wake").unwrap().is_some(),
            "retired before catch-up"
        );
    }
    assert!(workspace.chat("missing-row").unwrap().is_none());
    // Creation arrives after its notification has already started joining.
    workspace
        .create_chat("missing-row", None, Some("another-host"), None, None)
        .unwrap();
    relay.release.send_replace(true);
    until(|| store.sync_work_counts().unwrap().1 == 0).await;
    for id in ids {
        let snapshot = store.load_snapshot(id).unwrap().unwrap();
        let doc = loro::LoroDoc::new();
        doc.import(&snapshot).unwrap();
        assert_eq!(doc.get_text("slow-checkpoint").to_string(), "received");
    }
    host.shutdown_workers().await;
}

#[tokio::test]
async fn legacy_admission_rechecks_owner_and_retires_only_the_captured_wake() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let workspace = workspace(store.clone());
    legacy_chat(&workspace, "reassigned", "host", None);
    let host = DocHost::new(
        store.clone(),
        DocHostConfig {
            device_id: "host".into(),
            default_harness: HarnessId::Mock,
            edge: None,
        },
    );
    host.set_workspace(workspace.clone());
    let handle = host.open_local("reassigned").unwrap();
    assert_eq!(
        host.prepare_sync_admission(&handle, None).unwrap(),
        Some(SyncAdmission::Seed)
    );
    handle.sync_requested.store(true, Ordering::Release);
    handle
        .doc
        .doc()
        .get_text("local-edit")
        .insert(0, "keep this edit")
        .unwrap();
    handle.doc.doc().commit();
    let updates = crate::chat2_host::publication_updates(handle.doc.doc()).unwrap();
    store
        .enqueue_chat_update("reassigned", "local-batch", &updates[0])
        .unwrap();
    let outbox = store.pending_chat_updates("reassigned").unwrap();
    host.enqueue_wakeup("reassigned").unwrap();
    let old = store.sync_job_version("reassigned", "wake").unwrap();
    // Ownership changes while this handle is waiting for admission.
    workspace
        .set_chat_host("reassigned", "another-host")
        .unwrap();
    host.enqueue_wakeup("reassigned").unwrap();
    let newer = store.sync_job_version("reassigned", "wake").unwrap();
    assert_ne!(old, newer);
    assert_eq!(host.prepare_sync_admission(&handle, old).unwrap(), None);
    assert_eq!(store.sync_job_version("reassigned", "wake").unwrap(), newer);
    assert!(!handle.sync_requested.load(Ordering::Acquire));
    assert!(!handle.sync_started.load(Ordering::Acquire));
    assert_eq!(host.prepare_sync_admission(&handle, newer).unwrap(), None);
    assert_eq!(store.sync_job_version("reassigned", "wake").unwrap(), None);
    assert_eq!(store.pending_chat_updates("reassigned").unwrap(), outbox);
    assert_eq!(
        handle.doc.doc().get_text("local-edit").to_string(),
        "keep this edit"
    );
    // A cutover on another host can overtake this old cached handle: defer,
    // rather than retire the notification for the now-modern room.
    workspace.set_chat_room_gen("reassigned", 2).unwrap();
    host.enqueue_wakeup("reassigned").unwrap();
    let cutover = store.sync_job_version("reassigned", "wake").unwrap();
    assert_eq!(host.prepare_sync_admission(&handle, cutover).unwrap(), None);
    assert_eq!(
        store.sync_job_version("reassigned", "wake").unwrap(),
        cutover
    );
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forty_eight_chats_drain_without_restarting_views_or_writers() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    const CHATS: usize = 48;
    const PROTECTED: usize = 8;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut views = Vec::new();
    let mut writers = Vec::new();
    for i in 0..CHATS {
        let id = format!("load-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let handle = host.open_local(&id).unwrap();
        if i < PROTECTED / 2 {
            views.push(handle.watch_messages());
        } else if i < PROTECTED {
            writers.push(handle.writer());
        }
        handles.push(handle);
    }
    // Establish eight protected clients first, then contend for the remaining
    // twenty slots with forty chats. All 48 requests are outstanding together.
    for h in &handles[..PROTECTED] {
        host.activate_sync(h);
    }
    until(|| {
        handles[..PROTECTED]
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    let mut peak = 0;
    for round in 0..3 {
        for h in &handles {
            h.sync_last_started
                .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
            h.last_access
                .store(now_ms() - SYNC_IDLE_MS - 1, Ordering::Release);
            host.enqueue_wakeup(&h.chat_id).unwrap();
        }
        if round == 0 {
            for h in &handles[PROTECTED..] {
                host.activate_sync(h);
            }
        }
        // Local writes keep active outboxes busy while the backlog advances.
        for h in &handles[..PROTECTED] {
            h.doc.doc().get_text("load").insert(0, "edit").unwrap();
            h.doc.doc().commit();
        }
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let stats = host.sync_resources();
                let active = stats["retainedBy"]["connections"].as_u64().unwrap();
                peak = peak.max(active);
                assert!(active <= ACTIVE_SYNC_CAP as u64, "{stats}");
                for h in &handles[..PROTECTED] {
                    assert!(
                        h.sync_started.load(Ordering::Acquire),
                        "protected client retired"
                    );
                    assert_eq!(lock(&relay.joins_by_chat).get(&h.chat_id), Some(&1));
                }
                if store.sync_work_counts().unwrap() == (0, 0) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("48-chat backlog did not drain");
    }
    assert_eq!(lock(&relay.joins_by_chat).len(), CHATS);
    assert!(
        peak > PROTECTED as u64,
        "background never competed for capacity"
    );
    eprintln!(
        "48 chats, 3 wake waves, peak clients={peak}, protected reconnects=0, durable backlog=0"
    );
    drop((views, writers));
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_outboxes_do_not_rotate_clients_even_at_capacity() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    for i in 0..ACTIVE_SYNC_CAP {
        let id = format!("active-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        handles.push(host.open(&id).unwrap());
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    for h in &handles {
        // Simulate durable publication awaiting delivery. Keep it in SQLite
        // without notifying the client so the regression cannot miss a short
        // outbox window; no payload is applied to the document.
        store
            .enqueue_chat_update(&h.chat_id, "pending", b"held")
            .unwrap();
        h.sync_last_started
            .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP);
    assert!(
        handles
            .iter()
            .all(|h| h.sync_started.load(Ordering::Acquire))
    );
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spare_capacity_admits_waiters_without_rotating_aged_clients() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut writers = Vec::new();
    for i in 0..20 {
        let id = format!("spare-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open(&id).unwrap();
        writers.push(h.writer());
        handles.push(h);
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    for h in &handles {
        h.sync_last_started
            .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
    }
    host.focus_chat("extra").unwrap();
    let extra = host.open("extra").unwrap();
    let _writer = extra.writer();
    until(|| lock(&extra.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for h in &handles {
        assert_eq!(lock(&relay.joins_by_chat).get(&h.chat_id), Some(&1));
    }
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contention_retires_only_one_unprotected_client_for_one_waiter() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    for i in 0..ACTIVE_SYNC_CAP {
        let id = format!("rotate-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        handles.push(host.open(&id).unwrap());
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    // A repeated local open recently touched each otherwise quiet client.
    for h in &handles {
        h.sync_last_started
            .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
        h.touch();
    }
    let extra = host.open("extra").unwrap();
    let _view = extra.watch_messages();
    until(|| lock(&extra.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    let stopped = handles
        .iter()
        .filter(|h| !h.sync_started.load(Ordering::Acquire))
        .count();
    assert_eq!(stopped, 1, "retired more clients than needed");
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forty_eight_chats_preserve_fresh_views_then_drain_when_one_view_closes() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut views = Vec::new();
    for i in 0..ACTIVE_SYNC_CAP {
        let id = format!("view-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open(&id).unwrap();
        views.push(h.watch_messages());
        handles.push(h);
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    // Fresh clients are still inside the reuse grace. Once a view closes,
    // contention must reclaim its quiet slot without making newcomers wait.
    for i in 0..(48 - ACTIVE_SYNC_CAP) {
        let id = format!("background-{i:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        host.enqueue_wakeup(&id).unwrap();
    }
    // All slots are protected: keep their connections, retaining other work
    // durably instead of rotating a visible conversation to admit it.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP);
    assert_eq!(
        store.sync_work_counts().unwrap(),
        (0, (48 - ACTIVE_SYNC_CAP) as u64)
    );
    drop(views.pop());
    tokio::time::timeout(Duration::from_secs(20), async {
        while store.sync_work_counts().unwrap() != (0, 0) {
            assert!(
                host.sync_resources()["retainedBy"]["connections"]
                    .as_u64()
                    .unwrap()
                    <= ACTIVE_SYNC_CAP as u64
            );
            for h in &handles[..ACTIVE_SYNC_CAP - 1] {
                assert!(h.sync_started.load(Ordering::Acquire));
                assert_eq!(lock(&relay.joins_by_chat).get(&h.chat_id), Some(&1));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background work did not resume when a view closed");
    assert_eq!(lock(&relay.joins_by_chat).len(), 48);
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saturated_protected_clients_do_not_load_cold_backlog() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut views = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("view-{n}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        views.push(host.open(&id).unwrap().watch_messages());
    }
    until(|| relay.joins.load(Ordering::SeqCst) == ACTIVE_SYNC_CAP).await;
    let loads = host.inner.document_loads.load(Ordering::Relaxed);
    for n in 0..128 {
        let id = format!("cold-{n:03}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        host.enqueue_wakeup(&id).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(host.inner.document_loads.load(Ordering::Relaxed), loads);
    assert_eq!(lock(&host.inner.handles).len(), ACTIVE_SYNC_CAP);
    assert_eq!(store.sync_work_counts().unwrap(), (0, 128));
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_view_displaces_one_older_view_without_reconnect_ping_pong() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut views = Vec::new();
    let mut writers = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("view-{n:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open(&id).unwrap();
        views.push(h.watch_messages());
        if n == 0 {
            writers.push(h.writer());
        }
        handles.push(h);
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    // Make the writer the oldest candidate: it must still never be retired.
    for (n, h) in handles.iter().enumerate() {
        h.last_access
            .store(now_ms() - 2000 + n as i64, Ordering::Relaxed);
    }
    for n in 0..32 {
        host.enqueue_wakeup(&format!("cold-{n}")).unwrap();
    }
    host.focus_chat("new-view").unwrap();
    let fresh = host.open("new-view").unwrap();
    let _fresh_view = fresh.watch_messages();
    until(|| lock(&fresh.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP + 1);
    assert_eq!(
        handles
            .iter()
            .filter(|h| !h.sync_started.load(Ordering::Acquire))
            .count(),
        1
    );
    assert!(handles[0].sync_started.load(Ordering::Acquire));
    assert!(!handles[1].sync_started.load(Ordering::Acquire));
    // Refocusing the displaced view makes it an explicit interactive request.
    host.focus_chat(&handles[1].chat_id).unwrap();
    until(|| handles[1].sync_started.load(Ordering::Acquire)).await;
    assert!(handles[0].sync_started.load(Ordering::Acquire));
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focused_waiter_displaces_one_running_writer_without_stopping_local_writes() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut writers = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("writer-{n}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        writers.push(host.open(&id).unwrap().writer());
        host.focus_chat(&id).unwrap();
    }
    until(|| relay.joins.load(Ordering::SeqCst) == ACTIVE_SYNC_CAP).await;
    // Refocusing the oldest-created thread makes writer-1 the least
    // recently focused. A fresh internal read of writer-1 must not save it.
    host.focus_chat("writer-0").unwrap();
    host.open("writer-1").unwrap();
    host.focus_chat("new-view").unwrap();
    let fresh = host.open("new-view").unwrap();
    let _view = fresh.watch_messages();

    until(|| lock(&fresh.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP + 1);
    let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
    let displaced: Vec<_> = handles
        .iter()
        .filter(|h| {
            h.writers.load(Ordering::Acquire) > 0 && !h.sync_started.load(Ordering::Acquire)
        })
        .collect();
    assert_eq!(displaced.len(), 1);
    let h = displaced[0];
    assert_eq!(h.chat_id, "writer-1");
    h.doc
        .doc()
        .get_text("while-queued")
        .insert(0, "still running")
        .unwrap();
    h.doc.doc().commit();
    until(|| store.has_pending_chat_updates(&h.chat_id).unwrap()).await;
    assert!(h.sync_requested.load(Ordering::Acquire));
    // An incidental read/watch must not reclaim the slot or invent user focus.
    let old_focus = h.last_focus.load(Ordering::Acquire);
    let reread = host.open(&h.chat_id).unwrap();
    let _read = reread.watch_messages();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!h.sync_started.load(Ordering::Acquire));
    assert_eq!(h.last_focus.load(Ordering::Acquire), old_focus);
    host.focus_chat(&h.chat_id).unwrap();
    until(|| {
        lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up())
            && !store.has_pending_chat_updates(&h.chat_id).unwrap()
    })
    .await;
    assert_eq!(
        h.doc.doc().get_text("while-queued").to_string(),
        "still running"
    );
    assert_eq!(writers.len(), ACTIVE_SYNC_CAP);
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_handoff_prefers_an_unprotected_slot_over_a_view() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    // Put the next admission after the 28 residents on the normal
    // background-fairness turn. The freed slot must still serve the handoff.
    for n in 0..3 {
        host.enqueue_wakeup(&format!("phase-{n}")).unwrap();
    }
    until(|| relay.joins.load(Ordering::SeqCst) == 3).await;
    until(|| host.sync_resources()["retainedBy"]["connections"] == 0).await;
    let mut handles = Vec::new();
    let mut views = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("existing-{n:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open(&id).unwrap();
        if n > 0 {
            views.push(h.watch_messages());
        }
        handles.push(h);
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    for h in &handles {
        h.last_access.store(now_ms() - 1000, Ordering::Relaxed);
    }
    for n in 0..32 {
        host.enqueue_wakeup(&format!("cold-{n}")).unwrap();
    }
    host.focus_chat("new-view").unwrap();
    let fresh = host.open("new-view").unwrap();
    let _view = fresh.watch_messages();
    until(|| lock(&fresh.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for h in &handles[1..] {
        assert!(h.sync_started.load(Ordering::Acquire));
        assert_eq!(lock(&relay.joins_by_chat).get(h.chat_id()), Some(&1));
    }
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forty_eight_parent_and_subagent_writers_all_get_service_at_capacity() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    const CHATS: usize = 48;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut writers = Vec::new();
    // Twenty parent docs plus 28 live child docs, with the same independent
    // writer leases and IDs used by SessionsEngine's subagent sinks.
    for n in 0..CHATS {
        let id = if n < 20 {
            format!("parent-{n:02}")
        } else {
            crate::sessions::subagent_doc_id(
                &format!("parent-{:02}", n % 20),
                &format!("child-{n:02}"),
            )
        };
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open_local(&id).unwrap();
        // initialize_chat_outbox above models an already published baseline.
        // Seed that same baseline on the peer so later incremental updates
        // have their Loro dependencies (otherwise import stays pending).
        let remote = loro::LoroDoc::new();
        remote.import(&h.doc.export_snapshot().unwrap()).unwrap();
        lock(&relay.received).insert(id, remote);
        writers.push(h.writer());
        handles.push(h);
    }
    for h in &handles[..ACTIVE_SYNC_CAP] {
        host.activate_sync(h);
    }
    until(|| {
        handles[..ACTIVE_SYNC_CAP]
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    host.focus_chat(&handles[0].chat_id).unwrap();
    for h in &handles[ACTIVE_SYNC_CAP..] {
        host.activate_sync(h);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP);
    for round in 0..3 {
        let marker = format!("round-{round}");
        for h in &handles {
            h.doc
                .doc()
                .get_text(marker.as_str())
                .insert(0, "delivered")
                .unwrap();
            h.doc.doc().commit();
        }
        // Advance service ages, not network time. Transport work, publication,
        // cancellation and remote update import still run on real sockets.
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                for h in &handles {
                    if h.sync_started.load(Ordering::Acquire) {
                        // Age each admission once, preserving relative service
                        // order and the zero stamp of never-served waiters.
                        let started = h.sync_last_started.load(Ordering::Acquire);
                        if started > now_ms() - SYNC_QUANTUM_MS {
                            h.sync_last_started
                                .fetch_sub(SYNC_QUANTUM_MS + 1, Ordering::AcqRel);
                        }
                        h.sync_service_until.store(0, Ordering::Release);
                    } else {
                        h.sync_wait_since
                            .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
                    }
                }
                assert!(
                    host.sync_resources()["retainedBy"]["connections"]
                        .as_u64()
                        .unwrap()
                        <= ACTIVE_SYNC_CAP as u64
                );
                assert!(zeron_sync::budget::shared().stats().sockets <= 32);
                assert_eq!(
                    lock(&relay.joins_by_chat).get(&handles[0].chat_id),
                    Some(&1),
                    "latest user focus was rotated"
                );
                if handles.iter().all(|h| {
                    lock(&relay.received)
                        .get(&h.chat_id)
                        .is_some_and(|doc| doc.get_text(marker.as_str()).to_string() == "delivered")
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|error| {
            let missing: Vec<_> = handles
                .iter()
                .filter(|h| {
                    !lock(&relay.received)
                        .get(&h.chat_id)
                        .is_some_and(|doc| doc.get_text(marker.as_str()).to_string() == "delivered")
                })
                .map(|h| {
                    (
                        h.chat_id.clone(),
                        h.sync_started.load(Ordering::Acquire),
                        store.has_pending_chat_updates(&h.chat_id).unwrap(),
                        lock(&relay.joins_by_chat).get(&h.chat_id).copied(),
                    )
                })
                .collect();
            panic!("round {round} failed: {error}; missing={missing:?}");
        });
    }
    assert_eq!(lock(&relay.joins_by_chat).len(), CHATS);
    assert_eq!(
        writers.len(),
        CHATS,
        "agent leases must survive transport rotation"
    );
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overdue_writer_gets_a_full_service_turn_despite_new_focuses() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut handles = Vec::new();
    let mut writers = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("writer-{n:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        let h = host.open(&id).unwrap();
        writers.push(h.writer());
        handles.push(h);
    }
    until(|| {
        handles
            .iter()
            .all(|h| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up()))
    })
    .await;
    let overdue = host.open("overdue-child").unwrap();
    let _writer = overdue.writer();
    for h in &handles {
        h.sync_last_started
            .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
    }
    overdue
        .sync_wait_since
        .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
    until(|| lock(&overdue.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    assert!(overdue.sync_service_until.load(Ordering::Acquire) > now_ms());
    for n in 0..6 {
        let id = format!("new-focus-{n}");
        host.focus_chat(&id).unwrap();
        let h = host.open(&id).unwrap();
        let _view = h.watch_messages();
        until(|| lock(&h.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
        assert!(overdue.sync_started.load(Ordering::Acquire));
        assert_eq!(lock(&relay.joins_by_chat).get("overdue-child"), Some(&1));
    }
    host.shutdown_workers().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_saturation_rotates_between_profiles_below_their_local_caps() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let hosts: Vec<_> = dirs
        .iter()
        .map(|dir| host(Arc::new(DocsStore::open(dir.path()).unwrap()), &relay))
        .collect();
    let mut handles = Vec::new();
    let mut writers = Vec::new();
    for (profile, host) in hosts.iter().enumerate() {
        for n in 0..20 {
            let h = host
                .open(&format!("profile-{profile}-writer-{n:02}"))
                .unwrap();
            writers.push(h.writer());
            handles.push(h);
        }
    }
    until(|| zeron_sync::budget::shared().stats().socket_waiting > 0).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            for h in &handles {
                h.sync_last_started
                    .store(now_ms() - SYNC_QUANTUM_MS - 1, Ordering::Release);
            }
            assert!(zeron_sync::budget::shared().stats().sockets <= 32);
            if lock(&relay.joins_by_chat).len() == 40 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a profile monopolized the global socket budget");
    for host in hosts {
        host.shutdown_workers().await;
    }
    until(|| {
        zeron_sync::budget::shared().stats().sockets == 0
            && zeron_sync::budget::shared().stats().socket_waiting == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_jobs_get_a_real_timed_turn_without_any_writer_ending() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Backfill).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let mut writers = Vec::new();
    for n in 0..ACTIVE_SYNC_CAP {
        let id = format!("resident-{n:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        writers.push(host.open(&id).unwrap().writer());
    }
    until(|| relay.joins.load(Ordering::SeqCst) == ACTIVE_SYNC_CAP).await;
    for n in 0..8 {
        let id = format!("cold-timed-{n:02}");
        store.initialize_chat_outbox(&id, &[]).unwrap();
        host.enqueue_wakeup(&id).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(relay.joins.load(Ordering::SeqCst), ACTIVE_SYNC_CAP);
    // Actual wall-clock policy: several complete disk pages elapse without
    // resetting the wait age. No writer, view or lease is dropped to unblock it.
    tokio::time::timeout(
        Duration::from_millis(SYNC_QUANTUM_MS as u64 + 8_000),
        async {
            while store.sync_work_counts().unwrap() != (0, 0) {
                assert!(
                    host.sync_resources()["retainedBy"]["connections"]
                        .as_u64()
                        .unwrap()
                        <= ACTIVE_SYNC_CAP as u64
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        },
    )
    .await
    .expect("cold work starved behind active writers");
    assert_eq!(lock(&relay.joins_by_chat).len(), ACTIVE_SYNC_CAP + 8);
    assert_eq!(writers.len(), ACTIVE_SYNC_CAP);
    host.shutdown_workers().await;
}

/// A wake's receipt is written once. After complete_sync_job deletes the row,
/// the scheduler must stop re-checking it: each check reads the whole command
/// ledger (59–127 ms on real 3–7k-command chats) and forces a snapshot export,
/// and the scheduler ticks every 100 ms for as long as the chat syncs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completed_wake_is_not_rechecked_every_tick() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Hello).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.initialize_chat_outbox("busy", &[]).unwrap();
    let host = host(store.clone(), &relay);
    let busy = host.open("busy").unwrap();
    until(|| lock(&busy.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    host.enqueue_wakeup("busy").unwrap();
    until(|| store.sync_job_version("busy", "wake").unwrap().is_none()).await;
    assert!(busy.sync_started.load(Ordering::Acquire), "the chat must still sync");
    let before = host.inner.wake_checks.load(Ordering::Relaxed);
    // Ten scheduler ticks with the chat open and syncing, and no new wake.
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    busy.touch();
    assert!(busy.sync_started.load(Ordering::Acquire), "the chat must still sync");
    let rechecks = host.inner.wake_checks.load(Ordering::Relaxed) - before;
    assert!(rechecks <= 1, "a completed wake was re-checked {rechecks} times in 1 s");
    // A newer wake is still seen and retired.
    host.enqueue_wakeup("busy").unwrap();
    until(|| store.sync_job_version("busy", "wake").unwrap().is_none()).await;
    drop(busy);
    host.shutdown_workers().await;
}

/// A chat open in the app with its connection caught up and nothing owed
/// costs the dispatcher nothing: no pass runs while it sits there. When the
/// view goes, its idle grace starts counting and the connection closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watched_caught_up_connection_runs_no_passes() {
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let relay = relay(Hold::Checkpoint).await;
    relay.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = host(store.clone(), &relay);
    let handle = host.open("viewed").unwrap();
    let view = handle.watch_messages();
    until(|| lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up())).await;
    until(|| !store.has_pending_chat_updates("viewed").unwrap()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let passes = || host.inner.scheduler_passes.load(Ordering::Relaxed);
    let settled = passes();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(passes(), settled, "a settled, watched connection was polled");
    assert!(handle.sync_started.load(Ordering::Acquire));

    // The last viewer leaving wakes the dispatcher; the grace has run out.
    handle
        .last_access
        .store(now_ms() - SYNC_IDLE_MS - 1, Ordering::Release);
    drop(view);
    until(|| !handle.sync_started.load(Ordering::Acquire)).await;
    assert!(lock(&handle.chat2).is_none());
    host.shutdown_workers().await;
}

/// Retiring a viewed handle (a cutover, a delete) must end its viewers'
/// streams, so they resubscribe: the waiter that kicks the dispatcher when
/// views leave must not hold them open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_handle_ends_its_viewers_streams() {
    let dir = tempfile::tempdir().unwrap();
    let host = DocHost::new(
        Arc::new(DocsStore::open(dir.path()).unwrap()),
        DocHostConfig {
            device_id: "here".into(),
            default_harness: HarnessId::Mock,
            edge: None,
        },
    );
    let handle = host.open("viewed").unwrap();
    let mut messages = handle.watch_messages();
    let mut queue = handle.watch_queue();
    tokio::time::sleep(Duration::from_millis(50)).await; // the waiter runs
    lock(&host.inner.handles).remove("viewed");
    drop(handle);
    tokio::time::timeout(Duration::from_secs(2), async {
        while messages.changed().await.is_ok() {}
        while queue.changed().await.is_ok() {}
    })
    .await
    .expect("a viewer's stream outlived its dropped handle");
    host.shutdown_workers().await;
}

// ── a relayed chat whose host has left the room ─────────────────────────────

const ROOM_HOST: &str = "host-device";

/// The chat's host as its room sees it: a doc behind its own relay task, no
/// engine. It joins and leaves the room as a host's socket does.
struct HostPeer {
    doc: zeron_doc::SessionDoc,
    relay: zeron_sync::chat_relay::RelayHandle,
    /// What its local-update hook has handed out so far.
    sent: Mutex<loro::VersionVector>,
}

/// The host's doc as its relay task reads it (what `EngineChatSink` does).
struct PeerDoc(loro::LoroDoc);

impl zeron_sync::chat_client::ChatDocSink for PeerDoc {
    fn apply_row(&self, _: &[u8], _: u64) -> zeron_sync::chat_client::RowImportOutcome {
        zeron_sync::chat_client::RowImportOutcome::Applied
    }
    fn apply_checkpoint(&self, _: &[u8], _: u64) -> Result<(), String> {
        Ok(())
    }
    fn contains_frontier(&self, _: &[u8]) -> bool {
        true
    }
    fn advance_cursor(&self, _: u64) {}
    fn relay_version(&self) -> Option<Vec<u8>> {
        Some(self.0.oplog_vv().encode())
    }
    fn relay_lacks(&self, version: &[u8]) -> bool {
        loro::VersionVector::decode(version).is_ok_and(|vv| !self.0.oplog_vv().includes_vv(&vv))
    }
    fn relay_behind(&self, version: &[u8]) -> bool {
        loro::VersionVector::decode(version).is_ok_and(|vv| !vv.includes_vv(&self.0.oplog_vv()))
    }
    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let vv = loro::VersionVector::decode(since).ok()?;
        if vv.includes_vv(&self.0.oplog_vv()) {
            return Some(Vec::new());
        }
        self.0.export(loro::ExportMode::updates(&vv)).ok()
    }
    fn relay_import(&self, bytes: &[u8], _: bool) -> zeron_sync::chat_relay::RelayImport {
        match self.0.import(bytes) {
            Ok(status) if status.pending.is_some() => zeron_sync::chat_relay::RelayImport::Parked,
            Ok(_) => zeron_sync::chat_relay::RelayImport::Applied,
            Err(_) => zeron_sync::chat_relay::RelayImport::Unreadable,
        }
    }
}

impl HostPeer {
    fn new() -> Arc<Self> {
        let loro = loro::LoroDoc::new();
        let relay = zeron_sync::chat_relay::RelayHandle::spawn(
            ROOM_HOST,
            Arc::new(PeerDoc(loro.clone())),
            zeron_sync::chat_relay::RelayPolicy { confirm_queue: true },
        );
        Arc::new(Self {
            doc: zeron_doc::SessionDoc::from_doc(loro),
            relay,
            sent: Mutex::new(loro::VersionVector::default()),
        })
    }

    /// One entry of the host's run, relayed as the host relays every commit.
    fn say(&self, id: &str, role: zeron_doc::MessageRole, text: &str) {
        self.doc
            .push_message(&zeron_doc::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zeron_doc::MessagePart::Text { id: "text".into(), text: text.into() }],
                created_at: 1,
                device_id: ROOM_HOST.into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        let mut sent = lock(&self.sent);
        let bytes = self.doc.doc().export(loro::ExportMode::updates(&sent)).unwrap();
        *sent = self.doc.doc().oplog_vv();
        self.relay.local_update(bytes, true);
    }
}

enum RoomPeer {
    Socket(tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>),
    Host(zeron_sync::chat_relay::RelayHandle),
}

#[derive(Default)]
struct RelayRoomState {
    peers: HashMap<String, RoomPeer>,
    /// The host's frames are lost on their way (a link dying under them).
    lose_host_frames: bool,
    host_woken_at: Option<std::time::Instant>,
    wakes: usize,
}

/// A relay-era chat room as `edge/src/chat-room.ts` runs one with the layer
/// on: it stores nothing, forwards relay frames between whoever is in it, and
/// wakes the chat's host when a reader joins or asks the host directly while
/// it's away (at most once a minute). Plain-HTTP pulls are refused.
#[derive(Clone)]
struct RelayRoom {
    url: String,
    state: Arc<Mutex<RelayRoomState>>,
    host: Arc<HostPeer>,
    host_socket: Arc<Mutex<Option<CancellationToken>>>,
}

impl RelayRoom {
    async fn open() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let room = Self {
            url: format!("http://{}", listener.local_addr().unwrap()),
            state: Arc::default(),
            host: HostPeer::new(),
            host_socket: Arc::default(),
        };
        let serving = room.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(serving.clone().serve(stream));
            }
        });
        room
    }

    async fn serve(self, mut stream: tokio::net::TcpStream) {
        use tokio_tungstenite::tungstenite::Message;
        let mut buffer = [0; 8192];
        let headers = loop {
            let n = stream.peek(&mut buffer).await.unwrap_or(0);
            if n == 0 {
                return;
            }
            if let Some(end) = buffer[..n].windows(4).position(|v| v == b"\r\n\r\n") {
                break String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        if !headers.contains("upgrade: websocket") {
            let _ = stream.read(&mut buffer).await;
            let _ = stream
                .write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            return;
        }
        let Ok(ws) = tokio_tungstenite::accept_async(stream).await else { return };
        let (mut sink, mut reader) = ws.split();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if sink.send(message).await.is_err() {
                    return;
                }
            }
        });
        let reply = |kind, header: serde_json::Value| Message::from(encode(kind, &header, &[]));
        let mut device: Option<String> = None;
        while let Some(Ok(message)) = reader.next().await {
            if let Message::Text(text) = &message {
                if text.as_str() == "ping" {
                    let _ = tx.send(Message::from("pong"));
                }
                continue;
            }
            let Some(frame) = decode(&message.into_data()) else { continue };
            match frame.kind {
                frame_type::HELLO => {
                    let from = frame.header["device"].as_str().unwrap_or_default().to_owned();
                    let _ = tx.send(reply(frame_type::STATE, serde_json::json!({
                        "headSeq": 0, "seqFloor": 0, "checkpointSeq": 0, "checkpointSize": 0,
                        "rowCount": 0, "rowBytes": 0, "relay": 1, "legacyPeers": 0,
                    })));
                    let mut state = lock(&self.state);
                    state.peers.insert(from.clone(), RoomPeer::Socket(tx.clone()));
                    // A reader of a relayed chat is here and the host isn't.
                    if frame.header["gen"].as_u64() == Some(3) {
                        self.wake_host(&mut state);
                    }
                    device = Some(from);
                }
                frame_type::ROWS_REQ => {
                    let _ = tx.send(reply(frame_type::ROWS_DONE, serde_json::json!({ "headSeq": 0 })));
                }
                frame_type::PROBE => {
                    let _ = tx.send(reply(frame_type::PROBE_OK, serde_json::json!({ "headSeq": 0 })));
                }
                _ => {
                    if let Some(from) = &device {
                        self.route(from, frame);
                    }
                }
            }
        }
        if let Some(from) = device {
            let mut state = lock(&self.state);
            if matches!(state.peers.get(&from), Some(RoomPeer::Socket(peer)) if peer.same_channel(&tx)) {
                state.peers.remove(&from);
            }
        }
    }

    fn send(peer: &RoomPeer, kind: u8, header: serde_json::Value, payload: &[u8]) {
        match peer {
            RoomPeer::Socket(tx) => {
                let _ = tx.send(encode(kind, &header, payload).into());
            }
            RoomPeer::Host(relay) => relay.room_frame(zeron_sync::chat_frames::WireFrame {
                kind,
                header,
                payload: payload.to_vec(),
            }),
        }
    }

    /// `handleRelayFrame` and `handleQueueReq`, minus quotas and legacy peers.
    fn route(&self, from: &str, frame: zeron_sync::chat_frames::WireFrame) {
        let mut state = lock(&self.state);
        let mut header = frame.header.clone();
        let to = header.as_object_mut().and_then(|h| h.remove("to"));
        header["device"] = serde_json::json!(from);
        match frame.kind {
            frame_type::RELAY | frame_type::ANNOUNCE => {
                if from == ROOM_HOST && state.lose_host_frames {
                    return;
                }
                for (device, peer) in &state.peers {
                    if device != from {
                        Self::send(peer, frame.kind, header.clone(), &frame.payload);
                    }
                }
            }
            frame_type::SYNC_REQ | frame_type::SYNC_DATA => {
                let to = to.as_ref().and_then(|v| v.as_str()).unwrap_or_default();
                match state.peers.get(to) {
                    Some(peer) => Self::send(peer, frame.kind, header, &frame.payload),
                    // A reader asking a host that has already left: bring it back.
                    None if frame.kind == frame_type::SYNC_REQ && to == ROOM_HOST => {
                        self.wake_host(&mut state);
                    }
                    None => {}
                }
            }
            frame_type::QUEUE_REQ => {
                if let Some(peer) = state.peers.get(from) {
                    Self::send(peer, frame_type::QUEUE_DONE, serde_json::json!({ "count": 0 }), &[]);
                }
            }
            _ => {}
        }
    }

    /// `wakeIfAbsent`: the device room's nudge brings the host's socket back.
    fn wake_host(&self, state: &mut RelayRoomState) {
        if state.peers.contains_key(ROOM_HOST)
            || state.host_woken_at.is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        state.host_woken_at = Some(std::time::Instant::now());
        state.wakes += 1;
        let room = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            room.join_host();
        });
    }

    fn join_host(&self) {
        let mut state = lock(&self.state);
        if state.peers.contains_key(ROOM_HOST) {
            return;
        }
        state.peers.insert(ROOM_HOST.into(), RoomPeer::Host(self.host.relay.clone()));
        drop(state);
        let (tx, mut rx) = tokio::sync::mpsc::channel(zeron_sync::chat_relay::PATH_QUEUE);
        self.host.relay.room_up(tx);
        let stop = CancellationToken::new();
        *lock(&self.host_socket) = Some(stop.clone());
        let room = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return,
                    frame = rx.recv() => match frame.as_deref().and_then(decode) {
                        Some(frame) => room.route(ROOM_HOST, frame),
                        None => return,
                    },
                }
            }
        });
    }

    /// An idle host closes its socket: whatever it sends next goes nowhere.
    fn leave_host(&self) {
        if let Some(stop) = lock(&self.host_socket).take() {
            stop.cancel();
        }
        lock(&self.state).peers.remove(ROOM_HOST);
    }
}

fn entry_ids(handle: &ChatDocHandle) -> Vec<String> {
    handle
        .doc
        .read_entries()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.id)
        .collect()
}

/// A chat hosted on another device, relayed (generation 3), viewed here. Its
/// client caught up with the host while the host ran a turn; the end of that
/// turn never arrived (the room lost the frames and the announce that would
/// have said so), and the host, finished, left the room. Nothing here tells
/// the client it's behind. Opening the chat again must bring the host's end
/// of the turn without this device writing anything (2026-11-19: the turn
/// read "Interrupted" until a send pulled the rest in with it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_a_stale_relayed_chat_brings_what_its_host_finished_meanwhile() {
    use zeron_doc::MessageRole::{Assistant, User};
    let _budget_guard = SYNC_TEST_LOCK.lock().await;
    let room = RelayRoom::open().await;
    room.join_host();
    room.host.say("u1", User, "do the thing");
    room.host.say("a1", Assistant, "working on it");

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let viewer = DocHost::new(
        store.clone(),
        DocHostConfig {
            device_id: "viewer".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(&room.url, "test")),
        },
    );
    let workspace = WorkspaceHost::open(
        store.clone(),
        crate::workspace_host::WorkspaceHostConfig {
            device_id: "viewer".into(),
            device_name: "viewer".into(),
            platform: "macos".into(),
            org_id: "org".into(),
            user_id: "user".into(),
            edge: None,
            local_only: false,
        },
    )
    .unwrap();
    workspace.upsert_device_row(&zeron_proto::Device {
        id: ROOM_HOST.into(),
        name: "linux box".into(),
        platform: "linux".into(),
        last_seen_at: Some(chrono::Utc::now()),
        created_at: None,
        version: None,
        cursor_sdk_version: None,
        capabilities: vec![zeron_proto::relay_gen::CHAT_RELAY_CAPABILITY.into()],
    });
    legacy_chat(&workspace, "chat", ROOM_HOST, Some(zeron_proto::relay_gen::RELAY_ROOM_GEN));
    viewer.set_workspace(workspace);

    let handle = viewer.open("chat").unwrap();
    let _view = handle.watch_messages();
    let caught_up = || lock(&handle.chat2).as_ref().is_some_and(|c| c.caught_up());
    tokio::time::timeout(Duration::from_secs(10), async {
        while entry_ids(&handle) != ["u1", "a1"] || !caught_up() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the viewer followed the host's turn");

    // The end of the turn is lost on its way, and the host, done, leaves.
    lock(&room.state).lose_host_frames = true;
    room.host.say("a2", Assistant, "done: here is the rest");
    // Its announce of that commit is lost too.
    tokio::time::sleep(Duration::from_secs(3)).await;
    room.leave_host();
    lock(&room.state).lose_host_frames = false;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(entry_ids(&handle), ["u1", "a1"], "the viewer is behind");
    assert!(handle.sync_started.load(Ordering::Acquire), "its client still runs");
    assert!(caught_up(), "and reads as caught up");
    assert_eq!(lock(&room.state).wakes, 0, "nobody has asked for the host");
    // Hours later, as in the report (its join was its last catch-up).
    lock(&handle.reopen).asked_at = now_ms() - REOPEN_REFRESH_SPACING_MS - 1;

    // The person opens the chat again: the app's transcript watch opens it.
    let reopened = std::time::Instant::now();
    let again = viewer.open("chat").unwrap();
    let _view_again = again.watch_messages();
    tokio::time::timeout(Duration::from_secs(10), async {
        while entry_ids(&handle) != ["u1", "a1", "a2"] {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reopening the chat never brought the end of the host's turn");
    assert!(reopened.elapsed() < Duration::from_secs(5), "took {:?}", reopened.elapsed());
    assert_eq!(lock(&room.state).wakes, 1, "the viewer's ask woke the host once");
    tokio::time::timeout(Duration::from_secs(5), async {
        while lock(&handle.reopen).pending.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the reopen's ask was answered and settled");

    // Opening it again right away asks nobody: one ask per spacing.
    let asked_at = lock(&handle.reopen).asked_at;
    viewer.focus_chat("chat").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(lock(&handle.reopen).asked_at, asked_at);
    assert!(lock(&handle.reopen).pending.is_none());
    viewer.shutdown_workers().await;
}
