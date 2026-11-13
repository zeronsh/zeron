//! The engine's side of the relay layer (docs/design/relay-layer.md). On against
//! every edge unless `ZERON_RELAY_LAYER` turns it off ([`relay_layer_on`]). On, the
//! edge decides who relays, and a room that doesn't offer it (an edge without the
//! layer, or with its switch off) leaves everything on stored rows. Off, nothing here runs: no chat handle carries relay state, no
//! capability is advertised and no stream service is registered.
//!
//! The safety rule lives in [`DocHost::refresh_relay_mode`]: a host that stops
//! relaying sets its mode first and posts a checkpoint exported after that, so
//! no commit can fall between the checkpoint and the stored rows that follow.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zeron_proto::relay_gen::{RELAY_ROOM_GEN, STORED_ROOM_GEN};
use zeron_sync::chat_relay::{RelayEvent, RelayHandle, RelayPolicy};

use super::{ChatDocHandle, DocHost, lock};

/// `spawn_chat2_checkpoint`'s reason for the checkpoint a stop owes.
pub(super) const HEAL_REASON: &str = "relay-heal";

const MAINTENANCE_TICK: Duration = Duration::from_secs(1);
/// How often a host re-checks the room's answer and the chat's generation.
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
/// A snapshot past this stays on stored rows, or goes back to them. The room takes
/// checkpoints up to 32 MiB (`MAX_CHECKPOINT_BYTES`), and a chat over that can't
/// heal, can't bring an old reader up to date and can't let a Cloud machine sleep.
/// The gap is the room to grow while the heal lands.
const RELAY_SNAPSHOT_LIMIT: usize = 24 * 1024 * 1024;

/// The engine-wide switch, read once: [`relay_layer_on`] for this engine's
/// `ZERON_RELAY_LAYER`.
pub fn relay_layer_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| relay_layer_on(std::env::var("ZERON_RELAY_LAYER").ok().as_deref()))
}

/// Whether an engine runs the relay layer: yes, whatever its edge, unless
/// `ZERON_RELAY_LAYER` is set to anything but `1` (the kill switch). Whether a chat
/// relays is still the room's call: an edge that doesn't offer the layer leaves
/// every chat on stored rows, exactly as before it.
pub fn relay_layer_on(flag: Option<&str>) -> bool {
    flag.is_none_or(|flag| flag == "1")
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
    /// This process has seen its doc catch up with the room. Before that, its
    /// export may hold less than the checkpoint it would replace.
    caught_up: AtomicBool,
    /// This process has relayed this chat.
    relayed_here: AtomicBool,
    /// The room refused a checkpoint as too big, and it would again.
    too_large: AtomicBool,
    journal: Mutex<Journal>,
    /// What the heal this host owes must cover: its doc's version when it stopped
    /// relaying, which holds every op it relayed. What it commits after that goes
    /// to stored rows.
    heal_version: Mutex<Option<loro::VersionVector>>,
    /// The edge takes heal claims. Off once it says it doesn't know them.
    heal_claims: AtomicBool,
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
            caught_up: AtomicBool::new(false),
            relayed_here: AtomicBool::new(false),
            too_large: AtomicBool::new(false),
            journal: Mutex::new(Journal::default()),
            heal_version: Mutex::new(None),
            heal_claims: AtomicBool::new(true),
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
    /// process still holds in its doc. A host in the relay era stores none until it
    /// knows how: relaying, they're relayed (`adopt_unrelayed`); stopping, they wait
    /// for the heal. After a crash the outbox holds commits that were relayed, and as
    /// stored rows they'd depend on ops no checkpoint holds yet, so old readers would
    /// park on them. A chat too big to heal can't wait for one.
    pub(crate) fn uploadable(&self, pending: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
        if self.holds_uploads() {
            return Vec::new();
        }
        let unretired = lock(&self.unretired);
        pending
            .into_iter()
            .filter(|(id, _)| !unretired.ids.contains(id))
            .collect()
    }

    fn holds_uploads(&self) -> bool {
        self.hosting.load(Ordering::Acquire)
            && (self.relaying()
                || (self.heal_owed.load(Ordering::Acquire)
                    && !self.healed.load(Ordering::Acquire)
                    && !self.refused_as_too_large()))
    }

    /// Outbox entries a relaying host hasn't relayed in this process: commits from
    /// before a restart. They go out as relayed commits and retire after a save.
    pub(super) fn adopt_unrelayed(&self, pending: Vec<(String, Vec<u8>)>) -> usize {
        if !self.relaying() {
            return 0;
        }
        let mut unretired = lock(&self.unretired);
        let mut adopted = 0;
        for (id, bytes) in pending {
            if !unretired.ids.contains(&id) {
                self.relay.local_update(bytes, true);
                unretired.ids.push(id);
                adopted += 1;
            }
        }
        adopted
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

    /// The version a heal claim names, if the edge takes claims.
    pub(super) fn heal_claim(&self) -> Option<loro::VersionVector> {
        if !self.heal_claims.load(Ordering::Acquire) {
            return None;
        }
        lock(&self.heal_version).clone()
    }

    /// The room's answer to a heal claim. `epoch`: as for [`Self::checkpoint_landed`],
    /// sampled before the claim; a host that has relayed since owes a new heal.
    pub(super) fn heal_claim_answered(&self, answer: crate::chat2_checkpoint::HealClaim, epoch: u64) {
        use crate::chat2_checkpoint::HealClaim;
        match answer {
            HealClaim::Healed if epoch == self.mode_epoch() && !self.relaying() => {
                self.heal_owed.store(false, Ordering::Release);
                self.healed.store(true, Ordering::Release);
            }
            HealClaim::Unsupported => self.heal_claims.store(false, Ordering::Release),
            _ => {}
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

    /// The room refused this chat's checkpoint as too big. A doc only grows, so asking
    /// again would only send it all again.
    pub(super) fn refused_as_too_large(&self) -> bool {
        self.too_large.load(Ordering::Acquire)
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

    pub(crate) fn note_journaled(&self, ordinal: i64) {
        if let Some(state) = &*lock(&self.relay) {
            lock(&state.journal).newest = ordinal;
        }
    }
}

impl DocHost {
    /// This chat's relay state. It outlives a room session: an idle host's socket
    /// closes and reopens with each turn, and its mode carries over, or
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
            let mut last_commits = 0u64;
            let mut last_commit_at = Instant::now();
            loop {
                tokio::select! {
                    event = events.recv() => {
                        let Some(handle) = weak.upgrade() else { return };
                        if matches!(event, Ok(RelayEvent::LegacyPeer)) {
                            *lock(&state.legacy_until) = Some(Instant::now() + legacy_hold());
                            host.refresh_relay_mode(&handle, &state);
                            last_refresh = Some(Instant::now());
                        }
                    }
                    _ = tick.tick() => {
                        let Some(handle) = weak.upgrade() else { return };
                        let now = Instant::now();
                        let room = lock(&handle.chat2).as_ref().and_then(|c| c.relay_answer());
                        if room != last_room || last_refresh.is_none_or(|at| now - at >= MODE_REFRESH) {
                            last_room = room;
                            last_refresh = Some(now);
                            host.refresh_relay_mode(&handle, &state);
                        }
                        if state.relaying()
                            && let Ok(pending) = host.inner.store.pending_chat_updates(&handle.chat_id)
                            && !pending.is_empty()
                        {
                            let adopted = state.adopt_unrelayed(pending);
                            if adopted > 0 {
                                tracing::info!(chat = %handle.chat_id, adopted, "chat relay: relayed commits left from before a restart");
                            }
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
        // A chat close to the room's checkpoint limit can't keep the promise that a
        // checkpoint can always bring a reader up to date.
        let size = handle_size(handle);
        let small_enough = size.is_none_or(|bytes| bytes <= relay_snapshot_limit());

        // Moving on: only a room that has just said yes. Clients without the layer
        // don't hold this back: `generation >= 2` is all they read, and when one
        // opens this chat the room says so and this host stores while it's there.
        // Not before this chat's size is known, either from what was loaded or from a
        // save: until its first save, a chat opened from rows reads as empty.
        if is_host
            && generation == STORED_ROOM_GEN
            && small_enough
            && size.is_some()
            && answer == Some(true)
        {
            match workspace.set_chat_room_gen(chat_id, RELAY_ROOM_GEN) {
                Ok(_) => {
                    tracing::info!(chat = %chat_id, "chat relay: moved to generation 3");
                    generation = RELAY_ROOM_GEN;
                }
                Err(err) => tracing::warn!(chat = %chat_id, %err, "chat relay: generation write failed"),
            }
        }

        let holding_for_legacy = lock(&state.legacy_until).is_some_and(|until| Instant::now() < until);
        let relaying = decide_relaying(is_host, offered, generation, small_enough, holding_for_legacy);
        state.hosting.store(is_host, Ordering::Release);
        tracing::debug!(chat = %chat_id, is_host, ?answer, offered, generation, small_enough, holding_for_legacy, relaying, "chat relay: mode");
        // Mode first, checkpoint second. Reversed, a commit between the two would
        // be in neither the checkpoint nor the rows, and every old reader would
        // park all that followed it. Owed even when this process never relayed:
        // a restarted host can't know what it relayed before.
        if state.relaying.swap(relaying, Ordering::AcqRel) && !relaying {
            state.mode_epoch.fetch_add(1, Ordering::AcqRel);
            // Read after the mode changed: every commit relayed is in it.
            *lock(&state.heal_version) = Some(handle.doc.doc().oplog_vv());
        }
        state.relay.set_hello(generation >= RELAY_ROOM_GEN, relaying);
        if relaying {
            state.relayed_here.store(true, Ordering::Release);
            state.healed.store(false, Ordering::Release);
            state.heal_owed.store(false, Ordering::Release);
            *lock(&state.heal_version) = None;
        } else if is_host
            && (generation >= RELAY_ROOM_GEN || room_dirty || state.relayed_here.load(Ordering::Acquire))
            && !state.healed.load(Ordering::Acquire)
        {
            tracing::info!(chat = %chat_id, "chat relay: storing; a checkpoint is owed");
            // Not relayed in this process (a restart): what this host may have relayed
            // before is all in its doc now.
            lock(&state.heal_version).get_or_insert_with(|| handle.doc.doc().oplog_vv());
            state.heal_owed.store(true, Ordering::Release);
            self.spawn_chat2_checkpoint(handle, HEAL_REASON);
        }

        // Moving back: only after that checkpoint landed. A crash before it
        // leaves the chat on 3, and the next start owes the same checkpoint.
        if is_host
            && generation >= RELAY_ROOM_GEN
            && !small_enough
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
        // On the relay handle, not the client: a client that dials and pushes before
        // the first maintenance tick would otherwise store a send as a row. The room's
        // answer for each session still decides (`push_kind`).
        state
            .relay
            .set_queue_pushes(!is_host && offered && generation >= RELAY_ROOM_GEN && host_reads_queue);
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

    /// Stopping (a quit, an update's restart, a service stop): a relaying chat's
    /// newest changes may be only on this machine and the devices that saw them,
    /// and a device opening the chat before this one is back would get an older
    /// checkpoint. Save what's owed, for at most `budget`: a stop never waits
    /// longer, and a checkpoint that doesn't land is still owed on the next run.
    pub async fn checkpoint_relayed_chats_before_stop(&self, budget: Duration) {
        let owed = |handle: &Arc<ChatDocHandle>| {
            lock(&handle.relay).clone().is_some_and(|state| {
                state.relaying()
                    && !state.too_large.load(Ordering::Acquire)
                    && !state.checkpoint_covers(&handle.doc.doc().oplog_vv())
            })
        };
        let mut waiting: Vec<_> = lock(&self.inner.handles)
            .values()
            .filter(|handle| owed(handle))
            .cloned()
            .collect();
        if waiting.is_empty() {
            return;
        }
        let total = waiting.len();
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            waiting.retain(|handle| owed(handle));
            if waiting.is_empty() {
                tracing::info!(chats = total, "chat relay: checkpoints saved before stopping");
                return;
            }
            for handle in &waiting {
                self.spawn_chat2_checkpoint(handle, "relay-sleep");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!(chats = waiting.len(), "chat relay: stopping with checkpoints still owed");
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
}

/// Whether this device may post a checkpoint for a chat. On generation 3 only the
/// host does: a viewer's ordinary checkpoint (a rejected push, a row threshold) is
/// behind the host's as often as not, and it can be all the room holds.
pub(super) fn posts_checkpoint(reason: &str, room_gen: u32, is_host: bool) -> bool {
    is_host || room_gen < RELAY_ROOM_GEN || reason.starts_with("relay-")
}

/// [`RELAY_SNAPSHOT_LIMIT`]. A debug build takes `ZERON_RELAY_SNAPSHOT_LIMIT`, so the
/// two-engine smoke can make a chat too big to relay without making it huge.
fn legacy_hold() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("ZERON_RELAY_LEGACY_HOLD_SECS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_secs(secs);
    }
    LEGACY_HOLD
}

fn relay_snapshot_limit() -> usize {
    #[cfg(debug_assertions)]
    if let Some(limit) = std::env::var("ZERON_RELAY_SNAPSHOT_LIMIT").ok().and_then(|v| v.parse().ok()) {
        return limit;
    }
    RELAY_SNAPSHOT_LIMIT
}

fn handle_size(handle: &ChatDocHandle) -> Option<usize> {
    chat_size(
        handle.snapshot_bytes.load(Ordering::Relaxed),
        handle.persistence.as_ref().map_or(0, |saved| saved.snapshot_bytes()),
    )
}

/// A chat's snapshot size as best known: the larger of what was loaded and what the
/// last save wrote. `None` until one of them has happened.
fn chat_size(loaded: usize, saved: usize) -> Option<usize> {
    let bytes = loaded.max(saved);
    (bytes > 0).then_some(bytes)
}

/// Whether a device publishes a chat's ops by relay. Everything must agree:
/// any doubt means stored rows, which every reader can follow.
fn decide_relaying(
    is_host: bool,
    room_offers: bool,
    generation: u32,
    small_enough: bool,
    holding_for_legacy: bool,
) -> bool {
    is_host && room_offers && generation >= RELAY_ROOM_GEN && small_enough && !holding_for_legacy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_runs_by_default_or_when_asked() {
        assert!(relay_layer_on(None), "unset: on, whatever the edge");
        assert!(relay_layer_on(Some("1")));
    }

    #[test]
    fn the_layer_stays_off_only_when_switched_off() {
        assert!(!relay_layer_on(Some("0")), "the kill switch");
        assert!(!relay_layer_on(Some("")));
        assert!(!relay_layer_on(Some("true")), "only 1 means on");
        assert!(!relay_layer_on(Some("off")));
    }

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

    #[tokio::test]
    async fn a_restarted_host_relays_what_it_relayed_before_instead_of_storing_it() {
        let state = state();
        state.hosting.store(true, Ordering::Release);
        state.relaying.store(true, Ordering::Release);
        let pending = vec![("from-before-a-crash".to_string(), vec![3])];
        assert!(state.uploadable(pending.clone()).is_empty(), "never as a stored row");
        assert_eq!(state.adopt_unrelayed(pending.clone()), 1);
        assert_eq!(state.adopt_unrelayed(pending), 0, "once");
        assert!(state.retirable(true).is_empty());
        assert_eq!(state.retirable(true), vec!["from-before-a-crash".to_string()]);
    }

    #[tokio::test]
    async fn a_host_that_stopped_relaying_stores_only_after_its_heal() {
        let state = state();
        state.hosting.store(true, Ordering::Release);
        state.heal_owed.store(true, Ordering::Release);
        let pending = vec![("from-before-a-crash".to_string(), vec![3])];
        assert!(state.uploadable(pending.clone()).is_empty(), "rows before the heal would park on old readers");
        state.healed.store(true, Ordering::Release);
        assert_eq!(state.uploadable(pending.clone()).len(), 1);
        state.healed.store(false, Ordering::Release);
        state.checkpoint_refused(413, "", None);
        assert_eq!(state.uploadable(pending).len(), 1, "a chat too big to heal stores anyway");
    }

    #[test]
    fn only_the_host_checkpoints_a_relayed_chat() {
        assert!(posts_checkpoint("threshold", RELAY_ROOM_GEN, true));
        assert!(!posts_checkpoint("threshold", RELAY_ROOM_GEN, false));
        assert!(!posts_checkpoint("durable-rejection", RELAY_ROOM_GEN, false));
        assert!(posts_checkpoint("threshold", STORED_ROOM_GEN, false), "a stored chat, as before");
        assert!(posts_checkpoint(HEAL_REASON, RELAY_ROOM_GEN, false));
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
    async fn a_stop_saves_what_a_relaying_chat_owes_and_never_waits_past_its_budget() {
        let (url, posts) = edge_counting_checkpoints().await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let owed = state();
        owed.caught_up.store(true, Ordering::Release);
        owed.relaying.store(true, Ordering::Release);
        *lock(&handle.relay) = Some(owed.clone());
        let started = std::time::Instant::now();
        host.checkpoint_relayed_chats_before_stop(Duration::from_secs(5)).await;
        assert!(posts.load(Ordering::SeqCst) >= 1, "the owed checkpoint went");
        assert!(owed.checkpoint_covers(&handle.doc.doc().oplog_vv()));
        assert!(started.elapsed() < Duration::from_secs(5), "it returned once saved");
        // Nothing owed: nothing sent.
        let before = posts.load(Ordering::SeqCst);
        host.checkpoint_relayed_chats_before_stop(Duration::from_secs(5)).await;
        assert_eq!(posts.load(Ordering::SeqCst), before);

        // An edge that never answers: the stop waits out its budget and no more.
        let dir = tempfile::tempdir().unwrap();
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (host, _store) = host_on(&format!("http://{}", silent.local_addr().unwrap()), dir.path());
        let handle = host.open_local("chat").unwrap();
        let silent_state = state();
        silent_state.caught_up.store(true, Ordering::Release);
        silent_state.relaying.store(true, Ordering::Release);
        *lock(&handle.relay) = Some(silent_state.clone());
        let started = std::time::Instant::now();
        host.checkpoint_relayed_chats_before_stop(Duration::from_millis(600)).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!silent_state.checkpoint_covers(&handle.doc.doc().oplog_vv()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checkpoint_the_room_refused_as_too_big_is_not_sent_again() {
        let (url, posts) = edge_counting_checkpoints().await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = state();
        state.caught_up.store(true, Ordering::Release);
        *lock(&handle.relay) = Some(state.clone());
        state.checkpoint_refused(413, "{\"error\":\"too_large\"}", None);
        for reason in [HEAL_REASON, "relay-cloud", "relay-sleep"] {
            host.spawn_chat2_checkpoint(&handle, reason);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        assert!(crate::doc_host::checkpoint_fits(256 * 1024 * 1024));
        assert!(!crate::doc_host::checkpoint_fits(256 * 1024 * 1024 + 1));
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_viewer_knows_to_use_the_queue_before_its_first_dial() {
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
        workspace.upsert_device_row(&zeron_proto::Device {
            id: "host".into(),
            name: "host".into(),
            platform: "linux".into(),
            last_seen_at: Some(chrono::Utc::now()),
            created_at: None,
            version: None,
            cursor_sdk_version: None,
            capabilities: vec![zeron_proto::relay_gen::CHAT_RELAY_CAPABILITY.into()],
        });
        workspace.create_chat("chat", None, Some("host"), None, None).unwrap();
        let mut row = workspace.chat("chat").unwrap().unwrap();
        row.room_gen = Some(RELAY_ROOM_GEN);
        workspace.import_chat_row(&row).unwrap();
        host.set_workspace(workspace);
        let handle = host.open_local("chat").unwrap();
        let state = state();
        assert!(lock(&handle.chat2).is_none(), "no client yet");
        host.refresh_relay_mode(&handle, &state);
        assert!(state.relay.queue_pushes(), "a send goes to the queue from the first push");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_chat_loaded_from_disk_has_its_size_before_any_save() {
        let dir = tempfile::tempdir().unwrap();
        let (host, store) = host_on("http://127.0.0.1:1", dir.path());
        let doc = loro::LoroDoc::new();
        doc.get_text("body").insert(0, &"x".repeat(200_000)).unwrap();
        doc.commit();
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        store
            .save_snapshot_with_cursor("big", &snapshot, 0, crate::chat2_host::CHAT2_DOC_EPOCH)
            .unwrap();
        let handle = host.open_local("big").unwrap();
        assert_eq!(handle_size(&handle), Some(snapshot.len()), "known from the load, before a save");
    }

    #[test]
    fn a_chat_flips_only_once_its_size_is_known_and_small() {
        assert_eq!(chat_size(0, 0), None, "opened from rows, not saved yet");
        assert_eq!(chat_size(40 << 20, 0), Some(40 << 20), "loaded big, not saved yet");
        assert_eq!(chat_size(1000, 40 << 20), Some(40 << 20), "grew since it was loaded");
        assert_eq!(chat_size(40 << 20, 1000), Some(40 << 20));
        assert!(chat_size(40 << 20, 0).is_some_and(|bytes| bytes > RELAY_SNAPSHOT_LIMIT));
    }

    #[test]
    fn any_doubt_means_stored_rows() {
        assert!(decide_relaying(true, true, RELAY_ROOM_GEN, true, false));
        assert!(!decide_relaying(false, true, RELAY_ROOM_GEN, true, false), "not the host");
        assert!(!decide_relaying(true, false, RELAY_ROOM_GEN, true, false), "the room doesn't offer it");
        assert!(!decide_relaying(true, true, STORED_ROOM_GEN, true, false), "the chat is on stored rows");
        assert!(!decide_relaying(true, true, RELAY_ROOM_GEN, false, false), "too big to checkpoint");
        assert!(!decide_relaying(true, true, RELAY_ROOM_GEN, true, true), "an old reader is here");
    }

    /// What a room without the layer saw: production's edge before it, an old
    /// self-hosted edge, or one with `CHAT_RELAY_LAYER` off.
    #[derive(Default)]
    struct OldRoomSeen {
        hellos: Vec<serde_json::Value>,
        /// The pushes it stored as rows, in order.
        rows: Vec<Vec<u8>>,
        /// Frame kinds it doesn't know, each answered with `bad_frame`.
        refused: Vec<u8>,
        checkpoint_posts: usize,
    }

    /// A chat room as production's edge serves it today (edge/src/chat-room.ts on
    /// `main`): its state names no `relay`, it stores every push as a row, and it
    /// answers any frame it doesn't know with `bad_frame`. Plain-HTTP pulls are
    /// refused, so the socket does the work; checkpoint posts are counted.
    async fn room_without_the_layer() -> (String, Arc<Mutex<OldRoomSeen>>) {
        use futures::{SinkExt, StreamExt};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use zeron_sync::chat_frames::{decode, encode, frame_type};

        let seen = Arc::new(Mutex::new(OldRoomSeen::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let room = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let seen = room.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0; 64 * 1024];
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
                        let first = headers.lines().next().unwrap_or_default().to_owned();
                        let _ = stream.read(&mut buffer).await;
                        let reply: &[u8] = if first.starts_with("post") && first.contains("/checkpoint") {
                            lock(&seen).checkpoint_posts += 1;
                            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}"
                        } else {
                            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        };
                        let _ = stream.write_all(reply).await;
                        return;
                    }
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                    let mut head = 0u64;
                    while let Some(Ok(message)) = ws.next().await {
                        if let tokio_tungstenite::tungstenite::Message::Text(text) = &message {
                            if text == "ping" {
                                let _ = ws.send("pong".into()).await;
                            }
                            continue;
                        }
                        let Some(frame) = decode(&message.into_data()) else { continue };
                        let reply = match frame.kind {
                            frame_type::HELLO => {
                                lock(&seen).hellos.push(frame.header.clone());
                                encode(
                                    frame_type::STATE,
                                    &serde_json::json!({
                                        "headSeq": head, "seqFloor": 0, "checkpointSeq": 0,
                                        "checkpointSize": 0, "rowCount": head, "rowBytes": 0,
                                    }),
                                    &[],
                                )
                            }
                            frame_type::ROWS_REQ => {
                                encode(frame_type::ROWS_DONE, &serde_json::json!({ "headSeq": head }), &[])
                            }
                            frame_type::PUSH => {
                                lock(&seen).rows.push(frame.payload.to_vec());
                                head += 1;
                                encode(
                                    frame_type::ACK,
                                    &serde_json::json!({ "batchId": frame.header["batchId"], "seq": head, "dup": false }),
                                    &[],
                                )
                            }
                            frame_type::PRESENCE => continue,
                            frame_type::PROBE => {
                                encode(frame_type::PROBE_OK, &serde_json::json!({ "headSeq": head }), &[])
                            }
                            other => {
                                lock(&seen).refused.push(other);
                                encode(
                                    frame_type::ERROR,
                                    &serde_json::json!({ "code": "bad_frame", "message": format!("unexpected type {other}") }),
                                    &[],
                                )
                            }
                        };
                        if ws.send(reply.into()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (url, seen)
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The layer runs against every edge now, so an engine meets rooms that don't
    /// offer it: production's edge until it ships the layer, an old self-hosted
    /// edge, a local dev edge. There it must keep the chat on generation 2 and
    /// publish every commit as a stored row, exactly as a build without the layer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_with_the_layer_on_stays_on_stored_rows_in_a_room_without_it() {
        let _budget_guard = super::super::SYNC_TEST_LOCK.lock().await;
        assert!(relay_layer_on(None), "this is what an engine runs by default");
        let (url, seen) = room_without_the_layer().await;
        let dir = tempfile::tempdir().unwrap();
        let (host, store) = host_on(&url, dir.path());
        let workspace = crate::workspace_host::WorkspaceHost::open(
            store.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "host".into(),
                device_name: "host".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        workspace.create_chat("chat", None, Some("host"), None, None).unwrap();
        assert_eq!(workspace.chat("chat").unwrap().unwrap().room_gen, Some(STORED_ROOM_GEN));
        host.set_workspace(workspace);
        let handle = host.open("chat").unwrap();

        until("the room's answer", || {
            lock(&handle.chat2).as_ref().is_some_and(|client| client.relay_answer() == Some(false))
        })
        .await;
        let state = lock(&handle.relay).clone().expect("the layer is on: the chat has relay state");
        assert_eq!(lock(&seen).hellos[0]["relay"], 1, "the engine says it can; the room ignores it");

        let body = handle.doc.doc().get_text("body");
        for word in ["stored ", "as ", "rows"] {
            body.insert(body.len_unicode(), word).unwrap();
            handle.doc.doc().commit();
        }
        until("the commits as stored rows", || {
            let reader = loro::LoroDoc::new();
            for row in &lock(&seen).rows {
                let _ = reader.import(row);
            }
            reader.get_text("body").to_string() == "stored as rows"
        })
        .await;

        // Several maintenance ticks and a mode refresh: nothing moves.
        tokio::time::sleep(MAINTENANCE_TICK * 3).await;
        host.refresh_relay_mode(&handle, &state);
        let row = host.workspace().unwrap().chat("chat").unwrap().unwrap();
        assert_eq!(row.room_gen, Some(STORED_ROOM_GEN), "still on generation 2");
        assert!(!state.relaying());
        assert!(!state.heal_owed.load(Ordering::Acquire), "no heal: it never relayed");
        assert!(!state.relay.queue_pushes());
        let pending = store.pending_chat_updates("chat").unwrap();
        assert_eq!(state.uploadable(pending.clone()).len(), pending.len(), "a rejoin uploads the whole outbox, as before");
        let seen = lock(&seen);
        assert!(seen.refused.is_empty(), "no relay frame reached a room that never offered: {:?}", seen.refused);
        assert!(seen.hellos.iter().all(|hello| hello.get("relaying").is_none_or(|v| v.is_null())));
        assert!(seen.hellos.iter().all(|hello| hello.get("gen").is_none_or(|v| v.is_null())));
    }

    /// An edge that answers heal claims with `heal` and counts them, and takes every
    /// checkpoint upload, counting those too.
    async fn edge_answering_heals(heal: u16) -> (String, Arc<AtomicU64>, Arc<AtomicU64>) {
        let (claims, uploads) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (claimed, uploaded) = (claims.clone(), uploads.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (claimed, uploaded) = (claimed.clone(), uploaded.clone());
                tokio::spawn(async move {
                    let mut buffer = vec![0; 256 * 1024];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let status = if head.starts_with("POST") && head.contains("/heal") {
                        assert!(head.to_ascii_lowercase().contains("x-chat2-version:"), "a claim names its version");
                        claimed.fetch_add(1, Ordering::SeqCst);
                        heal
                    } else {
                        if head.starts_with("POST") && head.contains("/checkpoint") {
                            uploaded.fetch_add(1, Ordering::SeqCst);
                        }
                        200
                    };
                    let body = "{\"ok\":true}";
                    let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        (url, claims, uploads)
    }

    /// A host that has just stopped relaying and owes its heal.
    fn owing_heal(handle: &ChatDocHandle) -> Arc<ChatRelayState> {
        let state = state();
        state.hosting.store(true, Ordering::Release);
        state.caught_up.store(true, Ordering::Release);
        state.heal_owed.store(true, Ordering::Release);
        *lock(&state.heal_version) = Some(handle.doc.doc().oplog_vv());
        state
    }

    async fn settle(what: &str, done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_heal_the_rooms_checkpoint_already_covers_sends_no_snapshot() {
        let (url, claims, uploads) = edge_answering_heals(200).await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = owing_heal(&handle);
        *lock(&handle.relay) = Some(state.clone());
        host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
        settle("the heal", || state.healed.load(Ordering::Acquire)).await;
        assert_eq!(claims.load(Ordering::SeqCst), 1);
        assert!(!state.heal_owed.load(Ordering::Acquire));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(uploads.load(Ordering::SeqCst), 0, "the room already holds it all: nothing uploaded");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_heal_the_rooms_checkpoint_doesnt_cover_uploads_the_snapshot() {
        let (url, claims, uploads) = edge_answering_heals(409).await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = owing_heal(&handle);
        *lock(&handle.relay) = Some(state.clone());
        host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
        settle("the heal", || state.healed.load(Ordering::Acquire)).await;
        assert_eq!(claims.load(Ordering::SeqCst), 1);
        assert_eq!(uploads.load(Ordering::SeqCst), 1, "the full heal, as before");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_edge_without_heal_claims_gets_the_snapshot_and_no_more_claims() {
        let (url, claims, uploads) = edge_answering_heals(404).await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = owing_heal(&handle);
        *lock(&handle.relay) = Some(state.clone());
        host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
        settle("the heal", || state.healed.load(Ordering::Acquire)).await;
        assert_eq!(uploads.load(Ordering::SeqCst), 1);
        // The next heal goes straight to the upload.
        state.healed.store(false, Ordering::Release);
        state.heal_owed.store(true, Ordering::Release);
        settle("the in-flight flag", || !handle.checkpointing.load(Ordering::Acquire)).await;
        host.spawn_chat2_checkpoint(&handle, HEAL_REASON);
        settle("the second heal", || uploads.load(Ordering::SeqCst) == 2).await;
        assert_eq!(claims.load(Ordering::SeqCst), 1, "asked once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_claim_answered_after_relaying_again_settles_nothing() {
        let (url, _claims, _uploads) = edge_answering_heals(200).await;
        let dir = tempfile::tempdir().unwrap();
        let (host, _store) = host_on(&url, dir.path());
        let handle = host.open_local("chat").unwrap();
        let state = owing_heal(&handle);
        let epoch = state.mode_epoch();
        // It relayed again, then stopped: that stop owes a heal of its own.
        state.mode_epoch.fetch_add(1, Ordering::AcqRel);
        state.heal_claim_answered(crate::chat2_checkpoint::HealClaim::Healed, epoch);
        assert!(!state.healed.load(Ordering::Acquire));
        assert!(state.heal_owed.load(Ordering::Acquire));
        state.relaying.store(true, Ordering::Release);
        state.heal_claim_answered(crate::chat2_checkpoint::HealClaim::Healed, state.mode_epoch());
        assert!(!state.healed.load(Ordering::Acquire), "relaying: nothing to heal yet");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_relaying_records_what_the_heal_must_cover() {
        let (url, _claims, _uploads) = edge_answering_heals(409).await;
        let dir = tempfile::tempdir().unwrap();
        let (host, store) = host_on(&url, dir.path());
        let workspace = crate::workspace_host::WorkspaceHost::open(
            store.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "host".into(),
                device_name: "host".into(),
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
        let handle = host.open_local("chat").unwrap();
        handle.doc.doc().get_text("body").insert(0, "relayed").unwrap();
        handle.doc.doc().commit();
        let relayed = handle.doc.doc().oplog_vv();
        let state = state();
        state.relaying.store(true, Ordering::Release);
        // No answer from a room yet, so the last one stands: the room has said no.
        state.room_offered.store(false, Ordering::Release);
        host.refresh_relay_mode(&handle, &state);
        assert!(!state.relaying());
        assert!(state.heal_owed.load(Ordering::Acquire));
        assert_eq!(lock(&state.heal_version).clone(), Some(relayed), "everything relayed");
        // Relaying again forgets it: the next stop records its own.
        state.room_offered.store(true, Ordering::Release);
        state.relaying.store(true, Ordering::Release);
        *lock(&state.heal_version) = None;
        assert_eq!(state.heal_claim(), None);
    }
}
