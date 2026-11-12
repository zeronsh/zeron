//! The relay layer end to end: real `ChatClient`s, real sockets, real Loro docs
//! and a real ChatRoom Durable Object under `wrangler dev`, with an old client
//! (no relay layer at all) in the mix.
//!
//! Ignored by default, because it needs the edge running:
//!
//! ```sh
//! (cd edge && npx wrangler dev --port 27640 --var AUTH_MODE:dev --var CHAT_RELAY_LAYER:1)
//! ZERON_EDGE_WS=ws://127.0.0.1:27640 cargo test -p zeron-sync --test relay_edge -- --ignored
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use loro::{ExportMode, LoroDoc, VersionVector};
use zeron_sync::SyncError;
use zeron_sync::chat_client::{ChatClient, ChatDocSink, ChatTransport, CheckpointFetcher, RowImportOutcome};
use zeron_sync::chat_relay::{RelayEvent, RelayHandle, RelayImport, RelayPolicy};

const TOKEN: &str = "relay-e2e@org1";

fn ws_base() -> String {
    std::env::var("ZERON_EDGE_WS").unwrap_or_else(|_| "ws://127.0.0.1:27640".into())
}

fn http_base() -> String {
    ws_base().replacen("ws", "http", 1)
}

struct Sink {
    doc: LoroDoc,
    cursor: AtomicU64,
}

impl ChatDocSink for Sink {
    fn cursor_is_verified(&self) -> bool {
        true
    }
    fn apply_row(&self, bytes: &[u8], cursor: u64) -> RowImportOutcome {
        match self.doc.import(bytes) {
            Ok(status) if status.pending.is_some() => RowImportOutcome::PendingDependencies,
            _ => {
                self.cursor.store(cursor, Ordering::SeqCst);
                RowImportOutcome::Applied
            }
        }
    }
    fn apply_checkpoint(&self, bytes: &[u8], cursor: u64) -> Result<(), String> {
        self.doc.import(bytes).map_err(|e| e.to_string())?;
        self.cursor.store(cursor, Ordering::SeqCst);
        Ok(())
    }
    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        let Ok(vv) = VersionVector::decode(frontier) else {
            return false;
        };
        !vv.is_empty() && self.doc.oplog_vv().includes_vv(&vv)
    }
    fn advance_cursor(&self, cursor: u64) {
        self.cursor.store(cursor, Ordering::SeqCst);
    }
    fn relay_version(&self) -> Option<Vec<u8>> {
        Some(self.doc.oplog_vv().encode())
    }
    fn relay_lacks(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !self.doc.oplog_vv().includes_vv(&vv))
    }
    fn relay_behind(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !vv.includes_vv(&self.doc.oplog_vv()))
    }
    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let vv = VersionVector::decode(since).ok()?;
        if vv.includes_vv(&self.doc.oplog_vv()) {
            return Some(Vec::new());
        }
        self.doc.export(ExportMode::updates(&vv)).ok()
    }
    fn relay_import(&self, bytes: &[u8], _history: bool) -> RelayImport {
        match self.doc.import(bytes) {
            Ok(status) if status.pending.is_some() => RelayImport::Parked,
            Ok(_) => RelayImport::Applied,
            Err(_) => RelayImport::Unreadable,
        }
    }
}

struct Http {
    chat: String,
    device: String,
}

impl Http {
    fn url(&self, path: &str) -> String {
        format!("{}/chat2/{}/{path}", http_base(), self.chat)
    }
}

impl CheckpointFetcher for Http {
    fn fetch(&self) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let url = self.url("checkpoint");
        Box::pin(async move {
            let res = reqwest::Client::new()
                .get(&url)
                .bearer_auth(TOKEN)
                .send()
                .await
                .map_err(|e| SyncError::WebSocket(e.to_string()))?;
            if !res.status().is_success() {
                return Err(SyncError::Protocol(format!("checkpoint {}", res.status())));
            }
            Ok(res.bytes().await.map_err(|e| SyncError::WebSocket(e.to_string()))?.to_vec())
        })
    }
}

impl ChatTransport for Http {
    fn fetch_rows(&self, after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let url = format!("{}?after={after}&device={}&relay=1", self.url("rows"), self.device);
        Box::pin(async move {
            let res = reqwest::Client::new()
                .get(&url)
                .bearer_auth(TOKEN)
                .send()
                .await
                .map_err(|e| SyncError::WebSocket(e.to_string()))?;
            Ok(res.bytes().await.map_err(|e| SyncError::WebSocket(e.to_string()))?.to_vec())
        })
    }
    fn push(&self, batch: String, bytes: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>> {
        let url = format!("{}?batchId={batch}&device={}", self.url("rows"), self.device);
        Box::pin(async move {
            let res = reqwest::Client::new()
                .post(&url)
                .bearer_auth(TOKEN)
                .body(bytes)
                .send()
                .await
                .map_err(|e| SyncError::WebSocket(e.to_string()))?;
            res.text().await.map_err(|e| SyncError::WebSocket(e.to_string()))
        })
    }
}

struct Peer {
    name: String,
    sink: Arc<Sink>,
    client: ChatClient,
    relay: Option<RelayHandle>,
    sent: std::sync::Mutex<VersionVector>,
}

impl Peer {
    /// A device running the relay layer.
    async fn relay(chat: &str, name: &str, host: bool) -> Self {
        let sink = Arc::new(Sink { doc: LoroDoc::new(), cursor: AtomicU64::new(0) });
        Self::relay_with(chat, name, host, sink).await
    }

    async fn relay_with(chat: &str, name: &str, host: bool, sink: Arc<Sink>) -> Self {
        let policy = RelayPolicy { confirm_queue: host };
        let relay = RelayHandle::spawn(name, sink.clone(), policy);
        let http = Arc::new(Http { chat: chat.into(), device: name.into() });
        let url = Arc::new(zeron_sync::StaticUrl(format!(
            "{}/chat2/{chat}/ws?device={name}&token={TOKEN}",
            ws_base()
        )));
        let client = ChatClient::connect_via_relay(
            url,
            sink.clone(),
            http.clone(),
            name,
            0,
            http,
            Some(relay.clone()),
        )
        .await
        .expect("connect");
        client.set_queue_pushes(!host);
        let sent = std::sync::Mutex::new(sink.doc.oplog_vv());
        Self { name: name.into(), sink, client, relay: Some(relay), sent }
    }

    /// A client from before the layer: never asks for it, reads and writes rows.
    async fn old(chat: &str, name: &str) -> Self {
        let sink = Arc::new(Sink { doc: LoroDoc::new(), cursor: AtomicU64::new(0) });
        let http = Arc::new(Http { chat: chat.into(), device: name.into() });
        let client = ChatClient::connect(
            &format!("{}/chat2/{chat}/ws?device={name}&token={TOKEN}", ws_base()),
            sink.clone(),
            http,
            name,
            0,
        )
        .await
        .expect("connect");
        let sent = std::sync::Mutex::new(VersionVector::default());
        Self { name: name.into(), sink, client, relay: None, sent }
    }

    fn commit(&self, text: &str) -> Vec<u8> {
        let body = self.sink.doc.get_text("body");
        body.insert(body.len_unicode(), text).unwrap();
        self.sink.doc.commit();
        let mut sent = self.sent.lock().unwrap();
        let bytes = self.sink.doc.export(ExportMode::updates(&sent)).unwrap();
        *sent = self.sink.doc.oplog_vv();
        bytes
    }

    /// A host relaying a commit: no row, no outbox.
    fn stream(&self, text: &str) {
        let bytes = self.commit(text);
        self.relay.as_ref().expect("relay").local_update(bytes, true);
    }

    /// A commit through the durable path: the queue, or an old client's rows.
    fn send(&self, text: &str) {
        self.client.enqueue_update(self.commit(text));
    }

    fn body(&self) -> String {
        self.sink.doc.get_text("body").to_string()
    }

    async fn caught_up(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.client.caught_up() {
            assert!(Instant::now() < deadline, "{} never caught up", self.name);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Post what a host owes: a checkpoint at the floor, which prunes nothing.
    async fn post_checkpoint(&self, chat: &str, floor: u64) {
        use base64::Engine as _;
        let snapshot = self.sink.doc.export(ExportMode::Snapshot).unwrap();
        let frontier = self.sink.doc.oplog_vv().encode();
        let res = reqwest::Client::new()
            .post(format!("{}/chat2/{chat}/checkpoint?seqCovered={floor}", http_base()))
            .bearer_auth(TOKEN)
            .header("x-chat2-frontier", base64::engine::general_purpose::STANDARD.encode(frontier))
            .body(snapshot)
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }
}

async fn stats(chat: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}/chat2/{chat}/stats", http_base()))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn converged(peers: &[&Peer]) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let first = peers[0].sink.doc.oplog_vv();
        if peers.iter().all(|p| p.sink.doc.oplog_vv() == first) {
            let body = peers[0].body();
            assert!(peers.iter().all(|p| p.body() == body), "same ops, different text");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never converged: {:?}",
            peers.iter().map(|p| (p.name.clone(), p.body())).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check().await {
        assert!(Instant::now() < deadline, "never happened: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn chat_id() -> String {
    format!("relay-e2e-{}", uuid::Uuid::new_v4())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn two_devices_and_a_host_with_nothing_stored() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let a = Peer::relay(&chat, "a", false).await;
    let b = Peer::relay(&chat, "b", false).await;
    for peer in [&host, &a, &b] {
        peer.caught_up().await;
    }
    assert!(host.client.relay_room(), "run the edge with CHAT_RELAY_LAYER=1");
    // (Caught up includes a socket's answer: the queue has been read.)
    tokio::time::sleep(Duration::from_millis(200)).await;
    for i in 0..100 {
        host.stream(&format!("chunk {i} "));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    converged(&[&host, &a, &b]).await;
    assert!(a.body().contains("chunk 99"));
    let stats = stats(&chat).await;
    assert_eq!(stats["headSeq"], 0, "{stats}");
    assert_eq!(stats["rowCount"], 0, "{stats}");
    assert_eq!(stats["relayQueue"], 0, "{stats}");
    assert_eq!(stats["checkpointSize"], 0, "{stats}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_late_joiner_catches_up_from_the_host_through_the_room() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    host.caught_up().await;
    // Large enough to take several sync windows.
    let mut text = String::new();
    for i in 0..400_000u64 {
        text.push_str(&format!("{:x}", i.wrapping_mul(0x9E3779B97F4A7C15)));
    }
    host.stream(&text);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let late = Peer::relay(&chat, "late", false).await;
    converged(&[&host, &late]).await;
    assert_eq!(late.body().len(), text.len());
    let stats = stats(&chat).await;
    assert_eq!(stats["rowCount"], 0, "{stats}");
    assert_eq!(stats["checkpointSize"], 0, "{stats}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_message_to_an_offline_host_is_held_then_deleted() {
    let chat = chat_id();
    let phone = Peer::relay(&chat, "phone", false).await;
    phone.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    phone.send("sent while the host slept");
    until("the queue to hold it", async || stats(&chat).await["relayQueue"] == 1).await;
    assert_eq!(phone.client.stats().pending_pushes, 0, "acked means durable");
    // Hung up, so only the room has it.
    drop(phone);

    let host = Peer::relay(&chat, "host", true).await;
    until("the host to get it", async || host.body() == "sent while the host slept").await;
    until("the queue to empty", async || stats(&chat).await["relayQueue"] == 0).await;
    let late = Peer::relay(&chat, "late", false).await;
    converged(&[&host, &late]).await;
    assert_eq!(late.body(), "sent while the host slept");
    let stats = stats(&chat).await;
    assert_eq!(stats["rowCount"], 0, "{stats}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_reconnecting_viewer_recovers_what_it_missed() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let viewer = Peer::relay(&chat, "viewer", false).await;
    host.caught_up().await;
    viewer.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    host.stream("one ");
    converged(&[&host, &viewer]).await;
    // Drop the viewer's socket: a redial gets a fresh session.
    let doc = viewer.sink.clone();
    viewer.client.shutdown().await;
    for i in 0..20 {
        host.stream(&format!("missed {i} "));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let back = Peer::relay_with(&chat, "viewer", false, doc).await;
    converged(&[&host, &back]).await;
    assert!(back.body().contains("missed 19"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn the_host_going_offline_mid_stream_loses_nothing() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let viewer = Peer::relay(&chat, "viewer", false).await;
    host.caught_up().await;
    viewer.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    host.stream("seen ");
    converged(&[&host, &viewer]).await;
    let doc = host.sink.clone();
    let sent = host.sink.doc.oplog_vv();
    host.client.shutdown().await;
    // Work on both sides, unreachable from each other.
    let body = doc.doc.get_text("body");
    body.insert(body.len_unicode(), "written offline ").unwrap();
    doc.doc.commit();
    viewer.send("sent meanwhile ");
    until("the queue to hold it", async || stats(&chat).await["relayQueue"] == 1).await;
    assert!(!viewer.body().contains("written offline"));
    let back = Peer::relay_with(&chat, "host", true, doc).await;
    *back.sent.lock().unwrap() = sent;
    converged(&[&back, &viewer]).await;
    assert!(viewer.body().contains("written offline"));
    assert!(back.body().contains("sent meanwhile"));
    until("the queue to empty", async || stats(&chat).await["relayQueue"] == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn an_old_client_joining_is_healed_by_the_downgrade_rule() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let viewer = Peer::relay(&chat, "viewer", false).await;
    host.caught_up().await;
    viewer.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Relayed, so in no stored row an old client could read.
    host.stream("relayed before the old client came back ");
    viewer.send("queued too ");
    converged(&[&host, &viewer]).await;
    until("the queue to empty", async || stats(&chat).await["relayQueue"] == 0).await;

    let mut events = host.relay.as_ref().unwrap().events();
    let old = Peer::old(&chat, "old").await;
    let event = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("the host hears about it")
            .unwrap();
        if event == RelayEvent::LegacyPeer {
            break event;
        }
    };
    assert_eq!(event, RelayEvent::LegacyPeer);
    assert_eq!(old.body(), "", "nothing stored yet: it can't see relayed ops");

    // The rule, in order: mode first (new commits are rows), then a checkpoint
    // exported after that, at the floor, so it prunes nothing.
    host.client.enqueue_update(host.commit("stored after the switch "));
    host.post_checkpoint(&chat, 0).await;
    host.client.enqueue_update(host.commit("and more after the checkpoint "));
    // The old client's redial reads the checkpoint, as any reader does.
    old.client.redial();
    converged(&[&host, &old]).await;
    assert!(old.body().contains("relayed before the old client came back"));
    assert!(old.body().contains("queued too"));
    assert!(old.body().contains("and more after the checkpoint"));
    converged(&[&host, &viewer, &old]).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn an_old_clients_message_reaches_everyone_in_a_relay_room() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let viewer = Peer::relay(&chat, "viewer", false).await;
    let old = Peer::old(&chat, "old").await;
    host.caught_up().await;
    viewer.caught_up().await;
    old.send("from the old client");
    until("the row to land", async || stats(&chat).await["rowCount"] == 1).await;
    converged(&[&host, &viewer, &old]).await;
    assert_eq!(viewer.body(), "from the old client");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn existing_rows_and_checkpoints_survive_the_whole_flow() {
    let chat = chat_id();
    let old = Peer::old(&chat, "old").await;
    old.send("a stored row ");
    until("the row to land", async || stats(&chat).await["rowCount"] == 1).await;
    drop(old);
    let host = Peer::relay(&chat, "host", true).await;
    host.caught_up().await;
    assert_eq!(host.body(), "a stored row ");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let phone = Peer::relay(&chat, "phone", false).await;
    phone.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    phone.send("queued ");
    until("the queue to empty", async || {
        stats(&chat).await["relayQueue"] == 0 && host.body().contains("queued")
    })
    .await;
    host.post_checkpoint(&chat, 0).await;
    let stats = stats(&chat).await;
    assert_eq!(stats["rowCount"], 1, "the stored row is still there: {stats}");
    assert_eq!(stats["headSeq"], 1, "{stats}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_stored_row_that_depends_on_relayed_ops_doesnt_wedge_its_reader() {
    let chat = chat_id();
    let host = Peer::relay(&chat, "host", true).await;
    let viewer = Peer::relay(&chat, "viewer", false).await;
    host.caught_up().await;
    viewer.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    host.stream("relayed only ");
    converged(&[&host, &viewer]).await;
    // The viewer's message goes to stored rows and depends on what was relayed,
    // as one sent over the HTTPS fallback does.
    viewer.client.set_queue_pushes(false);
    viewer.send("stored, on top of it");
    until("the row to land", async || stats(&chat).await["rowCount"] == 1).await;
    // A reader that missed the relayed ops reads that row first.
    let late = Peer::relay(&chat, "late", false).await;
    converged(&[&host, &viewer, &late]).await;
    assert_eq!(late.body(), host.body());
    until("its cursor to walk past the row", async || late.client.stats().cursor == 1).await;
    assert_eq!(late.client.stats().disconnects, 0, "it never redialed");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_wake_counts_as_read_only_once_the_queue_has_been() {
    let chat = chat_id();
    let phone = Peer::relay(&chat, "phone", false).await;
    phone.caught_up().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    phone.send("wake me");
    until("the queue to hold it", async || stats(&chat).await["relayQueue"] == 1).await;
    let host = Peer::relay(&chat, "host", true).await;
    // Reporting caught up means the message is in the doc.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if host.client.caught_up() {
            assert_eq!(host.body(), "wake me", "caught up before reading the queue");
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
