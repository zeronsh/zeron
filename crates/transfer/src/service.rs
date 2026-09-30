//! The engine-facing service: transfer rows (the `WatchFileTransfers`
//! feed), receive settings, history, and the entry points for lanes that
//! peers open. Sending lives in `sender.rs`, receiving in `receiver.rs`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::{
    FileTransfer, FileTransferDirection, FileTransferSettings, FileTransferState,
    FileTransferTransport, SendFilesReply,
};

use crate::receiver::Incoming;
use crate::relay::RelayPipes;
use crate::sender::Outgoing;
use crate::wire::{self, Lane, Msg, PROTOCOL_VERSION};
use crate::{BoxIo, Network, now_ms};

/// Finished rows kept in the history.
const HISTORY_LIMIT: usize = 200;
/// Progress frames at most this often.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);
/// Throughput is measured over this trailing window.
const SPEED_WINDOW: Duration = Duration::from_secs(3);
/// Interrupted incoming transfers wait this long for their sender.
pub(crate) const INCOMING_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub struct TransfersConfig {
    pub device_id: String,
    pub device_name: String,
    /// Profile-scoped state: history and resumable incoming transfers.
    pub state_dir: PathBuf,
    /// Device-scoped receive settings file.
    pub settings_file: PathBuf,
    /// Home folder: the default inbox parent and an allowed destination root.
    pub home_dir: Option<PathBuf>,
}

/// Which transports a send may use (`auto` everywhere but tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportPolicy {
    #[default]
    Auto,
    P2p,
    Relay,
}

pub struct SendRequest {
    pub to: String,
    pub paths: Vec<PathBuf>,
    /// A folder on the receiving device (validated there); default = inbox.
    pub destination: Option<String>,
    pub policy: TransportPolicy,
}

#[derive(Clone)]
pub struct Transfers(pub(crate) Arc<Inner>);

/// A handle that doesn't keep the service alive (for callbacks the
/// service's own dependencies hold).
#[derive(Clone)]
pub struct WeakTransfers(std::sync::Weak<Inner>);

impl WeakTransfers {
    pub fn upgrade(&self) -> Option<Transfers> {
        self.0.upgrade().map(Transfers)
    }
}

pub(crate) struct Inner {
    pub(crate) config: TransfersConfig,
    pub(crate) network: Arc<dyn Network>,
    rows: Mutex<Rows>,
    snapshot: watch::Sender<Vec<FileTransfer>>,
    changed: Notify,
    settings: Mutex<FileTransferSettings>,
    pub(crate) incoming: Mutex<HashMap<String, Arc<Incoming>>>,
    /// Incoming transfers waiting for this device's user: the decision.
    pub(crate) pending: Mutex<HashMap<String, watch::Sender<Option<bool>>>>,
    pub(crate) outgoing: Mutex<HashMap<String, Arc<Outgoing>>>,
    pub(crate) pipes: RelayPipes,
    pub(crate) stop: CancellationToken,
}

#[derive(Default)]
struct Rows {
    list: Vec<FileTransfer>,
    meters: HashMap<String, VecDeque<(Instant, u64)>>,
    dirty: bool,
    last_publish: Option<Instant>,
}

impl Transfers {
    pub fn new(config: TransfersConfig, network: Arc<dyn Network>) -> Self {
        let _ = std::fs::create_dir_all(config.state_dir.join("incoming"));
        let settings = std::fs::read(&config.settings_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut list: Vec<FileTransfer> = std::fs::read(config.state_dir.join("history.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let now = now_ms();
        // A restart ends every live row: outgoing work is gone with the
        // process; incoming rows wait for their sender to resume.
        for row in list.iter_mut().filter(|r| r.is_live()) {
            row.bytes_per_sec = 0;
            match row.direction {
                FileTransferDirection::Outgoing => {
                    row.state = FileTransferState::Failed;
                    row.error = Some("Interrupted when Zeron stopped".into());
                    row.finished_at = Some(now);
                }
                FileTransferDirection::Incoming => {
                    row.state = FileTransferState::Reconnecting;
                }
            }
        }
        let transfers = Self(Arc::new(Inner {
            config,
            network,
            rows: Mutex::new(Rows {
                list,
                ..Default::default()
            }),
            snapshot: watch::channel(Vec::new()).0,
            changed: Notify::new(),
            settings: Mutex::new(settings),
            incoming: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            pipes: RelayPipes::default(),
            stop: CancellationToken::new(),
        }));
        transfers.load_incoming();
        transfers.forget_unaccepted();
        transfers.publish();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&transfers.0);
            let stop = transfers.0.stop.clone();
            handle.spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => return,
                        _ = tokio::time::sleep(PUBLISH_INTERVAL) => {}
                    }
                    let Some(inner) = weak.upgrade() else { return };
                    let transfers = Transfers(inner);
                    if transfers.0.rows.lock().unwrap().dirty {
                        transfers.publish();
                    }
                }
            });
        }
        transfers
    }

    /// Incoming rows that were never accepted wrote nothing and kept no
    /// resume state, so they can't outlive a restart: the sender, when it
    /// redials, simply asks again.
    fn forget_unaccepted(&self) {
        let incoming = self.0.incoming.lock().unwrap();
        self.0.rows.lock().unwrap().list.retain(|row| {
            row.direction != FileTransferDirection::Incoming
                || !row.is_live()
                || incoming.contains_key(&row.id)
        });
    }

    pub fn downgrade(&self) -> WeakTransfers {
        WeakTransfers(Arc::downgrade(&self.0))
    }

    pub fn device_id(&self) -> &str {
        &self.0.config.device_id
    }

    pub fn shutdown(&self) {
        self.0.stop.cancel();
    }

    // ── public surface ─────────────────────────────────────────────────

    pub async fn send(&self, request: SendRequest) -> anyhow::Result<SendFilesReply> {
        anyhow::ensure!(
            request.to != self.0.config.device_id,
            "Pick another device — this one already has these files."
        );
        anyhow::ensure!(!request.to.is_empty(), "No device to send to");
        // `~` is this engine's home: a remote viewer or an agent whose
        // chat runs in `~` can't know the absolute path.
        let home = self.0.config.home_dir.clone();
        let paths: Vec<PathBuf> = request
            .paths
            .iter()
            .map(|path| expand_home(path, home.as_deref()))
            .collect::<anyhow::Result<_>>()?;
        let built = tokio::task::spawn_blocking(move || crate::manifest::build(&paths)).await??;
        let to_name = self
            .0
            .network
            .device_name(&request.to)
            .unwrap_or_else(|| request.to.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        let mut items = built.manifest.items();
        for (item, source) in items.iter_mut().zip(
            built
                .manifest
                .entries
                .iter()
                .zip(&built.sources)
                .filter(|(e, _)| e.is_top_level())
                .map(|(_, s)| s),
        ) {
            item.path = Some(source.to_string_lossy().into_owned());
        }
        self.insert(FileTransfer {
            id: id.clone(),
            direction: FileTransferDirection::Outgoing,
            peer_device_id: request.to.clone(),
            peer_device_name: to_name.clone(),
            state: FileTransferState::Connecting,
            transport: None,
            items,
            file_count: built.manifest.file_count(),
            total_bytes: built.manifest.total_bytes(),
            done_bytes: 0,
            bytes_per_sec: 0,
            destination: request.destination.clone(),
            skipped: built.skipped,
            error: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
        });
        let to = request.to.clone();
        let outgoing = Arc::new(Outgoing::new(id.clone(), to_name.clone(), request, built));
        self.0
            .outgoing
            .lock()
            .unwrap()
            .insert(id.clone(), outgoing.clone());
        let transfers = self.clone();
        tokio::spawn(async move { transfers.run_outgoing(outgoing).await });
        Ok(SendFilesReply {
            transfer_id: id,
            to_device_id: to,
            to_device_name: to_name,
        })
    }

    pub fn watch(&self) -> watch::Receiver<Vec<FileTransfer>> {
        self.0.snapshot.subscribe()
    }

    pub fn list(&self) -> Vec<FileTransfer> {
        self.0.snapshot.borrow().clone()
    }

    /// Cancel from this side. `notify_peer` = false when the peer asked.
    pub fn cancel(&self, id: &str, notify_peer: bool) -> anyhow::Result<()> {
        let row = self
            .row(id)
            .ok_or_else(|| anyhow::anyhow!("No transfer {id}"))?;
        if row.state.is_terminal() {
            return Ok(());
        }
        if let Some(decision) = self.0.pending.lock().unwrap().get(id) {
            decision.send_replace(Some(false));
        }
        let outgoing = self.0.outgoing.lock().unwrap().get(id).cloned();
        if let Some(outgoing) = outgoing {
            outgoing.cancel(notify_peer);
            return Ok(());
        }
        let incoming = self.0.incoming.lock().unwrap().get(id).cloned();
        match incoming {
            Some(incoming) => self.cancel_incoming(&incoming, notify_peer),
            None => {
                self.finish(id, FileTransferState::Cancelled, None);
                if notify_peer {
                    self.notify_peer_cancel(&row.peer_device_id, id);
                }
            }
        }
        Ok(())
    }

    pub fn accept(&self, id: &str) -> anyhow::Result<()> {
        self.decide(id, true)
    }

    pub fn decline(&self, id: &str) -> anyhow::Result<()> {
        self.decide(id, false)
    }

    fn decide(&self, id: &str, accept: bool) -> anyhow::Result<()> {
        let pending = self.0.pending.lock().unwrap();
        let decision = pending
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("This transfer is no longer waiting for an answer"))?;
        decision.send_replace(Some(accept));
        Ok(())
    }

    /// Drop finished rows (`None` = every finished row).
    pub fn clear(&self, id: Option<&str>) {
        {
            let mut rows = self.0.rows.lock().unwrap();
            rows.list
                .retain(|r| r.is_live() || id.is_some_and(|id| r.id != id));
        }
        self.publish();
        self.persist_history();
    }

    pub fn settings(&self) -> FileTransferSettings {
        let mut settings = self.0.settings.lock().unwrap().clone();
        settings.inbox_dir = Some(self.inbox_root().to_string_lossy().into_owned());
        settings
    }

    pub fn set_settings(
        &self,
        settings: FileTransferSettings,
    ) -> anyhow::Result<FileTransferSettings> {
        let mut settings = settings;
        if let Some(dir) = settings.inbox_dir.as_deref().map(str::trim) {
            let default = self.default_inbox();
            if dir.is_empty() || default.as_deref() == Some(Path::new(dir)) {
                settings.inbox_dir = None;
            } else {
                let path = PathBuf::from(dir);
                anyhow::ensure!(path.is_absolute(), "The inbox must be an absolute folder");
                std::fs::create_dir_all(&path)?;
                settings.inbox_dir = Some(dir.to_owned());
            }
        }
        write_atomic(
            &self.0.config.settings_file,
            &serde_json::to_vec_pretty(&settings)?,
        )?;
        *self.0.settings.lock().unwrap() = settings;
        Ok(self.settings())
    }

    /// The relay-pipe server half (`FileTransferPipe*` RPCs).
    pub fn relay_pipes(&self) -> &RelayPipes {
        &self.0.pipes
    }

    /// A lane a peer opened (P2P stream or relay pipe). `peer` is the
    /// peer's device id as the transport established it.
    pub fn accept_lane(&self, peer: String, transport: FileTransferTransport, io: BoxIo) {
        let transfers = self.clone();
        tokio::spawn(async move {
            let stop = transfers.0.stop.clone();
            tokio::select! {
                _ = stop.cancelled() => {}
                result = transfers.serve_lane(peer, transport, io) => {
                    if let Err(error) = result {
                        tracing::debug!(%error, "file transfer lane ended");
                    }
                }
            }
        });
    }

    async fn serve_lane(
        &self,
        peer: String,
        transport: FileTransferTransport,
        mut io: BoxIo,
    ) -> anyhow::Result<()> {
        let hello =
            tokio::time::timeout(Duration::from_secs(30), wire::read_msg(&mut io)).await??;
        let Msg::Hello {
            v,
            transfer_id,
            session_id,
            lane,
            sender_name,
        } = hello
        else {
            anyhow::bail!("expected a transfer hello");
        };
        anyhow::ensure!(
            v == PROTOCOL_VERSION,
            "unsupported file transfer version {v}"
        );
        anyhow::ensure!(
            valid_id(&transfer_id) && valid_id(&session_id),
            "invalid transfer id"
        );
        anyhow::ensure!(
            peer != self.0.config.device_id,
            "a device cannot send to itself"
        );
        match lane {
            Lane::Control => {
                let name = self
                    .0
                    .network
                    .device_name(&peer)
                    .unwrap_or_else(|| sender_name.chars().take(80).collect());
                self.serve_control(peer, name, transport, transfer_id, session_id, io)
                    .await
            }
            Lane::Data => self.serve_data(peer, transfer_id, session_id, io).await,
        }
    }

    // ── rows ───────────────────────────────────────────────────────────

    pub(crate) fn row(&self, id: &str) -> Option<FileTransfer> {
        self.0
            .rows
            .lock()
            .unwrap()
            .list
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    pub(crate) fn insert(&self, row: FileTransfer) {
        {
            let mut rows = self.0.rows.lock().unwrap();
            rows.list.retain(|r| r.id != row.id);
            rows.list.push(row);
        }
        self.publish();
        self.persist_history();
    }

    /// Change a row's state (published and persisted at once).
    pub(crate) fn set_state(
        &self,
        id: &str,
        state: FileTransferState,
        transport: Option<FileTransferTransport>,
    ) {
        let changed = self.mutate(id, |row| {
            if row.state.is_terminal() || (row.state == state && transport.is_none()) {
                return false;
            }
            row.state = state;
            if transport.is_some() {
                row.transport = transport;
            }
            if state != FileTransferState::Reconnecting {
                row.error = None;
            }
            true
        });
        if changed {
            self.publish();
            self.persist_history();
        }
    }

    /// Record a transient problem without ending the transfer.
    pub(crate) fn set_note(&self, id: &str, state: FileTransferState, note: Option<String>) {
        if self.mutate(id, |row| {
            if row.state.is_terminal() {
                return false;
            }
            row.state = state;
            row.error = note;
            row.bytes_per_sec = 0;
            true
        }) {
            self.publish();
        }
    }

    pub(crate) fn finish(&self, id: &str, state: FileTransferState, error: Option<String>) {
        let changed = self.mutate(id, |row| {
            if row.state.is_terminal() {
                return false;
            }
            row.state = state;
            row.error = error.clone();
            row.bytes_per_sec = 0;
            row.finished_at = Some(now_ms());
            if state == FileTransferState::Completed {
                row.done_bytes = row.total_bytes;
            }
            true
        });
        self.0.pending.lock().unwrap().remove(id);
        self.0.outgoing.lock().unwrap().remove(id);
        self.0.incoming.lock().unwrap().remove(id);
        if changed {
            self.publish();
            self.persist_history();
        }
    }

    pub(crate) fn progress(&self, id: &str, done_bytes: u64) {
        let mut rows = self.0.rows.lock().unwrap();
        let Some(row) = rows.list.iter_mut().find(|r| r.id == id) else {
            return;
        };
        if row.done_bytes != done_bytes {
            row.done_bytes = done_bytes.min(row.total_bytes);
            row.updated_at = now_ms();
            rows.dirty = true;
        }
    }

    pub(crate) fn update_row(&self, id: &str, f: impl FnOnce(&mut FileTransfer)) {
        if self.mutate(id, |row| {
            f(row);
            true
        }) {
            self.publish();
            self.persist_history();
        }
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut FileTransfer) -> bool) -> bool {
        let mut rows = self.0.rows.lock().unwrap();
        let Some(row) = rows.list.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        let changed = f(row);
        if changed {
            row.updated_at = now_ms();
        }
        changed
    }

    fn publish(&self) {
        let snapshot = {
            let mut rows = self.0.rows.lock().unwrap();
            let now = Instant::now();
            let Rows {
                list,
                meters,
                dirty,
                last_publish,
            } = &mut *rows;
            meters.retain(|id, _| list.iter().any(|r| &r.id == id && r.is_live()));
            for row in list.iter_mut() {
                if row.state != FileTransferState::Transferring {
                    row.bytes_per_sec = 0;
                    continue;
                }
                let samples = meters.entry(row.id.clone()).or_default();
                samples.push_back((now, row.done_bytes));
                while samples
                    .front()
                    .is_some_and(|(at, _)| now.duration_since(*at) > SPEED_WINDOW)
                {
                    samples.pop_front();
                }
                if let (Some((t0, b0)), Some((t1, b1))) = (samples.front(), samples.back()) {
                    let dt = t1.duration_since(*t0).as_secs_f64();
                    if dt >= 0.2 {
                        row.bytes_per_sec = (b1.saturating_sub(*b0) as f64 / dt) as u64;
                    }
                }
            }
            *dirty = false;
            *last_publish = Some(now);
            let mut snapshot = list.clone();
            snapshot.sort_by_key(|row| std::cmp::Reverse(row.created_at));
            snapshot
        };
        self.0.snapshot.send_replace(snapshot);
        self.0.changed.notify_waiters();
    }

    fn persist_history(&self) {
        let rows = {
            let mut rows = self.0.rows.lock().unwrap();
            // Bound the history: drop the oldest finished rows.
            let finished = rows.list.iter().filter(|r| !r.is_live()).count();
            if finished > HISTORY_LIMIT {
                let mut excess = finished - HISTORY_LIMIT;
                rows.list.sort_by_key(|r| r.created_at);
                rows.list.retain(|r| {
                    if excess > 0 && !r.is_live() {
                        excess -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
            rows.list.clone()
        };
        let path = self.0.config.state_dir.join("history.json");
        if let Err(error) = serde_json::to_vec(&rows)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&path, &bytes))
        {
            tracing::warn!(%error, "could not save file transfer history");
        }
    }

    pub(crate) fn notify_peer_cancel(&self, peer: &str, id: &str) {
        let network = self.0.network.clone();
        let (peer, id) = (peer.to_owned(), id.to_owned());
        tokio::spawn(async move { network.notify_cancel(&peer, &id).await });
    }

    // ── receive locations ──────────────────────────────────────────────

    fn default_inbox(&self) -> Option<PathBuf> {
        self.0
            .config
            .home_dir
            .as_ref()
            .map(|home| home.join("Zeron Transfers"))
    }

    pub(crate) fn inbox_root(&self) -> PathBuf {
        let configured = self.0.settings.lock().unwrap().inbox_dir.clone();
        configured
            .map(PathBuf::from)
            .or_else(|| self.default_inbox())
            .unwrap_or_else(|| self.0.config.state_dir.join("received"))
    }

    pub(crate) fn require_confirmation(&self) -> bool {
        self.0.settings.lock().unwrap().require_confirmation
    }

    /// Where an incoming transfer lands: the sender's inbox folder, or an
    /// explicit destination inside this device's home or known projects.
    pub(crate) fn resolve_destination(
        &self,
        sender_name: &str,
        destination: Option<&str>,
    ) -> anyhow::Result<PathBuf> {
        let Some(destination) = destination.map(str::trim).filter(|d| !d.is_empty()) else {
            return Ok(self.inbox_root().join(folder_name(sender_name)));
        };
        let path = expand_home(Path::new(destination), self.0.config.home_dir.as_deref())?;
        anyhow::ensure!(
            path.is_absolute(),
            "The destination must be an absolute folder on the receiving device"
        );
        let canonical = path
            .canonicalize()
            .map_err(|_| anyhow::anyhow!("The destination folder {destination} doesn't exist"))?;
        anyhow::ensure!(
            canonical.is_dir(),
            "The destination {destination} is not a folder"
        );
        anyhow::ensure!(
            !canonical.components().any(|c| c.as_os_str() == ".git"),
            "Files can't be sent into a .git folder"
        );
        let mut roots: Vec<PathBuf> = self.0.network.destination_roots();
        roots.extend(self.0.config.home_dir.clone());
        roots.push(self.inbox_root());
        let allowed = roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| canonical.starts_with(&root));
        anyhow::ensure!(
            allowed,
            "The destination must be inside the home folder or a project on the receiving device"
        );
        Ok(canonical)
    }

    pub(crate) fn incoming_dir(&self, id: &str) -> PathBuf {
        self.0.config.state_dir.join("incoming").join(id)
    }
}

/// `~` or `~/…` (either separator) → under `home`; anything else as is.
/// `~user` forms are not expanded.
pub fn expand_home(path: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let Some(text) = path.to_str() else {
        return Ok(path.to_path_buf());
    };
    let rest = match text.strip_prefix('~') {
        Some("") => "",
        Some(rest) if rest.starts_with(['/', '\\']) => &rest[1..],
        _ => return Ok(path.to_path_buf()),
    };
    let home =
        home.ok_or_else(|| anyhow::anyhow!("This device has no home folder to resolve {text}"))?;
    Ok(if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    })
}

/// Transfer and session ids: UUIDs (or anything as tame).
pub(crate) fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A device name as a single safe folder name.
pub(crate) fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        "Unknown device".into()
    } else {
        cleaned.chars().take(80).collect()
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_is_this_engines_home() {
        let home = Path::new("/home/zeron");
        let expand = |p: &str| expand_home(Path::new(p), Some(home)).unwrap();
        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/out/app.apk"), home.join("out/app.apk"));
        assert_eq!(expand("/abs/~/x"), Path::new("/abs/~/x"));
        assert_eq!(
            expand("~other/x"),
            Path::new("~other/x"),
            "~user is not expanded"
        );
        assert!(expand_home(Path::new("~/x"), None).is_err());
    }

    #[test]
    fn device_names_become_single_folder_names() {
        assert_eq!(folder_name("Daniel's MacBook"), "Daniel's MacBook");
        assert_eq!(folder_name("../../etc"), "-..-etc");
        assert_eq!(folder_name("a/b\\c:d"), "a-b-c-d");
        assert_eq!(folder_name("  ..  "), "Unknown device");
        assert!(valid_id("0b5c6f3e-1a2b-4c3d-8e9f-001122334455"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
    }
}
