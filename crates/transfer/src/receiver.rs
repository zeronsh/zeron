//! Receiving: validate the offer, decide (auto-accept or ask), lay the
//! items out under a fresh name, then verify and write blocks as data lanes
//! deliver them. Verified blocks are persisted (after an fsync of the data
//! they describe) so a dropped tunnel — or a restart of this engine —
//! resumes where it stopped. Each file lands as a hidden `*.part`, is
//! checked against the sender's whole-file SHA-256, and only then renamed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::{FileTransfer, FileTransferDirection, FileTransferState, FileTransferTransport};

use crate::blocks::{BLOCK_SIZE, RangeSet, block_count, block_len};
use crate::manifest::{self, Entry, EntryKind, MAX_ENTRIES, Manifest, hex};
use crate::service::{INCOMING_RETENTION_MS, write_atomic};
use crate::wire::{self, FileHave, Frame, Msg};
use crate::{BoxIo, Transfers, now_ms};

/// Whole-file digest mismatches tolerated per file before giving up.
const MAX_RESENDS: u32 = 3;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const PENDING_PING: Duration = Duration::from_secs(10);
/// Free space kept beyond the transfer's remaining bytes.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// What survives a restart: written once when the transfer is accepted.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    id: String,
    peer: String,
    peer_name: String,
    digest: String,
    dest_root: PathBuf,
    /// Sender's top-level name → the name used here.
    tops: HashMap<String, String>,
    created_at: i64,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Default)]
struct Progress {
    files: Vec<FileHave>,
}

#[derive(Default)]
struct FileState {
    blocks: RangeSet,
    digest: Option<String>,
    done: bool,
    finalizing: bool,
    resends: u32,
    /// Written since the last durable flush.
    touched: bool,
    /// Final name (top-level files may be renamed around a conflict).
    landed: Option<PathBuf>,
}

#[derive(Default)]
struct RecvState {
    files: HashMap<u64, FileState>,
    done_bytes: u64,
    completing: bool,
}

enum Decision {
    Decided(bool),
    Cancelled,
    Lost(anyhow::Error),
}

pub(crate) struct Live {
    session_id: String,
    stop: CancellationToken,
    control: mpsc::Sender<Msg>,
}

pub(crate) struct Incoming {
    id: String,
    peer: String,
    digest: String,
    manifest: Manifest,
    dest_root: PathBuf,
    tops: HashMap<String, String>,
    state: Mutex<RecvState>,
    session: Mutex<Option<Live>>,
    /// This device's user cancelled.
    cancel: CancellationToken,
}

impl Incoming {
    fn final_path(&self, entry: &Entry) -> PathBuf {
        let (top, rest) = match entry.path.split_once('/') {
            Some((top, rest)) => (top, Some(rest)),
            None => (entry.path.as_str(), None),
        };
        let top = self.tops.get(top).map(String::as_str).unwrap_or(top);
        let root = self.dest_root.join(top);
        match rest {
            Some(rest) => manifest::join(&root, rest),
            None => root,
        }
    }

    fn part_path(&self, entry: &Entry) -> PathBuf {
        let target = self.final_path(entry);
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tag: String = self
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        target.with_file_name(format!(".{name}.zeron-{tag}.part"))
    }

    fn have(&self) -> Vec<FileHave> {
        let state = self.state.lock().unwrap();
        let mut have: Vec<FileHave> = state
            .files
            .iter()
            .filter(|(_, f)| f.done || !f.blocks.is_empty())
            .map(|(file, f)| FileHave {
                file: *file,
                blocks: f.blocks.clone(),
                done: f.done,
            })
            .collect();
        have.sort_by_key(|h| h.file);
        have
    }

    fn file_entries(&self) -> impl Iterator<Item = (u64, &Entry)> {
        self.manifest
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EntryKind::File)
            .map(|(i, e)| (i as u64, e))
    }

    fn all_done(&self) -> bool {
        let state = self.state.lock().unwrap();
        self.file_entries()
            .all(|(i, _)| state.files.get(&i).is_some_and(|f| f.done))
    }

    async fn send_control(&self, msg: Msg) {
        let control = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|l| l.control.clone());
        if let Some(control) = control {
            let _ = control.send(msg).await;
        }
    }
}

impl Transfers {
    /// Restore accepted, unfinished transfers so their senders can resume.
    pub(crate) fn load_incoming(&self) {
        let dir = self.0.config.state_dir.join("incoming");
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        for child in read.filter_map(Result::ok) {
            let path = child.path();
            let meta: Option<Meta> = std::fs::read(path.join("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let Some(meta) = meta else {
                let _ = std::fs::remove_dir_all(&path);
                continue;
            };
            let progress: Progress = std::fs::read(path.join("progress.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let row = self.row(&meta.id);
            let stale = now_ms() - meta.created_at > INCOMING_RETENTION_MS;
            if stale || row.as_ref().is_some_and(|r| r.state.is_terminal()) {
                let incoming = incoming_from(meta, progress);
                remove_parts(&incoming);
                let _ = std::fs::remove_dir_all(&path);
                if stale {
                    self.finish(
                        &incoming.id,
                        FileTransferState::Failed,
                        Some("The sending device never resumed this transfer".into()),
                    );
                }
                continue;
            }
            let peer_name = meta.peer_name.clone();
            let incoming = Arc::new(incoming_from(meta, progress));
            if row.is_none() {
                self.insert(incoming_row(
                    &incoming,
                    &peer_name,
                    FileTransferState::Reconnecting,
                ));
            }
            self.0
                .incoming
                .lock()
                .unwrap()
                .insert(incoming.id.clone(), incoming);
        }
    }

    pub(crate) fn cancel_incoming(&self, incoming: &Arc<Incoming>, notify_peer: bool) {
        let live = incoming.session.lock().unwrap().is_some();
        incoming.cancel.cancel();
        if !live {
            // No session to carry the news: clean up here.
            remove_parts(incoming);
            let _ = std::fs::remove_dir_all(self.incoming_dir(&incoming.id));
            self.finish(&incoming.id, FileTransferState::Cancelled, None);
            if notify_peer {
                self.notify_peer_cancel(&incoming.peer, &incoming.id);
            }
        }
    }

    pub(crate) async fn serve_control(
        &self,
        peer: String,
        peer_name: String,
        transport: FileTransferTransport,
        id: String,
        session_id: String,
        io: BoxIo,
    ) -> anyhow::Result<()> {
        let (mut read, mut write) = tokio::io::split(io);
        let Msg::Offer {
            entry_count,
            file_count,
            total_bytes,
            digest,
            destination,
            skipped,
        } = wire::read_msg(&mut read).await?
        else {
            anyhow::bail!("expected a transfer offer");
        };
        anyhow::ensure!(
            entry_count as usize <= MAX_ENTRIES,
            "the transfer lists too many entries"
        );
        let mut entries = Vec::with_capacity(entry_count.min(65536) as usize);
        loop {
            match wire::read_msg(&mut read).await? {
                Msg::Manifest { entries: chunk } => {
                    anyhow::ensure!(
                        entries.len() + chunk.len() <= entry_count as usize,
                        "the manifest is longer than announced"
                    );
                    entries.extend(chunk);
                }
                Msg::ManifestEnd => break,
                _ => anyhow::bail!("unexpected message in the manifest"),
            }
        }
        let manifest = Manifest { entries };
        let invalid = |message: String| Msg::Error { message };
        if let Err(error) = manifest.validate() {
            wire::write_msg(&mut write, &invalid(format!("Refused: {error}"))).await?;
            return Err(error);
        }
        if manifest.digest() != digest
            || manifest.file_count() != file_count
            || manifest.total_bytes() != total_bytes
            || manifest.entries.len() as u64 != entry_count
        {
            wire::write_msg(&mut write, &invalid("The manifest is inconsistent".into())).await?;
            anyhow::bail!("inconsistent manifest");
        }

        // Everything the sender says from here on arrives through a reader
        // task, so waiting (for the user, for data) never loses a frame.
        let (tx, mut rx) = mpsc::channel::<anyhow::Result<Msg>>(64);
        tokio::spawn(async move {
            loop {
                let msg = wire::read_msg(&mut read).await;
                let end = msg.is_err();
                if tx.send(msg).await.is_err() || end {
                    return;
                }
            }
        });

        let existing = self.0.incoming.lock().unwrap().get(&id).cloned();
        let incoming = match existing {
            Some(incoming) => {
                if incoming.peer != peer || incoming.digest != digest {
                    wire::write_msg(&mut write, &invalid("Conflicting transfer id".into())).await?;
                    anyhow::bail!("conflicting transfer id {id}");
                }
                if incoming.cancel.is_cancelled() {
                    wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await?;
                    return Ok(());
                }
                incoming
            }
            None => {
                if let Some(row) = self.row(&id)
                    && row.state.is_terminal()
                {
                    let answer = match row.state {
                        FileTransferState::Completed => Msg::Complete,
                        FileTransferState::Declined => Msg::Decline {
                            reason: "Declined".into(),
                        },
                        FileTransferState::Cancelled => Msg::Cancel { reason: None },
                        _ => invalid(row.error.unwrap_or_else(|| "The transfer failed".into())),
                    };
                    wire::write_msg(&mut write, &answer).await?;
                    return Ok(());
                }
                let dest_root = match self.resolve_destination(&peer_name, destination.as_deref()) {
                    Ok(root) => root,
                    Err(error) => {
                        wire::write_msg(&mut write, &invalid(error.to_string())).await?;
                        return Err(error);
                    }
                };
                if let Some(message) = space_problem(&dest_root, total_bytes) {
                    wire::write_msg(&mut write, &invalid(message.clone())).await?;
                    anyhow::bail!(message);
                }
                let mut row = new_row(&id, &peer, &peer_name, &manifest, skipped);
                row.destination = Some(dest_root.to_string_lossy().into_owned());
                if self.require_confirmation() {
                    row.state = FileTransferState::AwaitingAcceptance;
                    let (decision, decided) = watch::channel(None);
                    self.0.pending.lock().unwrap().insert(id.clone(), decision);
                    self.insert(row);
                    let accepted = match self.await_decision(&mut write, &mut rx, decided).await {
                        Decision::Decided(accepted) => accepted,
                        Decision::Cancelled => {
                            self.finish(&id, FileTransferState::Cancelled, None);
                            return Ok(());
                        }
                        Decision::Lost(error) => {
                            // The sender reconnects and asks again.
                            self.0.pending.lock().unwrap().remove(&id);
                            self.set_note(
                                &id,
                                FileTransferState::Reconnecting,
                                Some("Waiting for the sender to reconnect".into()),
                            );
                            return Err(error);
                        }
                    };
                    self.0.pending.lock().unwrap().remove(&id);
                    if !accepted {
                        self.finish(&id, FileTransferState::Declined, None);
                        wire::write_msg(
                            &mut write,
                            &Msg::Decline {
                                reason: "The other device declined the files".into(),
                            },
                        )
                        .await?;
                        return Ok(());
                    }
                } else {
                    row.state = FileTransferState::Connecting;
                    self.insert(row);
                }
                let incoming = match self
                    .lay_out(&id, &peer, &peer_name, &digest, dest_root, manifest)
                    .await
                {
                    Ok(incoming) => incoming,
                    Err(error) => {
                        let message = format!("Could not save the files: {error}");
                        self.finish(&id, FileTransferState::Failed, Some(message.clone()));
                        wire::write_msg(&mut write, &invalid(message)).await?;
                        return Err(error);
                    }
                };
                let paths: Vec<(String, PathBuf)> = incoming
                    .manifest
                    .entries
                    .iter()
                    .filter(|e| e.is_top_level())
                    .map(|e| (e.path.clone(), incoming.final_path(e)))
                    .collect();
                self.update_row(&id, |row| {
                    for (item, (_, path)) in row.items.iter_mut().zip(&paths) {
                        item.path = Some(path.to_string_lossy().into_owned());
                    }
                });
                self.0
                    .incoming
                    .lock()
                    .unwrap()
                    .insert(id.clone(), incoming.clone());
                incoming
            }
        };
        self.run_receive_session(incoming, session_id, transport, write, rx)
            .await
    }

    /// Tell the sender to wait (every [`PENDING_PING`]) until this device's
    /// user decides, the sender cancels, or the lane drops. A lane that
    /// fails while writing is drained first: a `Cancel` the sender sent
    /// just before closing must not be mistaken for a lost connection.
    async fn await_decision<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        write: &mut W,
        rx: &mut mpsc::Receiver<anyhow::Result<Msg>>,
        mut decided: watch::Receiver<Option<bool>>,
    ) -> Decision {
        let mut ping = tokio::time::interval(PENDING_PING);
        let lost = loop {
            tokio::select! {
                _ = decided.changed() => {
                    if let Some(accepted) = *decided.borrow() {
                        return Decision::Decided(accepted);
                    }
                }
                _ = ping.tick() => {
                    if let Err(error) = wire::write_msg(write, &Msg::Pending).await {
                        break error;
                    }
                }
                msg = rx.recv() => match msg {
                    Some(Ok(Msg::Cancel { .. })) => return Decision::Cancelled,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Decision::Lost(error),
                    None => return Decision::Lost(anyhow::anyhow!("the sender went away")),
                },
            }
        };
        loop {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(Ok(Msg::Cancel { .. }))) => return Decision::Cancelled,
                Ok(Some(Ok(_))) => {}
                _ => return Decision::Lost(lost),
            }
        }
    }

    async fn lay_out(
        &self,
        id: &str,
        peer: &str,
        peer_name: &str,
        digest: &str,
        dest_root: PathBuf,
        manifest: Manifest,
    ) -> anyhow::Result<Arc<Incoming>> {
        let meta_dir = self.incoming_dir(id);
        let (id, peer, peer_name, digest) = (
            id.to_owned(),
            peer.to_owned(),
            peer_name.to_owned(),
            digest.to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dest_root)?;
            let mut tops = HashMap::new();
            for entry in manifest.entries.iter().filter(|e| e.is_top_level()) {
                let mut name = entry.path.clone();
                let mut n = 2;
                while std::fs::symlink_metadata(dest_root.join(&name)).is_ok() {
                    name = manifest::numbered(&entry.path, n);
                    n += 1;
                }
                if entry.kind == EntryKind::Dir {
                    // Creating it reserves the name.
                    std::fs::create_dir(dest_root.join(&name))?;
                }
                tops.insert(entry.path.clone(), name);
            }
            let meta = Meta {
                id,
                peer,
                peer_name,
                digest,
                dest_root,
                tops,
                created_at: now_ms(),
                entries: manifest.entries,
            };
            let bytes = serde_json::to_vec(&meta)?;
            let incoming = incoming_from(meta, Progress::default());
            for entry in incoming
                .manifest
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Dir)
            {
                ensure_dir(&incoming.final_path(entry))?;
            }
            write_atomic(&meta_dir.join("meta.json"), &bytes)?;
            anyhow::Ok(Arc::new(incoming))
        })
        .await?
    }

    async fn run_receive_session<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        incoming: Arc<Incoming>,
        session_id: String,
        transport: FileTransferTransport,
        mut write: W,
        mut rx: mpsc::Receiver<anyhow::Result<Msg>>,
    ) -> anyhow::Result<()> {
        let stop = CancellationToken::new();
        let (control, mut outbox) = mpsc::channel::<Msg>(256);
        if let Some(old) = incoming.session.lock().unwrap().replace(Live {
            session_id: session_id.clone(),
            stop: stop.clone(),
            control,
        }) {
            old.stop.cancel();
        }
        let id = incoming.id.clone();
        let result = async {
            wire::write_msg(
                &mut write,
                &Msg::Accept {
                    have: incoming.have(),
                },
            )
            .await?;
            self.set_state(&id, FileTransferState::Transferring, Some(transport));
            self.progress(&id, incoming.state.lock().unwrap().done_bytes);
            if incoming.all_done() {
                self.spawn_complete(incoming.clone());
            }
            let mut progress = tokio::time::interval(PROGRESS_INTERVAL);
            let mut flush = tokio::time::interval(FLUSH_INTERVAL);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return Ok(()),
                    _ = incoming.cancel.cancelled() => {
                        let _ = wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await;
                        self.abandon(&incoming, FileTransferState::Cancelled, None).await;
                        return Ok(());
                    }
                    msg = rx.recv() => match msg {
                        Some(Ok(Msg::Digest { file, sha256 })) => self.on_digest(&incoming, file, sha256),
                        Some(Ok(Msg::Cancel { .. })) => {
                            self.abandon(&incoming, FileTransferState::Cancelled, None).await;
                            return Ok(());
                        }
                        Some(Ok(Msg::Error { message })) => {
                            self.abandon(&incoming, FileTransferState::Failed, Some(message)).await;
                            return Ok(());
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => return Err(error),
                        None => anyhow::bail!("the sender went away"),
                    },
                    Some(msg) = outbox.recv() => {
                        let end = matches!(msg, Msg::Complete | Msg::Error { .. });
                        wire::write_msg(&mut write, &msg).await?;
                        if end {
                            let _ = write.shutdown().await;
                            return Ok(());
                        }
                    }
                    _ = progress.tick() => {
                        let done_bytes = incoming.state.lock().unwrap().done_bytes;
                        self.progress(&id, done_bytes);
                        wire::write_msg(&mut write, &Msg::Progress { done_bytes }).await?;
                    }
                    _ = flush.tick() => self.flush(&incoming).await,
                }
            }
        }
        .await;
        {
            let mut session = incoming.session.lock().unwrap();
            if session.as_ref().is_some_and(|l| l.session_id == session_id) {
                session.take();
            }
        }
        if let Err(error) = &result
            && !stop.is_cancelled()
        {
            self.flush(&incoming).await;
            self.progress(&id, incoming.state.lock().unwrap().done_bytes);
            self.set_note(
                &id,
                FileTransferState::Reconnecting,
                Some(format!(
                    "Connection lost ({error}); waiting for the sender to resume"
                )),
            );
        }
        result
    }

    pub(crate) async fn serve_data(
        &self,
        peer: String,
        id: String,
        session_id: String,
        mut io: BoxIo,
    ) -> anyhow::Result<()> {
        let incoming = self
            .0
            .incoming
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown transfer {id}"))?;
        anyhow::ensure!(incoming.peer == peer, "transfer belongs to another device");
        let stop = {
            let session = incoming.session.lock().unwrap();
            let live = session
                .as_ref()
                .filter(|l| l.session_id == session_id)
                .ok_or_else(|| anyhow::anyhow!("stale transfer session"))?;
            live.stop.clone()
        };
        let mut files: HashMap<u32, Arc<std::fs::File>> = HashMap::new();
        loop {
            let frame = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                _ = incoming.cancel.cancelled() => return Ok(()),
                frame = wire::read_frame(&mut io) => frame?,
            };
            match frame {
                None => return Ok(()),
                Some(Frame::Msg(_)) => anyhow::bail!("unexpected message on a data lane"),
                Some(Frame::Block(block)) => self.on_block(&incoming, &mut files, block).await?,
            }
        }
    }

    async fn on_block(
        &self,
        incoming: &Arc<Incoming>,
        files: &mut HashMap<u32, Arc<std::fs::File>>,
        block: wire::Block,
    ) -> anyhow::Result<()> {
        let entry = incoming
            .manifest
            .entries
            .get(block.file as usize)
            .filter(|e| e.kind == EntryKind::File)
            .ok_or_else(|| anyhow::anyhow!("block for an unknown file"))?;
        anyhow::ensure!(block.offset.is_multiple_of(BLOCK_SIZE), "misaligned block");
        let index = block.offset / BLOCK_SIZE;
        anyhow::ensure!(
            index < block_count(entry.size)
                && block.data.len() as u64 == block_len(entry.size, index),
            "block outside {}",
            entry.path
        );
        let file = block.file as u64;
        {
            let state = incoming.state.lock().unwrap();
            if state
                .files
                .get(&file)
                .is_some_and(|f| f.done || f.blocks.contains(index))
            {
                return Ok(());
            }
        }
        let handle = match files.get(&block.file) {
            Some(handle) => handle.clone(),
            None => {
                if files.len() >= 16 {
                    files.clear();
                }
                let path = incoming.part_path(entry);
                let handle =
                    Arc::new(tokio::task::spawn_blocking(move || open_part(&path)).await??);
                files.insert(block.file, handle.clone());
                handle
            }
        };
        let length = block.data.len() as u64;
        let verified = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            if !block.verify() {
                return Ok(false);
            }
            write_at(&handle, &block.data, block.offset)?;
            Ok(true)
        })
        .await??;
        if !verified {
            incoming
                .send_control(Msg::Resend {
                    file,
                    blocks: Some(vec![index]),
                })
                .await;
            return Ok(());
        }
        let done_bytes = {
            let mut state = incoming.state.lock().unwrap();
            let entry_state = state.files.entry(file).or_default();
            if entry_state.blocks.insert(index) {
                entry_state.touched = true;
                state.done_bytes += length;
            }
            state.done_bytes
        };
        // Blocks still draining after the control lane dropped count too.
        self.progress(&incoming.id, done_bytes);
        self.maybe_finalize(incoming, file);
        Ok(())
    }

    fn on_digest(&self, incoming: &Arc<Incoming>, file: u64, sha256: String) {
        let valid = incoming
            .manifest
            .entries
            .get(file as usize)
            .is_some_and(|e| e.kind == EntryKind::File)
            && sha256.len() == 64;
        if !valid {
            return;
        }
        incoming
            .state
            .lock()
            .unwrap()
            .files
            .entry(file)
            .or_default()
            .digest = Some(sha256);
        self.maybe_finalize(incoming, file);
    }

    fn maybe_finalize(&self, incoming: &Arc<Incoming>, file: u64) {
        let entry = incoming.manifest.entries[file as usize].clone();
        let digest = {
            let mut state = incoming.state.lock().unwrap();
            let f = state.files.entry(file).or_default();
            if f.done || f.finalizing || f.blocks.len() < block_count(entry.size) {
                return;
            }
            let Some(digest) = f.digest.clone() else {
                return;
            };
            f.finalizing = true;
            digest
        };
        let transfers = self.clone();
        let incoming = incoming.clone();
        tokio::spawn(async move {
            let part = incoming.part_path(&entry);
            let target = incoming.final_path(&entry);
            let top_level = entry.is_top_level();
            let size = entry.size;
            let mode = entry.mode;
            let result = tokio::task::spawn_blocking(move || {
                finalize_file(&part, &target, size, mode, &digest, top_level)
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            match result {
                Ok(Some(landed)) => {
                    if top_level && landed != incoming.final_path(&entry) {
                        let shown = landed.to_string_lossy().into_owned();
                        let name = entry.path.clone();
                        transfers.update_row(&incoming.id, |row| {
                            if let Some(item) = row.items.iter_mut().find(|i| i.name == name) {
                                item.path = Some(shown);
                            }
                        });
                    }
                    {
                        let mut state = incoming.state.lock().unwrap();
                        let f = state.files.entry(file).or_default();
                        f.done = true;
                        f.finalizing = false;
                        f.landed = Some(landed);
                    }
                    if incoming.all_done() {
                        transfers.spawn_complete(incoming);
                    }
                }
                Ok(None) => {
                    // Whole-file digest mismatch: fetch the file again.
                    let resends = {
                        let mut state = incoming.state.lock().unwrap();
                        let f = state.files.entry(file).or_default();
                        let verified = f.blocks.bytes(entry.size);
                        f.blocks = RangeSet::default();
                        f.finalizing = false;
                        f.digest = None;
                        f.resends += 1;
                        let resends = f.resends;
                        state.done_bytes = state.done_bytes.saturating_sub(verified);
                        resends
                    };
                    if resends > MAX_RESENDS {
                        let message = format!("{} kept failing verification", entry.path);
                        incoming
                            .send_control(Msg::Error {
                                message: message.clone(),
                            })
                            .await;
                        transfers
                            .abandon(&incoming, FileTransferState::Failed, Some(message))
                            .await;
                    } else {
                        incoming
                            .send_control(Msg::Resend { file, blocks: None })
                            .await;
                    }
                }
                Err(error) => {
                    let message = format!("Could not save {}: {error}", entry.path);
                    incoming
                        .send_control(Msg::Error {
                            message: message.clone(),
                        })
                        .await;
                    transfers
                        .abandon(&incoming, FileTransferState::Failed, Some(message))
                        .await;
                }
            }
        });
    }

    fn spawn_complete(&self, incoming: Arc<Incoming>) {
        {
            let mut state = incoming.state.lock().unwrap();
            if state.completing {
                return;
            }
            state.completing = true;
        }
        let transfers = self.clone();
        tokio::spawn(async move {
            let links = incoming.clone();
            let result = tokio::task::spawn_blocking(move || finish_tree(&links)).await;
            if let Ok(Err(error)) | Err(error) = result.map_err(anyhow::Error::from) {
                tracing::warn!(%error, "file transfer: could not finish folders");
            }
            let _ = std::fs::remove_dir_all(transfers.incoming_dir(&incoming.id));
            transfers.finish(&incoming.id, FileTransferState::Completed, None);
            incoming.send_control(Msg::Complete).await;
        });
    }

    /// End an incoming transfer here: drop partial data and resume state.
    async fn abandon(
        &self,
        incoming: &Arc<Incoming>,
        state: FileTransferState,
        error: Option<String>,
    ) {
        let cleanup = incoming.clone();
        let _ = tokio::task::spawn_blocking(move || remove_parts(&cleanup)).await;
        let _ = std::fs::remove_dir_all(self.incoming_dir(&incoming.id));
        self.finish(&incoming.id, state, error);
    }

    /// Persist verified blocks — only after the bytes they describe are
    /// on disk, so a crash never records a block that isn't there.
    async fn flush(&self, incoming: &Arc<Incoming>) {
        let (touched, progress) = {
            let mut state = incoming.state.lock().unwrap();
            let touched: Vec<u64> = state
                .files
                .iter_mut()
                .filter_map(|(i, f)| (std::mem::take(&mut f.touched) && !f.done).then_some(*i))
                .collect();
            let files = state
                .files
                .iter()
                .filter(|(_, f)| f.done || !f.blocks.is_empty())
                .map(|(i, f)| FileHave {
                    file: *i,
                    blocks: f.blocks.clone(),
                    done: f.done,
                })
                .collect();
            (touched, Progress { files })
        };
        if incoming.cancel.is_cancelled() {
            return;
        }
        let parts: Vec<PathBuf> = touched
            .iter()
            .map(|i| incoming.part_path(&incoming.manifest.entries[*i as usize]))
            .collect();
        let path = self.incoming_dir(&incoming.id).join("progress.json");
        let _ = tokio::task::spawn_blocking(move || {
            for part in parts {
                if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&part) {
                    let _ = file.sync_data();
                }
            }
            if path.parent().is_some_and(Path::exists) {
                let _ = serde_json::to_vec(&progress)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| write_atomic(&path, &bytes));
            }
        })
        .await;
    }
}

fn incoming_from(meta: Meta, progress: Progress) -> Incoming {
    let mut state = RecvState::default();
    let manifest = Manifest {
        entries: meta.entries,
    };
    for have in progress.files {
        let Some(entry) = manifest
            .entries
            .get(have.file as usize)
            .filter(|e| e.kind == EntryKind::File)
        else {
            continue;
        };
        let count = block_count(entry.size);
        if !have.blocks.within(count) && !have.blocks.is_empty() {
            continue;
        }
        state.done_bytes += if have.done {
            entry.size
        } else {
            have.blocks.bytes(entry.size)
        };
        state.files.insert(
            have.file,
            FileState {
                blocks: if have.done {
                    RangeSet::full(count)
                } else {
                    have.blocks
                },
                done: have.done,
                ..Default::default()
            },
        );
    }
    Incoming {
        id: meta.id,
        peer: meta.peer,
        digest: meta.digest,
        manifest,
        dest_root: meta.dest_root,
        tops: meta.tops,
        state: Mutex::new(state),
        session: Mutex::new(None),
        cancel: CancellationToken::new(),
    }
}

fn new_row(
    id: &str,
    peer: &str,
    peer_name: &str,
    manifest: &Manifest,
    skipped: u64,
) -> FileTransfer {
    let now = now_ms();
    FileTransfer {
        id: id.to_owned(),
        direction: FileTransferDirection::Incoming,
        peer_device_id: peer.to_owned(),
        peer_device_name: peer_name.to_owned(),
        state: FileTransferState::Connecting,
        transport: None,
        items: manifest.items(),
        file_count: manifest.file_count(),
        total_bytes: manifest.total_bytes(),
        done_bytes: 0,
        bytes_per_sec: 0,
        destination: None,
        skipped,
        error: None,
        created_at: now,
        updated_at: now,
        finished_at: None,
    }
}

fn incoming_row(incoming: &Incoming, peer_name: &str, state: FileTransferState) -> FileTransfer {
    let mut row = new_row(
        &incoming.id,
        &incoming.peer,
        peer_name,
        &incoming.manifest,
        0,
    );
    row.state = state;
    row.destination = Some(incoming.dest_root.to_string_lossy().into_owned());
    row.done_bytes = incoming.state.lock().unwrap().done_bytes;
    for (item, entry) in row.items.iter_mut().zip(
        incoming
            .manifest
            .entries
            .iter()
            .filter(|e| e.is_top_level()),
    ) {
        item.path = Some(incoming.final_path(entry).to_string_lossy().into_owned());
    }
    row
}

/// A manifest directory: create it, or accept an existing real directory
/// (a resumed transfer) — never a symlink or file in its place.
fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path)?;
            anyhow::ensure!(
                meta.is_dir(),
                "{} exists and is not a folder",
                path.display()
            );
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn open_part(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    Ok(options.open(path)?)
}

fn write_at(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(data, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut written = 0;
        while written < data.len() {
            written += file.seek_write(&data[written..], offset + written as u64)?;
        }
        Ok(())
    }
}

/// Check the assembled part against the sender's digest and move it into
/// place. `Ok(None)` = mismatch (resend); `Ok(Some(path))` = where it landed.
fn finalize_file(
    part: &Path,
    target: &Path,
    size: u64,
    mode: u32,
    expected: &str,
    top_level: bool,
) -> anyhow::Result<Option<PathBuf>> {
    let file = open_part(part)?;
    if size == 0 {
        file.set_len(0)?;
    }
    anyhow::ensure!(
        file.metadata()?.len() >= size,
        "{} is shorter than expected",
        part.display()
    );
    file.set_len(size)?;
    let mut hasher = Sha256::new();
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, &file);
    std::io::copy(&mut reader, &mut hasher)?;
    if hex(&hasher.finalize()) != expected {
        return Ok(None);
    }
    file.sync_all()?;
    #[cfg(unix)]
    if mode != 0 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(part, std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut landed = target.to_path_buf();
    if top_level {
        // Something appeared under the reserved name meanwhile: keep both.
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut n = 2;
        while std::fs::symlink_metadata(&landed).is_ok() {
            landed = target.with_file_name(manifest::numbered(&name, n));
            n += 1;
        }
    }
    std::fs::rename(part, &landed)?;
    Ok(Some(landed))
}

/// Last step: symlinks (as symlinks) and folder permissions, deepest first
/// so a read-only folder never blocks its children.
fn finish_tree(incoming: &Incoming) -> anyhow::Result<()> {
    for entry in incoming
        .manifest
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::Symlink)
    {
        let path = incoming.final_path(entry);
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.target.as_deref().unwrap_or_default(), &path)?;
        #[cfg(not(unix))]
        tracing::debug!(path = %path.display(), "symlinks are not recreated on this platform");
    }
    #[cfg(unix)]
    for entry in incoming
        .manifest
        .entries
        .iter()
        .rev()
        .filter(|e| e.kind == EntryKind::Dir && e.mode != 0)
    {
        use std::os::unix::fs::PermissionsExt;
        // Keep folders writable by their owner: received files must stay
        // removable from the inbox.
        let mode = (entry.mode & 0o777) | 0o700;
        let _ = std::fs::set_permissions(
            incoming.final_path(entry),
            std::fs::Permissions::from_mode(mode),
        );
    }
    Ok(())
}

/// Remove unfinished `*.part` files and the folders left empty by them.
fn remove_parts(incoming: &Incoming) {
    let done: Vec<u64> = {
        let state = incoming.state.lock().unwrap();
        state
            .files
            .iter()
            .filter(|(_, f)| f.done)
            .map(|(i, _)| *i)
            .collect()
    };
    for (index, entry) in incoming.file_entries() {
        if !done.contains(&index) {
            let _ = std::fs::remove_file(incoming.part_path(entry));
        }
    }
    for entry in incoming
        .manifest
        .entries
        .iter()
        .rev()
        .filter(|e| e.kind == EntryKind::Dir)
    {
        let _ = std::fs::remove_dir(incoming.final_path(entry));
    }
}

/// `Some(message)` when the destination volume can't take `bytes` more.
fn space_problem(dest: &Path, bytes: u64) -> Option<String> {
    #[cfg(unix)]
    {
        let mut probe = dest.to_path_buf();
        while !probe.exists() {
            probe = probe.parent()?.to_path_buf();
        }
        let path = std::ffi::CString::new(probe.to_string_lossy().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let available = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
        if available < bytes.saturating_add(SPACE_MARGIN) {
            return Some(format!(
                "Not enough space on the receiving device: needs {} MB, {} MB free",
                bytes / 1_000_000 + 1,
                available / 1_000_000
            ));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = (dest, bytes);
        None
    }
}
