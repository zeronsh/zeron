//! Isolated, offline memory experiments for docs/memory-audit-2026-10-02.md.
//! Run each scenario in a separate process; this counts requested live Rust
//! allocations, not RSS, allocator metadata, native allocations, or GPU memory.
//! cargo run --release --locked -p zeron-engine --example memory-audit -- SCENARIO
//! Scenarios: docs, journal, outbox, rpc, terminal, history, sync, sync-catchup, engine, message-lookup.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use zeron_engine::{DocHost, DocHostConfig, RunJournal, Terminals};
use zeron_proto::{AgentEvent, HarnessId};
use zeron_rpc::{RpcError, RpcReply, RpcService};
use zeron_sync::DocsStore;
use zeron_sync::chat_client::{ChatClient, ChatDocSink, CheckpointFetcher, RowImportOutcome};
use zeron_sync::chat_frames::{decode, encode, frame_type};

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every allocation operation delegates unchanged pointers/layouts to
// System. Counters neither allocate nor modify the returned memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, size) };
        if !next.is_null() {
            if size >= layout.size() {
                add(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed);
            }
        }
        next
    }
}

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn baseline() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

fn sample(label: &str, base: usize, detail: Value) {
    let live = LIVE.load(Relaxed);
    let peak = PEAK.load(Relaxed);
    println!(
        "{}",
        json!({"scenario":label,
        "liveRustBytesAboveBaseline": live as i128 - base as i128,
        "peakRustBytesAboveBaseline": peak.saturating_sub(base), "detail":detail})
    );
}

async fn docs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(DocsStore::open(dir.path())?);
    let host = DocHost::new(
        store,
        DocHostConfig {
            device_id: "audit".into(),
            default_harness: HarnessId::Mock,
            edge: None,
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let base = baseline();
    let mut total_snapshot = 0usize;
    // Compressible fixtures deliberately test the snapshot-based estimate.
    for i in 0..12 {
        let handle = host.open_local(&format!("audit-{i}"))?;
        handle.write_user_message("message", &"x".repeat(2 * 1024 * 1024), i)?;
        total_snapshot += handle.doc().export_snapshot()?.len();
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let resources = host.sync_resources();
    sample(
        "docs",
        base,
        json!({"rawTextBytes":12 * 2 * 1024 * 1024,
        "compressedSnapshotBytes":total_snapshot,
        "estimateUsingCurrentFormulaBytes":(total_snapshot * 6).max(12 * 512 * 1024),
        "resources":resources}),
    );
    // A detached quiet watch has no subsequent commit to clear its mirror.
    for i in 0..12 {
        let handle = host.open_local(&format!("audit-{i}"))?;
        drop(handle.watch_messages());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    sample("docs-after-quiet-watches", base, host.sync_resources());
    // Cross the count limit to distinguish warm retention from a broken LRU.
    for i in 12..20 {
        drop(host.open_local(&format!("audit-{i}"))?);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    sample("docs-after-count-eviction", base, host.sync_resources());
    host.shutdown_workers().await;
    drop(host);
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample("docs-after-drop", base, json!({}));
    Ok(())
}

fn journal() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let journal = RunJournal::open(dir.path())?;
    let event = AgentEvent::TextDelta {
        text: "x".repeat(64 * 1024),
    };
    for _ in 0..256 {
        journal.append("audit", &event)?;
    }
    drop(event);
    let base = baseline();
    let replay = journal.replay("audit", 256)?;
    sample(
        "journal-empty-tail-replay",
        base,
        json!({"returnedEvents":replay.len(), "textOnDiskBytes":16 * 1024 * 1024}),
    );
    drop(replay);
    let base = baseline();
    let last = journal.last_event("audit")?;
    sample(
        "journal-last-event",
        base,
        json!({"returnedEvents":usize::from(last.is_some())}),
    );
    drop(last);
    // Reopening the journal exercises scan_tail's simultaneous raw file and
    // fully decoded event allocations before appending a tiny event.
    drop(journal);
    let reopened = RunJournal::open(dir.path())?;
    let base = baseline();
    reopened.append(
        "audit",
        &AgentEvent::TextDelta {
            text: "tail".into(),
        },
    )?;
    sample("journal-reopen-append", base, json!({}));
    Ok(())
}

fn outbox() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = DocsStore::open(dir.path())?;
    let bytes = vec![42; 256 * 1024];
    for i in 0..64 {
        store.enqueue_chat_update("audit", &format!("batch-{i}"), &bytes)?;
    }
    drop(bytes);
    let base = baseline();
    sample(
        "outbox-disk-only",
        base,
        json!({"durablePayloadBytes":16 * 1024 * 1024}),
    );
    let pending: Vec<(String, Arc<[u8]>)> = store
        .pending_chat_updates_window("audit", 32, 256 * 1024)?
        .into_iter()
        .map(|(id, bytes)| (id, bytes.into()))
        .collect();
    sample(
        "outbox-load",
        base,
        json!({"batches":pending.len(), "payloadBytes":pending.iter().map(|(_, b)|b.len()).sum::<usize>()}),
    );
    let copied = pending.clone();
    sample(
        "outbox-with-send-copy",
        base,
        json!({"copiedBatches":copied.len()}),
    );
    drop(copied);
    drop(pending);
    sample("outbox-after-drop", base, json!({}));
    Ok(())
}

// Isolate the real ChatClient's buffer ownership from CRDT allocation. This
// sink uses the production SQLite outbox but deliberately discards remote row
// bodies; the local wire fixture is not a full engine/edge convergence test.
struct AuditSink(Arc<DocsStore>);
impl ChatDocSink for AuditSink {
    fn pending_window(&self) -> Result<Option<Vec<(String, Vec<u8>)>>, String> {
        self.0
            .pending_chat_updates_window(
                "audit",
                zeron_sync::chat_client::PENDING_WINDOW_BATCHES,
                zeron_sync::chat_client::PENDING_WINDOW_BYTES,
            )
            .map(Some)
            .map_err(|e| e.to_string())
    }
    fn pending_update_count(&self) -> Result<Option<u64>, String> {
        self.0
            .pending_chat_update_count("audit")
            .map(Some)
            .map_err(|e| e.to_string())
    }
    fn cursor_is_verified(&self) -> bool {
        true
    }
    fn pending_updates(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.0
            .pending_chat_updates("audit")
            .map_err(|e| e.to_string())
    }
    fn persist_update(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.0
            .enqueue_chat_update("audit", id, bytes)
            .map_err(|e| e.to_string())
    }
    fn acknowledge_update(&self, id: &str) -> Result<(), String> {
        self.0
            .acknowledge_chat_update("audit", id)
            .map_err(|e| e.to_string())
    }
    fn apply_row(&self, _: &[u8], _: u64) -> RowImportOutcome {
        RowImportOutcome::Applied
    }
    fn apply_checkpoint(&self, _: &[u8], _: u64) -> Result<(), String> {
        Ok(())
    }
    fn contains_frontier(&self, _: &[u8]) -> bool {
        false
    }
    fn advance_cursor(&self, _: u64) {}
}

struct AuditFetcher(Option<Arc<tokio::sync::Notify>>);
impl CheckpointFetcher for AuditFetcher {
    fn fetch(&self) -> futures::future::BoxFuture<'static, Result<Vec<u8>, zeron_sync::SyncError>> {
        let gate = self.0.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok(vec![0])
        })
    }
}

struct AuditEdge {
    url: String,
    // 0: healthy acknowledgements; 1: read pushes without acknowledgements;
    // 2: disconnect existing and reject newly accepted sockets.
    mode: tokio::sync::watch::Sender<u8>,
    sent_rows: Arc<AtomicUsize>,
    stop: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl AuditEdge {
    async fn start(checkpoint: bool) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("ws://{}", listener.local_addr()?);
        let (mode, _) = tokio::sync::watch::channel(0u8);
        let mode_source = mode.clone();
        let sent_rows = Arc::new(AtomicUsize::new(0));
        let row_count = sent_rows.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopped.cancelled() => break,
                    _ = peers.join_next(), if !peers.is_empty() => {},
                    socket = listener.accept() => {
                        let Ok((socket, _)) = socket else { break };
                        if *mode_source.borrow() == 2 { continue; }
                        let mut mode = mode_source.subscribe();
                        let row_count = row_count.clone();
                        peers.spawn(async move {
                            let Ok(mut ws) = tokio_tungstenite::accept_async(socket).await else { return };
                            let mut sequence = 0u64;
                            loop {
                                let message = tokio::select! {
                                    changed = mode.changed() => {
                                        if changed.is_err() || *mode.borrow() == 2 { break; }
                                        continue;
                                    }
                                    message = ws.next() => match message { Some(Ok(m)) => m, _ => break },
                                };
                                if let tokio_tungstenite::tungstenite::Message::Text(text) = &message {
                                    if text == "ping" && ws.send("pong".into()).await.is_err() { break }
                                    continue;
                                }
                                let Some(frame) = decode(&message.into_data()) else { continue };
                                let reply = match frame.kind {
                                    frame_type::HELLO => encode(frame_type::STATE, &json!({
                                        "headSeq":if checkpoint {64} else {0},"seqFloor":0,"checkpointSeq":0,
                                        "checkpointSize":usize::from(checkpoint),"rowCount":if checkpoint {64} else {0},
                                        "rowBytes":if checkpoint {16 * 1024 * 1024} else {0}}), &[]),
                                    frame_type::ROWS_REQ => {
                                        if checkpoint {
                                            let bytes = vec![42; 256 * 1024];
                                            for seq in 1..=64 {
                                                let row = encode(frame_type::ROW, &json!({"seq":seq,
                                                    "device":"other","batchId":format!("row-{seq}")}), &bytes);
                                                if ws.send(row.into()).await.is_err() { return; }
                                                row_count.fetch_add(1, Relaxed);
                                            }
                                        }
                                        encode(frame_type::ROWS_DONE, &json!({"headSeq":if checkpoint {64} else {sequence}}), &[])
                                    }
                                    frame_type::PUSH => {
                                        if *mode.borrow() != 0 { continue; }
                                        sequence += 1;
                                        encode(frame_type::ACK, &json!({"batchId":frame.header["batchId"],"seq":sequence,"dup":false}), &[])
                                    }
                                    frame_type::PROBE => encode(frame_type::PROBE_OK, &json!({"headSeq":sequence}), &[]),
                                    _ => continue,
                                };
                                if ws.send(reply.into()).await.is_err() { break }
                            }
                        });
                    }
                }
            }
            peers.abort_all();
            while peers.join_next().await.is_some() {}
        });
        Ok(Self {
            url,
            mode,
            sent_rows,
            stop,
            task,
        })
    }

    async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

async fn sync() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(DocsStore::open(dir.path())?);
    let sink = Arc::new(AuditSink(store));
    let edge = AuditEdge::start(false).await?;
    let base = baseline();
    sample("sync-disabled", base, json!({"pendingBatches":0}));
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ChatClient::connect(&edge.url, sink, Arc::new(AuditFetcher(None)), "audit", 0),
    )
    .await??;
    for i in 0..64 {
        client.enqueue_batch(format!("healthy-{i}"), vec![42; 256 * 1024]);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while client.stats().pending_pushes != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    sample(
        "sync-healthy",
        base,
        json!({"pendingBatches":client.stats().pending_pushes,"emittedBytes":16 * 1024 * 1024}),
    );
    edge.mode.send_replace(1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    for i in 0..64 {
        client.enqueue_batch(format!("stalled-{i}"), vec![42; 256 * 1024]);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    sample(
        "sync-stalled-acks",
        base,
        json!({"pendingBatches":client.stats().pending_pushes,"pendingPayloadBytes":16 * 1024 * 1024}),
    );
    edge.mode.send_replace(2);
    tokio::time::sleep(Duration::from_millis(100)).await;
    for i in 0..64 {
        client.enqueue_batch(format!("offline-{i}"), vec![42; 256 * 1024]);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    sample(
        "sync-offline",
        base,
        json!({"pendingBatches":client.stats().pending_pushes,"pendingPayloadBytes":32 * 1024 * 1024}),
    );
    client.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample("sync-after-client-shutdown", base, json!({}));
    edge.shutdown().await;
    Ok(())
}

async fn sync_catchup() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let sink = Arc::new(AuditSink(Arc::new(DocsStore::open(dir.path())?)));
    let gate = Arc::new(tokio::sync::Notify::new());
    let edge = AuditEdge::start(true).await?;
    let url = edge.url.clone();
    let fetcher = Arc::new(AuditFetcher(Some(gate.clone())));
    let base = baseline();
    let connect =
        tokio::spawn(async move { ChatClient::connect(&url, sink, fetcher, "audit", 0).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while edge.sent_rows.load(Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    sample(
        "sync-catchup-waiting-checkpoint",
        base,
        json!({"sentRows":edge.sent_rows.load(Relaxed),"plannedRows":64,"rowPayloadBytes":16 * 1024 * 1024}),
    );
    gate.notify_one();
    let client = tokio::time::timeout(Duration::from_secs(10), connect).await???;
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample(
        "sync-catchup-complete",
        base,
        json!({"cursor":client.stats().cursor}),
    );
    client.shutdown().await;
    edge.shutdown().await;
    Ok(())
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}
struct QuietService(Arc<AtomicUsize>);
#[async_trait]
impl RpcService for QuietService {
    async fn handle(&self, method: &str, _: Value) -> Result<RpcReply, RpcError> {
        self.0.fetch_add(1, Relaxed);
        let guard = Active(self.0.clone());
        if method == "NeverReply" {
            let result = std::future::pending::<Result<RpcReply, RpcError>>().await;
            drop(guard);
            result
        } else {
            Ok(RpcReply::Stream(Box::pin(futures::stream::unfold(
                guard,
                |guard| async move {
                    std::future::pending::<()>().await;
                    Some((Value::Null, guard))
                },
            ))))
        }
    }
}

async fn rpc() -> anyhow::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    let client = zeron_rpc::memory_client(Arc::new(QuietService(active.clone())));
    let base = baseline();
    for _ in 0..64 {
        let result = tokio::time::timeout(
            Duration::from_millis(2),
            client.call("NeverReply", Value::Null),
        )
        .await;
        anyhow::ensure!(result.is_err(), "quiet call unexpectedly replied");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample(
        "rpc-cancelled-calls",
        base,
        json!({"activeServerRequests":active.load(Relaxed)}),
    );
    for _ in 0..32 {
        drop(client.subscribe("QuietStream", Value::Null).await?);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample(
        "rpc-dropped-legacy-streams",
        base,
        json!({"activeServerRequests":active.load(Relaxed)}),
    );
    for _ in 0..32 {
        drop(client.subscribe_scoped("QuietStream", Value::Null).await?);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample(
        "rpc-dropped-scoped-streams",
        base,
        json!({"activeServerRequests":active.load(Relaxed)}),
    );
    drop(client);
    tokio::time::sleep(Duration::from_millis(100)).await;
    sample(
        "rpc-after-connection-drop",
        base,
        json!({"activeServerRequests":active.load(Relaxed)}),
    );
    Ok(())
}

async fn terminal() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let terminals = Terminals::new();
    let shell = if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/sh"
    };
    let terminal = terminals.open_with_shell(dir.path().to_str().unwrap(), 120, 24, Some(shell))?;
    let mut receiver = terminals.subscribe(&terminal.id, None)?;
    let base = baseline();
    let command = if cfg!(windows) {
        "1..64 | ForEach-Object { [Console]::Write(('x' * 262144)) }; exit\r"
    } else {
        "head -c 16777216 /dev/zero | tr '\\000' x; exit\n"
    };
    terminals.write_bytes(&terminal.id, command.as_bytes())?;
    // Do not drain: simulate a subscriber blocked behind a slow RPC peer.
    tokio::time::sleep(Duration::from_secs(10)).await;
    sample(
        "terminal-stalled-subscriber",
        base,
        json!({"queuedEvents":receiver.len()}),
    );
    let mut bytes = 0usize;
    let mut exited = false;
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = receiver.recv().await {
            match event {
                zeron_proto::TerminalEvent::Data { data, .. } => bytes += data.len(),
                zeron_proto::TerminalEvent::Exit { .. } => exited = true,
                zeron_proto::TerminalEvent::Gap { .. } => {}
            }
        }
    })
    .await?;
    sample(
        "terminal-after-drain",
        base,
        json!({"queuedEncodedBytes":bytes,"receivedExit":exited}),
    );
    terminals.shutdown();
    Ok(())
}

fn random_text(seed: &mut u64, length: usize) -> String {
    (0..length)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (b'!' + (*seed % 90) as u8) as char
        })
        .collect()
}

fn history() -> anyhow::Result<()> {
    let doc = zeron_doc::SessionDoc::init("audit")?;
    let mut seed = 12345;
    let base = baseline();
    for _ in 0..128 {
        doc.doc()
            .get_map("meta")
            .insert("auditReplacement", random_text(&mut seed, 64 * 1024))?;
        doc.doc().commit();
    }
    let full = doc.export_snapshot()?;
    let thin = doc
        .doc()
        .export(loro::ExportMode::ShallowSnapshot(std::borrow::Cow::Owned(
            doc.doc().state_frontiers(),
        )))?;
    sample(
        "history-replacement",
        base,
        json!({"visibleFieldBytes":64 * 1024,
        "writtenBytes":128 * 64 * 1024,"fullSnapshotBytes":full.len(),"shallowSnapshotBytes":thin.len()}),
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        match scenario.as_str() {
            "docs" => docs().await,
            "journal" => journal(),
            "outbox" => outbox(),
            "rpc" => rpc().await,
            "terminal" => terminal().await,
            "history" => history(),
            "sync" => sync().await,
            "sync-catchup" => sync_catchup().await,
            "engine" => engine().await,
            "message-lookup" => message_lookup(),
            _ => anyhow::bail!(
                "Choose docs, journal, outbox, rpc, terminal, history, sync, sync-catchup, engine, or message-lookup"
            ),
        }
    })
}

async fn engine() -> anyhow::Result<()> {
    let base = baseline();
    for cycle in 0..4 {
        let dir = tempfile::tempdir()?;
        let core = zeron_engine::EngineCore::assemble(
            dir.path(),
            Arc::new(zeron_engine::default_registry()),
            HarnessId::Mock,
            None,
        )?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        sample("engine-live", base, json!({"cycle":cycle}));
        core.shutdown().await;
        drop(core);
        tokio::time::sleep(Duration::from_millis(200)).await;
        sample("engine-after-shutdown", base, json!({"cycle":cycle}));
    }
    Ok(())
}

// Compare the exact former idempotency read with the scalar lookup over the
// same document. CRDT history and publication remain unchanged by this test.
fn message_lookup() -> anyhow::Result<()> {
    for count in [8, 32, 128] {
        let doc = zeron_doc::SessionDoc::init("lookup")?;
        for i in 0..count {
            doc.push_message(&zeron_doc::SessionMessageEntry {
                id: format!("message-{i}"),
                role: zeron_doc::MessageRole::User,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: "text".into(),
                    text: "x".repeat(256_000),
                }],
                created_at: i,
                device_id: "host".into(),
                status: Some(zeron_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })?;
        }
        for legacy in [true, false] {
            let base = baseline();
            let start = std::time::Instant::now();
            for _ in 0..8 {
                let exists = if legacy {
                    doc.read_entries()?
                        .iter()
                        .any(|entry| entry.id == "missing")
                } else {
                    doc.has_message("missing")
                };
                assert!(!std::hint::black_box(exists));
            }
            sample(
                if legacy {
                    "message-lookup-full-history"
                } else {
                    "message-lookup-scalar"
                },
                base,
                json!({"entries":count, "textBytes":count * 256_000, "iterations":8, "elapsedNs":start.elapsed().as_nanos()}),
            );
        }
    }
    Ok(())
}
