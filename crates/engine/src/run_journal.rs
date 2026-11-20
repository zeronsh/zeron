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
//! Bounded-window compaction is deferred (whole file kept for now, per M2 scope); a torn
//! trailing line from a crash mid-write is tolerated everywhere.
//!
//! Lines are written in batches: a streaming agent sends tens of events a second, and
//! a `write` each (under the one lock every chat's appends share) was a large part of
//! a streaming engine's work. An event waits in memory for [`FLUSH_AFTER`] while
//! appends keep coming (or until [`RunJournal::flush`], which a run calls with each doc
//! commit), a turn's `Done` is written at once, and every read writes what's waiting
//! first. What the last appends left waiting (a turn's usage after its `Done`) a
//! flusher thread writes [`IDLE_FLUSH`] later: it sleeps while nothing waits.
//!
//! A write that fails keeps what didn't reach the file to retry with the next write,
//! so a passing failure leaves no gap; only past [`MAX_PENDING`] is it dropped, and
//! that is logged as an error.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use zeron_proto::{AgentEvent, ToolCall};

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

/// A [`JournalLine`] to write, borrowing its event.
#[derive(Serialize)]
struct JournalLineRef<'a> {
    seq: u64,
    event: &'a AgentEvent,
}

/// The longest an appended line waits in memory before it's written.
const FLUSH_AFTER: std::time::Duration = std::time::Duration::from_millis(100);
/// Lines waiting past this many bytes are written at once.
const FLUSH_BYTES: usize = 64 * 1024;
/// The longest a line waits when no append follows it (the flusher thread).
const IDLE_FLUSH: std::time::Duration = std::time::Duration::from_secs(1);
/// What a chat's journal keeps to retry while its writes fail; past it, what waits is
/// dropped (a gap in the journal, logged as an error).
const MAX_PENDING: usize = 8 * 1024 * 1024;

struct ChatJournal {
    file: File,
    next_seq: u64,
    /// True when the file ends without a newline (torn write) — the next append
    /// starts with one so the torn line stays isolated.
    needs_newline: bool,
    /// Lines appended and not yet written, and since when (or since the last failed
    /// write of them, which the flusher retries from).
    pending: Vec<u8>,
    pending_since: Option<std::time::Instant>,
    /// The last write failed (logged once until one succeeds).
    failing: bool,
}

impl ChatJournal {
    /// Write what's waiting. A failure keeps the unwritten rest to retry (it
    /// continues the line it stopped in, so nothing is torn or written twice) and
    /// is `Ok`; `Err` only when lines were dropped, past [`MAX_PENDING`].
    fn write_pending(&mut self, chat: &str) -> Result<(), JournalError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut written = 0;
        let result = loop {
            match self.file.write(&self.pending[written..]) {
                Ok(0) => break Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                Ok(n) => {
                    written += n;
                    if written == self.pending.len() {
                        break Ok(());
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => break Err(err),
            }
        };
        self.pending.drain(..written);
        let Err(err) = result else {
            self.pending_since = None;
            if std::mem::take(&mut self.failing) {
                tracing::info!(chat = %chat, "journal writes recovered");
            }
            return Ok(());
        };
        if self.pending.len() <= MAX_PENDING {
            if !std::mem::replace(&mut self.failing, true) {
                tracing::warn!(chat = %chat, error = %err, waiting = self.pending.len(), "journal write failed; retrying");
            }
            // Retried by the next append, or the flusher a while from now.
            self.pending_since = Some(std::time::Instant::now());
            return Ok(());
        }
        let lost = self.pending.iter().filter(|b| **b == b'\n').count();
        self.pending.clear();
        self.pending_since = None;
        // What reached the file may end mid-line: the next append starts with a
        // newline, so that torn line stays isolated.
        self.needs_newline = true;
        tracing::error!(chat = %chat, error = %err, lost, "journal write failed; events dropped");
        Err(err.into())
    }
}

/// Append-only JSONL journal store, one file per chat.
pub struct RunJournal {
    dir: PathBuf,
    shared: Arc<Shared>,
    flusher: Option<std::thread::JoinHandle<()>>,
}

/// What the journal shares with its flusher thread.
struct Shared {
    open_files: Mutex<HashMap<String, ChatJournal>>,
    /// Lines started waiting, or the journal closed.
    wake: Condvar,
    closed: std::sync::atomic::AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, ChatJournal>> {
        self.open_files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The flusher: writes what has waited [`IDLE_FLUSH`] with no append to take it
    /// along; asleep while nothing waits.
    fn flush_idle(&self) {
        use std::sync::atomic::Ordering;
        let mut files = self.lock();
        while !self.closed.load(Ordering::SeqCst) {
            let now = std::time::Instant::now();
            let mut next: Option<std::time::Instant> = None;
            for (chat, journal) in files.iter_mut() {
                let Some(since) = journal.pending_since else {
                    continue;
                };
                if since + IDLE_FLUSH <= now {
                    // (A drop is logged where it happens.)
                    let _ = journal.write_pending(chat);
                }
                if let Some(since) = journal.pending_since {
                    let due = since + IDLE_FLUSH;
                    next = Some(next.map_or(due, |next| next.min(due)));
                }
            }
            files = match next {
                Some(due) => {
                    self.wake
                        .wait_timeout(files, due.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .wake
                    .wait(files)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

impl Drop for RunJournal {
    fn drop(&mut self) {
        self.shared
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        {
            // (Under the lock: the flusher is waiting, or sees `closed` first.)
            let _files = self.lock();
            self.shared.wake.notify_all();
        }
        if let Some(flusher) = self.flusher.take() {
            let _ = flusher.join();
        }
        self.flush_all();
    }
}

impl RunJournal {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, JournalError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let shared = Arc::new(Shared {
            open_files: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        let flusher = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("journal-flush".into())
                .spawn(move || shared.flush_idle())?
        };
        Ok(Self {
            dir,
            shared,
            flusher: Some(flusher),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, ChatJournal>> {
        self.shared.lock()
    }

    fn path_for(&self, chat_id: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", sanitize_id(chat_id)))
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

    /// Append one event; returns its journal seq.
    pub fn append(&self, chat_id: &str, event: &AgentEvent) -> Result<u64, JournalError> {
        let mut files = self.lock();
        if !files.contains_key(chat_id) {
            // Bound the open-fd set: entries were never removed, so every chat
            // ever run held a descriptor for the process lifetime. Dropping is
            // safe — the next append reopens and rescans the tail. The cap
            // comfortably exceeds concurrent runs, so eviction stays rare.
            const OPEN_FILE_CAP: usize = 16;
            if files.len() >= OPEN_FILE_CAP {
                for (chat, journal) in files.iter_mut() {
                    // (A failure is logged by the write; what still waits is lost
                    // with the entry, as an error.)
                    let _ = journal.write_pending(chat);
                    if !journal.pending.is_empty() {
                        tracing::error!(chat = %chat, bytes = journal.pending.len(), "journal closed with unwritten events");
                    }
                }
                files.clear();
            }
            let path = self.path_for(chat_id);
            let (next_seq, needs_newline) = scan_tail(&path)?;
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            files.insert(
                chat_id.to_string(),
                ChatJournal {
                    file,
                    next_seq,
                    needs_newline,
                    pending: Vec::new(),
                    pending_since: None,
                    failing: false,
                },
            );
        }
        // Entry guaranteed present; avoid unwrap in a library path regardless.
        let Some(journal) = files.get_mut(chat_id) else {
            return Err(JournalError::Io(std::io::Error::other(
                "journal entry vanished under lock",
            )));
        };
        let seq = journal.next_seq;
        let line = serde_json::to_vec(&JournalLineRef { seq, event })?;
        if journal.pending.is_empty() {
            if std::mem::take(&mut journal.needs_newline) {
                journal.pending.push(b'\n');
            }
            journal.pending_since = Some(std::time::Instant::now());
            // The flusher writes it if no append comes to.
            self.shared.wake.notify_one();
        }
        journal.pending.extend_from_slice(&line);
        journal.pending.push(b'\n');
        journal.next_seq = seq + 1;
        let due = matches!(event, AgentEvent::Done { .. })
            || journal.pending.len() >= FLUSH_BYTES
            || journal
                .pending_since
                .is_some_and(|since| since.elapsed() >= FLUSH_AFTER);
        if due {
            journal.write_pending(chat_id)?;
        }
        Ok(seq)
    }

    /// Write what's waiting of a chat's journal (a run calls this with each
    /// doc commit, so the journal is never behind its transcript).
    pub fn flush(&self, chat_id: &str) {
        if let Some(journal) = self.lock().get_mut(chat_id) {
            // (A failure is logged by the write.)
            let _ = journal.write_pending(chat_id);
        }
    }

    /// Write what's waiting of every chat's journal.
    pub fn flush_all(&self) {
        for (chat, journal) in self.lock().iter_mut() {
            let _ = journal.write_pending(chat);
        }
    }

    /// Events with `seq > after_seq`, in order. A cursor ahead of the last issued seq is
    /// from a previous era (file replaced) — falls back to a full replay, mirroring zeron.
    pub fn replay(
        &self,
        chat_id: &str,
        after_seq: u64,
    ) -> Result<Vec<(u64, AgentEvent)>, JournalError> {
        self.flush(chat_id);
        let path = self.path_for(chat_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let all = read_lines(&path)?;
        let last_seq = all.last().map(|(seq, _)| *seq).unwrap_or(0);
        let from = if after_seq > last_seq { 0 } else { after_seq };
        Ok(all.into_iter().filter(|(seq, _)| *seq > from).collect())
    }

    /// The last event in a chat's journal, if any (ignores a torn tail line).
    pub fn last_event(&self, chat_id: &str) -> Result<Option<(u64, AgentEvent)>, JournalError> {
        self.flush(chat_id);
        last_valid_line(&self.path_for(chat_id))
    }

    /// Valid events newest-first, read backwards from the end of the file — for
    /// callers that want the most recent match and can stop early instead of
    /// materialising the whole journal like `replay` does.
    pub fn events_rev(
        &self,
        chat_id: &str,
    ) -> Result<impl Iterator<Item = Result<(u64, AgentEvent), JournalError>>, JournalError> {
        self.flush(chat_id);
        RevEvents::open(&self.path_for(chat_id))
    }

    /// One tool's call and (once it settled) its result, as the chat's
    /// journal last recorded them — what `ReadToolOutput` serves the whole
    /// output from. A subagent's tools are journaled under the chat that
    /// started it, wrapped in [`AgentEvent::Subagent`]; they are found the
    /// same way. Read newest-first, and only lines naming `tool_id` are
    /// parsed, so a long journal costs a byte scan, not a parse.
    /// `None` when the journal has no such call (an old chat whose journal
    /// is gone, or a chat this device never ran).
    pub fn tool_exchange(
        &self,
        chat_id: &str,
        tool_id: &str,
    ) -> Result<Option<ToolExchange>, JournalError> {
        self.flush(chat_id);
        let path = self.path_for(chat_id);
        let Some(mut lines) = RevLines::open(&path)? else {
            return Ok(None);
        };
        // The id as it is written in a line: a JSON string.
        let needle = serde_json::to_string(tool_id)?;
        let mut result: Option<ToolResultRecord> = None;
        while let Some(raw) = lines.next_line()? {
            if !contains(&raw, needle.as_bytes()) {
                continue;
            }
            let Ok(line) = std::str::from_utf8(&raw) else {
                continue;
            };
            let Some((_, mut event)) = parse_line(&path, line) else {
                continue;
            };
            while let AgentEvent::Subagent { event: inner, .. } = event {
                event = *inner;
            }
            match event {
                AgentEvent::ToolResult {
                    id,
                    output,
                    exit_code,
                    is_error,
                    ..
                } if id == tool_id && result.is_none() => {
                    result = Some(ToolResultRecord {
                        output,
                        exit_code,
                        is_error,
                    });
                }
                AgentEvent::ToolCall { id, call } if id == tool_id => {
                    return Ok(Some(ToolExchange { call, result }));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// Crash-recovery scan: chat ids whose journal's last event is NOT a `Done` — their
    /// runs died mid-stream and need recovery (stamp `aborted`, close the journal).
    pub fn stale_sessions(&self) -> Result<Vec<String>, JournalError> {
        self.flush_all();
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

    /// Remove a chat's journal file entirely (tests / future compaction).
    pub fn discard(&self, chat_id: &str) -> Result<(), JournalError> {
        self.lock().remove(chat_id);
        let path = self.path_for(chat_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// A tool's call and its result, from the journal ([`RunJournal::tool_exchange`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolExchange {
    /// The call as last recorded (a harness may refine it in place).
    pub call: ToolCall,
    /// `None` while the tool is still running.
    pub result: Option<ToolResultRecord>,
}

/// What a tool's [`AgentEvent::ToolResult`] carried.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultRecord {
    pub output: Option<String>,
    pub exit_code: Option<i32>,
    pub is_error: bool,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
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

    /// The lines on disk, read without the journal (whose reads write what
    /// waits first).
    fn on_disk(dir: &Path, chat: &str) -> Vec<u64> {
        read_lines(&dir.join(format!("{chat}.jsonl")))
            .unwrap()
            .into_iter()
            .map(|(seq, _)| seq)
            .collect()
    }

    /// A turn's usage comes after its `Done`, and then nothing: it reaches
    /// the file within a second or so, with no append or read to take it.
    #[test]
    fn an_event_left_waiting_is_written_without_another_append() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("chat-1", &text("a")).unwrap();
        journal.append("chat-1", &done()).unwrap();
        journal.append("chat-1", &text("usage")).unwrap();
        assert_eq!(on_disk(dir.path(), "chat-1"), vec![1, 2], "batched still");
        let deadline = std::time::Instant::now() + IDLE_FLUSH * 5;
        while on_disk(dir.path(), "chat-1") != vec![1, 2, 3] {
            assert!(
                std::time::Instant::now() < deadline,
                "the waiting event was never written"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Writes that fail keep what waits and retry it: once the file takes
    /// writes again, every event is there, in order, with no gap.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_is_retried_not_left_as_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("chat-1", &text("a")).unwrap();
        journal.flush("chat-1");
        let path = dir.path().join("chat-1.jsonl");
        // A handle that can't write: every write fails (EBADF).
        let writable = {
            let mut files = journal.lock();
            let entry = files.get_mut("chat-1").unwrap();
            std::mem::replace(&mut entry.file, File::open(&path).unwrap())
        };
        assert_eq!(journal.append("chat-1", &text("b")).unwrap(), 2);
        assert_eq!(
            journal.append("chat-1", &done()).unwrap(),
            3,
            "kept to retry"
        );
        journal.flush("chat-1");
        assert_eq!(on_disk(dir.path(), "chat-1"), vec![1]);
        journal.lock().get_mut("chat-1").unwrap().file = writable;
        assert_eq!(journal.append("chat-1", &text("c")).unwrap(), 4);
        journal.flush("chat-1");
        assert_eq!(on_disk(dir.path(), "chat-1"), vec![1, 2, 3, 4]);
    }

    /// Past [`MAX_PENDING`] what waits is dropped: reported as an error, and
    /// the journal goes on from a fresh line.
    #[cfg(unix)]
    #[test]
    fn a_write_failing_past_the_cap_reports_the_drop() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        journal.append("chat-1", &text("a")).unwrap();
        journal.flush("chat-1");
        let path = dir.path().join("chat-1.jsonl");
        let writable = {
            let mut files = journal.lock();
            let entry = files.get_mut("chat-1").unwrap();
            std::mem::replace(&mut entry.file, File::open(&path).unwrap())
        };
        let big = "x".repeat(1024 * 1024);
        let mut failed = false;
        for _ in 0..(MAX_PENDING / big.len() + 2) {
            failed |= journal.append("chat-1", &text(&big)).is_err();
        }
        assert!(failed, "the drop was not reported");
        journal.lock().get_mut("chat-1").unwrap().file = writable;
        let next = journal.append("chat-1", &done()).unwrap();
        journal.flush("chat-1");
        assert_eq!(on_disk(dir.path(), "chat-1").last(), Some(&next));
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
}
