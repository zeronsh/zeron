//! Terminals on other devices: their I/O takes a direct peer-to-peer link
//! ([`crate::terminals::link`]) whenever the two devices can reach each other,
//! and the device-room relay only when they cannot.
//!
//! - Subscribe opens the direct link when one is already up; otherwise it
//!   starts on the relay and keeps trying to pair, then moves the live stream
//!   onto the direct link mid-session by re-attaching after the last seq it
//!   delivered (no gap, no repeat). A direct link that dies falls back to the
//!   relay the same way, without ending the viewer's stream.
//! - Writes and resizes return at once and stay in typed order: an attached
//!   direct link carries them; otherwise a per-terminal queue forwards them over
//!   the relay one at a time. Writes only move onto a fresh direct link once the
//!   relay queue has drained.
//! - Input is numbered and held until the host acknowledges it. Whichever path
//!   takes over after another dies replays what is unacknowledged, and the host
//!   applies each number once, so a network change neither drops nor repeats
//!   a keystroke. A relayed write that fails is retried in place (in order)
//!   before it waits for the next path to replay it.
//! - The direct side can wake a relayed terminal's upgrade at once
//!   ([`Direct::changed`]: a network change, the app back in front) instead of
//!   it sitting out its retry pacing; the path each subscription is on is
//!   reported ([`TerminalPath`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, watch};
use zeron_preview::mux::BoxIo;
use zeron_proto::TerminalEvent;
use zeron_rpc::{RpcError, methods};

use crate::link::{self, Link, Outgoing};

/// How long a subscribe waits on an already-open direct connection before it
/// settles for the relay.
const DIRECT_ATTACH_BUDGET: Duration = Duration::from_secs(3);
/// Retry pacing for upgrading a relayed terminal to a direct link.
const UPGRADE_RETRY_MIN: Duration = Duration::from_secs(15);
const UPGRADE_RETRY_MAX: Duration = Duration::from_secs(300);
/// A relay write queue with nothing to send retires after this long.
const RELAY_QUEUE_IDLE: Duration = Duration::from_secs(60);
/// Recently closed or exited terminals remembered, so writes to them fail
/// like a local write would instead of vanishing into a queue.
const ENDED_MEMORY: usize = 256;
/// A relayed write that fails is retried this many times (backing off from
/// 200ms to 2s) before it's left for the next path to replay.
const RELAY_WRITE_ATTEMPTS: u32 = 6;

/// The relay path: ordinary device-addressed RPC forwarding.
#[async_trait]
pub trait Relay: Send + Sync + 'static {
    async fn call(
        &self,
        target: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError>;
    async fn subscribe(
        &self,
        target: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<BoxStream<'static, serde_json::Value>, RpcError>;
}

/// The direct path: peer-to-peer streams to another device.
#[async_trait]
pub trait Direct: Send + Sync + 'static {
    /// A direct connection to `device` is open right now.
    async fn ready(&self, device: &str) -> bool;
    /// Open `service` on `device`, pairing first when needed.
    async fn open(&self, device: &str, service: &str) -> anyhow::Result<BoxIo>;
    /// Resolves when a direct path may have just become possible (a network
    /// change, the app back in front): relayed terminals retry their upgrade
    /// at once rather than at their retry pacing. Never, by default.
    async fn changed(&self) {
        futures::future::pending::<()>().await
    }
}

#[async_trait]
impl Direct for zeron_preview::viewer::ViewerPeers {
    async fn ready(&self, device: &str) -> bool {
        zeron_preview::viewer::ViewerPeers::ready(self, device).await
    }
    async fn open(&self, device: &str, service: &str) -> anyhow::Result<BoxIo> {
        Ok(Box::new(
            zeron_preview::viewer::ViewerPeers::open(self, device, service).await?,
        ))
    }
    async fn changed(&self) {
        zeron_preview::viewer::ViewerPeers::changed(self).await
    }
}

/// Which path a terminal's I/O takes right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalPath {
    /// The device-room relay.
    Relay,
    /// A direct peer-to-peer link.
    Direct,
}

#[async_trait]
impl Direct for zeron_preview::PreviewService {
    async fn ready(&self, device: &str) -> bool {
        self.peer_ready(device).await
    }
    async fn open(&self, device: &str, service: &str) -> anyhow::Result<BoxIo> {
        Ok(Box::new(self.open_peer_stream(device, service).await?))
    }
}

type Key = (String, String);

/// What a viewer asks of a remote terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

/// Unacknowledged input kept for replay after a path change. Past this the
/// oldest is dropped: a path down that long has bigger problems, and the
/// bound keeps a dead link from holding a paste forever.
const MAX_UNACKED_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct Entry {
    /// Writers of attached direct links, each tagged with the subscription
    /// that owns it (two views of one terminal each attach their own).
    direct: Vec<(u64, mpsc::UnboundedSender<Outgoing>)>,
    /// Ordered relay writer, spawned on first use.
    relay: Option<mpsc::UnboundedSender<Outgoing>>,
    /// Relay writes queued or in flight.
    relay_pending: Arc<AtomicUsize>,
    /// Last input seq handed out. Never reset while this engine runs: the
    /// host drops any seq it has already applied from this client.
    next_input: u64,
    /// Input the host hasn't acknowledged yet, oldest first.
    unacked: std::collections::VecDeque<(u64, Vec<u8>)>,
    unacked_bytes: usize,
}

impl Entry {
    fn live_direct(&mut self) -> Option<&mpsc::UnboundedSender<Outgoing>> {
        self.direct.retain(|(_, tx)| !tx.is_closed());
        self.direct.first().map(|(_, tx)| tx)
    }

    fn ack(&mut self, seq: u64) {
        while self.unacked.front().is_some_and(|(s, _)| *s <= seq) {
            if let Some((_, bytes)) = self.unacked.pop_front() {
                self.unacked_bytes -= bytes.len();
            }
        }
    }

    fn replay(&self) -> Vec<Outgoing> {
        self.unacked
            .iter()
            .map(|(seq, bytes)| Outgoing::Input {
                seq: *seq,
                bytes: bytes.clone(),
            })
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct RemoteTerminals {
    /// This engine's identity as a remote viewer; input seqs are per client.
    client: Arc<str>,
    entries: Arc<Mutex<HashMap<Key, Entry>>>,
    ended: Arc<Mutex<std::collections::VecDeque<Key>>>,
    pumps: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for RemoteTerminals {
    fn default() -> Self {
        Self {
            client: uuid::Uuid::new_v4().to_string().into(),
            entries: Default::default(),
            ended: Default::default(),
            pumps: Default::default(),
        }
    }
}

impl RemoteTerminals {
    /// Queue `write` for the remote terminal; never waits on the network.
    /// Fails only for a terminal this device saw close or exit.
    pub fn send(
        &self,
        relay: &Arc<dyn Relay>,
        target: &str,
        terminal_id: &str,
        write: Write,
    ) -> Result<(), RpcError> {
        let key = (target.to_owned(), terminal_id.to_owned());
        if lock(&self.ended).contains(&key) {
            return Err(RpcError::Failed("Terminal has exited".into()));
        }
        let mut entries = lock(&self.entries);
        let entry = entries.entry(key.clone()).or_default();
        let out = match write {
            Write::Input(bytes) => {
                entry.next_input += 1;
                let seq = entry.next_input;
                entry.unacked_bytes += bytes.len();
                entry.unacked.push_back((seq, bytes.clone()));
                while entry.unacked_bytes > MAX_UNACKED_BYTES && entry.unacked.len() > 1 {
                    if let Some((_, dropped)) = entry.unacked.pop_front() {
                        entry.unacked_bytes -= dropped.len();
                    }
                }
                Outgoing::Input { seq, bytes }
            }
            Write::Resize { cols, rows } => Outgoing::Resize { cols, rows },
        };
        self.route(entry, relay, &key, vec![out]);
        Ok(())
    }

    /// Hand `outs` to the attached direct link, or to the relay queue while
    /// there is none or the relay still has earlier writes to deliver.
    fn route(&self, entry: &mut Entry, relay: &Arc<dyn Relay>, key: &Key, outs: Vec<Outgoing>) {
        if entry.relay_pending.load(Ordering::Acquire) == 0
            && let Some(direct) = entry.live_direct()
            && outs.iter().all(|out| direct.send(out.clone()).is_ok())
        {
            return;
        }
        let pending = entry.relay_pending.clone();
        let queue = entry.relay.get_or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(relay_writer(
                self.clone(),
                key.clone(),
                relay.clone(),
                pending.clone(),
                rx,
            ));
            tx
        });
        for out in outs {
            pending.fetch_add(1, Ordering::AcqRel);
            if queue.send(out).is_err() {
                pending.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    fn ack(&self, key: &Key, seq: u64) {
        if let Some(entry) = lock(&self.entries).get_mut(key) {
            entry.ack(seq);
        }
    }

    /// A path just took over: send it whatever the host hasn't acknowledged
    /// (input a dying link may have swallowed). The host applies each seq
    /// once, so a replay of input that did land is harmless.
    fn replay(&self, key: &Key, relay: &Arc<dyn Relay>) {
        let mut entries = lock(&self.entries);
        if let Some(entry) = entries.get_mut(key) {
            let outs = entry.replay();
            if !outs.is_empty() {
                self.route(entry, relay, key, outs);
            }
        }
    }

    /// The terminal closed or exited: detach its link, refuse further writes.
    pub fn ended(&self, target: &str, terminal_id: &str) {
        let key = (target.to_owned(), terminal_id.to_owned());
        lock(&self.entries).remove(&key);
        let mut ended = lock(&self.ended);
        if !ended.contains(&key) {
            if ended.len() == ENDED_MEMORY {
                ended.pop_front();
            }
            ended.push_back(key);
        }
    }

    fn attach_writer(&self, key: &Key, pump: u64, writer: mpsc::UnboundedSender<Outgoing>) {
        let mut entries = lock(&self.entries);
        if lock(&self.ended).contains(key) {
            return;
        }
        let entry = entries.entry(key.clone()).or_default();
        if entry
            .direct
            .iter()
            .any(|(owner, tx)| *owner == pump && tx.same_channel(&writer))
        {
            return; // already attached
        }
        // Replay first, on the new link itself: it carries them ahead of
        // anything typed from now on.
        for out in entry.replay() {
            let _ = writer.send(out);
        }
        entry.direct.push((pump, writer));
    }

    fn detach_writer(&self, key: &Key, pump: u64) {
        if let Some(entry) = lock(&self.entries).get_mut(key) {
            entry.direct.retain(|(owner, _)| *owner != pump);
        }
    }

    /// Whether a direct link carries this terminal's writes now (tests,
    /// diagnostics).
    #[doc(hidden)]
    pub fn direct_attached(&self, target: &str, terminal_id: &str) -> bool {
        lock(&self.entries)
            .get_mut(&(target.to_owned(), terminal_id.to_owned()))
            .is_some_and(|entry| entry.live_direct().is_some())
    }

    /// The terminal's events after `after_seq`, over the best path available
    /// now and the better one once it opens. Errors only when no path reaches
    /// the terminal at all.
    pub async fn subscribe(
        &self,
        relay: Arc<dyn Relay>,
        direct: Option<Arc<dyn Direct>>,
        target: &str,
        terminal_id: &str,
        after_seq: u64,
        host_replies: bool,
    ) -> Result<BoxStream<'static, serde_json::Value>, RpcError> {
        self.subscribe_tracked(relay, direct, target, terminal_id, after_seq, host_replies)
            .await
            .map(|(events, _)| events)
    }

    /// [`Self::subscribe`], also reporting which path carries the stream as
    /// it moves between them.
    pub async fn subscribe_tracked(
        &self,
        relay: Arc<dyn Relay>,
        direct: Option<Arc<dyn Direct>>,
        target: &str,
        terminal_id: &str,
        after_seq: u64,
        host_replies: bool,
    ) -> Result<
        (
            BoxStream<'static, serde_json::Value>,
            watch::Receiver<TerminalPath>,
        ),
        RpcError,
    > {
        let key = (target.to_owned(), terminal_id.to_owned());
        let mut first = None;
        if let Some(direct) = &direct
            && direct.ready(target).await
        {
            match tokio::time::timeout(
                DIRECT_ATTACH_BUDGET,
                attach_direct(
                    direct.as_ref(),
                    target,
                    terminal_id,
                    after_seq,
                    &self.client,
                ),
            )
            .await
            {
                Ok(Ok(link)) => first = Some(Source::Direct(link)),
                Ok(Err(error)) => tracing::debug!(%error, "direct terminal attach failed"),
                Err(_) => tracing::debug!("direct terminal attach timed out"),
            }
        }
        let first = match first {
            Some(source) => source,
            None => Source::Relay(
                relay_subscribe(relay.as_ref(), target, terminal_id, after_seq, host_replies)
                    .await?,
            ),
        };
        let (out, events) = mpsc::unbounded_channel();
        let (path, path_rx) = watch::channel(match first {
            Source::Direct(_) => TerminalPath::Direct,
            Source::Relay(_) => TerminalPath::Relay,
        });
        let id = self.pumps.fetch_add(1, Ordering::Relaxed);
        // Writes issued right after this returns must already take the link.
        if let Source::Direct(link) = &first {
            self.attach_writer(&key, id, link.outgoing.clone());
        }
        tokio::spawn(
            Pump {
                id,
                hub: self.clone(),
                key,
                relay,
                direct,
                out,
                last_seq: after_seq,
                host_replies,
                path,
            }
            .run(first),
        );
        let events = futures::stream::unfold(events, |mut events| async move {
            events.recv().await.map(|event| (event, events))
        })
        .boxed();
        Ok((events, path_rx))
    }
}

async fn attach_direct(
    direct: &dyn Direct,
    target: &str,
    terminal_id: &str,
    after_seq: u64,
    client: &str,
) -> anyhow::Result<Link> {
    let io = direct.open(target, &link::service_id(terminal_id)).await?;
    link::attach(io, after_seq, client).await
}

/// A direct link always leaves queries to the host (only engines with host
/// replies open one); over the relay the viewer's choice travels along.
async fn relay_subscribe(
    relay: &dyn Relay,
    target: &str,
    terminal_id: &str,
    after_seq: u64,
    host_replies: bool,
) -> Result<BoxStream<'static, serde_json::Value>, RpcError> {
    relay
        .subscribe(
            target,
            methods::SUBSCRIBE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "afterSeq": after_seq,
                "targetDeviceId": target,
                "hostReplies": host_replies,
            }),
        )
        .await
}

/// Forwards relayed writes one at a time, in order.
async fn relay_writer(
    hub: RemoteTerminals,
    key: Key,
    relay: Arc<dyn Relay>,
    pending: Arc<AtomicUsize>,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
) {
    loop {
        let out = match tokio::time::timeout(RELAY_QUEUE_IDLE, rx.recv()).await {
            Ok(Some(out)) => out,
            Ok(None) => return,
            Err(_) => {
                // Retire under the lock so no write lands in a dead queue.
                let mut entries = lock(&hub.entries);
                if let Ok(out) = rx.try_recv() {
                    drop(entries);
                    forward_one(&hub, &key, relay.as_ref(), out).await;
                    pending.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                if let Some(entry) = entries.get_mut(&key) {
                    entry.relay = None;
                }
                return;
            }
        };
        forward_one(&hub, &key, relay.as_ref(), out).await;
        pending.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Forward one write, retrying a failed one in place (later writes wait
/// behind it, so order holds) while the terminal is still live.
async fn forward_one(hub: &RemoteTerminals, key: &Key, relay: &dyn Relay, out: Outgoing) {
    let mut delay = Duration::from_millis(200);
    for attempt in 1..=RELAY_WRITE_ATTEMPTS {
        if forward_once(hub, key, relay, out.clone()).await {
            return;
        }
        let live = lock(&hub.entries).contains_key(key);
        if !live || attempt == RELAY_WRITE_ATTEMPTS {
            return;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(2));
    }
}

/// One attempt; `true` once the host took it (or refused it for good).
async fn forward_once(hub: &RemoteTerminals, key: &Key, relay: &dyn Relay, out: Outgoing) -> bool {
    use base64::Engine as _;
    let (target, terminal_id) = key;
    let (method, params, seq) = match out {
        Outgoing::Input { seq, bytes } => (
            methods::WRITE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                "targetDeviceId": target,
                "clientId": &*hub.client,
                "inputSeq": seq,
            }),
            Some(seq),
        ),
        Outgoing::Resize { cols, rows } => (
            methods::RESIZE_TERMINAL,
            serde_json::json!({
                "terminalId": terminal_id,
                "cols": cols,
                "rows": rows,
                "targetDeviceId": target,
            }),
            None,
        ),
    };
    match relay.call(target, method, params).await {
        Ok(_) => {
            if let Some(seq) = seq {
                hub.ack(key, seq);
            }
            true
        }
        // The host refused it (an ended terminal): retrying can't help.
        Err(RpcError::Failed(error) | RpcError::BadParams(error)) => {
            tracing::debug!(%error, method, "relayed terminal write refused");
            true
        }
        // Unacknowledged input stays queued for the retry or the next path.
        Err(error) => {
            tracing::debug!(%error, method, "relayed terminal write failed");
            false
        }
    }
}

enum Source {
    Direct(Link),
    Relay(BoxStream<'static, serde_json::Value>),
}

/// Why a source stopped delivering.
enum Ended {
    /// The terminal exited, the viewer went away, or the relay itself
    /// dropped (the viewer resubscribes with its own backoff, so a relay
    /// that keeps failing is never redialed in a tight loop).
    Done,
    /// The direct link died; resume on the relay at once.
    Lost,
    /// A direct link opened while relaying: switch to it.
    Upgrade(Link),
}

struct Pump {
    /// Owner tag for this subscription's direct writer.
    id: u64,
    hub: RemoteTerminals,
    key: Key,
    relay: Arc<dyn Relay>,
    direct: Option<Arc<dyn Direct>>,
    out: mpsc::UnboundedSender<serde_json::Value>,
    last_seq: u64,
    /// Carried to relay subscriptions (a direct link implies it).
    host_replies: bool,
    /// The path carrying the stream, for whoever watches.
    path: watch::Sender<TerminalPath>,
}

impl Pump {
    async fn run(mut self, mut source: Source) {
        loop {
            let ended = match source {
                Source::Direct(link) => {
                    self.path.send_replace(TerminalPath::Direct);
                    self.pump_direct(link).await
                }
                Source::Relay(stream) => {
                    self.path.send_replace(TerminalPath::Relay);
                    self.pump_relay(stream).await
                }
            };
            source = match ended {
                Ended::Done => break,
                Ended::Upgrade(link) => Source::Direct(link),
                Ended::Lost => {
                    // Ending here would make the viewer resubscribe after a
                    // pause; resuming on the relay right away keeps typing
                    // alive through a network change.
                    let (target, terminal_id) = &self.key;
                    match relay_subscribe(
                        self.relay.as_ref(),
                        target,
                        terminal_id,
                        self.last_seq,
                        self.host_replies,
                    )
                    .await
                    {
                        Ok(stream) => {
                            self.hub.replay(&self.key, &self.relay);
                            Source::Relay(stream)
                        }
                        Err(error) => {
                            tracing::debug!(%error, "terminal relay resume failed");
                            break;
                        }
                    }
                }
            };
        }
        self.hub.detach_writer(&self.key, self.id);
    }

    /// Deliver one event; `false` once the stream is over.
    fn deliver(&mut self, event: &TerminalEvent, value: serde_json::Value) -> bool {
        let (seq, exit) = match event {
            TerminalEvent::Data { seq, .. } => (*seq, false),
            TerminalEvent::Exit { seq, .. } => (*seq, true),
        };
        if seq <= self.last_seq {
            return true; // already delivered over the previous path
        }
        self.last_seq = seq;
        if exit {
            self.hub.ended(&self.key.0, &self.key.1);
        }
        self.out.send(value).is_ok() && !exit
    }

    async fn pump_direct(&mut self, link: Link) -> Ended {
        let Link {
            outgoing,
            mut events,
            mut acks,
        } = link;
        self.hub.attach_writer(&self.key, self.id, outgoing);
        let ended = loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { break Ended::Lost };
                    let Ok(value) = serde_json::to_value(&event) else { continue };
                    if !self.deliver(&event, value) {
                        break Ended::Done;
                    }
                }
                Some(seq) = acks.recv() => self.hub.ack(&self.key, seq),
                _ = self.out.closed() => break Ended::Done,
            }
        };
        self.hub.detach_writer(&self.key, self.id);
        ended
    }

    async fn pump_relay(&mut self, mut stream: BoxStream<'static, serde_json::Value>) -> Ended {
        let (target, terminal_id) = (self.key.0.clone(), self.key.1.clone());
        let direct = self.direct.clone();
        // Pairing runs beside the relay; a resolved direct stream is attached
        // only between relay events, so the attach point is exact.
        let mut upgrade: futures::future::BoxFuture<'static, BoxIo> = Box::pin(async move {
            let Some(direct) = direct else {
                return futures::future::pending::<BoxIo>().await;
            };
            let mut retry = UPGRADE_RETRY_MIN;
            loop {
                match direct.open(&target, &link::service_id(&terminal_id)).await {
                    Ok(io) => return io,
                    Err(error) => {
                        tracing::debug!(%error, "direct terminal link unavailable; staying on the relay")
                    }
                }
                // A network change or the app coming back retries now (and
                // starts the pacing over); otherwise back off.
                tokio::select! {
                    _ = tokio::time::sleep(retry) => retry = (retry * 2).min(UPGRADE_RETRY_MAX),
                    _ = direct.changed() => retry = UPGRADE_RETRY_MIN,
                }
            }
        });
        loop {
            tokio::select! {
                item = stream.next() => {
                    let Some(value) = item else { return Ended::Done };
                    let Ok(event) = serde_json::from_value::<TerminalEvent>(value.clone()) else {
                        continue;
                    };
                    if !self.deliver(&event, value) {
                        return Ended::Done;
                    }
                }
                io = &mut upgrade => {
                    match link::attach(io, self.last_seq, &self.hub.client).await {
                        Ok(link) => return Ended::Upgrade(link),
                        Err(error) => {
                            tracing::debug!(%error, "direct terminal attach failed; staying on the relay");
                            // Leave the relay in charge; no further upgrades
                            // this session (the host refused the link).
                            upgrade = Box::pin(futures::future::pending());
                        }
                    }
                }
                _ = self.out.closed() => return Ended::Done,
            }
        }
    }
}
