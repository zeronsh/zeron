//! The engine's side of the relay layer (docs/design/relay-layer.md), behind
//! `ZERON_RELAY_LAYER=1`. Off, nothing here runs: no chat handle carries relay
//! state, no capability is advertised and no stream service is registered.
//!
//! The safety rule lives in [`DocHost::refresh_relay_mode`]: a host that stops
//! relaying sets its mode first and posts a checkpoint exported after that, so
//! no commit can fall between the checkpoint and the stored rows that follow.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zeron_proto::relay_gen::{RELAY_ROOM_GEN, STORED_ROOM_GEN, relay_allowed};
use zeron_sync::chat_relay::{RelayEvent, RelayHandle, RelayPolicy, run_link};

use super::{ChatDocHandle, DocHost, DocHostInner, lock};

pub use zeron_proto::relay_gen::CHAT_LINK_PREFIX;
/// `spawn_chat2_checkpoint`'s reason for the checkpoint a stop owes.
pub(super) const HEAL_REASON: &str = "relay-heal";

const MAINTENANCE_TICK: Duration = Duration::from_secs(1);
/// How often a host re-checks who's on the account and the chat's generation.
const MODE_REFRESH: Duration = Duration::from_secs(10);
/// How long a host keeps storing after it last heard of an old reader.
const LEGACY_HOLD: Duration = Duration::from_secs(600);
/// Cloud checkpoints: a turn has ended after this long without a commit; never
/// closer together than the gap, and at least this often during a long turn.
const CLOUD_QUIET: Duration = Duration::from_secs(3);
const CLOUD_MIN_GAP: Duration = Duration::from_secs(10);
const CLOUD_MAX_GAP: Duration = Duration::from_secs(60);
/// How long a Cloud sleep waits for its final checkpoint.
const SLEEP_CHECKPOINT_WAIT: Duration = Duration::from_secs(20);
const CHECKPOINT_RETRY_MIN: Duration = Duration::from_secs(2);
const CHECKPOINT_RETRY_MAX: Duration = Duration::from_secs(300);
/// How long a host keeps its socket after a peer's last frame, so a peer it just
/// woke or that's mid-sync isn't cut off. A sync sends frames throughout.
const LINGER_AFTER_PEER: Duration = Duration::from_secs(15);
/// Direct link attempts, from a chat on screen to its host.
const LINK_RETRY_MIN: Duration = Duration::from_secs(15);
const LINK_RETRY_MAX: Duration = Duration::from_secs(300);
/// Longest a link attempt may take, pairing included. An open on a connection that
/// has stopped moving would otherwise wait forever and block every later attempt.
const LINK_OPEN_LIMIT: Duration = Duration::from_secs(45);
/// A snapshot past this stays on stored rows, or goes back to them. The room takes
/// checkpoints up to 32 MiB (`MAX_CHECKPOINT_BYTES`), and a chat over that can't
/// heal, can't bring an old reader up to date and can't let a Cloud machine sleep.
/// The gap is the room to grow while the heal lands.
const RELAY_SNAPSHOT_LIMIT: usize = 24 * 1024 * 1024;

/// The engine-wide flag.
pub fn relay_layer_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let on = std::env::var("ZERON_RELAY_LAYER").as_deref() == Ok("1");
        if on {
            zeron_sync::registry::announce_relay_layer();
        }
        on
    })
}

#[derive(Default)]
struct CheckpointBackoff {
    wait: Duration,
    until: Option<Instant>,
}

#[derive(Default)]
struct Unretired {
    ids: Vec<String>,
    settled: usize,
}

/// How far the kept copies of relayed updates are covered by a saved snapshot.
#[derive(Default)]
struct Journal {
    newest: i64,
    /// Listed for at least one maintenance tick.
    settled: i64,
}

/// One open chat's relay state. Exists only with the flag on.
pub(crate) struct ChatRelayState {
    pub(super) relay: RelayHandle,
    /// This device hosts the chat and publishes by relay, not stored rows.
    relaying: AtomicBool,
    /// This device hosts the chat, whichever way it's publishing.
    hosting: AtomicBool,
    /// A checkpoint is owed, because this host stopped relaying.
    heal_owed: AtomicBool,
    /// That checkpoint landed, since the last time this host relayed.
    healed: AtomicBool,
    /// The room's last answer about the layer.
    room_offered: AtomicBool,
    /// Bumped each time this host stops relaying.
    mode_epoch: AtomicU64,
    checkpoint_backoff: Mutex<CheckpointBackoff>,
    legacy_until: Mutex<Option<Instant>>,
    /// Relayed commits in the durable outbox, held until a saved snapshot covers
    /// them. A crash before then uploads them as stored rows on restart, as today.
    /// While this process lives they aren't uploaded: the doc has them. The first
    /// `settled` of them have been listed for at least one maintenance tick.
    unretired: Mutex<Unretired>,
    /// Commits so far, to tell a turn's end from a pause in it.
    commits: AtomicU64,
    /// The version the room's checkpoint last reached, and when.
    checkpointed: Mutex<Option<(loro::VersionVector, Instant)>>,
    link: Mutex<Option<CancellationToken>>,
    /// This process has seen its doc catch up with the room. Before that, its
    /// export may hold less than the checkpoint it would replace.
    caught_up: AtomicBool,
    /// This process has relayed this chat.
    relayed_here: AtomicBool,
    /// The room refused a checkpoint as too big, and it would again.
    too_large: AtomicBool,
    /// Ends the links this host accepted, when the chat's handle goes away. A link
    /// holds its own reference to the relay task, which has no doc after that.
    accepted: CancellationToken,
    journal: Mutex<Journal>,
}

impl Drop for ChatRelayState {
    fn drop(&mut self) {
        self.accepted.cancel();
    }
}

impl ChatRelayState {
    pub(super) fn new(relay: RelayHandle) -> Arc<Self> {
        Arc::new(Self {
            relay,
            relaying: AtomicBool::new(false),
            hosting: AtomicBool::new(false),
            heal_owed: AtomicBool::new(false),
            healed: AtomicBool::new(false),
            room_offered: AtomicBool::new(true),
            mode_epoch: AtomicU64::new(0),
            checkpoint_backoff: Mutex::new(CheckpointBackoff::default()),
            legacy_until: Mutex::new(None),
            unretired: Mutex::new(Unretired::default()),
            commits: AtomicU64::new(0),
            checkpointed: Mutex::new(None),
            link: Mutex::new(None),
            caught_up: AtomicBool::new(false),
            relayed_here: AtomicBool::new(false),
            too_large: AtomicBool::new(false),
            accepted: CancellationToken::new(),
            journal: Mutex::new(Journal::default()),
        })
    }

    pub(super) fn relaying(&self) -> bool {
        self.relaying.load(Ordering::Acquire)
    }

    pub(super) fn caught_up(&self) -> bool {
        self.caught_up.load(Ordering::Acquire)
    }

    /// The room refused a checkpoint. Too big won't change by retrying. Stale means
    /// the room holds something this doc doesn't: read it before posting again.
    pub(super) fn checkpoint_refused(&self, status: u16, body: &str, handle: Option<&ChatDocHandle>) {
        if status == 413 {
            self.too_large.store(true, Ordering::Release);
        } else if status == 409 && body.contains("stale") {
            self.caught_up.store(false, Ordering::Release);
            if let Some(handle) = handle
                && let Some(client) = &*lock(&handle.chat2)
            {
                client.redial();
            }
        }
    }

    /// Positions of kept updates a saved snapshot now covers. As with
    /// [`Self::retirable`], "saved" only covers those listed a tick ago.
    fn retirable_journal(&self, saved: bool) -> Option<i64> {
        let mut journal = lock(&self.journal);
        let through = (saved && journal.settled > 0).then_some(journal.settled);
        if through.is_some() {
            journal.settled = 0;
        }
        if journal.settled == 0 && journal.newest > 0 {
            journal.settled = journal.newest;
            journal.newest = 0;
        }
        through
    }

    /// Ids safe to retire from the outbox now. A commit reaches this list before
    /// its save is scheduled, so "saved" only covers ids listed a tick ago.
    fn retirable(&self, saved: bool) -> Vec<String> {
        let mut unretired = lock(&self.unretired);
        let settled = unretired.settled;
        let retired = if saved { unretired.ids.drain(..settled).collect() } else { Vec::new() };
        unretired.settled = unretired.ids.len();
        retired
    }

    /// Outbox entries to upload as stored rows: all but the relayed commits this
    /// process still holds in its doc.
    pub(crate) fn uploadable(&self, pending: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
        let unretired = lock(&self.unretired);
        pending
            .into_iter()
            .filter(|(id, _)| !unretired.ids.contains(id))
            .collect()
    }

    /// A host that isn't relaying must also store what the queue brings it:
    /// its own stored rows depend on those ops, and old readers never see the queue.
    pub(crate) fn republishes_queue_rows(&self) -> bool {
        self.hosting.load(Ordering::Acquire) && !self.relaying()
    }

    /// A local commit, already in the durable outbox. True: it's been relayed,
    /// and must not also become a stored row.
    pub(super) fn publish_local(&self, batch_id: &str, bytes: &[u8]) -> bool {
        self.commits.fetch_add(1, Ordering::Relaxed);
        let relaying = self.relaying();
        self.relay.local_update(bytes.to_vec(), relaying);
        if relaying {
            lock(&self.unretired).ids.push(batch_id.to_string());
        }
        relaying
    }

    /// `epoch`: the mode epoch sampled before this checkpoint's export. An export
    /// from before the host last stopped relaying can't settle what that stop owes:
    /// it may lack commits relayed after it began.
    pub(super) fn checkpoint_landed(&self, version: loro::VersionVector, reason: &str, epoch: u64) {
        *lock(&self.checkpointed) = Some((version, Instant::now()));
        *lock(&self.checkpoint_backoff) = CheckpointBackoff::default();
        if reason == HEAL_REASON && epoch == self.mode_epoch() && !self.relaying() {
            self.heal_owed.store(false, Ordering::Release);
            self.healed.store(true, Ordering::Release);
        }
    }

    pub(super) fn stop_link(&self) {
        if let Some(stop) = lock(&self.link).take() {
            stop.cancel();
        }
    }

    pub(super) fn mode_epoch(&self) -> u64 {
        self.mode_epoch.load(Ordering::Acquire)
    }

    /// A failed checkpoint waits before the next try: each is a full export and
    /// upload, and a chat over the size cap fails every time.
    pub(super) fn checkpoint_failed(&self) {
        let mut backoff = lock(&self.checkpoint_backoff);
        backoff.wait = (backoff.wait * 2).clamp(CHECKPOINT_RETRY_MIN, CHECKPOINT_RETRY_MAX);
        backoff.until = Some(Instant::now() + backoff.wait);
    }

    pub(super) fn checkpoint_may_run(&self) -> bool {
        lock(&self.checkpoint_backoff).until.is_none_or(|until| Instant::now() >= until)
    }

    fn checkpoint_covers(&self, version: &loro::VersionVector) -> bool {
        lock(&self.checkpointed)
            .as_ref()
            .is_some_and(|(reached, _)| reached.includes_vv(version))
    }
}

impl ChatDocHandle {
    /// May this chat's socket close, or its handle be evicted? Not while a peer is
    /// using it, a stop's checkpoint is owed, or a Cloud host's checkpoint is behind
    /// its doc. That checkpoint is all anyone can read until the machine wakes.
    pub(super) fn relay_lets_socket_close(&self) -> bool {
        let Some(state) = lock(&self.relay).clone() else {
            return true;
        };
        if state.too_large.load(Ordering::Acquire) {
            return !state.relay.recently_active(LINGER_AFTER_PEER);
        }
        if state.relay.recently_active(LINGER_AFTER_PEER) || state.heal_owed.load(Ordering::Acquire) {
            return false;
        }
        !(state.relaying()
            && crate::runner::session().is_some()
            && !state.checkpoint_covers(&self.doc.doc().oplog_vv()))
    }

    /// Another device has a link to this chat open. The link's relay task is this
    /// handle's: evicting it would leave that device talking to a task with no doc.
    pub(super) fn relay_has_links(&self) -> bool {
        lock(&self.relay).as_ref().is_some_and(|state| state.relay.stats().links > 0)
    }

    pub(crate) fn note_journaled(&self, ordinal: i64) {
        if let Some(state) = &*lock(&self.relay) {
            lock(&state.journal).newest = ordinal;
        }
    }
}

impl DocHost {
    /// This chat's relay state. It outlives a room session: an idle host's socket
    /// closes and reopens with each turn, and its mode and links carry over, or
    /// every reopening would send a burst of stored rows.
    pub(super) fn relay_state_for(
        &self,
        handle: &Weak<ChatDocHandle>,
        device: &str,
        sink: Arc<dyn zeron_sync::chat_client::ChatDocSink>,
    ) -> Option<Arc<ChatRelayState>> {
        let handle = handle.upgrade()?;
        let state = lock(&handle.relay)
            .get_or_insert_with(|| {
                let state = ChatRelayState::new(RelayHandle::spawn(device, sink, RelayPolicy::default()));
                // Replayed when this handle opened: covered by its next save.
                if let Ok((_, newest)) = self.inner.store.pending_chat_inbox(&handle.chat_id) {
                    lock(&state.journal).newest = newest;
                }
                state
            })
            .clone();
        self.refresh_relay_mode(&handle, &state);
        Some(state)
    }

    /// Start one chat's relay duties. Called when its client is admitted.
    pub(super) fn spawn_relay_maintenance(
        &self,
        handle: &Arc<ChatDocHandle>,
        state: Arc<ChatRelayState>,
        cancel: CancellationToken,
    ) {
        let host = self.clone();
        let weak = Arc::downgrade(handle);
        self.spawn_sync_worker(cancel, async move {
            let mut events = state.relay.events();
            let mut tick = tokio::time::interval(MAINTENANCE_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_refresh: Option<Instant> = None;
            let mut last_room = None;
            let mut link_retry = LINK_RETRY_MIN;
            let mut next_link = Instant::now();
            let mut last_commits = 0u64;
            let mut last_commit_at = Instant::now();
            loop {
                tokio::select! {
                    event = events.recv() => {
                        let Some(handle) = weak.upgrade() else { return };
                        if matches!(event, Ok(RelayEvent::LegacyPeer)) {
                            *lock(&state.legacy_until) = Some(Instant::now() + LEGACY_HOLD);
                            host.refresh_relay_mode(&handle, &state);
                            last_refresh = Some(Instant::now());
                        }
                    }
                    _ = tick.tick() => {
                        let Some(handle) = weak.upgrade() else { return };
                        let now = Instant::now();
                        let room = lock(&handle.chat2).as_ref().and_then(|c| c.relay_answer());
                        if room != last_room || last_refresh.is_none_or(|at| now - at >= MODE_REFRESH) {
                            // The room coming back means the network did: try now,
                            // not at the end of a long backoff.
                            if room.is_some() && last_room.is_none() {
                                next_link = now;
                                link_retry = LINK_RETRY_MIN;
                            }
                            last_room = room;
                            last_refresh = Some(now);
                            host.refresh_relay_mode(&handle, &state);
                        }
                        let saved = handle.persistence.as_ref().is_some_and(|p| p.is_clean());
                        for id in state.retirable(saved) {
                            if let Err(err) = host.inner.store.acknowledge_chat_update(&handle.chat_id, &id) {
                                tracing::warn!(%err, "chat relay: outbox retirement failed");
                            }
                        }
                        if let Some(through) = state.retirable_journal(saved)
                            && let Err(err) = host.inner.store.retire_chat_inbox(&handle.chat_id, through)
                        {
                            tracing::warn!(%err, "chat relay: journal retirement failed");
                        }
                        if !state.caught_up()
                            && lock(&handle.chat2)
                                .as_ref()
                                .is_some_and(|client| client.caught_up() && client.stats().server_known)
                        {
                            state.caught_up.store(true, Ordering::Release);
                        }

                        if state.heal_owed.load(Ordering::Acquire) {
                            host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
                        }

                        let commits = state.commits.load(Ordering::Relaxed);
                        if commits != last_commits {
                            last_commits = commits;
                            last_commit_at = now;
                        }
                        if state.relaying() && crate::runner::session().is_some() {
                            host.cloud_checkpoint_due(&handle, &state, now, last_commit_at);
                        }

                        if host.wants_chat_link(&handle) {
                            if lock(&state.link).is_none() && now >= next_link {
                                next_link = now + link_retry;
                                link_retry = (link_retry * 2).min(LINK_RETRY_MAX);
                                host.spawn_chat_link(&handle, &state);
                            }
                            if state.relay.stats().links > 0 {
                                link_retry = LINK_RETRY_MIN;
                            }
                        } else {
                            state.stop_link();
                        }
                    }
                }
            }
        });
    }

    /// Decide what this device does for this chat now. The one place the
    /// generation flips, and the one place a host stops relaying.
    pub(super) fn refresh_relay_mode(&self, handle: &Arc<ChatDocHandle>, state: &ChatRelayState) {
        let Some(workspace) = self.workspace() else {
            return;
        };
        let chat_id = &handle.chat_id;
        let Ok(Some(chat)) = workspace.chat(chat_id) else {
            return;
        };
        let (answer, socketless, room_dirty) = lock(&handle.chat2).as_ref().map_or(
            (None, false, false),
            |client| (client.relay_answer(), client.socketless(), client.room_relay_dirty()),
        );
        // Only a room that answered can say it has no relay, and between sessions
        // the last answer stands: a host offline for a day shouldn't owe the edge a
        // day of stored rows when it returns. A network with HTTPS and no sockets
        // can't relay at all, so everything has to go through stored rows.
        if let Some(offered) = answer {
            state.room_offered.store(offered, Ordering::Release);
        } else if socketless {
            state.room_offered.store(false, Ordering::Release);
        }
        let offered = state.room_offered.load(Ordering::Acquire);
        let is_host = workspace.is_host(chat_id);
        let mut generation = chat.room_gen.unwrap_or(1);
        let parent_host = chat
            .parent_chat_id
            .as_deref()
            .and_then(|parent| workspace.chat(parent).ok().flatten())
            .map(|parent| parent.device_id);
        // A chat close to the room's checkpoint limit can't keep the promise that a
        // checkpoint can always bring a reader up to date.
        let small_enough = handle
            .persistence
            .as_ref()
            .is_none_or(|saved| saved.snapshot_bytes() <= RELAY_SNAPSHOT_LIMIT);
        let allowed = small_enough
            && workspace.read_devices_seen().is_ok_and(|devices| {
                relay_allowed(&devices, &chat.device_id, parent_host.as_deref(), chrono::Utc::now())
            });
        // The registry's list of clients without the layer has been heard.
        let legacy_known = zeron_sync::registry::legacy_clients().is_some();

        // Moving on: only a room that has just said yes, and only once every
        // device can follow.
        if is_host && generation == STORED_ROOM_GEN && allowed && legacy_known && answer == Some(true) {
            match workspace.set_chat_room_gen(chat_id, RELAY_ROOM_GEN) {
                Ok(_) => {
                    tracing::info!(chat = %chat_id, "chat relay: moved to generation 3");
                    generation = RELAY_ROOM_GEN;
                }
                Err(err) => tracing::warn!(chat = %chat_id, %err, "chat relay: generation write failed"),
            }
        }

        let holding_for_legacy = lock(&state.legacy_until).is_some_and(|until| Instant::now() < until);
        let relaying = decide_relaying(is_host, offered, generation, allowed, holding_for_legacy);
        state.hosting.store(is_host, Ordering::Release);
        tracing::debug!(chat = %chat_id, is_host, ?answer, offered, generation, allowed, holding_for_legacy, relaying, "chat relay: mode");
        // Mode first, checkpoint second. Reversed, a commit between the two would
        // be in neither the checkpoint nor the rows, and every old reader would
        // park all that followed it. Owed even when this process never relayed:
        // a restarted host can't know what it relayed before.
        if state.relaying.swap(relaying, Ordering::AcqRel) && !relaying {
            state.mode_epoch.fetch_add(1, Ordering::AcqRel);
        }
        state.relay.set_hello(generation >= RELAY_ROOM_GEN, relaying);
        if relaying {
            state.relayed_here.store(true, Ordering::Release);
            state.healed.store(false, Ordering::Release);
            state.heal_owed.store(false, Ordering::Release);
        } else if is_host
            && (generation >= RELAY_ROOM_GEN || room_dirty || state.relayed_here.load(Ordering::Acquire))
            && !state.healed.load(Ordering::Acquire)
        {
            tracing::info!(chat = %chat_id, "chat relay: storing; a checkpoint is owed");
            state.heal_owed.store(true, Ordering::Release);
            self.spawn_chat2_checkpoint(handle, HEAL_REASON);
        }

        // Moving back: only after that checkpoint landed. A crash before it
        // leaves the chat on 3, and the next start owes the same checkpoint.
        if is_host
            && generation >= RELAY_ROOM_GEN
            && !allowed
            && state.healed.load(Ordering::Acquire)
        {
            match workspace.set_chat_room_gen(chat_id, STORED_ROOM_GEN) {
                Ok(_) => {
                    tracing::info!(chat = %chat_id, "chat relay: moved back to generation 2");
                    generation = STORED_ROOM_GEN;
                }
                Err(err) => tracing::warn!(chat = %chat_id, %err, "chat relay: generation write failed"),
            }
        }

        // Only into a queue the host can read: a host back on an older build keeps
        // the generation it wrote, but its device row no longer says it can.
        let host_reads_queue = workspace.read_devices().is_ok_and(|devices| {
            devices.iter().any(|device| {
                device.id == chat.device_id && device.supports(zeron_proto::relay_gen::CHAT_RELAY_CAPABILITY)
            })
        });
        if let Some(client) = &*lock(&handle.chat2) {
            client.set_queue_pushes(!is_host && offered && generation >= RELAY_ROOM_GEN && host_reads_queue);
        }
        state.relay.set_policy(RelayPolicy { confirm_queue: is_host });
        state.relay.set_host(Some(chat.device_id));
    }

    fn cloud_checkpoint_due(
        &self,
        handle: &Arc<ChatDocHandle>,
        state: &ChatRelayState,
        now: Instant,
        last_commit_at: Instant,
    ) {
        let version = handle.doc.doc().oplog_vv();
        if state.checkpoint_covers(&version) {
            return;
        }
        let last_at = lock(&state.checkpointed).as_ref().map(|(_, at)| *at);
        let since = last_at.map_or(Duration::MAX, |at| now - at);
        let turn_ended = now - last_commit_at >= CLOUD_QUIET && since >= CLOUD_MIN_GAP;
        if turn_ended || since >= CLOUD_MAX_GAP {
            self.spawn_chat2_checkpoint(handle, "relay-cloud");
        }
    }

    /// Before a Cloud sleep: the room must hold every relaying chat's latest
    /// state, because it's all anyone can read until the machine wakes.
    pub(super) async fn checkpoint_relayed_chats_for_sleep(&self) -> Result<(), String> {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let Some(state) = lock(&handle.relay).clone().filter(|s| s.relaying()) else {
                continue;
            };
            // Over the room's limit: no checkpoint will ever land, and refusing to
            // sleep would only keep the machine billing.
            if state.too_large.load(Ordering::Acquire) {
                tracing::warn!(chat = %handle.chat_id, "chat relay: sleeping with a chat too big to checkpoint");
                continue;
            }
            let deadline = Instant::now() + SLEEP_CHECKPOINT_WAIT;
            while !state.checkpoint_covers(&handle.doc.doc().oplog_vv()) {
                if Instant::now() >= deadline {
                    return Err(format!("chat {} couldn't save its checkpoint", handle.chat_id));
                }
                self.spawn_chat2_checkpoint(&handle, "relay-sleep");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    fn wants_chat_link(&self, handle: &Arc<ChatDocHandle>) -> bool {
        if handle.messages_tx.receiver_count() == 0 || self.inner.chat_links.get().is_none() {
            return false;
        }
        let Some(workspace) = self.workspace() else {
            return false;
        };
        let Ok(Some(chat)) = workspace.chat(&handle.chat_id) else {
            return false;
        };
        chat.room_gen.unwrap_or(1) >= RELAY_ROOM_GEN && !workspace.is_host(&handle.chat_id)
    }

    fn spawn_chat_link(&self, handle: &Arc<ChatDocHandle>, state: &Arc<ChatRelayState>) {
        let Some(previews) = self.inner.chat_links.get().cloned() else {
            return;
        };
        let Some(host_device) = self.remote_host_for(&handle.chat_id) else {
            return;
        };
        let stop = CancellationToken::new();
        *lock(&state.link) = Some(stop.clone());
        let state = state.clone();
        let service = format!("{CHAT_LINK_PREFIX}{}", handle.chat_id);
        let chat = handle.chat_id.clone();
        self.spawn_worker(async move {
            match open_within(LINK_OPEN_LIMIT, previews.open_peer_stream(&host_device, &service)).await {
                LinkOpen::Open(stream) => {
                    tracing::info!(chat = %chat, device = %host_device, "chat link up");
                    run_link(stream, host_device.clone(), state.relay.clone(), stop.clone()).await;
                    // Nobody asked it to end: the connection under it is dead or stuck.
                    // It can look open for minutes, and reusing it would stay on the
                    // room long after the network has recovered.
                    if !stop.is_cancelled() {
                        tracing::info!(chat = %chat, device = %host_device, "chat link lost; pairing afresh next time");
                        previews.reset_peer(&host_device).await;
                    }
                }
                LinkOpen::Stuck => {
                    tracing::info!(chat = %chat, device = %host_device, "chat link open stuck; pairing afresh next time");
                    previews.reset_peer(&host_device).await;
                }
                LinkOpen::Refused(error) => {
                    // The room carries this chat. The maintenance tick retries.
                    tracing::debug!(chat = %chat, %error, "chat link unavailable");
                }
            }
            *lock(&state.link) = None;
        });
    }

    /// This build has the layer turned off and hosts a chat a build with it on left
    /// on generation 3. Rows it stores next would depend on what was only relayed,
    /// so it owes the same checkpoint any host that stops relaying does, then moves
    /// the chat back. Only reachable for such a chat: an account that never ran the
    /// layer has none.
    pub(super) fn spawn_rollback_heal(&self, handle: &Arc<ChatDocHandle>, cancel: CancellationToken) {
        let owed = |host: &DocHost, chat: &str| {
            host.workspace().is_some_and(|workspace| {
                workspace.is_host(chat)
                    && matches!(workspace.chat(chat), Ok(Some(row)) if row.room_gen.unwrap_or(1) >= RELAY_ROOM_GEN)
            })
        };
        if !owed(self, &handle.chat_id) {
            return;
        }
        let host = self.clone();
        let weak = Arc::downgrade(handle);
        self.spawn_sync_worker(cancel, async move {
            let mut wait = CHECKPOINT_RETRY_MIN;
            loop {
                tokio::time::sleep(wait).await;
                let Some(handle) = weak.upgrade() else { return };
                if !owed(&host, &handle.chat_id) {
                    return;
                }
                let ready = lock(&handle.chat2)
                    .as_ref()
                    .is_some_and(|client| client.caught_up() && client.stats().server_known);
                if ready {
                    tracing::info!(chat = %handle.chat_id, "chat relay: layer off; healing a generation 3 chat");
                    host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
                    wait = (wait * 2).min(CHECKPOINT_RETRY_MAX);
                }
            }
        });
    }

    pub(super) fn finish_rollback_heal(&self, chat: &str) {
        if let Some(workspace) = self.workspace() {
            match workspace.set_chat_room_gen(chat, STORED_ROOM_GEN) {
                Ok(_) => tracing::info!(chat = %chat, "chat relay: layer off; moved back to generation 2"),
                Err(err) => tracing::warn!(chat = %chat, %err, "chat relay: generation write failed"),
            }
        }
    }

    /// Keep the service that opens direct links. Only with the flag on.
    pub fn set_chat_links(&self, previews: zeron_preview::PreviewService) {
        let _ = self.inner.chat_links.set(previews);
    }

    fn accept_chat_link(&self, chat: &str) -> anyhow::Result<Arc<ChatRelayState>> {
        let hosted = self.workspace().is_some_and(|workspace| {
            workspace.is_host(chat) && matches!(workspace.chat(chat), Ok(Some(_)))
        });
        if !hosted {
            anyhow::bail!("this device doesn't host that chat");
        }
        let handle = self.open(chat).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        lock(&handle.relay)
            .clone()
            .ok_or_else(|| anyhow::anyhow!("that chat isn't ready for a link yet"))
    }
}

enum LinkOpen<T> {
    Open(T),
    /// The connection answered no: this service isn't available there.
    Refused(anyhow::Error),
    /// No answer at all in time.
    Stuck,
}

async fn open_within<T>(
    limit: Duration,
    open: impl std::future::Future<Output = anyhow::Result<T>>,
) -> LinkOpen<T> {
    match tokio::time::timeout(limit, open).await {
        Ok(Ok(stream)) => LinkOpen::Open(stream),
        Ok(Err(error)) if error.downcast_ref::<tokio::time::error::Elapsed>().is_some() => LinkOpen::Stuck,
        Ok(Err(error)) => LinkOpen::Refused(error),
        Err(_) => LinkOpen::Stuck,
    }
}

/// Whether a device publishes a chat's ops by relay. Everything must agree:
/// any doubt means stored rows, which every reader can follow.
fn decide_relaying(
    is_host: bool,
    room_offers: bool,
    generation: u32,
    every_device_follows: bool,
    holding_for_legacy: bool,
) -> bool {
    is_host && room_offers && generation >= RELAY_ROOM_GEN && every_device_follows && !holding_for_legacy
}

/// Whether a device may open a chat link to this host. Cloud machines run agent
/// code, so a link stays between the owner's own devices.
fn link_peer_allowed(peer: Option<&str>) -> bool {
    peer.is_some_and(|peer| !zeron_proto::is_cloud_device(peer, ""))
}

/// `chatlink/{chatId}`: another device on the account reaching this chat's host.
pub struct ChatLinkService {
    host: Weak<DocHostInner>,
}

impl ChatLinkService {
    pub fn new(host: &DocHost) -> Self {
        Self { host: Arc::downgrade(&host.inner) }
    }
}

#[async_trait::async_trait]
impl zeron_preview::mux::Connector for ChatLinkService {
    async fn connect(&self, service: &str) -> anyhow::Result<zeron_preview::mux::BoxIo> {
        self.connect_from(None, service).await
    }

    async fn connect_from(
        &self,
        peer: Option<&str>,
        service: &str,
    ) -> anyhow::Result<zeron_preview::mux::BoxIo> {
        if !link_peer_allowed(peer) {
            anyhow::bail!("chat links are only open to the owner's devices");
        }
        let peer = peer.unwrap_or_default().to_string();
        let chat = service
            .strip_prefix(CHAT_LINK_PREFIX)
            .ok_or_else(|| anyhow::anyhow!("not a chat link"))?;
        let inner = self.host.upgrade().ok_or_else(|| anyhow::anyhow!("engine stopped"))?;
        let host = DocHost { inner };
        let state = host.accept_chat_link(chat)?;
        let (ours, theirs) = tokio::io::duplex(256 * 1024);
        let relay = state.relay.clone();
        let stop = state.accepted.child_token();
        host.spawn_worker(async move {
            run_link(ours, peer, relay, stop).await;
        });
        Ok(Box::new(theirs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<ChatRelayState> {
        struct NoDoc;
        impl zeron_sync::chat_client::ChatDocSink for NoDoc {
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
        }
        ChatRelayState::new(RelayHandle::spawn("device", Arc::new(NoDoc), RelayPolicy::default()))
    }

    #[tokio::test]
    async fn a_relayed_commit_is_retired_only_a_tick_after_a_save() {
        let state = state();
        state.relaying.store(true, Ordering::Release);
        assert!(state.publish_local("a", b"x"), "relayed, so no stored row");
        // Listed this tick: a save now may not cover it.
        assert!(state.retirable(true).is_empty());
        state.publish_local("b", b"x");
        assert_eq!(state.retirable(false), Vec::<String>::new(), "not saved yet");
        assert_eq!(state.retirable(true), vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn a_join_doesnt_upload_what_the_doc_already_holds() {
        let state = state();
        state.relaying.store(true, Ordering::Release);
        state.publish_local("relayed", b"x");
        state.relaying.store(false, Ordering::Release);
        assert!(!state.publish_local("stored", b"x"), "storing: it goes to rows");
        let pending = vec![
            ("relayed".to_string(), vec![1]),
            ("stored".to_string(), vec![2]),
            ("from-before-a-crash".to_string(), vec![3]),
        ];
        let ids: Vec<String> = state.uploadable(pending).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["stored".to_string(), "from-before-a-crash".to_string()]);
    }

    #[test]
    fn the_journal_retires_only_what_a_later_save_covers() {
        let state = runtime_state();
        lock(&state.journal).newest = 7;
        assert_eq!(state.retirable_journal(true), None, "listed this tick: a save now may not cover it");
        lock(&state.journal).newest = 9;
        assert_eq!(state.retirable_journal(false), None, "not saved yet");
        assert_eq!(state.retirable_journal(true), Some(7));
        assert_eq!(state.retirable_journal(false), None, "9 is listed now");
        assert_eq!(state.retirable_journal(true), Some(9));
        assert_eq!(state.retirable_journal(true), None);
    }

    fn runtime_state() -> Arc<ChatRelayState> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _guard = runtime.enter();
        let state = state();
        std::mem::forget(runtime);
        state
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_that_never_answers_gives_up() {
        let open = open_within::<()>(LINK_OPEN_LIMIT, std::future::pending());
        assert!(matches!(open.await, LinkOpen::Stuck));
        let elapsed = tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>())
            .await
            .unwrap_err();
        let timed_out = open_within::<()>(LINK_OPEN_LIMIT, async move { Err(elapsed.into()) });
        assert!(matches!(timed_out.await, LinkOpen::Stuck), "its ready wait timed out");
        let refused = open_within::<()>(LINK_OPEN_LIMIT, async { anyhow::bail!("no such chat") });
        assert!(matches!(refused.await, LinkOpen::Refused(_)));
    }

    #[tokio::test]
    async fn accepted_links_end_with_the_chats_handle() {
        let state = state();
        let link = state.accepted.child_token();
        assert!(!link.is_cancelled());
        drop(state);
        assert!(link.is_cancelled());
    }

    /// Answers every checkpoint POST and counts them.
    async fn edge_counting_checkpoints() -> (String, Arc<AtomicU64>) {
        let posts = Arc::new(AtomicU64::new(0));
        let counted = posts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let counted = counted.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0; 256 * 1024];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buffer[..read]).to_string();
                    if head.starts_with("POST") && head.contains("/checkpoint") {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                    let body = "{\"ok\":true}";
                    let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        (url, posts)
    }

    fn host_on(url: &str, dir: &std::path::Path) -> (DocHost, Arc<zeron_sync::DocsStore>) {
        let store = Arc::new(zeron_sync::DocsStore::open(dir).unwrap());
        let host = DocHost::new(
            store.clone(),
            crate::doc_host::DocHostConfig {
                device_id: "host".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: Some(crate::doc_host::EdgeConfig::with_static_token(url, "test")),
            },
        );
        (host, store)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checkpoint_waits_until_this_process_has_caught_up() {
        let (url, posts) = edge_counting_checkpoints().await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = state();
        *lock(&handle.relay) = Some(state.clone());
        for reason in [HEAL_REASON, "relay-cloud", "relay-sleep"] {
            host.spawn_chat2_checkpoint(&handle, reason);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(posts.load(Ordering::SeqCst), 0, "a doc this empty could replace everything");
        state.caught_up.store(true, Ordering::Release);
        host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while posts.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(posts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn what_came_by_relay_survives_a_crash_before_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            crate::doc_host::DocHostConfig {
                device_id: "viewer".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let workspace = crate::workspace_host::WorkspaceHost::open(
            store.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "viewer".into(),
                device_name: "viewer".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        workspace.create_chat("chat", None, Some("host"), None, None).unwrap();
        let mut row = workspace.chat("chat").unwrap().unwrap();
        row.room_gen = Some(RELAY_ROOM_GEN);
        workspace.import_chat_row(&row).unwrap();
        host.set_workspace(workspace);

        // Bytes that only ever arrived by relay: kept, never saved in a snapshot.
        let remote = loro::LoroDoc::new();
        remote.get_text("relayed").insert(0, "kept").unwrap();
        remote.commit();
        let update = remote.export(loro::ExportMode::all_updates()).unwrap();
        store.journal_chat_inbox("chat", &update).unwrap();

        let handle = host.open_local("chat").unwrap();
        assert_eq!(handle.doc.doc().get_text("relayed").to_string(), "kept");
        let (_, newest) = store.pending_chat_inbox("chat").unwrap();
        store.retire_chat_inbox("chat", newest).unwrap();
        assert!(store.pending_chat_inbox("chat").unwrap().0.is_empty());
    }

    #[test]
    fn any_doubt_means_stored_rows() {
        assert!(decide_relaying(true, true, RELAY_ROOM_GEN, true, false));
        assert!(!decide_relaying(false, true, RELAY_ROOM_GEN, true, false), "not the host");
        assert!(!decide_relaying(true, false, RELAY_ROOM_GEN, true, false), "the room doesn't offer it");
        assert!(!decide_relaying(true, true, STORED_ROOM_GEN, true, false), "the chat is on stored rows");
        assert!(!decide_relaying(true, true, RELAY_ROOM_GEN, false, false), "a device can't follow");
        assert!(!decide_relaying(true, true, RELAY_ROOM_GEN, true, true), "an old reader is here");
    }

    #[test]
    fn a_link_stays_between_the_owners_devices() {
        assert!(link_peer_allowed(Some("laptop")));
        assert!(!link_peer_allowed(None));
        let cloud = format!("{}machine", zeron_proto::cloud::CLOUD_DEVICE_PREFIX);
        assert!(!link_peer_allowed(Some(&cloud)));
    }
}
