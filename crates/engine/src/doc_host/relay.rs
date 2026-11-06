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

/// Stream prefix of a chat's direct link.
pub const CHAT_LINK_PREFIX: &str = "chatlink/";
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

/// The engine-wide flag.
pub fn relay_layer_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("ZERON_RELAY_LAYER").as_deref() == Ok("1"))
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
        })
    }

    pub(super) fn relaying(&self) -> bool {
        self.relaying.load(Ordering::Acquire)
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
        if state.relay.recently_active(LINGER_AFTER_PEER) || state.heal_owed.load(Ordering::Acquire) {
            return false;
        }
        !(state.relaying()
            && crate::runner::session().is_some()
            && !state.checkpoint_covers(&self.doc.doc().oplog_vv()))
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
            .get_or_insert_with(|| ChatRelayState::new(RelayHandle::spawn(device, sink, RelayPolicy::default())))
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
        let (answer, socketless) = lock(&handle.chat2)
            .as_ref()
            .map_or((None, false), |client| (client.relay_answer(), client.socketless()));
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
        let allowed = workspace.read_devices_seen().is_ok_and(|devices| {
            relay_allowed(&devices, &chat.device_id, parent_host.as_deref(), chrono::Utc::now())
        });

        // Moving on: only a room that has just said yes, and only once every
        // device can follow.
        if is_host && generation == STORED_ROOM_GEN && allowed && answer == Some(true) {
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
        if relaying {
            state.healed.store(false, Ordering::Release);
            state.heal_owed.store(false, Ordering::Release);
        } else if is_host && generation >= RELAY_ROOM_GEN && !state.healed.load(Ordering::Acquire) {
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
            match previews.open_peer_stream(&host_device, &service).await {
                Ok(stream) => {
                    tracing::info!(chat = %chat, device = %host_device, "chat link up");
                    run_link(stream, host_device, state.relay.clone(), stop).await;
                }
                Err(error) => {
                    // The room carries this chat. The maintenance tick retries.
                    tracing::debug!(chat = %chat, %error, "chat link unavailable");
                }
            }
            *lock(&state.link) = None;
        });
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
        host.spawn_worker(async move {
            run_link(ours, peer, relay, CancellationToken::new()).await;
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
