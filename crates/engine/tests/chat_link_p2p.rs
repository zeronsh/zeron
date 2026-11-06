//! A chat's relay layer over a real WebRTC pair (host candidates on this machine),
//! with real Loro docs: the same streams the app's direct links use. The room
//! here is a plain channel that forwards frames, as the DO does, so the fallback
//! can be watched taking over.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use loro::{ExportMode, LoroDoc, VersionVector};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zeron_preview::mux::{BoxIo, Connector};
use zeron_preview::peer::Peers;
use zeron_sync::chat_client::{ChatDocSink, RowImportOutcome};
use zeron_sync::chat_frames::{self as wire};
use zeron_sync::chat_relay::{RelayHandle, RelayImport, RelayPolicy, run_link};

struct Sink(LoroDoc);

impl ChatDocSink for Sink {
    fn apply_row(&self, _bytes: &[u8], _cursor: u64) -> RowImportOutcome {
        unreachable!()
    }
    fn apply_checkpoint(&self, _bytes: &[u8], _cursor: u64) -> Result<(), String> {
        unreachable!()
    }
    fn contains_frontier(&self, _frontier: &[u8]) -> bool {
        true
    }
    fn advance_cursor(&self, _cursor: u64) {}
    fn relay_version(&self) -> Option<Vec<u8>> {
        Some(self.0.oplog_vv().encode())
    }
    fn relay_lacks(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !self.0.oplog_vv().includes_vv(&vv))
    }
    fn relay_behind(&self, version: &[u8]) -> bool {
        VersionVector::decode(version).is_ok_and(|vv| !vv.includes_vv(&self.0.oplog_vv()))
    }
    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let vv = VersionVector::decode(since).ok()?;
        self.0.export(ExportMode::updates(&vv)).ok()
    }
    fn relay_import(&self, bytes: &[u8], _history: bool) -> RelayImport {
        match self.0.import(bytes) {
            Ok(status) if status.pending.is_some() => RelayImport::Parked,
            Ok(_) => RelayImport::Applied,
            Err(_) => RelayImport::Unreadable,
        }
    }
}

/// The host's `chatlink/` service, reduced to one chat.
struct LinkService(RelayHandle, CancellationToken);

#[async_trait::async_trait]
impl Connector for LinkService {
    async fn connect(&self, _service: &str) -> anyhow::Result<BoxIo> {
        anyhow::bail!("links name their peer")
    }
    async fn connect_from(&self, peer: Option<&str>, _service: &str) -> anyhow::Result<BoxIo> {
        let (ours, theirs) = tokio::io::duplex(256 * 1024);
        tokio::spawn(run_link(
            ours,
            peer.expect("known peer").to_string(),
            self.0.clone(),
            self.1.clone(),
        ));
        Ok(Box::new(theirs))
    }
}

/// Frames from `from` reach `to` the way a room forwards them.
fn room_path(from: &'static str, source: &RelayHandle, target: RelayHandle) {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);
    source.room_up(tx);
    tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            let mut frame = wire::decode(&bytes).expect("frame");
            let skipped = frame.header["skip"]
                .as_array()
                .is_some_and(|skip| skip.iter().any(|d| d.as_str().is_some()));
            if skipped {
                continue;
            }
            frame.header["device"] = serde_json::json!(from);
            target.room_frame(frame);
        }
    });
}

fn device(name: &str, host: bool) -> (LoroDoc, RelayHandle) {
    let doc = LoroDoc::new();
    let relay = RelayHandle::spawn(
        name,
        Arc::new(Sink(doc.clone())),
        RelayPolicy { confirm_queue: host },
    );
    (doc, relay)
}

fn commit(doc: &LoroDoc, sent: &mut VersionVector, text: &str) -> Vec<u8> {
    let body = doc.get_text("body");
    body.insert(body.len_unicode(), text).unwrap();
    doc.commit();
    let bytes = doc.export(ExportMode::updates(sent)).unwrap();
    *sent = doc.oplog_vv();
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_streams_over_webrtc_and_falls_back_to_the_room_when_it_dies() {
    let (host_doc, host_relay) = device("host", true);
    let (viewer_doc, viewer_relay) = device("viewer", false);
    room_path("host", &host_relay, viewer_relay.clone());
    room_path("viewer", &viewer_relay, host_relay.clone());

    let stop = CancellationToken::new();
    let link_stop = CancellationToken::new();
    let service = Arc::new(LinkService(host_relay.clone(), link_stop.clone()));
    let (mut host_peers, mut host_out) = Peers::new("host".into(), service.clone(), stop.clone());
    let (mut viewer_peers, mut viewer_out) = Peers::new("viewer".into(), service, stop.clone());
    host_peers.set_ice_servers(Vec::new()).unwrap();
    viewer_peers.set_ice_servers(Vec::new()).unwrap();
    let to_viewer = viewer_peers.clone();
    tokio::spawn(async move {
        while let Some(message) = host_out.recv().await {
            let _ = to_viewer.signal("host", message.signal).await;
        }
    });
    let to_host = host_peers.clone();
    tokio::spawn(async move {
        while let Some(message) = viewer_out.recv().await {
            let _ = to_host.signal("viewer", message.signal).await;
        }
    });

    let stream = tokio::time::timeout(
        Duration::from_secs(20),
        viewer_peers.open("host", "chatlink/one", false),
    )
    .await
    .expect("pairing in time")
    .expect("direct stream");
    tokio::spawn(run_link(stream, "host".to_string(), viewer_relay.clone(), link_stop.clone()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while host_relay.stats().links == 0 || viewer_relay.stats().links == 0 {
        assert!(Instant::now() < deadline, "the link never came up");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // The stream, one commit at a time, timed to its arrival in the viewer's doc.
    let mut sent = VersionVector::default();
    let mut samples = Vec::new();
    for i in 0..200 {
        let bytes = commit(&host_doc, &mut sent, &format!("chunk {i} "));
        let started = Instant::now();
        host_relay.local_update(bytes, true);
        let deadline = started + Duration::from_secs(5);
        while viewer_doc.oplog_vv() != host_doc.oplog_vv() {
            assert!(Instant::now() < deadline, "commit {i} never arrived");
            tokio::task::yield_now().await;
        }
        samples.push(started.elapsed());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[samples.len() * 95 / 100];
    println!("commit to viewer's doc over WebRTC (loopback): p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(100), "the link is holding frames: p95 {p95:?}");
    assert!(viewer_relay.stats().direct_frames_received >= 200);

    // A message from the viewer goes the other way.
    let mut viewer_sent = VersionVector::default();
    viewer_relay.local_update(commit(&viewer_doc, &mut viewer_sent, "a steer "), false);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !host_doc.get_text("body").to_string().contains("a steer") {
        assert!(Instant::now() < deadline, "the steer never reached the host");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // The link dies. Nothing's lost, and the room carries on.
    link_stop.cancel();
    let deadline = Instant::now() + Duration::from_secs(15);
    while host_relay.stats().links > 0 {
        assert!(Instant::now() < deadline, "the link never went down");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = viewer_relay.stats().room_frames_received;
    sent = host_doc.oplog_vv();
    for i in 0..20 {
        host_relay.local_update(commit(&host_doc, &mut sent, &format!("via room {i} ")), true);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while viewer_doc.oplog_vv() != host_doc.oplog_vv() {
        assert!(Instant::now() < deadline, "the room never took over");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(viewer_doc.get_text("body").to_string().contains("via room 19"));
    assert!(viewer_relay.stats().room_frames_received > before);
    stop.cancel();
}
