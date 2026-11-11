//! Convergence of the relay layer with real Loro docs, through a room that
//! follows the DO's relay rules (`edge/src/chat-room.ts`, tested against real
//! workerd in `chat-relay.workerd.test.ts`) and can lose or reorder anything.
//! Virtual clock: a five-minute scenario runs in milliseconds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{RelayEvent, RelayHandle, RelayImport, RelayPolicy};
use crate::chat_client::{ChatDocSink, RowImportOutcome};
use crate::chat_frames::{self as wire, frame_type};

// ── a doc behind the sink ───────────────────────────────────────────────────

struct DocSink {
    doc: LoroDoc,
    durable: AtomicBool,
    exports: AtomicUsize,
}

impl DocSink {
    fn new(doc: LoroDoc) -> Arc<Self> {
        Arc::new(Self {
            doc,
            durable: AtomicBool::new(true),
            exports: AtomicUsize::new(0),
        })
    }
}

impl ChatDocSink for DocSink {
    fn apply_row(&self, _bytes: &[u8], _cursor: u64) -> RowImportOutcome {
        unreachable!("rows aren't part of these scenarios")
    }
    fn apply_checkpoint(&self, _bytes: &[u8], _cursor: u64) -> Result<(), String> {
        unreachable!("checkpoints aren't part of these scenarios")
    }
    fn contains_frontier(&self, _frontier: &[u8]) -> bool {
        true
    }
    fn advance_cursor(&self, _cursor: u64) {}
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
        self.exports.fetch_add(1, Ordering::SeqCst);
        let vv = VersionVector::decode(since).ok()?;
        if !self.relay_behind(since) {
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
    fn relay_durable(&self) -> bool {
        self.durable.load(Ordering::SeqCst)
    }
}

// ── a room with faults ──────────────────────────────────────────────────────

#[derive(Default)]
struct RoomState {
    sockets: HashMap<String, RelayHandle>,
    queue: Vec<(String, String, Vec<u8>)>,
    /// Drop this many of the next `relay` frames (the room's copy only).
    drop_relay: usize,
    /// Drop every `syncData` frame while set.
    drop_sync: bool,
    /// Let this many more `syncData` frames through, then drop the rest.
    sync_budget: Option<usize>,
    /// Hold each window of a transfer back this long, as a thin link would.
    slow_sync: Option<Duration>,
    /// Hold `relay` frames and release them in reverse.
    reorder: bool,
    held: Vec<(String, wire::WireFrame)>,
    relayed_frames: usize,
    delivered: Vec<String>,
}

#[derive(Clone, Default)]
struct Room(Arc<Mutex<RoomState>>);

impl Room {
    /// Connect a device. Its frames run the DO's rules until it `leave`s.
    fn join(&self, device: &str, relay: &RelayHandle) -> CancellationToken {
        self.0.lock().unwrap().sockets.insert(device.to_string(), relay.clone());
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(super::PATH_QUEUE);
        relay.room_up(tx);
        let room = self.clone();
        let device = device.to_string();
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stopped.cancelled() => break,
                    frame = rx.recv() => match frame {
                        Some(bytes) => room.handle(&device, wire::decode(&bytes).expect("frame")),
                        None => break,
                    },
                }
            }
            room.0.lock().unwrap().sockets.remove(&device);
        });
        stop
    }

    fn handle(&self, from: &str, frame: wire::WireFrame) {
        let mut room = self.0.lock().unwrap();
        match frame.kind {
            frame_type::RELAY | frame_type::ANNOUNCE => {
                if frame.kind == frame_type::RELAY {
                    room.relayed_frames += 1;
                    if room.drop_relay > 0 {
                        room.drop_relay -= 1;
                        return;
                    }
                    if room.reorder {
                        room.held.push((from.to_string(), frame));
                        return;
                    }
                }
                let skip: Vec<String> = frame.header["skip"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let mut header = frame.header.clone();
                header.as_object_mut().unwrap().remove("skip");
                header["device"] = json!(from);
                for (device, peer) in &room.sockets {
                    if device != from && !skip.contains(device) {
                        peer.room_frame(wire::WireFrame {
                            kind: frame.kind,
                            header: header.clone(),
                            payload: frame.payload.clone(),
                        });
                    }
                }
            }
            frame_type::SYNC_REQ | frame_type::SYNC_DATA => {
                if frame.kind == frame_type::SYNC_DATA && room.drop_sync {
                    return;
                }
                if frame.kind == frame_type::SYNC_DATA
                    && let Some(budget) = room.sync_budget.as_mut()
                {
                    if *budget == 0 {
                        return;
                    }
                    *budget -= 1;
                }
                let to = frame.header["to"].as_str().expect("directed").to_string();
                let mut header = frame.header.clone();
                header.as_object_mut().unwrap().remove("to");
                header["device"] = json!(from);
                if let Some(peer) = room.sockets.get(&to) {
                    let forwarded = wire::WireFrame {
                        kind: frame.kind,
                        header,
                        payload: frame.payload,
                    };
                    match room.slow_sync.filter(|_| frame.kind == frame_type::SYNC_REQ) {
                        Some(delay) => {
                            let peer = peer.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(delay).await;
                                peer.room_frame(forwarded);
                            });
                        }
                        None => peer.room_frame(forwarded),
                    }
                }
            }
            frame_type::QUEUE_REQ => {
                let peer = room.sockets.get(from).expect("joined");
                for (device, batch, bytes) in &room.queue {
                    peer.room_frame(wire::WireFrame {
                        kind: frame_type::RELAY,
                        header: json!({ "device": device, "queued": true, "batchId": batch }),
                        payload: bytes.clone(),
                    });
                }
                peer.room_frame(wire::WireFrame {
                    kind: frame_type::QUEUE_DONE,
                    header: json!({ "count": room.queue.len() }),
                    payload: Vec::new(),
                });
            }
            frame_type::DELIVERED => {
                let ids: Vec<String> = frame.header["batchIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                room.queue.retain(|(device, batch, _)| device == from || !ids.contains(batch));
                room.delivered.extend(ids);
            }
            other => panic!("unexpected frame {other}"),
        }
    }

    /// What `enqueue` does, as the outbox path reaches it.
    fn enqueue(&self, from: &str, batch: &str, bytes: Vec<u8>) {
        let mut room = self.0.lock().unwrap();
        room.queue.push((from.to_string(), batch.to_string(), bytes.clone()));
        for (device, peer) in &room.sockets {
            if device != from {
                peer.room_frame(wire::WireFrame {
                    kind: frame_type::RELAY,
                    header: json!({ "device": from, "queued": true, "batchId": batch }),
                    payload: bytes.clone(),
                });
            }
        }
    }

    fn release_reversed(&self) {
        let mut room = self.0.lock().unwrap();
        room.reorder = false;
        let held = std::mem::take(&mut room.held);
        for (from, frame) in held.into_iter().rev() {
            let mut header = frame.header.clone();
            header["device"] = json!(from);
            for (device, peer) in &room.sockets {
                if *device != from {
                    peer.room_frame(wire::WireFrame {
                        kind: frame.kind,
                        header: header.clone(),
                        payload: frame.payload.clone(),
                    });
                }
            }
        }
    }

    fn queue_len(&self) -> usize {
        self.0.lock().unwrap().queue.len()
    }
}

// ── devices ─────────────────────────────────────────────────────────────────

struct Device {
    name: String,
    doc: LoroDoc,
    sink: Arc<DocSink>,
    relay: RelayHandle,
    /// Ops published so far: what a local-update hook hands out.
    sent: Mutex<VersionVector>,
}

impl Device {
    fn new(name: &str, policy: RelayPolicy) -> Self {
        let doc = LoroDoc::new();
        let sink = DocSink::new(doc.clone());
        let relay = RelayHandle::spawn(name, sink.clone(), policy);
        Self {
            name: name.to_string(),
            doc,
            sink,
            relay,
            sent: Mutex::new(VersionVector::default()),
        }
    }

    fn host(name: &str) -> Self {
        Self::new(name, RelayPolicy { confirm_queue: true })
    }

    fn viewer(name: &str) -> Self {
        Self::new(name, RelayPolicy::default())
    }

    /// Commit text, returning the update a local-update hook would publish.
    fn commit(&self, text: &str) -> Vec<u8> {
        let body = self.doc.get_text("body");
        body.insert(body.len_unicode(), text).unwrap();
        self.doc.commit();
        let mut sent = self.sent.lock().unwrap();
        let bytes = self.doc.export(ExportMode::updates(&sent)).unwrap();
        *sent = self.doc.oplog_vv();
        bytes
    }

    /// A host's commit: the relay layer alone carries it.
    fn stream(&self, text: &str) {
        let bytes = self.commit(text);
        self.relay.local_update(bytes, true);
    }

    fn body(&self) -> String {
        self.doc.get_text("body").to_string()
    }
}

async fn converged(devices: &[&Device]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let first = devices[0].doc.oplog_vv();
        if devices.iter().all(|d| d.doc.oplog_vv() == first) {
            let body = devices[0].body();
            assert!(devices.iter().all(|d| d.body() == body), "same ops, different text");
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never converged: {:?}",
            devices.iter().map(|d| (d.name.clone(), d.body().len())).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "never happened: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ── scenarios ───────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn two_devices_follow_a_streaming_host_with_nothing_stored() {
    let room = Room::default();
    let host = Device::host("host");
    let a = Device::viewer("a");
    let b = Device::viewer("b");
    let _sockets = (room.join("host", &host.relay), room.join("a", &a.relay), room.join("b", &b.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    for i in 0..200 {
        host.stream(&format!("chunk {i} "));
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    converged(&[&host, &a, &b]).await;
    assert!(a.body().contains("chunk 199"));
    assert_eq!(room.queue_len(), 0, "the host's stream is never stored");
    assert_eq!(a.relay.stats().sync_rounds, 0, "a clean stream needs no catch-up");
}

#[tokio::test(start_paused = true)]
async fn a_late_joiner_catches_up_from_the_host_alone() {
    let room = Room::default();
    let host = Device::host("host");
    let _host_socket = room.join("host", &host.relay);
    for i in 0..500 {
        host.stream(&format!("before the join {i} "));
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let late = Device::viewer("late");
    let _late_socket = room.join("late", &late.relay);
    converged(&[&host, &late]).await;
    assert!(late.body().contains("before the join 499"));
    // And it stays live afterward.
    host.stream("after the join");
    converged(&[&host, &late]).await;
    assert!(late.body().ends_with("after the join"));
}

#[tokio::test(start_paused = true)]
async fn a_chat_larger_than_several_windows_transfers_whole() {
    let room = Room::default();
    let host = Device::host("host");
    let _host_socket = room.join("host", &host.relay);
    // Incompressible enough that the export spans many windows.
    let mut text = String::new();
    for i in 0..400_000u64 {
        text.push_str(&format!("{:x}", i.wrapping_mul(0x9E3779B97F4A7C15)));
    }
    assert!(text.len() > 2 * super::SYNC_WINDOW);
    host.commit(&text);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let late = Device::viewer("late");
    let _late_socket = room.join("late", &late.relay);
    converged(&[&host, &late]).await;
    assert_eq!(late.body().len(), text.len());
}

#[tokio::test(start_paused = true)]
async fn a_message_to_an_offline_host_waits_and_leaves_only_once_saved() {
    let room = Room::default();
    let phone = Device::viewer("phone");
    let other = Device::viewer("other");
    let _phone_socket = room.join("phone", &phone.relay);
    let _other_socket = room.join("other", &other.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    room.enqueue("phone", "m1", phone.commit("sent while the host slept"));
    // The other device sees it without the host.
    converged(&[&phone, &other]).await;
    assert_eq!(room.queue_len(), 1);

    let host = Device::host("host");
    host.sink.durable.store(false, Ordering::SeqCst);
    let _host_socket = room.join("host", &host.relay);
    until("the host to import it", || host.body() == "sent while the host slept").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(room.queue_len(), 1, "not deleted until the host has saved it");

    host.sink.durable.store(true, Ordering::SeqCst);
    until("the queue to empty", || room.queue_len() == 0).await;

    // A device arriving now gets it from the host. The edge has nothing left.
    let late = Device::viewer("late");
    let _late_socket = room.join("late", &late.relay);
    converged(&[&host, &late]).await;
    assert_eq!(late.body(), "sent while the host slept");
}

#[tokio::test(start_paused = true)]
async fn a_viewer_never_confirms_the_queue() {
    let room = Room::default();
    let phone = Device::viewer("phone");
    let other = Device::viewer("other");
    let _sockets = (room.join("phone", &phone.relay), room.join("other", &other.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    room.enqueue("phone", "m1", phone.commit("for the host"));
    converged(&[&phone, &other]).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(room.queue_len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_host_going_offline_mid_stream_loses_nothing_on_its_return() {
    let room = Room::default();
    let host = Device::host("host");
    let viewer = Device::viewer("viewer");
    let host_socket = room.join("host", &host.relay);
    let _viewer_socket = room.join("viewer", &viewer.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    host.stream("seen ");
    converged(&[&host, &viewer]).await;
    host_socket.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Host keeps working unreachable; viewer keeps sending.
    host.stream("written offline ");
    room.enqueue("viewer", "m1", viewer.commit("sent meanwhile "));
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(!viewer.body().contains("written offline"), "the viewer only sees what it synced");
    let _back = room.join("host", &host.relay);
    converged(&[&host, &viewer]).await;
    assert!(viewer.body().contains("written offline"));
    assert!(host.body().contains("sent meanwhile"));
    until("the queue to empty", || room.queue_len() == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_viewer_reconnecting_recovers_what_it_missed() {
    let room = Room::default();
    let host = Device::host("host");
    let viewer = Device::viewer("viewer");
    let _host_socket = room.join("host", &host.relay);
    let socket = room.join("viewer", &viewer.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    host.stream("one ");
    converged(&[&host, &viewer]).await;
    socket.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    for i in 0..50 {
        host.stream(&format!("missed {i} "));
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    let _back = room.join("viewer", &viewer.relay);
    converged(&[&host, &viewer]).await;
    assert!(viewer.body().contains("missed 49"));
}

#[tokio::test(start_paused = true)]
async fn reordered_delivery_converges() {
    let room = Room::default();
    let host = Device::host("host");
    let viewer = Device::viewer("viewer");
    let _sockets = (room.join("host", &host.relay), room.join("viewer", &viewer.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    room.0.lock().unwrap().reorder = true;
    for i in 0..30 {
        host.stream(&format!("{i} "));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    room.release_reversed();
    converged(&[&host, &viewer]).await;
    assert_eq!(viewer.body(), (0..30).map(|i| format!("{i} ")).collect::<String>());
}

#[tokio::test(start_paused = true)]
async fn a_lost_tail_is_recovered_within_seconds() {
    let room = Room::default();
    let host = Device::host("host");
    let viewer = Device::viewer("viewer");
    let _sockets = (room.join("host", &host.relay), room.join("viewer", &viewer.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    host.stream("kept ");
    converged(&[&host, &viewer]).await;
    // The end of the turn is lost, and nothing follows it.
    room.0.lock().unwrap().drop_relay = 3;
    for tail in ["lost a ", "lost b ", "lost c"] {
        host.stream(tail);
    }
    let started = tokio::time::Instant::now();
    converged(&[&host, &viewer]).await;
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    assert!(viewer.body().ends_with("lost c"));
}

#[tokio::test(start_paused = true)]
async fn heavy_loss_still_converges() {
    let room = Room::default();
    let host = Device::host("host");
    let a = Device::viewer("a");
    let b = Device::viewer("b");
    let _sockets = (room.join("host", &host.relay), room.join("a", &a.relay), room.join("b", &b.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    for i in 0..300 {
        if i % 3 != 0 {
            room.0.lock().unwrap().drop_relay = 1;
        }
        host.stream(&format!("{i} "));
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    converged(&[&host, &a, &b]).await;
    assert!(a.body().contains("299 "));
}

#[tokio::test(start_paused = true)]
async fn a_sync_that_stalls_is_retried() {
    let room = Room::default();
    let host = Device::host("host");
    let _host_socket = room.join("host", &host.relay);
    host.stream("history");
    tokio::time::sleep(Duration::from_secs(5)).await;
    room.0.lock().unwrap().drop_sync = true;
    let late = Device::viewer("late");
    let _late_socket = room.join("late", &late.relay);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(late.body(), "");
    room.0.lock().unwrap().drop_sync = false;
    converged(&[&host, &late]).await;
    assert!(late.relay.stats().sync_rounds >= 2);
}

#[tokio::test(start_paused = true)]
async fn a_host_that_lost_ops_gets_them_back_from_a_viewer() {
    let room = Room::default();
    let host = Device::host("host");
    let viewer = Device::viewer("viewer");
    let _sockets = (room.join("host", &host.relay), room.join("viewer", &viewer.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    host.stream("the only copy is about to be on the viewer");
    converged(&[&host, &viewer]).await;
    // The host restarts from a snapshot older than these ops.
    let restarted = Device::host("host");
    let _socket = room.join("host", &restarted.relay);
    converged(&[&restarted, &viewer]).await;
    assert_eq!(restarted.body(), "the only copy is about to be on the viewer");
}

#[tokio::test(start_paused = true)]
async fn asking_a_peer_that_cannot_help_does_not_spin() {
    let room = Room::default();
    let viewer = Device::viewer("viewer");
    let _socket = room.join("viewer", &viewer.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // A peer claims ops it never hands over: it answers with nothing.
    let claimant = LoroDoc::new();
    claimant.get_text("body").insert(0, "never sent").unwrap();
    claimant.commit();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    {
        let relay = viewer.relay.clone();
        tokio::spawn(async move { while rx.recv().await.is_some() { let _ = &relay; } });
    }
    drop(tx);
    let announce = |payload: Vec<u8>| wire::WireFrame {
        kind: frame_type::ANNOUNCE,
        header: json!({ "device": "liar" }),
        payload,
    };
    viewer.relay.room_frame(announce(claimant.oplog_vv().encode()));
    for _ in 0..600 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        viewer.relay.room_frame(wire::WireFrame {
            kind: frame_type::SYNC_DATA,
            header: json!({ "device": "liar", "xfer": "", "offset": 0, "total": 0 }),
            payload: Vec::new(),
        });
    }
    let rounds = viewer.relay.stats().sync_rounds;
    assert!(rounds <= super::MAX_FRUITLESS_ROUNDS as u64, "rounds = {rounds}");
}

#[tokio::test(start_paused = true)]
async fn a_queue_row_that_parks_is_held_until_its_history_arrives() {
    let room = Room::default();
    let host = Device::host("host");
    let phone = Device::viewer("phone");
    let _sockets = (room.join("host", &host.relay), room.join("phone", &phone.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Two messages. The room's copy of the first is delayed past the second.
    let first = phone.commit("first ");
    let second = phone.commit("second");
    room.enqueue("phone", "m2", second);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.body(), "");
    assert_eq!(room.queue_len(), 1, "m2 parked on m1: it mustn't be confirmed");
    // Version exchange fills the gap from the phone, and the row is confirmed
    // only when it's re-read and imports cleanly.
    converged(&[&host, &phone]).await;
    assert_eq!(host.body(), "first second");
    until("m2 to be confirmed", || room.queue_len() == 0).await;
    room.enqueue("phone", "m1", first);
    until("m1 to be confirmed", || room.queue_len() == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_legacy_peer_reaches_the_engine() {
    let host = Device::host("host");
    let mut events = host.relay.events();
    host.relay.room_frame(wire::WireFrame {
        kind: frame_type::LEGACY_PEER,
        header: json!({ "device": "old" }),
        payload: Vec::new(),
    });
    assert_eq!(events.recv().await.unwrap(), RelayEvent::LegacyPeer);
}

#[tokio::test(start_paused = true)]
async fn a_slow_transfer_outlasts_its_exports_ttl() {
    // Every window takes longer than the export's lifetime to arrive, as a thin
    // link's would. A request in time keeps it alive.
    let room = Room::default();
    let host = Device::host("host");
    let _host_socket = room.join("host", &host.relay);
    let mut text = String::new();
    for i in 0..400_000u64 {
        text.push_str(&format!("{:x}", i.wrapping_mul(0x9E3779B97F4A7C15)));
    }
    host.commit(&text);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let late = Device::viewer("late");
    room.0.lock().unwrap().slow_sync = Some(Duration::from_secs(45));
    let _late_socket = room.join("late", &late.relay);
    converged(&[&host, &late]).await;
    assert_eq!(late.body().len(), text.len());
    assert_eq!(late.relay.stats().sync_rounds, 1, "one transfer, start to finish");
    assert_eq!(host.sink.exports.load(Ordering::SeqCst), 1, "exported once");
}

#[tokio::test(start_paused = true)]
async fn a_round_with_a_peer_that_left_gives_way_to_one_that_is_here() {
    let room = Room::default();
    let viewer = Device::viewer("viewer");
    let helper = Device::viewer("helper");
    helper.commit("only the helper has this");
    let _sockets = (room.join("viewer", &viewer.relay), room.join("helper", &helper.relay));
    tokio::time::sleep(Duration::from_millis(50)).await;
    // A peer announces more, then never answers.
    let gone = LoroDoc::new();
    gone.get_text("other").insert(0, "never sent").unwrap();
    gone.commit();
    viewer.relay.room_frame(wire::WireFrame {
        kind: frame_type::ANNOUNCE,
        header: json!({ "device": "gone", "hello": true }),
        payload: gone.oplog_vv().encode(),
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    helper.relay.ask_peers();
    let started = tokio::time::Instant::now();
    until("the helper's ops to arrive", || viewer.body() == "only the helper has this").await;
    assert!(started.elapsed() < Duration::from_secs(90), "took {:?}", started.elapsed());
}

#[tokio::test(start_paused = true)]
async fn a_round_asked_of_a_host_that_wasnt_there_restarts_when_it_joins() {
    // A phone back from the background asks the host, whose socket had closed. The
    // ask goes nowhere. The host's join must start a new round, not wait for a stall.
    let room = Room::default();
    let host = Device::host("host");
    host.commit("written while the phone was away");
    let phone = Device::viewer("phone");
    let _phone_socket = room.join("phone", &phone.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    phone.relay.room_frame(wire::WireFrame {
        kind: frame_type::ANNOUNCE,
        header: json!({ "device": "host" }),
        payload: host.doc.oplog_vv().encode(),
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(phone.body(), "", "the host isn't there yet");
    let joined = tokio::time::Instant::now();
    let _host_socket = room.join("host", &host.relay);
    converged(&[&host, &phone]).await;
    let took = joined.elapsed();
    assert!(took < Duration::from_secs(5), "took {took:?}");
}

#[tokio::test(start_paused = true)]
async fn a_transfer_cut_off_by_a_reconnecting_host_resumes_when_it_rejoins() {
    let room = Room::default();
    let host = Device::host("host");
    let mut text = String::new();
    for i in 0..300_000u64 {
        text.push_str(&format!("{:x}", i.wrapping_mul(0x9E3779B97F4A7C15)));
    }
    host.commit(&text);
    let host_socket = room.join("host", &host.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The first few fragments arrive, then the host's socket drops what's in flight.
    room.0.lock().unwrap().sync_budget = Some(3);
    let phone = Device::viewer("phone");
    let _phone_socket = room.join("phone", &phone.relay);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(phone.body().is_empty(), "the transfer is cut off part way");
    room.0.lock().unwrap().sync_budget = None;
    host_socket.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rejoined = tokio::time::Instant::now();
    let _host_again = room.join("host", &host.relay);
    converged(&[&host, &phone]).await;
    let took = rejoined.elapsed();
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

#[tokio::test(start_paused = true)]
async fn a_queue_read_the_room_had_no_room_for_goes_out_later() {
    let host = Device::host("host");
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1);
    tx.try_send(Vec::new()).unwrap(); // the room's channel is full
    host.relay.room_up(tx);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!host.relay.queue_settled());
    let asked = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(bytes) = rx.recv().await {
            if wire::decode(&bytes).is_some_and(|frame| frame.kind == frame_type::QUEUE_REQ) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(asked, Ok(true), "the dropped queue read was never asked again");
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_keeps_stalling_stops_being_asked() {
    let room = Room::default();
    let viewer = Device::viewer("viewer");
    let _socket = room.join("viewer", &viewer.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let gone = LoroDoc::new();
    gone.get_text("body").insert(0, "never sent").unwrap();
    gone.commit();
    viewer.relay.room_frame(wire::WireFrame {
        kind: frame_type::ANNOUNCE,
        header: json!({ "device": "gone" }),
        payload: gone.oplog_vv().encode(),
    });
    tokio::time::sleep(Duration::from_secs(3600)).await;
    let rounds = viewer.relay.stats().sync_rounds;
    assert!(rounds <= super::MAX_STALLS as u64, "rounds = {rounds}");
}

#[tokio::test(start_paused = true)]
async fn queue_reads_settle_only_when_answered() {
    let room = Room::default();
    let host = Device::host("host");
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(super::PATH_QUEUE);
    host.relay.room_up(tx);
    assert!(!host.relay.queue_settled(), "asked, not answered");
    let first = rx.recv().await.unwrap();
    assert_eq!(wire::decode(&first).unwrap().kind, frame_type::QUEUE_REQ);
    host.relay.room_frame(wire::WireFrame {
        kind: frame_type::QUEUE_DONE,
        header: json!({ "count": 0 }),
        payload: Vec::new(),
    });
    until("the answer to land", || host.relay.queue_settled()).await;
    drop(room);
}

#[tokio::test(start_paused = true)]
async fn a_queue_row_this_build_cant_read_is_never_confirmed() {
    let room = Room::default();
    let host = Device::host("host");
    let _socket = room.join("host", &host.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    room.enqueue("phone", "unreadable", vec![0xff; 32]);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(room.queue_len(), 1, "kept: a newer build may read it");
}

#[tokio::test(start_paused = true)]
async fn becoming_the_host_reads_the_queue_again_and_confirms_it() {
    let room = Room::default();
    let device = Device::viewer("new-host");
    let _socket = room.join("new-host", &device.relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let author = LoroDoc::new();
    author.get_text("t").insert(0, "queued").unwrap();
    author.commit();
    room.enqueue("phone", "m1", author.export(loro::ExportMode::all_updates()).unwrap());
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(room.queue_len(), 1, "it doesn't host the chat yet");
    device.relay.set_policy(RelayPolicy { confirm_queue: true });
    until("the queue to empty", || room.queue_len() == 0).await;
}

#[tokio::test(start_paused = true)]
async fn a_read_the_task_decided_on_is_counted() {
    let host = Device::host("host");
    let (tx, mut rx) = mpsc::channel(16);
    host.relay.room_up(tx);
    let _ = rx.recv().await.unwrap();
    host.relay.room_frame(wire::WireFrame {
        kind: frame_type::QUEUE_DONE,
        header: json!({ "count": 0 }),
        payload: Vec::new(),
    });
    until("the first answer", || host.relay.queue_settled()).await;
    host.relay.set_policy(RelayPolicy { confirm_queue: false });
    host.relay.set_policy(RelayPolicy { confirm_queue: true });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!host.relay.queue_settled(), "the read it just asked for isn't answered");
}


#[tokio::test(start_paused = true)]
async fn joining_the_room_counts_as_recent_activity() {
    // A host woken for a reader joins after that reader's announce went by. Until
    // the reader hears the host and asks, nothing arrives, and a host that read
    // that as idle left before the question came.
    let relay = RelayHandle::spawn("host", DocSink::new(LoroDoc::new()), RelayPolicy { confirm_queue: true });
    assert!(!relay.recently_active(Duration::from_secs(15)), "nothing has happened yet");
    let (tx, _rx) = mpsc::channel(super::PATH_QUEUE);
    relay.room_up(tx);
    assert!(relay.recently_active(Duration::from_secs(15)), "it has just joined");
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(!relay.recently_active(Duration::from_millis(5)), "the join is a while ago");
}

#[tokio::test(start_paused = true)]
async fn frames_a_full_room_queue_cant_take_yet_go_later_in_order() {
    // A slow socket fills the path to the room. A frame skipped then parks every
    // later one on the peers until a sync round; a skipped sync frame left the
    // round waiting a minute.
    let relay = RelayHandle::spawn("host", DocSink::new(LoroDoc::new()), RelayPolicy { confirm_queue: true });
    let (tx, mut rx) = mpsc::channel(2);
    relay.room_up(tx);
    for i in 0..10u8 {
        relay.local_update(vec![i; 4], true);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut relayed = Vec::new();
    while relayed.len() < 10 {
        let bytes = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("every held frame is offered again")
            .expect("the room is open");
        let frame = wire::decode(&bytes).expect("a frame");
        if frame.kind == frame_type::RELAY {
            relayed.push(frame.payload[0]);
        }
    }
    assert_eq!(relayed, (0..10u8).collect::<Vec<_>>(), "all of them, in order");
    assert_eq!(relay.stats().room_frames_dropped, 0);
}

#[tokio::test(start_paused = true)]
async fn frames_for_a_room_that_has_gone_are_counted() {
    let relay = RelayHandle::spawn("host", DocSink::new(LoroDoc::new()), RelayPolicy { confirm_queue: true });
    let (tx, rx) = mpsc::channel(super::PATH_QUEUE);
    relay.room_up(tx);
    drop(rx);
    for i in 0..3u8 {
        relay.local_update(vec![i; 4], true);
    }
    until("the drops to be counted", || relay.stats().room_frames_dropped >= 3).await;
}
