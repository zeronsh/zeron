//! The relay layer's device half (docs/design/relay-layer.md).
//!
//! Delivery here is a hint, not a guarantee. A chat's ops travel over whatever
//! is up: a direct peer-to-peer link or the room's relay. Any path may lose,
//! reorder or repeat them. Loro imports are idempotent, and devices compare
//! version vectors to find and fetch what they missed, so a failure only costs
//! time. Nothing in this file is ever the only copy of anything durable: a host's
//! ops live in its doc, and another device's sends stay in its outbox until the
//! room's offline queue has acked them.
//!
//! [`RelayHandle`] owns one task per chat, with no socket of its own.
//! [`crate::chat_client::ChatClient`] feeds it the room's frames, and
//! [`run_link`] feeds it a direct link's. It keeps running while the room is
//! down, so a direct link still works through an edge outage.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::chat_client::{ChatDocSink, MAX_PUSH_BYTES};
use crate::chat_frames::{self as wire, WireFrame, frame_type};

/// Fragment and window of one sync transfer. The requester pulls each window, so
/// a thin link sets the pace; the fragment is small enough that a 2 KiB/s link
/// still shows progress inside [`SYNC_STALL`].
const SYNC_FRAGMENT: usize = 64 * 1024;
const SYNC_WINDOW: usize = 512 * 1024;
/// A transfer with no new fragment for this long has lost a frame or its peer.
const SYNC_STALL: Duration = Duration::from_secs(60);
/// A finished export kept for resume, and how many at once.
const EXPORT_TTL: Duration = Duration::from_secs(60);
const EXPORT_CAP: usize = 4;
/// Wait before asking for a gap: the frame that fills it may be in flight.
const GAP_GRACE: Duration = Duration::from_millis(750);
/// Announces after a local change. An idle chat sends nothing, so its room can
/// hibernate.
const ANNOUNCE_SOON: Duration = Duration::from_secs(2);
const ANNOUNCE_SETTLE: Duration = Duration::from_secs(30);
/// How often to re-check whether queue rows are saved and can be confirmed.
const DELIVERED_POLL: Duration = Duration::from_millis(500);
/// Sync rounds that may end with no progress before waiting for an announce.
const MAX_FRUITLESS_ROUNDS: u32 = 3;
/// Stalled rounds in a row before waiting for a peer to announce instead.
const MAX_STALLS: u32 = 3;
const MAX_DELIVERED_IDS: usize = 100;
/// Frames queued toward one path before later ones are dropped. They heal.
pub const PATH_QUEUE: usize = 256;

/// A direct link's keepalive. A dead link would otherwise swallow frames that
/// the `skip` list kept off the room, so it must be noticed quickly.
const LINK_PROBE: Duration = Duration::from_secs(4);
const LINK_DEAD: Duration = Duration::from_secs(10);
/// A link frame carries at most one sync fragment or one update.
const LINK_MAX_FRAME: usize = 2 * 1024 * 1024;

/// What a device does with the room's offline queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayPolicy {
    /// The chat's host: confirm saved queue rows, so the room can delete them.
    pub confirm_queue: bool,
}

/// What became of a relayed update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayImport {
    Applied,
    /// Held until missing history arrives. Nothing advanced.
    Parked,
    /// Bytes this build can't read, perhaps a newer format. Not retried live, but
    /// a queue row stays queued rather than be confirmed and lost.
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEvent {
    /// A reader that can't take relay frames is in the room. The host must store.
    LegacyPeer,
    /// A remote update landed.
    Applied,
    /// A direct link came up or went down.
    LinksChanged,
}

enum Input {
    Local { bytes: Vec<u8>, to_room: bool },
    RoomUp(mpsc::Sender<Vec<u8>>),
    RoomFrame(WireFrame),
    LinkUp { peer: String, id: u64, tx: mpsc::Sender<Vec<u8>> },
    /// Only ends the link with this id: an old link timing out mustn't take its
    /// replacement with it.
    LinkDown { peer: String, id: u64 },
    LinkFrame { peer: String, frame: WireFrame },
    Exported { to: String, bytes: Option<Vec<u8>> },
    Policy(RelayPolicy),
    Host(Option<String>),
    ReadQueue,
    AskPeers,
}

#[derive(Default)]
struct Counters {
    direct_frames_sent: AtomicU64,
    room_frames_sent: AtomicU64,
    direct_frames_received: AtomicU64,
    room_frames_received: AtomicU64,
    sync_rounds: AtomicU64,
    links: AtomicU64,
    /// Queue reads asked for and answered, for catch-up to wait on.
    queue_reads_asked: AtomicU64,
    queue_reads_done: AtomicU64,
    /// When a peer's frame last arrived, in ms since this task began.
    last_inbound_ms: AtomicU64,
    /// What the next hello tells the room.
    knows_relay_gen: AtomicBool,
    relaying: AtomicBool,
    /// Pushes go to the room's offline queue. Here rather than on the client, so
    /// it's already set when the client's first dial and first pull run.
    queue_pushes: AtomicBool,
    /// Answer a sync only from the chat's host: this device is on a metered link
    /// and isn't anyone else's source of truth.
    serve_host_only: AtomicBool,
    started: Option<std::time::Instant>,
}

/// What's gone over each path, for tests and rollout measurements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayStats {
    pub direct_frames_sent: u64,
    pub room_frames_sent: u64,
    pub direct_frames_received: u64,
    pub room_frames_received: u64,
    pub sync_rounds: u64,
    pub links: u64,
}

/// One chat's relay task. Dropping every clone ends it.
#[derive(Clone)]
pub struct RelayHandle {
    tx: mpsc::UnboundedSender<Input>,
    events: broadcast::Sender<RelayEvent>,
    counters: Arc<Counters>,
    device: Arc<str>,
}

impl RelayHandle {
    pub fn spawn(device: &str, sink: Arc<dyn ChatDocSink>, policy: RelayPolicy) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(64);
        let counters = Arc::new(Counters {
            started: Some(std::time::Instant::now()),
            ..Counters::default()
        });
        let driver = Driver {
            sink,
            policy,
            host: None,
            room: None,
            links: HashMap::new(),
            peers: HashMap::new(),
            want: None,
            round: None,
            fruitless: 0,
            stalls: 0,
            exporting: HashSet::new(),
            announce_soon: None,
            announce_settle: None,
            exports: HashMap::new(),
            unconfirmed: Vec::new(),
            parked_queue: false,
            events: events.clone(),
            counters: counters.clone(),
            input: tx.downgrade(),
        };
        tokio::spawn(driver.run(rx));
        Self {
            tx,
            events,
            counters,
            device: device.into(),
        }
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// A local commit. `to_room`: the host, whose ops the room relays. Another
    /// device's ops reach the room through its outbox; here they only take the
    /// direct link, for latency.
    pub fn local_update(&self, bytes: Vec<u8>, to_room: bool) {
        let _ = self.tx.send(Input::Local { bytes, to_room });
    }

    pub fn set_policy(&self, policy: RelayPolicy) {
        let _ = self.tx.send(Input::Policy(policy));
    }

    /// What this device knows about the chat, for the next hello.
    pub fn set_hello(&self, knows_relay_gen: bool, relaying: bool) {
        self.counters.knows_relay_gen.store(knows_relay_gen, Relaxed);
        self.counters.relaying.store(relaying, Relaxed);
    }

    pub(crate) fn hello(&self) -> (Option<u8>, Option<u8>) {
        (
            self.counters.knows_relay_gen.load(Relaxed).then_some(3),
            self.counters.relaying.load(Relaxed).then_some(1),
        )
    }

    /// Send pushes to the room's offline queue, when the room has one.
    pub fn set_queue_pushes(&self, queue: bool) {
        self.counters.queue_pushes.store(queue, Relaxed);
    }

    pub(crate) fn queue_pushes(&self) -> bool {
        self.counters.queue_pushes.load(Relaxed)
    }

    /// A phone: serve only the chat's host, which may be missing this device's
    /// sends. A viewer asking a phone instead could pull a whole chat up its
    /// cellular link, and the host answers that better.
    pub fn set_serve_host_only(&self, host_only: bool) {
        self.counters.serve_host_only.store(host_only, Relaxed);
    }

    /// Which peer to ask first when several have what this device lacks.
    pub fn set_host(&self, host: Option<String>) {
        let _ = self.tx.send(Input::Host(host));
    }

    /// The room offered the layer on a new session: frames for it go here.
    pub fn room_up(&self, tx: mpsc::Sender<Vec<u8>>) {
        // Any earlier read died with its socket. This session asks once.
        let done = self.counters.queue_reads_done.load(Relaxed);
        self.counters.queue_reads_asked.store(done + 1, Relaxed);
        let _ = self.tx.send(Input::RoomUp(tx));
    }

    /// Every queue read asked of this session has been answered, so a wake can't
    /// be called done with its message still unread.
    pub fn queue_settled(&self) -> bool {
        self.counters.queue_reads_done.load(Relaxed) >= self.counters.queue_reads_asked.load(Relaxed)
    }

    /// A peer's frame arrived within `window`: it may be mid-sync with this device.
    pub fn recently_active(&self, window: Duration) -> bool {
        let last = self.counters.last_inbound_ms.load(Relaxed);
        let now = self.counters.started.map_or(0, |at| at.elapsed().as_millis() as u64);
        last > 0 && now.saturating_sub(last) < window.as_millis() as u64
    }

    /// Say where this device is, so a peer holding more answers.
    pub(crate) fn ask_peers(&self) {
        let _ = self.tx.send(Input::AskPeers);
    }

    pub fn room_frame(&self, frame: WireFrame) {
        let _ = self.tx.send(Input::RoomFrame(frame));
    }

    /// Re-read the offline queue: a wake said something's waiting.
    pub(crate) fn read_queue(&self) {
        self.counters.queue_reads_asked.fetch_add(1, Relaxed);
        let _ = self.tx.send(Input::ReadQueue);
    }

    pub(crate) fn legacy_peer(&self) {
        let _ = self.events.send(RelayEvent::LegacyPeer);
    }

    pub fn events(&self) -> broadcast::Receiver<RelayEvent> {
        self.events.subscribe()
    }

    pub fn stats(&self) -> RelayStats {
        RelayStats {
            direct_frames_sent: self.counters.direct_frames_sent.load(Relaxed),
            room_frames_sent: self.counters.room_frames_sent.load(Relaxed),
            direct_frames_received: self.counters.direct_frames_received.load(Relaxed),
            room_frames_received: self.counters.room_frames_received.load(Relaxed),
            sync_rounds: self.counters.sync_rounds.load(Relaxed),
            links: self.counters.links.load(Relaxed),
        }
    }
}

/// An inbound sync transfer.
struct Round {
    peer: String,
    last_progress: Instant,
    xfer: String,
    buf: Vec<u8>,
    /// The version it started from, to tell whether the round achieved anything.
    version: Vec<u8>,
}

struct Driver {
    sink: Arc<dyn ChatDocSink>,
    policy: RelayPolicy,
    host: Option<String>,
    room: Option<mpsc::Sender<Vec<u8>>>,
    links: HashMap<String, (u64, mpsc::Sender<Vec<u8>>)>,
    /// Each peer's last announced version.
    peers: HashMap<String, Vec<u8>>,
    want: Option<(String, Instant)>,
    round: Option<Round>,
    fruitless: u32,
    /// Stalled rounds in a row, against any peer.
    stalls: u32,
    /// Peers with an export under way. One each: a repeat waits for the first.
    exporting: HashSet<String>,
    announce_soon: Option<Instant>,
    announce_settle: Option<Instant>,
    /// Finished exports kept for the next window.
    exports: HashMap<String, (Vec<u8>, Instant)>,
    /// Queue rows imported but not yet confirmed to the room.
    unconfirmed: Vec<String>,
    /// A queue row parked on missing history: re-read the queue once it's filled.
    parked_queue: bool,
    events: broadcast::Sender<RelayEvent>,
    counters: Arc<Counters>,
    input: mpsc::WeakUnboundedSender<Input>,
}

impl Driver {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Input>) {
        loop {
            let deadline = self.next_deadline();
            let input = tokio::select! {
                input = rx.recv() => match input {
                    Some(input) => Some(input),
                    None => return,
                },
                _ = sleep_until_opt(deadline) => None,
            };
            let now = Instant::now();
            if let Some(input) = input {
                self.handle(input, now);
            }
            self.tick(Instant::now());
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.want.as_ref().filter(|_| self.round.is_none()).map(|(_, at)| *at),
            self.round.as_ref().map(|r| r.last_progress + SYNC_STALL),
            self.announce_soon,
            self.announce_settle,
            (!self.unconfirmed.is_empty() && self.room_open())
                .then(|| Instant::now() + DELIVERED_POLL),
            self.exports.values().map(|(_, at)| *at + EXPORT_TTL).min(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn room_open(&self) -> bool {
        self.room.as_ref().is_some_and(|room| !room.is_closed())
    }

    fn handle(&mut self, input: Input, now: Instant) {
        match input {
            Input::Local { bytes, to_room } => self.local(bytes, to_room, now),
            Input::RoomUp(tx) => {
                self.room = Some(tx);
                // A round over the old socket won't finish, and waiting out its
                // stall would turn a brief drop into a minute's delay.
                if self.round.as_ref().is_some_and(|r| !self.links.contains_key(&r.peer)) {
                    self.round = None;
                }
                // Read what waits for us, and say where we are: whoever has more
                // answers, and whatever a dropped socket lost comes back.
                self.to_room(wire::encode(frame_type::QUEUE_REQ, &json!({}), &[]));
                self.announce(true);
            }
            Input::RoomFrame(frame) => {
                self.counters.room_frames_received.fetch_add(1, Relaxed);
                if frame.kind == frame_type::QUEUE_DONE {
                    self.counters.queue_reads_done.fetch_add(1, Relaxed);
                } else if frame.kind == frame_type::LEGACY_PEER {
                    let _ = self.events.send(RelayEvent::LegacyPeer);
                } else if let Some(from) = frame.header["device"].as_str().map(str::to_string) {
                    self.frame(from, frame, now);
                }
            }
            Input::LinkUp { peer, id, tx } => {
                self.links.insert(peer.clone(), (id, tx));
                self.counters.links.store(self.links.len() as u64, Relaxed);
                let _ = self.events.send(RelayEvent::LinksChanged);
                self.announce_to_link(&peer, true);
            }
            Input::LinkDown { peer, id } => {
                if self.links.get(&peer).is_none_or(|(current, _)| *current != id) {
                    return;
                }
                self.links.remove(&peer);
                self.counters.links.store(self.links.len() as u64, Relaxed);
                let _ = self.events.send(RelayEvent::LinksChanged);
                // A round over that link won't finish. Waiting out its stall would
                // turn a dead link into a minute's delay.
                if self.round.as_ref().is_some_and(|r| r.peer == peer) {
                    self.round = None;
                    self.want = Some((peer, now));
                }
                // Frames that went down the dead link were kept off the room.
                self.announce(true);
            }
            Input::LinkFrame { peer, frame } => {
                self.counters.direct_frames_received.fetch_add(1, Relaxed);
                self.frame(peer, frame, now);
            }
            Input::Exported { to, bytes } => {
                self.exporting.remove(&to);
                self.exported(to, bytes);
            }
            Input::Policy(policy) => {
                // Now the host: what's queued was read without confirming it.
                let became_host = policy.confirm_queue && !self.policy.confirm_queue;
                self.policy = policy;
                if became_host && self.room_open() {
                    self.read_queue_again();
                }
            }
            Input::Host(host) => self.host = host,
            Input::ReadQueue => self.to_room(wire::encode(frame_type::QUEUE_REQ, &json!({}), &[])),
            Input::AskPeers => {
                self.fruitless = 0;
                self.announce(true);
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        if self.round.as_ref().is_some_and(|r| now >= r.last_progress + SYNC_STALL) {
            let round = self.round.take().expect("just checked");
            self.stalls += 1;
            tracing::info!(peer = %round.peer, stalls = self.stalls, "chat relay: sync stalled");
            // Anyone else who has more goes first. A peer that keeps stalling is
            // gone: stop waking the room for it until it announces again.
            let other = self
                .peers
                .iter()
                .find(|(peer, vv)| **peer != round.peer && self.sink.relay_lacks(vv))
                .map(|(peer, _)| peer.clone());
            if let Some(other) = other {
                self.want = Some((other, now));
            } else if self.stalls < MAX_STALLS {
                self.want = Some((round.peer, now));
            } else {
                self.peers.remove(&round.peer);
            }
        }
        if self.round.is_none()
            && let Some((peer, at)) = self.want.clone()
            && now >= at
        {
            self.want = None;
            self.start_round(peer, now);
        }
        if self.announce_soon.is_some_and(|at| now >= at) {
            self.announce_soon = None;
            self.announce(false);
        }
        if self.announce_settle.is_some_and(|at| now >= at) {
            self.announce_settle = None;
            self.announce(false);
        }
        self.exports.retain(|_, (_, at)| now < *at + EXPORT_TTL);
        self.confirm_saved();
    }

    fn local(&mut self, bytes: Vec<u8>, to_room: bool, now: Instant) {
        // Too big for one frame: the announce below brings peers to fetch it.
        if bytes.len() <= MAX_PUSH_BYTES {
            let direct = wire::encode(frame_type::RELAY, &json!({}), &bytes);
            let mut skip = Vec::new();
            for (peer, (_, link)) in &self.links {
                if link.try_send(direct.clone()).is_ok() {
                    self.counters.direct_frames_sent.fetch_add(1, Relaxed);
                    skip.push(peer.clone());
                }
            }
            if to_room {
                let header = if skip.is_empty() { json!({}) } else { json!({ "skip": skip }) };
                self.to_room(wire::encode(frame_type::RELAY, &header, &bytes));
            }
        }
        self.announce_soon.get_or_insert(now + ANNOUNCE_SOON);
        self.announce_settle = Some(now + ANNOUNCE_SETTLE);
    }

    fn frame(&mut self, from: String, frame: WireFrame, now: Instant) {
        let since_start = self.counters.started.map_or(0, |at| at.elapsed().as_millis() as u64);
        self.counters.last_inbound_ms.store(since_start.max(1), Relaxed);
        match frame.kind {
            frame_type::RELAY => self.relayed(from, frame, now),
            frame_type::ANNOUNCE => self.announced(from, frame, now),
            frame_type::SYNC_REQ => self.sync_requested(from, frame),
            frame_type::SYNC_DATA => self.sync_data(from, frame, now),
            other => tracing::debug!(kind = other, "chat relay: ignoring frame"),
        }
    }

    fn relayed(&mut self, from: String, frame: WireFrame, now: Instant) {
        let outcome = self.sink.relay_import(&frame.payload, false);
        let queued = frame.header["queued"].as_bool() == Some(true);
        let batch_id = frame.header["batchId"].as_str().unwrap_or("").to_string();
        match outcome {
            RelayImport::Applied => {
                if queued && self.policy.confirm_queue && !batch_id.is_empty() {
                    self.unconfirmed.push(batch_id);
                }
                let _ = self.events.send(RelayEvent::Applied);
            }
            RelayImport::Parked => {
                self.parked_queue |= queued;
                self.want_soon(from, now + GAP_GRACE);
            }
            RelayImport::Unreadable => {
                tracing::warn!(queued, "chat relay: couldn't read an update");
            }
        }
    }

    fn announced(&mut self, from: String, frame: WireFrame, now: Instant) {
        let hello = frame.header["hello"].as_bool() == Some(true);
        if self.sink.relay_lacks(&frame.payload) {
            self.fruitless = 0;
            let at = if hello { now } else { now + GAP_GRACE };
            self.want_soon(from.clone(), at);
        }
        if hello && self.sink.relay_behind(&frame.payload) {
            if self.links.contains_key(&from) {
                self.announce_to_link(&from, false);
            } else {
                self.announce(false);
            }
        }
        self.peers.insert(from, frame.payload);
    }

    /// Ask `peer` at `at`, unless a nearer request or a round is under way. The
    /// host wins a tie, since it's the source of truth.
    fn want_soon(&mut self, peer: String, at: Instant) {
        let replace = match &self.want {
            None => true,
            Some((current, when)) => {
                at < *when || (self.host.as_deref() == Some(&peer) && self.host.as_deref() != Some(current))
            }
        };
        if replace {
            self.want = Some((peer, at));
        }
    }

    fn start_round(&mut self, peer: String, now: Instant) {
        let Some(version) = self.sink.relay_version() else {
            return;
        };
        self.counters.sync_rounds.fetch_add(1, Relaxed);
        self.round = Some(Round {
            peer: peer.clone(),
            last_progress: now,
            xfer: String::new(),
            buf: Vec::new(),
            version: version.clone(),
        });
        self.direct(&peer, frame_type::SYNC_REQ, json!({}), &version);
    }

    /// Answer off this task: an export of a large chat takes real time.
    fn sync_requested(&mut self, from: String, frame: WireFrame) {
        if self.counters.serve_host_only.load(Relaxed) && self.host.as_deref() != Some(from.as_str()) {
            // Reads as a task with nothing to give: bounded retries, then the
            // requester waits for an announce from someone who can answer.
            self.direct(&from, frame_type::SYNC_DATA, json!({ "xfer": "", "offset": 0, "total": 0, "gone": true, "noDoc": true }), &[]);
            return;
        }
        if let Some(xfer) = frame.header["xfer"].as_str().filter(|x| !x.is_empty()) {
            let offset = frame.header["offset"].as_u64().unwrap_or(0) as usize;
            if self.exports.contains_key(xfer) {
                self.send_window(&from, xfer, offset);
            } else {
                // Expired: the requester starts again from its current version.
                self.direct(&from, frame_type::SYNC_DATA, json!({ "xfer": "", "offset": 0, "total": 0, "gone": true }), &[]);
            }
            return;
        }
        if !self.exporting.insert(from.clone()) {
            return;
        }
        let sink = self.sink.clone();
        let input = self.input.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = sink.relay_export(&frame.payload);
            if let Some(input) = input.upgrade() {
                let _ = input.send(Input::Exported { to: from, bytes });
            }
        });
    }

    fn exported(&mut self, to: String, bytes: Option<Vec<u8>>) {
        // Nothing to export from: this task outlived its doc. "Empty" would read
        // as "you have everything".
        let Some(bytes) = bytes else {
            self.direct(&to, frame_type::SYNC_DATA, json!({ "xfer": "", "offset": 0, "total": 0, "gone": true, "noDoc": true }), &[]);
            return;
        };
        if bytes.len() <= SYNC_FRAGMENT {
            let total = bytes.len();
            self.direct(&to, frame_type::SYNC_DATA, json!({ "xfer": "", "offset": 0, "total": total }), &bytes);
            return;
        }
        let xfer = uuid::Uuid::new_v4().to_string();
        if self.exports.len() >= EXPORT_CAP
            && let Some(oldest) = self.exports.iter().min_by_key(|(_, (_, at))| *at).map(|(id, _)| id.clone())
        {
            self.exports.remove(&oldest);
        }
        self.exports.insert(xfer.clone(), (bytes, Instant::now()));
        self.send_window(&to, &xfer, 0);
    }

    fn send_window(&mut self, to: &str, xfer: &str, offset: usize) {
        let Some((bytes, kept)) = self.exports.get_mut(xfer) else {
            return;
        };
        // In use: a slow link's transfer can take far longer than the TTL.
        *kept = Instant::now();
        let total = bytes.len();
        let end = (offset + SYNC_WINDOW).min(total);
        let chunks: Vec<(usize, Vec<u8>)> = (offset..end)
            .step_by(SYNC_FRAGMENT)
            .map(|at| (at, bytes[at..(at + SYNC_FRAGMENT).min(end)].to_vec()))
            .collect();
        for (at, chunk) in chunks {
            self.direct(to, frame_type::SYNC_DATA, json!({ "xfer": xfer, "offset": at, "total": total }), &chunk);
        }
    }

    fn sync_data(&mut self, from: String, frame: WireFrame, now: Instant) {
        let xfer = frame.header["xfer"].as_str().unwrap_or("").to_string();
        let offset = frame.header["offset"].as_u64().unwrap_or(0) as usize;
        let total = frame.header["total"].as_u64().unwrap_or(0) as usize;
        let gone = frame.header["gone"].as_bool() == Some(true);
        let in_round = self.round.as_ref().is_some_and(|r| r.peer == from);
        if gone {
            // A link to a task with no doc is no use to anyone: drop it, so the
            // ask goes through the room to wherever that device's chat is now.
            let no_doc = frame.header["noDoc"].as_bool() == Some(true);
            if no_doc && self.links.remove(&from).is_some() {
                self.counters.links.store(self.links.len() as u64, Relaxed);
                let _ = self.events.send(RelayEvent::LinksChanged);
            }
            if in_round {
                self.round = None;
                if no_doc {
                    self.stalls += 1;
                }
                if self.stalls < MAX_STALLS {
                    self.want = Some((from, now + if no_doc { GAP_GRACE } else { Duration::ZERO }));
                }
            }
            return;
        }
        if xfer.is_empty() {
            // Whole in one frame. Safe to import even unasked.
            if total > 0 {
                self.sink.relay_import(&frame.payload, true);
                let _ = self.events.send(RelayEvent::Applied);
            }
            if in_round {
                self.round_finished(now);
            }
            return;
        }
        if !in_round {
            return;
        }
        let round = self.round.as_mut().expect("just checked");
        if offset == 0 && round.xfer.is_empty() {
            round.xfer = xfer.clone();
            round.buf.reserve_exact(total.min(64 * 1024 * 1024));
        }
        if round.xfer != xfer || offset != round.buf.len() {
            // A lost or stale fragment. Ask again from wherever the doc is now.
            self.round = None;
            self.want = Some((from, now + GAP_GRACE));
            return;
        }
        round.buf.extend_from_slice(&frame.payload);
        round.last_progress = now;
        self.stalls = 0;
        let received = round.buf.len();
        if received >= total {
            let blob = std::mem::take(&mut round.buf);
            self.sink.relay_import(&blob, true);
            let _ = self.events.send(RelayEvent::Applied);
            self.round_finished(now);
        } else if received % SYNC_WINDOW == 0 {
            self.direct(&from, frame_type::SYNC_REQ, json!({ "xfer": xfer, "offset": received }), &[]);
        }
    }

    /// See whether anyone still has more, and don't spin if asking achieves nothing.
    fn round_finished(&mut self, now: Instant) {
        let Some(round) = self.round.take() else {
            return;
        };
        let version = self.sink.relay_version().unwrap_or_default();
        self.fruitless = if version == round.version { self.fruitless + 1 } else { 0 };
        if std::mem::take(&mut self.parked_queue) {
            self.read_queue_again();
        }
        if self.fruitless >= MAX_FRUITLESS_ROUNDS {
            return;
        }
        let behind: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, vv)| self.sink.relay_lacks(vv))
            .map(|(peer, _)| peer.clone())
            .collect();
        let pick = behind
            .iter()
            .find(|peer| self.host.as_deref() == Some(peer.as_str()))
            .or_else(|| behind.first())
            .cloned();
        if let Some(peer) = pick {
            self.want_soon(peer, now);
        }
    }

    /// A queue read this task decided on. Counted, or its answer would look like
    /// the answer to a read still under way.
    fn read_queue_again(&mut self) {
        self.counters.queue_reads_asked.fetch_add(1, Relaxed);
        self.to_room(wire::encode(frame_type::QUEUE_REQ, &json!({}), &[]));
    }

    fn confirm_saved(&mut self) {
        if self.unconfirmed.is_empty() || !self.room_open() || !self.sink.relay_durable() {
            return;
        }
        // The stored copy goes only once this device has saved its own.
        for ids in std::mem::take(&mut self.unconfirmed).chunks(MAX_DELIVERED_IDS) {
            self.to_room(wire::encode(frame_type::DELIVERED, &json!({ "batchIds": ids }), &[]));
        }
    }

    fn announce(&mut self, hello: bool) {
        let Some(version) = self.sink.relay_version() else {
            return;
        };
        let header = if hello { json!({ "hello": true }) } else { json!({}) };
        let frame = wire::encode(frame_type::ANNOUNCE, &header, &version);
        for (_, link) in self.links.values() {
            if link.try_send(frame.clone()).is_ok() {
                self.counters.direct_frames_sent.fetch_add(1, Relaxed);
            }
        }
        self.to_room(frame);
    }

    fn announce_to_link(&mut self, peer: &str, hello: bool) {
        let Some(version) = self.sink.relay_version() else {
            return;
        };
        let header = if hello { json!({ "hello": true }) } else { json!({}) };
        let frame = wire::encode(frame_type::ANNOUNCE, &header, &version);
        if let Some((_, link)) = self.links.get(peer)
            && link.try_send(frame).is_ok()
        {
            self.counters.direct_frames_sent.fetch_add(1, Relaxed);
        }
    }

    /// To one device: its direct link if up, otherwise through the room.
    fn direct(&mut self, to: &str, kind: u8, mut header: Value, payload: &[u8]) {
        if let Some((_, link)) = self.links.get(to)
            && link.try_send(wire::encode(kind, &header, payload)).is_ok()
        {
            self.counters.direct_frames_sent.fetch_add(1, Relaxed);
            return;
        }
        header["to"] = json!(to);
        self.to_room(wire::encode(kind, &header, payload));
    }

    fn to_room(&mut self, frame: Vec<u8>) {
        if let Some(room) = &self.room
            && room.try_send(frame).is_ok()
        {
            self.counters.room_frames_sent.fetch_add(1, Relaxed);
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Carry one chat over a direct link until it dies or `stop` fires. Both ends
/// run this. The stream is reliable and ordered while it lives, but it's still
/// only a hint: losing it costs the time until the room path takes over.
pub async fn run_link<I>(io: I, peer: String, relay: RelayHandle, stop: tokio_util::sync::CancellationToken)
where
    I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(io);
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(PATH_QUEUE);
    static NEXT_LINK: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_LINK.fetch_add(1, Relaxed);
    let _ = relay.tx.send(Input::LinkUp { peer: peer.clone(), id, tx });
    let write = async {
        let probe = wire::encode(frame_type::PROBE, &json!({}), &[]);
        let mut beat = tokio::time::interval(LINK_PROBE);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let frame = tokio::select! {
                frame = rx.recv() => match frame {
                    Some(frame) => frame,
                    None => return,
                },
                _ = beat.tick() => probe.clone(),
            };
            let len = (frame.len() as u32).to_le_bytes();
            if writer.write_all(&len).await.is_err()
                || writer.write_all(&frame).await.is_err()
                || writer.flush().await.is_err()
            {
                return;
            }
        }
    };
    let read = async {
        loop {
            let mut len = [0u8; 4];
            match tokio::time::timeout(LINK_DEAD, reader.read_exact(&mut len)).await {
                Ok(Ok(_)) => {}
                _ => return,
            }
            let len = u32::from_le_bytes(len) as usize;
            if len > LINK_MAX_FRAME {
                tracing::warn!(len, "chat link: oversized frame; closing");
                return;
            }
            let mut bytes = vec![0u8; len];
            match tokio::time::timeout(SYNC_STALL, reader.read_exact(&mut bytes)).await {
                Ok(Ok(_)) => {}
                _ => return,
            }
            let Some(frame) = wire::decode(&bytes) else {
                tracing::warn!("chat link: bad frame; closing");
                return;
            };
            if frame.kind == frame_type::PROBE {
                continue;
            }
            if relay
                .tx
                .send(Input::LinkFrame { peer: peer.clone(), frame })
                .is_err()
            {
                return;
            }
        }
    };
    tokio::select! {
        _ = stop.cancelled() => {}
        _ = write => {}
        _ = read => {}
    }
    let _ = relay.tx.send(Input::LinkDown { peer, id });
}

#[cfg(test)]
mod tests;
