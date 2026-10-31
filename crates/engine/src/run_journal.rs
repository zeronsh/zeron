//! Per-session on-disk event journal (port of zeron's `run-journal.ts`, JSONL-shaped).
//!
//! One append-only JSONL file per chat under `{data_dir}/journals/{chat_id}.jsonl`; each
//! line is `{"seq": n, "event": AgentEvent}` with a monotonically increasing `seq`. The
//! journal is the durable replay source for live streams (`Subscribe` = replay then tail
//! the broadcast hub) and the crash-recovery gauge: a journal whose LAST event is not
//! `Done` belongs to a run that died mid-stream — boot recovery stamps its doc entry
//! `aborted` and closes the journal with a synthetic `Done`. A turn the engine's own
//! shutdown cut off closes normally, so a `{chat_id}.shutdown` marker beside the
//! journal flags it for the same pick-up on the next boot.
//!
//! Bounded-window compaction is not done: existing files are append-only and are never
//! rewritten, truncated, or deleted except by the test-only [`RunJournal::discard`]. A torn
//! trailing line from a crash mid-write is tolerated everywhere.
//!
//! Appends are queued to one writer thread, which batches consecutive lines per chat into
//! a single `write_all`. A caller that needs today's durability (`append`, and the session
//! barriers at `SessionStarted`, `Done`, and shutdown) waits until that write has returned.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use zeron_proto::AgentEvent;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalLine {
    seq: u64,
    event: AgentEvent,
}

/// Serialize shape of one line. Field order matches [`JournalLine`], so the bytes
/// are identical to the owned encoding.
#[derive(Serialize)]
struct JournalLineRef<'a> {
    seq: u64,
    event: &'a AgentEvent,
}

/// Per-chat seq assignment. The writer thread owns the file descriptor; this
/// mutex covers the counter and the send-order queue only.
struct ChatState {
    next_seq: u64,
    /// Next seq to hand the writer. Holes stay queued until the missing seq arrives,
    /// so two threads appending to one chat cannot invert lines.
    next_send: u64,
    ready: BTreeMap<u64, ReadyLine>,
    loaded: bool,
    /// Hash of the last `AvailableCommands` recorded for this chat. `None` until
    /// seeded from the file or the first record.
    commands_hash: Option<[u8; 32]>,
    commands_seq: u64,
}

impl ChatState {
    fn fresh() -> Self {
        Self {
            next_seq: 1,
            next_send: 1,
            ready: BTreeMap::new(),
            loaded: false,
            commands_hash: None,
            commands_seq: 0,
        }
    }
}

struct ReadyLine {
    /// `None` is a hole (serialization failed): the writer is not involved.
    line: Option<Vec<u8>>,
    ack: Option<SyncSender<Result<(), JournalError>>>,
    error: Option<String>,
}

struct OpenJournal {
    file: File,
    /// True when the file ends without a newline (torn write) — the next append
    /// starts with one so the torn line stays isolated.
    needs_newline: bool,
    last_used: u64,
}

enum Job {
    Write {
        chat_id: String,
        seq: u64,
        line: Vec<u8>,
        ack: Option<SyncSender<Result<(), JournalError>>>,
    },
    /// Every write queued before this job is on disk (or has failed).
    Barrier {
        ack: SyncSender<Result<(), JournalError>>,
    },
    Forget {
        chat_id: String,
        ack: SyncSender<()>,
    },
    Stop,
}

/// How many appends may sit in the writer queue before publishers block.
const JOURNAL_QUEUE: usize = 256;
/// Upper bound on jobs pulled into one batch. The writer also stops at the first
/// empty `try_recv`, so a quiet journal still writes each line promptly.
const JOURNAL_BATCH: usize = 64;
/// Open descriptors on the writer. Past this, the least recently written chat is
/// closed; the next append reopens it and rescans the tail.
const OPEN_FILE_CAP: usize = 16;

/// Append-only JSONL journal store, one file per chat.
pub struct RunJournal {
    dir: PathBuf,
    /// Lookup and insert only. Per-chat ordering lives on [`ChatState`].
    chats: Mutex<HashMap<String, Arc<Mutex<ChatState>>>>,
    /// Held across enqueue and across [`Self::discard`], so a discard cannot
    /// delete a file a publisher has already sequenced but not yet sent.
    gate: Mutex<()>,
    tx: Mutex<Option<SyncSender<Job>>>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Seqs whose `write_all` failed. The writer appends here; publishers drain
    /// their chat's entries so a failed `AvailableCommands` record is not treated
    /// as already on disk.
    failed_writes: Arc<Mutex<Vec<(String, u64)>>>,
}

impl RunJournal {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, JournalError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let (tx, rx) = mpsc::sync_channel(JOURNAL_QUEUE);
        let failed_writes = Arc::new(Mutex::new(Vec::new()));
        let failed = Arc::clone(&failed_writes);
        let writer_dir = dir.clone();
        let writer = std::thread::Builder::new()
            .name("zeron-journal".into())
            .spawn(move || writer_loop(writer_dir, rx, failed))?;
        Ok(Self {
            dir,
            chats: Mutex::new(HashMap::new()),
            gate: Mutex::new(()),
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            failed_writes,
        })
    }

    fn path_for(&self, chat_id: &str) -> PathBuf {
        journal_file(&self.dir, chat_id)
    }

    fn attempts_path(&self, chat_id: &str) -> PathBuf {
        self.dir.join(format!("{}.resume", sanitize_id(chat_id)))
    }

    fn shutdown_path(&self, chat_id: &str) -> PathBuf {
        self.dir.join(format!("{}.shutdown", sanitize_id(chat_id)))
    }

    fn stop_path(&self, chat_id: &str) -> PathBuf {
        self.dir.join(format!("{}.stopped", sanitize_id(chat_id)))
    }

    /// Auto-resume revival budget (zeron `resumeAttempt`/`MAX_AUTO_RESUME`):
    /// persisted beside the journal so a run that CRASHES THE ENGINE cannot
    /// revive itself in an infinite boot loop. Stored as `{count} {last_ms}`;
    /// a ledger whose last revival is older than `since_ms` has expired (a
    /// chain from days ago must not refuse today's crash). A bare count (the
    /// older format) never expires.
    pub fn resume_attempts(&self, chat_id: &str, since_ms: i64) -> u32 {
        let Ok(raw) = std::fs::read_to_string(self.attempts_path(chat_id)) else {
            return 0;
        };
        let mut fields = raw.split_whitespace();
        let count = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        match fields.next().and_then(|s| s.parse::<i64>().ok()) {
            Some(last) if last < since_ms => 0,
            _ => count,
        }
    }

    /// Record one revival at `at_ms`, counting on from the unexpired ledger
    /// (`since_ms` as in [`Self::resume_attempts`]); returns the new count.
    pub fn note_resume_attempt(&self, chat_id: &str, at_ms: i64, since_ms: i64) -> u32 {
        let next = self.resume_attempts(chat_id, since_ms) + 1;
        if let Err(err) = std::fs::write(self.attempts_path(chat_id), format!("{next} {at_ms}")) {
            tracing::warn!(chat = %chat_id, error = %err, "resume-attempt ledger write failed");
        }
        next
    }

    /// Mark a chat whose turn the engine's own shutdown cut off (at `at_ms`):
    /// its journal closes with an ordinary `Done{interrupted}`, so this marker
    /// is what tells the next boot the turn was not the user's to end.
    pub fn note_shutdown_interrupt(&self, chat_id: &str, at_ms: i64) {
        if let Err(err) = std::fs::write(self.shutdown_path(chat_id), at_ms.to_string()) {
            tracing::warn!(chat = %chat_id, error = %err, "shutdown marker write failed");
        }
    }

    /// Chats marked by [`Self::note_shutdown_interrupt`], with when.
    pub fn shutdown_interrupts(&self) -> Result<Vec<(String, i64)>, JournalError> {
        let mut marked = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("shutdown") {
                continue;
            }
            let Some(chat_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let at = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            marked.push((chat_id.to_string(), at));
        }
        marked.sort();
        Ok(marked)
    }

    pub fn clear_shutdown_interrupt(&self, chat_id: &str) {
        let _ = std::fs::remove_file(self.shutdown_path(chat_id));
    }

    /// Mark a chat whose turn the user stopped. Its journal closes only once
    /// the stopped run settles (seconds later, for a harness slow to let
    /// go): an engine that dies in between leaves a journal that reads as a
    /// crash, and this marker keeps the next boot from reviving the turn.
    pub fn note_user_stop(&self, chat_id: &str) {
        if let Err(err) = std::fs::write(self.stop_path(chat_id), b"") {
            tracing::warn!(chat = %chat_id, error = %err, "stop marker write failed");
        }
    }

    /// Whether [`Self::note_user_stop`] marked the chat's last turn.
    pub fn user_stopped(&self, chat_id: &str) -> bool {
        self.stop_path(chat_id).exists()
    }

    pub fn clear_user_stop(&self, chat_id: &str) {
        let _ = std::fs::remove_file(self.stop_path(chat_id));
    }

    /// A cleanly completed turn resets the budget — only consecutive
    /// crash-revive-crash cycles exhaust it.
    pub fn clear_resume_attempts(&self, chat_id: &str) {
        let _ = std::fs::remove_file(self.attempts_path(chat_id));
    }

    /// Append one event; returns its journal seq. Waits until `write_all` returns.
    /// An `AvailableCommands` list identical to the last one recorded for this
    /// chat is not written again; the returned seq is that earlier record.
    pub fn append(&self, chat_id: &str, event: &AgentEvent) -> Result<u64, JournalError> {
        self.record(chat_id, event, true).map(|(seq, _)| seq)
    }

    /// Queue `event` without waiting for the write. `recorded` is false when an
    /// unchanged `AvailableCommands` list was skipped; `seq` is then the copy
    /// already on disk (and nothing is broadcast for it).
    pub(crate) fn enqueue(
        &self,
        chat_id: &str,
        event: &AgentEvent,
    ) -> Result<(u64, bool), JournalError> {
        self.record(chat_id, event, false)
    }

    /// Block until every append queued before this call has finished `write_all`
    /// (or a write since the previous barrier failed).
    pub fn flush(&self) -> Result<(), JournalError> {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        self.send_job(Job::Barrier { ack: ack_tx })?;
        ack_rx.recv().map_err(|_| writer_stopped())?
    }

    /// Events with `seq > after_seq`, in order. A cursor ahead of the last issued
    /// seq is from a previous era (file replaced) — falls back to a full replay.
    /// `after_seq > 0` reads backwards and stops at the first seq at or below the
    /// cursor instead of parsing the whole file.
    pub fn replay(
        &self,
        chat_id: &str,
        after_seq: u64,
    ) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
        self.flush()?;
        let path = self.path_for(chat_id);
        let fast = replay_fast(&path, after_seq)?;
        verify_replay(&path, after_seq, fast)
    }

    /// The last event in a chat's journal, if any (ignores a torn tail line).
    pub fn last_event(&self, chat_id: &str) -> Result<Option<(u64, AgentEvent)>, JournalError> {
        self.flush()?;
        last_valid_line(&self.path_for(chat_id))
    }

    /// Valid events newest-first, read backwards from the end of the file — for
    /// callers that want the most recent match and can stop early instead of
    /// materialising the whole journal like `replay` does.
    pub fn events_rev(
        &self,
        chat_id: &str,
    ) -> Result<impl Iterator<Item = Result<(u64, AgentEvent), JournalError>>, JournalError> {
        self.flush()?;
        RevEvents::open(&self.path_for(chat_id))
    }

    /// Crash-recovery scan: chat ids whose journal's last event is NOT a `Done` — their
    /// runs died mid-stream and need recovery (stamp `aborted`, close the journal).
    pub fn stale_sessions(&self) -> Result<Vec<String>, JournalError> {
        self.flush()?;
        let mut stale = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(chat_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Tail read only: journals reach tens of MB and boot scans every one.
            match last_valid_line(&path)? {
                Some((_, AgentEvent::Done { .. })) | None => {}
                Some(_) => stale.push(chat_id.to_string()),
            }
        }
        stale.sort();
        Ok(stale)
    }

    /// Remove a chat's journal file entirely. Test-only: nothing in the engine
    /// compacts or deletes journal data.
    pub fn discard(&self, chat_id: &str) -> Result<(), JournalError> {
        let _gate = lock_mutex(&self.gate);
        self.flush()?;
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        self.send_job(Job::Forget {
            chat_id: chat_id.to_string(),
            ack: ack_tx,
        })?;
        ack_rx.recv().map_err(|_| writer_stopped())?;
        lock_mutex(&self.chats).remove(chat_id);
        let path = self.path_for(chat_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn record(
        &self,
        chat_id: &str,
        event: &AgentEvent,
        wait: bool,
    ) -> Result<(u64, bool), JournalError> {
        let proposed = match event {
            AgentEvent::AvailableCommands { commands } => hash_commands(commands),
            _ => None,
        };
        // Seed the hash from disk once per process, outside the gate: the scan
        // can walk a large file, and it must not call `events_rev` (that flushes).
        if proposed.is_some() {
            let state = self.chat(chat_id);
            let mut chat = lock_mutex(&state);
            self.apply_failed(chat_id, &mut chat);
            if chat.commands_hash.is_none() {
                drop(chat);
                self.flush()?;
            }
        }

        let _gate = lock_mutex(&self.gate);
        let state = self.chat(chat_id);
        let mut chat = lock_mutex(&state);
        self.ensure_loaded(chat_id, &mut chat)?;
        self.apply_failed(chat_id, &mut chat);
        if let Some(hash) = proposed {
            if chat.commands_hash.is_none()
                && let Some((found, found_seq)) = commands_on_disk(&self.path_for(chat_id))?
            {
                chat.commands_hash = Some(found);
                chat.commands_seq = found_seq;
            }
            if chat.commands_hash == Some(hash) {
                return Ok((chat.commands_seq, false));
            }
        }

        let seq = chat.next_seq;
        chat.next_seq = seq + 1;
        if let Some(hash) = proposed {
            chat.commands_hash = Some(hash);
            chat.commands_seq = seq;
        }
        drop(chat);

        let line = match serde_json::to_vec(&JournalLineRef { seq, event }) {
            Ok(line) => line,
            Err(err) => {
                let mut chat = lock_mutex(&state);
                if proposed.is_some() && chat.commands_seq == seq {
                    chat.commands_hash = None;
                    chat.commands_seq = 0;
                }
                chat.ready.insert(
                    seq,
                    ReadyLine {
                        line: None,
                        ack: None,
                        error: Some(err.to_string()),
                    },
                );
                self.pump(chat_id, &mut chat)?;
                return Err(JournalError::Json(err));
            }
        };

        let (ack_tx, ack_rx) = if wait {
            let (tx, rx) = mpsc::sync_channel(1);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        {
            let mut chat = lock_mutex(&state);
            chat.ready.insert(
                seq,
                ReadyLine {
                    line: Some(line),
                    ack: ack_tx,
                    error: None,
                },
            );
            self.pump(chat_id, &mut chat)?;
        }
        drop(_gate);

        if let Some(rx) = ack_rx {
            match rx.recv() {
                Ok(Ok(())) => Ok((seq, true)),
                Ok(Err(err)) => {
                    let mut chat = lock_mutex(&state);
                    if chat.commands_seq == seq {
                        chat.commands_hash = None;
                        chat.commands_seq = 0;
                    }
                    self.apply_failed(chat_id, &mut chat);
                    Err(err)
                }
                Err(_) => Err(writer_stopped()),
            }
        } else {
            Ok((seq, true))
        }
    }

    fn chat(&self, chat_id: &str) -> Arc<Mutex<ChatState>> {
        let mut chats = lock_mutex(&self.chats);
        chats
            .entry(chat_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(ChatState::fresh())))
            .clone()
    }

    fn ensure_loaded(&self, chat_id: &str, chat: &mut ChatState) -> Result<(), JournalError> {
        if chat.loaded {
            return Ok(());
        }
        let (next_seq, _) = scan_tail(&self.path_for(chat_id))?;
        chat.next_seq = next_seq;
        chat.next_send = next_seq;
        chat.loaded = true;
        Ok(())
    }

    /// Drop this chat's failed write seqs. A failed `AvailableCommands` record
    /// must not suppress the next copy of that list.
    fn apply_failed(&self, chat_id: &str, chat: &mut ChatState) {
        let mut failed = lock_mutex(&self.failed_writes);
        failed.retain(|(id, seq)| {
            if id != chat_id {
                return true;
            }
            if chat.commands_seq == *seq {
                chat.commands_hash = None;
                chat.commands_seq = 0;
            }
            false
        });
    }

    /// Send every ready line from `next_send` upward, in seq order.
    fn pump(&self, chat_id: &str, chat: &mut ChatState) -> Result<(), JournalError> {
        while let Some(ready) = chat.ready.remove(&chat.next_send) {
            let seq = chat.next_send;
            chat.next_send = seq + 1;
            let Some(line) = ready.line else {
                if let Some(ack) = ready.ack {
                    let message = ready
                        .error
                        .unwrap_or_else(|| "journal line dropped".to_string());
                    let _ = ack.send(Err(JournalError::Io(std::io::Error::other(message))));
                }
                continue;
            };
            self.send_job(Job::Write {
                chat_id: chat_id.to_string(),
                seq,
                line,
                ack: ready.ack,
            })?;
        }
        Ok(())
    }

    fn send_job(&self, job: Job) -> Result<(), JournalError> {
        let tx = lock_mutex(&self.tx);
        let Some(tx) = tx.as_ref() else {
            return Err(writer_stopped());
        };
        tx.send(job).map_err(|_| writer_stopped())
    }

    fn stop_writer(&self) {
        let tx = lock_mutex(&self.tx).take();
        if let Some(tx) = tx {
            let _ = tx.send(Job::Stop);
        }
        if let Some(handle) = lock_mutex(&self.writer).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RunJournal {
    fn drop(&mut self) {
        self.stop_writer();
    }
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn writer_stopped() -> JournalError {
    JournalError::Io(std::io::Error::other("journal writer stopped"))
}

fn journal_file(dir: &Path, chat_id: &str) -> PathBuf {
    dir.join(format!("{}.jsonl", sanitize_id(chat_id)))
}

fn hash_commands(commands: &[zeron_proto::SlashCommand]) -> Option<[u8; 32]> {
    let bytes = serde_json::to_vec(commands).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Some(hasher.finalize().into())
}

/// Newest `AvailableCommands` already in the file, if any. Direct tail walk:
/// `RunJournal::events_rev` would flush, and this runs while the caller holds
/// the chat lock.
fn commands_on_disk(path: &Path) -> Result<Option<([u8; 32], u64)>, JournalError> {
    for item in RevEvents::open(path)? {
        let (seq, event) = item?;
        if let AgentEvent::AvailableCommands { commands } = event {
            return Ok(hash_commands(&commands).map(|hash| (hash, seq)));
        }
    }
    Ok(None)
}

fn writer_loop(dir: PathBuf, rx: Receiver<Job>, failed: Arc<Mutex<Vec<(String, u64)>>>) {
    let mut open = HashMap::new();
    let mut clock = 0u64;
    let mut last_error: Option<String> = None;
    let mut deferred: Option<Job> = None;
    loop {
        let job = if let Some(job) = deferred.take() {
            job
        } else {
            match rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };
        match job {
            Job::Write {
                chat_id,
                seq,
                line,
                ack,
            } => {
                let mut batch = vec![Job::Write {
                    chat_id,
                    seq,
                    line,
                    ack,
                }];
                while batch.len() < JOURNAL_BATCH {
                    match rx.try_recv() {
                        Ok(next @ Job::Write { .. }) => batch.push(next),
                        Ok(next) => {
                            deferred = Some(next);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                write_batch(&dir, &mut open, &mut clock, &mut last_error, &failed, batch);
            }
            other => {
                if handle_control(&mut open, &mut last_error, other) {
                    break;
                }
            }
        }
    }
}

fn handle_control(
    open: &mut HashMap<String, OpenJournal>,
    last_error: &mut Option<String>,
    job: Job,
) -> bool {
    match job {
        Job::Barrier { ack } => {
            let result = match last_error.take() {
                Some(message) => Err(JournalError::Io(std::io::Error::other(message))),
                None => Ok(()),
            };
            let _ = ack.send(result);
            false
        }
        Job::Forget { chat_id, ack } => {
            open.remove(&chat_id);
            let _ = ack.send(());
            false
        }
        Job::Stop => true,
        Job::Write { .. } => false,
    }
}

fn write_batch(
    dir: &Path,
    open: &mut HashMap<String, OpenJournal>,
    clock: &mut u64,
    last_error: &mut Option<String>,
    failed: &Mutex<Vec<(String, u64)>>,
    batch: Vec<Job>,
) {
    let mut order = Vec::new();
    let mut groups: HashMap<
        String,
        Vec<(u64, Vec<u8>, Option<SyncSender<Result<(), JournalError>>>)>,
    > = HashMap::new();
    for job in batch {
        let Job::Write {
            chat_id,
            seq,
            line,
            ack,
        } = job
        else {
            continue;
        };
        if !groups.contains_key(&chat_id) {
            order.push(chat_id.clone());
        }
        groups.entry(chat_id).or_default().push((seq, line, ack));
    }
    for chat_id in order {
        let Some(lines) = groups.remove(&chat_id) else {
            continue;
        };
        let result = write_chat(dir, open, clock, &chat_id, &lines);
        if let Err(err) = &result {
            tracing::error!(chat = %chat_id, error = %err, "journal write failed");
            *last_error = Some(err.to_string());
            open.remove(&chat_id);
            let mut failed = lock_mutex(failed);
            for (seq, _, _) in &lines {
                failed.push((chat_id.clone(), *seq));
            }
        }
        for (_, _, ack) in lines {
            let Some(ack) = ack else { continue };
            let _ = ack.send(match &result {
                Ok(()) => Ok(()),
                Err(err) => Err(JournalError::Io(std::io::Error::other(err.to_string()))),
            });
        }
    }
}

fn write_chat(
    dir: &Path,
    open: &mut HashMap<String, OpenJournal>,
    clock: &mut u64,
    chat_id: &str,
    lines: &[(u64, Vec<u8>, Option<SyncSender<Result<(), JournalError>>>)],
) -> Result<(), JournalError> {
    let journal = open_chat(dir, open, clock, chat_id)?;
    let borrowed: Vec<&[u8]> = lines.iter().map(|(_, line, _)| line.as_slice()).collect();
    let buf = encode_batch(journal.needs_newline, &borrowed);
    journal.file.write_all(&buf)?;
    journal.needs_newline = false;
    Ok(())
}

fn open_chat<'a>(
    dir: &Path,
    open: &'a mut HashMap<String, OpenJournal>,
    clock: &mut u64,
    chat_id: &str,
) -> Result<&'a mut OpenJournal, JournalError> {
    if !open.contains_key(chat_id) {
        if open.len() >= OPEN_FILE_CAP {
            if let Some(victim) = open
                .iter()
                .min_by_key(|(_, journal)| journal.last_used)
                .map(|(id, _)| id.clone())
            {
                open.remove(&victim);
            }
        }
        let path = journal_file(dir, chat_id);
        let (_, needs_newline) = scan_tail(&path)?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        *clock = clock.wrapping_add(1);
        open.insert(
            chat_id.to_string(),
            OpenJournal {
                file,
                needs_newline,
                last_used: *clock,
            },
        );
    } else {
        *clock = clock.wrapping_add(1);
        if let Some(journal) = open.get_mut(chat_id) {
            journal.last_used = *clock;
        }
    }
    match open.get_mut(chat_id) {
        Some(journal) => Ok(journal),
        None => Err(JournalError::Io(std::io::Error::other(
            "journal fd missing after open",
        ))),
    }
}

/// Bytes the writer appends for one batch: a leading newline only when the
/// file ends mid-line, then each serialized line plus its own newline.
/// `File::flush` is intentionally not called; durability is `write_all`.
fn encode_batch(needs_newline: bool, lines: &[&[u8]]) -> Vec<u8> {
    let mut cap = lines.iter().map(|line| line.len() + 1).sum::<usize>();
    if needs_newline {
        cap += 1;
    }
    let mut buf = Vec::with_capacity(cap);
    if needs_newline {
        buf.push(b'\n');
    }
    for line in lines {
        buf.extend_from_slice(line);
        buf.push(b'\n');
    }
    buf
}

fn replay_full(path: &Path, after_seq: u64) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
    let all = read_lines(path)?;
    let last_seq = all.last().map(|(seq, _)| *seq).unwrap_or(0);
    let from = if after_seq > last_seq { 0 } else { after_seq };
    Ok(all.into_iter().filter(|(seq, _)| *seq > from).collect())
}

fn replay_fast(path: &Path, after_seq: u64) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
    if after_seq == 0 || !path.exists() {
        return replay_full(path, after_seq);
    }
    let Some((last_seq, _)) = last_valid_line(path)? else {
        return Ok(Vec::new());
    };
    if after_seq > last_seq {
        return replay_full(path, after_seq);
    }
    let mut out = Vec::new();
    for item in RevEvents::open(path)? {
        let (seq, event) = item?;
        if seq <= after_seq {
            break;
        }
        out.push((seq, event));
    }
    out.reverse();
    Ok(out)
}

fn verify_replay(
    path: &Path,
    after_seq: u64,
    fast: Vec<(u64, AgentEvent)>,
) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
    if !replay_verify_this_call() {
        return Ok(fast);
    }
    let full = replay_full(path, after_seq)?;
    if full != fast {
        tracing::error!(
            path = %path.display(),
            after_seq,
            fast = fast.len(),
            full = full.len(),
            "journal replay diverged from the full read; using the full result"
        );
        return Ok(full);
    }
    Ok(fast)
}

fn replay_verify_this_call() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled =
        *ENABLED.get_or_init(|| std::env::var("ZERON_PERF_VERIFY").ok().as_deref() == Some("1"));
    if !enabled {
        return false;
    }
    static TICKS: AtomicU64 = AtomicU64::new(0);
    TICKS.fetch_add(1, Ordering::Relaxed) % 32 == 0
}

/// Parse every valid line; malformed lines (torn tail writes) are skipped.
fn read_lines(path: &Path) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        if let Some(parsed) = parse_line(path, &line?) {
            out.push(parsed);
        }
    }
    Ok(out)
}

/// One journal line → `(seq, event)`; blank lines are skipped silently, malformed
/// ones (torn tail writes) with a warning.
fn parse_line(path: &Path, line: &str) -> Option<(u64, AgentEvent)> {
    if line.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<JournalLine>(line) {
        Ok(parsed) => Some((parsed.seq, parsed.event)),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "journal: skipping malformed line");
            None
        }
    }
}

/// The last valid event in the file — what `read_lines(path).last()` returns, without
/// reading more than the tail.
fn last_valid_line(path: &Path) -> Result<Option<(u64, AgentEvent)>, JournalError> {
    RevEvents::open(path)?.next().transpose()
}

/// Next seq (last valid seq + 1, starting at 1) and whether the file ends mid-line.
fn scan_tail(path: &Path) -> Result<(u64, bool), JournalError> {
    let Some(mut lines) = RevLines::open(path)? else {
        return Ok((1, false));
    };
    let needs_newline = lines.ends_mid_line()?;
    let next_seq = RevEvents::from_lines(path, Some(lines))
        .next()
        .transpose()?
        .map(|(seq, _)| seq + 1)
        .unwrap_or(1);
    Ok((next_seq, needs_newline))
}

/// First tail-read size. Journal lines are usually far shorter; a longer line grows the
/// read geometrically, so a multi-MB line costs O(len) rather than O(len²).
const TAIL_CHUNK: usize = 64 * 1024;

/// Raw lines of a file, last to first, read backwards in chunks — memory is bounded by
/// the longest line rather than the file. `\n` separators are stripped (a preceding `\r`
/// is left in place; JSON parsing treats it as whitespace), so a file ending in `\n`
/// yields an empty line first, which callers skip like any blank line.
struct RevLines {
    file: File,
    /// File offset of `buf[0]`; bytes before it have not been read yet.
    pos: u64,
    /// Read but not yet yielded: the file's bytes `[pos, pos + buf.len())`.
    buf: Vec<u8>,
    finished: bool,
}

impl RevLines {
    /// `None` when the file does not exist (an empty journal, as `read_lines` treats it).
    fn open(path: &Path) -> std::io::Result<Option<Self>> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let pos = file.metadata()?.len();
        Ok(Some(Self {
            file,
            pos,
            buf: Vec::new(),
            finished: false,
        }))
    }

    /// Prepend the previous chunk of the file to `buf` (at least as large as `buf`).
    fn fill(&mut self) -> std::io::Result<()> {
        let want = TAIL_CHUNK.max(self.buf.len()) as u64;
        let n = want.min(self.pos);
        self.pos -= n;
        let mut chunk = vec![0u8; n as usize];
        self.file.seek(SeekFrom::Start(self.pos))?;
        self.file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&self.buf);
        self.buf = chunk;
        Ok(())
    }

    /// True when the file is non-empty and its last byte is not `\n` (a torn write).
    /// Call before the first `next_line`.
    fn ends_mid_line(&mut self) -> std::io::Result<bool> {
        if self.buf.is_empty() && self.pos > 0 {
            self.fill()?;
        }
        Ok(self.buf.last().is_some_and(|b| *b != b'\n'))
    }

    fn next_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if self.finished {
                return Ok(None);
            }
            if let Some(i) = self.buf.iter().rposition(|b| *b == b'\n') {
                let line = self.buf.split_off(i + 1);
                self.buf.truncate(i);
                return Ok(Some(line));
            }
            if self.pos == 0 {
                self.finished = true;
                return Ok(Some(std::mem::take(&mut self.buf)));
            }
            self.fill()?;
        }
    }
}

/// Valid journal events newest-first: `read_lines` in reverse, with the same
/// blank/malformed-line skipping, reading only as far back as the caller iterates.
struct RevEvents {
    path: PathBuf,
    /// `None` once exhausted, after an I/O error, or when the file does not exist.
    lines: Option<RevLines>,
}

impl RevEvents {
    fn open(path: &Path) -> Result<Self, JournalError> {
        Ok(Self::from_lines(path, RevLines::open(path)?))
    }

    fn from_lines(path: &Path, lines: Option<RevLines>) -> Self {
        Self {
            path: path.to_path_buf(),
            lines,
        }
    }
}

impl Iterator for RevEvents {
    type Item = Result<(u64, AgentEvent), JournalError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let raw = match self.lines.as_mut()?.next_line() {
                Ok(Some(raw)) => raw,
                Ok(None) => {
                    self.lines = None;
                    return None;
                }
                Err(err) => {
                    self.lines = None;
                    return Some(Err(err.into()));
                }
            };
            // A torn write can split a multi-byte char: treat it as malformed too.
            let Ok(line) = std::str::from_utf8(&raw) else {
                tracing::warn!(path = %self.path.display(), "journal: skipping non-UTF-8 line");
                continue;
            };
            if let Some(parsed) = parse_line(&self.path, line) {
                return Some(Ok(parsed));
            }
        }
    }
}

/// Journal (`.jsonl`) and resume-budget (`.resume`) paths for `chat_id` under an
/// arbitrary journals directory — profile import copies these files between
/// profiles without opening a `RunJournal`.
pub fn journal_paths(dir: &Path, chat_id: &str) -> (PathBuf, PathBuf) {
    let stem = sanitize_id(chat_id);
    (
        dir.join(format!("{stem}.jsonl")),
        dir.join(format!("{stem}.resume")),
    )
}

/// Chat ids become file names; anything outside a conservative set is replaced so a
/// hostile id cannot traverse paths. (Ids are uuids in practice.)
fn sanitize_id(chat_id: &str) -> String {
    chat_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::DoneStatus;

    fn text(s: &str) -> AgentEvent {
        AgentEvent::TextDelta { text: s.into() }
    }

    fn done() -> AgentEvent {
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        }
    }

    #[test]
    fn appends_are_monotonic_and_replayable() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.append("chat-1", &text("a")).unwrap(), 1);
        assert_eq!(journal.append("chat-1", &text("b")).unwrap(), 2);
        assert_eq!(journal.append("chat-1", &done()).unwrap(), 3);

        let all = journal.replay("chat-1", 0).unwrap();
        assert_eq!(all.len(), 3);
        let after = journal.replay("chat-1", 2).unwrap();
        assert_eq!(after.len(), 1);
        assert!(matches!(after[0].1, AgentEvent::Done { .. }));
        // Era fallback: cursor ahead of last seq replays everything.
        assert_eq!(journal.replay("chat-1", 99).unwrap().len(), 3);
    }

    #[test]
    fn seq_continues_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let journal = RunJournal::open(dir.path()).unwrap();
            journal.append("chat-1", &text("a")).unwrap();
        }
        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.append("chat-1", &text("b")).unwrap(), 2);
    }

    #[test]
    fn stale_scan_flags_journals_without_terminal_done() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("dead", &text("partial")).unwrap();
        journal.append("clean", &text("full")).unwrap();
        journal.append("clean", &done()).unwrap();
        assert_eq!(journal.stale_sessions().unwrap(), vec!["dead".to_string()]);
        // Closing the stale journal with a Done clears the flag.
        journal.append("dead", &done()).unwrap();
        assert!(journal.stale_sessions().unwrap().is_empty());
    }

    #[test]
    fn resume_ledger_counts_on_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.resume_attempts("chat-1", 0), 0);
        assert_eq!(journal.note_resume_attempt("chat-1", 1_000, 0), 1);
        assert_eq!(journal.note_resume_attempt("chat-1", 2_000, 0), 2);
        assert_eq!(journal.resume_attempts("chat-1", 2_000), 2);
        // The last revival is older than the window: the chain has expired.
        assert_eq!(journal.resume_attempts("chat-1", 2_001), 0);
        assert_eq!(journal.note_resume_attempt("chat-1", 9_000, 5_000), 1);
        journal.clear_resume_attempts("chat-1");
        assert_eq!(journal.resume_attempts("chat-1", 0), 0);
        // The older bare-count format still counts, and never expires.
        std::fs::write(dir.path().join("chat-2.resume"), "3").unwrap();
        assert_eq!(journal.resume_attempts("chat-2", i64::MAX), 3);
    }

    #[test]
    fn shutdown_markers_list_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("chat-b", &done()).unwrap();
        journal.note_shutdown_interrupt("chat-b", 42);
        journal.note_shutdown_interrupt("chat-a", 7);
        assert_eq!(
            journal.shutdown_interrupts().unwrap(),
            vec![("chat-a".to_string(), 7), ("chat-b".to_string(), 42)]
        );
        // A marker never makes a closed journal stale.
        assert!(journal.stale_sessions().unwrap().is_empty());
        journal.clear_shutdown_interrupt("chat-a");
        assert_eq!(
            journal.shutdown_interrupts().unwrap(),
            vec![("chat-b".to_string(), 42)]
        );
    }

    #[test]
    fn torn_tail_line_is_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        {
            let journal = RunJournal::open(dir.path()).unwrap();
            journal.append("chat-1", &text("a")).unwrap();
        }
        // Simulate a crash mid-write: garbage with no trailing newline.
        let path = dir.path().join("chat-1.jsonl");
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"seq\":2,\"event\":{\"type\":\"textD")
            .unwrap();
        drop(f);

        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.replay("chat-1", 0).unwrap().len(), 1);
        assert_eq!(journal.append("chat-1", &text("b")).unwrap(), 2);
        let all = journal.replay("chat-1", 0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].0, 2);
    }

    fn line(seq: u64, event: AgentEvent) -> String {
        serde_json::to_string(&JournalLine { seq, event }).unwrap()
    }

    fn rev_lines(path: &Path) -> Vec<Vec<u8>> {
        let Some(mut lines) = RevLines::open(path).unwrap() else {
            return Vec::new();
        };
        std::iter::from_fn(|| lines.next_line().unwrap()).collect()
    }

    /// Journal contents covering the tail reader's edge cases; each must read the same
    /// backwards as the full forward parse did.
    fn tail_cases() -> Vec<(&'static str, Vec<u8>)> {
        let huge = "x".repeat(5 * TAIL_CHUNK + 123);
        let a = line(1, text("a"));
        let b = line(2, text("b"));
        let big = line(3, text(&huge));
        let fin = line(4, done());
        vec![
            ("empty", Vec::new()),
            ("only-newlines", b"\n\n\r\n  \n".to_vec()),
            ("trailing-newline", format!("{a}\n{b}\n").into_bytes()),
            ("no-trailing-newline", format!("{a}\n{b}").into_bytes()),
            ("crlf", format!("{a}\r\n{b}\r\n").into_bytes()),
            (
                "blank-lines-at-end",
                format!("{a}\n{b}\n\n  \n").into_bytes(),
            ),
            (
                "garbage-last-line",
                format!("{a}\n{b}\n{{\"seq\":3,\"event\":{{\"type\":\"textD").into_bytes(),
            ),
            (
                "garbage-then-newline",
                format!("{a}\n{b}\nnot json\n\n").into_bytes(),
            ),
            ("all-garbage", b"nope\n{\"seq\":\nstill nope".to_vec()),
            (
                "multi-chunk-last-line",
                format!("{a}\n{big}\n").into_bytes(),
            ),
            (
                "multi-chunk-last-line-unterminated",
                format!("{a}\n{big}").into_bytes(),
            ),
            (
                "multi-chunk-middle-line",
                format!("{a}\n{big}\n{fin}\n").into_bytes(),
            ),
            (
                "multi-chunk-garbage-tail",
                format!("{a}\n{b}\n{}", &big[..big.len() - 7]).into_bytes(),
            ),
            ("multi-chunk-only-line", big.clone().into_bytes()),
        ]
    }

    #[test]
    fn rev_lines_are_forward_lines_reversed() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in tail_cases() {
            let path = dir.path().join(format!("{name}.jsonl"));
            std::fs::write(&path, &bytes).unwrap();
            let mut expected: Vec<Vec<u8>> =
                bytes.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
            expected.reverse();
            assert_eq!(rev_lines(&path), expected, "{name}");
        }
        assert!(rev_lines(&dir.path().join("missing.jsonl")).is_empty());
    }

    #[test]
    fn tail_reads_match_a_full_forward_parse() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in tail_cases() {
            let path = dir.path().join(format!("{name}.jsonl"));
            std::fs::write(&path, &bytes).unwrap();
            let all = read_lines(&path).unwrap();

            // Every valid event, newest-first.
            let mut expected = all.clone();
            expected.reverse();
            let rev: Vec<_> = RevEvents::open(&path)
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(rev, expected, "{name}");

            assert_eq!(
                last_valid_line(&path).unwrap(),
                all.last().cloned(),
                "{name}"
            );
            let old_scan_tail = (
                all.last().map(|(seq, _)| seq + 1).unwrap_or(1),
                bytes.last().is_some_and(|b| *b != b'\n'),
            );
            assert_eq!(scan_tail(&path).unwrap(), old_scan_tail, "{name}");
        }
        let missing = dir.path().join("missing.jsonl");
        assert_eq!(last_valid_line(&missing).unwrap(), None);
        assert_eq!(scan_tail(&missing).unwrap(), (1, false));
    }

    #[test]
    fn stale_sessions_unchanged_by_tail_reads() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in tail_cases() {
            std::fs::write(dir.path().join(format!("{name}.jsonl")), bytes).unwrap();
        }
        std::fs::write(dir.path().join("ignored.resume"), "2").unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();

        // The pre-tail-read implementation: full parse, inspect the last event.
        let mut expected = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let last = read_lines(&path).unwrap().into_iter().next_back();
            if matches!(last, Some((_, ref event)) if !matches!(event, AgentEvent::Done { .. })) {
                expected.push(path.file_stem().unwrap().to_str().unwrap().to_string());
            }
        }
        expected.sort();
        assert_eq!(journal.stale_sessions().unwrap(), expected);
        assert!(expected.contains(&"multi-chunk-garbage-tail".to_string()));
        assert!(!expected.contains(&"multi-chunk-middle-line".to_string()));
    }

    #[test]
    fn append_after_multi_chunk_torn_tail_isolates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat-1.jsonl");
        let big = line(7, text(&"y".repeat(3 * TAIL_CHUNK)));
        std::fs::write(
            &path,
            format!("{}\n{}", line(6, text("a")), &big[..big.len() / 2]),
        )
        .unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.append("chat-1", &text("b")).unwrap(), 7);
        assert!(matches!(
            journal.last_event("chat-1").unwrap(),
            Some((7, AgentEvent::TextDelta { .. }))
        ));
        assert_eq!(journal.replay("chat-1", 0).unwrap().len(), 2);
    }

    #[test]
    fn non_utf8_tail_line_is_skipped_as_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat-1.jsonl");
        let mut bytes = format!("{}\n", line(1, done())).into_bytes();
        // A torn write that split a multi-byte char.
        bytes
            .extend_from_slice(b"{\"seq\":2,\"event\":{\"type\":\"textDelta\",\"text\":\"\xE2\x82");
        std::fs::write(&path, bytes).unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        assert!(matches!(
            journal.last_event("chat-1").unwrap(),
            Some((1, AgentEvent::Done { .. }))
        ));
        assert!(journal.stale_sessions().unwrap().is_empty());
    }

    #[test]
    fn journal_line_ref_matches_owned_line_bytes() {
        let samples = [
            text("plain"),
            text("quotes \" and \\ slash"),
            done(),
            commands("build"),
        ];
        for event in samples {
            let owned = serde_json::to_vec(&JournalLine {
                seq: 7,
                event: event.clone(),
            })
            .unwrap();
            let borrowed = serde_json::to_vec(&JournalLineRef {
                seq: 7,
                event: &event,
            })
            .unwrap();
            assert_eq!(owned, borrowed);
        }
    }

    #[test]
    fn writer_batch_bytes_match_sequential_appends() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("c", &text("a")).unwrap();
        journal.append("c", &text("b")).unwrap();
        journal.append("c", &done()).unwrap();
        let on_disk = std::fs::read(dir.path().join("c.jsonl")).unwrap();
        let lines = [
            serde_json::to_vec(&JournalLineRef {
                seq: 1,
                event: &text("a"),
            })
            .unwrap(),
            serde_json::to_vec(&JournalLineRef {
                seq: 2,
                event: &text("b"),
            })
            .unwrap(),
            serde_json::to_vec(&JournalLineRef {
                seq: 3,
                event: &done(),
            })
            .unwrap(),
        ];
        assert_eq!(
            on_disk,
            encode_batch(false, &[&lines[0], &lines[1], &lines[2]])
        );
    }

    #[test]
    fn concurrent_chats_keep_dense_per_chat_order() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Arc::new(RunJournal::open(dir.path()).unwrap());
        let mut handles = Vec::new();
        for name in ["chat-a", "chat-b"] {
            let journal = Arc::clone(&journal);
            handles.push(std::thread::spawn(move || {
                for i in 0..80 {
                    let seq = journal.append(name, &text(&format!("{name}-{i}"))).unwrap();
                    assert_eq!(seq, i as u64 + 1);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        for name in ["chat-a", "chat-b"] {
            let all = journal.replay(name, 0).unwrap();
            assert_eq!(all.len(), 80, "{name}");
            for (i, (seq, event)) in all.iter().enumerate() {
                assert_eq!(*seq, i as u64 + 1, "{name}");
                assert!(
                    matches!(event, AgentEvent::TextDelta { text } if text == &format!("{name}-{i}"))
                );
            }
        }
    }

    #[test]
    fn one_chat_two_threads_keep_each_caller_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Arc::new(RunJournal::open(dir.path()).unwrap());
        let mut handles = Vec::new();
        for thread_id in 0..2 {
            let journal = Arc::clone(&journal);
            handles.push(std::thread::spawn(move || {
                let mut seqs = Vec::new();
                for i in 0..50 {
                    let seq = journal
                        .append("chat", &text(&format!("{thread_id}-{i}")))
                        .unwrap();
                    seqs.push((seq, format!("{thread_id}-{i}")));
                }
                seqs
            }));
        }
        let mut all = Vec::new();
        for handle in handles {
            let seqs = handle.join().unwrap();
            let mut ordered = seqs.clone();
            ordered.sort_by_key(|(seq, _)| *seq);
            assert_eq!(seqs, ordered, "each caller sees increasing seqs");
            all.extend(seqs);
        }
        all.sort_by_key(|(seq, _)| *seq);
        assert_eq!(
            all.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
            (1..=100).collect::<Vec<_>>()
        );
        let replay = journal.replay("chat", 0).unwrap();
        assert_eq!(replay.len(), 100);
        let texts: Vec<String> = replay
            .iter()
            .map(|(_, event)| match event {
                AgentEvent::TextDelta { text } => text.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        for thread_id in 0..2 {
            let got: Vec<_> = texts
                .iter()
                .filter(|text| text.starts_with(&format!("{thread_id}-")))
                .cloned()
                .collect();
            let expect: Vec<_> = (0..50).map(|i| format!("{thread_id}-{i}")).collect();
            assert_eq!(got, expect);
        }
    }

    #[test]
    fn drop_drains_queued_appends() {
        let dir = tempfile::tempdir().unwrap();
        {
            let journal = RunJournal::open(dir.path()).unwrap();
            for i in 0..100 {
                let (seq, recorded) = journal.enqueue("chat-1", &text(&format!("n{i}"))).unwrap();
                assert!(recorded);
                assert_eq!(seq, i as u64 + 1);
            }
        }
        let journal = RunJournal::open(dir.path()).unwrap();
        let all = journal.replay("chat-1", 0).unwrap();
        assert_eq!(all.len(), 100);
        assert_eq!(all[99].0, 100);
        assert!(matches!(&all[99].1, AgentEvent::TextDelta { text } if text == "n99"));
    }

    #[test]
    fn torn_batch_prefix_is_a_valid_recoverable_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat-1.jsonl");
        let lines = [
            serde_json::to_vec(&JournalLineRef {
                seq: 1,
                event: &text("a"),
            })
            .unwrap(),
            serde_json::to_vec(&JournalLineRef {
                seq: 2,
                event: &text("b"),
            })
            .unwrap(),
            serde_json::to_vec(&JournalLineRef {
                seq: 3,
                event: &done(),
            })
            .unwrap(),
        ];
        let batch = encode_batch(false, &[&lines[0], &lines[1], &lines[2]]);
        let keep = batch.len() - lines[2].len() / 2;
        let torn = batch[..keep].to_vec();
        std::fs::write(&path, &torn).unwrap();

        let journal = RunJournal::open(dir.path()).unwrap();
        let replayed = journal.replay("chat-1", 0).unwrap();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[1].0, 2);
        assert_eq!(
            journal.stale_sessions().unwrap(),
            vec!["chat-1".to_string()]
        );
        assert_eq!(journal.append("chat-1", &text("c")).unwrap(), 3);
        let after = std::fs::read(&path).unwrap();
        assert!(after.starts_with(&torn));
        assert_eq!(journal.replay("chat-1", 1).unwrap().len(), 2);
        assert!(
            journal
                .stale_sessions()
                .unwrap()
                .contains(&"chat-1".to_string())
        );
    }

    #[test]
    fn available_commands_skip_unchanged_lists_and_readers_see_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat-1.jsonl");
        let journal = RunJournal::open(dir.path()).unwrap();
        assert_eq!(journal.append("chat-1", &commands("build")).unwrap(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(journal.append("chat-1", &commands("build")).unwrap(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(journal.append("chat-1", &text("turn")).unwrap(), 2);
        assert_eq!(journal.append("chat-1", &commands("build")).unwrap(), 1);
        let changed = journal.append("chat-1", &commands("test")).unwrap();
        assert_eq!(changed, 3);

        let replay = journal.replay("chat-1", 0).unwrap();
        assert!(matches!(
            &replay.last().unwrap().1,
            AgentEvent::AvailableCommands { commands } if commands[0].name == "test"
        ));
        let rev = journal
            .events_rev("chat-1")
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            &rev[0].1,
            AgentEvent::AvailableCommands { commands } if commands[0].name == "test"
        ));
        drop(journal);

        let journal = RunJournal::open(dir.path()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(journal.append("chat-1", &commands("test")).unwrap(), 3);
        assert_eq!(std::fs::read(&path).unwrap(), before);

        // Copies already in the file stay put; a new list is appended after them.
        let first = serde_json::to_string(&JournalLine {
            seq: 1,
            event: commands("old"),
        })
        .unwrap();
        let second = serde_json::to_string(&JournalLine {
            seq: 2,
            event: commands("old"),
        })
        .unwrap();
        let original = format!("{first}\n{second}\n");
        let archived_path = dir.path().join("archived.jsonl");
        std::fs::write(&archived_path, &original).unwrap();
        assert_eq!(journal.append("archived", &commands("old")).unwrap(), 2);
        assert_eq!(std::fs::read(&archived_path).unwrap(), original.as_bytes());
        assert_eq!(journal.append("archived", &commands("new")).unwrap(), 3);
        assert!(
            std::fs::read(&archived_path)
                .unwrap()
                .starts_with(original.as_bytes())
        );
    }

    #[test]
    fn open_file_cap_reopens_evicted_chats() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        for i in 0..20 {
            let id = format!("c{i:02}");
            assert_eq!(journal.append(&id, &text("a")).unwrap(), 1);
            assert_eq!(journal.append(&id, &text("b")).unwrap(), 2);
        }
        for i in 0..20 {
            let id = format!("c{i:02}");
            assert_eq!(journal.replay(&id, 0).unwrap().len(), 2, "{id}");
            assert_eq!(journal.append(&id, &text("c")).unwrap(), 3, "{id}");
        }
    }

    #[test]
    fn replay_fast_matches_full_over_random_journals() {
        let seeds = std::env::var("PERF_SEEDS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(300);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("random.jsonl");
        for seed_i in 0..seeds {
            let mut state = 0xA5A5_0000_0000_0000u64
                .wrapping_add(seed_i)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
            if state == 0 {
                state = 1;
            }
            let mut bytes = Vec::new();
            let mut seq = 1u64;
            for _ in 0..150 {
                match xorshift(&mut state) % 5 {
                    0 | 1 => {
                        let text = format!("s{seed_i}-{seq}-{}", xorshift(&mut state) % 40);
                        let line = serde_json::to_vec(&JournalLine {
                            seq,
                            event: text_event(&text),
                        })
                        .unwrap();
                        bytes.extend_from_slice(&line);
                        bytes.push(b'\n');
                        seq += 1;
                    }
                    2 => bytes.push(b'\n'),
                    3 => {
                        bytes.extend_from_slice(br#"{"seq":999,"event":{"type":"textD"#);
                        if xorshift(&mut state) % 2 == 0 {
                            bytes.push(b'\n');
                        }
                    }
                    _ => {
                        seq += 1 + (xorshift(&mut state) % 3);
                        let line = serde_json::to_vec(&JournalLine { seq, event: done() }).unwrap();
                        bytes.extend_from_slice(&line);
                        bytes.push(b'\n');
                        seq += 1;
                    }
                }
            }
            std::fs::write(&path, &bytes).unwrap();
            let last = last_valid_line(&path)
                .unwrap()
                .map(|(seq, _)| seq)
                .unwrap_or(0);
            for after in [0, 1, last / 2, last, last.saturating_add(1), 99_999] {
                let fast = replay_fast(&path, after).unwrap();
                let full = replay_full(&path, after).unwrap();
                assert_eq!(fast, full, "seed {seed_i} after {after}");
            }
        }
    }

    fn text_event(s: &str) -> AgentEvent {
        text(s)
    }

    fn commands(name: &str) -> AgentEvent {
        AgentEvent::AvailableCommands {
            commands: vec![zeron_proto::SlashCommand {
                name: name.into(),
                description: "desc".into(),
                input_hint: None,
            }],
        }
    }

    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        *state = x;
        x
    }
}
