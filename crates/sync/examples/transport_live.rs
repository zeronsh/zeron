//! Production ChatClient + Loro + SQLite against an explicit test edge.
//! Run through scripts/transport-proxy.py for controlled network conditions.
//! Usage: transport_live <proxy-base> <cloudflare-origin> <stream|outage|http|catchup|upload>
use futures::future::BoxFuture;
use loro::{ExportMode, LoroDoc, VersionVector};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering::Relaxed},
};
use std::time::{Duration, Instant};
use zeron_sync::chat_client::{ChatTransport, RowImportOutcome};
use zeron_sync::{ChatClient, ChatDocSink, CheckpointFetcher, DocsStore, StaticUrl, SyncError};

struct Sink {
    doc: Mutex<LoroDoc>,
    store: DocsStore,
    cursor: AtomicU64,
    arrivals: Mutex<Vec<(u64, Instant)>>,
}
impl Sink {
    fn new(dir: &std::path::Path) -> Self {
        let store = DocsStore::open(dir).unwrap();
        let doc = LoroDoc::new();
        if let Some(bytes) = store.load_snapshot("live").unwrap() {
            doc.import(&bytes).unwrap();
        }
        Self {
            doc: Mutex::new(doc),
            store,
            cursor: AtomicU64::new(0),
            arrivals: Mutex::new(vec![]),
        }
    }
    fn append(&self, text: &str) -> Vec<u8> {
        let doc = self.doc.lock().unwrap();
        let before = doc.oplog_vv();
        let t = doc.get_text("text");
        t.insert(t.len_unicode(), text).unwrap();
        doc.commit();
        self.store
            .save_snapshot("live", &doc.export(ExportMode::Snapshot).unwrap())
            .unwrap();
        doc.export(ExportMode::updates(&before)).unwrap()
    }
    fn text(&self) -> String {
        self.doc.lock().unwrap().get_text("text").to_string()
    }
}
impl ChatDocSink for Sink {
    fn pending_updates(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.store
            .pending_chat_updates("live")
            .map_err(|e| e.to_string())
    }
    fn persist_update(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.store
            .enqueue_chat_update("live", id, bytes)
            .map_err(|e| e.to_string())
    }
    fn acknowledge_update(&self, id: &str) -> Result<(), String> {
        self.store
            .acknowledge_chat_update("live", id)
            .map_err(|e| e.to_string())
    }
    fn apply_row(&self, bytes: &[u8], cursor: u64) -> RowImportOutcome {
        let doc = self.doc.lock().unwrap();
        if doc.import(bytes).unwrap().pending.is_some() {
            return RowImportOutcome::PendingDependencies;
        }
        self.store
            .save_snapshot_with_cursor(
                "live",
                &doc.export(ExportMode::Snapshot).unwrap(),
                cursor,
                0,
            )
            .unwrap();
        self.cursor.store(cursor, Relaxed);
        self.arrivals.lock().unwrap().push((cursor, Instant::now()));
        RowImportOutcome::Applied
    }
    fn apply_checkpoint(&self, bytes: &[u8], cursor: u64) -> Result<(), String> {
        self.apply_row(bytes, cursor);
        Ok(())
    }
    fn contains_frontier(&self, bytes: &[u8]) -> bool {
        VersionVector::decode(bytes)
            .is_ok_and(|v| self.doc.lock().unwrap().oplog_vv().includes_vv(&v))
    }
    fn advance_cursor(&self, cursor: u64) {
        self.cursor.store(cursor, Relaxed);
    }
}

#[derive(Clone)]
struct Http {
    client: reqwest::Client,
    room: String,
    token: String,
}
impl Http {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, SyncError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.room))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| SyncError::WebSocket(e.to_string()))?;
        let response = response
            .error_for_status()
            .map_err(|e| SyncError::WebSocket(e.to_string()))?;
        Ok(response
            .bytes()
            .await
            .map_err(|e| SyncError::WebSocket(e.to_string()))?
            .to_vec())
    }
}
impl ChatTransport for Http {
    fn fetch_rows(&self, after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let this = self.clone();
        Box::pin(async move {
            this.request(reqwest::Method::GET, &format!("/rows?after={after}"), None)
                .await
        })
    }
    fn push(&self, id: String, bytes: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>> {
        let this = self.clone();
        Box::pin(async move {
            let bytes = this
                .request(
                    reqwest::Method::POST,
                    &format!("/rows?device=sender&batchId={id}"),
                    Some(bytes),
                )
                .await?;
            String::from_utf8(bytes).map_err(|e| SyncError::Protocol(e.to_string()))
        })
    }
}
impl CheckpointFetcher for Http {
    fn fetch(&self) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let this = self.clone();
        Box::pin(async move {
            this.request(reqwest::Method::GET, "/checkpoint", None)
                .await
        })
    }
}
async fn until(mut check: impl FnMut() -> bool, seconds: u64) {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("delivery deadline exceeded");
}
async fn connect(
    base: &str,
    chat: &str,
    token: &str,
    device: &str,
    sink: Arc<Sink>,
    http: reqwest::Client,
    fallback: bool,
) -> ChatClient {
    let transport = Arc::new(Http {
        client: http,
        room: format!("{base}/chat2/{chat}"),
        token: token.into(),
    });
    let ws = format!(
        "{}/chat2/{chat}/ws?token={token}&device={device}",
        base.replacen("http", "ws", 1)
    );
    if fallback {
        ChatClient::connect_via_transport(
            Arc::new(StaticUrl(ws)),
            sink,
            transport.clone(),
            device,
            0,
            transport,
        )
        .await
        .unwrap()
    } else {
        ChatClient::connect(&ws, sink, transport, device, 0)
            .await
            .unwrap()
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (base, origin, mode) = (&args[1], &args[2], args[3].as_str());
    assert!(
        origin.contains("zeron-transport-385-20260915"),
        "only the isolated test worker is allowed"
    );
    let chat = format!("net-{}", uuid::Uuid::new_v4().simple());
    let token = format!("test-{}", uuid::Uuid::new_v4().simple());
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = Arc::new(Sink::new(&dir.path().join("source")));
    let receiver = Arc::new(Sink::new(&dir.path().join("receiver")));
    let mut expected = String::new();
    let mut update_bytes = 0;
    let started = Instant::now();
    if mode == "catchup" || mode == "upload" {
        let mut state = 271828u64;
        for _ in 0..128 * 1024 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            expected.push((b' ' + (state % 95) as u8) as char);
        }
        let update = source.append(&expected);
        update_bytes = update.len();
        let direct = Http {
            client: http.clone(),
            room: format!("{origin}/chat2/{chat}"),
            token: token.clone(),
        };
        if mode == "catchup" {
            direct.push("seed".into(), update).await.unwrap();
            let client = connect(
                base,
                &chat,
                &token,
                "reader",
                receiver.clone(),
                http.clone(),
                false,
            )
            .await;
            assert_eq!(receiver.text(), expected);
            println!(
                "RESULT {}",
                serde_json::json!({"mode":mode,"text_bytes":expected.len(),"update_bytes":update_bytes,"elapsed_s":started.elapsed().as_secs_f64(),"receiver_disconnects":client.stats().disconnects,"cursor":client.stats().cursor})
            );
            client.shutdown().await;
        } else {
            let sender = connect(
                base,
                &chat,
                &token,
                "sender",
                source.clone(),
                http.clone(),
                false,
            )
            .await;
            let reader = connect(
                base,
                &chat,
                &token,
                "reader",
                receiver.clone(),
                http.clone(),
                false,
            )
            .await;
            sender.enqueue_batch("large-update".into(), update);
            until(
                || receiver.text() == expected && sender.stats().pending_pushes == 0,
                360,
            )
            .await;
            println!(
                "RESULT {}",
                serde_json::json!({"mode":mode,"text_bytes":expected.len(),"update_bytes":update_bytes,"elapsed_s":started.elapsed().as_secs_f64(),"sender_disconnects":sender.stats().disconnects,"receiver_disconnects":reader.stats().disconnects,"pending":sender.stats().pending_pushes})
            );
            sender.shutdown().await;
            reader.shutdown().await;
        }
    } else {
        let sender = connect(
            base,
            &chat,
            &token,
            "sender",
            source.clone(),
            http.clone(),
            true,
        )
        .await;
        let reader = connect(
            base,
            &chat,
            &token,
            "reader",
            receiver.clone(),
            http.clone(),
            true,
        )
        .await;
        until(
            || sender.stats().server_known && reader.stats().server_known,
            45,
        )
        .await;
        if mode == "outage" {
            http.get(format!("{base}/__test__/blackout?seconds=30"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }
        let sending = Instant::now();
        let mut issued = Vec::new();
        for i in 0..20 {
            let text = format!(
                "{i:02}: {}\n",
                "Streaming a real Loro update over a constrained connection. ".repeat(4)
            );
            expected.push_str(&text);
            let bytes = source.append(&text);
            update_bytes += bytes.len();
            issued.push(Instant::now());
            sender.enqueue_batch(format!("update-{i:02}"), bytes);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        until(
            || receiver.text() == expected && sender.stats().pending_pushes == 0,
            180,
        )
        .await;
        let stats: serde_json::Value = serde_json::from_slice(
            &Http {
                client: http.clone(),
                room: format!("{origin}/chat2/{chat}"),
                token: token.clone(),
            }
            .request(reqwest::Method::GET, "/stats", None)
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(stats["headSeq"], 20);
        assert_eq!(stats["rowCount"], 20);
        assert_eq!(source.text(), receiver.text());
        assert!(source.pending_updates().unwrap().is_empty());
        let mut delays = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (seq, at) in receiver.arrivals.lock().unwrap().iter() {
            if *seq > 0 && *seq <= 20 && seen.insert(*seq) {
                delays.push(
                    at.saturating_duration_since(issued[*seq as usize - 1])
                        .as_secs_f64(),
                );
            }
        }
        delays.sort_by(f64::total_cmp);
        println!(
            "RESULT {}",
            serde_json::json!({"mode":mode,"text_bytes":expected.len(),"update_bytes":update_bytes,"elapsed_s":started.elapsed().as_secs_f64(),"delivery_s":sending.elapsed().as_secs_f64(),"p95_update_s":delays.get(delays.len()*95/100),"sender_disconnects":sender.stats().disconnects,"receiver_disconnects":reader.stats().disconnects,"pending":sender.stats().pending_pushes,"head_seq":stats["headSeq"],"row_count":stats["rowCount"],"push_outcomes":stats["pushOutcomes"]})
        );
        sender.shutdown().await;
        reader.shutdown().await;
    }
    let proxy: serde_json::Value = http
        .get(format!("{base}/__test__/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("PROXY {proxy}");
}
